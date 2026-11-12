//! Менеджер внешних инструментов: скачивание/обновление из GitHub Releases или по прямой ссылке.
//! Реестр по умолчанию встроен; его можно переопределить файлом `data/tools/registry.json`.

use crate::util::{copy_progress, dir, es, spawn_task, Ctx};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

const DEFAULT_REGISTRY: &str = r#"[
  {
    "id": "magiskboot",
    "name": "MagiskBoot",
    "category": "Boot / Recovery",
    "desc": "Распаковка, патчинг, модификация и сборка boot.img, recovery.img, ramdisk.cpio, kernel_dtb и dtb",
    "repo": "affggh/magiskboot_build",
    "match": ["windows", "x86_64-standalone"],
    "bins": ["magiskboot.exe"]
  },
  {
    "id": "boot-image-editor",
    "name": "Android Boot Image Editor",
    "category": "Boot / Recovery",
    "desc": "Детальный разбор структуры boot/vendor_boot/recovery и генерация dts",
    "repo": "cfig/Android_boot_image_editor",
    "match": ["boot_editor"],
    "bins": ["gradlew.bat"]
  },
  {
    "id": "apktool",
    "name": "Apktool",
    "category": "APK Modding",
    "desc": "Декомпиляция APK/JAR в Smali, редактирование ресурсов (res/values/xml) и обратная сборка",
    "repo": "iBotPeaches/Apktool",
    "match": [".jar"],
    "bins": ["apktool.jar"]
  },
  {
    "id": "apk-editor",
    "name": "APKEditor",
    "category": "APK Modding",
    "desc": "Быстрый разбор ресурсов arsc, XML декодирование, слияние и сплит-APK модификации",
    "repo": "REAndroid/APKEditor",
    "match": [".jar"],
    "bins": ["APKEditor.jar"]
  },
  {
    "id": "uber-apk-signer",
    "name": "Uber APK Signer",
    "category": "APK Modding",
    "desc": "Подпись APK/Zip v1, v2, v3 и v4 схемами с автоматическим выравниванием zipalign",
    "repo": "patrickfav/uber-apk-signer",
    "match": [".jar"],
    "bins": ["uber-apk-signer.jar"]
  },
  {
    "id": "jadx",
    "name": "JADX Decompiler",
    "category": "APK Modding",
    "desc": "Декомпилятор DEX в чистый Java исходный код с поддержкой GUI",
    "repo": "skylot/jadx",
    "match": ["jadx-gui", "win.zip"],
    "exclude": ["with-jre"],
    "bins": ["jadx-gui.bat", "jadx.bat"]
  },
  {
    "id": "payload-dumper-go",
    "name": "payload-dumper-go",
    "category": "Partitions",
    "desc": "Высокоскоростная распаковка payload.bin из A/B OTA-прошивок",
    "repo": "ssut/payload-dumper-go",
    "match": ["windows", "amd64"],
    "bins": ["payload-dumper-go.exe"]
  },
  {
    "id": "erofs-utils",
    "name": "erofs-utils",
    "category": "Partitions",
    "desc": "extract.erofs / mkfs.erofs — разборка и сборка высокопроизводительных разделов EROFS",
    "repo": "sekaiacg/erofs-utils",
    "match": ["x86_64"],
    "exclude": ["linux", "android", "darwin", "macos", "aarch64", "arm64"],
    "bins": ["extract.erofs.exe", "mkfs.erofs.exe"]
  },
  {
    "id": "platform-tools",
    "name": "Android Platform Tools",
    "category": "Flasher & Bridge",
    "desc": "Официальные adb и fastboot от Google для прошивки и отладки устройств",
    "url": "https://dl.google.com/android/repository/platform-tools-latest-windows.zip",
    "bins": ["adb.exe", "fastboot.exe"]
  },
  {
    "id": "lz4",
    "name": "LZ4 Compression",
    "category": "Compression",
    "desc": "Утилита сжатия/распаковки lz4 для образов boot.img и ramdisk",
    "repo": "lz4/lz4",
    "match": ["win64"],
    "bins": ["lz4.exe"]
  },
  {
    "id": "zstd",
    "name": "Zstandard (zstd)",
    "category": "Compression",
    "desc": "Сжатие алгоритмом Zstandard для современных OTA-пакетов и payload",
    "repo": "facebook/zstd",
    "match": ["win64.zip"],
    "bins": ["zstd.exe"]
  },
  {
    "id": "partition-tools",
    "name": "Partition Tools (lpmake)",
    "category": "Partitions",
    "desc": "lpmake, lpunpack, lpadd — упаковка и редактирование динамических разделов (super.img)",
    "url": "https://raw.githubusercontent.com/affggh/lpmake_and_lpunpack_cygwin/master/partition_tools_cygwin64_affggh_static.zip",
    "bins": ["lpmake.exe", "lpunpack.exe", "cygwin1.dll"]
  }
]"#;

#[derive(Deserialize, Serialize, Clone)]
pub struct ToolDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(rename = "match", default)]
    pub matches: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub bins: Vec<String>,
}

#[derive(Serialize)]
pub struct ToolInfo {
    #[serde(flatten)]
    def: ToolDef,
    installed: Option<String>,
    ok: bool,
    path: String,
}

fn tools_dir() -> PathBuf {
    dir("tools")
}

fn registry() -> Vec<ToolDef> {
    let user = tools_dir().join("registry.json");
    if let Ok(s) = fs::read_to_string(&user) {
        if let Ok(v) = serde_json::from_str(&s) {
            return v;
        }
    }
    serde_json::from_str(DEFAULT_REGISTRY).unwrap()
}

fn installed() -> HashMap<String, String> {
    fs::read_to_string(tools_dir().join("installed.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_installed(m: &HashMap<String, String>) {
    let _ = fs::write(tools_dir().join("installed.json"), serde_json::to_string_pretty(m).unwrap());
}

fn find(d: &Path, name: &str) -> Option<PathBuf> {
    for e in fs::read_dir(d).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(r) = find(&p, name) {
                return Some(r);
            }
        } else if p.file_name().map_or(false, |n| n.to_string_lossy().eq_ignore_ascii_case(name)) {
            return Some(p);
        }
    }
    None
}

/// Путь к исполняемому файлу инструмента.
pub fn bin(id: &str, exe: &str) -> Result<PathBuf, String> {
    find(&tools_dir().join(id), exe)
        .ok_or_else(|| format!("не найден {exe}. Установите «{id}» в разделе «Инструменты»"))
}

#[tauri::command]
pub fn tools_list() -> Vec<ToolInfo> {
    let inst = installed();
    registry()
        .into_iter()
        .map(|d| {
            let p = tools_dir().join(&d.id);
            let ok = !d.bins.is_empty() && d.bins.iter().all(|b| find(&p, b).is_some());
            ToolInfo { installed: inst.get(&d.id).cloned(), ok, path: p.to_string_lossy().into(), def: d }
        })
        .collect()
}

struct Release {
    tag: String,
    assets: Vec<(String, String)>,
}

fn gh_latest(repo: &str) -> Result<Release, String> {
    let j: serde_json::Value = ureq::get(&format!("https://api.github.com/repos/{repo}/releases/latest"))
        .set("User-Agent", "MeyaForge")
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(es)?
        .into_json()
        .map_err(es)?;
    let assets = j["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok(Release { tag: j["tag_name"].as_str().unwrap_or("?").to_string(), assets })
}

fn pick(def: &ToolDef, assets: &[(String, String)]) -> Option<(String, String)> {
    assets
        .iter()
        .find(|(n, _)| {
            let l = n.to_lowercase();
            [".zip", ".tar.gz", ".tgz", ".exe", ".jar"].iter().any(|e| l.ends_with(e))
                && def.matches.iter().all(|m| l.contains(&m.to_lowercase()))
                && !def.exclude.iter().any(|m| l.contains(&m.to_lowercase()))
        })
        .cloned()
}

/// Получение последней версии конкретного инструмента (id) с GitHub
#[tauri::command]
pub async fn tool_check_latest(id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let def = registry()
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(|| "Инструмент не найден".to_string())?;

        match (&def.repo, &def.url) {
            (Some(r), _) => match gh_latest(r) {
                Ok(rel) if pick(&def, &rel.assets).is_some() => Ok(rel.tag),
                Ok(rel) => Ok(format!("нет подходящего файла в {}", rel.tag)),
                Err(e) => Err(format!("ошибка: {e}")),
            },
            (None, Some(_)) => Ok("latest".into()),
            _ => Err("нет источника".into()),
        }
    })
    .await
    .map_err(es)
    .and_then(|r| r)
}

fn download(ctx: &Ctx, url: &str, dst: &Path) -> Result<(), String> {
    ctx.log(format!("  загрузка {url}"));
    let resp = ureq::get(url).set("User-Agent", "MeyaForge").call().map_err(es)?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut w = BufWriter::new(File::create(dst).map_err(es)?);
    let n = copy_progress(ctx, &mut resp.into_reader(), &mut w, total).map_err(es)?;
    w.flush().map_err(es)?;
    ctx.log(format!("  получено {:.1} МБ", n as f64 / 1048576.0));
    Ok(())
}

pub fn unzip_all(path: &Path, to: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(File::open(path).map_err(es)?).map_err(es)?;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(es)?;
        let Some(rel) = e.enclosed_name() else { continue };
        let out = to.join(rel);
        if e.is_dir() {
            fs::create_dir_all(&out).map_err(es)?;
        } else {
            if let Some(p) = out.parent() {
                fs::create_dir_all(p).map_err(es)?;
            }
            std::io::copy(&mut e, &mut File::create(&out).map_err(es)?).map_err(es)?;
        }
    }
    Ok(())
}

fn install(ctx: &Ctx, def: &ToolDef) -> Result<String, String> {
    let (version, name, url) = match (&def.repo, &def.url) {
        (Some(repo), _) => {
            ctx.log(format!("  GitHub: {repo}"));
            let rel = gh_latest(repo)?;
            let (n, u) = pick(def, &rel.assets).ok_or_else(|| {
                let list: Vec<_> = rel.assets.iter().map(|a| a.0.as_str()).collect();
                format!("в релизе {} нет подходящего файла. Есть: {}", rel.tag, list.join(", "))
            })?;
            (rel.tag, n, u)
        }
        (None, Some(u)) => ("latest".to_string(), u.rsplit('/').next().unwrap_or("tool.zip").to_string(), u.clone()),
        _ => return Err("у инструмента не задан источник".into()),
    };
    let tmp = crate::util::dir("tmp").join(format!("meyaforge-{}", name));
    download(ctx, &url, &tmp)?;
    let target = tools_dir().join(&def.id);
    if target.exists() {
        fs::remove_dir_all(&target).map_err(es)?;
    }
    fs::create_dir_all(&target).map_err(es)?;
    ctx.stage(format!("Установка {}", def.name));
    let l = name.to_lowercase();
    if l.ends_with(".zip") {
        unzip_all(&tmp, &target)?;
    } else if l.ends_with(".tar.gz") || l.ends_with(".tgz") {
        let gz = flate2::read::GzDecoder::new(File::open(&tmp).map_err(es)?);
        tar::Archive::new(gz).unpack(&target).map_err(es)?;
    } else if l.ends_with(".jar") {
        // Если это jar (например apktool_3.0.3.jar), сохраняем как каноническое имя или как в def.bins
        let canonical_name = def.bins.first().cloned().unwrap_or(name.clone());
        fs::copy(&tmp, target.join(&canonical_name)).map_err(es)?;
    } else {
        fs::copy(&tmp, target.join(&name)).map_err(es)?;
    }
    let _ = fs::remove_file(&tmp);
    let missing: Vec<_> = def.bins.iter().filter(|b| find(&target, b).is_none()).cloned().collect();
    if !missing.is_empty() {
        ctx.err(format!("  внимание: в архиве не найдены {}", missing.join(", ")));
    }
    let mut m = installed();
    m.insert(def.id.clone(), version.clone());
    save_installed(&m);
    Ok(format!("{} {} установлен", def.name, version))
}

#[tauri::command]
pub fn tool_install(app: AppHandle, id: String) -> Result<(), String> {
    let def = registry().into_iter().find(|d| d.id == id).ok_or("неизвестный инструмент")?;
    spawn_task(app, format!("Загрузка {}", def.name), move |ctx| install(ctx, &def))
}

#[tauri::command]
pub fn tool_remove(id: String) -> Result<(), String> {
    let d = tools_dir().join(&id);
    if d.exists() {
        fs::remove_dir_all(&d).map_err(es)?;
    }
    let mut m = installed();
    m.remove(&id);
    save_installed(&m);
    Ok(())
}
