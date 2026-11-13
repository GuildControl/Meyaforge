//! Модуль разбора и модификации ELF-бинарников (.so библиотек и исполняемых файлов Android).
//! Извлечение символов, экспортных/импортных функций и установка заглушек (function stubs / patches).

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ElfSymbol {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub file_offset: Option<u64>,
    pub is_function: bool,
    pub is_import: bool,
    pub binding: String,
    pub section: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ElfAnalysis {
    pub file_name: String,
    pub file_size: u64,
    pub is_64bit: bool,
    pub is_little_endian: bool,
    pub architecture: String,
    pub elf_type: String,
    pub total_symbols: usize,
    pub functions: Vec<ElfSymbol>,
    pub has_backup: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PatchRequest {
    pub file_path: String,
    pub symbol_name: Option<String>,
    pub file_offset: Option<u64>,
    pub stub_type: String, // "return_0", "return_1", "return_minus_1", "return_void", "nop", "custom_hex"
    pub custom_hex: Option<String>,
    pub nop_count: Option<usize>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PatchResult {
    pub success: bool,
    pub message: String,
    pub offset: u64,
    pub old_bytes_hex: String,
    pub new_bytes_hex: String,
    pub backup_created: bool,
}

fn read_u16(buf: &[u8], offset: usize, le: bool) -> u16 {
    if offset + 2 > buf.len() {
        return 0;
    }
    let slice = &buf[offset..offset + 2];
    if le {
        u16::from_le_bytes(slice.try_into().unwrap())
    } else {
        u16::from_be_bytes(slice.try_into().unwrap())
    }
}

fn read_u32(buf: &[u8], offset: usize, le: bool) -> u32 {
    if offset + 4 > buf.len() {
        return 0;
    }
    let slice = &buf[offset..offset + 4];
    if le {
        u32::from_le_bytes(slice.try_into().unwrap())
    } else {
        u32::from_be_bytes(slice.try_into().unwrap())
    }
}

fn read_u64(buf: &[u8], offset: usize, le: bool) -> u64 {
    if offset + 8 > buf.len() {
        return 0;
    }
    let slice = &buf[offset..offset + 8];
    if le {
        u64::from_le_bytes(slice.try_into().unwrap())
    } else {
        u64::from_be_bytes(slice.try_into().unwrap())
    }
}

fn get_string_at(buf: &[u8], offset: usize) -> String {
    if offset >= buf.len() {
        return String::new();
    }
    let mut end = offset;
    while end < buf.len() && buf[end] != 0 {
        end += 1;
    }
    String::from_utf8_lossy(&buf[offset..end]).to_string()
}

pub fn analyze_elf_file(path_str: &str) -> Result<ElfAnalysis, String> {
    let p = Path::new(path_str);
    if !p.is_file() {
        return Err(format!("Файл не найден: {path_str}"));
    }

    let mut file = File::open(p).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).map_err(|e| e.to_string())?;

    if buf.len() < 52 || !buf.starts_with(b"\x7FELF") {
        return Err("Файл не является корректным ELF-бинарником".into());
    }

    let is_64bit = buf[4] == 2;
    let is_le = buf[5] == 1;
    let e_type = read_u16(&buf, 16, is_le);
    let e_machine = read_u16(&buf, 18, is_le);

    let elf_type = match e_type {
        2 => "Executable (EXEC)".into(),
        3 => "Shared Object (DYN / .so)".into(),
        _ => format!("Type {e_type}"),
    };

    let architecture = match e_machine {
        0x28 => "ARM (32-bit)".into(),
        0xB7 => "AArch64 (ARM 64-bit)".into(),
        0x3E => "x86_64 (AMD64)".into(),
        0x03 => "x86 (i386)".into(),
        0x08 => "MIPS".into(),
        0xF3 => "RISC-V".into(),
        other => format!("Unknown ({:#x})", other),
    };

    struct Section {
        name: String,
        sh_type: u32,
        sh_addr: u64,
        sh_offset: u64,
        sh_size: u64,
        sh_link: u32,
        sh_entsize: u64,
    }

    let mut sections = Vec::new();

    if is_64bit {
        let e_shoff = read_u64(&buf, 40, is_le) as usize;
        let e_shentsize = read_u16(&buf, 58, is_le) as usize;
        let e_shnum = read_u16(&buf, 60, is_le) as usize;
        let e_shstrndx = read_u16(&buf, 62, is_le) as usize;

        // Предварительно считываем таблицу имен секций
        let mut shstrtab_offset = 0;
        if e_shstrndx < e_shnum && e_shoff + (e_shstrndx + 1) * e_shentsize <= buf.len() {
            let hdr = e_shoff + e_shstrndx * e_shentsize;
            shstrtab_offset = read_u64(&buf, hdr + 24, is_le) as usize;
        }

        for i in 0..e_shnum {
            let offset = e_shoff + i * e_shentsize;
            if offset + 64 > buf.len() {
                break;
            }
            let name_idx = read_u32(&buf, offset, is_le) as usize;
            let sh_type = read_u32(&buf, offset + 4, is_le);
            let sh_addr = read_u64(&buf, offset + 16, is_le);
            let sh_offset = read_u64(&buf, offset + 24, is_le);
            let sh_size = read_u64(&buf, offset + 32, is_le);
            let sh_link = read_u32(&buf, offset + 40, is_le);
            let sh_entsize = read_u64(&buf, offset + 56, is_le);

            let name = if shstrtab_offset > 0 {
                get_string_at(&buf, shstrtab_offset + name_idx)
            } else {
                format!(".sec{i}")
            };

            sections.push(Section {
                name,
                sh_type,
                sh_addr,
                sh_offset,
                sh_size,
                sh_link,
                sh_entsize,
            });
        }
    } else {
        // 32-bit ELF
        let e_shoff = read_u32(&buf, 32, is_le) as usize;
        let e_shentsize = read_u16(&buf, 46, is_le) as usize;
        let e_shnum = read_u16(&buf, 48, is_le) as usize;
        let e_shstrndx = read_u16(&buf, 50, is_le) as usize;

        let mut shstrtab_offset = 0;
        if e_shstrndx < e_shnum && e_shoff + (e_shstrndx + 1) * e_shentsize <= buf.len() {
            let hdr = e_shoff + e_shstrndx * e_shentsize;
            shstrtab_offset = read_u32(&buf, hdr + 16, is_le) as usize;
        }

        for i in 0..e_shnum {
            let offset = e_shoff + i * e_shentsize;
            if offset + 40 > buf.len() {
                break;
            }
            let name_idx = read_u32(&buf, offset, is_le) as usize;
            let sh_type = read_u32(&buf, offset + 4, is_le);
            let sh_addr = read_u32(&buf, offset + 12, is_le) as u64;
            let sh_offset = read_u32(&buf, offset + 16, is_le) as u64;
            let sh_size = read_u32(&buf, offset + 20, is_le) as u64;
            let sh_link = read_u32(&buf, offset + 24, is_le);
            let sh_entsize = read_u32(&buf, offset + 36, is_le) as u64;

            let name = if shstrtab_offset > 0 {
                get_string_at(&buf, shstrtab_offset + name_idx)
            } else {
                format!(".sec{i}")
            };

            sections.push(Section {
                name,
                sh_type,
                sh_addr,
                sh_offset,
                sh_size,
                sh_link,
                sh_entsize,
            });
        }
    }

    let mut functions = Vec::new();

    // Сканируем таблицы символов (.dynsym = 11, .symtab = 2)
    for sec in &sections {
        if sec.sh_type == 2 || sec.sh_type == 11 {
            let strtab_idx = sec.sh_link as usize;
            let strtab_offset = if strtab_idx < sections.len() {
                sections[strtab_idx].sh_offset as usize
            } else {
                0
            };

            let entsize = if sec.sh_entsize > 0 {
                sec.sh_entsize as usize
            } else if is_64bit {
                24
            } else {
                16
            };

            let count = (sec.sh_size as usize) / entsize;
            for i in 0..count {
                let sym_off = (sec.sh_offset as usize) + i * entsize;
                if sym_off + entsize > buf.len() {
                    break;
                }

                let (st_name, st_info, st_shndx, st_value, st_size) = if is_64bit {
                    let name = read_u32(&buf, sym_off, is_le) as usize;
                    let info = buf[sym_off + 4];
                    let shndx = read_u16(&buf, sym_off + 6, is_le);
                    let val = read_u64(&buf, sym_off + 8, is_le);
                    let sz = read_u64(&buf, sym_off + 16, is_le);
                    (name, info, shndx, val, sz)
                } else {
                    let name = read_u32(&buf, sym_off, is_le) as usize;
                    let val = read_u32(&buf, sym_off + 4, is_le) as u64;
                    let sz = read_u32(&buf, sym_off + 8, is_le) as u64;
                    let info = buf[sym_off + 12];
                    let shndx = read_u16(&buf, sym_off + 14, is_le);
                    (name, info, shndx, val, sz)
                };

                let sym_type = st_info & 0x0f;
                let sym_bind = st_info >> 4;
                let is_func = sym_type == 2; // STT_FUNC
                let is_import = st_shndx == 0; // SHN_UNDEF

                let name = if strtab_offset > 0 {
                    get_string_at(&buf, strtab_offset + st_name)
                } else {
                    String::new()
                };

                if name.is_empty() {
                    continue;
                }

                // Вычисляем смещение в файле
                let mut file_offset = None;
                let mut sec_name = "UND".to_string();

                if !is_import && (st_shndx as usize) < sections.len() {
                    let target_sec = &sections[st_shndx as usize];
                    sec_name = target_sec.name.clone();

                    if st_value >= target_sec.sh_addr {
                        let delta = st_value - target_sec.sh_addr;
                        file_offset = Some(target_sec.sh_offset + delta);
                    }
                }

                let binding = match sym_bind {
                    0 => "LOCAL",
                    1 => "GLOBAL",
                    2 => "WEAK",
                    _ => "OTHER",
                }.to_string();

                if is_func || (sym_type == 0 && !is_import && file_offset.is_some()) {
                    functions.push(ElfSymbol {
                        name,
                        address: st_value,
                        size: st_size,
                        file_offset,
                        is_function: is_func,
                        is_import,
                        binding,
                        section: sec_name,
                    });
                }
            }
        }
    }

    // Сортируем и убираем дубликаты
    functions.sort_by(|a, b| a.name.cmp(&b.name));
    functions.dedup_by(|a, b| a.name == b.name && a.file_offset == b.file_offset);

    let backup_path = format!("{path_str}.bak");
    let has_backup = Path::new(&backup_path).is_file();

    Ok(ElfAnalysis {
        file_name: p.file_name().unwrap_or_default().to_string_lossy().to_string(),
        file_size: buf.len() as u64,
        is_64bit,
        is_little_endian: is_le,
        architecture,
        elf_type,
        total_symbols: functions.len(),
        functions,
        has_backup,
    })
}

/// Установка заглушки на функцию или по смещению
pub fn patch_elf(req: PatchRequest) -> Result<PatchResult, String> {
    let p = Path::new(&req.file_path);
    if !p.is_file() {
        return Err(format!("Файл не найден: {}", req.file_path));
    }

    // Анализируем файл для получения архитектуры и адреса функции
    let analysis = analyze_elf_file(&req.file_path)?;

    let target_offset = match req.file_offset {
        Some(off) => off,
        None => {
            if let Some(ref sym) = req.symbol_name {
                let found = analysis.functions.iter().find(|f| &f.name == sym);
                match found {
                    Some(f) => match f.file_offset {
                        Some(off) => off,
                        None => return Err(format!("Символ {sym} является импортом (внешней функцией) и не имеет тела в данном файле")),
                    },
                    None => return Err(format!("Символ {sym} не найден в таблице символов")),
                }
            } else {
                return Err("Не указано ни имя функции, ни смещение".into());
            }
        }
    };

    // Создаем резервную копию .bak при первом патче
    let backup_path = format!("{}.bak", req.file_path);
    let mut backup_created = false;
    if !Path::new(&backup_path).exists() {
        fs::copy(&req.file_path, &backup_path).map_err(|e| format!("Не удалось создать бэкап: {e}"))?;
        backup_created = true;
    }

    // Формируем байты патча в зависимости от архитектуры
    let is_aarch64 = analysis.architecture.contains("AArch64");
    let is_arm32 = analysis.architecture.contains("ARM (32-bit)");
    let is_x86_64 = analysis.architecture.contains("x86_64");
    let is_x86 = analysis.architecture.contains("x86 (i386)");

    let patch_bytes: Vec<u8> = match req.stub_type.as_str() {
        "return_0" => {
            if is_aarch64 {
                // mov w0, #0; ret
                vec![0x00, 0x00, 0x80, 0x52, 0xc0, 0x03, 0x5f, 0xd6]
            } else if is_arm32 {
                // ARM: mov r0, #0; bx lr
                vec![0x00, 0x00, 0xa0, 0xe3, 0x1e, 0xff, 0x2f, 0xe1]
            } else if is_x86_64 || is_x86 {
                // xor eax, eax; ret
                vec![0x31, 0xc0, 0xc3]
            } else {
                // generic fallback ret
                vec![0xc0, 0x03, 0x5f, 0xd6]
            }
        }
        "return_1" => {
            if is_aarch64 {
                // mov w0, #1; ret
                vec![0x20, 0x00, 0x80, 0x52, 0xc0, 0x03, 0x5f, 0xd6]
            } else if is_arm32 {
                // ARM: mov r0, #1; bx lr
                vec![0x01, 0x00, 0xa0, 0xe3, 0x1e, 0xff, 0x2f, 0xe1]
            } else if is_x86_64 || is_x86 {
                // mov eax, 1; ret
                vec![0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3]
            } else {
                vec![0x20, 0x00, 0x80, 0x52, 0xc0, 0x03, 0x5f, 0xd6]
            }
        }
        "return_minus_1" => {
            if is_aarch64 {
                // movn w0, #0; ret (w0 = 0xFFFFFFFF = -1)
                vec![0x00, 0x00, 0x80, 0x12, 0xc0, 0x03, 0x5f, 0xd6]
            } else if is_arm32 {
                // mvn r0, #0; bx lr
                vec![0x00, 0x00, 0xe0, 0xe3, 0x1e, 0xff, 0x2f, 0xe1]
            } else if is_x86_64 || is_x86 {
                // mov eax, -1; ret
                vec![0xb8, 0xff, 0xff, 0xff, 0xff, 0xc3]
            } else {
                vec![0xc0, 0x03, 0x5f, 0xd6]
            }
        }
        "return_void" => {
            if is_aarch64 {
                // ret
                vec![0xc0, 0x03, 0x5f, 0xd6]
            } else if is_arm32 {
                // bx lr
                vec![0x1e, 0xff, 0x2f, 0xe1]
            } else if is_x86_64 || is_x86 {
                // ret
                vec![0xc3]
            } else {
                vec![0xc0, 0x03, 0x5f, 0xd6]
            }
        }
        "nop" => {
            let count = req.nop_count.unwrap_or(4);
            if is_aarch64 {
                // nop = 1f 20 03 d5 (4 байта)
                let instructions = (count + 3) / 4;
                let mut v = Vec::with_capacity(instructions * 4);
                for _ in 0..instructions {
                    v.extend_from_slice(&[0x1f, 0x20, 0x03, 0xd5]);
                }
                v
            } else if is_arm32 {
                // ARM nop = 00 00 a0 e1 (4 байта)
                let instructions = (count + 3) / 4;
                let mut v = Vec::with_capacity(instructions * 4);
                for _ in 0..instructions {
                    v.extend_from_slice(&[0x00, 0x00, 0xa0, 0xe1]);
                }
                v
            } else {
                // x86 nop = 0x90
                vec![0x90; count]
            }
        }
        "custom_hex" => {
            let hex_str = req.custom_hex.ok_or("Не передана шестнадцатеричная строка custom_hex")?;
            let clean: String = hex_str.chars().filter(|c| !c.is_whitespace()).collect();
            if clean.len() % 2 != 0 {
                return Err("Длина hex-строки должна быть четной".into());
            }
            (0..clean.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).map_err(|e| format!("Неверный hex: {e}")))
                .collect::<Result<Vec<u8>, String>>()?
        }
        other => return Err(format!("Неизвестный тип заглушки: {other}")),
    };

    // Читаем старые байты и записываем новые
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&req.file_path)
        .map_err(|e| e.to_string())?;

    file.seek(SeekFrom::Start(target_offset)).map_err(|e| e.to_string())?;
    let mut old_bytes = vec![0u8; patch_bytes.len()];
    let read_len = file.read(&mut old_bytes).map_err(|e| e.to_string())?;
    old_bytes.truncate(read_len);

    file.seek(SeekFrom::Start(target_offset)).map_err(|e| e.to_string())?;
    file.write_all(&patch_bytes).map_err(|e| e.to_string())?;

    let old_hex = old_bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
    let new_hex = patch_bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");

    let target_desc = req.symbol_name.unwrap_or_else(|| format!("{:#X}", target_offset));

    Ok(PatchResult {
        success: true,
        message: format!("Функция/смещение {} успешно пропатчена ({})", target_desc, req.stub_type),
        offset: target_offset,
        old_bytes_hex: old_hex,
        new_bytes_hex: new_hex,
        backup_created,
    })
}

/// Восстановление файла из резервной копии .bak
pub fn restore_backup(file_path: &str) -> Result<String, String> {
    let bak = format!("{file_path}.bak");
    if !Path::new(&bak).is_file() {
        return Err(format!("Файл резервной копии не найден: {bak}"));
    }

    fs::copy(&bak, file_path).map_err(|e| format!("Ошибка восстановления: {e}"))?;
    Ok("Файл успешно восстановлен из резервной копии (.bak)".into())
}


#[tauri::command]
pub fn elf_analyze(file_path: String) -> Result<ElfAnalysis, String> {
    analyze_elf_file(&file_path)
}

#[tauri::command]
pub fn elf_apply_patch(req: PatchRequest) -> Result<PatchResult, String> {
    patch_elf(req)
}

#[tauri::command]
pub fn elf_restore_backup(file_path: String) -> Result<String, String> {
    restore_backup(&file_path)
}


#[tauri::command]
pub fn elf_patch_string(file_path: String, old_str: String, new_str: String) -> Result<String, String> {
    let p = Path::new(&file_path);
    if !p.is_file() {
        return Err(format!("Файл не найден: {file_path}"));
    }
    if new_str.len() > old_str.len() {
        return Err(format!("Длина новой строки ({}) не может превышать длину исходной строки ({}) во избежание сдвига структуры ELF", new_str.len(), old_str.len()));
    }
    let bak = format!("{file_path}.bak");
    if !Path::new(&bak).exists() {
        let _ = fs::copy(&file_path, &bak);
    }
    let mut data = fs::read(&file_path).map_err(|e| e.to_string())?;
    let old_bytes = old_str.as_bytes();
    let mut new_bytes = new_str.as_bytes().to_vec();
    while new_bytes.len() < old_bytes.len() {
        new_bytes.push(0);
    }
    let mut count = 0;
    for i in 0..=(data.len().saturating_sub(old_bytes.len())) {
        if &data[i..i+old_bytes.len()] == old_bytes {
            data[i..i+old_bytes.len()].copy_from_slice(&new_bytes);
            count += 1;
        }
    }
    if count == 0 {
        return Err("Строка не найдена в бинарном файле".into());
    }
    fs::write(&file_path, &data).map_err(|e| e.to_string())?;
    Ok(format!("Успешно заменено {count} вхождений строки «{old_str}» ➔ «{new_str}» (бэкап .bak создан)"))
}
