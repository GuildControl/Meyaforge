//! Android sparse image → raw (аналог simg2img), нативно.

use crate::util::Ctx;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const MAGIC: u32 = 0xED26_FF3A;
const RAW: u16 = 0xCAC1;
const FILL: u16 = 0xCAC2;
const DONT_CARE: u16 = 0xCAC3;
const CRC: u16 = 0xCAC4;

pub fn is_sparse(path: &Path) -> bool {
    let mut b = [0u8; 4];
    if let Ok(mut f) = File::open(path) {
        if f.read_exact(&mut b).is_ok() {
            return u32::from_le_bytes(b) == MAGIC;
        }
    }
    false
}

struct Hdr {
    blk: u64,
    total_blks: u64,
    chunks: u32,
    chs: u16,
}

fn bad(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.to_string())
}

fn read_hdr<R: Read + Seek>(r: &mut BufReader<R>) -> io::Result<Hdr> {
    let mut b = [0u8; 28];
    r.read_exact(&mut b)?;
    if u32::from_le_bytes(b[0..4].try_into().unwrap()) != MAGIC {
        return Err(bad("не sparse-образ"));
    }
    let fhs = u16::from_le_bytes([b[8], b[9]]);
    let chs = u16::from_le_bytes([b[10], b[11]]);
    let h = Hdr {
        blk: u32::from_le_bytes(b[12..16].try_into().unwrap()) as u64,
        total_blks: u32::from_le_bytes(b[16..20].try_into().unwrap()) as u64,
        chunks: u32::from_le_bytes(b[20..24].try_into().unwrap()),
        chs,
    };
    if fhs > 28 {
        r.seek_relative(fhs as i64 - 28)?;
    }
    Ok(h)
}

fn read_chunk<R: Read + Seek>(r: &mut BufReader<R>, h: &Hdr) -> io::Result<(u16, u64)> {
    let mut c = [0u8; 12];
    r.read_exact(&mut c)?;
    if h.chs > 12 {
        r.seek_relative(h.chs as i64 - 12)?;
    }
    let ty = u16::from_le_bytes([c[0], c[1]]);
    let n = u32::from_le_bytes(c[4..8].try_into().unwrap()) as u64 * h.blk;
    Ok((ty, n))
}

/// Восстанавливает первые `want` байт развёрнутого образа (для детекта).
pub fn peek(path: &Path, want: usize) -> io::Result<Vec<u8>> {
    let mut f = BufReader::new(File::open(path)?);
    let h = read_hdr(&mut f)?;
    let mut out = Vec::with_capacity(want);
    for _ in 0..h.chunks {
        if out.len() >= want {
            break;
        }
        let (ty, n) = read_chunk(&mut f, &h)?;
        let take = (want - out.len()).min(n as usize);
        match ty {
            RAW => {
                let s = out.len();
                out.resize(s + take, 0);
                f.read_exact(&mut out[s..])?;
                f.seek_relative((n - take as u64) as i64)?;
            }
            FILL => {
                let mut p = [0u8; 4];
                f.read_exact(&mut p)?;
                out.extend((0..take).map(|i| p[i % 4]));
            }
            DONT_CARE => out.resize(out.len() + take, 0),
            CRC => f.seek_relative(4)?,
            _ => return Err(bad("неизвестный тип чанка")),
        }
    }
    Ok(out)
}

pub fn unsparse(ctx: &Ctx, src: &Path, dst: &Path) -> io::Result<()> {
    let mut f = BufReader::with_capacity(1 << 20, File::open(src)?);
    let h = read_hdr(&mut f)?;
    let total = h.total_blks * h.blk;
    let out = File::create(dst)?;
    out.set_len(total)?;
    let mut w = BufWriter::with_capacity(1 << 20, out);
    let mut pos = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    for _ in 0..h.chunks {
        let (ty, n) = read_chunk(&mut f, &h)?;
        match ty {
            RAW => {
                let mut left = n;
                while left > 0 {
                    let k = left.min(buf.len() as u64) as usize;
                    f.read_exact(&mut buf[..k])?;
                    w.write_all(&buf[..k])?;
                    left -= k as u64;
                    ctx.progress(pos + n - left, total);
                }
            }
            FILL => {
                let mut p = [0u8; 4];
                f.read_exact(&mut p)?;
                if p == [0; 4] {
                    w.seek(SeekFrom::Current(n as i64))?;
                } else {
                    let pat: Vec<u8> = (0..h.blk as usize).map(|i| p[i % 4]).collect();
                    for _ in 0..n / h.blk {
                        w.write_all(&pat)?;
                    }
                }
            }
            DONT_CARE => {
                w.seek(SeekFrom::Current(n as i64))?;
            }
            CRC => f.seek_relative(4)?,
            t => return Err(bad(&format!("неизвестный тип чанка {t:#x}"))),
        }
        pos += n;
        ctx.progress(pos, total);
    }
    w.flush()?;
    Ok(())
}
