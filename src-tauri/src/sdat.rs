//! Конвертер Android sparse dat (*.new.dat + *.transfer.list) в стандартный raw .img (sdat2img).

use crate::util::{es, Ctx};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

const BLOCK_SIZE: u64 = 4096;

pub fn sdat2img(ctx: &Ctx, transfer_list: &Path, new_dat: &Path, out_img: &Path) -> Result<(), String> {
    let t_file = File::open(transfer_list).map_err(es)?;
    let mut reader = BufReader::new(t_file);

    let mut line = String::new();
    reader.read_line(&mut line).map_err(es)?;
    let version: u32 = line.trim().parse().map_err(|_| "неверная версия transfer.list")?;

    line.clear();
    reader.read_line(&mut line).map_err(es)?;
    let total_blocks: u64 = line.trim().parse().map_err(|_| "неверное число блоков")?;

    // Для версий 3+: пропускаем строки stash
    if version >= 3 {
        line.clear();
        reader.read_line(&mut line).map_err(es)?; // stash entries
        line.clear();
        reader.read_line(&mut line).map_err(es)?; // max stashed blocks
    }

    let mut dat_file = File::open(new_dat).map_err(es)?;
    let mut out_file = File::create(out_img).map_err(es)?;
    // Заранее задаем размер выходного файла
    out_file.set_len(total_blocks * BLOCK_SIZE).map_err(es)?;

    let total_bytes = total_blocks * BLOCK_SIZE;
    let mut processed_blocks = 0u64;

    let mut buf = vec![0u8; 1024 * 1024]; // 1MB buffer

    for l in reader.lines() {
        let cmd_line = l.map_err(es)?;
        let trimmed = cmd_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let cmd = parts[0];
        if cmd == "new" && parts.len() >= 2 {
            let ranges: Vec<u64> = parts[1]
                .split(',')
                .filter_map(|s| s.trim().parse::<u64>().ok())
                .collect();

            if ranges.len() >= 3 {
                let range_count = ranges[0] as usize;
                let pairs = &ranges[1..];
                if pairs.len() >= range_count {
                    for i in (0..range_count).step_by(2) {
                        let start_block = pairs[i];
                        let end_block = pairs[i + 1];
                        let blocks_to_copy = end_block.saturating_sub(start_block);
                        let mut bytes_to_copy = blocks_to_copy * BLOCK_SIZE;

                        out_file.seek(SeekFrom::Start(start_block * BLOCK_SIZE)).map_err(es)?;

                        while bytes_to_copy > 0 {
                            let chunk_size = (bytes_to_copy as usize).min(buf.len());
                            dat_file.read_exact(&mut buf[..chunk_size]).map_err(es)?;
                            out_file.write_all(&buf[..chunk_size]).map_err(es)?;
                            bytes_to_copy -= chunk_size as u64;
                            processed_blocks += (chunk_size as u64) / BLOCK_SIZE;
                        }

                        let done_bytes = (processed_blocks * BLOCK_SIZE).min(total_bytes);
                        ctx.progress(done_bytes, total_bytes);
                    }
                }
            }
        }
    }

    out_file.flush().map_err(es)?;
    Ok(())
}
