//! What the screen recorder can capture on this system: its monitors, a
//! microphone, and the sound the computer plays.
//!
//! - Linux: X11 monitors from `xrandr`; sound through PulseAudio (or
//!   PipeWire's Pulse server), whose default output always has a monitor.
//! - Windows: DirectShow audio devices. Computer sound needs a loopback
//!   device such as Stereo Mix or a virtual cable.
//! - macOS: AVFoundation's screens and audio devices. Computer sound needs a
//!   loopback driver such as BlackHole.

use std::path::Path;

use crate::options::{RecordAudio, ScreenRect};
use crate::process;

/// Audio devices that carry what the computer plays rather than a microphone.
const LOOPBACK_NAMES: [&str; 9] = [
    "stereo mix",
    "what u hear",
    "wave out",
    "virtual-audio-capturer",
    "cable output",
    "voicemeeter out",
    "blackhole",
    "soundflower",
    "loopback",
];

/// A monitor the recorder can capture alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub name: String,
    /// Its place on the desktop, where the system says.
    pub rect: Option<ScreenRect>,
    /// The capture device that records it (macOS).
    pub device: Option<String>,
}

/// Everything the recorder found, as ffmpeg names it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Devices {
    pub monitors: Vec<Monitor>,
    pub microphone: Option<String>,
    /// Records what the computer plays.
    pub loopback: Option<String>,
}

impl Devices {
    /// The sound choices these devices allow.
    pub fn audio_choices(&self) -> Vec<RecordAudio> {
        let mut choices = vec![RecordAudio::None];
        if self.microphone.is_some() {
            choices.push(RecordAudio::Microphone);
        }
        if self.loopback.is_some() {
            choices.push(RecordAudio::System);
            if self.microphone.is_some() {
                choices.push(RecordAudio::Both);
            }
        }
        choices
    }

    /// ffmpeg's input format and device for each sound to record.
    pub fn sound_inputs(&self, audio: RecordAudio) -> Vec<(&'static str, String)> {
        let format = if cfg!(windows) {
            "dshow"
        } else if cfg!(target_os = "macos") {
            "avfoundation"
        } else {
            "pulse"
        };
        let wanted = [
            (audio.microphone(), &self.microphone),
            (audio.system(), &self.loopback),
        ];
        wanted
            .into_iter()
            .filter_map(|(want, device)| device.as_ref().filter(|_| want))
            .map(|device| {
                let input = if cfg!(windows) {
                    format!("audio={device}")
                } else if cfg!(target_os = "macos") {
                    format!(":{device}")
                } else {
                    device.clone()
                };
                (format, input)
            })
            .collect()
    }
}

/// Finds this system's capture devices; `ffmpeg` lists them on Windows and
/// macOS. Takes a moment, so call it off the UI thread.
pub fn detect(ffmpeg: Option<&Path>) -> Devices {
    if cfg!(windows) {
        let names = ffmpeg
            .map(|f| list_devices(f, "dshow", "dummy"))
            .unwrap_or_default();
        let audio: Vec<String> = names.lines().filter_map(parse_dshow_audio).collect();
        split_sound(audio)
    } else if cfg!(target_os = "macos") {
        let listing = ffmpeg
            .map(|f| list_devices(f, "avfoundation", ""))
            .unwrap_or_default();
        let (screens, audio) = parse_avfoundation(&listing);
        let monitors = screens
            .into_iter()
            .enumerate()
            .map(|(i, index)| Monitor {
                name: format!("{} {}", crate::i18n::tr("record.screen"), i + 1),
                rect: None,
                device: Some(index),
            })
            .collect();
        let (indexes, names): (Vec<String>, Vec<String>) = audio.into_iter().unzip();
        let mut devices = split_sound(names.clone());
        // AVFoundation opens audio by index.
        let index_of = |name: &String| {
            names
                .iter()
                .position(|n| n == name)
                .map(|i| indexes[i].clone())
        };
        devices.microphone = devices.microphone.as_ref().and_then(index_of);
        devices.loopback = devices.loopback.as_ref().and_then(index_of);
        devices.monitors = monitors;
        devices
    } else {
        Devices {
            monitors: if is_wayland() {
                Vec::new()
            } else {
                xrandr_monitors()
            },
            microphone: Some("default".to_owned()),
            loopback: Some("@DEFAULT_MONITOR@".to_owned()),
        }
    }
}

/// Picks the first loopback device and the first real microphone.
fn split_sound(audio: Vec<String>) -> Devices {
    let is_loopback = |name: &str| {
        let name = name.to_lowercase();
        LOOPBACK_NAMES.iter().any(|l| name.contains(l))
    };
    Devices {
        monitors: Vec::new(),
        loopback: audio.iter().find(|n| is_loopback(n)).cloned(),
        microphone: audio.into_iter().find(|n| !is_loopback(n)),
    }
}

/// ffmpeg's device list for an input format (written to stderr).
fn list_devices(ffmpeg: &Path, format: &str, input: &str) -> String {
    process::command(ffmpeg)
        .args([
            "-hide_banner",
            "-list_devices",
            "true",
            "-f",
            format,
            "-i",
            input,
        ])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stderr).into_owned())
        .unwrap_or_default()
}

/// `[dshow @ 0x1] "Microphone (Realtek Audio)" (audio)` → the quoted name.
fn parse_dshow_audio(line: &str) -> Option<String> {
    let rest = line.trim_end().strip_suffix("(audio)")?;
    let (_, quoted) = rest.split_once('"')?;
    let (name, _) = quoted.split_once('"')?;
    Some(name.to_owned())
}

/// AVFoundation's screen indexes, and its audio devices as (index, name).
fn parse_avfoundation(listing: &str) -> (Vec<String>, Vec<(String, String)>) {
    let mut screens = Vec::new();
    let mut audio = Vec::new();
    let mut in_audio = false;
    for line in listing.lines() {
        if line.contains("AVFoundation video devices") {
            in_audio = false;
        } else if line.contains("AVFoundation audio devices") {
            in_audio = true;
        } else if let Some((index, name)) = avfoundation_device(line) {
            if in_audio {
                audio.push((index, name));
            } else if name.starts_with("Capture screen") {
                screens.push(index);
            }
        }
    }
    (screens, audio)
}

/// `[AVFoundation indev @ 0x1] [2] Capture screen 0` → ("2", "Capture screen 0").
fn avfoundation_device(line: &str) -> Option<(String, String)> {
    let (_, rest) = line.split_once("] [")?;
    let (index, name) = rest.split_once("] ")?;
    index.parse::<u32>().ok()?;
    Some((index.to_owned(), name.trim().to_owned()))
}

fn xrandr_monitors() -> Vec<Monitor> {
    let Ok(out) = process::command(Path::new("xrandr"))
        .arg("--listmonitors")
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_xrandr_monitor)
        .collect()
}

/// One line of `xrandr --listmonitors`: ` 0: +*eDP-1 3072/310x1728/170+0+0  eDP-1`.
fn parse_xrandr_monitor(line: &str) -> Option<Monitor> {
    let mut words = line.split_whitespace();
    words.next()?.strip_suffix(':')?;
    words.next()?;
    let geometry = words.next()?;
    let name = words.next()?.to_owned();
    let (width, rest) = geometry.split_once('x')?;
    let mut rest = rest.split('+');
    let height = rest.next()?;
    let number = |text: &str| text.split('/').next()?.parse::<u32>().ok();
    Some(Monitor {
        name,
        rect: Some(ScreenRect {
            x: rest.next()?.parse().ok()?,
            y: rest.next()?.parse().ok()?,
            width: number(width)?,
            height: number(height)?,
        }),
        device: None,
    })
}

/// Whether the system shows its own dialog to choose what is recorded.
pub fn system_picks_area() -> bool {
    is_wayland()
}

#[cfg(target_os = "linux")]
pub fn is_wayland() -> bool {
    crate::screencast::is_wayland()
}

#[cfg(not(target_os = "linux"))]
pub fn is_wayland() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xrandr_monitors_parse() {
        let m =
            parse_xrandr_monitor(" 1: +HDMI-1 1920/530x1080/300+3072+0  HDMI-1").expect("monitor");
        assert_eq!(m.name, "HDMI-1");
        assert_eq!(
            m.rect,
            Some(ScreenRect {
                x: 3072,
                y: 0,
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(parse_xrandr_monitor("Monitors: 2"), None);
    }

    #[test]
    fn dshow_lists_audio_devices_by_name() {
        let line = r#"[dshow @ 000001] "Microphone (Realtek Audio)" (audio)"#;
        assert_eq!(
            parse_dshow_audio(line).as_deref(),
            Some("Microphone (Realtek Audio)")
        );
        assert_eq!(parse_dshow_audio(r#"[dshow @ 01] "Webcam" (video)"#), None);
    }

    #[test]
    fn loopback_devices_are_not_microphones() {
        let d = split_sound(vec![
            "Stereo Mix (Realtek Audio)".to_owned(),
            "Microphone (USB)".to_owned(),
        ]);
        assert_eq!(d.microphone.as_deref(), Some("Microphone (USB)"));
        assert_eq!(d.loopback.as_deref(), Some("Stereo Mix (Realtek Audio)"));
        assert_eq!(
            d.audio_choices(),
            [
                RecordAudio::None,
                RecordAudio::Microphone,
                RecordAudio::System,
                RecordAudio::Both
            ]
        );
        let mic_only = split_sound(vec!["Microphone".to_owned()]);
        assert_eq!(
            mic_only.audio_choices(),
            [RecordAudio::None, RecordAudio::Microphone]
        );
    }

    #[test]
    fn avfoundation_lists_screens_and_audio() {
        let listing = "\
[AVFoundation indev @ 0x7f] AVFoundation video devices:
[AVFoundation indev @ 0x7f] [0] FaceTime HD Camera
[AVFoundation indev @ 0x7f] [1] Capture screen 0
[AVFoundation indev @ 0x7f] [2] Capture screen 1
[AVFoundation indev @ 0x7f] AVFoundation audio devices:
[AVFoundation indev @ 0x7f] [0] BlackHole 2ch
[AVFoundation indev @ 0x7f] [1] MacBook Pro Microphone
";
        let (screens, audio) = parse_avfoundation(listing);
        assert_eq!(screens, ["1", "2"]);
        assert_eq!(
            audio,
            [
                ("0".to_owned(), "BlackHole 2ch".to_owned()),
                ("1".to_owned(), "MacBook Pro Microphone".to_owned())
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_records_both_sounds_through_pulse() {
        let d = Devices {
            monitors: Vec::new(),
            microphone: Some("default".to_owned()),
            loopback: Some("@DEFAULT_MONITOR@".to_owned()),
        };
        assert_eq!(
            d.sound_inputs(RecordAudio::Both),
            [
                ("pulse", "default".to_owned()),
                ("pulse", "@DEFAULT_MONITOR@".to_owned())
            ]
        );
        assert!(d.sound_inputs(RecordAudio::None).is_empty());
    }
}
