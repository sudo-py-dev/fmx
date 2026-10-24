//! Copying a whole optical disc (or any block device or image) to an ISO file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cso::CsoError;

/// Bytes read per step; a multiple of the 2048-byte disc sector.
const CHUNK: usize = 1 << 20;

/// Copies `device` to `output` byte for byte, atomically, with progress.
pub fn copy_image(
    device: &Path,
    output: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<(), CsoError> {
    let mut source = File::open(device)?;
    // Block devices report length 0 in metadata; seeking to the end gives their size.
    let total = source.seek(SeekFrom::End(0))?;
    source.seek(SeekFrom::Start(0))?;
    let dir = output.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let mut buf = vec![0u8; CHUNK];
    let mut copied = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CsoError::Cancelled);
        }
        let n = source.read(&mut buf)?;
        if n == 0 {
            break;
        }
        tmp.write_all(&buf[..n])?;
        copied += n as u64;
        if total > 0 {
            on_progress((copied as f64 / total as f64).min(1.0) as f32);
        }
    }
    tmp.flush()?;
    tmp.persist(output).map_err(|e| CsoError::Io(e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_exactly_and_honours_cancel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("disc.bin");
        let data: Vec<u8> = (0..(CHUNK * 2 + 777)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).expect("write");
        let out = dir.path().join("disc.iso");
        let mut last = 0.0;
        copy_image(&src, &out, &AtomicBool::new(false), |p| last = p).expect("copy");
        assert_eq!(std::fs::read(&out).expect("read"), data);
        assert_eq!(last, 1.0);

        let cancelled = dir.path().join("c.iso");
        let res = copy_image(&src, &cancelled, &AtomicBool::new(true), |_| {});
        assert!(matches!(res, Err(CsoError::Cancelled)));
        assert!(!cancelled.exists());
    }
}
