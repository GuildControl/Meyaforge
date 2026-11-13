//! Модуль патчинга boot.img, recovery.img, vendor_boot, ramdisk и fstab.
//! Поддерживает: SELinux Permissive, отключение DM-Verity/AVB, отключение ForceEncrypt,
//! патчинг adb/insecure свойств в default.prop, ручное редактирование cmdline.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BootPatchOptions {
    pub selinux_permissive: bool,
    pub disable_dm_verity: bool,
    pub disable_force_encrypt: bool,
    pub enable_adb_root: bool,
    pub custom_cmdline: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BootDetails {
    pub folder_name: String,
    pub has_kernel: bool,
    pub has_ramdisk: bool,
    pub has_header: bool,
    pub has_dtb: bool,
    pub cmdline: String,
    pub os_version: String,
    pub os_patch_level: String,
    pub fstab_files: Vec<String>,
    pub prop_files: Vec<String>,
}

/// Поиск всех файлов fstab и prop в каталоге
fn scan_configs(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut fstabs = Vec::new();
    let mut props = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let (f, pr) = scan_configs(&p);
                fstabs.extend(f);
                props.extend(pr);
            } else if p.is_file() {
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                if name.starts_with("fstab") || name.ends_with(".fstab") {
                    fstabs.push(p.clone());
                } else if name == "default.prop" || name == "prop.default" || name.ends_with(".prop") {
                    props.push(p);
                }
            }
        }
    }

    (fstabs, props)
}

/// Получение информации о распакованном boot/recovery
pub fn inspect_boot_dir(dir_path: &str) -> Result<BootDetails, String> {
    let base = Path::new(dir_path);
    if !base.is_dir() {
        return Err(format!("Папка не существует: {dir_path}"));
    }

    let folder_name = base.file_name().unwrap_or_default().to_string_lossy().to_string();
    let has_kernel = base.join("kernel").is_file();
    let has_ramdisk = base.join("ramdisk.cpio").is_file() || base.join("ramdisk").is_dir();
    let header_path = base.join("header");
    let has_header = header_path.is_file();
    let has_dtb = base.join("dtb").is_file();

    let mut cmdline = String::new();
    let mut os_version = String::new();
    let mut os_patch_level = String::new();

    if has_header {
        if let Ok(content) = fs::read_to_string(&header_path) {
            for line in content.lines() {
                let line_trim = line.trim();
                if let Some(val) = line_trim.strip_prefix("cmdline=") {
                    cmdline = val.to_string();
                } else if let Some(val) = line_trim.strip_prefix("os_version=") {
                    os_version = val.to_string();
                } else if let Some(val) = line_trim.strip_prefix("os_patch_level=") {
                    os_patch_level = val.to_string();
                }
            }
        }
    }

    let (fstabs, props) = scan_configs(base);

    Ok(BootDetails {
        folder_name,
        has_kernel,
        has_ramdisk,
        has_header,
        has_dtb,
        cmdline,
        os_version,
        os_patch_level,
        fstab_files: fstabs.into_iter().map(|p| p.to_string_lossy().to_string()).collect(),
        prop_files: props.into_iter().map(|p| p.to_string_lossy().to_string()).collect(),
    })
}

/// Применение комплекса патчей к распакованному boot образу
pub fn apply_patches(dir_path: &str, opts: BootPatchOptions) -> Result<Vec<String>, String> {
    let base = Path::new(dir_path);
    if !base.is_dir() {
        return Err(format!("Каталог не найден: {dir_path}"));
    }

    let mut log = Vec::new();
    let header_path = base.join("header");

    // 1. Патчинг header (cmdline)
    if header_path.is_file() {
        let content = fs::read_to_string(&header_path).map_err(|e| e.to_string())?;
        let mut new_lines = Vec::new();
        let mut found_cmdline = false;

        for line in content.lines() {
            if line.starts_with("cmdline=") {
                found_cmdline = true;
                let mut cur_cmd = line.strip_prefix("cmdline=").unwrap_or("").to_string();

                if let Some(ref custom) = opts.custom_cmdline {
                    cur_cmd = custom.clone();
                    log.push("Применен пользовательский cmdline".into());
                }

                if opts.selinux_permissive {
                    if cur_cmd.contains("androidboot.selinux=enforcing") {
                        cur_cmd = cur_cmd.replace("androidboot.selinux=enforcing", "androidboot.selinux=permissive");
                    } else if !cur_cmd.contains("androidboot.selinux=permissive") {
                        cur_cmd.push_str(" androidboot.selinux=permissive");
                    }
                    if !cur_cmd.contains("enforcing=0") {
                        cur_cmd.push_str(" enforcing=0");
                    }
                    log.push("Cmdline: включен SELinux Permissive (androidboot.selinux=permissive enforcing=0)".into());
                }

                if opts.disable_dm_verity {
                    if cur_cmd.contains("androidboot.veritymode=enforcing") {
                        cur_cmd = cur_cmd.replace("androidboot.veritymode=enforcing", "androidboot.veritymode=disabled");
                    } else if !cur_cmd.contains("androidboot.veritymode=disabled") {
                        cur_cmd.push_str(" androidboot.veritymode=disabled");
                    }
                    log.push("Cmdline: отключен veritymode (androidboot.veritymode=disabled)".into());
                }

                new_lines.push(format!("cmdline={}", cur_cmd.trim()));
            } else {
                new_lines.push(line.to_string());
            }
        }

        if !found_cmdline && (opts.selinux_permissive || opts.disable_dm_verity) {
            let mut cur_cmd = String::new();
            if opts.selinux_permissive {
                cur_cmd.push_str("androidboot.selinux=permissive enforcing=0 ");
            }
            if opts.disable_dm_verity {
                cur_cmd.push_str("androidboot.veritymode=disabled ");
            }
            new_lines.push(format!("cmdline={}", cur_cmd.trim()));
        }

        fs::write(&header_path, new_lines.join("\n")).map_err(|e| e.to_string())?;
        log.push("Обновлен файл заголовка MagiskBoot (header)".into());
    }

    // 2. Сканирование и патчинг fstab
    let (fstabs, props) = scan_configs(base);

    if opts.disable_dm_verity || opts.disable_force_encrypt {
        for fstab in fstabs {
            let rel = fstab.strip_prefix(base).unwrap_or(&fstab).to_string_lossy().to_string();
            if let Ok(content) = fs::read_to_string(&fstab) {
                let mut modified = false;
                let mut out_lines = Vec::new();

                for line in content.lines() {
                    let mut l = line.to_string();

                    if opts.disable_dm_verity {
                        if l.contains(",verify") {
                            l = l.replace(",verify", "");
                            modified = true;
                        }
                        if l.contains("verify,") {
                            l = l.replace("verify,", "");
                            modified = true;
                        }
                        if l.contains(",avb") {
                            // удаляем ,avb или ,avb=...
                            let parts: Vec<&str> = l.split_whitespace().collect();
                            if parts.len() >= 4 {
                                let mut flags: Vec<&str> = parts[3].split(',').collect();
                                flags.retain(|f| !f.starts_with("avb") && *f != "verify" && *f != "support_scfs");
                                let new_flags = flags.join(",");
                                l = l.replace(parts[3], &new_flags);
                                modified = true;
                            }
                        }
                    }

                    if opts.disable_force_encrypt {
                        let replacements = [
                            ("fileencryption=", "encryptable="),
                            ("forceencrypt=", "encryptable="),
                            ("forcefde=", "encryptable="),
                        ];
                        for (from, to) in replacements {
                            if l.contains(from) {
                                l = l.replace(from, to);
                                modified = true;
                            }
                        }
                    }

                    out_lines.push(l);
                }

                if modified {
                    if fs::write(&fstab, out_lines.join("\n")).is_ok() {
                        log.push(format!("Патчен fstab: {rel} (AVB/Verity/ForceEncrypt отключены)"));
                    }
                }
            }
        }
    }

    // 3. Патчинг prop-файлов (ADB Root / Insecure)
    if opts.enable_adb_root {
        for prop in props {
            let rel = prop.strip_prefix(base).unwrap_or(&prop).to_string_lossy().to_string();
            if let Ok(content) = fs::read_to_string(&prop) {
                let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                let mut replaced_keys = std::collections::HashSet::new();

                for line in &mut lines {
                    if line.starts_with("ro.secure=") {
                        *line = "ro.secure=0".into();
                        replaced_keys.insert("ro.secure");
                    } else if line.starts_with("ro.adb.secure=") {
                        *line = "ro.adb.secure=0".into();
                        replaced_keys.insert("ro.adb.secure");
                    } else if line.starts_with("ro.debuggable=") {
                        *line = "ro.debuggable=1".into();
                        replaced_keys.insert("ro.debuggable");
                    } else if line.starts_with("persist.sys.usb.config=") {
                        *line = "persist.sys.usb.config=adb".into();
                        replaced_keys.insert("persist.sys.usb.config");
                    }
                }

                if !replaced_keys.contains("ro.secure") {
                    lines.push("ro.secure=0".into());
                }
                if !replaced_keys.contains("ro.adb.secure") {
                    lines.push("ro.adb.secure=0".into());
                }
                if !replaced_keys.contains("ro.debuggable") {
                    lines.push("ro.debuggable=1".into());
                }

                if fs::write(&prop, lines.join("\n")).is_ok() {
                    log.push(format!("Патчен {rel}: ro.secure=0, ro.debuggable=1, ro.adb.secure=0"));
                }
            }
        }
    }

    if log.is_empty() {
        log.push("Никаких изменений не потребовалось или файлы не найдены".into());
    }

    Ok(log)
}


#[tauri::command]
pub fn boot_inspect_extracted(project: String, target_name: String) -> Result<BootDetails, String> {
    let pdir = crate::util::dir("projects").join(&project);
    let target_dir = pdir.join("extracted").join(&target_name);
    inspect_boot_dir(&target_dir.to_string_lossy())
}

#[tauri::command]
pub fn boot_apply_patches(project: String, target_name: String, options: BootPatchOptions) -> Result<Vec<String>, String> {
    let pdir = crate::util::dir("projects").join(&project);
    let target_dir = pdir.join("extracted").join(&target_name);
    apply_patches(&target_dir.to_string_lossy(), options)
}


fn hex_to_bytes(s: &str) -> Result<Vec<u8>, String> {
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if clean.len() % 2 != 0 {
        return Err("Длина HEX строки должна быть четной".into());
    }
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[tauri::command]
pub fn boot_kernel_hex_patch(project: String, target_name: String, pattern_hex: String, replace_hex: String) -> Result<String, String> {
    let pdir = crate::util::dir("projects").join(&project);
    let kernel_path = pdir.join("extracted").join(&target_name).join("kernel");
    if !kernel_path.is_file() {
        return Err(format!("Файл kernel не найден в extracted/{target_name}"));
    }
    let bak = pdir.join("extracted").join(&target_name).join("kernel.bak");
    if !bak.exists() {
        let _ = fs::copy(&kernel_path, &bak);
    }
    let mut data = fs::read(&kernel_path).map_err(|e| e.to_string())?;
    let pat = hex_to_bytes(&pattern_hex)?;
    let rep = hex_to_bytes(&replace_hex)?;
    if pat.len() != rep.len() {
        return Err("Длина шаблона поиска и замены в HEX должна совпадать".into());
    }
    let mut count = 0;
    for i in 0..=(data.len().saturating_sub(pat.len())) {
        if &data[i..i+pat.len()] == &pat[..] {
            data[i..i+pat.len()].copy_from_slice(&rep);
            count += 1;
        }
    }
    if count == 0 {
        return Err("Шаблон HEX не найден в файле kernel".into());
    }
    fs::write(&kernel_path, &data).map_err(|e| e.to_string())?;
    Ok(format!("Успешно заменено {count} вхождений в ядре (бэкап kernel.bak создан)"))
}
