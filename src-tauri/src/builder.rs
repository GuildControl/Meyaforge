//! Модуль упаковки и сборки прошивок:
//!   1. Сборка разделов из extracted/<part>/ в images/<part>.img (EROFS, EXT4, MagiskBoot).
//!   2. Сборка супер-образа super.img через lpmake.
//!   3. Генерация готовых пакетов прошивки (Fastboot ROM Flasher, Recovery ZIP).
//!   4. Полная интеграция с живым Telegram-прогрессом.

use crate::tools;
use crate::util::{dir, es, run_stream, spawn_task};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

fn pdir(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
        return Err("некорректное имя проекта".into());
    }
    let p = dir("projects").join(name);
    if !p.is_dir() {
        return Err(format!("Проект «{name}» не найден"));
    }
    Ok(p)
}

#[derive(Serialize)]
pub struct ExtractedPartInfo {
    pub name: String,
    pub path: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub detected_fs: String,
    pub has_fs_config: bool,
}

#[derive(Serialize)]
pub struct ImagePartInfo {
    pub name: String,
    pub file_name: String,
    pub size: u64,
    pub is_sparse: bool,
}

#[derive(Serialize)]
pub struct BuildTargetsInfo {
    pub extracted: Vec<ExtractedPartInfo>,
    pub images: Vec<ImagePartInfo>,
    pub super_ready: bool,
    pub out_files: Vec<String>,
}

fn count_dir_stats(path: &Path) -> (usize, u64) {
    let mut count = 0;
    let mut bytes = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                count += 1;
                bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if p.is_dir() {
                let (c, b) = count_dir_stats(&p);
                count += c;
                bytes += b;
            }
        }
    }
    (count, bytes)
}

/// Получение информации о целях сборки в проекте
#[tauri::command]
pub fn get_build_targets(project: String) -> Result<BuildTargetsInfo, String> {
    let d = pdir(&project)?;
    let ext_dir = d.join("extracted");
    let img_dir = d.join("images");
    let out_dir = d.join("out");
    let cfg_dir = d.join("config");

    let mut extracted = Vec::new();
    if ext_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&ext_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let (file_count, total_bytes) = count_dir_stats(&p);
                    let has_fs_config = cfg_dir.join(format!("{name}_fs_config")).is_file();

                    let detected_fs = if p.join("boot.img").is_file() || p.join("ramdisk.cpio").is_file() || name == "boot" || name == "recovery" || name == "vendor_boot" {
                        "boot".to_string()
                    } else if has_fs_config {
                        "erofs".to_string()
                    } else {
                        "erofs".to_string()
                    };

                    extracted.push(ExtractedPartInfo {
                        name,
                        path: p.to_string_lossy().to_string(),
                        file_count,
                        total_bytes,
                        detected_fs,
                        has_fs_config,
                    });
                }
            }
        }
    }

    let mut images = Vec::new();
    if img_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&img_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file() && p.extension().map(|x| x == "img").unwrap_or(false) {
                    let fname = e.file_name().to_string_lossy().to_string();
                    let name = fname.trim_end_matches(".img").to_string();
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    let is_sparse = crate::sparse::is_sparse(&p);
                    images.push(ImagePartInfo {
                        name,
                        file_name: fname,
                        size,
                        is_sparse,
                    });
                }
            }
        }
    }

    let mut out_files = Vec::new();
    if out_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&out_dir) {
            for e in entries.flatten() {
                out_files.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }

    // Готовы ли разделы для сборки super.img (наличие system, vendor или product)
    let super_ready = images.iter().any(|i| i.name == "system" || i.name == "vendor" || i.name == "product");

    Ok(BuildTargetsInfo {
        extracted,
        images,
        super_ready,
        out_files,
    })
}

/// Сборка раздела из папки extracted/<part> в images/<part>.img
#[tauri::command]
pub fn repack_partition(
    app: AppHandle,
    project: String,
    part: String,
    fs_type: String,
    comp_algorithm: Option<String>,
) -> Result<(), String> {
    let d = pdir(&project)?;
    let src_dir = d.join("extracted").join(&part);
    if !src_dir.is_dir() {
        return Err(format!("Папка раздела extracted/{part} не найдена"));
    }

    // Результаты сборки всегда складываем в out/ проекта
    let out_dir = d.join("out");
    fs::create_dir_all(&out_dir).map_err(es)?;
    let out_img = out_dir.join(format!("{part}.img"));

    let task_title = format!("Сборка раздела {part} ({fs_type})");

    spawn_task(app, task_title, move |ctx| {
        crate::telegram::on_build_start(&format!("Раздел {part}"));

        match fs_type.to_lowercase().as_str() {
            "boot" | "recovery" | "vendor_boot" => {
                ctx.stage(format!("Сборка загрузочного образа {part} через MagiskBoot"));
                crate::telegram::on_build_progress(&format!("Раздел {part}"), "Сборка boot/recovery через MagiskBoot", 40);

                let exe = tools::bin("magiskboot", "magiskboot.exe")?;
                let work_img = src_dir.join("boot.img");
                if !work_img.is_file() {
                    let err = "Оригинальный boot.img не найден в рабочей папке extracted/".to_string();
                    crate::telegram::on_build_fail(&format!("Раздел {part}"), "Проверка исходных файлов", &err);
                    return Err(err);
                }

                run_stream(ctx, &exe, &["repack".into(), "boot.img".into(), "new-boot.img".into()], Some(&src_dir))?;

                let new_boot = src_dir.join("new-boot.img");
                if !new_boot.is_file() {
                    let err = "new-boot.img не был создан".to_string();
                    crate::telegram::on_build_fail(&format!("Раздел {part}"), "Генерация образа", &err);
                    return Err(err);
                }

                fs::copy(&new_boot, &out_img).map_err(es)?;
                let _ = fs::remove_file(&new_boot);
            }
            "erofs" => {
                ctx.stage(format!("Сборка {part}.img (EROFS)"));
                crate::telegram::on_build_progress(&format!("Раздел {part}"), "Сборка EROFS через mkfs.erofs", 30);

                let exe = tools::bin("erofs-utils", "mkfs.erofs.exe")?;
                let comp = comp_algorithm.as_deref().unwrap_or("lz4hc,9");

                let mut args: Vec<String> = vec![
                    "-z".into(),
                    comp.into(),
                    "-C".into(),
                    "65536".into(),
                    format!("--mount-point=/{part}"),
                ];

                let fs_config = d.join("config").join(format!("{part}_fs_config"));
                if fs_config.is_file() {
                    args.push(format!("--fs-config-file={}", fs_config.to_string_lossy()));
                }

                let file_contexts = d.join("config").join(format!("{part}_file_contexts"));
                if file_contexts.is_file() {
                    args.push(format!("--file-contexts={}", file_contexts.to_string_lossy()));
                }

                // Временный выходной файл, чтобы не повреждать старый при ошибке
                let tmp_out = d.join("out").join(format!("{part}.tmp.img"));
                if tmp_out.is_file() {
                    let _ = fs::remove_file(&tmp_out);
                }

                args.push(tmp_out.to_string_lossy().to_string());
                args.push(src_dir.to_string_lossy().to_string());

                ctx.log(format!("Запуск: mkfs.erofs {}", args.join(" ")));
                if let Err(e) = run_stream(ctx, &exe, &args, None) {
                    let _ = fs::remove_file(&tmp_out);
                    crate::telegram::on_build_fail(&format!("Раздел {part}"), "Выполнение mkfs.erofs", &e);
                    return Err(e);
                }

                if !tmp_out.is_file() {
                    let err = "mkfs.erofs завершился, но выходной файл не создан".to_string();
                    crate::telegram::on_build_fail(&format!("Раздел {part}"), "Проверка файла", &err);
                    return Err(err);
                }

                fs::rename(&tmp_out, &out_img).map_err(es)?;
            }
            "ext4" => {
                ctx.stage(format!("Сборка {part}.img (EXT4)"));
                crate::telegram::on_build_progress(&format!("Раздел {part}"), "Сборка EXT4 раздела", 30);

                // Если есть WSL, собираем через WSL mke2fs
                if crate::wsl_status() {
                    ctx.log("Используется WSL для сборки ext4");
                    let wsl_path = |path: &Path| -> Result<String, String> {
                        let mut command = std::process::Command::new("wsl");
                        command.args(["--exec", "wslpath", "-a", "-u"]).arg(path);
                        let output = crate::util::hidden(&mut command).output().map_err(es)?;
                        let converted = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !output.status.success() || converted.is_empty() {
                            return Err(format!(
                                "WSL не смог преобразовать путь {}: {}",
                                path.display(),
                                String::from_utf8_lossy(&output.stderr).trim()
                            ));
                        }
                        Ok(converted)
                    };
                    let wsl_src = wsl_path(&src_dir)?;
                    let wsl_out = wsl_path(&out_img)?;

                    let mut cmd = std::process::Command::new("wsl");
                    cmd.args(["--exec", "mke2fs", "-t", "ext4", "-b", "4096", "-d", &wsl_src, &wsl_out]);
                    crate::util::hidden(&mut cmd);
                    let status = cmd.status().map_err(es)?;
                    if !status.success() {
                        let err = "Ошибка выполнения mke2fs в WSL".to_string();
                        crate::telegram::on_build_fail(&format!("Раздел {part}"), "WSL mke2fs", &err);
                        return Err(err);
                    }
                } else {
                    return Err("Для сборки EXT4 требуется установленный WSL (кнопка в Настройках). Современные Android-прошивки используют файловую систему EROFS — выберите EROFS для мгновенной сборки без WSL.".into());
                }
            }
            other => {
                let err = format!("Неподдерживаемый тип файловой системы: {other}");
                crate::telegram::on_build_fail(&format!("Раздел {part}"), "Параметры сборки", &err);
                return Err(err);
            }
        }

        let size_bytes = out_img.metadata().map(|m| m.len()).unwrap_or(0);
        let size_str = format!("{:.1} МБ", size_bytes as f64 / (1024.0 * 1024.0));
        let fname = format!("{part}.img");

        crate::telegram::on_build_done(&format!("Раздел {part}"), &fname, &size_str, "", "", "");

        Ok(format!("Раздел {part} успешно собран: out/{fname} ({size_str})"))
    })
}

/// Сборка супер-образа super.img из динамических разделов через lpmake
#[tauri::command]
pub fn repack_super(
    app: AppHandle,
    project: String,
    partitions: Vec<String>,
    sparse: bool,
    custom_size: Option<u64>,
) -> Result<(), String> {
    let d = pdir(&project)?;
    let img_dir = d.join("images");
    let out_dir = d.join("out");
    fs::create_dir_all(&out_dir).map_err(es)?;

    if partitions.is_empty() {
        return Err("Не выбрано ни одного раздела для включения в super.img".into());
    }

    let out_super = out_dir.join("super.img");

    spawn_task(app, format!("Сборка super.img для {project}"), move |ctx| {
        crate::telegram::on_build_start(&format!("Super.img ({project})"));
        crate::telegram::on_build_progress(&format!("Super.img ({project})"), "Подготовка разделов и вычисление размеров", 15);

        let lpmake_exe = tools::bin("partition-tools", "lpmake.exe")?;
        let lpmake_dir = lpmake_exe.parent().unwrap_or(Path::new("."));

        ctx.stage("Анализ разделов для упаковки в super.img...");
        let mut total_part_size: u64 = 0;
        let mut part_specs = Vec::new();

        for part in &partitions {
            // Приоритет свежей сборке из out/, иначе исходный образ из images/
            let p_out = out_dir.join(format!("{part}.img"));
            let p_img = if p_out.is_file() {
                ctx.log(format!("  Раздел {part}: используется свежая сборка из out/"));
                p_out
            } else {
                img_dir.join(format!("{part}.img"))
            };
            if !p_img.is_file() {
                let err = format!("Образ раздела {part}.img не найден ни в out/, ни в images/");
                crate::telegram::on_build_fail(&format!("Super.img ({project})"), "Поиск образов", &err);
                return Err(err);
            }

            // Выравнивание размера раздела до 4096 байт
            let len = p_img.metadata().map_err(es)?.len();
            let aligned = (len + 4095) & !4095;
            total_part_size += aligned;

            ctx.log(format!("  Раздел {part}: {:.1} МБ (выровнен: {} б)", len as f64 / 1048576.0, aligned));
            part_specs.push((part.clone(), aligned, p_img));
        }

        // Вычисляем размер супер-блока: если не задан вручную, берем сумму + 8 МБ резерва, выравнивая по 4096
        let device_size = match custom_size {
            Some(s) if s > total_part_size => (s + 4095) & !4095,
            _ => (total_part_size + 8 * 1024 * 1024 + 4095) & !4095,
        };

        ctx.log(format!("Итоговый размер супер-раздела: {:.2} МБ ({} байт)", device_size as f64 / 1048576.0, device_size));
        crate::telegram::on_build_progress(&format!("Super.img ({project})"), "Генерация таблицы динамических разделов (lpmake)", 45);

        let mut args: Vec<String> = vec![
            "--metadata-size".into(),
            "65536".into(),
            "--super-name".into(),
            "super".into(),
            "--metadata-slots".into(),
            "2".into(),
            "--device".into(),
            format!("super:{device_size}"),
            "--group".into(),
            format!("main:{device_size}"),
        ];

        for (part_name, aligned_sz, img_path) in &part_specs {
            args.push("--partition".into());
            args.push(format!("{part_name}:readonly:{aligned_sz}:main"));
            args.push("--image".into());
            args.push(format!("{part_name}={}", img_path.to_string_lossy()));
        }

        if sparse {
            args.push("--sparse".into());
        }

        let tmp_super = out_dir.join("super.tmp.img");
        if tmp_super.is_file() {
            let _ = fs::remove_file(&tmp_super);
        }

        args.push("--output".into());
        args.push(tmp_super.to_string_lossy().to_string());

        ctx.stage("Запуск lpmake...");
        ctx.log(format!("lpmake {}", args.join(" ")));

        // Запускаем lpmake из папки с cygwin1.dll
        if let Err(e) = run_stream(ctx, &lpmake_exe, &args, Some(lpmake_dir)) {
            let _ = fs::remove_file(&tmp_super);
            crate::telegram::on_build_fail(&format!("Super.img ({project})"), "Выполнение lpmake", &e);
            return Err(e);
        }

        if !tmp_super.is_file() {
            let err = "lpmake завершился, но super.img не был создан".to_string();
            crate::telegram::on_build_fail(&format!("Super.img ({project})"), "Проверка результата", &err);
            return Err(err);
        }

        fs::rename(&tmp_super, &out_super).map_err(es)?;

        let final_sz = out_super.metadata().map(|m| m.len()).unwrap_or(0);
        let final_str = format!("{:.1} МБ", final_sz as f64 / 1048576.0);

        crate::telegram::on_build_progress(&format!("Super.img ({project})"), "Подсчёт контрольной суммы", 90);
        let md5 = crate::util::file_md5(&out_super).unwrap_or_default();

        crate::telegram::on_build_done(&format!("Super.img ({project})"), "super.img", &final_str, &md5, "", "");

        Ok(format!("Образ super.img успешно собран: out/super.img ({final_str}, MD5: {md5})"))
    })
}

/// Сборка готового Fastboot ROM пакета с автоматическими скриптами прошивки
#[tauri::command]
pub fn build_fastboot_package(
    app: AppHandle,
    project: String,
    rom_title: String,
    author: String,
) -> Result<(), String> {
    let d = pdir(&project)?;
    let out_dir = d.join("out");
    let images_dir = d.join("images");
    fs::create_dir_all(&out_dir).map_err(es)?;

    let pkg_name = format!("{project}_fastboot_rom");
    let pkg_dir = out_dir.join(&pkg_name);

    spawn_task(app, format!("Формирование Fastboot ROM: {rom_title}"), move |ctx| {
        crate::telegram::on_build_start(&rom_title);
        crate::telegram::on_build_progress(&rom_title, "Формирование структуры Fastboot пакета", 20);

        if pkg_dir.exists() {
            let _ = fs::remove_dir_all(&pkg_dir);
        }
        fs::create_dir_all(&pkg_dir).map_err(es)?;

        // Копируем super.img (из out/ или images/)
        ctx.stage("Копирование образов прошивки в пакет...");
        let super_in_out = out_dir.join("super.img");
        let super_in_img = images_dir.join("super.img");
        if super_in_out.is_file() {
            ctx.log("  Копирование super.img из out/");
            fs::copy(&super_in_out, pkg_dir.join("super.img")).map_err(es)?;
        } else if super_in_img.is_file() {
            ctx.log("  Копирование super.img из images/");
            fs::copy(&super_in_img, pkg_dir.join("super.img")).map_err(es)?;
        }

        // Копируем критические загрузочные образы (приоритет свежей сборке из out/)
        let boot_parts = ["boot.img", "vendor_boot.img", "recovery.img", "vbmeta.img", "dtbo.img"];
        for bp in boot_parts {
            let fresh = out_dir.join(bp);
            let orig = images_dir.join(bp);
            let src = if fresh.is_file() { fresh } else { orig };
            if src.is_file() {
                ctx.log(format!("  Копирование {bp}"));
                fs::copy(&src, pkg_dir.join(bp)).map_err(es)?;
            }
        }

        crate::telegram::on_build_progress(&rom_title, "Генерация скриптов прошивки (flash_all)", 60);

        // Генерация Windows flash_all.bat
        let bat_content = format!(
            r#"@echo off
chcp 65001 >nul
title MEYAFORGE Fastboot Flasher - {rom_title}
cls
echo ========================================================
echo   MEYAFORGE Fastboot Flasher
echo   Прошивка: {rom_title}
echo   Автор сборки: {author}
echo ========================================================
echo.
echo [1/5] Проверка подключения устройства в режиме Fastboot...
fastboot wait-for-device
echo Устройство обнаружено!
echo.

if exist vbmeta.img (
    echo [2/5] Прошивка vbmeta с отключением AVB/Verity...
    fastboot flash --disable-verity --disable-verification vbmeta vbmeta.img
) else (
    echo [2/5] vbmeta.img отсутствует, пропуск.
)

if exist boot.img (
    echo [3/5] Прошивка ядра (boot)...
    fastboot flash boot boot.img
)

if exist vendor_boot.img (
    fastboot flash vendor_boot vendor_boot.img
)

if exist dtbo.img (
    fastboot flash dtbo dtbo.img
)

if exist super.img (
    echo [4/5] Прошивка динамических разделов (super.img)...
    fastboot flash super super.img
) else (
    echo [4/5] Внимание: super.img не найден в пакете!
)

echo.
echo ========================================================
echo   Прошивка успешно завершена!
echo ========================================================
echo.
set /p reboot="Перезагрузить устройство в систему? (Y/N): "
if /i "%reboot%"=="Y" fastboot reboot
pause
"#,
            rom_title = rom_title,
            author = author
        );
        fs::write(pkg_dir.join("flash_all.bat"), bat_content).map_err(es)?;

        // Генерация Linux / macOS flash_all.sh
        let sh_content = format!(
            r#"#!/bin/bash
echo "========================================================"
echo "  MEYAFORGE Fastboot Flasher"
echo "  ROM: {rom_title}"
echo "  Author: {author}"
echo "========================================================"
fastboot wait-for-device
if [ -f vbmeta.img ]; then
    echo "Flashing vbmeta..."
    fastboot flash --disable-verity --disable-verification vbmeta vbmeta.img
fi
if [ -f boot.img ]; then
    echo "Flashing boot..."
    fastboot flash boot boot.img
fi
if [ -f vendor_boot.img ]; then
    fastboot flash vendor_boot vendor_boot.img
fi
if [ -f super.img ]; then
    echo "Flashing super..."
    fastboot flash super super.img
fi
echo "Flashing completed!"
read -p "Reboot to system now? (y/n) " ans
if [ "$ans" = "y" ]; then
    fastboot reboot
fi
"#,
            rom_title = rom_title,
            author = author
        );
        fs::write(pkg_dir.join("flash_all.sh"), sh_content).map_err(es)?;

        let (_, total_bytes) = count_dir_stats(&pkg_dir);
        let size_str = format!("{:.1} МБ", total_bytes as f64 / 1048576.0);

        crate::telegram::on_build_done(&rom_title, &pkg_name, &size_str, "", &author, "");

        Ok(format!("Fastboot ROM пакет успешно сформирован в out/{pkg_name} ({size_str})"))
    })
}
