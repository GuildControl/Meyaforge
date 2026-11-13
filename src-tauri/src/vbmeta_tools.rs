//! Модуль работы с VBMeta (Android Verified Boot / AVB 2.0/3.0).
//! Поддерживает: чтение заголовка AVB0, анализ флагов, создание пустой vbmeta
//! с отключенной проверкой (--disable-verity --disable-verification) и патчинг существующих образов.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

pub const AVB_MAGIC: &[u8; 4] = b"AVB0";
pub const AVB_HEADER_SIZE: usize = 256;

// Флаги AVB VBMeta
pub const AVB_VBMETA_IMAGE_FLAGS_HASHTREE_DISABLED: u32 = 1 << 0;
pub const AVB_VBMETA_IMAGE_FLAGS_VERIFICATION_DISABLED: u32 = 1 << 1;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VbmetaInfo {
    pub file_name: String,
    pub file_size: u64,
    pub is_valid: bool,
    pub required_libavb_version: String,
    pub flags: u32,
    pub verity_disabled: bool,
    pub verification_disabled: bool,
    pub release_string: String,
    pub algorithm_type: u32,
    pub algorithm_name: String,
    pub partitions_in_vbmeta: Vec<String>,
}

fn u32be(b: &[u8], offset: usize) -> u32 {
    if offset + 4 > b.len() {
        return 0;
    }
    u32::from_be_bytes(b[offset..offset + 4].try_into().unwrap())
}

fn u64be(b: &[u8], offset: usize) -> u64 {
    if offset + 8 > b.len() {
        return 0;
    }
    u64::from_be_bytes(b[offset..offset + 8].try_into().unwrap())
}

fn algorithm_to_string(alg: u32) -> &'static str {
    match alg {
        0 => "NONE",
        1 => "SHA256_RSA2048",
        2 => "SHA256_RSA4096",
        3 => "SHA256_RSA8192",
        4 => "SHA512_RSA2048",
        5 => "SHA512_RSA4096",
        6 => "SHA512_RSA8192",
        _ => "CUSTOM/UNKNOWN",
    }
}

pub fn inspect_vbmeta_bytes(name: &str, data: &[u8]) -> Result<VbmetaInfo, String> {
    if data.len() < AVB_HEADER_SIZE {
        return Err("Размер файла меньше минимального заголовка AVB0 (256 байт)".into());
    }

    if &data[0..4] != AVB_MAGIC {
        return Err("Сигнатура AVB0 не найдена в начале файла".into());
    }

    let major = u32be(data, 4);
    let minor = u32be(data, 8);
    let auth_data_block_size = u64be(data, 16);
    let aux_data_block_size = u64be(data, 24);
    let algorithm = u32be(data, 32);
    let flags = u32be(data, 120);

    let verity_disabled = (flags & AVB_VBMETA_IMAGE_FLAGS_HASHTREE_DISABLED) != 0;
    let verification_disabled = (flags & AVB_VBMETA_IMAGE_FLAGS_VERIFICATION_DISABLED) != 0;

    // Считываем release_string (offset 72..120, null-terminated)
    let release_raw = &data[72..120.min(data.len())];
    let release_end = release_raw.iter().position(|&c| c == 0).unwrap_or(release_raw.len());
    let release_string = String::from_utf8_lossy(&release_raw[..release_end]).to_string();

    // Извлечение имен разделов из блока aux_data
    let mut partitions = Vec::new();
    let aux_start = 256 + auth_data_block_size as usize;
    let aux_end = aux_start + (aux_data_block_size as usize).min(data.len().saturating_sub(aux_start));

    if aux_start < data.len() && aux_end <= data.len() {
        let aux_slice = &data[aux_start..aux_end];
        let known = ["boot", "system", "vendor", "product", "system_ext", "odm", "dtbo", "recovery", "init_boot", "vendor_boot"];
        for part in known {
            if aux_slice.windows(part.len()).any(|w| w == part.as_bytes()) {
                partitions.push(part.to_string());
            }
        }
    }

    Ok(VbmetaInfo {
        file_name: name.to_string(),
        file_size: data.len() as u64,
        is_valid: true,
        required_libavb_version: format!("{major}.{minor}"),
        flags,
        verity_disabled,
        verification_disabled,
        release_string,
        algorithm_type: algorithm,
        algorithm_name: algorithm_to_string(algorithm).into(),
        partitions_in_vbmeta: partitions,
    })
}

#[tauri::command]
pub fn vbmeta_inspect(file_path: String) -> Result<VbmetaInfo, String> {
    let p = Path::new(&file_path);
    if !p.is_file() {
        return Err(format!("Файл не найден: {file_path}"));
    }
    let data = fs::read(p).map_err(|e| e.to_string())?;
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    inspect_vbmeta_bytes(&name, &data)
}

/// Патчинг существующего образа vbmeta (включение флагов disable-verity и disable-verification)
#[tauri::command]
pub fn vbmeta_patch_disable_flags(file_path: String) -> Result<String, String> {
    let p = Path::new(&file_path);
    if !p.is_file() {
        return Err(format!("Файл не найден: {file_path}"));
    }

    // Создаем резервную копию
    let bak = format!("{file_path}.bak");
    if !Path::new(&bak).exists() {
        let _ = fs::copy(p, &bak);
    }

    let mut data = fs::read(p).map_err(|e| e.to_string())?;
    if data.len() < AVB_HEADER_SIZE || &data[0..4] != AVB_MAGIC {
        return Err("Файл не является корректным VBMeta (AVB0)".into());
    }

    // Смещение flags в заголовке AVB0 = 120 (4 байта Big Endian)
    let cur_flags = u32be(&data, 120);
    let new_flags = cur_flags
        | AVB_VBMETA_IMAGE_FLAGS_HASHTREE_DISABLED
        | AVB_VBMETA_IMAGE_FLAGS_VERIFICATION_DISABLED;

    let flag_bytes = new_flags.to_be_bytes();
    data[120..124].copy_from_slice(&flag_bytes);

    fs::write(p, &data).map_err(|e| e.to_string())?;
    Ok(format!("VBMeta успешно пропатчен: flags={new_flags:#010X} (verity & verification disabled, бэкап .bak сохранен)"))
}

/// Генерация чистого пустого vbmeta.img с отключенными проверками (размером 4 КБ)
#[tauri::command]
pub fn vbmeta_generate_blank(out_path: String) -> Result<String, String> {
    let mut buf = vec![0u8; 4096];

    // Сигнатура "AVB0"
    buf[0..4].copy_from_slice(AVB_MAGIC);
    // Версия libavb 1.0 (major=1, minor=0)
    buf[4..8].copy_from_slice(&1u32.to_be_bytes());
    buf[8..12].copy_from_slice(&0u32.to_be_bytes());

    // auth_data_block_size = 0 (offset 16..24)
    // aux_data_block_size = 0 (offset 24..32)
    // algorithm_type = 0 (NONE, offset 32..36)

    // release_string = "avbtool 1.2.0 (MEYAFORGE Blank VBMeta)" (offset 72..120)
    let release = b"avbtool 1.2.0 (MEYAFORGE Blank VBMeta)";
    buf[72..72 + release.len()].copy_from_slice(release);

    // flags = HASHTREE_DISABLED (1) | VERIFICATION_DISABLED (2) = 3 (offset 120..124)
    let flags: u32 = AVB_VBMETA_IMAGE_FLAGS_HASHTREE_DISABLED | AVB_VBMETA_IMAGE_FLAGS_VERIFICATION_DISABLED;
    buf[120..124].copy_from_slice(&flags.to_be_bytes());

    // rollback_index = 0 (offset 124..132)

    fs::write(&out_path, &buf).map_err(|e| format!("Ошибка сохранения vbmeta.img: {e}"))?;
    Ok(format!("Пустой vbmeta.img с флагами --disable-verity --disable-verification создан: {out_path}"))
}
