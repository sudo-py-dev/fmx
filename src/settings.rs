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
    /// Start jobs as soon as they are added instead of waiting for Start.
    /// (Renamed from `auto_start`, which defaulted to on, so old saves start paused.)
    pub start_on_add: bool,
    pub options: Options,
    pub last_tool: Tool,
    /// UI language code; `None` follows the system locale.
    pub language: Option<String>,
    /// Device path for audio-CD ripping.
    pub cd_device: String,
    pub theme: ThemeChoice,
    /// Turns off transitions and smooth scrolling.
    pub reduce_motion: bool,
    /// Show a system notification when the queue finishes.
    pub notify_when_done: bool,
    /// Look for a newer Muxr on GitHub about once a day.
    pub check_updates: bool,
    /// When Muxr last looked for updates (seconds since 1970).
    pub last_update_check: u64,
    /// A release the user chose to skip; it is not offered again.
    pub skipped_version: Option<String>,
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

impl Settings {
    pub fn sanitize(&mut self) {
        self.parallel_jobs = self.parallel_jobs.clamp(1, MAX_PARALLEL_JOBS);
        self.cd_device.retain(|c| !c.is_control());
        if self.cd_device.trim().is_empty() {
            self.cd_device = default_cd_device().to_owned();
        }
        self.options.delogo.w = self.options.delogo.w.min(16_384);
        self.options.delogo.h = self.options.delogo.h.min(16_384);
        self.options.delogo.x = self.options.delogo.x.min(16_384);
        self.options.delogo.y = self.options.delogo.y.min(16_384);
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: None,
            parallel_jobs: default_parallel_jobs(),
            ffmpeg_path: None,
            start_on_add: false,
            options: Options::default(),
            last_tool: Tool::ToVideo(VideoFormat::Mp4),
            language: None,
            cd_device: default_cd_device().to_owned(),
            theme: ThemeChoice::System,
            reduce_motion: false,
            notify_when_done: true,
            check_updates: true,
            last_update_check: 0,
            skipped_version: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: Settings = serde_json::from_str(r#"{"start_on_add": true}"#).expect("parse");
        assert!(s.start_on_add);
        assert!(s.parallel_jobs >= 1);
        assert_eq!(s.options, Options::default());
    }

    #[test]
    fn jobs_wait_for_start_by_default() {
        // Saves from before the rename carried `auto_start: true`; it must not apply.
        let s: Settings = serde_json::from_str(r#"{"auto_start": true}"#).expect("parse");
        assert!(!s.start_on_add);
    }
}
