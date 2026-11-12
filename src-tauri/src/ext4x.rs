//! Нативная разборка ext4 (через ext4-view) с сохранением fs_config и симлинков.

use crate::util::{es, long, Ctx};
use ext4_view::{Ext4, PathBuf as EPath};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Имя, допустимое в Windows.
pub fn sanitize(n: &str) -> String {
    let mut s: String = n
        .chars()
        .map(|c| if "<>:\"\\|?*".contains(c) || (c as u32) < 32 { '_' } else { c })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
        s.push('_');
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        s.insert(0, '_');
    }
    s
}

pub fn extract(ctx: &Ctx, img: &Path, out: &Path, cfg: &Path, part: &str) -> Result<String, String> {
    let fs4 = Ext4::load_from_path(img).map_err(|e| format!("ext4: {e}"))?;
    fs::create_dir_all(long(out)).map_err(es)?;
    fs::create_dir_all(cfg).map_err(es)?;
    let mut conf = BufWriter::new(File::create(cfg.join(format!("{part}_fs_config"))).map_err(es)?);
    let mut links = BufWriter::new(File::create(cfg.join(format!("{part}_symlinks"))).map_err(es)?);
    let mut ren = BufWriter::new(File::create(cfg.join(format!("{part}_renamed"))).map_err(es)?);

    let root = EPath::try_from("/").unwrap();
    if let Ok(m) = fs4.metadata(&root) {
        let _ = writeln!(conf, "{part} {} {} {:04o}", m.uid(), m.gid(), m.mode() & 0o7777);
    }
    let (mut files, mut dirs, mut syml, mut errs, mut bytes) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut stack: Vec<(EPath, PathBuf, String)> = vec![(root, out.to_path_buf(), String::new())];

    while let Some((edir, hdir, rel)) = stack.pop() {
        let rd = match fs4.read_dir(&edir) {
            Ok(r) => r,
            Err(e) => {
                ctx.err(format!("  {rel}: {e}"));
                errs += 1;
                continue;
            }
        };
        for ent in rd {
            let ent = match ent {
                Ok(e) => e,
                Err(e) => {
                    ctx.err(format!("  {rel}: {e}"));
                    errs += 1;
                    continue;
                }
            };
            let name = match ent.file_name().as_str() {
                Ok(s) => s.to_string(),
                Err(_) => ent.file_name().display().to_string(),
            };
            if name == "." || name == ".." {
                continue;
            }
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let ep = ent.path();
            let md = match ent.metadata() {
                Ok(m) => m,
                Err(e) => {
                    ctx.err(format!("  {r}: {e}"));
                    errs += 1;
                    continue;
                }
            };
            let safe = sanitize(&name);
            if safe != name {
                let _ = writeln!(ren, "{part}/{r}\t{safe}");
            }
            let hp = hdir.join(&safe);
            let ft = md.file_type();
            let _ = writeln!(conf, "{part}/{r} {} {} {:04o}", md.uid(), md.gid(), md.mode() & 0o7777);

            if ft.is_dir() {
                if let Err(e) = fs::create_dir_all(long(&hp)) {
                    ctx.err(format!("  {r}: {e}"));
                    errs += 1;
                    continue;
                }
                dirs += 1;
                stack.push((ep, hp, r));
            } else if ft.is_symlink() {
                match fs4.read_link(&ep) {
                    Ok(t) => {
                        let _ = writeln!(links, "{part}/{r} {}", t.display());
                        syml += 1;
                    }
                    Err(e) => {
                        ctx.err(format!("  {r}: {e}"));
                        errs += 1;
                    }
                }
            } else if ft.is_regular_file() {
                let res = (|| -> Result<u64, String> {
                    let mut src = fs4.open(&ep).map_err(es)?;
                    let mut dst = BufWriter::with_capacity(256 << 10, File::create(long(&hp)).map_err(es)?);
                    let n = std::io::copy(&mut src, &mut dst).map_err(es)?;
                    dst.flush().map_err(es)?;
                    Ok(n)
                })();
                match res {
                    Ok(n) => {
                        files += 1;
                        bytes += n;
                        if files % 1000 == 0 {
                            ctx.log(format!("  … {files} файлов, {:.0} МБ", bytes as f64 / 1048576.0));
                        }
                    }
                    Err(e) => {
                        ctx.err(format!("  {r}: {e}"));
                        errs += 1;
                    }
                }
            }
        }
    }
    conf.flush().map_err(es)?;
    links.flush().map_err(es)?;
    ren.flush().map_err(es)?;
    Ok(format!(
        "{part}: {files} файлов, {dirs} папок, {syml} симлинков, {:.1} МБ{}",
        bytes as f64 / 1048576.0,
        if errs > 0 { format!(", ошибок: {errs}") } else { String::new() }
    ))
}
