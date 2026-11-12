//! Разбор super.img (Android dynamic partitions / lpmetadata), аналог lpunpack.

use crate::util::{es, Ctx};
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const GEOMETRY_MAGIC: u32 = 0x616C_4467;
const HEADER_MAGIC: u32 = 0x414C_5030;
const META_OFFSET: u64 = 4096 + 4096 * 2;
const SECTOR: u64 = 512;

#[derive(Serialize, Clone)]
pub struct LpPart {
    pub name: String,
    pub size: u64,
    #[serde(skip)]
    extents: Vec<(Option<u64>, u64)>, // (смещение или None для ZERO, длина)
}

fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

pub fn read(path: &Path) -> Result<Vec<LpPart>, String> {
    let mut f = File::open(path).map_err(es)?;
    let mut g = [0u8; 4];
    f.seek(SeekFrom::Start(4096)).map_err(es)?;
    f.read_exact(&mut g).map_err(es)?;
    if u32::from_le_bytes(g) != GEOMETRY_MAGIC {
        return Err("нет LP geometry — это не super".into());
    }
    let mut h = [0u8; 128];
    f.seek(SeekFrom::Start(META_OFFSET)).map_err(es)?;
    f.read_exact(&mut h).map_err(es)?;
    if u32at(&h, 0) != HEADER_MAGIC {
        return Err("повреждён заголовок lpmetadata".into());
    }
    let header_size = u32at(&h, 8) as u64;
    let tables_size = u32at(&h, 44) as usize;
    let desc = |o: usize| (u32at(&h, o) as usize, u32at(&h, o + 4) as usize, u32at(&h, o + 8) as usize);
    let (p_off, p_num, p_sz) = desc(80);
    let (e_off, e_num, e_sz) = desc(92);
    let (_, _, _) = desc(116);
    if u32at(&h, 116 + 4) > 1 {
        return Err("super из нескольких block device не поддерживается".into());
    }

    let mut t = vec![0u8; tables_size];
    f.seek(SeekFrom::Start(META_OFFSET + header_size)).map_err(es)?;
    f.read_exact(&mut t).map_err(es)?;

    let mut exts = Vec::with_capacity(e_num);
    for i in 0..e_num {
        let b = e_off + i * e_sz;
        let len = u64at(&t, b) * SECTOR;
        let ty = u32at(&t, b + 8);
        let data = u64at(&t, b + 12);
        exts.push(if ty == 0 { (Some(data * SECTOR), len) } else { (None, len) });
    }
    let mut parts = Vec::with_capacity(p_num);
    for i in 0..p_num {
        let b = p_off + i * p_sz;
        let name: String = String::from_utf8_lossy(&t[b..b + 36]).trim_end_matches('\0').to_string();
        let first = u32at(&t, b + 40) as usize;
        let num = u32at(&t, b + 44) as usize;
        let extents: Vec<_> = exts.get(first..first + num).unwrap_or(&[]).to_vec();
        let size = extents.iter().map(|e| e.1).sum();
        parts.push(LpPart { name, size, extents });
    }
    Ok(parts)
}

pub fn unpack(ctx: &Ctx, path: &Path, out_dir: &Path) -> Result<Vec<String>, String> {
    let parts = read(path)?;
    let total: u64 = parts.iter().map(|p| p.size).sum();
    let mut f = File::open(path).map_err(es)?;
    let mut done = 0u64;
    let mut names = vec![];
    let mut buf = vec![0u8; 1 << 20];
    for p in parts.iter() {
        if p.size == 0 {
            ctx.log(format!("  {} — пустой, пропуск", p.name));
            continue;
        }
        ctx.log(format!("  {} — {:.1} МБ", p.name, p.size as f64 / 1048576.0));
        let out = File::create(out_dir.join(format!("{}.img", p.name))).map_err(es)?;
        out.set_len(p.size).map_err(es)?;
        let mut w = BufWriter::with_capacity(1 << 20, out);
        for &(off, len) in &p.extents {
            match off {
                None => {
                    w.seek(SeekFrom::Current(len as i64)).map_err(es)?;
                    done += len;
                }
                Some(o) => {
                    f.seek(SeekFrom::Start(o)).map_err(es)?;
                    let mut left = len;
                    while left > 0 {
                        let k = left.min(buf.len() as u64) as usize;
                        f.read_exact(&mut buf[..k]).map_err(es)?;
                        w.write_all(&buf[..k]).map_err(es)?;
                        left -= k as u64;
                        done += k as u64;
                        ctx.progress(done, total);
                    }
                }
            }
        }
        w.flush().map_err(es)?;
        names.push(p.name.clone());
    }
    Ok(names)
}
