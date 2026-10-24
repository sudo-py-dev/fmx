//! Muxr's own copy of ffmpeg, so media tools work without a system install.
//!
//! A pinned static build is downloaded once into the user's data folder,
//! checked against its published SHA-256 digest, and unpacked atomically.

use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

use crate::i18n::{tr, trf};

/// The ffmpeg release Muxr installs (static builds from eugeneware/ffmpeg-static).
pub const VERSION: &str = "6.1.1";
const RELEASE_URL: &str = "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1";
/// Upper bound for the unpacked binary; guards against a decompression bomb.
const MAX_BINARY_BYTES: u64 = 512 << 20;
const CHUNK: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// One downloadable build: asset name, its SHA-256, and its compressed size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Build {
    pub asset: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// The build for this computer, if one is published.
pub fn build_for_this_platform() -> Option<Build> {
    let build = |asset, sha256, size| {
        Some(Build {
            asset,
            sha256,
            size,
        })
    };
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => build(
            "ffmpeg-linux-x64.gz",
            "bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa",
            29_354_986,
        ),
        ("linux", "aarch64") => build(
            "ffmpeg-linux-arm64.gz",
            "754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0",
            25_568_691,
        ),
        ("macos", "x86_64") => build(
            "ffmpeg-darwin-x64.gz",
            "929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106",
            25_296_431,
        ),
        ("macos", "aarch64") => build(
            "ffmpeg-darwin-arm64.gz",
            "8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa",
            19_246_198,
        ),
        ("windows", "x86_64") => build(
            "ffmpeg-win32-x64.gz",
            "8883a3dffbd0a16cf4ef95206ea05283f78908dbfb118f73c83f4951dcc06d77",
            29_581_307,
        ),
        _ => None,
    }
}

/// Where Muxr keeps its own ffmpeg (inside the per-user data folder).
pub fn managed_binary() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    dirs::data_dir().map(|d| d.join("muxr").join(format!("ffmpeg-{VERSION}")).join(name))
}

#[derive(Debug)]
pub enum SetupError {
    Unsupported,
    Network(String),
    Io(std::io::Error),
    Checksum,
    TooLarge,
    Cancelled,
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str(tr("setup.unsupported")),
            Self::Network(e) => f.write_str(&trf("setup.network", &[("error", e)])),
            Self::Io(e) => f.write_str(&trf("error.io", &[("error", e)])),
            Self::Checksum => f.write_str(tr("setup.checksum")),
            Self::TooLarge => f.write_str(tr("setup.too_large")),
            Self::Cancelled => f.write_str(tr("status.cancelled")),
        }
    }
}

impl std::error::Error for SetupError {}

impl From<std::io::Error> for SetupError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Streams `source` into `dest` while hashing it; fails unless the digest matches.
fn download_verified(
    mut source: impl Read,
    dest: &mut impl Write,
    build: &Build,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32),
) -> Result<(), SetupError> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut received = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(SetupError::Cancelled);
        }
        let n = source
            .read(&mut buf)
            .map_err(|e| SetupError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        received += n as u64;
        // The compressed size is known; anything bigger is not our file.
        if received > build.size {
            return Err(SetupError::Checksum);
        }
        hasher.update(&buf[..n]);
        dest.write_all(&buf[..n])?;
        on_progress((received as f64 / build.size as f64).min(1.0) as f32);
    }
    if hex(&hasher.finalize()) == build.sha256 {
        Ok(())
    } else {
        Err(SetupError::Checksum)
    }
}

/// Unpacks a verified `.gz` archive into an executable file at `target`.
fn unpack(gz: &Path, target: &Path) -> Result<(), SetupError> {
    let dir = target.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let mut decoder = GzDecoder::new(BufReader::new(File::open(gz)?)).take(MAX_BINARY_BYTES + 1);
    let written = std::io::copy(&mut decoder, tmp.as_file_mut())?;
    if written > MAX_BINARY_BYTES {
        return Err(SetupError::TooLarge);
    }
    tmp.as_file_mut().flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    tmp.persist(target).map_err(|e| SetupError::Io(e.error))?;
    Ok(())
}

/// Installs a build from any reader (the network in practice, a buffer in tests).
fn install_from(
    source: impl Read,
    build: &Build,
    target: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32),
) -> Result<PathBuf, SetupError> {
    let dir = target.parent().ok_or(SetupError::Unsupported)?;
    std::fs::create_dir_all(dir)?;
    let mut archive = tempfile::Builder::new().suffix(".gz").tempfile_in(dir)?;
    download_verified(source, archive.as_file_mut(), build, cancel, on_progress)?;
    archive.as_file_mut().flush()?;
    unpack(archive.path(), target)?;
    Ok(target.to_path_buf())
}

/// Downloads, verifies and installs ffmpeg for this platform.
pub fn install(
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<PathBuf, SetupError> {
    let build = build_for_this_platform().ok_or(SetupError::Unsupported)?;
    let target = managed_binary().ok_or(SetupError::Unsupported)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_per_read(Some(READ_TIMEOUT))
        .build()
        .into();
    let url = format!("{RELEASE_URL}/{}", build.asset);
    let mut response = agent
        .get(&url)
        .call()
        .map_err(|e| SetupError::Network(e.to_string()))?;
    let reader = response.body_mut().as_reader();
    install_from(reader, &build, &target, cancel, &mut on_progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    fn fake_build(payload: &[u8]) -> (Vec<u8>, Build) {
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(payload).expect("gzip");
        let gz = enc.finish().expect("gzip");
        let sha: &'static str = Box::leak(hex(&Sha256::digest(&gz)).into_boxed_str());
        let size = gz.len() as u64;
        (
            gz,
            Build {
                asset: "test.gz",
                sha256: sha,
                size,
            },
        )
    }

    #[test]
    fn installs_verified_archive_as_executable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("bin").join("ffmpeg");
        let payload = b"\x7fELF fake binary".repeat(1000);
        let (gz, build) = fake_build(&payload);
        let mut last = 0.0;
        install_from(
            gz.as_slice(),
            &build,
            &target,
            &AtomicBool::new(false),
            &mut |p| last = p,
        )
        .expect("install");
        assert_eq!(std::fs::read(&target).expect("read"), payload);
        assert_eq!(last, 1.0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&target)
                    .expect("meta")
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
    }

    #[test]
    fn tampered_download_is_rejected_and_nothing_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("ffmpeg");
        let (mut gz, build) = fake_build(b"payload");
        let last = gz.len() - 1;
        gz[last] ^= 0xff;
        let result = install_from(
            gz.as_slice(),
            &build,
            &target,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(matches!(result, Err(SetupError::Checksum)));
        assert!(!target.exists());
        // No stray temporary files are left behind.
        assert_eq!(std::fs::read_dir(dir.path()).expect("dir").count(), 0);
    }

    #[test]
    fn oversized_download_stops_early() {
        let (gz, mut build) = fake_build(b"payload");
        build.size = 5;
        let result = download_verified(
            gz.as_slice(),
            &mut Vec::new(),
            &build,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(matches!(result, Err(SetupError::Checksum)));
    }

    #[test]
    fn this_platform_has_a_pinned_build() {
        if let Some(build) = build_for_this_platform() {
            assert_eq!(build.sha256.len(), 64);
            assert!(build.asset.ends_with(".gz"));
        }
    }

    /// Network check of the pinned URL and digest. It downloads but never runs
    /// the file: `cargo test ffmpeg_setup -- --ignored`.
    #[test]
    #[ignore = "downloads ~30 MB"]
    fn pinned_download_matches_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let build = build_for_this_platform().expect("platform");
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .build()
            .into();
        let mut response = agent
            .get(&format!("{RELEASE_URL}/{}", build.asset))
            .call()
            .expect("download");
        let target = dir.path().join("ffmpeg");
        install_from(
            response.body_mut().as_reader(),
            &build,
            &target,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("install");
        let head = std::fs::read(&target).expect("read");
        let is_executable_format = head.starts_with(b"\x7fELF")
            || head.starts_with(b"MZ")
            || head.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]);
        assert!(is_executable_format, "unexpected file header");
    }
}
