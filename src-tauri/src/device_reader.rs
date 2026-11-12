//! Модуль глубокого сбора информации об Android-устройстве (ADB / Fastboot / Bootloader / Recovery)
//! и локальное хранилище профилей устройств (data/devices рядом с приложением).

use crate::tools;
use crate::util::{dir, es, hidden, spawn_task};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DeviceMode {
    pub serial: String,
    pub mode: String, // "system" (adb), "recovery", "sideload", "fastboot", "fastbootd", "unauthorized", "offline", "none"
    pub description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PartitionInfo {
    pub name: String,
    pub filesystem: String,
    pub size_human: String,
    pub mount_point: String,
    pub block_device: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ComprehensiveDeviceInfo {
    pub mode: DeviceMode,
    pub model: String,
    pub brand: String,
    pub manufacturer: String,
    pub device: String,
    pub product: String,
    pub soc: String,
    pub cpu_abi: String,
    pub android_version: String,
    pub sdk_version: String,
    pub security_patch: String,
    pub build_id: String,
    pub build_fingerprint: String,
    pub display: String,
    pub baseband: String,
    pub slot: String,        // a, b, or none
    pub is_ab: bool,
    pub is_dynamic: bool,    // Super partition / dynamic partitions
    pub is_root: bool,
    pub selinux: String,
    pub kernel: String,
    pub kernel_cmdline: String,
    pub battery: String,
    pub memory_info: String,
    pub recovery_type: String,
    pub bootloader_status: String, // Unlocked / Locked
    pub oem_device_info: String,
    pub partitions: Vec<PartitionInfo>,
    pub build_props: BTreeMap<String, String>,
    pub fastboot_vars: BTreeMap<String, String>,
    pub raw_fstab: String,
    #[serde(default)]
    pub android_settings: BTreeMap<String, DiagnosticOutput>,
    #[serde(default)]
    pub android_diagnostics: BTreeMap<String, DiagnosticOutput>,
    #[serde(default)]
    pub boot_images: BTreeMap<String, BootImageResult>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DiagnosticOutput {
    pub output: String,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BootImageResult {
    pub partition: Option<String>,
    pub relative_path: Option<String>,
    pub size_bytes: Option<u64>,
    pub error: Option<String>,
}

/// Профиль устройства, сохраняемый в локальное хранилище
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SavedDeviceProfile {
    pub id: String,
    pub title: String,
    pub timestamp: u64,
    pub date_human: String,
    pub success: bool,
    pub failure_stage: Option<String>,
    pub failure_error: Option<String>,
    pub data: ComprehensiveDeviceInfo,
}

/// Краткая сводка профиля для списка в интерфейсе
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DeviceSummary {
    pub id: String,
    pub title: String,
    pub brand: String,
    pub model: String,
    pub serial: String,
    pub android_version: String,
    pub timestamp: u64,
    pub date_human: String,
    pub success: bool,
    pub failure_stage: Option<String>,
    pub failure_error: Option<String>,
    pub partitions_count: usize,
    pub props_count: usize,
}

fn devices_dir() -> PathBuf {
    dir("devices")
}

fn current_timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn format_epoch_seconds(secs: u64) -> String {
    let days = secs / 86400;
    let rem_secs = secs % 86400;
    let hours = rem_secs / 3600;
    let minutes = (rem_secs % 3600) / 60;
    let seconds = rem_secs % 60;

    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02} {hours:02}:{minutes:02}:{seconds:02} UTC")
}

fn adb_bin() -> Result<PathBuf, String> {
    if let Ok(p) = tools::bin("platform-tools", "adb.exe") {
        return Ok(p);
    }
    if let Ok(out) = Command::new("where.exe").arg("adb").output() {
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
    Err("adb не найден. Установите «Android Platform Tools» в разделе «Инструменты»".into())
}

fn fastboot_bin() -> Result<PathBuf, String> {
    if let Ok(p) = tools::bin("platform-tools", "fastboot.exe") {
        return Ok(p);
    }
    if let Ok(out) = Command::new("where.exe").arg("fastboot").output() {
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
    Err("fastboot не найден. Установите «Android Platform Tools» в разделе «Инструменты»".into())
}

fn exec_cmd(exe: &PathBuf, args: &[&str]) -> (bool, String) {
    let (ok, output) = exec_cmd_raw(exe, args);
    (ok, output.trim().to_string())
}

fn exec_device(exe: &Path, serial: &str, args: &[&str]) -> (bool, String) {
    let mut command = vec!["-s", serial];
    command.extend_from_slice(args);
    let (ok, output) = exec_cmd_raw(exe, &command);
    (ok, output.trim().to_string())
}

fn read_adb_properties(adb: &Path, serial: &str) -> Result<BTreeMap<String, String>, String> {
    for attempt in 0..3 {
        let (ok, output) = exec_device(adb, serial, &["shell", "getprop"]);
        if ok {
            let properties = parse_adb_properties(&output);
            if !properties.is_empty() {
                return Ok(properties);
            }
            return Err("ADB getprop вернул пустой список свойств".into());
        }
        let transient = ["not found", "offline", "transport", "closed"]
            .iter().any(|message| output.contains(message));
        if !transient || attempt == 2 {
            return Err(format!("ADB getprop: {output}"));
        }
        sleep(Duration::from_millis(700));
    }
    unreachable!()
}

fn parse_adb_properties(output: &str) -> BTreeMap<String, String> {
    output.lines().filter_map(|line| {
        let (key, value) = line.strip_prefix('[')?.strip_suffix(']')?.split_once("]: [")?;
        Some((key.to_string(), value.to_string()))
    }).collect()
}

fn exec_cmd_raw(exe: &Path, args: &[&str]) -> (bool, String) {
    let mut c = Command::new(exe);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    match hidden(&mut c).output() {
        Ok(out) => {
            let bytes = if !out.status.success() && !out.stderr.is_empty()
                || out.stdout.is_empty()
            {
                &out.stderr
            } else {
                &out.stdout
            };
            (out.status.success(), String::from_utf8_lossy(bytes).into_owned())
        }
        Err(e) => (false, format!("Ошибка запуска: {e}")),
    }
}

fn collect_android_diagnostics(adb: &PathBuf, info: &mut ComprehensiveDeviceInfo) {
    let read = |args: &[&str]| {
        let mut command = vec!["-s", info.mode.serial.as_str(), "shell"];
        command.extend_from_slice(args);
        let (ok, output) = exec_cmd_raw(adb, &command);
        if ok {
            DiagnosticOutput { output, error: None }
        } else {
            DiagnosticOutput {
                output: String::new(),
                error: Some(if output.is_empty() { "Команда завершилась с ошибкой".into() } else { output }),
            }
        }
    };
    for namespace in ["global", "secure", "system"] {
        info.android_settings.insert(
            namespace.into(),
            read(&["settings", "--user", "current", "list", namespace]),
        );
    }
    for (key, args) in [
        ("features", ["pm", "list", "features"].as_slice()),
        ("libraries", ["pm", "list", "libraries"].as_slice()),
        ("overlays", ["cmd", "overlay", "list", "--user", "current"].as_slice()),
        ("bootconfig", ["cat", "/proc/bootconfig"].as_slice()),
    ] {
        info.android_diagnostics.insert(key.into(), read(args));
    }
}

fn find_boot_partition<'a>(
    name: &str,
    slot: &str,
    partitions: &'a [PartitionInfo],
) -> Option<(String, &'a str)> {
    let mut names = Vec::new();
    if matches!(slot, "a" | "b") {
        names.push(format!("{name}_{slot}"));
    }
    names.push(name.to_string());
    names.into_iter().find_map(|name| {
        let part = partitions.iter().find(|part| part.name == name)?;
        let path = part.block_device.as_str();
        if path.starts_with("/dev/block/") && !path.contains("..")
            && path.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
        {
            Some((name, path))
        } else {
            None
        }
    })
}

fn read_boot_image(adb: &Path, serial: &str, block: &str, destination: &Path) -> Result<u64, String> {
    const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
    let (size_ok, size) = exec_cmd_raw(adb, &["-s", serial, "shell", "blockdev", "--getsize64", block]);
    let expected_size = size_ok.then(|| size.trim().parse::<u64>().ok()).flatten();
    if expected_size.is_some_and(|bytes| bytes > MAX_IMAGE_BYTES) {
        return Err("Размер образа превышает 256 МиБ".into());
    }
    if expected_size.is_none() {
        let (ok, features) = exec_cmd_raw(adb, &["-s", serial, "features"]);
        if !ok || !features.split(|c: char| c == ',' || c.is_ascii_whitespace()).any(|feature| feature == "shell_v2") {
            return Err("ADB не сообщает размер раздела и не поддерживает проверку результата чтения".into());
        }
    }
    let temporary = destination.with_extension("partial");
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(es)?;
    let result = (|| {
        let mut command = Command::new(adb);
        command
            .args(["-s", serial, "shell", "-T", "test", "-b", block, "&&", "cat", block])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = hidden(&mut command).spawn().map_err(es)?;
        let mut stdout = child.stdout.take().expect("ADB stdout is piped");
        let copied = io::copy(&mut (&mut stdout).take(MAX_IMAGE_BYTES + 1), &mut file);
        if !matches!(copied, Ok(bytes) if bytes <= MAX_IMAGE_BYTES) {
            let _ = child.kill();
        }
        drop(stdout);
        let status = child.wait().map_err(es)?;
        let bytes = copied.map_err(es)?;
        if bytes > MAX_IMAGE_BYTES {
            return Err("Размер образа превышает 256 МиБ".into());
        }
        if !status.success() || bytes == 0 {
            return Err("Блочное устройство не удалось прочитать с текущими правами ADB".into());
        }
        if let Some(expected) = expected_size {
            if bytes != expected {
                return Err(format!("Получено {bytes} байт из {expected}"));
            }
        }
        file.sync_all().map_err(es)?;
        Ok(bytes)
    })();
    drop(file);
    let result = result.and_then(|bytes| {
        fs::rename(&temporary, destination).map_err(es)?;
        Ok(bytes)
    });
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn collect_boot_images(adb: &Path, scan_id: &str, info: &mut ComprehensiveDeviceInfo) {
    let image_dir = devices_dir().join(scan_id);
    for name in ["lk", "boot"] {
        let mut result = BootImageResult {
            partition: None,
            relative_path: None,
            size_bytes: None,
            error: Some("Раздел не найден в доступной разметке".into()),
        };
        if let Some((partition, block)) = find_boot_partition(name, &info.slot, &info.partitions) {
            let filename = format!("{partition}.img");
            result.partition = Some(partition.clone());
            match fs::create_dir_all(&image_dir).map_err(es).and_then(|()| {
                read_boot_image(adb, &info.mode.serial, block, &image_dir.join(&filename))
            }) {
                Ok(bytes) => {
                    result.relative_path = Some(format!("devices/{scan_id}/{filename}"));
                    result.size_bytes = Some(bytes);
                    result.error = None;
                }
                Err(error) => result.error = Some(error),
            }
        }
        info.boot_images.insert(name.into(), result);
    }
}

/// Определение текущего подключенного устройства и его режима работы
#[tauri::command]
pub fn device_detect() -> Result<DeviceMode, String> {
    if let Ok(adb) = adb_bin() {
        let (_, out) = exec_cmd(&adb, &["devices"]);
        for line in out.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let serial = parts[0].to_string();
                let state = parts[1].to_lowercase();
                let (mode, desc) = match state.as_str() {
                    "device" => ("system".to_string(), "Android ОС (ADB доступен)".to_string()),
                    "recovery" => ("recovery".to_string(), "Режим Recovery (ADB)".to_string()),
                    "sideload" => ("sideload".to_string(), "Режим ADB Sideload".to_string()),
                    "unauthorized" => ("unauthorized".to_string(), "Устройство не авторизовано (подтвердите RSA на экране)".to_string()),
                    "offline" => ("offline".to_string(), "Устройство офлайн".to_string()),
                    _ => (state.clone(), format!("Режим {state}")),
                };
                return Ok(DeviceMode { serial, mode, description: desc });
            }
        }
    }

    if let Ok(fastboot) = fastboot_bin() {
        let (_, out) = exec_cmd(&fastboot, &["devices"]);
        for line in out.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let serial = parts[0].to_string();
                let state = parts[1].to_lowercase();
                let (mode, desc) = if state == "fastbootd" {
                    ("fastbootd".to_string(), "Режим FastbootD".to_string())
                } else {
                    ("fastboot".to_string(), "Режим Bootloader (Fastboot)".to_string())
                };
                return Ok(DeviceMode { serial, mode, description: desc });
            }
        }
    }

    Ok(DeviceMode {
        serial: "".to_string(),
        mode: "none".to_string(),
        description: "Устройство не подключено".to_string(),
    })
}

fn detect_recovery_from_os(adb: &Path, serial: &str, props: &BTreeMap<String, String>) -> String {
    let run = |args: &[&str]| exec_device(adb, serial, args);
    let (_, rec_check) = run(
        &["shell", "ls -d /sdcard/TWRP /sdcard/Fox /data/media/0/TWRP /data/media/0/Fox /cache/recovery 2>/dev/null"],
    );
    let mut custom_names = vec![];
    if rec_check.contains("TWRP") {
        custom_names.push("TWRP");
    }
    if rec_check.contains("Fox") {
        custom_names.push("OrangeFox Recovery");
    }

    for (key, value) in props {
        if (key.contains("twrp") || value.contains("twrp")) && !custom_names.contains(&"TWRP") {
            custom_names.push("TWRP");
        }
        if (key.contains("orangefox") || value.contains("orangefox")) && !custom_names.contains(&"OrangeFox Recovery") {
            custom_names.push("OrangeFox Recovery");
        }
    }

    let (_, has_rec_part) = run(
        &["shell", "ls /dev/block/by-name/recovery 2>/dev/null || ls /dev/block/bootdevice/by-name/recovery 2>/dev/null"],
    );
    let part_type = if !has_rec_part.trim().is_empty() {
        "выделенный раздел recovery"
    } else {
        "интегрировано в boot/vendor_boot ramdisk (A/B)"
    };

    if !custom_names.is_empty() {
        format!("Кастомное: {} ({})", custom_names.join(" / "), part_type)
    } else {
        format!("Стоковое OEM / AOSP ({})", part_type)
    }
}

fn collect_adb_partitions(adb: &Path, info: &mut ComprehensiveDeviceInfo) {
    let run = |args: &[&str]| exec_device(adb, &info.mode.serial, args);
    let mut block_sizes: BTreeMap<String, u64> = BTreeMap::new();
    let (_, proc_parts) = run(&["shell", "cat", "/proc/partitions"]);
    for line in proc_parts.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 4 {
            if let (Ok(blocks), name) = (cols[2].parse::<u64>(), cols[3]) {
                block_sizes.insert(name.to_string(), blocks * 1024);
            }
        }
    }

    let mut fs_by_mnt: BTreeMap<String, String> = BTreeMap::new();
    let (_, mounts_out) = run(&["shell", "cat", "/proc/mounts"]);
    for line in mounts_out.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 3 {
            let mnt = cols[1];
            let fstype = cols[2];
            fs_by_mnt.insert(mnt.to_string(), fstype.to_string());
        }
    }

    let (_, by_name_out) = run(
        &[
            "shell",
            "ls -l /dev/block/by-name/ 2>/dev/null || ls -l /dev/block/bootdevice/by-name/ 2>/dev/null || ls -l /dev/block/platform/*/by-name/ 2>/dev/null",
        ],
    );

    let mut discovered: BTreeMap<String, PartitionInfo> = BTreeMap::new();

    for line in by_name_out.lines() {
        if let Some(arrow) = line.find("->") {
            let left = line[..arrow].trim();
            let right = line[arrow + 2..].trim();
            if let Some(part_name) = left.split_whitespace().last() {
                let block_node = right.rsplit('/').next().unwrap_or(right);
                let size_bytes = block_sizes.get(block_node).copied().unwrap_or(0);
                let size_str = if size_bytes > 0 {
                    let mb = size_bytes as f64 / (1024.0 * 1024.0);
                    if mb >= 1024.0 {
                        format!("{:.2} ГБ", mb / 1024.0)
                    } else {
                        format!("{:.1} МБ", mb)
                    }
                } else {
                    "—".to_string()
                };

                let mnt_target = format!("/{}", part_name);
                let fs = fs_by_mnt.get(&mnt_target).cloned().unwrap_or_else(|| "raw".into());

                discovered.insert(
                    part_name.to_string(),
                    PartitionInfo {
                        name: part_name.to_string(),
                        filesystem: fs,
                        size_human: size_str,
                        mount_point: mnt_target,
                        block_device: right.to_string(),
                    },
                );
            }
        }
    }

    if discovered.is_empty() {
        let (_, df_out) = run(&["shell", "df", "-h"]);
        for line in df_out.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 6 {
                let dev = cols[0];
                let size = cols[1];
                let mnt = cols[5];
                if mnt.starts_with('/') {
                    let fs = fs_by_mnt.get(mnt).cloned().unwrap_or_else(|| "ext4/erofs".into());
                    info.partitions.push(PartitionInfo {
                        name: mnt.trim_start_matches('/').replace('/', "_"),
                        filesystem: fs,
                        size_human: size.to_string(),
                        mount_point: mnt.to_string(),
                        block_device: dev.to_string(),
                    });
                }
            }
        }
    } else {
        for (pname, mut pinfo) in discovered {
            let test_mnt = format!("/{}", pname);
            if let Some(fs) = fs_by_mnt.get(&test_mnt) {
                pinfo.filesystem = fs.clone();
            }
            info.partitions.push(pinfo);
        }
    }
}

fn collect_adb_data(adb: &Path, info: &mut ComprehensiveDeviceInfo, props: BTreeMap<String, String>) {
    let serial = info.mode.serial.as_str();
    let run = |args: &[&str]| exec_device(adb, serial, args);
    let (_, id_out) = run(&["shell", "id"]);
    info.is_root = id_out.contains("uid=0");

    info.model = props
        .get("ro.product.model")
        .cloned()
        .unwrap_or_else(|| props.get("ro.product.system.model").cloned().unwrap_or_default());
    info.brand = props.get("ro.product.brand").cloned().unwrap_or_default();
    info.manufacturer = props.get("ro.product.manufacturer").cloned().unwrap_or_default();
    info.device = props.get("ro.product.device").cloned().unwrap_or_default();
    info.product = props.get("ro.build.product").cloned().unwrap_or_default();
    info.soc = props
        .get("ro.soc.model")
        .cloned()
        .unwrap_or_else(|| props.get("ro.hardware").cloned().unwrap_or_default());
    info.cpu_abi = props.get("ro.product.cpu.abi").cloned().unwrap_or_default();
    info.android_version = props.get("ro.build.version.release").cloned().unwrap_or_default();
    info.sdk_version = props.get("ro.build.version.sdk").cloned().unwrap_or_default();
    info.security_patch = props.get("ro.build.version.security_patch").cloned().unwrap_or_default();
    info.build_id = props.get("ro.build.id").cloned().unwrap_or_default();
    info.build_fingerprint = props.get("ro.build.fingerprint").cloned().unwrap_or_default();
    info.baseband = props.get("gsm.version.baseband").cloned().unwrap_or_default();

    let slot_prop = props.get("ro.boot.slot_suffix").cloned().unwrap_or_default();
    info.slot = slot_prop.trim_start_matches('_').to_string();
    info.is_ab = !info.slot.is_empty() || props.get("ro.build.ab_update").map(|v| v == "true").unwrap_or(false);
    info.is_dynamic = props.get("ro.boot.dynamic_partitions").map(|v| v == "true").unwrap_or(false);

    let vb = props.get("ro.boot.verifiedbootstate").cloned().unwrap_or_default();
    let fl = props.get("ro.boot.flash.locked").cloned().unwrap_or_default();
    info.bootloader_status = if vb == "orange" || fl == "0" {
        "Разблокирован (Unlocked)".to_string()
    } else if vb == "green" || fl == "1" {
        "Заблокирован (Locked)".to_string()
    } else {
        vb
    };

    let (_, selinux) = run(&["shell", "getenforce"]);
    info.selinux = selinux;

    let (_, uname) = run(&["shell", "uname", "-a"]);
    info.kernel = uname;

    let (_, cmdline) = run(&["shell", "cat", "/proc/cmdline"]);
    info.kernel_cmdline = cmdline;

    let (_, wm_size) = run(&["shell", "wm", "size"]);
    let (_, wm_density) = run(&["shell", "wm", "density"]);
    let size_clean = wm_size.replace("Physical size:", "").trim().to_string();
    let density_clean = wm_density.replace("Physical density:", "").trim().to_string();
    if !size_clean.is_empty() {
        info.display = format!("{} ({} dpi)", size_clean, density_clean);
    }

    let (_, dumpsys_bat) = run(&["shell", "dumpsys", "battery"]);
    let mut bat_lines = vec![];
    for l in dumpsys_bat.lines() {
        let trim = l.trim();
        if trim.starts_with("level:") || trim.starts_with("temperature:") || trim.starts_with("health:") || trim.starts_with("status:") {
            bat_lines.push(trim.to_string());
        }
    }
    info.battery = bat_lines.join(" | ");

    let (_, meminfo) = run(&["shell", "cat", "/proc/meminfo"]);
    let mut total_ram = String::new();
    let mut free_ram = String::new();
    for l in meminfo.lines() {
        if l.starts_with("MemTotal:") {
            total_ram = l.replace("MemTotal:", "").trim().to_string();
        } else if l.starts_with("MemAvailable:") {
            free_ram = l.replace("MemAvailable:", "").trim().to_string();
        }
    }
    info.memory_info = format!("Всего: {}, Доступно: {}", total_ram, free_ram);

    let (_, fstab) = run(&["shell", "cat /vendor/etc/fstab.* /system/etc/fstab.* /odm/etc/fstab.* 2>/dev/null"]);
    info.raw_fstab = fstab;

    info.recovery_type = detect_recovery_from_os(adb, serial, &props);
    info.build_props = props;
}

fn collect_recovery_data(adb: &Path, info: &mut ComprehensiveDeviceInfo) {
    let run = |args: &[&str]| exec_device(adb, &info.mode.serial, args);
    let (_, id_out) = run(&["shell", "id"]);
    info.is_root = id_out.contains("uid=0");
    let (ok, props) = run(&["shell", "getprop"]);
    if ok {
        info.build_props = parse_adb_properties(&props);
    }
    info.slot = info.build_props.get("ro.boot.slot_suffix")
        .map(|slot| slot.trim_start_matches('_').to_string())
        .unwrap_or_default();

    let (_, twrp_ver) = run(&["shell", "cat", "/etc/recovery.version"]);
    let (_, twrp_prop) = run(&["shell", "getprop", "ro.twrp.version"]);
    if !twrp_ver.trim().is_empty() {
        info.recovery_type = format!("TWRP / Custom ({})", twrp_ver.trim());
    } else if !twrp_prop.trim().is_empty() {
        info.recovery_type = format!("TWRP {}", twrp_prop.trim());
    } else {
        info.recovery_type = "Кастомное Recovery (активен ADB Root)".to_string();
    }

    let (_, fstab) = run(
        &["shell", "cat /etc/twrp.fstab 2>/dev/null || cat /etc/recovery.fstab 2>/dev/null"],
    );
    if !fstab.trim().is_empty() {
        info.raw_fstab = fstab;
    }

    let (_, uname) = run(&["shell", "uname", "-a"]);
    info.kernel = uname;

    let (_, cmdline) = run(&["shell", "cat", "/proc/cmdline"]);
    info.kernel_cmdline = cmdline;

    collect_adb_partitions(adb, info);
}

fn collect_fastboot_data(fastboot: &PathBuf, info: &mut ComprehensiveDeviceInfo) {
    let (_, getvar_all) = exec_device(fastboot, &info.mode.serial, &["getvar", "all"]);
    let mut vars = BTreeMap::new();

    for line in getvar_all.lines() {
        let clean = line.trim_start_matches("(bootloader) ").trim();
        if let Some(colon) = clean.find(':') {
            let k = clean[..colon].trim().to_string();
            let v = clean[colon + 1..].trim().to_string();
            vars.insert(k, v);
        }
    }

    let (_, oem_out) = exec_device(fastboot, &info.mode.serial, &["oem", "device-info"]);
    if !oem_out.trim().is_empty() {
        info.oem_device_info = oem_out.clone();
        for line in oem_out.lines() {
            let l = line.to_lowercase();
            if l.contains("device unlocked: true") || l.contains("unlocked: true") {
                info.bootloader_status = "Разблокирован (Unlocked - OEM)".to_string();
            } else if l.contains("device unlocked: false") || l.contains("unlocked: false") {
                info.bootloader_status = "Заблокирован (Locked - OEM)".to_string();
            }
        }
    }

    if info.product.is_empty() {
        info.product = vars.get("product").cloned().unwrap_or_default();
    }
    if info.model.is_empty() {
        info.model = vars.get("product").cloned().unwrap_or_else(|| "Fastboot Device".into());
    }
    if info.slot.is_empty() {
        info.slot = vars.get("current-slot").cloned().unwrap_or_default();
    }
    info.is_ab = !info.slot.is_empty() || vars.contains_key("slot-count");
    info.is_dynamic = vars.get("is-userspace").map(|v| v == "yes").unwrap_or(false) || info.mode.mode == "fastbootd";

    let unlocked = vars.get("unlocked").cloned().unwrap_or_default();
    if !unlocked.is_empty() {
        info.bootloader_status = if unlocked == "yes" {
            "Разблокирован (Unlocked)".to_string()
        } else {
            "Заблокирован (Locked)".to_string()
        };
    }

    if info.soc.is_empty() {
        info.soc = vars.get("cpu").cloned().unwrap_or_default();
    }
    if info.build_id.is_empty() {
        info.build_id = vars.get("version-bootloader").cloned().unwrap_or_default();
    }
    if info.security_patch.is_empty() {
        info.security_patch = vars.get("security-patch-level").cloned().unwrap_or_default();
    }
    if info.baseband.is_empty() {
        info.baseband = vars.get("version-baseband").cloned().unwrap_or_default();
    }

    if info.partitions.is_empty() {
        for (k, v) in &vars {
            if let Some(pname) = k.strip_prefix("partition-size:") {
                let hex_bytes = u64::from_str_radix(v.trim_start_matches("0x"), 16).unwrap_or(0);
                let mb = (hex_bytes as f64) / (1024.0 * 1024.0);
                let size_str = if mb >= 1024.0 {
                    format!("{:.2} ГБ", mb / 1024.0)
                } else {
                    format!("{:.1} МБ", mb)
                };
                info.partitions.push(PartitionInfo {
                    name: pname.to_string(),
                    filesystem: vars.get(&format!("partition-type:{pname}")).cloned().unwrap_or_else(|| "raw".into()),
                    size_human: size_str,
                    mount_point: format!("/{pname}"),
                    block_device: format!("partition ({pname})"),
                });
            }
        }
    }

    info.fastboot_vars = vars;
}

fn wait_for_mode(wanted: &str, timeout_secs: u64) -> Result<DeviceMode, String> {
    let start = Instant::now();
    while start.elapsed().as_secs() < timeout_secs {
        if let Ok(d) = device_detect() {
            if wanted == "adb" && (d.mode == "system" || d.mode == "recovery") {
                return Ok(d);
            }
            if wanted == "fastboot" && (d.mode == "fastboot" || d.mode == "fastbootd") {
                return Ok(d);
            }
        }
        sleep(Duration::from_millis(1500));
    }
    Err(format!("Таймаут ожидания подключения устройства в режиме {wanted}"))
}

/// Сохранённый результат последнего глубокого сканирования устройства
static LAST_DEVICE_SCAN: Mutex<Option<ComprehensiveDeviceInfo>> = Mutex::new(None);
static LAST_SCAN_PROFILE: Mutex<Option<SavedDeviceProfile>> = Mutex::new(None);

#[tauri::command]
pub fn get_last_device_info() -> Option<ComprehensiveDeviceInfo> {
    LAST_DEVICE_SCAN.lock().ok().and_then(|guard| guard.clone())
}

#[tauri::command]
pub fn get_last_scan_profile() -> Option<SavedDeviceProfile> {
    LAST_SCAN_PROFILE.lock().ok().and_then(|guard| guard.clone())
}

// ---------------- Хранилище профилей устройств ----------------

fn save_device_profile_file(profile: &SavedDeviceProfile) -> Result<(), String> {
    let dir = devices_dir();
    fs::create_dir_all(&dir).map_err(es)?;
    let p = device_profile_path(&profile.id)?;
    let json_text = serde_json::to_string_pretty(profile).map_err(es)?;
    fs::write(p, json_text).map_err(es)
}

#[tauri::command]
pub fn save_device_profile(profile: SavedDeviceProfile) -> Result<(), String> {
    save_device_profile_file(&profile)
}

#[tauri::command]
pub fn list_saved_devices() -> Result<Vec<DeviceSummary>, String> {
    let dir = devices_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut list = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e.eq_ignore_ascii_case("json")) {
                if let Ok(text) = fs::read_to_string(&path) {
                    if let Ok(p) = serde_json::from_str::<SavedDeviceProfile>(&text) {
                        list.push(DeviceSummary {
                            id: p.id,
                            title: p.title,
                            brand: p.data.brand,
                            model: p.data.model,
                            serial: p.data.mode.serial,
                            android_version: p.data.android_version,
                            timestamp: p.timestamp,
                            date_human: p.date_human,
                            success: p.success,
                            failure_stage: p.failure_stage,
                            failure_error: p.failure_error,
                            partitions_count: p.data.partitions.len(),
                            props_count: p.data.build_props.len(),
                        });
                    }
                }
            }
        }
    }
    list.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(list)
}

#[tauri::command]
pub fn get_saved_device(id: String) -> Result<SavedDeviceProfile, String> {
    let p = device_profile_path(&id)?;
    if !p.is_file() {
        return Err(format!("Профиль устройства {id} не найден"));
    }
    let text = fs::read_to_string(p).map_err(es)?;
    serde_json::from_str(&text).map_err(es)
}

#[tauri::command]
pub fn delete_saved_device(id: String) -> Result<(), String> {
    let p = device_profile_path(&id)?;
    let image_dir = p.with_extension("");
    if fs::symlink_metadata(&image_dir).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        for name in ["lk", "lk_a", "lk_b", "boot", "boot_a", "boot_b"] {
            let image = image_dir.join(format!("{name}.img"));
            if image.is_file() {
                fs::remove_file(image).map_err(es)?;
            }
        }
        if fs::read_dir(&image_dir).map_err(es)?.next().is_none() {
            fs::remove_dir(image_dir).map_err(es)?;
        }
    }
    if p.is_file() {
        fs::remove_file(p).map_err(es)?;
    }
    Ok(())
}

fn device_profile_path(id: &str) -> Result<PathBuf, String> {
    if !id.strip_prefix("dev_").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        return Err("Некорректный идентификатор устройства".into());
    }
    Ok(devices_dir().join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partition(name: &str, path: &str) -> PartitionInfo {
        PartitionInfo {
            name: name.into(),
            block_device: path.into(),
            ..PartitionInfo::default()
        }
    }

    #[test]
    fn reads_active_slot_or_unqualified_partition() {
        let parts = [
            partition("boot_a", "/dev/block/sda1"),
            partition("boot_b", "/dev/block/sda2"),
            partition("lk", "/dev/block/mmcblk0p1"),
        ];
        assert_eq!(find_boot_partition("boot", "b", &parts), Some(("boot_b".into(), "/dev/block/sda2")));
        assert_eq!(find_boot_partition("lk", "a", &parts), Some(("lk".into(), "/dev/block/mmcblk0p1")));
        assert!(find_boot_partition("boot", "", &parts).is_none());
    }

    #[test]
    fn rejects_partition_paths_that_are_not_safe_block_paths() {
        for path in ["/dev/block/sda1; echo", "/dev/block/../null", "/data/boot.img"] {
            assert!(find_boot_partition("boot", "", &[partition("boot", path)]).is_none());
        }
    }

    #[test]
    fn old_profiles_open_without_new_diagnostics() {
        let mut profile = serde_json::to_value(SavedDeviceProfile::default()).unwrap();
        let data = profile["data"].as_object_mut().unwrap();
        for field in ["android_settings", "android_diagnostics", "boot_images"] {
            data.remove(field);
        }
        let loaded: SavedDeviceProfile = serde_json::from_value(profile).unwrap();
        assert!(loaded.data.android_settings.is_empty());
        assert!(loaded.data.android_diagnostics.is_empty());
        assert!(loaded.data.boot_images.is_empty());
    }
}

// ---------------- Сквозной сбор информации ----------------

#[tauri::command]
pub fn read_device_info_deep(app: AppHandle, include_boot_images: Option<bool>) -> Result<(), String> {
    spawn_task(app, "Глубокий сбор данных об устройстве", move |ctx| {
        if let Ok(mut info) = LAST_DEVICE_SCAN.lock() {
            *info = None;
        }
        if let Ok(mut profile) = LAST_SCAN_PROFILE.lock() {
            *profile = None;
        }
        let initial_mode = device_detect()?;
        if initial_mode.mode == "none" {
            return Err("Устройство не обнаружено ни в ADB, ни в Fastboot. Проверьте подключение USB кабеля и драйверы.".into());
        }

        let now_secs = current_timestamp();
        let mut profile = SavedDeviceProfile {
            id: format!("dev_{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()),
            title: String::new(),
            timestamp: now_secs,
            date_human: format_epoch_seconds(now_secs),
            success: true,
            failure_stage: None,
            failure_error: None,
            data: ComprehensiveDeviceInfo::default(),
        };

        let adb = adb_bin().ok();
        let fastboot = fastboot_bin().ok();
        let mut info = ComprehensiveDeviceInfo::default();
        info.mode = initial_mode.clone();

        if initial_mode.mode == "system" {
            let a = match adb.as_ref() {
                Some(p) => p,
                None => {
                    profile.success = false;
                    profile.failure_stage = Some("1/4: ADB утилита".into());
                    profile.failure_error = Some("ADB утилита не найдена в платформенных инструментах".into());
                    return Err("ADB утилита не найдена".into());
                }
            };

            ctx.stage("1/4: Считывание параметров Android OS через ADB...");
            ctx.log(format!("  Обнаружено устройство: {} в режиме Android OS", initial_mode.serial));
            match read_adb_properties(a, &initial_mode.serial) {
                Ok(properties) => {
                    collect_adb_data(a, &mut info, properties);
                    collect_adb_partitions(a, &mut info);
                    ctx.stage("1/4: Чтение настроек и возможностей Android...");
                    collect_android_diagnostics(a, &mut info);
                    if include_boot_images == Some(true) {
                        ctx.stage("1/4: Чтение образов lk и boot...");
                        collect_boot_images(a, &profile.id, &mut info);
                    }
                    ctx.log(format!(
                        "  Собрано: {} свойств getprop, {} разделов устройства",
                        info.build_props.len(),
                        info.partitions.len()
                    ));
                    let (ok, state) = exec_device(a, &initial_mode.serial, &["get-state"]);
                    if !ok || state != "device" {
                        profile.success = false;
                        profile.failure_stage = Some("1/4: Соединение ADB прервано".into());
                        profile.failure_error = Some(format!("ADB get-state: {state}"));
                    }
                }
                Err(error) => {
                    profile.success = false;
                    profile.failure_stage = Some("1/4: Чтение свойств Android через ADB".into());
                    profile.failure_error = Some(error);
                }
            }

            // Переход в Fastboot
            if !profile.success {
                ctx.log("  Опрос загрузчика пропущен из-за ошибки на этапе ADB.");
            } else if let Some(ref fb) = fastboot {
                ctx.stage("2/4: Перезагрузка в Bootloader (Fastboot) для снятия аппаратных данных...");
                ctx.log("  Отправка команды 'adb reboot bootloader'...");
                let (reboot_ok, err_msg) = exec_device(a, &initial_mode.serial, &["reboot", "bootloader"]);
                if reboot_ok {
                    ctx.log("  Ожидание перехода устройства в режим Fastboot (до 45 сек)...");
                    match wait_for_mode("fastboot", 45) {
                        Ok(fb_mode) => {
                            info.mode = fb_mode;
                            ctx.stage("3/4: Опрос Bootloader / Fastboot (getvar all, oem status)...");
                            ctx.log("  Устройство успешно подключилось в Fastboot!");
                            collect_fastboot_data(fb, &mut info);
                            ctx.log(format!(
                                "  Считано {} переменных Fastboot, статус загрузчика: {}",
                                info.fastboot_vars.len(),
                                info.bootloader_status
                            ));

                            ctx.stage("4/4: Возврат устройства в обычный режим (перезагрузка в ОС)...");
                            ctx.log("  Отправка команды 'fastboot reboot'...");
                            let _ = exec_device(fb, &info.mode.serial, &["reboot"]);
                            ctx.log("  Ожидание возвращения телефона в Android OS (до 60 сек)...");
                            match wait_for_mode("adb", 60) {
                                Ok(restored_mode) => {
                                    info.mode = restored_mode;
                                    ctx.log("  Устройство успешно вернулось в Android OS и готово к работе!");
                                }
                                Err(e) => {
                                    profile.success = false;
                                    profile.failure_stage = Some("4/4: Ожидание возврата в ОС".into());
                                    profile.failure_error = Some(format!("Устройство перезагружается, таймаут ADB: {e}"));
                                    ctx.log("  Устройство перезагружается в систему.");
                                }
                            }
                        }
                        Err(e) => {
                            profile.success = false;
                            profile.failure_stage = Some("2/4: Перезагрузка в Fastboot".into());
                            profile.failure_error = Some(format!("Таймаут подключения в Fastboot: {e}"));
                            ctx.err(format!("  Таймаут перехода в Fastboot: {e}. Данные сохранены из режима ОС."));
                        }
                    }
                } else {
                    profile.success = false;
                    profile.failure_stage = Some("2/4: Команда reboot bootloader".into());
                    profile.failure_error = Some(format!("Команда перезагрузки вернула ошибку: {err_msg}"));
                }
            } else {
                profile.success = false;
                profile.failure_stage = Some("2/4: Fastboot утилита".into());
                profile.failure_error = Some("Fastboot не установлен, опрос bootloader пропущен".into());
                ctx.log("  Fastboot утилита не найдена, пропуск шага опроса bootloader.");
            }
        } else if initial_mode.mode == "recovery" {
            let a = match adb.as_ref() {
                Some(p) => p,
                None => {
                    profile.success = false;
                    profile.failure_stage = Some("1/4: ADB утилита".into());
                    profile.failure_error = Some("ADB не найден".into());
                    return Err("ADB не найден".into());
                }
            };
            ctx.stage("1/4: Сбор данных из Recovery через ADB Root...");
            ctx.log(format!("  Устройство в режиме Recovery: {}", initial_mode.serial));
            collect_recovery_data(a, &mut info);
            if include_boot_images == Some(true) {
                ctx.stage("1/4: Чтение образов lk и boot...");
                collect_boot_images(a, &profile.id, &mut info);
            }

            if let Some(ref fb) = fastboot {
                ctx.stage("2/4: Перезагрузка в Bootloader (Fastboot)...");
                let (reboot_ok, error) = exec_device(a, &initial_mode.serial, &["reboot", "bootloader"]);
                if reboot_ok {
                    ctx.log("  Ожидание перехода устройства в режим Fastboot (до 45 сек)...");
                    match wait_for_mode("fastboot", 45) {
                        Ok(fb_mode) => {
                            info.mode = fb_mode;
                            ctx.stage("3/4: Опрос переменных Fastboot...");
                            collect_fastboot_data(fb, &mut info);

                            ctx.stage("4/4: Перезагрузка устройства...");
                            let _ = exec_device(fb, &info.mode.serial, &["reboot"]);
                        }
                        Err(e) => {
                            profile.success = false;
                            profile.failure_stage = Some("2/4: Перезагрузка в Fastboot".into());
                            profile.failure_error = Some(e);
                        }
                    }
                } else {
                    profile.success = false;
                    profile.failure_stage = Some("2/4: Команда reboot bootloader".into());
                    profile.failure_error = Some(format!("Команда перезагрузки вернула ошибку: {error}"));
                }
            }
        } else if initial_mode.mode == "fastboot" || initial_mode.mode == "fastbootd" {
            let fb = match fastboot.as_ref() {
                Some(p) => p,
                None => {
                    profile.success = false;
                    profile.failure_stage = Some("1/2: Fastboot утилита".into());
                    profile.failure_error = Some("Fastboot не найден".into());
                    return Err("Fastboot не найден".into());
                }
            };
            ctx.stage("1/2: Опрос устройства в режиме Fastboot / Bootloader...");
            collect_fastboot_data(fb, &mut info);
            if include_boot_images == Some(true) {
                for name in ["lk", "boot"] {
                    info.boot_images.insert(name.into(), BootImageResult {
                        partition: None,
                        relative_path: None,
                        size_bytes: None,
                        error: Some("Считывание образа доступно через ADB в Android или Recovery".into()),
                    });
                }
            }
            ctx.stage("2/2: Завершено. Устройство сохранено в режиме Fastboot.");
        }

        let dev_brand = if info.brand.is_empty() { "Android" } else { &info.brand };
        let dev_model = if info.model.is_empty() { &info.mode.serial } else { &info.model };
        profile.title = format!("{dev_brand} {dev_model}");
        profile.data = info.clone();

        // Если сканирование полностью успешно — автоматически сохраняем в хранилище!
        if profile.success {
            match save_device_profile_file(&profile) {
                Ok(()) => ctx.log("  Профиль сохранён в локальное хранилище."),
                Err(error) => {
                    profile.success = false;
                    profile.failure_stage = Some("Сохранение профиля".into());
                    profile.failure_error = Some(error);
                }
            }
        }

        if let Ok(mut g) = LAST_DEVICE_SCAN.lock() {
            *g = Some(info.clone());
        }
        if let Ok(mut g) = LAST_SCAN_PROFILE.lock() {
            *g = Some(profile.clone());
        }

        crate::telegram::on_device_scanned(dev_brand, dev_model, &info.android_version, info.partitions.len(), profile.success);

        if profile.success {
            Ok(format!(
                "Глубокий анализ успешно завершен: {} {} (Android: {}, Загрузчик: {}, Разделов: {})",
                info.brand, info.model, info.android_version, info.bootloader_status, info.partitions.len()
            ))
        } else {
            if let Some(error) = &profile.failure_error {
                ctx.err(format!("  {error}"));
            }
            Ok(format!(
                "Анализ завершился частично (ошибка на этапе «{}»). Собрано данных: {} свойств, {} разделов.",
                profile.failure_stage.as_deref().unwrap_or("?"),
                info.build_props.len(),
                info.partitions.len()
            ))
        }
    })
}

/// Чтение логов устройства (logcat, dmesg, recovery.log)
#[tauri::command]
pub async fn read_device_logs(log_type: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let adb = adb_bin()?;
        let mode = device_detect()?;

        if mode.mode != "system" && mode.mode != "recovery" && mode.mode != "sideload" {
            return Err("Чтение логов доступно только в режимах Android OS или Recovery (ADB).".into());
        }

        let output = match log_type.as_str() {
            "logcat" => {
                let (_, out) = exec_cmd(&adb, &["logcat", "-d", "-v", "time"]);
                out
            }
            "dmesg" => {
                let (_, out) = exec_cmd(&adb, &["shell", "dmesg"]);
                out
            }
            "recovery" => {
                let (_, out1) = exec_cmd(&adb, &["shell", "cat", "/tmp/recovery.log"]);
                if !out1.trim().is_empty() && !out1.contains("No such file") {
                    out1
                } else {
                    let (_, out2) = exec_cmd(&adb, &["shell", "cat", "/cache/recovery/last_log"]);
                    if !out2.trim().is_empty() && !out2.contains("No such file") {
                        out2
                    } else {
                        let (_, out3) = exec_cmd(&adb, &["shell", "cat", "/cache/recovery/log"]);
                        if !out3.trim().is_empty() && !out3.contains("No such file") {
                            out3
                        } else {
                            let (_, out4) = exec_cmd(&adb, &["shell", "cat", "/data/media/0/Fox/FoxFiles/Fox.log"]);
                            if !out4.trim().is_empty() && !out4.contains("No such file") {
                                out4
                            } else {
                                "Лог recovery не обнаружен (проверены /tmp/recovery.log, /cache/recovery/last_log, Fox.log). Если телефон в ОС, логи предыдущих прошивок могли быть очищены системой.".to_string()
                            }
                        }
                    }
                }
            }
            _ => return Err("Неизвестный тип лога".into()),
        };

        if output.is_empty() {
            Ok("Лог пуст или недостаточно прав доступа.".to_string())
        } else {
            Ok(output)
        }
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r)
}
