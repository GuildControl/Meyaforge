//! Модуль глубокой очистки прошивки (Debloat Studio):
//! сканирование приложений в extracted/ (system, product, system_ext, vendor, odm),
//! сопоставление с базой мусорных/рекламных пакетов по вендорам и удаление/бэкап.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BloatItem {
    pub package_name: String,
    pub app_name: String,
    pub rel_path: String,
    pub full_path: String,
    pub size_bytes: u64,
    pub category: String, // "ad_telemetry", "google_heavy", "vendor_bloat", "partner_apps"
    pub vendor: String,   // "xiaomi", "samsung", "google", "bbk", "transsion", "generic"
    pub description: String,
    pub safe_to_remove: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DebloatScanResult {
    pub project_name: String,
    pub total_apps_found: usize,
    pub bloat_apps_count: usize,
    pub total_bloat_bytes: u64,
    pub items: Vec<BloatItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DebloatActionRequest {
    pub project: String,
    pub target_paths: Vec<String>,
    pub backup_mode: bool, // если true — перемещает в projects/<name>/removed_bloat/
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DebloatActionResult {
    pub removed_count: usize,
    pub freed_bytes: u64,
    pub backup_dir: Option<String>,
    pub errors: Vec<String>,
}

struct KnownBloatDef {
    folder_match: &'static str,
    app_name: &'static str,
    vendor: &'static str,
    category: &'static str,
    description: &'static str,
    safe: bool,
}

// База данных известного мусора и аналитики
const KNOWN_BLOAT: &[KnownBloatDef] = &[
    // --- Xiaomi / MIUI / HyperOS ---
    KnownBloatDef {
        folder_match: "MSA",
        app_name: "MIUI System Ads (MSA)",
        vendor: "xiaomi",
        category: "ad_telemetry",
        description: "Основной рекламный демон MIUI/HyperOS, подгружающий баннеры в систему",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "Analytics",
        app_name: "Xiaomi Analytics",
        vendor: "xiaomi",
        category: "ad_telemetry",
        description: "Служба сбора телеметрии и аналитики использования системы",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "MiBrowserGlobal",
        app_name: "Mi Browser",
        vendor: "xiaomi",
        category: "vendor_bloat",
        description: "Предустановленный браузер Xiaomi со встроенной рекламой и лентой новостей",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "MiDrop",
        app_name: "ShareMe / MiDrop",
        vendor: "xiaomi",
        category: "vendor_bloat",
        description: "Утилита передачи файлов ShareMe",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "Joyose",
        app_name: "Joyose",
        vendor: "xiaomi",
        category: "ad_telemetry",
        description: "Сервис ограничения частоты процессора/FPS в играх и сбора статистики",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "YellowPage",
        app_name: "Жёлтые страницы (YellowPages)",
        vendor: "xiaomi",
        category: "vendor_bloat",
        description: "Китайский справочник организаций и рекламный определитель номеров",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "CleanMaster",
        app_name: "CleanMaster SDK",
        vendor: "xiaomi",
        category: "ad_telemetry",
        description: "Движок очистки со встроенными трекерами",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "GetApps",
        app_name: "GetApps / Mi Market",
        vendor: "xiaomi",
        category: "vendor_bloat",
        description: "Встроенный магазин приложений Xiaomi с постоянными уведомлениями",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "MiPay",
        app_name: "Mi Pay",
        vendor: "xiaomi",
        category: "vendor_bloat",
        description: "Платежный сервис Xiaomi (не используется за пределами Китая/Индии)",
        safe: true,
    },

    // --- Samsung / OneUI ---
    KnownBloatDef {
        folder_match: "Bixby",
        app_name: "Samsung Bixby Voice / Service",
        vendor: "samsung",
        category: "vendor_bloat",
        description: "Голосовой помощник Bixby и сопутствующие фоновые сервисы",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "SamsungCarMode",
        app_name: "Samsung Car Mode",
        vendor: "samsung",
        category: "vendor_bloat",
        description: "Автомобильный режим Samsung",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "GameOptimizingService",
        app_name: "Game Optimizing Service (GOS)",
        vendor: "samsung",
        category: "ad_telemetry",
        description: "Сервис замедления и троттлинга процессора в играх",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "AREmoji",
        app_name: "AR Emoji / AR Zone",
        vendor: "samsung",
        category: "vendor_bloat",
        description: "Анимированные AR стикеры камеры Samsung",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "SamsungPass",
        app_name: "Samsung Pass",
        vendor: "samsung",
        category: "vendor_bloat",
        description: "Менеджер паролей Samsung (не работает на кастомах без Knox)",
        safe: true,
    },

    // --- Google Apps (тяжелые медиа-пакеты) ---
    KnownBloatDef {
        folder_match: "YouTube",
        app_name: "YouTube",
        vendor: "google",
        category: "google_heavy",
        description: "Клиент YouTube (~150 МБ). Рекомендуется ставить кастомный или из Play Store",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "Music2",
        app_name: "YouTube Music",
        vendor: "google",
        category: "google_heavy",
        description: "Предустановленный плеер YouTube Music",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "Chrome",
        app_name: "Google Chrome",
        vendor: "google",
        category: "google_heavy",
        description: "Браузер Google Chrome (~200 МБ с WebView)",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "Maps",
        app_name: "Google Maps",
        vendor: "google",
        category: "google_heavy",
        description: "Карты Google Карты (~120 МБ)",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "Duo",
        app_name: "Google Meet / Duo",
        vendor: "google",
        category: "google_heavy",
        description: "Приложение видеозвонков Google Meet",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "Gmail2",
        app_name: "Gmail",
        vendor: "google",
        category: "google_heavy",
        description: "Почтовый клиент Gmail",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "Videos",
        app_name: "Google TV / Google Play Movies",
        vendor: "google",
        category: "google_heavy",
        description: "Сервис фильмов Google TV",
        safe: true,
    },

    // --- Партнерские приложения и соцсети ---
    KnownBloatDef {
        folder_match: "Facebook",
        app_name: "Facebook Services / App Installer",
        vendor: "partner",
        category: "partner_apps",
        description: "Фоновые трекеры и службы автоматической загрузки Facebook/Meta",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "Netflix",
        app_name: "Netflix Activation / Stub",
        vendor: "partner",
        category: "partner_apps",
        description: "Предустановленные заглушки и сервисы активации Netflix",
        safe: true,
    },
    KnownBloatDef {
        folder_match: "TikTok",
        app_name: "TikTok Stub",
        vendor: "partner",
        category: "partner_apps",
        description: "Рекламный установщик TikTok",
        safe: true,
    },
];

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                total += p.metadata().map(|m| m.len()).unwrap_or(0);
            } else if p.is_dir() {
                total += dir_size(&p);
            }
        }
    }
    total
}

/// Сканирование всех приложений в проекте
pub fn scan_project_apps(project_name: &str) -> Result<DebloatScanResult, String> {
    let pdir = crate::util::dir("projects").join(project_name);
    let ext_dir = pdir.join("extracted");
    if !ext_dir.is_dir() {
        return Err(format!("Папка extracted/ проекта «{project_name}» не найдена"));
    }

    // Сканируем системные каталоги приложений
    let target_subdirs = [
        "system/app",
        "system/priv-app",
        "system/system/app",
        "system/system/priv-app",
        "product/app",
        "product/priv-app",
        "system_ext/app",
        "system_ext/priv-app",
        "vendor/app",
        "odm/app",
    ];

    let mut items = Vec::new();
    let mut total_apps = 0;
    let mut bloat_bytes = 0u64;

    for sub in &target_subdirs {
        let app_folder = ext_dir.join(sub);
        if !app_folder.is_dir() {
            continue;
        }

        if let Ok(entries) = fs::read_dir(&app_folder) {
            for entry in entries.flatten() {
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                total_apps += 1;
                let folder_name = entry.file_name().to_string_lossy().to_string();
                let folder_lower = folder_name.to_lowercase();
                let size = dir_size(&p);

                let rel_path = p.strip_prefix(&ext_dir).unwrap_or(&p).to_string_lossy().to_string();

                // Поиск совпадений в базе
                let matched = KNOWN_BLOAT.iter().find(|def| {
                    folder_lower.contains(&def.folder_match.to_lowercase())
                });

                if let Some(def) = matched {
                    bloat_bytes += size;
                    items.push(BloatItem {
                        package_name: folder_name.clone(),
                        app_name: def.app_name.to_string(),
                        rel_path,
                        full_path: p.to_string_lossy().to_string(),
                        size_bytes: size,
                        category: def.category.to_string(),
                        vendor: def.vendor.to_string(),
                        description: def.description.to_string(),
                        safe_to_remove: def.safe,
                    });
                } else if folder_lower.contains("analytics")
                    || folder_lower.contains("telemetry")
                    || folder_lower.contains("partner")
                    || folder_lower.contains("feedback")
                    || folder_lower.contains("bugreport")
                {
                    bloat_bytes += size;
                    items.push(BloatItem {
                        package_name: folder_name.clone(),
                        app_name: format!("Обнаружен трекер ({folder_name})"),
                        rel_path,
                        full_path: p.to_string_lossy().to_string(),
                        size_bytes: size,
                        category: "ad_telemetry".into(),
                        vendor: "generic".into(),
                        description: "Подозрительное приложение аналитики/отчетов об ошибках".into(),
                        safe_to_remove: true,
                    });
                }
            }
        }
    }

    // Сортируем: сначала самые тяжелые
    items.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    let bloat_count = items.len();

    Ok(DebloatScanResult {
        project_name: project_name.to_string(),
        total_apps_found: total_apps,
        bloat_apps_count: bloat_count,
        total_bloat_bytes: bloat_bytes,
        items,
    })
}

#[tauri::command]
pub fn debloat_scan(project: String) -> Result<DebloatScanResult, String> {
    scan_project_apps(&project)
}

#[tauri::command]
pub fn debloat_remove(req: DebloatActionRequest) -> Result<DebloatActionResult, String> {
    let pdir = crate::util::dir("projects").join(&req.project);
    if !pdir.is_dir() {
        return Err(format!("Проект «{}» не найден", req.project));
    }

    let backup_dir = pdir.join("removed_bloat");
    if req.backup_mode {
        fs::create_dir_all(&backup_dir).map_err(|e| e.to_string())?;
    }

    let mut removed_count = 0;
    let mut freed_bytes = 0u64;
    let mut errors = Vec::new();

    for path_str in &req.target_paths {
        let p = Path::new(path_str);
        if !p.exists() {
            continue;
        }

        let size = if p.is_dir() { dir_size(p) } else { p.metadata().map(|m| m.len()).unwrap_or(0) };
        let folder_name = p.file_name().unwrap_or_default().to_string_lossy().to_string();

        if req.backup_mode {
            let target_backup = backup_dir.join(&folder_name);
            // Если в бэкапе уже есть такая папка, удаляем старую версию
            if target_backup.exists() {
                let _ = fs::remove_dir_all(&target_backup);
            }
            if let Err(e) = fs::rename(p, &target_backup) {
                // Если rename не сработал (например, между разделами), делаем copy + remove
                errors.push(format!("Ошибка перемещения {folder_name}: {e}"));
            } else {
                removed_count += 1;
                freed_bytes += size;
            }
        } else {
            let res = if p.is_dir() {
                fs::remove_dir_all(p)
            } else {
                fs::remove_file(p)
            };
            if let Err(e) = res {
                errors.push(format!("Ошибка удаления {folder_name}: {e}"));
            } else {
                removed_count += 1;
                freed_bytes += size;
            }
        }
    }

    Ok(DebloatActionResult {
        removed_count,
        freed_bytes,
        backup_dir: if req.backup_mode { Some(backup_dir.to_string_lossy().to_string()) } else { None },
        errors,
    })
}

#[tauri::command]
pub fn debloat_restore_all(project: String) -> Result<String, String> {
    let pdir = crate::util::dir("projects").join(&project);
    let backup_dir = pdir.join("removed_bloat");
    if !backup_dir.is_dir() {
        return Err("Папка резервных копий удаленных приложений removed_bloat/ не найдена".into());
    }

    let ext_dir = pdir.join("extracted");
    let mut restored_count = 0;

    if let Ok(entries) = fs::read_dir(&backup_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = e.file_name();
                // Восстанавливаем в system/app по умолчанию
                let dst = ext_dir.join("system/app").join(&name);
                if let Ok(()) = fs::rename(&p, &dst) {
                    restored_count += 1;
                }
            }
        }
    }

    Ok(format!("Восстановлено {restored_count} приложений из бэкапа"))
}
