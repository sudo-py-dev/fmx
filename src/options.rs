//! User-tunable conversion options shared by all tools.

use serde::{Deserialize, Serialize};

use crate::catalog::{AudioFormat, ImageFormat};
use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    High,
    Medium,
    Low,
}

impl Quality {
    pub const ALL: [Self; 3] = [Self::High, Self::Medium, Self::Low];

    pub fn label(self) -> &'static str {
        match self {
            Self::High => tr("quality.high"),
            Self::Medium => tr("quality.medium"),
            Self::Low => tr("quality.low"),
        }
    }

    /// Picks the value for this quality level from a (high, medium, low) triple.
    pub fn pick<T>(self, high: T, medium: T, low: T) -> T {
        match self {
            Self::High => high,
            Self::Medium => medium,
            Self::Low => low,
        }
    }
}

/// Output height cap. Videos are never upscaled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    Original,
    P2160,
    P1440,
    P1080,
    P720,
    P480,
    P360,
}

impl Resolution {
    pub const ALL: [Self; 7] = [
        Self::Original,
        Self::P2160,
        Self::P1440,
        Self::P1080,
        Self::P720,
        Self::P480,
        Self::P360,
    ];

    pub fn max_height(self) -> Option<u32> {
        match self {
            Self::Original => None,
            Self::P2160 => Some(2160),
            Self::P1440 => Some(1440),
            Self::P1080 => Some(1080),
            Self::P720 => Some(720),
            Self::P480 => Some(480),
            Self::P360 => Some(360),
        }
    }

    pub fn label(self) -> String {
        self.max_height()
            .map_or_else(|| tr("resolution.original").to_owned(), |h| format!("{h}p"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    Clockwise,
    CounterClockwise,
    UpsideDown,
    FlipHorizontal,
    FlipVertical,
}

impl Rotation {
    pub const ALL: [Self; 5] = [
        Self::Clockwise,
        Self::CounterClockwise,
        Self::UpsideDown,
        Self::FlipHorizontal,
        Self::FlipVertical,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Clockwise => tr("rotation.cw"),
            Self::CounterClockwise => tr("rotation.ccw"),
            Self::UpsideDown => tr("rotation.180"),
            Self::FlipHorizontal => tr("rotation.flip_h"),
            Self::FlipVertical => tr("rotation.flip_v"),
        }
    }

    pub fn video_filter(self) -> &'static str {
        match self {
            Self::Clockwise => "transpose=1",
            Self::CounterClockwise => "transpose=2",
            Self::UpsideDown => "hflip,vflip",
            Self::FlipHorizontal => "hflip",
            Self::FlipVertical => "vflip",
        }
    }
}

pub const MIN_SPEED: f32 = 0.25;
pub const MAX_SPEED: f32 = 4.0;
pub const MIN_TARGET_MB: u32 = 1;
pub const MAX_TARGET_MB: u32 = 100_000;
pub const MIN_FRAME_INTERVAL_S: f32 = 0.1;
pub const MAX_FRAME_INTERVAL_S: f32 = 3600.0;
pub const MIN_IMAGE_SIDE: u32 = 16;
pub const MAX_IMAGE_SIDE: u32 = 16_384;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub quality: Quality,
    pub resolution: Resolution,
    /// Trim window as typed by the user (`ss`, `mm:ss` or `hh:mm:ss[.fff]`).
    pub trim_start: String,
    pub trim_end: String,
    /// Cut on keyframes without re-encoding (instant, slightly imprecise).
    pub stream_copy: bool,
    pub target_size_mb: u32,
    pub frame_interval_s: f32,
    pub frame_format: ImageFormat,
    pub rotation: Rotation,
    pub speed: f32,
    pub join_audio_format: AudioFormat,
    /// Longest image side after resizing; `None` keeps the original size.
    pub image_max_side: Option<u32>,
    pub grayscale: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            quality: Quality::Medium,
            resolution: Resolution::Original,
            trim_start: "0:00".to_owned(),
            trim_end: String::new(),
            stream_copy: true,
            target_size_mb: 25,
            frame_interval_s: 1.0,
            frame_format: ImageFormat::Jpeg,
            rotation: Rotation::Clockwise,
            speed: 1.5,
            join_audio_format: AudioFormat::Mp3,
            image_max_side: None,
            grayscale: false,
        }
    }
}

/// Parses `ss[.f]`, `mm:ss[.f]` or `hh:mm:ss[.f]` into seconds.
/// Empty input yields `None`; malformed input yields `Err(())`.
pub fn parse_timestamp(text: &str) -> Result<Option<f64>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return Err(());
    }
    let (last, rest) = parts.split_last().ok_or(())?;
    let seconds: f64 = last.parse().map_err(|_| ())?;
    let has_minutes = !rest.is_empty();
    if !seconds.is_finite() || seconds < 0.0 || (has_minutes && seconds >= 60.0) {
        return Err(());
    }
    let whole = rest.iter().try_fold(0u64, |acc, part| {
        let value: u64 = part.parse().map_err(|_| ())?;
        Ok::<_, ()>(acc * 60 + value)
    })?;
    if rest.len() == 2 && rest[1].parse::<u64>().map_err(|_| ())? >= 60 {
        return Err(());
    }
    Ok(Some(whole as f64 * 60.0 + seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_parse_in_all_forms() {
        assert_eq!(parse_timestamp(""), Ok(None));
        assert_eq!(parse_timestamp("90"), Ok(Some(90.0)));
        assert_eq!(parse_timestamp("1:30"), Ok(Some(90.0)));
        assert_eq!(parse_timestamp("01:02:03.5"), Ok(Some(3723.5)));
        assert_eq!(parse_timestamp(" 0:05 "), Ok(Some(5.0)));
    }

    #[test]
    fn malformed_timestamps_are_rejected() {
        for bad in ["abc", "1:2:3:4", "1:75", "-3", "1:60:00", "1::2", "nan"] {
            assert_eq!(parse_timestamp(bad), Err(()), "{bad}");
        }
    }

    #[test]
    fn resolution_never_reports_original_height() {
        assert_eq!(Resolution::Original.max_height(), None);
        assert_eq!(Resolution::P720.max_height(), Some(720));
    }
}
