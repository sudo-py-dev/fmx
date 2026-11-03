//! Persisted user preferences.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::catalog::{Tool, VideoFormat};
use crate::options::{Edit, Options};
use crate::theme::ThemeChoice;

pub const STORAGE_KEY: &str = "fmx.settings.v1";
pub const MAX_PARALLEL_JOBS: usize = 16;
/// Longest preset name, in characters.
pub const MAX_PRESET_NAME: usize = 60;

/// A tool and its settings saved under a name, run in one click from the app
/// or the right-click menu.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    /// Stays the same when the preset is renamed; the right-click menu runs it by this.
    pub id: u32,
    pub name: String,
    pub tool: Tool,
    pub options: Options,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            tool: Tool::ToVideo(VideoFormat::Mp4),
            options: Options::default(),
        }
    }
}

/// A preset name without control characters or surrounding spaces, and not too long.
pub fn clean_preset_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    name.trim()
        .chars()
        .take(MAX_PRESET_NAME)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

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
    /// Saved tool-and-settings combinations, in the order they were added.
    pub presets: Vec<Preset>,
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
    /// The welcome guide was finished or skipped.
    pub welcome_seen: bool,
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
    /// Adds a tool to the end of favorites, or removes all its entries if present.
    pub fn toggle_favorite(&mut self, tool: Tool) {
        if self.favorite_tools.contains(&tool) {
            self.favorite_tools.retain(|t| *t != tool);
        } else {
            self.favorite_tools.push(tool);
        }
    }

    /// Saves `tool` with a copy of `options` as a new preset and returns its id.
    /// Clips, crops and watermarks belong to particular files, so they are left out.
    pub fn add_preset(&mut self, name: &str, tool: Tool, options: &Options) -> u32 {
        let id = self.presets.iter().map(|p| p.id).max().map_or(1, |m| m + 1);
        let mut options = options.clone();
        options.edit = Edit::default();
        options.collage_photos.clear();
        self.presets.push(Preset {
            id,
            name: clean_preset_name(name),
            tool,
            options,
        });
        id
    }

    pub fn preset(&self, id: u32) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// Renames a preset; an empty name is ignored.
    pub fn rename_preset(&mut self, id: u32, name: &str) {
        let name = clean_preset_name(name);
        if let Some(preset) = self.presets.iter_mut().find(|p| p.id == id)
            && !name.is_empty()
        {
            preset.name = name;
        }
    }

    pub fn remove_preset(&mut self, id: u32) {
        self.presets.retain(|p| p.id != id);
    }

    /// Normalizes saved settings, including removing duplicate favorites while
    /// preserving their first occurrence.
    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.favorite_tools.retain(|tool| seen.insert(*tool));
        // Ids must be unique and non-zero, or the right-click menu runs the wrong one.
        let mut ids = std::collections::HashSet::new();
        let mut next = self.presets.iter().map(|p| p.id).max().unwrap_or(0);
        for preset in &mut self.presets {
            if preset.id == 0 || !ids.insert(preset.id) {
                next += 1;
                preset.id = next;
                ids.insert(next);
            }
            preset.name = clean_preset_name(&preset.name);
            if preset.name.is_empty() {
                preset.name = preset.tool.label().to_owned();
            }
        }
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
            presets: Vec::new(),
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
            welcome_seen: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_dismissal_survives_restart() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(!settings.welcome_seen);
        settings.welcome_seen = true;
        let saved = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&saved).unwrap();
        assert!(restored.welcome_seen);
    }

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
    fn presets_persist_rename_and_delete() {
        let mut settings = Settings::default();
        let options = Options {
            image_max_side: Some(2000),
            edit: Edit {
                trim_start: "5".into(),
                ..Edit::default()
            },
            ..Options::default()
        };
        let web = settings.add_preset(
            "  Web 2000px\n",
            Tool::ToImage(crate::catalog::ImageFormat::Webp),
            &options,
        );
        let mp3 = settings.add_preset(
            "Podcast",
            Tool::ToAudio(crate::catalog::AudioFormat::Mp3),
            &Options::default(),
        );
        assert_ne!(web, mp3);
        let saved = serde_json::to_string(&settings).unwrap();
        let mut restored: Settings = serde_json::from_str(&saved).unwrap();
        restored.sanitize();
        assert_eq!(restored.presets, settings.presets);
        let preset = restored.preset(web).expect("saved");
        assert_eq!(preset.name, "Web 2000px");
        assert_eq!(preset.options.image_max_side, Some(2000));
        // Clips belong to the files they were set for.
        assert_eq!(preset.options.edit, Edit::default());

        restored.rename_preset(web, "Blog");
        restored.rename_preset(web, "   ");
        assert_eq!(restored.preset(web).unwrap().name, "Blog");
        restored.remove_preset(web);
        assert!(restored.preset(web).is_none());
        // A new preset never reuses a live id.
        let next = restored.add_preset("Again", Tool::Record, &Options::default());
        assert!(next > mp3);

        let old: Settings = serde_json::from_str("{}").unwrap();
        assert!(old.presets.is_empty());
    }

    #[test]
    fn sanitize_repairs_preset_ids_and_names() {
        let mut settings: Settings = serde_json::from_str(
            r#"{"presets": [{"id": 3, "name": "A", "tool": "Record"}, {"id": 3, "name": " ", "tool": "Record"}, {"name": "C", "tool": "Record"}]}"#,
        )
        .expect("parse");
        settings.sanitize();
        let ids: Vec<u32> = settings.presets.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![3, 4, 5]);
        assert_eq!(settings.presets[1].name, Tool::Record.label());
        assert_eq!(
            clean_preset_name(&"x".repeat(100)).chars().count(),
            MAX_PRESET_NAME
        );
    }

    #[test]
    fn jobs_wait_for_start_by_default() {
        // Saves from before the rename carried `auto_start: true`; it must not apply.
        let s: Settings = serde_json::from_str(r#"{"auto_start": true}"#).expect("parse");
        assert!(!s.start_on_add);
    }
}
