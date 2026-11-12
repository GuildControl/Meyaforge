//! Общие утилиты: пути, логирование в терминал UI, фоновые задачи, запуск внешних утилит.

use serde::Serialize;
use std::cell::{Cell, RefCell};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tauri::{AppHandle, Emitter};

pub static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Clone)]
pub struct TermLine {
    pub kind: &'static str, // out | err | sys | exit
    pub text: String,
}

#[derive(Serialize, Clone)]
struct TaskEv {
    active: bool,
    label: String,
    pct: f64, // < 0 — неопределённый прогресс
}

pub fn emit(app: &AppHandle, kind: &'static str, text: impl Into<String>) {
    let _ = app.emit("term", TermLine { kind, text: text.into() });
}

/// Контекст фоновой задачи: лог + прогресс.
pub struct Ctx {
    pub app: AppHandle,
    label: RefCell<String>,
    last: Cell<i64>,
}

impl Ctx {
    pub fn log(&self, t: impl Into<String>) {
        emit(&self.app, "out", t)
    }
    pub fn sys(&self, t: impl Into<String>) {
        emit(&self.app, "sys", t)
    }
    pub fn err(&self, t: impl Into<String>) {
        emit(&self.app, "err", t)
    }
    pub fn stage(&self, label: impl Into<String>) {
        let l = label.into();
        self.sys(format!("» {l}"));
        *self.label.borrow_mut() = l;
        self.last.set(-1);
        self.send(-1.0);
    }
    pub fn progress(&self, done: u64, total: u64) {
        if total == 0 {
            return;
        }
        let p = (done as f64 / total as f64 * 1000.0) as i64;
        if p != self.last.get() {
            self.last.set(p);
            self.send(p as f64 / 10.0);
        }
    }
    fn send(&self, pct: f64) {
        let _ = self.app.emit("task", TaskEv { active: true, label: self.label.borrow().clone(), pct });
    }
}

/// Запускает задачу в отдельном потоке. Одновременно — только одна тяжёлая задача.
pub fn spawn_task<F>(app: AppHandle, label: impl Into<String>, f: F) -> Result<(), String>
where
    F: FnOnce(&Ctx) -> Result<String, String> + Send + 'static,
{
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("Уже выполняется другая задача".into());
    }
    let label = label.into();
    std::thread::spawn(move || {
        let ctx = Ctx { app: app.clone(), label: RefCell::new(label.clone()), last: Cell::new(-1) };
        ctx.stage(&label);
        let t = Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&ctx)))
            .unwrap_or_else(|_| Err("внутренняя ошибка (panic)".into()));
        match r {
            Ok(m) => emit(&app, "sys", format!("✔ {m} ({:.1} с)", t.elapsed().as_secs_f32())),
            Err(e) => emit(&app, "err", format!("✖ {e}")),
        }
        BUSY.store(false, Ordering::SeqCst);
        let _ = app.emit("task", TaskEv { active: false, label: String::new(), pct: 100.0 });
        let _ = app.emit("changed", ());
    });
    Ok(())
}

#[tauri::command]
pub fn task_busy() -> bool {
    BUSY.load(Ordering::SeqCst)
}

// ---------------- пути ----------------

pub fn root() -> PathBuf {
    std::env::current_exe()
        .expect("Не удалось определить путь к MEYAFORGE")
        .with_file_name("data")
}

pub fn dir(sub: &str) -> PathBuf {
    let d = root().join(sub);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Префикс `\\?\` для путей длиннее MAX_PATH.
pub fn long(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if p.is_absolute() && !s.starts_with(r"\\?\") {
            return PathBuf::from(format!(r"\\?\{}", s.replace('/', "\\")));
        }
    }
    p.to_path_buf()
}

pub fn es<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub fn copy_progress(ctx: &Ctx, r: &mut impl Read, w: &mut impl Write, total: u64) -> io::Result<u64> {
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        done += n as u64;
        ctx.progress(done, total);
    }
    Ok(done)
}

// ---------------- процессы ----------------

pub fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Читает поток, режет по \r и \n; строки-прогрессбары (\r) прореживаются.
fn pump(app: AppHandle, mut r: impl Read, kind: &'static str) {
    let mut buf = [0u8; 8192];
    let mut line: Vec<u8> = Vec::new();
    let mut last_cr = Instant::now() - std::time::Duration::from_secs(1);
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                if !line.is_empty() {
                    let s = String::from_utf8_lossy(&line).trim_end().to_string();
                    if b == b'\n' || last_cr.elapsed().as_millis() > 400 {
                        if b == b'\r' {
                            last_cr = Instant::now();
                        }
                        if !s.is_empty() {
                            emit(&app, kind, s);
                        }
                    }
                    line.clear();
                }
            } else {
                line.push(b);
            }
        }
    }
    if !line.is_empty() {
        emit(&app, kind, String::from_utf8_lossy(&line).to_string());
    }
}

pub fn run_stream(ctx: &Ctx, exe: &Path, args: &[String], cwd: Option<&Path>) -> Result<(), String> {
    ctx.log(format!("$ {} {}", exe.file_name().unwrap_or_default().to_string_lossy(), args.join(" ")));
    let mut c = Command::new(exe);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(d) = cwd {
        c.current_dir(d);
    } else if let Some(d) = exe.parent() {
        c.current_dir(d);
    }
    let mut child = hidden(&mut c).spawn().map_err(|e| format!("не удалось запустить {}: {e}", exe.display()))?;
    let err = child.stderr.take().unwrap();
    let out = child.stdout.take().unwrap();
    let a = ctx.app.clone();
    let t = std::thread::spawn(move || pump(a, err, "err"));
    pump(ctx.app.clone(), out, "out");
    let _ = t.join();
    let st = child.wait().map_err(es)?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("утилита завершилась с кодом {}", st.code().unwrap_or(-1)))
    }
}

pub fn file_md5(path: &Path) -> Result<String, String> {
    let mut cmd = Command::new("certutil");
    cmd.args(["-hashfile", &path.to_string_lossy(), "MD5"]);
    hidden(&mut cmd);
    let out = cmd.output().map_err(es)?;
    if !out.status.success() {
        return Err("certutil failed".into());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    for line in s.lines() {
        let t = line.trim().replace(' ', "");
        if t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(t.to_lowercase());
        }
    }
    Ok(String::new())
}
