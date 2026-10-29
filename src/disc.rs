//! Copying a whole optical disc (or any block device or image) to an ISO
//! file, and finding the computer's disc drives.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cso::CsoError;

/// A disc drive: the path ffmpeg opens it by, and a name to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    pub path: String,
    pub name: String,
}

/// The computer's optical drives. Takes a moment on Windows and macOS, so
/// call it off the UI thread.
pub fn optical_drives() -> Vec<Drive> {
    if cfg!(windows) {
        let script =
            "Get-CimInstance Win32_CDROMDrive | ForEach-Object { \"$($_.Drive)|$($_.Caption)\" }";
        let out = crate::process::command(Path::new("powershell"))
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output();
        out.map(|o| parse_windows_drives(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default()
    } else if cfg!(target_os = "macos") {
        let out = crate::process::command(Path::new("diskutil"))
            .arg("list")
            .output();
        out.map(|o| parse_diskutil(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default()
    } else {
        linux_drives(Path::new("/sys/block"))
    }
}

/// `/sys/block/sr0` and its siblings, named by their vendor and model.
fn linux_drives(sys_block: &Path) -> Vec<Drive> {
    let Ok(entries) = std::fs::read_dir(sys_block) else {
        return Vec::new();
    };
    let mut drives: Vec<Drive> = entries
        .flatten()
        .filter_map(|entry| {
            let dev = entry.file_name().to_string_lossy().into_owned();
            dev.starts_with("sr").then_some(())?;
            let read = |file: &str| {
                std::fs::read_to_string(entry.path().join("device").join(file))
                    .map(|t| t.trim().to_owned())
                    .unwrap_or_default()
            };
            let name = format!("{} {}", read("vendor"), read("model"))
                .trim()
                .to_owned();
            Some(Drive {
                path: format!("/dev/{dev}"),
                name,
            })
        })
        .collect();
    drives.sort_by(|a, b| a.path.cmp(&b.path));
    drives
}

/// Lines of `D:|HL-DT-ST DVDRAM GH24NSD1` from PowerShell.
fn parse_windows_drives(text: &str) -> Vec<Drive> {
    text.lines()
        .filter_map(|line| {
            let (letter, name) = line.trim().split_once('|')?;
            (letter.len() == 2 && letter.ends_with(':')).then(|| Drive {
                path: letter.to_owned(),
                name: name.trim().to_owned(),
            })
        })
        .collect()
}

/// Disks in `diskutil list` that hold a CD's partition scheme.
fn parse_diskutil(text: &str) -> Vec<Drive> {
    let mut drives = Vec::new();
    let mut disk: Option<&str> = None;
    for line in text.lines() {
        if line.starts_with("/dev/disk") {
            disk = line.split_whitespace().next();
        } else if line.contains("CD_partition_scheme")
            && let Some(path) = disk.take()
        {
            drives.push(Drive {
                path: path.to_owned(),
                name: String::new(),
            });
        }
    }
    drives
}

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
    fn linux_drives_are_named_by_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        for (dev, vendor, model) in [
            ("sr1", "ASUS", "BW-16D1HT"),
            ("sr0", "HL-DT-ST", "DVDRAM GH24"),
        ] {
            let device = dir.path().join(dev).join("device");
            std::fs::create_dir_all(&device).expect("dir");
            std::fs::write(device.join("vendor"), format!("{vendor}  \n")).expect("vendor");
            std::fs::write(device.join("model"), format!("{model}\n")).expect("model");
        }
        std::fs::create_dir_all(dir.path().join("sda")).expect("disk");
        let drives = linux_drives(dir.path());
        assert_eq!(
            drives,
            [
                Drive {
                    path: "/dev/sr0".into(),
                    name: "HL-DT-ST DVDRAM GH24".into()
                },
                Drive {
                    path: "/dev/sr1".into(),
                    name: "ASUS BW-16D1HT".into()
                },
            ]
        );
    }

    #[test]
    fn windows_and_mac_listings_parse() {
        let win =
            parse_windows_drives("D:|HL-DT-ST DVDRAM GH24NSD1\r\nbad line\r\nE:|Virtual CD\r\n");
        assert_eq!(win.len(), 2);
        assert_eq!(
            win[0],
            Drive {
                path: "D:".into(),
                name: "HL-DT-ST DVDRAM GH24NSD1".into()
            }
        );
        let mac = parse_diskutil(
            "/dev/disk0 (internal, physical):\n   0: GUID_partition_scheme *500 GB disk0\n\n/dev/disk4 (external, physical):\n   0: CD_partition_scheme *700 MB disk4\n",
        );
        assert_eq!(
            mac,
            [Drive {
                path: "/dev/disk4".into(),
                name: String::new()
            }]
        );
    }

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
