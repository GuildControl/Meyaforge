//! Модуль анализа LK (Little Kernel), ABL, XBL и других загрузчиков Android.
//! Извлечение fastboot и OEM команд, разделов, SoC-сигнатур и отладочных строк.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OemCmdItem {
    pub name: String,
    pub offset: usize,
    pub context: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LkAnalysisResult {
    pub file_name: String,
    pub file_size: u64,
    pub is_bootloader: bool,
    pub platform: String,
    pub soc_hints: Vec<String>,
    pub oem_commands: Vec<OemCmdItem>,
    pub fastboot_commands: Vec<String>,
    pub partitions: Vec<String>,
    pub security_flags: Vec<String>,
    pub total_strings_found: usize,
}

/// Извлечение ASCII и UTF-16LE строк из байтового среза
fn extract_ascii_strings(data: &[u8], min_len: usize) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    let mut current = Vec::new();
    let mut start_idx = 0;

    for (i, &b) in data.iter().enumerate() {
        if (0x20..=0x7E).contains(&b) {
            if current.is_empty() {
                start_idx = i;
            }
            current.push(b);
        } else {
            if current.len() >= min_len {
                if let Ok(s) = String::from_utf8(current.clone()) {
                    results.push((start_idx, s));
                }
            }
            current.clear();
        }
    }
    if current.len() >= min_len {
        if let Ok(s) = String::from_utf8(current) {
            results.push((start_idx, s));
        }
    }
    results
}

pub fn analyze_lk_bytes(name: &str, data: &[u8]) -> LkAnalysisResult {
    let strings = extract_ascii_strings(data, 3);
    let total_strings = strings.len();

    let mut oem_commands_map = std::collections::BTreeMap::new();
    let mut fastboot_commands = BTreeSet::new();
    let mut partitions = BTreeSet::new();
    let mut soc_hints = BTreeSet::new();
    let mut security_flags = BTreeSet::new();

    // Распространенные имена разделов для поиска
    let known_partitions: HashSet<&str> = [
        "boot", "init_boot", "vendor_boot", "recovery", "system", "vendor", "product", "system_ext",
        "odm", "super", "userdata", "cache", "persist", "metadata", "dtbo", "vbmeta", "vbmeta_system",
        "vbmeta_vendor", "cust", "splash", "logo", "misc", "frp", "para", "nvram", "nvdata",
        "nvcfg", "proinfo", "tee1", "tee2", "lk", "lk2", "abl", "xbl", "tz", "rpm", "hyp",
        "modem", "dsp", "bootloader", "radio", "secro", "md1img", "spmfw", "sspm", "pi_img"
    ].iter().cloned().collect();

    // Известные стандартные fastboot команды
    let standard_fastboot: HashSet<&str> = [
        "flash", "erase", "format", "reboot", "reboot-bootloader", "reboot-recovery",
        "reboot-fastboot", "reboot-edl", "continue", "boot", "getvar", "download",
        "set_active", "snapshot-update", "flashing", "stage", "oem", "dump"
    ].iter().cloned().collect();

    let mut detected_platform = "Не определено (Generic LK/Bootloader)".to_string();

    for (offset, s) in &strings {
        let trimmed = s.trim();
        let lower = trimmed.to_lowercase();

        // 1. Поиск платформы
        if lower.contains("mediatek") || lower.contains("mtk") || lower.contains("alps") {
            detected_platform = "MediaTek (MTK)".into();
        } else if lower.contains("qualcomm") || lower.contains("snapdragon") || lower.contains("qcom") {
            detected_platform = "Qualcomm (Snapdragon)".into();
        } else if lower.contains("spreadtrum") || lower.contains("unisoc") || lower.contains("sprd") {
            detected_platform = "Unisoc / Spreadtrum".into();
        } else if lower.contains("exynos") || lower.contains("sboot") {
            detected_platform = "Samsung Exynos".into();
        } else if lower.contains("rockchip") {
            detected_platform = "Rockchip".into();
        }

        // Чипсетные подсказки
        if lower.starts_with("mt6") || lower.starts_with("mt8") || lower.starts_with("sm8")
            || lower.starts_with("sm7") || lower.starts_with("sm6") || lower.starts_with("sdm")
            || lower.starts_with("msm") || lower.starts_with("sc98") || lower.starts_with("ums")
        {
            if trimmed.len() <= 12 && !trimmed.contains(' ') {
                soc_hints.insert(trimmed.to_string());
            }
        }

        // 2. Извлечение OEM команд
        // Форматы: "oem <command>", "oem_<command>", "fastboot: oem <command>", "oem-"
        if lower.starts_with("oem ") || lower.starts_with("oem_") || lower.starts_with("oem-") {
            let cmd_name = trimmed.to_string();
            if cmd_name.len() <= 64 && !cmd_name.contains('\n') && !cmd_name.contains('\t') {
                oem_commands_map.entry(cmd_name.clone()).or_insert_with(|| OemCmdItem {
                    name: cmd_name,
                    offset: *offset,
                    context: trimmed.to_string(),
                });
            }
        } else if let Some(idx) = lower.find("oem ") {
            // Выделяем команду из контекста
            let slice = &trimmed[idx..];
            let end = slice.find(|c: char| c == '"' || c == '\'' || c == '\n' || c == ',' || c == ';')
                .unwrap_or(slice.len());
            let clean = slice[..end].trim();
            if clean.starts_with("oem ") && clean.len() >= 5 && clean.len() <= 50 {
                oem_commands_map.entry(clean.to_string()).or_insert_with(|| OemCmdItem {
                    name: clean.to_string(),
                    offset: *offset + idx,
                    context: trimmed.to_string(),
                });
            }
        } else if lower.starts_with("fastboot_publish(") || lower.starts_with("fastboot_register(") {
            let inner = trimmed;
            fastboot_commands.insert(inner.to_string());
        }

        // 3. Fastboot команды
        if standard_fastboot.contains(lower.as_str()) {
            fastboot_commands.insert(lower.clone());
        } else if lower.starts_with("fastboot ") || lower.starts_with("fastboot:") {
            fastboot_commands.insert(trimmed.to_string());
        } else if lower.starts_with("flashing ") {
            fastboot_commands.insert(trimmed.to_string());
        }

        // 4. Поиск разделов
        if known_partitions.contains(lower.as_str()) {
            partitions.insert(lower.clone());
        }

        // 5. Флаги безопасности / отладки
        if lower.contains("unlock") || lower.contains("lock") || lower.contains("secure boot")
            || lower.contains("tamper") || lower.contains("warranty") || lower.contains("frp")
            || lower.contains("verity") || lower.contains("avb") || lower.contains("anti-rollback")
            || lower.contains("rollback") || lower.contains("efuse")
        {
            if trimmed.len() <= 60 && !trimmed.contains('\n') {
                security_flags.insert(trimmed.to_string());
            }
        }
    }

    let is_bootloader = !oem_commands_map.is_empty()
        || !fastboot_commands.is_empty()
        || name.contains("lk")
        || name.contains("abl")
        || name.contains("xbl")
        || name.contains("bootloader");

    LkAnalysisResult {
        file_name: name.to_string(),
        file_size: data.len() as u64,
        is_bootloader,
        platform: detected_platform,
        soc_hints: soc_hints.into_iter().take(15).collect(),
        oem_commands: oem_commands_map.into_values().collect(),
        fastboot_commands: fastboot_commands.into_iter().collect(),
        partitions: partitions.into_iter().collect(),
        security_flags: security_flags.into_iter().take(25).collect(),
        total_strings_found: total_strings,
    }
}

pub fn analyze_file(path_str: &str) -> Result<LkAnalysisResult, String> {
    let p = Path::new(path_str);
    if !p.is_file() {
        return Err(format!("Файл не найден: {path_str}"));
    }

    let file_name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
    let mut file = File::open(p).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).map_err(|e| e.to_string())?;

    Ok(analyze_lk_bytes(&file_name, &buf))
}


#[tauri::command]
pub fn lk_analyze_file(file_path: String) -> Result<LkAnalysisResult, String> {
    analyze_file(&file_path)
}

#[tauri::command]
pub fn lk_analyze_partition(project: String, partition_name: String) -> Result<LkAnalysisResult, String> {
    let pdir = crate::util::dir("projects").join(&project);
    if !pdir.is_dir() {
        return Err(format!("Проект «{project}» не найден"));
    }
    let candidates = [
        pdir.join("images").join(format!("{partition_name}.img")),
        pdir.join("images").join(format!("{partition_name}.bin")),
        pdir.join("images").join(&partition_name),
        pdir.join("input").join(format!("{partition_name}.img")),
        pdir.join("input").join(format!("{partition_name}.bin")),
    ];
    for c in &candidates {
        if c.is_file() {
            return analyze_file(&c.to_string_lossy());
        }
    }
    Err(format!("Образ раздела «{partition_name}» не найден в проекте"))
}
