//! Определение типа образа или архива по сигнатурам и расширению.

use std::fs::File;
use std::io::Read;
use std::path::Path;

fn u32le(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn u16le(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).map_or(0, |s| u16::from_le_bytes(s.try_into().unwrap()))
}

pub fn detect_buf(b: &[u8]) -> &'static str {
    if b.len() < 4 {
        return "raw";
    }
    if u32le(b, 0) == crate::sparse::MAGIC {
        return "sparse";
    }
    if b.starts_with(b"PK\x03\x04") {
        return "zip";
    }
    if b.starts_with(b"Rar!\x1A\x07") {
        return "rar";
    }
    if b.starts_with(b"7z\xBC\xAF\x27\x1C") {
        return "7z";
    }
    if b.starts_with(b"\x1F\x8B") {
        return "gzip";
    }
    if b.starts_with(b"\xFD7zXZ\x00") {
        return "xz";
    }
    if b.starts_with(b"BZh") {
        return "bzip2";
    }
    if b.starts_with(b"\x04\"M\x18") || u32le(b, 0) == 0x184D_2204 {
        return "lz4";
    }
    if b.starts_with(b"(\xB5/\xFD") || u32le(b, 0) == 0xFD2F_B528 {
        return "zstd";
    }
    if b.len() >= 262 && &b[257..262] == b"ustar" {
        return "tar";
    }
    if b.starts_with(b"CrAU") {
        return "payload";
    }
    if b.starts_with(b"AVB0") {
        return "vbmeta";
    }
    if b.starts_with(b"ANDROID!") {
        return "boot";
    }
    if b.starts_with(b"VNDRBOOT") {
        return "vendor_boot";
    }
    if u32le(b, 1024) == 0xE0F5_E1E2 {
        return "erofs";
    }
    if u16le(b, 1024 + 0x38) == 0xEF53 {
        return "ext4";
    }
    if u32le(b, 1024) == 0xF2F5_2010 {
        return "f2fs";
    }
    if u32le(b, 4096) == crate::lp::GEOMETRY_MAGIC {
        return "super";
    }
    "raw"
}

pub struct Det {
    pub kind: &'static str,
    /// Для sparse — тип содержимого.
    pub inner: Option<&'static str>,
}

pub fn detect(path: &Path) -> std::io::Result<Det> {
    let mut buf = Vec::with_capacity(8192);
    let _ = File::open(path)?.take(8192).read_to_end(&mut buf);
    let mut kind = detect_buf(&buf);

    // Если по байтам не определилось (или определилось как raw), проверяем имя файла
    if kind == "raw" {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        if name.ends_with(".tar.md5") || name.ends_with(".tar") {
            kind = "tar";
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            kind = "gzip";
        } else if name.ends_with(".rar") {
            kind = "rar";
        } else if name.ends_with(".7z") {
            kind = "7z";
        } else if name.ends_with(".zip") {
            kind = "zip";
        } else if name.ends_with(".lz4") {
            kind = "lz4";
        } else if name.ends_with(".zst") || name.ends_with(".zstd") {
            kind = "zstd";
        } else if name == "payload.bin" {
            kind = "payload";
        } else if name == "super.img" {
            kind = "super";
        } else if name.starts_with("boot") && name.ends_with(".img") {
            kind = "boot";
        } else if name.starts_with("vendor_boot") && name.ends_with(".img") {
            kind = "vendor_boot";
        } else if name.starts_with("recovery") && name.ends_with(".img") {
            kind = "boot";
        } else if name.starts_with("init_boot") && name.ends_with(".img") {
            kind = "boot";
        }
    }

    let inner = if kind == "sparse" {
        crate::sparse::peek(path, 8192).ok().map(|b| detect_buf(&b))
    } else {
        None
    };
    Ok(Det { kind, inner })
}
