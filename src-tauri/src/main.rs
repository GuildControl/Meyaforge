#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod android_tools;
mod builder;
mod detect;
mod device_reader;
mod ext4x;
mod file_manager;
mod lp;
mod porting;
mod project;
mod sdat;
mod sparse;
mod telegram;
mod tools;
mod util;

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tauri::AppHandle;

// ---------------- Обновления ----------------

#[derive(Serialize)]
struct UpdateInfo {
    current: String,
    latest: Option<String>,
    available: bool,
    url: Option<String>,
    notes: Option<String>,
    error: Option<String>,
}

#[tauri::command]
async fn check_update(app: AppHandle) -> UpdateInfo {
    let current = app.package_info().version.to_string();
    let res = tauri_plugin_updater::UpdaterExt::updater(&app)
        .map_err(|e| e.to_string())
        .and_then(|u| Ok(u));

    let info = match res {
        Ok(updater) => match updater.check().await {
            Ok(Some(update)) => UpdateInfo {
                current,
                latest: Some(update.version.clone()),
                available: true,
                url: Some("https://github.com/GuildControl/Meyaforge/releases/latest".into()),
                notes: update.body.clone(),
                error: None,
            },
            Ok(None) => UpdateInfo {
                current,
                latest: None,
                available: false,
                url: None,
                notes: None,
                error: None,
            },
            Err(e) => UpdateInfo {
                current,
                latest: None,
                available: false,
                url: None,
                notes: None,
                error: Some(e.to_string()),
            },
        },
        Err(e) => UpdateInfo {
            current,
            latest: None,
            available: false,
            url: None,
            notes: None,
            error: Some(e),
        },
    };
    info
}

#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Обновлений нет".to_string())?;

    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}

// ---------------- Профиль ----------------

#[derive(Serialize, Deserialize, Default, Clone)]
struct Profile {
    nickname: String,
    telegram: String,
    github: String,
    device: String,
    about: String,
}

#[tauri::command]
fn get_profile() -> Result<Option<Profile>, String> {
    let p = util::root().join("profile.json");
    if !p.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
    Ok(serde_json::from_str(&s).ok())
}

#[tauri::command]
fn save_profile(profile: Profile) -> Result<(), String> {
    if profile.nickname.trim().is_empty() {
        return Err("Ник обязателен".into());
    }
    let s = serde_json::to_string_pretty(&profile).map_err(|e| e.to_string())?;
    std::fs::write(util::root().join("profile.json"), s).map_err(|e| e.to_string())
}

// ---------------- Терминал ----------------

#[tauri::command]
fn run_command(app: AppHandle, cmd: String) -> Result<(), String> {
    let mut c = Command::new("cmd");
    c.args(["/D", "/C", &format!("chcp 65001>nul & {cmd}")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = util::hidden(&mut c).spawn().map_err(|e| e.to_string())?;

    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let a1 = app.clone();
    let t1 = std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            util::emit(&a1, "out", l);
        }
    });
    let a2 = app.clone();
    let t2 = std::thread::spawn(move || {
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            util::emit(&a2, "err", l);
        }
    });
    std::thread::spawn(move || {
        let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let _ = t1.join();
        let _ = t2.join();
        util::emit(&app, "exit", code.to_string());
    });
    Ok(())
}

// ---------------- WSL ----------------

#[tauri::command]
fn wsl_status() -> bool {
    let mut c = Command::new("wsl");
    c.args(["--status"]).stdout(Stdio::null()).stderr(Stdio::null());
    util::hidden(&mut c).status().map(|s| s.success()).unwrap_or(false)
}

#[tauri::command]
fn wsl_install() -> Result<(), String> {
    let mut c = Command::new("powershell");
    c.args([
        "-NoProfile",
        "-Command",
        "Start-Process wsl -ArgumentList '--install','--no-launch' -Verb RunAs",
    ]);
    util::hidden(&mut c).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("bad url".into());
    }
    let mut c = Command::new("explorer");
    c.arg(url);
    util::hidden(&mut c).spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn main() {
    let data = util::root();
    let temporary = data.join("tmp");
    let android = data.join("android");
    for path in [&temporary, &android] {
        std::fs::create_dir_all(path)
            .unwrap_or_else(|e| panic!("Не удалось создать {}: {e}", path.display()));
    }
    // Set these before Tauri and subprocesses start.
    std::env::set_var("TMP", &temporary);
    std::env::set_var("TEMP", &temporary);
    std::env::set_var("ANDROID_USER_HOME", &android);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let webview = util::root().join("webview");
            std::fs::create_dir_all(&webview).map_err(|e| {
                std::io::Error::new(e.kind(), format!("{}: {e}", webview.display()))
            })?;
            let config = &app.config().app.windows[0];
            tauri::WebviewWindowBuilder::from_config(app, config)?
                .data_directory(webview)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_profile,
            save_profile,
            check_update,
            install_update,
            run_command,
            wsl_status,
            wsl_install,
            open_url,
            // task & general
            util::task_busy,
            // tools
            tools::tools_list,
            tools::tool_check_latest,
            tools::tool_install,
            tools::tool_remove,
            // projects
            project::projects_root,
            project::list_projects,
            project::create_project,
            project::get_project,
            project::delete_project,
            project::open_in_explorer,
            project::import_firmware,
            project::unsparse_partition,
            project::extract_partition,
            project::extract_all_partitions,
            // android boot & apk tools
            android_tools::unpack_boot,
            android_tools::repack_boot,
            android_tools::decompile_apk,
            android_tools::build_apk,
            // device reader & library
            device_reader::device_detect,
            device_reader::get_last_device_info,
            device_reader::get_last_scan_profile,
            device_reader::read_device_info_deep,
            device_reader::read_device_logs,
            device_reader::list_saved_devices,
            device_reader::get_saved_device,
            device_reader::save_device_profile,
            device_reader::delete_saved_device,
            // telegram bot integration
            telegram::tg_get_config,
            telegram::tg_save_config,
            telegram::tg_check_token,
            telegram::tg_send_test_message,
            telegram::tg_poll_updates_once,
            telegram::tg_start_listener,
            telegram::tg_stop_listener,
            telegram::tg_remove_channel,
            // builder & repacker
            builder::get_build_targets,
            builder::repack_partition,
            builder::repack_super,
            builder::build_fastboot_package,
            // porting engine
            porting::compare_projects,
            porting::apply_porting_transfer,
            // file manager
            file_manager::fs_list_dir,
            file_manager::fs_read_file,
            file_manager::fs_write_file,
            file_manager::fs_create_file,
            file_manager::fs_create_folder,
            file_manager::fs_delete_entry,
            file_manager::fs_rename_entry,
            file_manager::fs_copy_entry
        ])
        .run(tauri::generate_context!())
        .expect("error while running MEYAFORGE");
}
