//! Проекты: создание, импорт прошивки (zip, tar, tar.md5, rar, 7z, payload, super, lz4, raw),
//! рекурсивное нахождение разделов и их полная разборка в папки с файлами (ext4, erofs, boot, recovery).
//!
//! Структура проекта (`data/projects/<name>` рядом с исполняемым файлом):
//!   project.json  — метаданные
//!   input/        — исходники (payload.bin, super.img и т.п.)
//!   images/       — образы разделов (*.img, raw)
//!   extracted/    — разобранные разделы (папки с файлами)
//!   config/       — fs_config, симлинки, file_contexts
//!   out/          — результаты сборки

use crate::detect::detect;
use crate::util::{copy_progress, dir, es, hidden, long, spawn_task, Ctx};
use crate::{android_tools, ext4x, lp, sdat, sparse, tools};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

const SUBDIRS: [&str; 5] = ["input", "images", "extracted", "config", "out"];

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Meta {
    pub name: String,
    pub created: u64,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Serialize)]
pub struct Part {
    name: String,
    size: u64,
    kind: &'static str,
    inner: Option<&'static str>,
    extracted: bool,
}

#[derive(Serialize)]
pub struct Summary {
    meta: Meta,
    parts: usize,
}

#[derive(Serialize)]
pub struct Detail {
    meta: Meta,
    path: String,
    parts: Vec<Part>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImportOptions {
    #[serde(default)]
    pub delete_archive: bool,
    #[serde(default = "default_true")]
    pub delete_intermediate: bool,
    #[serde(default = "default_true")]
    pub auto_extract_parts: bool,
    #[serde(default)]
    pub delete_raw_imgs: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            delete_archive: false,
            delete_intermediate: true,
            auto_extract_parts: true,
            delete_raw_imgs: false,
        }
    }
}

fn projects() -> PathBuf {
    dir("projects")
}

fn valid(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 48 {
        return Err("имя: 1–48 символов".into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) || name.starts_with('.') {
        return Err("имя: только латиница, цифры и - _ . (для совместимости утилит)".into());
    }
    Ok(())
}

fn pdir(name: &str) -> Result<PathBuf, String> {
    valid(name)?;
    let d = projects().join(name);
    if !d.join("project.json").exists() {
        return Err(format!("проект «{name}» не найден"));
    }
    Ok(d)
}

fn read_meta(d: &Path) -> Option<Meta> {
    serde_json::from_str(&fs::read_to_string(d.join("project.json")).ok()?).ok()
}

fn write_meta(d: &Path, m: &Meta) -> Result<(), String> {
    fs::write(d.join("project.json"), serde_json::to_string_pretty(m).unwrap()).map_err(es)
}

fn images(d: &Path) -> Vec<PathBuf> {
    let mut v: Vec<_> = fs::read_dir(d.join("images"))
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map_or(false, |e| e.eq_ignore_ascii_case("img")))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[tauri::command]
pub fn projects_root() -> String {
    projects().to_string_lossy().into()
}

#[tauri::command]
pub fn list_projects() -> Vec<Summary> {
    let mut v: Vec<Summary> = fs::read_dir(projects())
        .map(|r| {
            r.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let meta = read_meta(&p)?;
                    Some(Summary { parts: images(&p).len(), meta })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.meta.created.cmp(&a.meta.created));
    v
}

#[tauri::command]
pub fn create_project(name: String) -> Result<(), String> {
    valid(&name)?;
    let d = projects().join(&name);
    if d.exists() {
        return Err("проект с таким именем уже есть".into());
    }
    for s in SUBDIRS {
        fs::create_dir_all(d.join(s)).map_err(es)?;
    }
    let created = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    write_meta(&d, &Meta { name, created, source: None })
}

#[tauri::command]
pub fn get_project(name: String) -> Result<Detail, String> {
    let d = pdir(&name)?;
    let meta = read_meta(&d).unwrap_or_default();
    let parts = images(&d)
        .into_iter()
        .map(|p| {
            let n = p.file_stem().unwrap().to_string_lossy().to_string();
            let det = detect(&p).ok();
            Part {
                size: p.metadata().map(|m| m.len()).unwrap_or(0),
                kind: det.as_ref().map_or("?", |x| x.kind),
                inner: det.and_then(|x| x.inner),
                extracted: d.join("extracted").join(&n).is_dir(),
                name: n,
            }
        })
        .collect();
    Ok(Detail { meta, path: d.to_string_lossy().into(), parts })
}

#[tauri::command]
pub fn delete_project(app: AppHandle, name: String) -> Result<(), String> {
    let d = pdir(&name)?;
    spawn_task(app, format!("Удаление проекта {name}"), move |_| {
        fs::remove_dir_all(long(&d)).map_err(es)?;
        Ok(format!("проект {name} удалён"))
    })
}

#[tauri::command]
pub fn open_in_explorer(name: String, sub: String) -> Result<(), String> {
    let mut p = pdir(&name)?;
    if !sub.is_empty() {
        if sub.contains("..") {
            return Err("bad path".into());
        }
        p = p.join(sub);
    }
    let mut c = Command::new("explorer");
    c.arg(&p);
    c.spawn().map(|_| ()).map_err(es)
}

// ---------------- Утилиты распаковки и сжатия ----------------

fn copy_file(ctx: &Ctx, src: &Path, dst: &Path) -> Result<(), String> {
    let total = src.metadata().map(|m| m.len()).unwrap_or(0);
    let mut r = File::open(src).map_err(es)?;
    let mut w = BufWriter::with_capacity(1 << 20, File::create(dst).map_err(es)?);
    copy_progress(ctx, &mut r, &mut w, total).map_err(es)?;
    w.flush().map_err(es)
}

fn run_payload(ctx: &Ctx, payload: &Path, out: &Path) -> Result<(), String> {
    let exe = tools::bin("payload-dumper-go", "payload-dumper-go.exe")?;
    ctx.stage("Распаковка payload.bin (A/B OTA)");
    crate::util::run_stream(
        ctx,
        &exe,
        &["-o".into(), out.to_string_lossy().into(), payload.to_string_lossy().into()],
        None,
    )
}

/// Распаковка любых архивов: zip, tar, tar.gz, tar.md5, rar, 7z через bsdtar (System32) с fallback
fn unpack_archive(ctx: &Ctx, archive: &Path, out_dir: &Path) -> Result<(), String> {
    let fname = archive.file_name().unwrap_or_default().to_string_lossy().to_string();
    ctx.stage(format!("Распаковка архива {fname}..."));
    fs::create_dir_all(out_dir).map_err(es)?;

    let tar_bin = if Path::new(r"C:\Windows\System32\tar.exe").is_file() {
        PathBuf::from(r"C:\Windows\System32\tar.exe")
    } else {
        PathBuf::from("tar.exe")
    };

    let mut cmd = Command::new(&tar_bin);
    cmd.args(["-xf", &archive.to_string_lossy(), "-C", &out_dir.to_string_lossy()]);
    
    match hidden(&mut cmd).output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(_) | Err(_) => {
            // Fallback для ZIP
            if fname.to_lowercase().ends_with(".zip") {
                return unpack_zip_fallback(ctx, archive, out_dir);
            }
            // Fallback для TAR / TAR.GZ
            if fname.to_lowercase().ends_with(".tar") || fname.to_lowercase().ends_with(".tar.md5") || fname.to_lowercase().ends_with(".tar.gz") || fname.to_lowercase().ends_with(".tgz") {
                return unpack_tar_fallback(ctx, archive, out_dir);
            }
            Err(format!("Не удалось распаковать архив {fname}. Убедитесь, что архив не повреждён."))
        }
    }
}

fn unpack_zip_fallback(ctx: &Ctx, zip_path: &Path, out_dir: &Path) -> Result<(), String> {
    let file = File::open(zip_path).map_err(es)?;
    let mut archive = zip::ZipArchive::new(file).map_err(es)?;
    let count = archive.len();

    for i in 0..count {
        let mut file = archive.by_index(i).map_err(es)?;
        let enclosed = match file.enclosed_name() {
            Some(p) => p,
            None => continue,
        };
        let out_path = out_dir.join(&enclosed);

        if file.is_dir() {
            fs::create_dir_all(&out_path).map_err(es)?;
        } else {
            if let Some(p) = out_path.parent() {
                fs::create_dir_all(p).map_err(es)?;
            }
            let mut outfile = File::create(&out_path).map_err(es)?;
            std::io::copy(&mut file, &mut outfile).map_err(es)?;
        }
        ctx.progress(i as u64, count as u64);
    }
    Ok(())
}

fn unpack_tar_fallback(_ctx: &Ctx, tar_path: &Path, out_dir: &Path) -> Result<(), String> {
    let file = File::open(tar_path).map_err(es)?;
    let fname = tar_path.to_string_lossy().to_lowercase();
    if fname.ends_with(".gz") || fname.ends_with(".tgz") {
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);
        archive.unpack(out_dir).map_err(es)?;
    } else {
        let mut archive = tar::Archive::new(file);
        archive.unpack(out_dir).map_err(es)?;
    }
    Ok(())
}

/// Распаковка LZ4 (например Samsung .img.lz4)
fn decompress_lz4(ctx: &Ctx, lz4_file: &Path, out_img: &Path) -> Result<(), String> {
    let fname = lz4_file.file_name().unwrap_or_default().to_string_lossy().to_string();
    ctx.stage(format!("Распаковка LZ4: {fname}..."));

    if let Ok(lz4_exe) = tools::bin("lz4", "lz4.exe") {
        let mut cmd = Command::new(&lz4_exe);
        cmd.args(["-d", "-f", &lz4_file.to_string_lossy(), &out_img.to_string_lossy()]);
        let out = hidden(&mut cmd).output().map_err(es)?;
        if out.status.success() {
            return Ok(());
        }
    }

    // Fallback: tar.exe с поддержкой lz4
    let tar_bin = PathBuf::from(r"C:\Windows\System32\tar.exe");
    let mut cmd = Command::new(&tar_bin);
    cmd.args(["-xf", &lz4_file.to_string_lossy(), "-O"]);
    if let Ok(out) = hidden(&mut cmd).output() {
        if out.status.success() && !out.stdout.is_empty() {
            fs::write(out_img, &out.stdout).map_err(es)?;
            return Ok(());
        }
    }

    Err(format!("Для распаковки {fname} требуется утилита LZ4. Установите её в разделе «Инструменты»."))
}

/// Распаковка Zstandard (.img.zst)
fn decompress_zstd(ctx: &Ctx, zst_file: &Path, out_img: &Path) -> Result<(), String> {
    let fname = zst_file.file_name().unwrap_or_default().to_string_lossy().to_string();
    ctx.stage(format!("Распаковка ZSTD: {fname}..."));

    let zstd_exe = tools::bin("zstd", "zstd.exe")?;
    let mut cmd = Command::new(&zstd_exe);
    cmd.args(["-d", "-f", &zst_file.to_string_lossy(), "-o", &out_img.to_string_lossy()]);
    let out = hidden(&mut cmd).output().map_err(es)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("Ошибка распаковки zstd: {}", String::from_utf8_lossy(&out.stderr)))
    }
}

/// Sparse → raw (на месте).
fn unsparse_in_place(ctx: &Ctx, img: &Path) -> Result<(), String> {
    let n = img.file_name().unwrap().to_string_lossy().to_string();
    ctx.stage(format!("sparse → raw: {n}"));
    let tmp = img.with_extension("img.raw");
    sparse::unsparse(ctx, img, &tmp).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("{n}: {e}")
    })?;
    fs::remove_file(img).map_err(es)?;
    fs::rename(&tmp, img).map_err(es)
}

/// Рекурсивный поиск файлов в директории
fn walk_dir_files(dir: &Path) -> Vec<PathBuf> {
    let mut res = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                res.extend(walk_dir_files(&p));
            } else if p.is_file() {
                res.push(p);
            }
        }
    }
    res
}

/// Рекурсивное нахождение всех разделов (в т.ч. сжатых) и сбор их в `images/`
fn scan_and_collect_images(ctx: &Ctx, scan_dir: &Path, images_dir: &Path, opts: &ImportOptions) -> Result<(), String> {
    let all_files = walk_dir_files(scan_dir);

    // 1. Поиск payload.bin
    for f in &all_files {
        if f.file_name().map_or(false, |n| n.eq_ignore_ascii_case("payload.bin")) {
            ctx.log(format!("  Обнаружен payload.bin: {}", f.display()));
            run_payload(ctx, f, images_dir)?;
            if opts.delete_intermediate {
                let _ = fs::remove_file(f);
            }
        }
    }

    // 2. Поиск Samsung LZ4 разделов (*.img.lz4)
    for f in &all_files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        if name.to_lowercase().ends_with(".img.lz4") || name.to_lowercase().ends_with(".lz4") {
            let stem = name.trim_end_matches(".lz4").trim_end_matches(".img").to_string();
            let out_img = images_dir.join(format!("{stem}.img"));
            ctx.log(format!("  Распаковка сжатого раздела: {name} → {stem}.img"));
            if let Err(e) = decompress_lz4(ctx, f, &out_img) {
                ctx.err(format!("  Ошибка распаковки LZ4 {name}: {e}"));
            } else if opts.delete_intermediate {
                let _ = fs::remove_file(f);
            }
        }
    }

    // 3. Поиск Zstandard разделов (*.img.zst)
    for f in &all_files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        if name.to_lowercase().ends_with(".img.zst") || name.to_lowercase().ends_with(".zst") || name.to_lowercase().ends_with(".zstd") {
            let stem = name.trim_end_matches(".zstd").trim_end_matches(".zst").trim_end_matches(".img").to_string();
            let out_img = images_dir.join(format!("{stem}.img"));
            ctx.log(format!("  Распаковка сжатого раздела ZSTD: {name} → {stem}.img"));
            if let Err(e) = decompress_zstd(ctx, f, &out_img) {
                ctx.err(format!("  Ошибка распаковки ZSTD {name}: {e}"));
            } else if opts.delete_intermediate {
                let _ = fs::remove_file(f);
            }
        }
    }

    // 4. Поиск AOSP sparse dat (*.new.dat + *.transfer.list)
    for f in &all_files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        if name.ends_with(".transfer.list") {
            let part_name = name.trim_end_matches(".transfer.list");
            let dat_candidate = f.with_file_name(format!("{part_name}.new.dat"));
            if dat_candidate.is_file() {
                let out_img = images_dir.join(format!("{part_name}.img"));
                ctx.stage(format!("Конвертация sdat2img: {part_name}.new.dat"));
                if let Err(e) = sdat::sdat2img(ctx, f, &dat_candidate, &out_img) {
                    ctx.err(format!("  Ошибка конвертации sdat2img: {e}"));
                } else if opts.delete_intermediate {
                    let _ = fs::remove_file(f);
                    let _ = fs::remove_file(&dat_candidate);
                }
            }
        }
    }

    // 5. Поиск стандартных *.img файлов
    for f in &all_files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        if name.to_lowercase().ends_with(".img") && !name.to_lowercase().ends_with(".lz4") && !name.to_lowercase().ends_with(".zst") {
            let dst = images_dir.join(&name);
            if !dst.exists() {
                let _ = fs::copy(f, &dst);
            }
        }
    }

    Ok(())
}

/// Развертывание sparse и super контейнеров в images/
fn post_process_images(ctx: &Ctx, d: &Path, opts: &ImportOptions) -> Result<(), String> {
    let images_dir = d.join("images");
    let input_dir = d.join("input");

    // 1-й проход: unsparse и распаковка super
    for img in images(d) {
        if detect(&img).map_err(es)?.kind == "sparse" {
            unsparse_in_place(ctx, &img)?;
        }
        if detect(&img).map_err(es)?.kind == "super" {
            ctx.stage("Распаковка dynamic partitions (super.img)");
            let parts = lp::unpack(ctx, &img, &images_dir)?;
            ctx.log(format!("  Из super извлечены разделы: {}", parts.join(", ")));
            if opts.delete_intermediate {
                let _ = fs::remove_file(&img);
            } else {
                let keep = input_dir.join(img.file_name().unwrap());
                let _ = fs::rename(&img, &keep);
            }
        }
    }

    // 2-й проход: проверка разделов, извлечённых из super (на случай если они sparse)
    for img in images(d) {
        if detect(&img).map_err(es)?.kind == "sparse" {
            unsparse_in_place(ctx, &img)?;
        }
    }

    Ok(())
}

/// Разборка отдельного раздела в папку extracted/{part}/
fn extract_single_partition_internal(ctx: &Ctx, d: &Path, part: &str, delete_raw_img: bool) -> Result<(), String> {
    let img = d.join("images").join(format!("{part}.img"));
    if !img.is_file() {
        return Err(format!("образ {part}.img не найден"));
    }

    if detect(&img).map_err(es)?.kind == "sparse" {
        unsparse_in_place(ctx, &img)?;
    }

    let kind = detect(&img).map_err(es)?.kind;
    let ext = d.join("extracted");
    let out = ext.join(part);

    if out.exists() {
        ctx.log(format!("  Очистка предыдущей разборки {part}..."));
        let _ = fs::remove_dir_all(long(&out));
    }

    match kind {
        "ext4" => {
            ctx.stage(format!("ext4 → extracted/{part}/"));
            ext4x::extract(ctx, &img, &out, &d.join("config"), part)?;
            ctx.log(format!("  Раздел {part} успешно разобран (ext4)"));
        }
        "erofs" => {
            let exe = tools::bin("erofs-utils", "extract.erofs.exe")?;
            ctx.stage(format!("erofs → extracted/{part}/"));
            crate::util::run_stream(
                ctx,
                &exe,
                &[
                    "-i".into(),
                    img.to_string_lossy().into(),
                    "-x".into(),
                    "-o".into(),
                    ext.to_string_lossy().into(),
                ],
                None,
            )?;
            let cfg = ext.join("config");
            if cfg.is_dir() {
                for e in fs::read_dir(&cfg).map_err(es)?.flatten() {
                    let _ = fs::rename(e.path(), d.join("config").join(e.file_name()));
                }
                let _ = fs::remove_dir_all(&cfg);
            }
            if !out.is_dir() {
                return Err("extract.erofs отработал, но папка раздела не появилась".into());
            }
            ctx.log(format!("  Раздел {part} успешно разобран (erofs)"));
        }
        "boot" | "vendor_boot" => {
            ctx.stage(format!("MagiskBoot → extracted/{part}/ (kernel, dtb, ramdisk)"));
            android_tools::extract_boot_to_dir(ctx, &img, &out)?;
            ctx.log(format!("  Загрузочный образ {part} успешно разобран на kernel, dtb и дерево ramdisk/"));
        }
        "super" => {
            ctx.stage("Распаковка super (lpmetadata)");
            let parts = lp::unpack(ctx, &img, &d.join("images"))?;
            let keep = d.join("input").join(format!("{part}.img"));
            let _ = fs::rename(&img, &keep);
            ctx.log(format!("  super распакован: {}", parts.join(", ")));
        }
        other => {
            ctx.log(format!("  Пропуск разборки {part}: тип «{other}» не является файловой системой или загрузочным ядром"));
            return Ok(());
        }
    }

    // Если включена опция удаления сырых .img после разборки в папки
    if delete_raw_img && out.is_dir() {
        ctx.log(format!("  Удаление сырого образа {part}.img для экономии места"));
        let _ = fs::remove_file(&img);
    }

    Ok(())
}

// ---------------- Команды Tauri ----------------

#[tauri::command]
pub fn import_firmware(app: AppHandle, name: String, path: String, options: Option<ImportOptions>) -> Result<(), String> {
    let d = pdir(&name)?;
    let src = PathBuf::from(&path);
    if !src.is_file() {
        return Err("файл не найден".into());
    }

    let opts = options.unwrap_or_default();

    spawn_task(app, format!("Импорт и разборка в {name}"), move |ctx| {
        let det = detect(&src).map_err(es)?;
        let fname = src.file_name().unwrap().to_string_lossy().to_string();
        ctx.log(format!("Файл: {fname} (определен как «{}»)", det.kind));

        let images_dir = d.join("images");
        fs::create_dir_all(&images_dir).map_err(es)?;

        match det.kind {
            "zip" | "tar" | "gzip" | "xz" | "bzip2" | "rar" | "7z" => {
                let temp_dir = d.join("input/unpacked");
                if temp_dir.exists() {
                    let _ = fs::remove_dir_all(&temp_dir);
                }
                unpack_archive(ctx, &src, &temp_dir)?;
                scan_and_collect_images(ctx, &temp_dir, &images_dir, &opts)?;
                // Удаляем временную папку распакованного архива
                let _ = fs::remove_dir_all(&temp_dir);

                if opts.delete_archive {
                    ctx.log(format!("Удаление исходного архива: {fname}"));
                    let _ = fs::remove_file(&src);
                }
            }
            "payload" => {
                run_payload(ctx, &src, &images_dir)?;
                if opts.delete_intermediate {
                    ctx.log(format!("Удаление промежуточного payload.bin: {fname}"));
                    let _ = fs::remove_file(&src);
                }
            }
            "lz4" => {
                let stem = fname.trim_end_matches(".lz4").trim_end_matches(".img").to_string();
                let out_img = images_dir.join(format!("{stem}.img"));
                decompress_lz4(ctx, &src, &out_img)?;
                if opts.delete_intermediate {
                    let _ = fs::remove_file(&src);
                }
            }
            "zstd" => {
                let stem = fname.trim_end_matches(".zstd").trim_end_matches(".zst").trim_end_matches(".img").to_string();
                let out_img = images_dir.join(format!("{stem}.img"));
                decompress_zstd(ctx, &src, &out_img)?;
                if opts.delete_intermediate {
                    let _ = fs::remove_file(&src);
                }
            }
            _ => {
                // Одиночный .img или сырой раздел
                let stem = src.file_stem().unwrap().to_string_lossy().to_string();
                let stem = stem.trim_end_matches(".img").to_string();
                ctx.stage(format!("Копирование {fname}"));
                copy_file(ctx, &src, &images_dir.join(format!("{stem}.img")))?;
            }
        }

        // Постобработка: sparse, super
        post_process_images(ctx, &d, &opts)?;

        // Автоматическая разборка всех разделов в папки (extracted/)
        if opts.auto_extract_parts {
            ctx.stage("Автоматическая разборка всех обнаруженных разделов в папки...");
            let all_images = images(&d);
            let total = all_images.len();
            for (idx, img_path) in all_images.into_iter().enumerate() {
                let part_name = img_path.file_stem().unwrap().to_string_lossy().to_string();
                ctx.stage(format!("Разборка раздела ({}/{}): {part_name}", idx + 1, total));
                let _ = extract_single_partition_internal(ctx, &d, &part_name, opts.delete_raw_imgs);
            }
        }

        let mut m = read_meta(&d).unwrap_or_default();
        m.source = Some(path);
        write_meta(&d, &m)?;

        crate::telegram::on_firmware_imported(&name, &fname, images(&d).len());

        Ok(format!(
            "Импорт и разборка завершены! Образов в проекте: {}, разобранных папок: {}",
            images(&d).len(),
            fs::read_dir(d.join("extracted")).map(|r| r.flatten().count()).unwrap_or(0)
        ))
    })
}

#[tauri::command]
pub fn extract_partition(app: AppHandle, name: String, part: String) -> Result<(), String> {
    let d = pdir(&name)?;
    spawn_task(app, format!("Разборка раздела {part}"), move |ctx| {
        extract_single_partition_internal(ctx, &d, &part, false)?;
        Ok(format!("Раздел {part} успешно разобран в extracted/{part}"))
    })
}

#[tauri::command]
pub fn extract_all_partitions(app: AppHandle, name: String, delete_raw_imgs: bool) -> Result<(), String> {
    let d = pdir(&name)?;
    spawn_task(app, format!("Разборка всех разделов {name}"), move |ctx| {
        let all_images = images(&d);
        if all_images.is_empty() {
            return Err("В проекте нет образов для разборки".into());
        }
        let total = all_images.len();
        for (idx, img_path) in all_images.into_iter().enumerate() {
            let part_name = img_path.file_stem().unwrap().to_string_lossy().to_string();
            ctx.stage(format!("Разборка ({}/{}): {part_name}", idx + 1, total));
            let _ = extract_single_partition_internal(ctx, &d, &part_name, delete_raw_imgs);
        }
        Ok(format!("Все разделы ({total}) обработаны!"))
    })
}

#[tauri::command]
pub fn unsparse_partition(app: AppHandle, name: String, part: String) -> Result<(), String> {
    let img = d_part_img(&pdir(&name)?, &part)?;
    spawn_task(app, format!("sparse → raw: {part}"), move |ctx| {
        unsparse_in_place(ctx, &img)?;
        Ok(format!("{part}.img преобразован в raw"))
    })
}

fn d_part_img(d: &Path, part: &str) -> Result<PathBuf, String> {
    if part.contains(['/', '\\']) || part.contains("..") {
        return Err("bad name".into());
    }
    let p = d.join("images").join(format!("{part}.img"));
    if !p.is_file() {
        return Err(format!("образ {part}.img не найден"));
    }
    Ok(p)
}
