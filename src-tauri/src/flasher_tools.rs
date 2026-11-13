//! Модуль прошивки устройств через Fastboot / FastbootD и переключения сервисных режимов (EDL, BROM, Recovery).

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FastbootDevice {
    pub serial: String,
    pub mode: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FlashRequest {
    pub partition: String,
    pub image_path: String,
    pub slot: String, // "current", "a", "b", "both"
}

fn fastboot_cmd() -> String {
    if let Ok(bin) = crate::tools::bin("platform-tools", "fastboot.exe") {
        return bin.to_string_lossy().to_string();
    }
    if let Ok(out) = Command::new("where.exe").arg("fastboot").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(first) = s.lines().next() {
                return first.trim().to_string();
            }
        }
    }
    "fastboot".to_string()
}

#[tauri::command]
pub fn fb_list_devices() -> Result<Vec<FastbootDevice>, String> {
    let cmd = fastboot_cmd();
    let out = Command::new(&cmd)
        .arg("devices")
        .output()
        .map_err(|e| format!("Ошибка запуска fastboot: {e}"))?;

    let s = String::from_utf8_lossy(&out.stdout);
    let mut list = Vec::new();

    for line in s.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            list.push(FastbootDevice {
                serial: parts[0].to_string(),
                mode: parts[1].to_string(),
            });
        }
    }

    Ok(list)
}

#[tauri::command]
pub fn fb_flash_partition(req: FlashRequest) -> Result<String, String> {
    let cmd = fastboot_cmd();
    let mut args = Vec::new();

    let target_part = match req.slot.as_str() {
        "a" => format!("{}_a", req.partition),
        "b" => format!("{}_b", req.partition),
        _ => req.partition.clone(),
    };

    args.push("flash".to_string());
    args.push(target_part.clone());
    args.push(req.image_path.clone());

    let out = Command::new(&cmd)
        .args(&args)
        .output()
        .map_err(|e| format!("Ошибка выполнения fastboot flash: {e}"))?;

    let full_log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    if out.status.success() {
        Ok(format!("Раздел {target_part} успешно прошит!\n{full_log}"))
    } else {
        Err(format!("Ошибка прошивки {target_part}:\n{full_log}"))
    }
}

#[tauri::command]
pub fn fb_reboot_mode(mode: String) -> Result<String, String> {
    let cmd = fastboot_cmd();
    let mut args = vec!["reboot".to_string()];

    match mode.as_str() {
        "system" => {}
        "bootloader" => args.push("bootloader".into()),
        "recovery" => args.push("recovery".into()),
        "fastbootd" => args.push("fastboot".into()),
        "edl" => args.push("edl".into()),
        other => return Err(format!("Неизвестный режим перезагрузки: {other}")),
    }

    let out = Command::new(&cmd)
        .args(&args)
        .output()
        .map_err(|e| format!("Ошибка перезагрузки: {e}"))?;

    if out.status.success() {
        Ok(format!("Команда перезагрузки в режим «{mode}» отправлена успешно."))
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("Fastboot ошибка: {err}"))
    }
}
