//! APK Kitchen: сканирование APK в прошивке, декомпиляция, деодекс (baksmali/smali), сборка и подпись.

use crate::tools;
use crate::util::{dir, es, run_stream, spawn_task};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

fn project_dir(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
        return Err("некорректное имя проекта".into());
    }
    let p = dir("projects").join(name);
    if !p.is_dir() {
        return Err(format!("Проект «{name}» не существует"));
    }
    Ok(p)
}

fn java_exe() -> Result<PathBuf, String> {
    if let Ok(out) = std::process::Command::new("where.exe").arg("java").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(first) = s.lines().next() {
                let p = PathBuf::from(first.trim());
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
    }
    Err("Java Runtime (JRE/JDK) не найден в PATH. Установите Java для работы с APK.".into())
}

/// Безопасно превращает относительный путь проекта в абсолютный (без выхода за пределы проекта).
fn safe_rel(pdir: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_clean = rel.replace('/', "\\");
    if rel_clean.contains("..") || rel_clean.starts_with('\\') || rel_clean.contains(':') {
        return Err("Доступ за пределы каталога проекта запрещён".into());
    }
    let p = pdir.join(&rel_clean);
    Ok(p)
}

// ---------------- Сканирование ----------------

#[derive(Serialize)]
pub struct ApkEntry {
    pub name: String,
    pub rel_path: String,
    pub size: u64,
    pub has_odex: bool,
    pub odex_count: usize,
}

fn walk_apks(root: &Path, base: &Path, out: &mut Vec<ApkEntry>, depth: usize) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            // Пропускаем папки декомпилированных приложений и служебные каталоги
            if name.starts_with("apk_") || name == "removed_bloat" {
                continue;
            }
            walk_apks(&p, base, out, depth + 1);
        } else if name.to_lowercase().ends_with(".apk") {
            let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let odexes = find_odex_files(&p);
            out.push(ApkEntry {
                name,
                rel_path: rel,
                size: p.metadata().map(|m| m.len()).unwrap_or(0),
                has_odex: !odexes.is_empty(),
                odex_count: odexes.len(),
            });
        }
    }
}

/// Ищет рядом с APK odex-файлы: <stem>.odex, oat/arm|arm64/<stem>.odex
fn odex_files_for(apk: &Path) -> Vec<PathBuf> {
    find_odex_files(apk)
}

fn find_odex_files(apk: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(parent) = apk.parent() else { return out };
    let stem = apk.file_stem().unwrap_or_default().to_string_lossy().to_string();

    let sibling = parent.join(format!("{stem}.odex"));
    if sibling.is_file() {
        out.push(sibling);
    }
    let oat = parent.join("oat");
    if oat.is_dir() {
        for arch in ["arm64", "arm", "x86_64", "x86"] {
            let f = oat.join(arch).join(format!("{stem}.odex"));
            if f.is_file() {
                out.push(f);
            }
        }
    }
    out
}

#[tauri::command]
pub fn apk_scan_project(project: String) -> Result<Vec<ApkEntry>, String> {
    let pdir = project_dir(&project)?;
    let mut out = Vec::new();
    walk_apks(&pdir.join("extracted"), &pdir, &mut out, 0);
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

// ---------------- Декомпилированные ----------------

#[derive(Serialize)]
pub struct DecompiledEntry {
    pub folder: String,    // относительно проекта: extracted/apk_X
    pub name: String,      // apk_X
    pub has_smali: bool,
    pub has_resources: bool,
    pub file_count: usize,
}

fn count_files(root: &Path, depth: usize) -> usize {
    if depth > 14 {
        return 0;
    }
    let mut n = 0;
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += count_files(&p, depth + 1);
            } else {
                n += 1;
            }
        }
    }
    n
}

#[tauri::command]
pub fn apk_list_decompiled(project: String) -> Result<Vec<DecompiledEntry>, String> {
    let pdir = project_dir(&project)?;
    let ext = pdir.join("extracted");
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&ext) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if !p.is_dir() || !name.starts_with("apk_") {
                continue;
            }
            let has_smali = p.join("smali").is_dir() || p.join("smali_classes2").is_dir();
            let has_resources = p.join("res").is_dir() || p.join("resources.arsc").is_file();
            out.push(DecompiledEntry {
                folder: format!("extracted/{name}"),
                name,
                has_smali,
                has_resources,
                file_count: count_files(&p, 0),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

// ---------------- Декомпиляция APK из проекта ----------------

#[tauri::command]
pub fn apk_decompile_project(app: AppHandle, project: String, rel_path: String) -> Result<(), String> {
    let pdir = project_dir(&project)?;
    let apk = safe_rel(&pdir, &rel_path)?;
    if !apk.is_file() {
        return Err("APK файл не найден".into());
    }
    let java = java_exe()?;
    let stem = apk.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let out_dir = pdir.join("extracted").join(format!("apk_{stem}"));

    spawn_task(app, format!("Декомпиляция {stem}.apk"), move |ctx| {
        let apktool_jar = tools::bin("apktool", "apktool.jar")?;
        ctx.stage(format!("Apktool: decode {stem}"));
        let args = vec![
            "-jar".to_string(),
            apktool_jar.to_string_lossy().to_string(),
            "d".to_string(),
            apk.to_string_lossy().to_string(),
            "-o".to_string(),
            out_dir.to_string_lossy().to_string(),
            "-f".to_string(),
        ];
        run_stream(ctx, &java, &args, None)?;
        if !out_dir.is_dir() {
            return Err("Apktool отработал, но папка результата не появилась".into());
        }
        Ok(format!("{stem}.apk декомпилирован в extracted/apk_{stem}"))
    })
}

// ---------------- Деодекс ----------------

/// Собирает bootclasspath из framework-jar'ов проекта.
fn collect_framework_jars(pdir: &Path) -> Vec<PathBuf> {
    let mut jars = Vec::new();
    let ext = pdir.join("extracted");
    let mut stack = vec![ext];
    let mut depth_guard = 0usize;
    while let Some(d) = stack.pop() {
        depth_guard += 1;
        if depth_guard > 4000 {
            break;
        }
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if p.is_dir() {
                if name == "framework" {
                    if let Ok(frd) = fs::read_dir(&p) {
                        for fe in frd.flatten() {
                            let fp = fe.path();
                            if fp.extension().map(|x| x == "jar").unwrap_or(false) {
                                jars.push(fp);
                            }
                        }
                    }
                } else {
                    stack.push(p);
                }
            }
        }
    }
    jars.sort();
    jars.dedup();
    jars
}

#[tauri::command]
pub fn apk_deodex(app: AppHandle, project: String, rel_path: String) -> Result<(), String> {
    let pdir = project_dir(&project)?;
    let apk = safe_rel(&pdir, &rel_path)?;
    if !apk.is_file() {
        return Err("APK файл не найден".into());
    }
    let odexes = odex_files_for(&apk);
    if odexes.is_empty() {
        return Err("Рядом с APK не найдены .odex файлы (проверены <имя>.odex и oat/<arch>/<имя>.odex)".into());
    }
    let java = java_exe()?;
    let stem = apk.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let out_dir = pdir.join("out");

    spawn_task(app, format!("Деодекс {stem}.apk"), move |ctx| {
        let baksmali = tools::bin("baksmali", "baksmali.jar")
            .map_err(|_| "baksmali не найден. Установите «baksmali/smali» в разделе «Инструменты»".to_string())?;
        let smali = tools::bin("smali", "smali.jar")
            .map_err(|_| "smali не найден. Установите «baksmali/smali» в разделе «Инструменты»".to_string())?;

        let work = dir("tmp").join(format!("deodex_{stem}"));
        if work.exists() {
            let _ = fs::remove_dir_all(&work);
        }
        fs::create_dir_all(&work).map_err(es)?;

        let jars = collect_framework_jars(&pdir);
        let classpath = jars.iter().map(|j| j.to_string_lossy().to_string()).collect::<Vec<_>>().join(";");
        if jars.is_empty() {
            ctx.err("  внимание: framework jar'ы не найдены в проекте — деодекс может не сработать");
        } else {
            ctx.log(format!("  bootclasspath: {} framework jar(ов)", jars.len()));
        }

        // 1. odex -> smali, 2. smali -> classesN.dex
        let mut dex_files: Vec<PathBuf> = Vec::new();
        for (i, odex) in odexes.iter().enumerate() {
            let smali_dir = work.join(format!("smali_{}", i + 1));
            ctx.stage(format!("baksmali: разборка {}", odex.file_name().unwrap_or_default().to_string_lossy()));
            let mut d_args = vec![
                "-jar".to_string(),
                baksmali.to_string_lossy().to_string(),
                "d".to_string(),
                odex.to_string_lossy().to_string(),
                "-o".to_string(),
                smali_dir.to_string_lossy().to_string(),
            ];
            if !classpath.is_empty() {
                d_args.push("-c".to_string());
                d_args.push(classpath.clone());
            }
            run_stream(ctx, &java, &d_args, None)?;
            if !smali_dir.is_dir() {
                return Err(format!("baksmali не создал smali для {}", odex.display()));
            }

            let dex_name = if i == 0 { "classes.dex".to_string() } else { format!("classes{}.dex", i + 1) };
            let dex_out = work.join(&dex_name);
            ctx.stage(format!("smali: сборка {dex_name}"));
            let a_args = vec![
                "-jar".to_string(),
                smali.to_string_lossy().to_string(),
                "a".to_string(),
                smali_dir.to_string_lossy().to_string(),
                "-o".to_string(),
                dex_out.to_string_lossy().to_string(),
            ];
            run_stream(ctx, &java, &a_args, None)?;
            if !dex_out.is_file() {
                return Err(format!("smali не создал {dex_name}"));
            }
            dex_files.push(dex_out);
        }

        // 3. Перепаковываем APK: копируем все записи, кроме старых classes*.dex, добавляем новые
        ctx.stage("Упаковка classes.dex в APK");
        fs::create_dir_all(&out_dir).map_err(es)?;
        let out_apk = out_dir.join(format!("{stem}-deodexed.apk"));

        let src_file = fs::File::open(&apk).map_err(es)?;
        let mut zin = zip::ZipArchive::new(src_file).map_err(|e| format!("не удалось прочитать APK: {e}"))?;
        let dst_file = fs::File::create(&out_apk).map_err(es)?;
        let mut zout = zip::ZipWriter::new(dst_file);

        for i in 0..zin.len() {
            let entry = zin.by_index_raw(i).map_err(|e| e.to_string())?;
            let ename = entry.name().map_err(|e| e.to_string())?.to_string();
            let is_dex = ename.starts_with("classes") && ename.ends_with(".dex") && !ename.contains('/');
            if is_dex {
                continue;
            }
            zout.raw_copy_file(entry).map_err(|e| format!("ошибка копирования записи {ename}: {e}"))?;
        }
        for dex in &dex_files {
            let dname = dex.file_name().unwrap_or_default().to_string_lossy().to_string();
            zout.start_file(&dname, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).map_err(|e| e.to_string())?;
            let data = fs::read(dex).map_err(es)?;
            std::io::Write::write_all(&mut zout, &data).map_err(es)?;
        }
        zout.finish().map_err(|e| e.to_string())?;
        let _ = fs::remove_dir_all(&work);

        Ok(format!("Деодекс завершен: out/{} (dex файлов: {})", format!("{stem}-deodexed.apk"), dex_files.len()))
    })
}
