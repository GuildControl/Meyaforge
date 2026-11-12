//! Модуль работы с boot.img, recovery.img, vendor_boot, ramdisk и APK.

use crate::tools;
use crate::util::{dir, es, run_stream, spawn_task};
use std::fs;
use std::path::PathBuf;
use tauri::AppHandle;

fn project_dir(name: &str) -> Result<PathBuf, String> {
    let p = dir("projects").join(name);
    if !p.exists() {
        return Err(format!("Проект «{name}» не существует"));
    }
    Ok(p)
}

// ---------------- BOOT / RECOVERY ----------------

pub fn extract_boot_to_dir(ctx: &crate::util::Ctx, img: &std::path::Path, out_dir: &std::path::Path) -> Result<(), String> {
    let exe = tools::bin("magiskboot", "magiskboot.exe")?;
    fs::create_dir_all(out_dir).map_err(es)?;

    // Копируем образ в рабочую папку boot'а, так как magiskboot работает в cwd
    let work_img = out_dir.join("boot.img");
    fs::copy(img, &work_img).map_err(es)?;

    ctx.stage(format!("MagiskBoot: unpack {}", img.display()));
    run_stream(ctx, &exe, &["unpack".into(), "-h".into(), "boot.img".into()], Some(out_dir))?;

    // Если распаковался ramdisk.cpio, пробуем распаковать его
    let ramdisk = out_dir.join("ramdisk.cpio");
    if ramdisk.is_file() {
        ctx.stage("Распаковка ramdisk.cpio");
        let ramdisk_dir = out_dir.join("ramdisk");
        fs::create_dir_all(&ramdisk_dir).map_err(es)?;
        let _ = run_stream(ctx, &exe, &["cpio".into(), "ramdisk.cpio".into(), "extract".into()], Some(out_dir));
    }
    Ok(())
}

#[tauri::command]
pub fn unpack_boot(app: AppHandle, project: String, image_path: String) -> Result<(), String> {
    let pdir = project_dir(&project)?;
    let img = PathBuf::from(&image_path);
    if !img.is_file() {
        return Err("Файл boot-образа не найден".into());
    }

    let stem = img.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let out_dir = pdir.join("extracted").join(&stem);

    spawn_task(app, format!("Распаковка {stem}"), move |ctx| {
        extract_boot_to_dir(ctx, &img, &out_dir)?;
        Ok(format!("{stem} успешно распакован в extracted/{stem}"))
    })
}

#[tauri::command]
pub fn repack_boot(app: AppHandle, project: String, target_name: String) -> Result<(), String> {
    let pdir = project_dir(&project)?;
    let work_dir = pdir.join("extracted").join(&target_name);
    if !work_dir.is_dir() {
        return Err(format!("Папка extracted/{target_name} не найдена"));
    }

    let out_dir = pdir.join("out");

    spawn_task(app, format!("Сборка {target_name}"), move |ctx| {
        crate::telegram::on_build_start(&target_name);
        let exe = match tools::bin("magiskboot", "magiskboot.exe") {
            Ok(e) => e,
            Err(err) => {
                crate::telegram::on_build_fail(&target_name, "Поиск MagiskBoot", &err);
                return Err(err);
            }
        };
        fs::create_dir_all(&out_dir).map_err(es)?;

        let work_img = work_dir.join("boot.img");
        if !work_img.is_file() {
            let err = "Оригинальный boot.img не найден в рабочей папке".to_string();
            crate::telegram::on_build_fail(&target_name, "Проверка исходных файлов", &err);
            return Err(err);
        }

        ctx.stage(format!("MagiskBoot: repack {}", target_name));
        crate::telegram::on_build_progress(&target_name, "Сборка нового ядра через MagiskBoot", 50);

        if let Err(e) = run_stream(ctx, &exe, &["repack".into(), "boot.img".into(), "new-boot.img".into()], Some(&work_dir)) {
            crate::telegram::on_build_fail(&target_name, "Выполнение MagiskBoot repack", &e);
            return Err(e);
        }

        let built_img = work_dir.join("new-boot.img");
        if !built_img.is_file() {
            let err = "Ошибка: new-boot.img не был создан".to_string();
            crate::telegram::on_build_fail(&target_name, "Проверка собранного образа", &err);
            return Err(err);
        }

        let dst_name = format!("{target_name}-repacked.img");
        let dst = out_dir.join(&dst_name);
        fs::rename(&built_img, &dst).map_err(es)?;

        let size_str = dst.metadata().map(|m| {
            let mb = m.len() as f64 / (1024.0 * 1024.0);
            format!("{:.1} МБ", mb)
        }).unwrap_or_else(|_| "—".into());

        crate::telegram::on_build_done(&target_name, &dst_name, &size_str, "", "", "");

        Ok(format!("{target_name} собран: out/{dst_name}"))
    })
}

// ---------------- APK MODDING ----------------

fn java_exe() -> Result<PathBuf, String> {
    // Проверяем системный java
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

#[tauri::command]
pub fn decompile_apk(app: AppHandle, project: String, apk_path: String) -> Result<(), String> {
    let pdir = project_dir(&project)?;
    let apk = PathBuf::from(&apk_path);
    if !apk.is_file() {
        return Err("APK файл не найден".into());
    }

    let java = java_exe()?;
    let stem = apk.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let out_dir = pdir.join("extracted").join(format!("apk_{stem}"));

    spawn_task(app, format!("Декомпиляция APK {stem}"), move |ctx| {
        let apktool_jar = tools::bin("apktool", "apktool.jar")?;

        ctx.stage(format!("Apktool: decode {}", stem));
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
        Ok(format!("APK декомпилирован в extracted/apk_{stem}"))
    })
}

#[tauri::command]
pub fn build_apk(app: AppHandle, project: String, target_folder: String, sign: bool) -> Result<(), String> {
    let pdir = project_dir(&project)?;
    let src_dir = pdir.join("extracted").join(&target_folder);
    if !src_dir.is_dir() {
        return Err(format!("Папка {target_folder} не найдена в extracted"));
    }

    let java = java_exe()?;
    let out_dir = pdir.join("out");

    spawn_task(app, format!("Сборка APK {target_folder}"), move |ctx| {
        let apktool_jar = tools::bin("apktool", "apktool.jar")?;
        fs::create_dir_all(&out_dir).map_err(es)?;

        let unsigned_apk = out_dir.join(format!("{target_folder}-unsigned.apk"));

        ctx.stage(format!("Apktool: build {}", target_folder));
        let build_args = vec![
            "-jar".to_string(),
            apktool_jar.to_string_lossy().to_string(),
            "b".to_string(),
            src_dir.to_string_lossy().to_string(),
            "-o".to_string(),
            unsigned_apk.to_string_lossy().to_string(),
        ];

        run_stream(ctx, &java, &build_args, None)?;

        if sign {
            let signer_jar = tools::bin("uber-apk-signer", "uber-apk-signer.jar")?;
            ctx.stage("Подпись APK (Uber Apk Signer)");
            let sign_args = vec![
                "-jar".to_string(),
                signer_jar.to_string_lossy().to_string(),
                "-a".to_string(),
                unsigned_apk.to_string_lossy().to_string(),
                "-o".to_string(),
                out_dir.to_string_lossy().to_string(),
                "--allowResign".to_string(),
            ];
            run_stream(ctx, &java, &sign_args, None)?;
            Ok(format!("APK собран и подписан в out/"))
        } else {
            Ok(format!("APK собран: out/{target_folder}-unsigned.apk"))
        }
    })
}
