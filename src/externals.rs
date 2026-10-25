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

/// Searches beside our executable, then `PATH`, for any of `names`.
fn find_on_path(names: &[&str]) -> Option<PathBuf> {
    let with_ext = |name: &str| {
        if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        }
    };
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let dirs: Vec<PathBuf> = beside_exe
        .into_iter()
        .chain(
            std::env::var_os("PATH")
                .iter()
                .flat_map(std::env::split_paths),
        )
        .collect();
    names.iter().find_map(|name| {
        let file = with_ext(name);
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
    /// Finds every external program; `ffmpeg_override` wins over the search.
    pub fn detect(ffmpeg_override: Option<&Path>) -> Self {
        let paths: HashMap<External, PathBuf> = External::ALL
            .into_iter()
            .filter_map(|ext| {
                let found = match (ext, ffmpeg_override) {
                    (External::Ffmpeg, Some(path)) => path.is_file().then(|| path.to_path_buf()),
                    // Fmx's own copy is the fallback when the system has none.
                    (External::Ffmpeg, None) => find_on_path(ext.binaries())
                        .or_else(|| crate::ffmpeg_setup::managed_binary().filter(|p| p.is_file())),
                    // Fmx's own yt-dlp wins: it is the one kept up to date from
                    // Settings, while system copies are often too old for YouTube.
                    (External::YtDlp, _) => crate::ytdlp_setup::managed_binary()
                        .filter(|p| p.is_file())
                        .or_else(|| find_on_path(ext.binaries())),
                    _ => find_on_path(ext.binaries()),
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
