//! Checking GitHub for a newer FMX and installing it safely.
//!
//! What keeps this safe:
//! - Requests go only over HTTPS, to this repository's GitHub releases.
//! - Only a version newer than the running one is offered, never a downgrade,
//!   draft or pre-release.
//! - Every downloaded file needs a minisign (Ed25519) signature made with the
//!   release key, whose public half is compiled into FMX. The signed "trusted
//!   comment" must name exactly this file and version, so an older or different
//!   signed file cannot be swapped in.
//! - Downloads are size-limited, verified while streaming, and only moved into
//!   place once the signature checks out. Nothing unverified is ever run.

use std::fmt;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;

use crate::externals::TOOLS_DIR;
use crate::i18n::{tr, trf};
use crate::net::{agent, persist_executable};
use crate::process::spawn_detached;

/// The running version, from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/sudo-py-dev/fmx/releases/latest";
const DOWNLOAD_PREFIX: &str = "https://github.com/sudo-py-dev/fmx/releases/download/";
const RELEASES_PAGE: &str = "https://github.com/sudo-py-dev/fmx/releases";
/// The release signing key (public half); the private half signs in CI.
const RELEASE_KEY: &str = include_str!("../packaging/update-key.pub");
const MAX_API_BYTES: u64 = 1 << 20;
const MAX_SIGNATURE_BYTES: u64 = 4 << 10;
const MAX_PACKAGE_BYTES: u64 = 800 << 20;
const MAX_BINARY_BYTES: u64 = 400 << 20;
const CHUNK: usize = 64 * 1024;
/// The most release-note text shown in the app.
const MAX_NOTES_CHARS: usize = 20_000;
/// How often FMX looks for updates on its own, in seconds.
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// A plain `major.minor.patch` version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    /// Parses `1.2.3` or `v1.2.3`. Pre-release and build suffixes are refused,
    /// so test builds are never offered as updates.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        let mut next = || parts.next()?.parse::<u64>().ok();
        let version = Self {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        parts.next().is_none().then_some(version)
    }

    pub fn current() -> Self {
        Self::parse(VERSION).expect("Cargo.toml has a plain version")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// How FMX was installed, which decides what an update downloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// The Debian package (`/usr/bin/fmx`).
    Deb,
    /// The portable Linux archive, run from wherever it was unpacked.
    LinuxPortable,
    WindowsSetup,
    MacDmg,
}

impl InstallKind {
    /// This computer's kind, if releases include a build for it.
    pub fn detect() -> Option<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => Some(Self::WindowsSetup),
            ("macos", _) => Some(Self::MacDmg),
            ("linux", "x86_64") => {
                // Snaps are updated automatically by snapd. Attempting to self-update
                // would fail against the read-only SquashFS mount.
                if std::env::var_os("SNAP").is_some() || std::env::var_os("SNAP_NAME").is_some() {
                    return None;
                }
                let exe = std::env::current_exe().ok()?;
                Some(if exe.starts_with("/usr") {
                    Self::Deb
                } else {
                    Self::LinuxPortable
                })
            }
            _ => None,
        }
    }

    /// The release file for this kind; the release workflow uses the same names.
    pub fn asset_name(self, version: Version) -> String {
        match self {
            Self::Deb => format!("fmx_{version}_amd64.deb"),
            Self::LinuxPortable => format!("fmx-{version}-linux-x86_64.tar.gz"),
            Self::WindowsSetup => format!("fmx-{version}-windows-x64-setup.exe"),
            Self::MacDmg => format!("fmx-{version}-macos-universal.dmg"),
        }
    }
}

/// The signed comment every release file's signature must carry.
pub fn trusted_comment(version: Version, asset: &str) -> String {
    format!("fmx {version} {asset}")
}

/// A newer release, ready to download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release page, for "What's new" when the release has no notes.
    pub page: String,
    /// What changed, from the release's notes; empty when it has none.
    pub notes: String,
    pub kind: InstallKind,
    asset: String,
    url: String,
    signature_url: String,
    size: u64,
}

impl Release {
    /// A made-up newer release, for screenshots of the update UI. It is never downloaded.
    #[cfg(debug_assertions)]
    pub fn sample() -> Self {
        let version = Version::parse("99.0.0").expect("valid version");
        Self {
            version,
            page: RELEASES_PAGE.to_owned(),
            notes:
                "### New\n- A cleaner look\n- **Faster** conversions\n\n### Fixed\n- A rare crash"
                    .to_owned(),
            kind: InstallKind::LinuxPortable,
            asset: String::new(),
            url: String::new(),
            signature_url: String::new(),
            size: 0,
        }
    }
}

#[derive(Debug)]
pub enum UpdateError {
    Unsupported,
    Network(String),
    BadRelease,
    Signature,
    TooLarge,
    /// The package manager did not install the update, or the password prompt was dismissed.
    NotInstalled,
    Io(std::io::Error),
    Cancelled,
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str(tr("update.unsupported")),
            Self::Network(e) => f.write_str(&trf("update.network", &[("error", e)])),
            Self::BadRelease => f.write_str(tr("update.bad_release")),
            Self::Signature => f.write_str(tr("update.bad_signature")),
            Self::TooLarge => f.write_str(tr("update.too_large")),
            Self::NotInstalled => f.write_str(tr("update.not_installed")),
            Self::Io(e) => f.write_str(&trf("error.io", &[("error", e)])),
            Self::Cancelled => f.write_str(tr("status.cancelled")),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn network(e: ureq::Error) -> UpdateError {
    UpdateError::Network(e.to_string())
}

/// Picks the update for `kind` out of GitHub's "latest release" answer, if it
/// is newer than `current` and well-formed.
fn newer_release(
    json: &str,
    current: Version,
    kind: InstallKind,
) -> Result<Option<Release>, UpdateError> {
    let api: ApiRelease = serde_json::from_str(json).map_err(|_| UpdateError::BadRelease)?;
    let Some(version) = Version::parse(&api.tag_name) else {
        return Ok(None);
    };
    if api.draft || api.prerelease || version <= current {
        return Ok(None);
    }
    let asset = kind.asset_name(version);
    let signature = format!("{asset}.minisig");
    // Files must come from this repository's download area for this exact tag.
    let expected_prefix = format!("{DOWNLOAD_PREFIX}{}/", api.tag_name);
    let find = |name: &str| {
        api.assets.iter().find(|a| {
            a.name == name && a.browser_download_url == format!("{expected_prefix}{name}")
        })
    };
    let (Some(file), Some(sig)) = (find(&asset), find(&signature)) else {
        return Err(UpdateError::BadRelease);
    };
    if file.size > MAX_PACKAGE_BYTES {
        return Err(UpdateError::TooLarge);
    }
    let page = if api.html_url.starts_with(RELEASES_PAGE) {
        api.html_url
    } else {
        RELEASES_PAGE.to_owned()
    };
    Ok(Some(Release {
        version,
        page,
        notes: whats_new(api.body.as_deref().unwrap_or_default()),
        kind,
        url: file.browser_download_url.clone(),
        signature_url: sig.browser_download_url.clone(),
        size: file.size,
        asset,
    }))
}

/// The "What's new" part of a release's notes: everything before its
/// download instructions, without the section's own heading.
fn whats_new(body: &str) -> String {
    let body = body.replace("\r\n", "\n");
    let notes = body.split("\n## Download").next().unwrap_or_default();
    let notes = notes.trim_start();
    let notes = notes
        .strip_prefix("## What's new")
        .or_else(|| notes.strip_prefix("## Download").map(|_| ""))
        .unwrap_or(notes)
        .trim();
    notes.chars().take(MAX_NOTES_CHARS).collect()
}

/// Asks GitHub for the latest release; `None` means FMX is up to date.
pub fn check() -> Result<Option<Release>, UpdateError> {
    let kind = InstallKind::detect().ok_or(UpdateError::Unsupported)?;
    let response = agent()
        .get(LATEST_RELEASE_API)
        .header("User-Agent", &format!("fmx/{VERSION}"))
        .header("Accept", "application/vnd.github+json")
        .call();
    let mut response = match response {
        // No published release yet: nothing newer exists.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        other => other.map_err(network)?,
    };
    let json = response
        .body_mut()
        .with_config()
        .limit(MAX_API_BYTES)
        .read_to_string()
        .map_err(network)?;
    newer_release(&json, Version::current(), kind)
}

/// Checks a signature file against `key` and the comment it must carry.
fn checked_signature(text: &str, key: &PublicKey, comment: &str) -> Result<Signature, UpdateError> {
    let signature = Signature::decode(text).map_err(|_| UpdateError::Signature)?;
    // The comment is covered by the signature, which `finalize` checks later.
    if signature.trusted_comment() != comment {
        return Err(UpdateError::Signature);
    }
    key.verify_stream(&signature)
        .map_err(|_| UpdateError::Signature)?;
    Ok(signature)
}

/// Streams `source` into `dest` while verifying it against `signature`.
fn download_signed(
    mut source: impl Read,
    dest: &mut impl std::io::Write,
    key: &PublicKey,
    signature: &Signature,
    size: u64,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32),
) -> Result<(), UpdateError> {
    let mut verifier = key
        .verify_stream(signature)
        .map_err(|_| UpdateError::Signature)?;
    let mut buf = vec![0u8; CHUNK];
    let mut received = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(UpdateError::Cancelled);
        }
        let n = source
            .read(&mut buf)
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > size {
            return Err(UpdateError::TooLarge);
        }
        verifier.update(&buf[..n]);
        dest.write_all(&buf[..n])?;
        on_progress((received as f64 / size.max(1) as f64).min(1.0) as f32);
    }
    verifier.finalize().map_err(|_| UpdateError::Signature)
}

fn release_key() -> Result<PublicKey, UpdateError> {
    PublicKey::decode(RELEASE_KEY).map_err(|_| UpdateError::Signature)
}

/// Where downloaded updates wait to be installed.
fn updates_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("fmx").join("updates"))
}

/// Downloads `release` and verifies its signature. Returns the verified file.
pub fn download(
    release: &Release,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<PathBuf, UpdateError> {
    let key = release_key()?;
    let dir = updates_dir().ok_or(UpdateError::Unsupported)?;
    std::fs::create_dir_all(&dir)?;
    let agent = agent();
    let signature_text = agent
        .get(&release.signature_url)
        .call()
        .map_err(network)?
        .body_mut()
        .with_config()
        .limit(MAX_SIGNATURE_BYTES)
        .read_to_string()
        .map_err(network)?;
    let comment = trusted_comment(release.version, &release.asset);
    let signature = checked_signature(&signature_text, &key, &comment)?;
    let mut response = agent.get(&release.url).call().map_err(network)?;
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
    download_signed(
        response.body_mut().as_reader(),
        tmp.as_file_mut(),
        &key,
        &signature,
        release.size,
        cancel,
        &mut on_progress,
    )?;
    let target = dir.join(&release.asset);
    tmp.persist(&target).map_err(|e| UpdateError::Io(e.error))?;
    Ok(target)
}

/// What the user still has to do after [`install`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Installed {
    /// The installer is running; FMX should close so it can replace files.
    QuitForInstaller,
    /// The system's package installer or disk image is open.
    FinishInInstaller,
    /// The new version is in place and starts on the next launch.
    Restart,
}

/// Hands a verified update to the system, or replaces the portable binary.
pub fn install(release: &Release, file: &Path) -> Result<Installed, UpdateError> {
    match release.kind {
        InstallKind::WindowsSetup => {
            // The setup closes FMX, upgrades it, and starts it again (its [Run] section).
            spawn_detached(
                &file.to_string_lossy(),
                [
                    "/SILENT",
                    "/SUPPRESSMSGBOXES",
                    "/NORESTART",
                    "/CLOSEAPPLICATIONS",
                    "/NORESTARTAPPLICATIONS",
                ],
            )?;
            Ok(Installed::QuitForInstaller)
        }
        InstallKind::MacDmg => {
            spawn_detached("open", [file])?;
            Ok(Installed::FinishInInstaller)
        }
        InstallKind::Deb => {
            install_deb(file)?;
            Ok(Installed::Restart)
        }
        InstallKind::LinuxPortable => {
            let exe = std::env::current_exe()?;
            replace_from_archive(file, &exe)?;
            Ok(Installed::Restart)
        }
    }
}

/// Installs a verified `.deb` over the current one, asking for the password
/// through the desktop's own prompt. Blocks until the package manager is done,
/// so callers run it off the UI thread.
fn install_deb(file: &Path) -> Result<(), UpdateError> {
    let status = std::process::Command::new("pkexec")
        .args(["apt-get", "install", "-y", "--allow-downgrades"])
        .arg(file)
        .stdin(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        tracing::info!(%status, "package manager did not install the update");
        Err(UpdateError::NotInstalled)
    }
}

/// Where an archive entry goes: the program itself (top level or inside the
/// archive's one folder) or a file in its bundled `tools/bin` or `tools/lib`.
/// Anything else, including `..` or absolute paths, is skipped.
fn portable_dest(path: &Path, exe: &Path) -> Option<PathBuf> {
    let parts: Vec<&str> = path
        .components()
        .map(|c| match c {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let tail = match parts.as_slice() {
        [_, rest @ ..] if parts.len() > 1 && parts[0] != TOOLS_DIR => rest,
        all => all,
    };
    match tail {
        ["fmx"] => Some(exe.to_path_buf()),
        [tools, sub @ ("bin" | "lib"), name] if *tools == TOOLS_DIR => {
            Some(exe.parent()?.join(TOOLS_DIR).join(sub).join(name))
        }
        _ => None,
    }
}

/// Takes `fmx` and its bundled tools out of a verified `.tar.gz` and swaps
/// each in beside `exe` in one step (a running program can be replaced on Linux).
fn replace_from_archive(archive: &Path, exe: &Path) -> Result<(), UpdateError> {
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    let mut entries = tar::Archive::new(gz);
    let mut replaced_app = false;
    for entry in entries.entries()? {
        let mut entry = entry?;
        let Some(target) = portable_dest(&entry.path()?, exe) else {
            continue;
        };
        if !entry.header().entry_type().is_file() {
            continue;
        }
        if entry.size() > MAX_BINARY_BYTES {
            return Err(UpdateError::TooLarge);
        }
        let dir = target.parent().ok_or(UpdateError::Unsupported)?;
        std::fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        std::io::copy(&mut (&mut entry).take(MAX_BINARY_BYTES), tmp.as_file_mut())?;
        persist_executable(tmp, &target)?;
        replaced_app |= target == *exe;
    }
    if replaced_app {
        Ok(())
    } else {
        Err(UpdateError::BadRelease)
    }
}

/// Starts the updated program with the restart flag; the caller then closes this one.
/// Returns an error if the current executable cannot be found or the process cannot start.
pub fn relaunch() -> Result<(), UpdateError> {
    let exe = std::env::current_exe()?;
    // Linux names a running program whose file was replaced "<path> (deleted)".
    let exe = exe.to_string_lossy();
    spawn_detached(
        exe.trim_end_matches(" (deleted)"),
        ["--restart-after-update"],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway key pair used only by these tests (not the release key).
    const TEST_KEY: &str = include_str!("../packaging/test-fixtures/test-key.pub");
    const TEST_PAYLOAD: &[u8] = include_bytes!("../packaging/test-fixtures/payload.bin");
    const TEST_SIGNATURE: &str = include_str!("../packaging/test-fixtures/payload.bin.minisig");
    /// The trusted comment the fixture was signed with.
    const TEST_COMMENT: &str = "fmx 9.9.9 fmx-9.9.9-linux-x86_64.tar.gz";

    fn v(text: &str) -> Version {
        Version::parse(text).expect("version")
    }

    #[test]
    fn versions_parse_and_order() {
        assert!(v("0.0.2") > v("0.0.1"));
        assert!(v("v0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
        for bad in ["1.2", "1.2.3.4", "1.2.3-beta", "x.y.z", ""] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(Version::current().to_string(), VERSION);
    }

    fn api_json(tag: &str, names: &[&str]) -> String {
        let assets: Vec<String> = names
            .iter()
            .map(|n| {
                format!(
                    r#"{{"name":"{n}","size":1000,"browser_download_url":"{DOWNLOAD_PREFIX}{tag}/{n}"}}"#
                )
            })
            .collect();
        format!(
            r#"{{"tag_name":"{tag}","html_url":"{RELEASES_PAGE}/tag/{tag}","draft":false,"prerelease":false,"assets":[{}]}}"#,
            assets.join(",")
        )
    }

    #[test]
    fn only_newer_well_formed_releases_are_offered() {
        let kind = InstallKind::LinuxPortable;
        let name = kind.asset_name(v("0.0.2"));
        let sig = format!("{name}.minisig");
        let json = api_json("v0.0.2", &[&name, &sig]);
        let release = newer_release(&json, v("0.0.1"), kind)
            .expect("parse")
            .expect("newer");
        assert_eq!(release.version, v("0.0.2"));
        assert_eq!(release.url, format!("{DOWNLOAD_PREFIX}v0.0.2/{name}"));
        // Same or older: nothing to offer.
        assert_eq!(newer_release(&json, v("0.0.2"), kind).expect("parse"), None);
        assert_eq!(newer_release(&json, v("0.1.0"), kind).expect("parse"), None);
        // No signature published: refused.
        let unsigned = api_json("v0.0.2", &[&name]);
        assert!(newer_release(&unsigned, v("0.0.1"), kind).is_err());
    }

    #[test]
    fn notes_are_the_part_before_the_downloads() {
        let body = "## What's new\r\n\r\n- Faster\r\n- Cleaner\r\n\r\n## Download\r\n\r\n| x |";
        assert_eq!(whats_new(body), "- Faster\n- Cleaner");
        // Only download instructions: nothing to show.
        assert_eq!(whats_new("## Download\n\n| System | File |"), "");
        assert_eq!(whats_new(""), "");
        assert_eq!(whats_new("Just text"), "Just text");
    }

    #[test]
    fn files_from_elsewhere_are_refused() {
        let kind = InstallKind::Deb;
        let name = kind.asset_name(v("0.0.2"));
        let json = api_json("v0.0.2", &[&name, &format!("{name}.minisig")])
            .replace("github.com/sudo-py-dev/fmx", "evil.example/fmx");
        assert!(newer_release(&json, v("0.0.1"), kind).is_err());
    }

    #[test]
    fn signed_file_verifies_and_tampering_is_caught() {
        let key = PublicKey::decode(TEST_KEY).expect("key");
        let cancel = AtomicBool::new(false);
        let signature = checked_signature(TEST_SIGNATURE, &key, TEST_COMMENT).expect("signature");
        let size = TEST_PAYLOAD.len() as u64;
        let mut out = Vec::new();
        download_signed(
            TEST_PAYLOAD,
            &mut out,
            &key,
            &signature,
            size,
            &cancel,
            &mut |_| {},
        )
        .expect("valid");
        assert_eq!(out, TEST_PAYLOAD);

        let mut tampered = TEST_PAYLOAD.to_vec();
        tampered[0] ^= 1;
        let result = download_signed(
            tampered.as_slice(),
            &mut Vec::new(),
            &key,
            &signature,
            size,
            &cancel,
            &mut |_| {},
        );
        assert!(matches!(result, Err(UpdateError::Signature)));
    }

    #[test]
    fn signature_for_another_file_or_version_is_refused() {
        let key = PublicKey::decode(TEST_KEY).expect("key");
        let other = "fmx 9.9.8 fmx-9.9.8-linux-x86_64.tar.gz";
        assert!(matches!(
            checked_signature(TEST_SIGNATURE, &key, other),
            Err(UpdateError::Signature)
        ));
        // A signature from a different key is refused even with the right comment.
        let release = release_key().expect("release key");
        assert!(checked_signature(TEST_SIGNATURE, &release, TEST_COMMENT).is_err());
    }

    fn archive_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(gz);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, name, *data)
                .expect("append");
        }
        builder.into_inner().expect("tar").finish().expect("gzip")
    }

    #[test]
    fn portable_paths_stay_inside_the_install() {
        let exe = Path::new("/opt/fmx/fmx");
        let dest = |p: &str| portable_dest(Path::new(p), exe);
        assert_eq!(dest("fmx"), Some(exe.to_path_buf()));
        assert_eq!(dest("fmx-1.0.0/fmx"), Some(exe.to_path_buf()));
        assert_eq!(
            dest("fmx-1.0.0/tools/bin/yt-dlp"),
            Some(PathBuf::from("/opt/fmx/tools/bin/yt-dlp"))
        );
        assert_eq!(
            dest("tools/lib/libavutil.so.60"),
            Some(PathBuf::from("/opt/fmx/tools/lib/libavutil.so.60"))
        );
        for bad in [
            "a/b/fmx",
            "fmx-1.0.0/tools/../../fmx",
            "/etc/fmx",
            "fmx-1.0.0/tools/bin/../../../x",
            "fmx-1.0.0/tools/share/x",
            "fmx-1.0.0/tools/bin/sub/x",
        ] {
            assert_eq!(dest(bad), None, "{bad}");
        }
    }

    #[test]
    fn portable_update_replaces_the_program() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("fmx");
        std::fs::write(&exe, b"old program").expect("old");
        let archive = dir.path().join("update.tar.gz");
        let data = archive_with(&[
            ("fmx-1.0.0/README.md", b"readme"),
            ("fmx-1.0.0/fmx", b"new program"),
            ("fmx-1.0.0/tools/bin/ffmpeg", b"new ffmpeg"),
            ("fmx-1.0.0/tools/lib/libavcodec.so.62", b"new lib"),
            ("fmx-1.0.0/tools/share/other", b"ignored"),
        ]);
        std::fs::write(&archive, data).expect("archive");
        replace_from_archive(&archive, &exe).expect("replace");
        assert_eq!(std::fs::read(&exe).expect("read"), b"new program");
        let tools = dir.path().join(TOOLS_DIR);
        assert_eq!(
            std::fs::read(tools.join("bin/ffmpeg")).expect("ffmpeg"),
            b"new ffmpeg"
        );
        assert_eq!(
            std::fs::read(tools.join("lib/libavcodec.so.62")).expect("lib"),
            b"new lib"
        );
        assert!(!tools.join("share").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&exe).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        // An archive without the program changes nothing.
        let empty = dir.path().join("empty.tar.gz");
        std::fs::write(&empty, archive_with(&[("other", b"x")])).expect("archive");
        assert!(replace_from_archive(&empty, &exe).is_err());
        assert_eq!(std::fs::read(&exe).expect("read"), b"new program");
    }

    #[test]
    fn release_key_is_valid() {
        release_key().expect("the bundled release key decodes");
    }
}
