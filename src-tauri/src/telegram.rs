//! Модуль интеграции с Telegram ботом для уведомлений о сборке, релизах, импорте и диагностике.
//! Поддерживает живое обновление прогресса сборки (редактирование сообщений)
//! и привязку каналов через команду `/meyakitchen apply <trigger>` прямо в Telegram.

use crate::util::{dir, es};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::{self, sleep};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ChannelBinding {
    pub chat_id: i64,
    pub title: String,
    pub chat_type: String, // "channel", "supergroup", "group", "private"
    pub triggers: Vec<String>, // "all", "build", "release", "import", "device"
    pub added_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TriggerEvents {
    pub notify_build_start: bool,
    pub notify_build_progress: bool,
    pub notify_build_done: bool,
    pub notify_import: bool,
    pub notify_device_scan: bool,
    pub notify_release: bool,
}

impl Default for TriggerEvents {
    fn default() -> Self {
        Self {
            notify_build_start: true,
            notify_build_progress: true,
            notify_build_done: true,
            notify_import: true,
            notify_device_scan: true,
            notify_release: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub bot_username: String,
    pub bot_name: String,
    pub enabled: bool,
    pub poll_active: bool,
    pub channels: Vec<ChannelBinding>,
    pub events: TriggerEvents,
}

#[derive(Clone, Debug)]
struct LiveMessage {
    chat_id: i64,
    message_id: i64,
    last_edit: Instant,
}

// Активные сообщения о текущих сборках для живого редактирования
static LIVE_BUILD_MSGS: Mutex<Option<HashMap<String, Vec<LiveMessage>>>> = Mutex::new(None);

// Флаг работы фонового слушателя getUpdates
static LISTENER_RUNNING: AtomicBool = AtomicBool::new(false);

fn config_path() -> PathBuf {
    dir("").join("telegram.json")
}

fn current_timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn load_config_internal() -> TelegramConfig {
    let p = config_path();
    if p.is_file() {
        if let Ok(text) = fs::read_to_string(p) {
            if let Ok(cfg) = serde_json::from_str::<TelegramConfig>(&text) {
                return cfg;
            }
        }
    }
    TelegramConfig::default()
}

pub fn save_config_internal(cfg: &TelegramConfig) -> Result<(), String> {
    let p = config_path();
    let text = serde_json::to_string_pretty(cfg).map_err(es)?;
    fs::write(p, text).map_err(es)
}

#[tauri::command]
pub fn tg_get_config() -> Result<TelegramConfig, String> {
    Ok(load_config_internal())
}

#[tauri::command]
pub fn tg_save_config(config: TelegramConfig) -> Result<(), String> {
    save_config_internal(&config)
}

#[derive(Serialize)]
pub struct BotInfo {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub username: String,
}

/// Проверка валидности токена бота через Telegram getMe
#[tauri::command]
pub fn tg_check_token(token: String) -> Result<BotInfo, String> {
    let clean = token.trim();
    if clean.is_empty() {
        return Err("Токен бота не может быть пустым".into());
    }

    let url = format!("https://api.telegram.org/bot{clean}/getMe");
    let resp = ureq::get(&url)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| format!("Ошибка подключения к Telegram API: {e}"))?;

    let json: serde_json::Value = resp.into_json().map_err(es)?;
    if !json["ok"].as_bool().unwrap_or(false) {
        let desc = json["description"].as_str().unwrap_or("Неизвестная ошибка");
        return Err(format!("Telegram API вернул ошибку: {desc}"));
    }

    let res = &json["result"];
    let id = res["id"].as_i64().unwrap_or(0);
    let is_bot = res["is_bot"].as_bool().unwrap_or(true);
    let first_name = res["first_name"].as_str().unwrap_or("").to_string();
    let username = res["username"].as_str().unwrap_or("").to_string();

    // Автоматически обновляем bot_username в конфиге
    let mut cfg = load_config_internal();
    cfg.bot_token = clean.to_string();
    cfg.bot_username = username.clone();
    cfg.bot_name = first_name.clone();
    let _ = save_config_internal(&cfg);

    Ok(BotInfo { id, is_bot, first_name, username })
}

/// Отправка сообщения в Telegram
fn send_msg(token: &str, chat_id: i64, text: &str) -> Result<i64, String> {
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "HTML",
        "disable_web_page_preview": true
    });

    let resp = ureq::post(&url)
        .timeout(Duration::from_secs(10))
        .send_json(payload)
        .map_err(|e| format!("Не удалось отправить сообщение: {e}"))?;

    let json: serde_json::Value = resp.into_json().map_err(es)?;
    if json["ok"].as_bool().unwrap_or(false) {
        let msg_id = json["result"]["message_id"].as_i64().unwrap_or(0);
        Ok(msg_id)
    } else {
        let desc = json["description"].as_str().unwrap_or("Ошибка");
        Err(format!("Telegram API: {desc}"))
    }
}

/// Редактирование существующего сообщения
fn edit_msg(token: &str, chat_id: i64, message_id: i64, text: &str) -> Result<(), String> {
    let url = format!("https://api.telegram.org/bot{token}/editMessageText");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "text": text,
        "parse_mode": "HTML",
        "disable_web_page_preview": true
    });

    let _ = ureq::post(&url).timeout(Duration::from_secs(8)).send_json(payload);
    Ok(())
}

/// Тестовое сообщение в выбранный канал или во все привязанные
#[tauri::command]
pub fn tg_send_test_message(chat_id: Option<i64>) -> Result<String, String> {
    let cfg = load_config_internal();
    if cfg.bot_token.trim().is_empty() {
        return Err("Токен Telegram бота не настроен".into());
    }

    let targets: Vec<i64> = if let Some(id) = chat_id {
        vec![id]
    } else {
        cfg.channels.iter().map(|c| c.chat_id).collect()
    };

    if targets.is_empty() {
        return Err("Нет привязанных каналов для отправки. Напишите /meyakitchen apply в канале или группе.".into());
    }

    let text = "✨ <b>MEYAFORGE Тестовое уведомление</b> ✨\n\nБот успешно подключён к кухне прошивок и готов передавать статусы сборки, разборки и релизов!";
    let mut sent = 0;
    for cid in targets {
        if send_msg(&cfg.bot_token, cid, text).is_ok() {
            sent += 1;
        }
    }

    Ok(format!("Тестовое сообщение успешно отправлено в {sent} чат(ов)!"))
}

#[tauri::command]
pub fn tg_remove_channel(chat_id: i64) -> Result<(), String> {
    let mut cfg = load_config_internal();
    cfg.channels.retain(|c| c.chat_id != chat_id);
    save_config_internal(&cfg)
}

// ---------------- Обработка команд в Telegram (/meyakitchen apply <trigger>) ----------------

/// Опрос getUpdates и привязка каналов
#[tauri::command]
pub fn tg_poll_updates_once(app: AppHandle) -> Result<usize, String> {
    let mut cfg = load_config_internal();
    if cfg.bot_token.trim().is_empty() {
        return Err("Токен бота не настроен".into());
    }

    let url = format!("https://api.telegram.org/bot{}/getUpdates?timeout=0", cfg.bot_token);
    let resp = ureq::get(&url)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| format!("getUpdates error: {e}"))?;

    let json: serde_json::Value = resp.into_json().map_err(es)?;
    if !json["ok"].as_bool().unwrap_or(false) {
        return Ok(0);
    }

    let updates = match json["result"].as_array() {
        Some(a) => a,
        None => return Ok(0),
    };

    let mut highest_update_id = 0i64;
    let mut added_or_updated = 0;

    for update in updates {
        if let Some(uid) = update["update_id"].as_i64() {
            if uid > highest_update_id {
                highest_update_id = uid;
            }
        }

        // Сообщение может быть message или channel_post
        let msg = if update["message"].is_object() {
            &update["message"]
        } else if update["channel_post"].is_object() {
            &update["channel_post"]
        } else {
            continue;
        };

        let text = msg["text"].as_str().unwrap_or("").trim();
        let chat = &msg["chat"];
        let chat_id = match chat["id"].as_i64() {
            Some(id) => id,
            None => continue,
        };

        let chat_type = chat["type"].as_str().unwrap_or("channel").to_string();
        let chat_title = chat["title"]
            .as_str()
            .or_else(|| chat["username"].as_str())
            .or_else(|| chat["first_name"].as_str())
            .unwrap_or("Без названия")
            .to_string();

        // Проверяем команду /meyakitchen
        let lower = text.to_lowercase();
        if lower.starts_with("/meyakitchen") {
            let parts: Vec<&str> = text.split_whitespace().collect();
            if parts.len() >= 2 && parts[1].eq_ignore_ascii_case("apply") {
                let trigger_name = if parts.len() >= 3 {
                    parts[2].to_lowercase()
                } else {
                    "all".to_string()
                };

                let valid_trigger = match trigger_name.as_str() {
                    "build" | "release" | "import" | "device" | "all" => trigger_name,
                    _ => "all".to_string(),
                };

                // Добавляем или обновляем привязку канала
                if let Some(existing) = cfg.channels.iter_mut().find(|c| c.chat_id == chat_id) {
                    existing.title = chat_title.clone();
                    if !existing.triggers.contains(&valid_trigger) {
                        existing.triggers.push(valid_trigger.clone());
                    }
                } else {
                    cfg.channels.push(ChannelBinding {
                        chat_id,
                        title: chat_title.clone(),
                        chat_type: chat_type.clone(),
                        triggers: vec![valid_trigger.clone()],
                        added_at: current_timestamp(),
                    });
                }
                added_or_updated += 1;

                // Отправляем подтверждение в чат
                let reply = format!(
                    "✅ <b>MEYAFORGE Подключение успешно!</b>\n\nЧат/канал «<b>{}</b>» успешно привязан к триггеру <code>{}</code>.\nТеперь сюда будут приходить уведомления из кухни прошивок!",
                    chat_title, valid_trigger
                );
                let _ = send_msg(&cfg.bot_token, chat_id, &reply);
            } else if parts.len() >= 2 && parts[1].eq_ignore_ascii_case("status") {
                let current_triggers = cfg
                    .channels
                    .iter()
                    .find(|c| c.chat_id == chat_id)
                    .map(|c| c.triggers.join(", "))
                    .unwrap_or_else(|| "Канал не привязан".into());

                let reply = format!(
                    "ℹ️ <b>MEYAFORGE Статус</b>\nЧат: <b>{}</b> (ID: <code>{}</code>)\nАктивные триггеры: <b>{}</b>",
                    chat_title, chat_id, current_triggers
                );
                let _ = send_msg(&cfg.bot_token, chat_id, &reply);
            } else if parts.len() >= 2 && parts[1].eq_ignore_ascii_case("remove") {
                cfg.channels.retain(|c| c.chat_id != chat_id);
                let reply = "🗑️ Чат успешно отвязан от уведомлений MEYAFORGE.";
                let _ = send_msg(&cfg.bot_token, chat_id, reply);
                added_or_updated += 1;
            } else {
                let help = "<b>Команды MEYAFORGE:</b>\n• <code>/meyakitchen apply &lt;trigger&gt;</code> — привязать канал (триггеры: <code>all</code>, <code>build</code>, <code>release</code>, <code>import</code>, <code>device</code>)\n• <code>/meyakitchen status</code> — статус привязки\n• <code>/meyakitchen remove</code> — отвязать чат";
                let _ = send_msg(&cfg.bot_token, chat_id, help);
            }
        }
    }

    // Подтверждаем прочитанные апдейты в Telegram (сдвигаем offset)
    if highest_update_id > 0 {
        let ack_url = format!(
            "https://api.telegram.org/bot{}/getUpdates?offset={}&limit=1",
            cfg.bot_token,
            highest_update_id + 1
        );
        let _ = ureq::get(&ack_url).timeout(Duration::from_secs(5)).call();
    }

    if added_or_updated > 0 {
        let _ = save_config_internal(&cfg);
        let _ = app.emit("tg-channels-updated", &cfg.channels);
    }

    Ok(added_or_updated)
}

/// Запуск постоянного фонового слушателя getUpdates
#[tauri::command]
pub fn tg_start_listener(app: AppHandle) -> Result<(), String> {
    if LISTENER_RUNNING.load(Ordering::SeqCst) {
        return Ok(());
    }

    let mut cfg = load_config_internal();
    if cfg.bot_token.trim().is_empty() {
        return Err("Укажите и сохраните токен бота перед запуском слушателя".into());
    }

    LISTENER_RUNNING.store(true, Ordering::SeqCst);
    cfg.poll_active = true;
    let _ = save_config_internal(&cfg);

    thread::spawn(move || {
        while LISTENER_RUNNING.load(Ordering::SeqCst) {
            let _ = tg_poll_updates_once(app.clone());
            sleep(Duration::from_secs(3));
        }
    });

    Ok(())
}

/// Остановка фонового слушателя
#[tauri::command]
pub fn tg_stop_listener() -> Result<(), String> {
    LISTENER_RUNNING.store(false, Ordering::SeqCst);
    let mut cfg = load_config_internal();
    cfg.poll_active = false;
    let _ = save_config_internal(&cfg);
    Ok(())
}

// ---------------- Публичные триггеры для вызова из модулей сборки ----------------

fn get_active_channels_for_trigger(cfg: &TelegramConfig, trigger: &str) -> Vec<i64> {
    if !cfg.enabled || cfg.bot_token.trim().is_empty() {
        return Vec::new();
    }
    cfg.channels
        .iter()
        .filter(|c| c.triggers.iter().any(|t| t == "all" || t == trigger))
        .map(|c| c.chat_id)
        .collect()
}

/// Триггер начала сборки: создаёт сообщение и запоминает его id для каждого канала
pub fn on_build_start(project: &str) {
    let cfg = load_config_internal();
    if !cfg.events.notify_build_start {
        return;
    }

    let channels = get_active_channels_for_trigger(&cfg, "build");
    if channels.is_empty() {
        return;
    }

    let text = format!(
        "🔨 <b>Сборка прошивки: {}</b>\n\n⚙️ <b>Статус:</b> Инициализация сборки...\n📊 <b>Прогресс:</b> [░░░░░░░░░░] 0%\n🕒 <b>Время:</b> Just now",
        project
    );

    let mut list = Vec::new();
    for cid in channels {
        if let Ok(msg_id) = send_msg(&cfg.bot_token, cid, &text) {
            list.push(LiveMessage {
                chat_id: cid,
                message_id: msg_id,
                last_edit: Instant::now(),
            });
        }
    }

    let mut guard = LIVE_BUILD_MSGS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(project.to_string(), list);
}

/// Триггер этапа сборки: редактирует существующее сообщение
pub fn on_build_progress(project: &str, stage: &str, percent: u32) {
    let cfg = load_config_internal();
    if !cfg.events.notify_build_progress {
        return;
    }

    let mut guard = LIVE_BUILD_MSGS.lock().unwrap();
    let map = match guard.as_mut() {
        Some(m) => m,
        None => return,
    };

    let msgs = match map.get_mut(project) {
        Some(v) => v,
        None => return,
    };

    // Ограничение частоты редактирования Telegram (не чаще чем раз в 1.5 сек на сообщение)
    let now = Instant::now();
    let p_clamped = percent.min(100);
    let filled = (p_clamped / 10) as usize;
    let empty = 10usize.saturating_sub(filled);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

    let text = format!(
        "🔨 <b>Сборка прошивки: {}</b>\n\n⚙️ <b>Этап:</b> {}\n📊 <b>Прогресс:</b> [{}] {}%\n⏳ <i>Выполняется сборка компонентов...</i>",
        project, stage, bar, p_clamped
    );

    for m in msgs.iter_mut() {
        if now.duration_since(m.last_edit) >= Duration::from_millis(1500) {
            let _ = edit_msg(&cfg.bot_token, m.chat_id, m.message_id, &text);
            m.last_edit = now;
        }
    }
}

/// Триггер завершения сборки: финальное редактирование сообщения
pub fn on_build_done(project: &str, file_name: &str, size_human: &str, md5: &str, duration_str: &str, author: &str) {
    let cfg = load_config_internal();
    if !cfg.events.notify_build_done {
        return;
    }

    let mut guard = LIVE_BUILD_MSGS.lock().unwrap();
    if let Some(map) = guard.as_mut() {
        if let Some(msgs) = map.remove(project) {
            let author_line = if !author.is_empty() {
                format!("\n👤 <b>Автор:</b> {}", author)
            } else {
                String::new()
            };

            let md5_line = if !md5.is_empty() {
                format!("\n🔒 <b>MD5:</b> <code>{}</code>", md5)
            } else {
                String::new()
            };

            let text = format!(
                "✅ <b>Сборка прошивки успешно завершена!</b>\n\n📦 <b>Проект:</b> {}\n📁 <b>Файл:</b> <code>{}</code> ({})\n⏱️ <b>Время сборки:</b> {}{}{}",
                project, file_name, size_human, duration_str, md5_line, author_line
            );

            for m in msgs {
                let _ = edit_msg(&cfg.bot_token, m.chat_id, m.message_id, &text);
            }
        }
    }
}

/// Триггер сбоя сборки: финальное редактирование сообщения
pub fn on_build_fail(project: &str, stage: &str, error: &str) {
    let cfg = load_config_internal();
    let mut guard = LIVE_BUILD_MSGS.lock().unwrap();
    if let Some(map) = guard.as_mut() {
        if let Some(msgs) = map.remove(project) {
            let text = format!(
                "❌ <b>Ошибка при сборке прошивки: {}</b>\n\n⚠️ <b>Сбой на этапе:</b> {}\nПричина: <code>{}</code>",
                project, stage, error
            );
            for m in msgs {
                let _ = edit_msg(&cfg.bot_token, m.chat_id, m.message_id, &text);
            }
        }
    }
}

/// Триггер успешного импорта прошивки
pub fn on_firmware_imported(project: &str, filename: &str, parts_count: usize) {
    let cfg = load_config_internal();
    if !cfg.events.notify_import {
        return;
    }

    let channels = get_active_channels_for_trigger(&cfg, "import");
    if channels.is_empty() {
        return;
    }

    let text = format!(
        "📥 <b>Импорт и разборка прошивки</b>\n\n📦 <b>Проект:</b> {}\n📄 <b>Исходник:</b> <code>{}</code>\n🧩 <b>Извлечено разделов:</b> {}",
        project, filename, parts_count
    );

    for cid in channels {
        let _ = send_msg(&cfg.bot_token, cid, &text);
    }
}

/// Триггер сохранения слепка устройства
pub fn on_device_scanned(brand: &str, model: &str, android: &str, parts_count: usize, success: bool) {
    let cfg = load_config_internal();
    if !cfg.events.notify_device_scan {
        return;
    }

    let channels = get_active_channels_for_trigger(&cfg, "device");
    if channels.is_empty() {
        return;
    }

    let status_icon = if success { "✅" } else { "⚠️" };
    let text = format!(
        "📱 <b>Слепок устройства {}</b>\n\n<b>Устройство:</b> {} {}\n<b>Версия Android:</b> {}\n<b>Разделов в таблице:</b> {}\n<b>Статус:</b> {}",
        status_icon,
        brand,
        model,
        android,
        parts_count,
        if success { "Полный анализ (ADB + Fastboot)" } else { "Частичный слепок (с ошибкой)" }
    );

    for cid in channels {
        let _ = send_msg(&cfg.bot_token, cid, &text);
    }
}
