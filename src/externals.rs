//! Discovery of the external programs some tools delegate to.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::process::command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum External {
    Ffmpeg,
    YtDlp,
    Pdftoppm,
    Pdftotext,
    Pdftohtml,
    Pdfunite,
    Pdfseparate,
    Ghostscript,
    Soffice,
    EbookConvert,
    Xorriso,
    MonkeysAudio,
}

impl External {
    pub const ALL: [Self; 12] = [
        Self::Ffmpeg,
        Self::YtDlp,
        Self::Pdftoppm,
        Self::Pdftotext,
        Self::Pdftohtml,
        Self::Pdfunite,
        Self::Pdfseparate,
        Self::Ghostscript,
        Self::Soffice,
        Self::EbookConvert,
        Self::Xorriso,
        Self::MonkeysAudio,
    ];

    /// Executable names to try, most common first.
    fn binaries(self) -> &'static [&'static str] {
        match self {
            Self::Ffmpeg => &["ffmpeg"],
            Self::YtDlp => &["yt-dlp"],
            Self::Pdftoppm => &["pdftoppm"],
            Self::Pdftotext => &["pdftotext"],
            Self::Pdftohtml => &["pdftohtml"],
            Self::Pdfunite => &["pdfunite"],
            Self::Pdfseparate => &["pdfseparate"],
            Self::Ghostscript => &["gs", "gswin64c", "gswin32c"],
            Self::Soffice => &["soffice", "libreoffice"],
            Self::EbookConvert => &["ebook-convert"],
            Self::Xorriso => &["xorriso"],
            Self::MonkeysAudio => &["mac"],
        }
    }

    /// Whether FMX offers installation help for this program on this OS.
    /// Monkey's Audio's linked installer is Windows-only; xorriso targets Unix.
    /// Discovery remains independent so existing command-line ports still work.
    pub fn installation_supported(self) -> bool {
        match self {
            Self::MonkeysAudio => cfg!(target_os = "windows"),
            Self::Xorriso => cfg!(unix),
            _ => true,
        }
    }

    /// Official download or installation documentation for the program.
    pub fn install_url(self) -> &'static str {
        match self {
            Self::Ffmpeg => "https://ffmpeg.org/download.html",
            Self::YtDlp => "https://github.com/yt-dlp/yt-dlp#installation",
            Self::Pdftoppm
            | Self::Pdftotext
            | Self::Pdftohtml
            | Self::Pdfunite
            | Self::Pdfseparate => "https://poppler.freedesktop.org/",
            Self::Ghostscript => "https://ghostscript.com/releases/gsdnld.html",
            Self::Soffice => "https://www.libreoffice.org/download/download-libreoffice/",
            Self::EbookConvert => "https://calibre-ebook.com/download",
            Self::Xorriso => "https://www.gnu.org/software/xorriso/",
            Self::MonkeysAudio => "https://monkeysaudio.com/download.html",
        }
    }

    /// Program name shown to users (not translated).
    pub fn name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::YtDlp => "yt-dlp",
            Self::Pdftoppm
            | Self::Pdftotext
            | Self::Pdftohtml
            | Self::Pdfunite
            | Self::Pdfseparate => "Poppler (poppler-utils)",
            Self::Ghostscript => "Ghostscript",
            Self::Soffice => "LibreOffice",
            Self::EbookConvert => "Calibre",
            Self::Xorriso => "xorriso",
            Self::MonkeysAudio => "Monkey's Audio (mac)",
        }
    }
}

/// Folder beside the executable that release packages fill with ffmpeg,
/// ffprobe and yt-dlp (`tools/bin`, plus `tools/lib` on Linux).
pub const TOOLS_DIR: &str = "tools";

/// Where release packages put FMX's bundled programs: `tools/bin` beside the
/// executable (portable archive, Windows setup, macOS app bundle, snap) or,
/// for the Debian package, `/usr/lib/fmx/bin`. The executable's own folder
/// comes last, for programs a user drops next to a portable copy.
fn bundled_dirs() -> Vec<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let mut dirs: Vec<PathBuf> = exe_dir
        .iter()
        .map(|d| d.join(TOOLS_DIR).join("bin"))
        .collect();
    if cfg!(target_os = "linux") {
        dirs.push(PathBuf::from("/usr/lib/fmx/bin"));
    }
    dirs.extend(exe_dir);
    dirs
}

/// Searches the bundled directories first, then `PATH`, for any of `names`,
/// so a release always runs the exact tools it shipped with.
fn find_program(names: &[&str]) -> Option<PathBuf> {
    let path_dirs = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    let dirs: Vec<PathBuf> = bundled_dirs().into_iter().chain(path_dirs).collect();
    names.iter().find_map(|name| {
        let file = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            (*name).to_owned()
        };
        dirs.iter().map(|dir| dir.join(&file)).find(|p| p.is_file())
    })
}

/// Located external programs plus what the found ffmpeg can encode.
#[derive(Clone, Debug, Default)]
pub struct Toolbox {
    paths: HashMap<External, PathBuf>,
    encoders: HashSet<String>,
}

impl Toolbox {
    /// Finds every external program. For ffmpeg, `ffmpeg_override` (from
    /// Settings) wins, then `FMX_FFMPEG`, then the normal search.
    pub fn detect(ffmpeg_override: Option<&Path>) -> Self {
        let override_path = ffmpeg_override
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("FMX_FFMPEG").map(PathBuf::from));
        let paths: HashMap<External, PathBuf> = External::ALL
            .into_iter()
            .filter_map(|ext| {
                let found = match (ext, override_path.as_deref()) {
                    (External::Ffmpeg, Some(path)) => path.is_file().then(|| path.to_path_buf()),
                    _ => find_program(ext.binaries()),
                };
                found.map(|p| (ext, p))
            })
            .collect();
        let encoders = paths
            .get(&External::Ffmpeg)
            .map(|ffmpeg| list_encoders(ffmpeg))
            .unwrap_or_default();
        Self { paths, encoders }
    }

    pub fn get(&self, ext: External) -> Option<&Path> {
        self.paths.get(&ext).map(PathBuf::as_path)
    }

    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders.contains(name)
    }

    /// The first missing requirement, if any.
    pub fn missing(&self, needed: &[External]) -> Option<External> {
        needed
            .iter()
            .copied()
            .find(|ext| !self.paths.contains_key(ext))
    }
}

fn list_encoders(ffmpeg: &Path) -> HashSet<String> {
    let output = command(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(out) => parse_encoders(&String::from_utf8_lossy(&out.stdout)),
        Err(e) => {
            tracing::warn!(error = %e, "could not list ffmpeg encoders");
            HashSet::new()
        }
    }
}

/// Parses `ffmpeg -encoders`: rows look like ` V....D libx264   description`.
pub fn parse_encoders(text: &str) -> HashSet<String> {
    text.lines()
        .skip_while(|line| !line.trim_start().starts_with("------"))
        .skip(1)
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let flags = parts.next()?;
            let name = parts.next()?;
            (flags.len() == 6).then(|| name.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_encoder_table() {
        let text = "Encoders:\n V..... = Video\n ------\n V....D libx264              H.264\n A....D aac                  AAC\n";
        let set = parse_encoders(text);
        assert!(set.contains("libx264") && set.contains("aac"));
        assert!(!set.contains("="));
    }

    #[test]
    fn override_to_missing_file_is_not_found() {
        let tb = Toolbox::detect(Some(Path::new("/definitely/not/here/ffmpeg")));
        assert!(tb.get(External::Ffmpeg).is_none());
        assert_eq!(tb.missing(&[External::Ffmpeg]), Some(External::Ffmpeg));
    }
}
