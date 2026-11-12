//! Полноценный файловый менеджер проектов:
//!   - Безопасная навигация по дереву файлов проектов (input, images, extracted, config, out).
//!   - Чтение, сохранение, создание, переименование, копирование и удаление файлов и папок.
//!   - Определение бинарных и текстовых форматов для встроенного редактора.

use crate::util::{dir, es};
use serde::Serialize;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

fn resolve_safe_path(project: &str, rel_path: Option<&str>) -> Result<(PathBuf, PathBuf), String> {
    if project.is_empty() || project.contains(['/', '\\']) || project.contains("..") {
        return Err("Недопустимое имя проекта".into());
    }
    let p_dir = dir("projects").join(project);
    if !p_dir.is_dir() {
        return Err(format!("Проект «{project}» не найден"));
    }

    let rel = rel_path.unwrap_or("").trim_start_matches(['/', '\\']);
    let target = if rel.is_empty() {
        p_dir.clone()
    } else {
        p_dir.join(rel)
    };

    // Нормализация и защита от выхода за пределы проекта
    let norm_target = target.canonicalize().unwrap_or_else(|_| target.clone());
    let norm_proj = p_dir.canonicalize().unwrap_or_else(|_| p_dir.clone());

    if !norm_target.starts_with(&norm_proj) {
        return Err("Доступ за пределы каталога проекта запрещён".into());
    }

    Ok((p_dir, target))
}

#[derive(Serialize, Clone, Debug)]
pub struct FsEntry {
    pub name: String,
    pub rel_path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
    pub ext: String,
    pub has_children: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct FsFileData {
    pub name: String,
    pub rel_path: String,
    pub size: u64,
    pub is_binary: bool,
    pub content: Option<String>,
    pub ext: String,
}

/// Список файлов и папок в заданной директории проекта
#[tauri::command]
pub fn fs_list_dir(project: String, rel_path: Option<String>) -> Result<Vec<FsEntry>, String> {
    let (p_dir, target) = resolve_safe_path(&project, rel_path.as_deref())?;
    if !target.is_dir() {
        return Err("Указанный путь не является директорией".into());
    }

    let mut entries = Vec::new();
    let read_dir = fs::read_dir(&target).map_err(es)?;

    for entry in read_dir.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Пропускаем скрытые файлы .git и системные метаданные
        if name.starts_with('.') {
            continue;
        }

        let is_dir = p.is_dir();
        let meta = entry.metadata().ok();
        let size = if is_dir { 0 } else { meta.as_ref().map(|m| m.len()).unwrap_or(0) };
        let modified = meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let ext = if is_dir {
            String::new()
        } else {
            p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
        };

        let has_children = if is_dir {
            fs::read_dir(&p).map(|mut r| r.next().is_some()).unwrap_or(false)
        } else {
            false
        };

        let rel = match p.strip_prefix(&p_dir) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => name.clone(),
        };

        entries.push(FsEntry {
            name,
            rel_path: rel,
            is_dir,
            size,
            modified,
            ext,
            has_children,
        });
    }

    // Сортировка: сначала папки, затем файлы в алфавитном порядке
    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    Ok(entries)
}

/// Чтение файла для встроенного редактора
#[tauri::command]
pub fn fs_read_file(project: String, rel_path: String) -> Result<FsFileData, String> {
    let (_, target) = resolve_safe_path(&project, Some(&rel_path))?;
    if !target.is_file() {
        return Err("Файл не найден".into());
    }

    let meta = target.metadata().map_err(es)?;
    let size = meta.len();
    let name = target.file_name().unwrap_or_default().to_string_lossy().to_string();
    let ext = target.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();

    // Ограничение на редактирование текстовых файлов: 8 МБ
    if size > 8 * 1024 * 1024 {
        return Ok(FsFileData {
            name,
            rel_path,
            size,
            is_binary: true,
            content: None,
            ext,
        });
    }

    let mut file = File::open(&target).map_err(es)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).map_err(es)?;

    // Проверка на бинарный файл (нулевой байт в первых 8 КБ)
    let check_len = buf.len().min(8192);
    let is_binary = buf[..check_len].contains(&0);

    if is_binary {
        Ok(FsFileData {
            name,
            rel_path,
            size,
            is_binary: true,
            content: None,
            ext,
        })
    } else {
        let text = String::from_utf8_lossy(&buf).to_string();
        Ok(FsFileData {
            name,
            rel_path,
            size,
            is_binary: false,
            content: Some(text),
            ext,
        })
    }
}

/// Сохранение текстового файла из редактора
#[tauri::command]
pub fn fs_write_file(project: String, rel_path: String, content: String) -> Result<(), String> {
    let (_, target) = resolve_safe_path(&project, Some(&rel_path))?;
    if target.is_dir() {
        return Err("Путь указывает на папку, а не на файл".into());
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(es)?;
    }

    fs::write(&target, content).map_err(es)
}

/// Создание нового файла
#[tauri::command]
pub fn fs_create_file(project: String, rel_path: String) -> Result<(), String> {
    let (_, target) = resolve_safe_path(&project, Some(&rel_path))?;
    if target.exists() {
        return Err("Файл или папка с таким именем уже существует".into());
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(es)?;
    }

    File::create(&target).map_err(es)?;
    Ok(())
}

/// Создание новой директории
#[tauri::command]
pub fn fs_create_folder(project: String, rel_path: String) -> Result<(), String> {
    let (_, target) = resolve_safe_path(&project, Some(&rel_path))?;
    if target.exists() {
        return Err("Папка с таким именем уже существует".into());
    }

    fs::create_dir_all(&target).map_err(es)
}

/// Удаление файла или папки
#[tauri::command]
pub fn fs_delete_entry(project: String, rel_path: String) -> Result<(), String> {
    let (p_dir, target) = resolve_safe_path(&project, Some(&rel_path))?;
    if target == p_dir {
        return Err("Нельзя удалить корень проекта".into());
    }
    if !target.exists() {
        return Err("Объект не найден".into());
    }

    if target.is_dir() {
        fs::remove_dir_all(&target).map_err(es)
    } else {
        fs::remove_file(&target).map_err(es)
    }
}

/// Переименование / перемещение
#[tauri::command]
pub fn fs_rename_entry(project: String, old_rel_path: String, new_rel_path: String) -> Result<(), String> {
    let (_, src) = resolve_safe_path(&project, Some(&old_rel_path))?;
    let (_, dst) = resolve_safe_path(&project, Some(&new_rel_path))?;

    if !src.exists() {
        return Err("Исходный объект не найден".into());
    }
    if dst.exists() {
        return Err("Объект с новым именем уже существует".into());
    }

    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(es)?;
    }

    fs::rename(&src, &dst).map_err(es)
}

/// Копирование файла или директории
#[tauri::command]
pub fn fs_copy_entry(project: String, src_rel_path: String, dst_rel_path: String) -> Result<(), String> {
    let (_, src) = resolve_safe_path(&project, Some(&src_rel_path))?;
    let (_, dst) = resolve_safe_path(&project, Some(&dst_rel_path))?;

    if !src.exists() {
        return Err("Исходный объект не найден".into());
    }

    if src.is_dir() {
        copy_dir_recursive(&src, &dst)
    } else {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(es)?;
        }
        fs::copy(&src, &dst).map_err(es)?;
        Ok(())
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(es)?;
    for entry in fs::read_dir(src).map_err(es)?.flatten() {
        let p = entry.path();
        let d = dst.join(entry.file_name());
        if p.is_dir() {
            copy_dir_recursive(&p, &d)?;
        } else {
            fs::copy(&p, &d).map_err(es)?;
        }
    }
    Ok(())
}
