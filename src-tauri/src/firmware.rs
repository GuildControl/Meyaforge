//! Формирование полноценной прошивки (Full Fastboot ROM):
//!   - проверки: кодовое имя устройства (codename) по build.prop, anti-rollback индекс;
//!   - пакет: images/, вложенные platform-tools (adb/fastboot), flash.bat / flash.sh;
//!   - firmware.meyaforge — зашифрованный конфиг для официальной тулзы прошивки
//!     (список разделов, информация об устройстве, пошаговые инструкции).

use crate::tools;
use crate::util::{dir, es, spawn_task};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
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

// ---------------- чтение build.prop ----------------

fn read_prop(path: &Path, keys: &[&str]) -> Vec<(String, String)> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            if keys.contains(&k) {
                out.push((k.to_string(), v.trim().to_string()));
            }
        }
    }
    out
}

#[derive(Default, Clone)]
struct DeviceProps {
    codename: Option<String>,
    android: Option<String>,
    sdk: Option<String>,
    security_patch: Option<String>,
    fingerprint: Option<String>,
    model: Option<String>,
    brand: Option<String>,
}

fn detect_device_props(d: &Path) -> DeviceProps {
    let candidates = [
        "extracted/system/system/build.prop",
        "extracted/system/build.prop",
        "extracted/system/system/system/build.prop",
        "extracted/vendor/build.prop",
        "extracted/vendor/vendor/build.prop",
        "extracted/product/build.prop",
        "extracted/product/product/build.prop",
        "extracted/system_ext/build.prop",
    ];
    let keys = [
        "ro.product.device",
        "ro.product.system.device",
        "ro.product.vendor.device",
        "ro.build.product",
        "ro.product.model",
        "ro.product.brand",
        "ro.build.version.release",
        "ro.build.version.sdk",
        "ro.build.version.security_patch",
        "ro.build.fingerprint",
        "ro.system.build.fingerprint",
    ];
    let mut props = DeviceProps::default();
    for rel in candidates {
        let vals = read_prop(&d.join(rel), &keys);
        if vals.is_empty() {
            continue;
        }
        let get = |k: &str| vals.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        if props.codename.is_none() {
            props.codename = get("ro.product.system.device")
                .or_else(|| get("ro.product.device"))
                .or_else(|| get("ro.product.vendor.device"))
                .or_else(|| get("ro.build.product"));
        }
        if props.model.is_none() {
            props.model = get("ro.product.model");
        }
        if props.brand.is_none() {
            props.brand = get("ro.product.brand");
        }
        if props.android.is_none() {
            props.android = get("ro.build.version.release");
        }
        if props.sdk.is_none() {
            props.sdk = get("ro.build.version.sdk");
        }
        if props.security_patch.is_none() {
            props.security_patch = get("ro.build.version.security_patch");
        }
        if props.fingerprint.is_none() {
            props.fingerprint = get("ro.system.build.fingerprint").or_else(|| get("ro.build.fingerprint"));
        }
    }
    props
}

// ---------------- шифрование firmware.meyaforge ----------------

const FW_MAGIC: &[u8; 8] = b"MEYAFW01";
// Встроенный ключ потокового шифра (обфускация конфига от чтения пользователем).
const FW_KEY: &[u8; 32] = b"\x4d\x65\x79\x61\x46\x6f\x72\x67\x65\x53\x65\x63\x75\x72\x65\x46\x57\x43\x6f\x6e\x66\x69\x67\x4b\x65\x79\x32\x30\x32\x36\x21\x4d";

fn fw_crypt(data: &[u8]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, b)| {
            let stream = FW_KEY[i % FW_KEY.len()]
                .wrapping_add((i as u32).wrapping_mul(0x9E37_79B1) as u8)
                .rotate_left((i % 7) as u32);
            b ^ stream
        })
        .collect()
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// Упаковка JSON-конфига в зашифрованный контейнер firmware.meyaforge.
/// Формат: magic(8) | payload_len u32 LE | fnv1a64(payload) u64 LE | ciphertext.
pub fn encrypt_fw_config(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20 + payload.len());
    out.extend_from_slice(FW_MAGIC);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&fnv1a64(payload).to_le_bytes());
    out.extend_from_slice(&fw_crypt(payload));
    out
}

/// Расшифровка контейнера (понадобится официальной тулзе прошивки).
#[allow(dead_code)]
pub fn decrypt_fw_config(blob: &[u8]) -> Result<Vec<u8>, String> {
    if blob.len() < 20 || &blob[0..8] != FW_MAGIC {
        return Err("Это не файл конфигурации MEYAFORGE (неверная сигнатура)".into());
    }
    let len = u32::from_le_bytes(blob[8..12].try_into().unwrap()) as usize;
    let check = u64::from_le_bytes(blob[12..20].try_into().unwrap());
    if blob.len() != 20 + len {
        return Err("Файл конфигурации повреждён (длина не совпадает)".into());
    }
    let plain = fw_crypt(&blob[20..]);
    if fnv1a64(&plain) != check {
        return Err("Файл конфигурации повреждён (контрольная сумма не совпала)".into());
    }
    Ok(plain)
}

// ---------------- префлайт-проверки ----------------

#[derive(Serialize)]
pub struct FwImageInfo {
    pub name: String,
    pub file_name: String,
    pub size: u64,
    pub fresh: bool, // true — свежая сборка из out/
}

#[derive(Serialize)]
pub struct FwPreflight {
    pub codename_detected: Option<String>,
    pub model: Option<String>,
    pub brand: Option<String>,
    pub android: Option<String>,
    pub sdk: Option<String>,
    pub security_patch: Option<String>,
    pub images: Vec<FwImageInfo>,
    pub platform_tools: bool,
    pub warnings: Vec<String>,
}

/// Собирает список образов для пакета: приоритет out/ (свежая сборка), затем images/.
fn collect_images(d: &Path) -> Vec<(String, PathBuf, bool)> {
    let out_dir = d.join("out");
    let img_dir = d.join("images");
    let mut map: Vec<(String, PathBuf, bool)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&img_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map_or(false, |x| x.eq_ignore_ascii_case("img")) {
                let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                map.push((name, p, false));
            }
        }
    }
    if let Ok(entries) = fs::read_dir(&out_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map_or(false, |x| x.eq_ignore_ascii_case("img")) {
                let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                if let Some(slot) = map.iter_mut().find(|(n, _, _)| *n == name) {
                    *slot = (name, p, true);
                } else {
                    map.push((name, p, true));
                }
            }
        }
    }
    map.sort_by(|a, b| a.0.cmp(&b.0));
    map
}

/// Порядок прошивки разделов: vbmeta → загрузочные → super → остальные.
fn flash_order(name: &str) -> u32 {
    match name {
        "vbmeta" | "vbmeta_system" | "vbmeta_vendor" => 0,
        "boot" | "init_boot" => 10,
        "vendor_boot" => 11,
        "dtbo" => 12,
        "recovery" => 13,
        "super" => 20,
        _ => 30,
    }
}

fn platform_tools_dir() -> Option<PathBuf> {
    let adb = tools::bin("platform-tools", "adb.exe").ok()?;
    adb.parent().map(|p| p.to_path_buf())
}

/// Префлайт: определение codename/версий и списка образов перед формированием прошивки.
#[tauri::command]
pub fn firmware_preflight(project: String) -> Result<FwPreflight, String> {
    let d = pdir(&project)?;
    let props = detect_device_props(&d);
    let images: Vec<FwImageInfo> = collect_images(&d)
        .into_iter()
        .map(|(name, p, fresh)| FwImageInfo {
            file_name: format!("{name}.img"),
            size: p.metadata().map(|m| m.len()).unwrap_or(0),
            name,
            fresh,
        })
        .collect();

    let mut warnings = Vec::new();
    if props.codename.is_none() {
        warnings.push("Не удалось определить codename устройства из build.prop — укажите его вручную.".to_string());
    }
    if images.is_empty() {
        warnings.push("В проекте нет ни одного .img образа (images/ и out/ пусты).".to_string());
    }
    if !images.iter().any(|i| i.name == "super")
        && !images.iter().any(|i| i.name == "system")
    {
        warnings.push("Нет super.img и system.img — пакет может быть неполным.".to_string());
    }
    let pt = platform_tools_dir().is_some();
    if !pt {
        warnings.push("Platform Tools (adb/fastboot) не установлены в MEYAFORGE — в пакет они не войдут, скрипты будут использовать fastboot из PATH.".to_string());
    }

    Ok(FwPreflight {
        codename_detected: props.codename,
        model: props.model,
        brand: props.brand,
        android: props.android,
        sdk: props.sdk,
        security_patch: props.security_patch,
        images,
        platform_tools: pt,
        warnings,
    })
}

// ---------------- формирование прошивки ----------------

fn file_fnv_hex(path: &Path) -> String {
    let Ok(mut f) = fs::File::open(path) else {
        return String::new();
    };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let Ok(n) = f.read(&mut buf) else { break };
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    format!("{h:016x}")
}

/// Формирование полноценной прошивки с проверками codename / anti-rollback.
#[tauri::command]
pub fn build_full_firmware(
    app: AppHandle,
    project: String,
    rom_title: String,
    author: String,
    codename: String,
    antirollback: u32,
) -> Result<(), String> {
    let d = pdir(&project)?;
    let out_dir = d.join("out");
    fs::create_dir_all(&out_dir).map_err(es)?;

    let codename_l = codename.trim().to_lowercase();
    if codename_l.is_empty() {
        return Err("Укажите кодовое имя устройства (codename), например: lisa, gauguin, sweet".into());
    }

    // Проверка codename против build.prop проекта
    let props = detect_device_props(&d);
    if let Some(det) = &props.codename {
        if det.to_lowercase() != codename_l {
            return Err(format!(
                "Codename не совпадает: в build.prop проекта указано «{det}», а вы ввели «{codename_l}». \
                 Прошивка с неверным codename может превратить устройство в «кирпич» — исправьте значение."
            ));
        }
    }

    let images = collect_images(&d);
    if images.is_empty() {
        return Err("Нет образов для упаковки: соберите разделы (шаг 1–2) или импортируйте прошивку в проект.".into());
    }

    let safe_title: String = rom_title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let pkg_name = format!("{}_{}_fw", if safe_title.is_empty() { &project } else { &safe_title }, codename_l);
    let pkg_dir = out_dir.join(&pkg_name);

    spawn_task(app, format!("Формирование прошивки: {rom_title} ({codename_l})"), move |ctx| {
        crate::telegram::on_build_start(&rom_title);
        crate::telegram::on_build_progress(&rom_title, "Проверки codename / anti-rollback", 10);

        if pkg_dir.exists() {
            let _ = fs::remove_dir_all(&pkg_dir);
        }
        let images_out = pkg_dir.join("images");
        fs::create_dir_all(&images_out).map_err(es)?;

        // ---- копирование образов + контрольные суммы ----
        ctx.stage("Копирование образов прошивки и подсчёт контрольных сумм...");
        let mut cfg_parts: Vec<serde_json::Value> = Vec::new();
        let mut copied: Vec<(String, u32)> = Vec::new(); // (имя раздела, порядок) для скриптов
        let total = images.len();
        for (idx, (name, src, fresh)) in images.iter().enumerate() {
            let size = src.metadata().map(|m| m.len()).unwrap_or(0);
            ctx.log(format!(
                "  {name}.img — {:.1} МБ {}",
                size as f64 / 1_048_576.0,
                if *fresh { "(свежая сборка из out/)" } else { "(оригинал из images/)" }
            ));
            fs::copy(src, images_out.join(format!("{name}.img"))).map_err(es)?;
            let sum = file_fnv_hex(&images_out.join(format!("{name}.img")));
            let order = flash_order(name);
            copied.push((name.clone(), order));
            cfg_parts.push(json!({
                "name": name,
                "file": format!("images/{name}.img"),
                "size": size,
                "checksum_fnv1a64": sum,
                "order": order,
                "critical": matches!(name.as_str(), "boot" | "init_boot" | "super" | "vbmeta"),
                "flash": if name == "vbmeta" {
                    "fastboot flash --disable-verity --disable-verification vbmeta images/vbmeta.img".to_string()
                } else {
                    format!("fastboot flash {name} images/{name}.img")
                },
            }));
            ctx.progress((idx + 1) as u64, total as u64 + 2);
        }
        copied.sort_by_key(|(_, o)| *o);

        crate::telegram::on_build_progress(&rom_title, "Встраивание platform-tools (adb/fastboot)", 55);

        // ---- вложенные platform-tools ----
        let mut tools_note = "PATH";
        if let Some(pt_dir) = platform_tools_dir() {
            let win_dir = pkg_dir.join("tools").join("win");
            fs::create_dir_all(&win_dir).map_err(es)?;
            let mut copied_bins = 0;
            if let Ok(entries) = fs::read_dir(&pt_dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    let fname = e.file_name().to_string_lossy().to_lowercase();
                    if p.is_file()
                        && (fname == "adb.exe"
                            || fname == "fastboot.exe"
                            || fname.ends_with(".dll"))
                    {
                        let _ = fs::copy(&p, win_dir.join(e.file_name()));
                        copied_bins += 1;
                    }
                }
            }
            if copied_bins > 0 {
                tools_note = "tools/win";
                ctx.log(format!("  Вложено platform-tools: {copied_bins} файлов (adb, fastboot, dll)"));
            }
        } else {
            ctx.log("  Platform-tools не найдены — скрипты будут использовать fastboot из PATH");
        }

        crate::telegram::on_build_progress(&rom_title, "Генерация flash.bat / flash.sh с проверками", 70);

        // ---- генерация команд прошивки для скриптов ----
        let mut bat_flash = String::new();
        let mut sh_flash = String::new();
        let mut step_no = 3usize;
        for (name, _) in &copied {
            let file = format!("images\\{name}.img");
            let file_sh = format!("images/{name}.img");
            if name == "vbmeta" {
                bat_flash.push_str(&format!(
                    "if exist {file} (\r\n  echo [{step_no}] Прошивка vbmeta с отключением AVB/dm-verity...\r\n  \"%FB%\" flash --disable-verity --disable-verification vbmeta {file} || goto :flash_fail\r\n)\r\n"
                ));
                sh_flash.push_str(&format!(
                    "if [ -f {file_sh} ]; then\n  echo \"[{step_no}] Flashing vbmeta (AVB/dm-verity disabled)...\"\n  \"$FB\" flash --disable-verity --disable-verification vbmeta {file_sh} || exit 1\nfi\n"
                ));
            } else {
                bat_flash.push_str(&format!(
                    "if exist {file} (\r\n  echo [{step_no}] Прошивка раздела {name}...\r\n  \"%FB%\" flash {name} {file} || goto :flash_fail\r\n)\r\n"
                ));
                sh_flash.push_str(&format!(
                    "if [ -f {file_sh} ]; then\n  echo \"[{step_no}] Flashing {name}...\"\n  \"$FB\" flash {name} {file_sh} || exit 1\nfi\n"
                ));
            }
            step_no += 1;
        }

        // ---- flash.bat с проверками codename и anti-rollback ----
        let bat = format!(
            "@echo off\r\nchcp 65001 >nul\r\ntitle MEYAFORGE Firmware Flasher - {title}\r\ncls\r\n\
             echo ========================================================\r\n\
             echo   MEYAFORGE Firmware Flasher\r\n\
             echo   Прошивка: {title}\r\n\
             echo   Устройство (codename): {code}\r\n\
             echo   Anti-Rollback индекс: {arb}\r\n\
             echo   Автор сборки: {author}\r\n\
             echo ========================================================\r\n\
             echo.\r\n\
             set FB=fastboot\r\nif exist \"tools\\win\\fastboot.exe\" set FB=tools\\win\\fastboot.exe\r\n\
             echo [1] Ожидание устройства в режиме Fastboot...\r\n\"%FB%\" wait-for-device\r\n\
             \r\necho [2] Проверка кодового имени устройства...\r\n\
             set \"DEVCODE=\"\r\nfor /f \"tokens=2\" %%a in ('\"%FB%\" getvar product 2^>^&1 ^| findstr /b /c:\"product:\"') do set \"DEVCODE=%%a\"\r\n\
             if not defined DEVCODE (echo ОШИБКА: не удалось прочитать codename устройства.& pause & exit /b 1)\r\n\
             echo   Устройство: %DEVCODE% ^| Ожидается: {code}\r\n\
             if /i not \"%DEVCODE%\"==\"{code}\" (\r\n  echo.\r\n  echo ОШИБКА: Эта прошивка собрана для «{code}», а подключено «%DEVCODE%»!\r\n  echo Прошивка отменена во избежание кирпича.\r\n  pause\r\n  exit /b 1\r\n)\r\n\
             \r\necho [ARB] Проверка Anti-Rollback...\r\n\
             set \"DEVANTI=\"\r\nfor /f \"tokens=2\" %%a in ('\"%FB%\" getvar anti 2^>^&1 ^| findstr /b /c:\"anti:\"') do set \"DEVANTI=%%a\"\r\n\
             if defined DEVANTI (\r\n  echo   Anti устройства: %DEVANTI% ^| Anti прошивки: {arb}\r\n  if %DEVANTI% GTR {arb} (\r\n    echo ОШИБКА: Anti-Rollback устройства ^(%DEVANTI%^) выше, чем у прошивки ^({arb}^).\r\n    echo Понижение запрещено — прошивка отменена.\r\n    pause\r\n    exit /b 1\r\n  )\r\n) else (\r\n  echo   Устройство не сообщает anti-rollback, пропуск проверки.\r\n)\r\n\
             echo.\r\n{flash}\
             echo.\r\necho ========================================================\r\necho   Прошивка успешно завершена!\r\necho ========================================================\r\n\
             set /p reboot=\"Перезагрузить устройство в систему? (Y/N): \"\r\nif /i \"%reboot%\"==\"Y\" \"%FB%\" reboot\r\npause\r\nexit /b 0\r\n\
             \r\n:flash_fail\r\necho.\r\necho ОШИБКА: fastboot вернул ошибку. Прошивка прервана.\r\npause\r\nexit /b 1\r\n",
            title = rom_title,
            code = codename_l,
            arb = antirollback,
            author = author,
            flash = bat_flash
        );
        fs::write(pkg_dir.join("flash.bat"), bat).map_err(es)?;

        let sh = format!(
            "#!/bin/bash\n\
             echo \"========================================================\"\n\
             echo \"  MEYAFORGE Firmware Flasher\"\n\
             echo \"  ROM: {title} | Device: {code} | ARB: {arb}\"\n\
             echo \"========================================================\"\n\
             FB=fastboot\n[ -x \"$(dirname \"$0\")/tools/win/fastboot\" ] && FB=\"$(dirname \"$0\")/tools/win/fastboot\"\n\
             echo \"[1] Waiting for fastboot device...\"\n\"$FB\" wait-for-device\n\
             DEVCODE=$(\"$FB\" getvar product 2>&1 | grep -oP '(?<=product: ).*' | head -n1)\n\
             echo \"[2] Device: $DEVCODE | Expected: {code}\"\n\
             if [ \"$DEVCODE\" != \"{code}\" ]; then\n  echo \"ERROR: firmware is built for '{code}', connected device is '$DEVCODE'. Aborting.\" 1>&2\n  exit 1\nfi\n\
             DEVANTI=$(\"$FB\" getvar anti 2>&1 | grep -oP '(?<=anti: )[0-9]+' | head -n1)\n\
             if [ -n \"$DEVANTI\" ] && [ \"$DEVANTI\" -gt {arb} ]; then\n  echo \"ERROR: device anti-rollback ($DEVANTI) is higher than firmware ({arb}). Downgrade blocked.\" 1>&2\n  exit 1\nfi\n\
             {flash}\
             echo \"Flashing completed!\"\nread -p \"Reboot to system now? (y/n) \" ans\n[ \"$ans\" = \"y\" ] && \"$FB\" reboot\n",
            title = rom_title,
            code = codename_l,
            arb = antirollback,
            flash = sh_flash
        );
        fs::write(pkg_dir.join("flash.sh"), sh).map_err(es)?;

        crate::telegram::on_build_progress(&rom_title, "Шифрование firmware.meyaforge", 85);

        // ---- firmware.meyaforge (зашифрованный конфиг) ----
        ctx.stage("Формирование зашифрованного firmware.meyaforge...");
        let created = SystemTime::now().duration_since(UNIX_EPOCH).map(|t| t.as_secs()).unwrap_or(0);
        let steps: Vec<String> = copied
            .iter()
            .enumerate()
            .map(|(i, (name, _))| {
                if name == "vbmeta" {
                    format!("{}. Прошить vbmeta с отключением AVB: fastboot flash --disable-verity --disable-verification vbmeta images/vbmeta.img", i + 3)
                } else {
                    format!("{}. Прошить раздел {name}: fastboot flash {name} images/{name}.img", i + 3)
                }
            })
            .collect();
        let mut all_steps = vec![
            "1. Перевести устройство в режим Fastboot и подключить к ПК".to_string(),
            format!("2. Проверить codename: fastboot getvar product → должен быть «{codename_l}»; проверить anti-rollback: fastboot getvar anti → не выше {antirollback}"),
        ];
        all_steps.extend(steps);
        all_steps.push(format!("{}. Перезагрузить устройство: fastboot reboot", all_steps.len() + 1));

        let config = json!({
            "format": 1,
            "tool": "MEYAFORGE",
            "rom_title": rom_title,
            "author": author,
            "created": created,
            "device": {
                "codename": codename_l,
                "antirollback": antirollback,
                "model": props.model,
                "brand": props.brand,
                "android": props.android,
                "sdk": props.sdk,
                "security_patch": props.security_patch,
                "fingerprint": props.fingerprint,
            },
            "checks": {
                "require_codename": true,
                "codename": codename_l,
                "antirollback_max_device": antirollback,
            },
            "package": {
                "images_dir": "images",
                "platform_tools": tools_note,
                "flash_windows": "flash.bat",
                "flash_linux": "flash.sh",
            },
            "partitions": cfg_parts,
            "steps": all_steps,
        });
        let payload = serde_json::to_vec_pretty(&config).map_err(es)?;
        let blob = encrypt_fw_config(&payload);
        fs::write(pkg_dir.join("firmware.meyaforge"), &blob).map_err(es)?;
        ctx.log(format!("  firmware.meyaforge записан ({} байт, зашифрован)", blob.len()));

        let (_, total_bytes) = {
            let mut count = 0usize;
            let mut bytes = 0u64;
            let mut stack = vec![pkg_dir.clone()];
            while let Some(dir) = stack.pop() {
                if let Ok(rd) = fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            stack.push(p);
                        } else {
                            count += 1;
                            bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                        }
                    }
                }
            }
            (count, bytes)
        };
        let size_str = format!("{:.1} МБ", total_bytes as f64 / 1_048_576.0);

        crate::telegram::on_build_done(&rom_title, &pkg_name, &size_str, "", &author, "");

        Ok(format!(
            "Прошивка «{rom_title}» сформирована: out/{pkg_name} ({size_str}, codename: {codename_l}, ARB: {antirollback})"
        ))
    })
}
