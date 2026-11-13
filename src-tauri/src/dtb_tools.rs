//! Модуль анализа и работы с Device Tree (DTB, DTBO, DTS).
//! Поддерживает: чтение заголовка FDT (Flattened Device Tree - 0xD00DFEED),
//! извлечение совместимых SoC/плат (compatible strings), декомпиляцию и сборку.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;

pub const FDT_MAGIC: u32 = 0xD00DFEED;
pub const DTBO_MAGIC: u32 = 0xD7B00000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DtbDetails {
    pub file_name: String,
    pub file_size: u64,
    pub is_dtb: bool,
    pub is_dtbo: bool,
    pub total_size: u32,
    pub version: u32,
    pub last_comp_version: u32,
    pub boot_cpuid_phys: u32,
    pub model: String,
    pub compatible: Vec<String>,
    pub bootargs: String,
    pub found_nodes: Vec<String>,
}

fn u32be(b: &[u8], offset: usize) -> u32 {
    if offset + 4 > b.len() {
        return 0;
    }
    u32::from_be_bytes(b[offset..offset + 4].try_into().unwrap())
}

pub fn parse_fdt_header(name: &str, data: &[u8]) -> Result<DtbDetails, String> {
    if data.len() < 40 {
        return Err("Размер файла меньше заголовка FDT".into());
    }

    let magic = u32be(data, 0);
    let is_dtb = magic == FDT_MAGIC;
    let is_dtbo = magic == DTBO_MAGIC;

    if !is_dtb && !is_dtbo {
        return Err(format!("Некорректная сигнатура Device Tree (получено {magic:#010X}, ожидалось {FDT_MAGIC:#010X} или {DTBO_MAGIC:#010X})"));
    }

    let total_size = u32be(data, 4);
    let _off_dt_struct = u32be(data, 8) as usize;
    let _off_dt_strings = u32be(data, 12) as usize;
    let version = u32be(data, 20);
    let last_comp_version = u32be(data, 24);
    let boot_cpuid_phys = u32be(data, 28);

    // Извлечение строк из таблицы строк и структуры
    let mut model = String::new();
    let mut compatible = Vec::new();
    let mut bootargs = String::new();
    let mut found_nodes = Vec::new();

    // Быстрое сканирование ASCII строк внутри blob
    let mut cur_str = Vec::new();
    let mut all_strings = Vec::new();

    for &b in data {
        if (0x20..=0x7E).contains(&b) {
            cur_str.push(b);
        } else {
            if cur_str.len() >= 3 {
                if let Ok(s) = String::from_utf8(cur_str.clone()) {
                    all_strings.push(s);
                }
            }
            cur_str.clear();
        }
    }

    for s in &all_strings {
        let low = s.to_lowercase();
        if model.is_empty() && (low.contains("qualcomm technologies") || low.contains("mediatek") || low.contains("samsung") || low.contains("xiaomi")) {
            if s.len() < 80 {
                model = s.clone();
            }
        }
        if low.starts_with("qcom,") || low.starts_with("mediatek,") || low.starts_with("sprd,") || low.starts_with("exynos,") || low.starts_with("arm,") {
            if !compatible.contains(s) && s.len() < 60 {
                compatible.push(s.clone());
            }
        }
        if low.contains("console=") || low.contains("androidboot.") || low.contains("bootargs") {
            if bootargs.is_empty() && s.len() < 200 {
                bootargs = s.clone();
            }
        }
        if s.starts_with("soc") || s.starts_with("cpus") || s.starts_with("memory") || s.starts_with("chosen")
            || s.starts_with("display") || s.starts_with("touchscreen") || s.starts_with("camera")
            || s.starts_with("battery") || s.starts_with("charger") || s.starts_with("panel")
        {
            if !found_nodes.contains(s) && found_nodes.len() < 25 {
                found_nodes.push(s.clone());
            }
        }
    }

    Ok(DtbDetails {
        file_name: name.to_string(),
        file_size: data.len() as u64,
        is_dtb,
        is_dtbo,
        total_size,
        version,
        last_comp_version,
        boot_cpuid_phys,
        model: if model.is_empty() { "Generic Board / SoC".into() } else { model },
        compatible: compatible.into_iter().take(12).collect(),
        bootargs,
        found_nodes,
    })
}

#[tauri::command]
pub fn dtb_inspect(file_path: String) -> Result<DtbDetails, String> {
    let p = Path::new(&file_path);
    if !p.is_file() {
        return Err(format!("Файл не найден: {file_path}"));
    }
    let data = fs::read(p).map_err(|e| e.to_string())?;
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    parse_fdt_header(&name, &data)
}

fn resolve_dtc() -> String {
    if let Ok(bin) = crate::tools::bin("dtc", "dtc.exe") {
        return bin.to_string_lossy().to_string();
    }
    if let Ok(out) = Command::new("where.exe").arg("dtc").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(first) = s.lines().next() {
                return first.trim().to_string();
            }
        }
    }
    "dtc".to_string()
}

/// Декомпиляция DTB ➔ DTS через системный или встроенный dtc
#[tauri::command]
pub fn dtb_decompile_to_dts(dtb_path: String, out_dts_path: String) -> Result<String, String> {
    let dtb = Path::new(&dtb_path);
    if !dtb.is_file() {
        return Err(format!("Исходный DTB не найден: {dtb_path}"));
    }

    let dtc_cmd = resolve_dtc();

    let output = Command::new(&dtc_cmd)
        .arg("-I")
        .arg("dtb")
        .arg("-O")
        .arg("dts")
        .arg("-o")
        .arg(&out_dts_path)
        .arg(&dtb_path)
        .output();

    match output {
        Ok(out) => {
            if out.status.success() {
                Ok(format!("Device Tree успешно декомпилирован в DTS: {out_dts_path}"))
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(format!("Ошибка dtc: {err}"))
            }
        }
        Err(_) => {
            Err("Утилита dtc (Device Tree Compiler) не найдена. Установите её через 'Менеджер утилит' или добавьте в PATH.".into())
        }
    }
}

/// Компиляция исходника DTS ➔ бинарный DTB
#[tauri::command]
pub fn dtb_compile_from_dts(dts_path: String, out_dtb_path: String) -> Result<String, String> {
    let dts = Path::new(&dts_path);
    if !dts.is_file() {
        return Err(format!("Исходный DTS не найден: {dts_path}"));
    }

    let dtc_cmd = resolve_dtc();

    let output = Command::new(&dtc_cmd)
        .arg("-I")
        .arg("dts")
        .arg("-O")
        .arg("dtb")
        .arg("-o")
        .arg(&out_dtb_path)
        .arg(&dts_path)
        .output();

    match output {
        Ok(out) => {
            if out.status.success() {
                Ok(format!("DTS успешно скомпилирован в DTB: {out_dtb_path}"))
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(format!("Ошибка компиляции dtc: {err}"))
            }
        }
        Err(_) => {
            Err("Утилита dtc не найдена в системе.".into())
        }
    }
}
