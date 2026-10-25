//! Fmx's own copy of yt-dlp, so the video downloader works without a system
//! install.
//!
//! The official standalone build is downloaded from yt-dlp's latest GitHub
//! release and checked against the SHA-256 list published with that release.
//! Video sites change often and old yt-dlp versions stop working, so instead
//! of pinning a version, "Update" installs the newest release again.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::ffmpeg_setup::{SetupError, agent, download_verified, persist_executable};

const RELEASE_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
/// The checksum list published with every release.
const SUMS_ASSET: &str = "SHA2-256SUMS";
const MAX_SUMS_BYTES: u64 = 64 << 10;
/// Upper bound for the program itself (the Linux build is about 40 MB).
const MAX_BINARY_BYTES: u64 = 256 << 20;

/// The release file that runs on this computer without Python, if there is one.
pub fn asset_for_this_platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("yt-dlp_linux"),
        ("linux", "aarch64") => Some("yt-dlp_linux_aarch64"),
        // A universal build for Intel and Apple silicon.
        ("macos", _) => Some("yt-dlp_macos"),
        ("windows", "x86_64") => Some("yt-dlp.exe"),
        ("windows", "aarch64") => Some("yt-dlp_arm64.exe"),
        _ => None,
    }
}

/// Where FMX keeps its own yt-dlp (inside the per-user data folder).
pub fn managed_binary() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    };
    dirs::data_dir().map(|d| {
        let fmx_path = d.join("fmx").join("yt-dlp").join(name);
        if fmx_path.is_file() {
            fmx_path
        } else {
            let legacy_path = d.join("muxr").join("yt-dlp").join(name);
            if legacy_path.is_file() {
                legacy_path
            } else {
                fmx_path
            }
        }
    })
}

/// The digest listed for `asset` in a `SHA2-256SUMS` file
/// (`<64 hex digits>  <file name>` per line).
fn digest_for(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        let valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        (valid && name == asset).then(|| hash.to_ascii_lowercase())
    })
}

/// Installs the program from any reader (the network in practice, a buffer in
/// tests). `size` is the announced length, if any.
fn install_from(
    source: impl Read,
    sha256: &str,
    size: Option<u64>,
    target: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32),
) -> Result<PathBuf, SetupError> {
    let dir = target.parent().ok_or(SetupError::Unsupported)?;
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let limit = size.unwrap_or(MAX_BINARY_BYTES).min(MAX_BINARY_BYTES);
    download_verified(
        source,
        tmp.as_file_mut(),
        sha256,
        limit,
        cancel,
        on_progress,
    )?;
    persist_executable(tmp, target)?;
    Ok(target.to_path_buf())
}

/// Downloads, verifies and installs (or updates) yt-dlp for this platform.
pub fn install(
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<PathBuf, SetupError> {
    let asset = asset_for_this_platform().ok_or(SetupError::Unsupported)?;
    let target = managed_binary().ok_or(SetupError::Unsupported)?;
    let agent = agent();
    let network = |e: ureq::Error| SetupError::Network(e.to_string());
    let sums = agent
        .get(&format!("{RELEASE_URL}/{SUMS_ASSET}"))
        .call()
        .map_err(network)?
        .body_mut()
        .with_config()
        .limit(MAX_SUMS_BYTES)
        .read_to_string()
        .map_err(network)?;
    let sha256 = digest_for(&sums, asset).ok_or(SetupError::Checksum)?;
    let mut response = agent
        .get(&format!("{RELEASE_URL}/{asset}"))
        .call()
        .map_err(network)?;
    let size = response.body().content_length();
    let reader = response.body_mut().as_reader();
    install_from(reader, &sha256, size, &target, cancel, &mut on_progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    use crate::ffmpeg_setup::hex;

    #[test]
    fn digest_is_found_by_exact_name() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  yt-dlp_linux.zip\n{b}  yt-dlp_linux\nnot a line\n");
        assert_eq!(digest_for(&sums, "yt-dlp_linux"), Some("b".repeat(64)));
        assert_eq!(digest_for(&sums, "yt-dlp_linux.zip"), Some(a));
        assert_eq!(digest_for(&sums, "yt-dlp"), None);
        // Malformed digests are never accepted.
        assert_eq!(digest_for("xyz  yt-dlp", "yt-dlp"), None);
    }

    #[test]
    fn installs_verified_program_and_rejects_tampering() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("bin").join("yt-dlp");
        let payload = b"\x7fELF fake yt-dlp".repeat(500);
        let sha = hex(&Sha256::digest(&payload));
        let size = Some(payload.len() as u64);
        let cancel = AtomicBool::new(false);
        let mut tampered = payload.clone();
        tampered[10] ^= 0xff;
        let result = install_from(
            tampered.as_slice(),
            &sha,
            size,
            &target,
            &cancel,
            &mut |_| {},
        );
        assert!(matches!(result, Err(SetupError::Checksum)));
        assert!(!target.exists());

        install_from(
            payload.as_slice(),
            &sha,
            size,
            &target,
            &cancel,
            &mut |_| {},
        )
        .expect("install");
        assert_eq!(std::fs::read(&target).expect("read"), payload);
    }

    #[test]
    fn this_platform_has_a_build_name() {
        if let Some(asset) = asset_for_this_platform() {
            assert!(asset.starts_with("yt-dlp"));
        }
    }

    /// Network check against the live release. It downloads but never runs the
    /// file: `cargo test ytdlp_setup -- --ignored`.
    #[test]
    #[ignore = "downloads about 40 MB"]
    fn latest_release_installs_and_verifies() {
        let dir = tempfile::tempdir().expect("tempdir");
        let asset = asset_for_this_platform().expect("platform");
        let agent = agent();
        let sums = agent
            .get(&format!("{RELEASE_URL}/{SUMS_ASSET}"))
            .call()
            .expect("sums")
            .body_mut()
            .read_to_string()
            .expect("text");
        let sha = digest_for(&sums, asset).expect("listed");
        let mut response = agent
            .get(&format!("{RELEASE_URL}/{asset}"))
            .call()
            .expect("download");
        let size = response.body().content_length();
        let target = dir.path().join("yt-dlp");
        install_from(
            response.body_mut().as_reader(),
            &sha,
            size,
            &target,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("install");
        let head = std::fs::read(&target).expect("read");
        let is_executable_format = head.starts_with(b"\x7fELF")
            || head.starts_with(b"MZ")
            || head.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
            || head.starts_with(&[0xca, 0xfe, 0xba, 0xbe]);
        assert!(is_executable_format, "unexpected file header");
    }
}
