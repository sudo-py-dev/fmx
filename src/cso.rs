//! Native CISO (`.cso`) compression and decompression of ISO images.
//!
//! Layout: a 24-byte header (`CISO`, header size, total bytes, block size,
//! version, index alignment), then `blocks + 1` little-endian u32 offsets
//! (shifted right by the alignment; bit 31 marks a block stored uncompressed),
//! then the blocks as raw deflate streams.

use std::fmt;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use rayon::prelude::*;

use crate::i18n::{tr, trf};

const MAGIC: &[u8; 4] = b"CISO";
const HEADER_SIZE: u32 = 0x18;
const BLOCK_SIZE: u32 = 2048;
const VERSION: u8 = 1;
const PLAIN_FLAG: u32 = 0x8000_0000;
/// Blocks compressed in parallel per batch (8 MiB of input).
const BATCH_BLOCKS: usize = 4096;
/// Largest block size we accept when reading foreign files.
const MAX_BLOCK_SIZE: u32 = 1 << 20;

#[derive(Debug)]
pub enum CsoError {
    Io(std::io::Error),
    NotCso,
    Corrupt,
    Cancelled,
}

impl fmt::Display for CsoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => f.write_str(&trf("error.io", &[("error", e)])),
            Self::NotCso => f.write_str(tr("error.not_cso")),
            Self::Corrupt => f.write_str(tr("error.corrupt_cso")),
            Self::Cancelled => f.write_str(tr("status.cancelled")),
        }
    }
}

impl std::error::Error for CsoError {}

impl From<std::io::Error> for CsoError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

fn block_count(total: u64, block_size: u32) -> u64 {
    total.div_ceil(u64::from(block_size))
}

/// Smallest alignment shift that keeps every offset below bit 31.
fn alignment_for(total: u64, blocks: u64) -> u8 {
    let mut align = 0u8;
    loop {
        let padding = blocks * ((1u64 << align) - 1);
        let worst_end = u64::from(HEADER_SIZE) + (blocks + 1) * 4 + total + padding;
        if worst_end >> align < u64::from(PLAIN_FLAG) {
            return align;
        }
        align += 1;
    }
}

fn deflate(block: &[u8], level: u32) -> Vec<u8> {
    let mut enc = DeflateEncoder::new(Vec::with_capacity(block.len()), Compression::new(level));
    // Writing into a Vec cannot fail.
    let _ = enc.write_all(block);
    enc.finish().unwrap_or_default()
}

/// Reads up to `limit` bytes, stopping early only at end of file.
fn read_up_to(reader: &mut impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(limit);
    reader.take(limit as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Compresses `input` (an ISO image) into `output` at deflate `level` 1–9.
pub fn compress(
    input: &Path,
    output: &Path,
    level: u32,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<(), CsoError> {
    let level = level.clamp(1, 9);
    let mut reader = BufReader::new(File::open(input)?);
    let total = reader.get_ref().metadata()?.len();
    let blocks = block_count(total, BLOCK_SIZE);
    let align = alignment_for(total, blocks);
    let index_len = usize::try_from(blocks + 1).map_err(|_| CsoError::Corrupt)?;

    let dir = output.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let mut index = Vec::with_capacity(index_len);
    {
        let mut out = BufWriter::new(tmp.as_file_mut());
        out.write_all(MAGIC)?;
        out.write_all(&HEADER_SIZE.to_le_bytes())?;
        out.write_all(&total.to_le_bytes())?;
        out.write_all(&BLOCK_SIZE.to_le_bytes())?;
        out.write_all(&[VERSION, align, 0, 0])?;
        out.write_all(&vec![0u8; index_len * 4])?;
        let mut pos = u64::from(HEADER_SIZE) + index_len as u64 * 4;
        let unit = 1u64 << align;

        let mut done = 0u64;
        while done < blocks {
            if cancel.load(Ordering::Relaxed) {
                return Err(CsoError::Cancelled);
            }
            let batch = read_up_to(&mut reader, BATCH_BLOCKS * BLOCK_SIZE as usize)?;
            if batch.is_empty() {
                break;
            }
            let packed: Vec<(Vec<u8>, bool)> = batch
                .par_chunks(BLOCK_SIZE as usize)
                .map(|block| {
                    let z = deflate(block, level);
                    if z.len() < block.len() {
                        (z, false)
                    } else {
                        (block.to_vec(), true)
                    }
                })
                .collect();
            for (data, plain) in &packed {
                let padding = (unit - pos % unit) % unit;
                out.write_all(&vec![0u8; padding as usize])?;
                pos += padding;
                let entry = u32::try_from(pos >> align).map_err(|_| CsoError::Corrupt)?;
                index.push(if *plain { entry | PLAIN_FLAG } else { entry });
                out.write_all(data)?;
                pos += data.len() as u64;
            }
            done += packed.len() as u64;
            on_progress(done as f32 / blocks.max(1) as f32);
        }
        let padding = (unit - pos % unit) % unit;
        out.write_all(&vec![0u8; padding as usize])?;
        pos += padding;
        index.push(u32::try_from(pos >> align).map_err(|_| CsoError::Corrupt)?);

        out.seek(SeekFrom::Start(u64::from(HEADER_SIZE)))?;
        let table: Vec<u8> = index.iter().flat_map(|e| e.to_le_bytes()).collect();
        out.write_all(&table)?;
        out.flush()?;
    }
    if index.len() != index_len {
        return Err(CsoError::Corrupt);
    }
    tmp.persist(output).map_err(|e| CsoError::Io(e.error))?;
    on_progress(1.0);
    Ok(())
}

struct Header {
    total: u64,
    block_size: u32,
    align: u8,
}

fn read_header(reader: &mut impl Read) -> Result<Header, CsoError> {
    let mut raw = [0u8; HEADER_SIZE as usize];
    reader.read_exact(&mut raw).map_err(|_| CsoError::NotCso)?;
    if &raw[0..4] != MAGIC {
        return Err(CsoError::NotCso);
    }
    let le32 = |at: usize| u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
    let total = u64::from_le_bytes(raw[8..16].try_into().map_err(|_| CsoError::NotCso)?);
    let block_size = le32(16);
    let align = raw[21];
    if block_size == 0 || block_size > MAX_BLOCK_SIZE || align > 16 || le32(4) != HEADER_SIZE {
        return Err(CsoError::Corrupt);
    }
    Ok(Header {
        total,
        block_size,
        align,
    })
}

/// Expands a `.cso` file back into a plain ISO image.
pub fn decompress(
    input: &Path,
    output: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<(), CsoError> {
    let mut reader = BufReader::new(File::open(input)?);
    let file_len = reader.get_ref().metadata()?.len();
    let header = read_header(&mut reader)?;
    let blocks = block_count(header.total, header.block_size);
    // Each index entry takes 4 bytes; a table larger than the file is a lie.
    if (blocks + 1) * 4 > file_len {
        return Err(CsoError::Corrupt);
    }
    let raw_index = read_up_to(&mut reader, ((blocks + 1) * 4) as usize)?;
    let index: Vec<u32> = raw_index
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    if index.len() as u64 != blocks + 1 {
        return Err(CsoError::Corrupt);
    }
    let offset = |entry: u32| u64::from(entry & !PLAIN_FLAG) << header.align;
    let block_size = header.block_size as usize;

    let dir = output.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut out = BufWriter::new(tmp.as_file_mut());
        let mut position = reader.stream_position()?;
        for (batch_no, window) in index
            .windows(2)
            .collect::<Vec<_>>()
            .chunks(BATCH_BLOCKS)
            .enumerate()
        {
            if cancel.load(Ordering::Relaxed) {
                return Err(CsoError::Cancelled);
            }
            let first = batch_no * BATCH_BLOCKS;
            let mut raw_blocks = Vec::with_capacity(window.len());
            for pair in window {
                let (start, end) = (offset(pair[0]), offset(pair[1]));
                if end < start
                    || end > file_len
                    || end - start > (block_size as u64) * 2 + (1 << header.align)
                {
                    return Err(CsoError::Corrupt);
                }
                if start != position {
                    reader.seek(SeekFrom::Start(start))?;
                }
                let data = read_up_to(&mut reader, (end - start) as usize)?;
                position = end;
                raw_blocks.push((data, pair[0] & PLAIN_FLAG != 0));
            }
            let expanded: Vec<Result<Vec<u8>, CsoError>> = raw_blocks
                .par_iter()
                .enumerate()
                .map(|(i, (data, plain))| {
                    let block_no = (first + i) as u64;
                    let want = (header.total - block_no * u64::from(header.block_size))
                        .min(header.block_size as u64) as usize;
                    let plain_block = if *plain {
                        data.get(..want)
                            .map(<[u8]>::to_vec)
                            .ok_or(CsoError::Corrupt)?
                    } else {
                        read_up_to(&mut DeflateDecoder::new(data.as_slice()), want)
                            .map_err(|_| CsoError::Corrupt)?
                    };
                    if plain_block.len() == want {
                        Ok(plain_block)
                    } else {
                        Err(CsoError::Corrupt)
                    }
                })
                .collect();
            for block in expanded {
                out.write_all(&block?)?;
            }
            on_progress(((first + window.len()) as f32 / blocks.max(1) as f32).min(1.0));
        }
        out.flush()?;
    }
    tmp.persist(output).map_err(|e| CsoError::Io(e.error))?;
    on_progress(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic mix of compressible and incompressible bytes.
    fn sample(len: usize) -> Vec<u8> {
        let mut state = 0x2545_F491_u32;
        (0..len)
            .map(|i| {
                if (i / 3000) % 2 == 0 {
                    (i % 7) as u8
                } else {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                }
            })
            .collect()
    }

    fn round_trip(len: usize, level: u32) {
        let dir = tempfile::tempdir().expect("tempdir");
        let iso = dir.path().join("in.iso");
        let cso = dir.path().join("out.cso");
        let back = dir.path().join("back.iso");
        let data = sample(len);
        std::fs::write(&iso, &data).expect("write");
        let never = AtomicBool::new(false);
        compress(&iso, &cso, level, &never, |_| {}).expect("compress");
        let mut last = 0.0;
        decompress(&cso, &back, &never, |p| last = p).expect("decompress");
        assert_eq!(std::fs::read(&back).expect("read"), data, "len {len}");
        assert_eq!(last, 1.0);
    }

    #[test]
    fn round_trips_partial_and_exact_blocks() {
        round_trip(BLOCK_SIZE as usize * 5 + 123, 9);
        round_trip(BLOCK_SIZE as usize * 4, 1);
        round_trip(0, 6);
    }

    #[test]
    fn compressible_data_shrinks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let iso = dir.path().join("z.iso");
        std::fs::write(&iso, vec![0u8; 1 << 20]).expect("write");
        let cso = dir.path().join("z.cso");
        compress(&iso, &cso, 9, &AtomicBool::new(false), |_| {}).expect("compress");
        assert!(std::fs::metadata(&cso).expect("meta").len() < 64 * 1024);
    }

    #[test]
    fn rejects_non_cso_and_honours_cancel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bogus = dir.path().join("x.cso");
        std::fs::write(&bogus, b"NOPE and more bytes here to fill a header").expect("write");
        let out = dir.path().join("o.iso");
        assert!(matches!(
            decompress(&bogus, &out, &AtomicBool::new(false), |_| {}),
            Err(CsoError::NotCso)
        ));

        let iso = dir.path().join("a.iso");
        std::fs::write(&iso, sample(10_000)).expect("write");
        let res = compress(
            &iso,
            &dir.path().join("c.cso"),
            9,
            &AtomicBool::new(true),
            |_| {},
        );
        assert!(matches!(res, Err(CsoError::Cancelled)));
        assert!(!dir.path().join("c.cso").exists());
    }

    #[test]
    fn alignment_grows_for_huge_images() {
        assert_eq!(alignment_for(1 << 20, 512), 0);
        assert!(alignment_for(5 << 30, (5 << 30) / 2048) >= 2);
    }
}
