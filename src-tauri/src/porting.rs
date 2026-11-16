//! Модуль сравнения и портирования прошивок:
//!   - Глубокое сравнение двух проектов (Сток/База vs Порт/Донор):
//!     свойства build.prop, разделы, HAL модули vendor, init-скрипты, fstab.
//!   - Мастер портирования: перенос ядра, fstab, аппаратных драйверов (камера, аудио, датчики),
//!     патчинг build.prop под целевое устройство, отключение AVB/dm-verity.

use crate::util::{dir, es, spawn_task};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
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

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct PropMap {
    pub android_version: String,
    pub sdk_level: String,
    pub security_patch: String,
    pub device: String,
    pub model: String,
    pub brand: String,
    pub manufacturer: String,
    pub cpu_abi: String,
    pub vndk_version: String,
    pub fingerprint: String,
    pub build_id: String,
    pub extra: BTreeMap<String, String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct HalModuleInfo {
    pub name: String,
    pub in_base: bool,
    pub in_port: bool,
    pub category: String, // "audio", "camera", "sensors", "display", "etc"
}

#[derive(Serialize, Clone, Debug)]
pub struct PortComparison {
    pub base_name: String,
    pub port_name: String,
    pub base_props: PropMap,
    pub port_props: PropMap,
    pub common_partitions: Vec<String>,
    pub base_only_partitions: Vec<String>,
    pub port_only_partitions: Vec<String>,
    pub hal_modules: Vec<HalModuleInfo>,
    pub compatibility_score: u32,
    pub compatibility_notes: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PortingOptions {
    pub transfer_kernel: bool,
    pub transfer_fstab: bool,
    pub sync_hal_blobs: bool,
    pub sync_firmware: bool,
    pub patch_build_props: bool,
    pub remove_avb_dm_verity: bool,
}

/// Поиск и разбор build.prop в проекте (проверяет extracted/system, extracted/system/system, etc.)
fn find_and_parse_props(project_dir: &Path) -> PropMap {
    let mut props = PropMap::default();
    let mut map: HashMap<String, String> = HashMap::new();

    let candidates = [
        project_dir.join("extracted/system/build.prop"),
        project_dir.join("extracted/system/system/build.prop"),
        project_dir.join("extracted/system_ext/build.prop"),
        project_dir.join("extracted/product/build.prop"),
        project_dir.join("extracted/vendor/build.prop"),
    ];

    for c in &candidates {
        if c.is_file() {
            if let Ok(file) = File::open(c) {
                let reader = BufReader::new(file);
                for line in reader.lines().flatten() {
                    let trim = line.trim();
                    if trim.starts_with('#') || trim.is_empty() {
                        continue;
                    }
                    if let Some((k, v)) = trim.split_once('=') {
                        map.insert(k.trim().to_string(), v.trim().to_string());
                    }
                }
            }
        }
    }

    props.android_version = map.get("ro.build.version.release")
        .or_else(|| map.get("ro.system.build.version.release"))
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.sdk_level = map.get("ro.build.version.sdk")
        .or_else(|| map.get("ro.system.build.version.sdk"))
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.security_patch = map.get("ro.build.version.security_patch")
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.device = map.get("ro.product.device")
        .or_else(|| map.get("ro.product.system.device"))
        .or_else(|| map.get("ro.product.vendor.device"))
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.model = map.get("ro.product.model")
        .or_else(|| map.get("ro.product.system.model"))
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.brand = map.get("ro.product.brand")
        .or_else(|| map.get("ro.product.system.brand"))
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.manufacturer = map.get("ro.product.manufacturer")
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.cpu_abi = map.get("ro.product.cpu.abi")
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.vndk_version = map.get("ro.vndk.version")
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.fingerprint = map.get("ro.build.fingerprint")
        .or_else(|| map.get("ro.system.build.fingerprint"))
        .cloned()
        .unwrap_or_else(|| "—".into());

    props.build_id = map.get("ro.build.display.id")
        .or_else(|| map.get("ro.build.id"))
        .cloned()
        .unwrap_or_else(|| "—".into());

    // Сохраняем первые 50 интересных свойств
    let mut extra = BTreeMap::new();
    for (k, v) in map {
        if k.starts_with("ro.") || k.starts_with("persist.") {
            extra.insert(k, v);
            if extra.len() >= 60 {
                break;
            }
        }
    }
    props.extra = extra;

    props
}

/// Получение списка всех обнаруженных разделов в проекте
fn get_project_partitions(project_dir: &Path) -> HashSet<String> {
    let mut parts = HashSet::new();

    let img_dir = project_dir.join("images");
    if let Ok(entries) = fs::read_dir(img_dir) {
        for e in entries.flatten() {
            if let Some(ext) = e.path().extension() {
                if ext == "img" {
                    let stem = e.path().file_stem().unwrap().to_string_lossy().to_string();
                    parts.insert(stem);
                }
            }
        }
    }

    let ext_dir = project_dir.join("extracted");
    if let Ok(entries) = fs::read_dir(ext_dir) {
        for e in entries.flatten() {
            if e.path().is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                parts.insert(name);
            }
        }
    }

    parts
}

/// Сканирование HAL библиотек из vendor/lib/hw и vendor/lib64/hw
fn scan_hal_modules(project_dir: &Path) -> HashSet<String> {
    let mut hals = HashSet::new();
    let dirs = [
        project_dir.join("extracted/vendor/lib/hw"),
        project_dir.join("extracted/vendor/lib64/hw"),
    ];

    for d in &dirs {
        if let Ok(entries) = fs::read_dir(d) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.ends_with(".so") {
                    hals.insert(name);
                }
            }
        }
    }
    hals
}

fn categorize_hal(name: &str) -> String {
    let l = name.to_lowercase();
    if l.contains("audio") || l.contains("sound") {
        "Аудио".into()
    } else if l.contains("camera") {
        "Камера".into()
    } else if l.contains("sensor") {
        "Датчики".into()
    } else if l.contains("gralloc") || l.contains("hwcomposer") || l.contains("display") {
        "Графика/Дисплей".into()
    } else if l.contains("bluetooth") || l.contains("bt") {
        "Bluetooth".into()
    } else if l.contains("wifi") || l.contains("wlan") {
        "Wi-Fi".into()
    } else if l.contains("light") || l.contains("led") {
        "Подсветка".into()
    } else if l.contains("fingerprint") || l.contains("biometric") {
        "Сканер отпечатков".into()
    } else {
        "Прочее".into()
    }
}

/// Сравнение двух проектов (База vs Порт)
#[tauri::command]
pub fn compare_projects(base_name: String, port_name: String) -> Result<PortComparison, String> {
    let base_dir = pdir(&base_name)?;
    let port_dir = pdir(&port_name)?;

    let base_props = find_and_parse_props(&base_dir);
    let port_props = find_and_parse_props(&port_dir);

    let base_parts = get_project_partitions(&base_dir);
    let port_parts = get_project_partitions(&port_dir);

    let mut common_partitions: Vec<String> = base_parts.intersection(&port_parts).cloned().collect();
    common_partitions.sort();

    let mut base_only_partitions: Vec<String> = base_parts.difference(&port_parts).cloned().collect();
    base_only_partitions.sort();

    let mut port_only_partitions: Vec<String> = port_parts.difference(&base_parts).cloned().collect();
    port_only_partitions.sort();

    let base_hals = scan_hal_modules(&base_dir);
    let port_hals = scan_hal_modules(&port_dir);

    let all_hals: HashSet<String> = base_hals.union(&port_hals).cloned().collect();
    let mut hal_modules = Vec::new();
    for h in all_hals {
        let in_base = base_hals.contains(&h);
        let in_port = port_hals.contains(&h);
        let category = categorize_hal(&h);
        hal_modules.push(HalModuleInfo {
            name: h,
            in_base,
            in_port,
            category,
        });
    }
    hal_modules.sort_by(|a, b| a.category.cmp(&b.category).then_with(|| a.name.cmp(&b.name)));

    // Расчет оценки совместимости (Compatibility Score)
    let mut score = 100u32;
    let mut notes = Vec::new();

    if base_props.android_version != "—" && port_props.android_version != "—" {
        if base_props.android_version == port_props.android_version {
            notes.push(format!("✓ Версии Android идентичны ({}) — идеальная совместимость фреймворка", base_props.android_version));
        } else {
            score = score.saturating_sub(25);
            notes.push(format!("⚠️ Разные версии Android: База ({}) vs Порт ({}) — потребуется адаптация бинарников vendor", base_props.android_version, port_props.android_version));
        }
    }

    if base_props.cpu_abi != "—" && port_props.cpu_abi != "—" {
        if base_props.cpu_abi == port_props.cpu_abi {
            notes.push(format!("✓ Архитектура процессора совпадает ({})", base_props.cpu_abi));
        } else {
            score = score.saturating_sub(40);
            notes.push(format!("⛔ Несовпадение архитектур: База ({}) vs Порт ({})", base_props.cpu_abi, port_props.cpu_abi));
        }
    }

    if base_props.vndk_version != "—" && port_props.vndk_version != "—" {
        if base_props.vndk_version == port_props.vndk_version {
            notes.push(format!("✓ Версия VNDK совпадает ({})", base_props.vndk_version));
        } else {
            score = score.saturating_sub(15);
            notes.push(format!("⚠️ Разные уровни VNDK: База ({}) vs Порт ({})", base_props.vndk_version, port_props.vndk_version));
        }
    }

    if !base_only_partitions.is_empty() {
        notes.push(format!("ℹ️ В базе есть специфичные разделы: {}", base_only_partitions.join(", ")));
    }

    Ok(PortComparison {
        base_name,
        port_name,
        base_props,
        port_props,
        common_partitions,
        base_only_partitions,
        port_only_partitions,
        hal_modules,
        compatibility_score: score,
        compatibility_notes: notes,
    })
}

/// Применение мастера портирования
#[tauri::command]
pub fn apply_porting_transfer(
    app: AppHandle,
    base_name: String,
    port_name: String,
    options: PortingOptions,
) -> Result<(), String> {
    let base_dir = pdir(&base_name)?;
    let port_dir = pdir(&port_name)?;

    let base_props = find_and_parse_props(&base_dir);

    spawn_task(app, format!("Портирование: {base_name} → {port_name}"), move |ctx| {
        crate::telegram::on_build_start(&format!("Порт {port_name} (база {base_name})"));
        crate::telegram::on_build_progress(&format!("Порт {port_name}"), "Инициализация мастера переноса", 10);

        // 1. Перенос ядра и boot
        if options.transfer_kernel {
            ctx.stage("[1/5] Перенос ядра, dtbo и загрузочных образов...");
            let boot_images = ["boot.img", "vendor_boot.img", "dtbo.img", "recovery.img", "vbmeta.img"];
            for img in boot_images {
                let src = base_dir.join("images").join(img);
                let dst = port_dir.join("images").join(img);
                if src.is_file() {
                    ctx.log(format!("  Копирование {img} из базы"));
                    fs::copy(&src, &dst).map_err(es)?;
                }
            }

            // Также копируем разобранную папку extracted/boot, если есть
            let ext_boot_src = base_dir.join("extracted/boot");
            let ext_boot_dst = port_dir.join("extracted/boot");
            if ext_boot_src.is_dir() {
                ctx.log("  Копирование разобранного extracted/boot из базы");
                copy_dir_recursive(&ext_boot_src, &ext_boot_dst)?;
            }
        }

        // 2. Синхронизация fstab
        if options.transfer_fstab {
            ctx.stage("[2/5] Синхронизация fstab и точек монтирования...");
            let base_vendor_etc = base_dir.join("extracted/vendor/etc");
            let port_vendor_etc = port_dir.join("extracted/vendor/etc");

            if base_vendor_etc.is_dir() && port_vendor_etc.is_dir() {
                if let Ok(entries) = fs::read_dir(&base_vendor_etc) {
                    for e in entries.flatten() {
                        let fname = e.file_name().to_string_lossy().to_string();
                        if fname.starts_with("fstab.") {
                            ctx.log(format!("  Перенос fstab: {fname}"));
                            let dst = port_vendor_etc.join(&fname);
                            fs::copy(e.path(), &dst).map_err(es)?;

                            if options.remove_avb_dm_verity {
                                ctx.log(format!("    Отключение AVB / dm-verity в {fname}"));
                                patch_fstab_remove_verity(&dst)?;
                            }
                        }
                    }
                }
            }
        }

        // 3. Синхронизация аппаратных HAL модулей
        if options.sync_hal_blobs {
            ctx.stage("[3/5] Синхронизация драйверов оборудования (vendor HAL)...");
            crate::telegram::on_build_progress(&format!("Порт {port_name}"), "Перенос драйверов HAL (камера, звук, сенсоры)", 45);

            let hal_subdirs = ["lib/hw", "lib64/hw"];
            for sub in hal_subdirs {
                let b_dir = base_dir.join("extracted/vendor").join(sub);
                let p_dir = port_dir.join("extracted/vendor").join(sub);

                if b_dir.is_dir() && p_dir.is_dir() {
                    if let Ok(entries) = fs::read_dir(&b_dir) {
                        for e in entries.flatten() {
                            let fname = e.file_name().to_string_lossy().to_string();
                            let l = fname.to_lowercase();
                            if l.contains("audio") || l.contains("camera") || l.contains("sensor") || l.contains("light") || l.contains("fingerprint") {
                                ctx.log(format!("  HAL перенос: {sub}/{fname}"));
                                let _ = fs::copy(e.path(), p_dir.join(&fname));
                            }
                        }
                    }
                }
            }
        }

        // 4. Синхронизация прошивок модема/Wi-Fi/Bluetooth (vendor/firmware)
        if options.sync_firmware {
            ctx.stage("[4/5] Синхронизация модема и радиомодулей (vendor/firmware)...");
            let b_fw = base_dir.join("extracted/vendor/firmware");
            let p_fw = port_dir.join("extracted/vendor/firmware");
            if b_fw.is_dir() {
                ctx.log("  Копирование vendor/firmware из базы");
                copy_dir_recursive(&b_fw, &p_fw)?;
            }
        }

        // 5. Патчинг build.prop под целевое устройство
        if options.patch_build_props {
            ctx.stage("[5/5] Патчинг идентификаторов устройства в build.prop...");
            crate::telegram::on_build_progress(&format!("Порт {port_name}"), "Патчинг свойств build.prop", 80);

            let prop_files = [
                port_dir.join("extracted/system/build.prop"),
                port_dir.join("extracted/system/system/build.prop"),
                port_dir.join("extracted/product/build.prop"),
            ];

            let patch_keys = [
                ("ro.product.device", &base_props.device),
                ("ro.product.model", &base_props.model),
                ("ro.product.brand", &base_props.brand),
                ("ro.product.manufacturer", &base_props.manufacturer),
                ("ro.build.product", &base_props.device),
            ];

            for pf in &prop_files {
                if pf.is_file() {
                    ctx.log(format!("  Обновление {}", pf.file_name().unwrap().to_string_lossy()));
                    patch_properties_file(pf, &patch_keys)?;
                }
            }
        }

        crate::telegram::on_build_done(&format!("Портирование {port_name}"), "Готово к сборке", "—", "", "MeyaForge", "");

        Ok(format!("Операции портирования успешно применены к проекту {port_name}! Теперь вы можете пересобрать разделы на вкладке «Распаковка / Сборка»."))
    })
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(es)?;
    if let Ok(entries) = fs::read_dir(src) {
        for e in entries.flatten() {
            let p = e.path();
            let d = dst.join(e.file_name());
            if p.is_dir() {
                copy_dir_recursive(&p, &d)?;
            } else if p.is_file() {
                let _ = fs::copy(&p, &d);
            }
        }
    }
    Ok(())
}

fn patch_fstab_remove_verity(fstab_path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(fstab_path).map_err(es)?;
    let mut out_lines = Vec::new();

    for line in content.lines() {
        if line.trim().starts_with('#') || line.trim().is_empty() {
            out_lines.push(line.to_string());
            continue;
        }

        // Удаляем флаги avb, verify, support_scfs и т.п. из колонки флагов
        let modified = line
            .replace(",avb", "")
            .replace("avb,", "")
            .replace(",verify", "")
            .replace("verify,", "")
            .replace("avb", "");

        out_lines.push(modified);
    }

    fs::write(fstab_path, out_lines.join("\n")).map_err(es)
}

fn patch_properties_file(path: &Path, patches: &[(&str, &String)]) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(es)?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    for (k, v) in patches {
        if v.is_empty() || *v == "—" {
            continue;
        }
        let prefix = format!("{k}=");
        let mut found = false;
        for line in &mut lines {
            if line.starts_with(&prefix) {
                *line = format!("{k}={v}");
                found = true;
                break;
            }
        }
        if !found {
            lines.push(format!("{k}={v}"));
        }
    }

    fs::write(path, lines.join("\n")).map_err(es)
}

// ---------------- Сравнение сохранённых профилей (устройства и прошивки из хранилища) ----------------

#[derive(Serialize, Clone, Debug)]
pub struct PropDiff {
    pub key: String,
    pub base_val: String,
    pub port_val: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct SpecRow {
    pub label: String,
    pub base_val: String,
    pub port_val: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProfileComparison {
    pub base_title: String,
    pub port_title: String,
    pub base_source: String,
    pub port_source: String,
    pub specs: Vec<SpecRow>,
    pub diffs: Vec<PropDiff>,
    pub common_partitions: Vec<String>,
    pub base_only_partitions: Vec<String>,
    pub port_only_partitions: Vec<String>,
}

fn spec_row(label: &str, base: String, port: String) -> SpecRow {
    SpecRow {
        label: label.into(),
        base_val: if base.is_empty() { "—".into() } else { base },
        port_val: if port.is_empty() { "—".into() } else { port },
    }
}

/// Сравнение двух профилей из хранилища устройств (живое устройство или прошивка)
#[tauri::command]
pub fn compare_saved_profiles(base_id: String, port_id: String) -> Result<ProfileComparison, String> {
    let base = crate::device_reader::get_saved_device(base_id)?;
    let port = crate::device_reader::get_saved_device(port_id)?;
    let b = &base.data;
    let p = &port.data;

    let yn = |v: bool| if v { "да".into() } else { "нет".into() };
    let specs = vec![
        spec_row("Модель", b.model.clone(), p.model.clone()),
        spec_row("Бренд", b.brand.clone(), p.brand.clone()),
        spec_row("Устройство (device)", b.device.clone(), p.device.clone()),
        spec_row("SoC / Платформа", b.soc.clone(), p.soc.clone()),
        spec_row("Архитектура CPU", b.cpu_abi.clone(), p.cpu_abi.clone()),
        spec_row("Android OS", b.android_version.clone(), p.android_version.clone()),
        spec_row("SDK API", b.sdk_version.clone(), p.sdk_version.clone()),
        spec_row("Патч безопасности", b.security_patch.clone(), p.security_patch.clone()),
        spec_row("Build ID", b.build_id.clone(), p.build_id.clone()),
        spec_row("Fingerprint", b.build_fingerprint.clone(), p.build_fingerprint.clone()),
        spec_row("SELinux", b.selinux.clone(), p.selinux.clone()),
        spec_row("Слоты A/B", yn(b.is_ab), yn(p.is_ab)),
        spec_row("Динамические разделы", yn(b.is_dynamic), yn(p.is_dynamic)),
        spec_row("Разделов в профиле", b.partitions.len().to_string(), p.partitions.len().to_string()),
        spec_row("Свойств build.prop", b.build_props.len().to_string(), p.build_props.len().to_string()),
    ];

    // Различия свойств
    let mut keys: std::collections::BTreeSet<&String> = b.build_props.keys().collect();
    keys.extend(p.build_props.keys());
    let mut diffs = Vec::new();
    for k in keys {
        let bv = b.build_props.get(k).cloned().unwrap_or_default();
        let pv = p.build_props.get(k).cloned().unwrap_or_default();
        if bv != pv {
            diffs.push(PropDiff {
                key: k.clone(),
                base_val: if bv.is_empty() { "—".into() } else { bv },
                port_val: if pv.is_empty() { "—".into() } else { pv },
            });
            if diffs.len() >= 400 {
                break;
            }
        }
    }

    // Разделы
    let b_parts: std::collections::BTreeSet<String> = b.partitions.iter().map(|x| x.name.clone()).collect();
    let p_parts: std::collections::BTreeSet<String> = p.partitions.iter().map(|x| x.name.clone()).collect();
    let common_partitions = b_parts.intersection(&p_parts).cloned().collect();
    let base_only_partitions = b_parts.difference(&p_parts).cloned().collect();
    let port_only_partitions = p_parts.difference(&b_parts).cloned().collect();

    Ok(ProfileComparison {
        base_title: base.title.clone(),
        port_title: port.title.clone(),
        base_source: base.source.clone(),
        port_source: port.source.clone(),
        specs,
        diffs,
        common_partitions,
        base_only_partitions,
        port_only_partitions,
    })
}
