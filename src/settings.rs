//! Persisted user preferences.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::catalog::{Tool, VideoFormat};
use crate::options::Options;
use crate::theme::ThemeChoice;

pub const STORAGE_KEY: &str = "muxr.settings.v1";
pub const MAX_PARALLEL_JOBS: usize = 16;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `None` writes each output next to its source file.
    pub output_dir: Option<PathBuf>,
    pub parallel_jobs: usize,
    /// `None` searches next to the app, then `PATH`.
    pub ffmpeg_path: Option<PathBuf>,
    pub auto_start: bool,
    pub options: Options,
    pub last_tool: Tool,
    /// UI language code; `None` follows the system locale.
    pub language: Option<String>,
    /// Device path for audio-CD ripping.
    pub cd_device: String,
    pub theme: ThemeChoice,
    /// Turns off transitions and smooth scrolling.
    pub reduce_motion: bool,
}

/// ffmpeg already spreads one encode over several cores, so run about half as
/// many jobs as there are cores.
fn default_parallel_jobs() -> usize {
    std::thread::available_parallelism()
        .map_or(2, |n| n.get() / 2)
        .clamp(1, MAX_PARALLEL_JOBS)
}

fn default_cd_device() -> &'static str {
    if cfg!(windows) { "D:" } else { "/dev/cdrom" }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: None,
            parallel_jobs: default_parallel_jobs(),
            ffmpeg_path: None,
            auto_start: true,
            options: Options::default(),
            last_tool: Tool::ToVideo(VideoFormat::Mp4),
            language: None,
            cd_device: default_cd_device().to_owned(),
            theme: ThemeChoice::System,
            reduce_motion: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: Settings = serde_json::from_str(r#"{"auto_start": false}"#).expect("parse");
        assert!(!s.auto_start);
        assert!(s.parallel_jobs >= 1);
        assert_eq!(s.options, Options::default());
    }
}
