//! Persisted user preferences.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::catalog::{Tool, VideoFormat};
use crate::options::Options;
use crate::theme::ThemeChoice;

pub const STORAGE_KEY: &str = "fmx.settings.v1";
pub const MAX_PARALLEL_JOBS: usize = 16;

/// Which sound to play when the queue finishes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompletionSound {
    /// Play the system's default completion sound.
    #[default]
    Default,
    /// A gentle bell tone.
    Bell,
    /// An upbeat victory jingle.
    Victory,
    /// A user-supplied audio file (see `sound::CUSTOM_EXTENSIONS`).
    Custom(PathBuf),
    /// Silent — do not play anything.
    Off,
}

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
    /// Tools pinned for quick access, in the order they were added.
    pub favorite_tools: Vec<Tool>,
    /// UI language code; `None` follows the system locale.
    pub language: Option<String>,
    /// Device path for audio-CD ripping.
    pub cd_device: String,
    pub theme: ThemeChoice,
    /// Turns off transitions and smooth scrolling.
    pub reduce_motion: bool,
    /// Show a system notification when the queue finishes.
    pub notify_when_done: bool,
    /// Look for a newer FMX on GitHub about once a day.
    pub check_updates: bool,
    /// When FMX last looked for updates (seconds since 1970).
    pub last_update_check: u64,
    /// A release the user chose to skip; it is not offered again.
    pub skipped_version: Option<String>,
    /// Sound to play when all jobs finish; `Off` is silent.
    pub completion_sound: CompletionSound,
    /// The right-click menu was added once (first start); later the user decides.
    pub shell_menu_setup: bool,
}

/// ffmpeg already spreads one encode over several cores, so run about half as
/// many jobs as there are cores.
fn default_parallel_jobs() -> usize {
    std::thread::available_parallelism()
        .map_or(2, |n| n.get() / 2)
        .clamp(1, MAX_PARALLEL_JOBS)
}

/// The drive assumed until one is found or chosen.
pub fn default_cd_device() -> &'static str {
    if cfg!(windows) { "D:" } else { "/dev/cdrom" }
}

impl Settings {
    pub fn toggle_favorite(&mut self, tool: Tool) {
        if self.favorite_tools.contains(&tool) {
            self.favorite_tools.retain(|t| *t != tool);
        } else {
            self.favorite_tools.push(tool);
        }
    }

    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.favorite_tools.retain(|tool| seen.insert(*tool));
        self.parallel_jobs = self.parallel_jobs.clamp(1, MAX_PARALLEL_JOBS);
        self.cd_device.retain(|c| !c.is_control());
        if self.cd_device.trim().is_empty() {
            self.cd_device = default_cd_device().to_owned();
        }
        self.options.delogo.w = self.options.delogo.w.min(16_384);
        self.options.delogo.h = self.options.delogo.h.min(16_384);
        self.options.delogo.x = self.options.delogo.x.min(16_384);
        self.options.delogo.y = self.options.delogo.y.min(16_384);
        // Older versions could save "custom" before a file was chosen.
        if matches!(&self.completion_sound, CompletionSound::Custom(p) if p.as_os_str().is_empty())
        {
            self.completion_sound = CompletionSound::Default;
        }
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
            favorite_tools: Vec::new(),
            language: None,
            cd_device: default_cd_device().to_owned(),
            theme: ThemeChoice::System,
            reduce_motion: false,
            notify_when_done: true,
            check_updates: true,
            last_update_check: 0,
            skipped_version: None,
            completion_sound: CompletionSound::Default,
            shell_menu_setup: false,
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
    fn favorites_persist_and_toggle_without_duplicates() {
        let mut settings = Settings::default();
        assert!(settings.favorite_tools.is_empty());
        settings.toggle_favorite(Tool::RecordAudio);
        settings.toggle_favorite(Tool::Record);
        let saved = serde_json::to_string(&settings).unwrap();
        let mut restored: Settings = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            restored.favorite_tools,
            vec![Tool::RecordAudio, Tool::Record]
        );
        restored.favorite_tools.push(Tool::RecordAudio);
        restored.sanitize();
        assert_eq!(restored.favorite_tools, settings.favorite_tools);
        restored.toggle_favorite(Tool::RecordAudio);
        assert_eq!(restored.favorite_tools, vec![Tool::Record]);
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert!(old.favorite_tools.is_empty());
    }

    #[test]
    fn jobs_wait_for_start_by_default() {
        // Saves from before the rename carried `auto_start: true`; it must not apply.
        let s: Settings = serde_json::from_str(r#"{"auto_start": true}"#).expect("parse");
        assert!(!s.start_on_add);
    }
}
