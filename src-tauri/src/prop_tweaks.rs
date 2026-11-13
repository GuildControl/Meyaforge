//! Модуль управления свойствами системы (build.prop, system.prop, vendor.prop)
//! и каталог предустановленных твиков (Pixel Spoofing, частота экрана, автономность, производительность).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PropEntry {
    pub key: String,
    pub value: String,
    pub line_num: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PropFileInfo {
    pub file_path: String,
    pub total_props: usize,
    pub props: Vec<PropEntry>,
    pub duplicates: Vec<String>,
    pub has_backup: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TweakPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub props: BTreeMap<String, String>,
}

pub fn get_available_tweak_presets() -> Vec<TweakPreset> {
    vec![
        TweakPreset {
            id: "pixel_spoof".into(),
            name: "Pixel 8 Pro Spoofing (Google Photos & 90/120 FPS)".into(),
            description: "Маскировка под флагман Google для безлимитного хранилища Google Photos и разблокировки высоких FPS в играх".into(),
            category: "Кастомизация".into(),
            props: BTreeMap::from([
                ("ro.product.model".into(), "Pixel 8 Pro".into()),
                ("ro.product.brand".into(), "google".into()),
                ("ro.product.manufacturer".into(), "Google".into()),
                ("ro.product.device".into(), "husky".into()),
            ]),
        },
        TweakPreset {
            id: "battery_saver".into(),
            name: "Глубокий сон и оптимизация батареи".into(),
            description: "Увеличение интервала опроса Wi-Fi, включение глубокого сна модема и отключение фоновой телеметрии".into(),
            category: "Автономность".into(),
            props: BTreeMap::from([
                ("wifi.supplicant_scan_interval".into(), "240".into()),
                ("ro.ril.disable.power.collapse".into(), "0".into()),
                ("pm.sleep_mode".into(), "1".into()),
                ("profiler.force_disable_ulog".into(), "1".into()),
            ]),
        },
        TweakPreset {
            id: "perf_touch".into(),
            name: "Ускорение UI и отзывчивость тача".into(),
            description: "Принудительный аппаратный рендеринг GPU, мгновенный отклик на касания и отключение дизеринга".into(),
            category: "Производительность".into(),
            props: BTreeMap::from([
                ("debug.sf.hw".into(), "1".into()),
                ("debug.performance.tuning".into(), "1".into()),
                ("video.accelerate.hw".into(), "1".into()),
                ("persist.sys.use_dithering".into(), "0".into()),
                ("touch.pressure.scale".into(), "0.001".into()),
            ]),
        },
        TweakPreset {
            id: "disable_logging".into(),
            name: "Отключение тяжелого логирования".into(),
            description: "Снижает нагрузку на процессор и диск UFS/eMMC за счет отключения постоянной записи логов отладки".into(),
            category: "Оптимизация".into(),
            props: BTreeMap::from([
                ("logcat.live".into(), "disable".into()),
                ("profiler.force_disable_err_rpt".into(), "1".into()),
                ("ro.config.nocheckin".into(), "1".into()),
            ]),
        },
    ]
}

#[tauri::command]
pub fn prop_list_presets() -> Vec<TweakPreset> {
    get_available_tweak_presets()
}

#[tauri::command]
pub fn prop_read_file(file_path: String) -> Result<PropFileInfo, String> {
    let p = Path::new(&file_path);
    if !p.is_file() {
        return Err(format!("Файл свойств не найден: {file_path}"));
    }

    let content = fs::read_to_string(p).map_err(|e| e.to_string())?;
    let mut props = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut duplicates = Vec::new();

    for (idx, line) in content.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let key = k.trim().to_string();
            let value = v.trim().to_string();

            if seen.contains(&key) {
                duplicates.push(key.clone());
            } else {
                seen.insert(key.clone());
            }

            props.push(PropEntry {
                key,
                value,
                line_num: idx + 1,
            });
        }
    }

    let has_backup = Path::new(&format!("{file_path}.bak")).is_file();

    Ok(PropFileInfo {
        file_path,
        total_props: props.len(),
        props,
        duplicates,
        has_backup,
    })
}

#[tauri::command]
pub fn prop_apply_tweaks(file_path: String, tweaks: BTreeMap<String, String>) -> Result<String, String> {
    let p = Path::new(&file_path);
    if !p.is_file() {
        return Err(format!("Файл свойств не найден: {file_path}"));
    }

    // Создаем .bak бэкап
    let bak = format!("{file_path}.bak");
    if !Path::new(&bak).exists() {
        let _ = fs::copy(p, &bak);
    }

    let content = fs::read_to_string(p).map_err(|e| e.to_string())?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut updated_keys = std::collections::HashSet::new();

    for line in &mut lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let parsed_pair = t.split_once('=').map(|(k, _)| k.trim().to_string());
        if let Some(key) = parsed_pair {
            if let Some(new_val) = tweaks.get(&key) {
                *line = format!("{key}={new_val}");
                updated_keys.insert(key);
            }
        }
    }

    // Добавляем новые ключи, которых не было в файле
    let mut added_count = 0;
    for (k, v) in &tweaks {
        if !updated_keys.contains(k) {
            lines.push(format!("{k}={v}"));
            added_count += 1;
        }
    }

    fs::write(p, lines.join("\n")).map_err(|e| e.to_string())?;
    Ok(format!("Применено {} твиков (добавлено новых: {added_count}) к файлу {}", tweaks.len(), p.file_name().unwrap_or_default().to_string_lossy()))
}
