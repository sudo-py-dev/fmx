//! User-tunable conversion options shared by all tools.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::catalog::{AudioFormat, ImageFormat, VideoEncoder, VideoFormat};
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
    P240,
}

impl Resolution {
    pub const ALL: [Self; 8] = [
        Self::Original,
        Self::P2160,
        Self::P1440,
        Self::P1080,
        Self::P720,
        Self::P480,
        Self::P360,
        Self::P240,
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
            Self::P240 => Some(240),
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

/// How a picture is turned and mirrored: any mix of quarter turns and
/// flips comes down to one of these eight. The mirror is applied first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Orientation {
    /// Quarter turns clockwise, 0 to 3.
    pub turns: u8,
    /// Mirrored left to right.
    pub mirror: bool,
}

impl Orientation {
    pub fn is_unchanged(self) -> bool {
        self.turns.is_multiple_of(4) && !self.mirror
    }

    pub fn rotate_cw(self) -> Self {
        Self {
            turns: (self.turns % 4 + 1) % 4,
            ..self
        }
    }

    pub fn rotate_ccw(self) -> Self {
        Self {
            turns: (self.turns % 4 + 3) % 4,
            ..self
        }
    }

    /// Mirrors the picture as it looks now (a flip after a turn reverses it).
    pub fn flip_horizontal(self) -> Self {
        Self {
            turns: (4 - self.turns % 4) % 4,
            mirror: !self.mirror,
        }
    }

    /// A vertical flip is a horizontal one turned half way.
    pub fn flip_vertical(self) -> Self {
        let h = self.flip_horizontal();
        Self {
            turns: (h.turns + 2) % 4,
            ..h
        }
    }

    pub fn swaps_dimensions(self) -> bool {
        self.turns % 2 == 1
    }

    pub fn transformed_size(self, width: u32, height: u32) -> (u32, u32) {
        if self.swaps_dimensions() {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// ffmpeg's filters for it, or `None` when nothing changes.
    pub fn video_filter(self) -> Option<String> {
        let turn = match self.turns % 4 {
            1 => Some("transpose=1"),
            2 => Some("hflip,vflip"),
            3 => Some("transpose=2"),
            _ => None,
        };
        let filters: Vec<&str> = self
            .mirror
            .then_some("hflip")
            .into_iter()
            .chain(turn)
            .collect();
        (!filters.is_empty()).then(|| filters.join(","))
    }

    /// Where a point of the source (0..1 on both axes) ends up.
    pub fn map_point(self, (x, y): (f32, f32)) -> (f32, f32) {
        let mut p = if self.mirror { (1.0 - x, y) } else { (x, y) };
        for _ in 0..self.turns % 4 {
            p = (1.0 - p.1, p.0);
        }
        p
    }

    /// A short description such as "Turned 90° right, mirrored".
    pub fn describe(self) -> String {
        let turn = match self.turns % 4 {
            1 => Some(tr("rotation.turned_right")),
            2 => Some(tr("rotation.turned_180")),
            3 => Some(tr("rotation.turned_left")),
            _ => None,
        };
        match (turn, self.mirror) {
            (None, false) => tr("rotation.unchanged").to_owned(),
            (None, true) => tr("rotation.mirrored").to_owned(),
            (Some(turn), false) => turn.to_owned(),
            (Some(turn), true) => format!("{turn}, {}", tr("rotation.mirrored_lower")),
        }
    }
}

impl From<Rotation> for Orientation {
    fn from(r: Rotation) -> Self {
        let (turns, mirror) = match r {
            Rotation::Clockwise => (1, false),
            Rotation::CounterClockwise => (3, false),
            Rotation::UpsideDown => (2, false),
            Rotation::FlipHorizontal => (0, true),
            Rotation::FlipVertical => (2, true),
        };
        Self { turns, mirror }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HwAccel {
    Off,
    Nvenc,
    Qsv,
    Vaapi,
}

impl HwAccel {
    pub const ALL: [Self; 4] = [Self::Off, Self::Nvenc, Self::Qsv, Self::Vaapi];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => tr("hw.off"),
            Self::Nvenc => "NVIDIA NVENC",
            Self::Qsv => "Intel Quick Sync",
            Self::Vaapi => "VA-API (Intel/AMD)",
        }
    }

    /// ffmpeg's encoder-name suffix for this backend.
    pub fn suffix(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Nvenc => Some("nvenc"),
            Self::Qsv => Some("qsv"),
            Self::Vaapi => Some("vaapi"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channels {
    Original,
    Mono,
    Stereo,
}

impl Channels {
    pub const ALL: [Self; 3] = [Self::Original, Self::Mono, Self::Stereo];

    pub fn label(self) -> &'static str {
        match self {
            Self::Original => tr("channels.original"),
            Self::Mono => tr("channels.mono"),
            Self::Stereo => tr("channels.stereo"),
        }
    }

    pub fn count(self) -> Option<u32> {
        match self {
            Self::Original => None,
            Self::Mono => Some(1),
            Self::Stereo => Some(2),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

impl Corner {
    pub const ALL: [Self; 5] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
        Self::Center,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => tr("corner.top_left"),
            Self::TopRight => tr("corner.top_right"),
            Self::BottomLeft => tr("corner.bottom_left"),
            Self::BottomRight => tr("corner.bottom_right"),
            Self::Center => tr("corner.center"),
        }
    }

    /// `overlay` filter position with a fixed margin.
    pub fn overlay_position(self) -> &'static str {
        match self {
            Self::TopLeft => "16:16",
            Self::TopRight => "W-w-16:16",
            Self::BottomLeft => "16:H-h-16",
            Self::BottomRight => "W-w-16:H-h-16",
            Self::Center => "(W-w)/2:(H-h)/2",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MixLayout {
    SideBySide,
    TopBottom,
    PictureInPicture,
}

impl MixLayout {
    pub const ALL: [Self; 3] = [Self::SideBySide, Self::TopBottom, Self::PictureInPicture];

    pub fn label(self) -> &'static str {
        match self {
            Self::SideBySide => tr("mix.side_by_side"),
            Self::TopBottom => tr("mix.top_bottom"),
            Self::PictureInPicture => tr("mix.pip"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinLayout {
    Vertical,
    Horizontal,
}

impl JoinLayout {
    pub const ALL: [Self; 2] = [Self::Vertical, Self::Horizontal];

    pub fn label(self) -> &'static str {
        match self {
            Self::Vertical => tr("join.vertical"),
            Self::Horizontal => tr("join.horizontal"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadKind {
    Video,
    /// Audio only (saved by older versions as `AudioMp3`).
    #[serde(alias = "AudioMp3")]
    Audio,
}

impl DownloadKind {
    pub const ALL: [Self; 2] = [Self::Video, Self::Audio];

    pub fn label(self) -> &'static str {
        match self {
            Self::Video => tr("download.video"),
            Self::Audio => tr("download.audio"),
        }
    }
}

/// The container a downloaded video is saved in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadContainer {
    Mp4,
    Mkv,
    Webm,
}

impl DownloadContainer {
    pub const ALL: [Self; 3] = [Self::Mp4, Self::Mkv, Self::Webm];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Webm => "webm",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => tr("download.container_mp4"),
            Self::Mkv => tr("download.container_mkv"),
            Self::Webm => tr("download.container_webm"),
        }
    }
}

/// The format of a downloaded soundtrack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadAudio {
    Mp3,
    M4a,
    Opus,
    Flac,
    Wav,
}

impl DownloadAudio {
    pub const ALL: [Self; 5] = [Self::Mp3, Self::M4a, Self::Opus, Self::Flac, Self::Wav];

    /// yt-dlp's name for the format, which is also the file extension.
    pub fn ext(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Opus => "opus",
            Self::Flac => "flac",
            Self::Wav => "wav",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp3 => tr("download.audio_mp3"),
            Self::M4a => tr("download.audio_m4a"),
            Self::Opus => tr("download.audio_opus"),
            Self::Flac => tr("download.audio_flac"),
            Self::Wav => tr("download.audio_wav"),
        }
    }
}

/// What sound a screen recording carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordAudio {
    None,
    Microphone,
    /// What the computer plays (the default output's monitor).
    System,
    /// Microphone and system sound mixed together.
    Both,
}

impl RecordAudio {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => tr("record.audio_none"),
            Self::Microphone => tr("record.audio_mic"),
            Self::System => tr("record.audio_system"),
            Self::Both => tr("record.audio_both"),
        }
    }

    pub fn microphone(self) -> bool {
        matches!(self, Self::Microphone | Self::Both)
    }

    pub fn system(self) -> bool {
        matches!(self, Self::System | Self::Both)
    }
}

/// Which part of the screen is recorded (where the system lets FMX choose).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordArea {
    /// Every screen.
    Screen,
    /// One monitor, by its place in the system's list.
    Monitor(u32),
    /// The rectangle in [`RecordOptions::region`].
    Region,
}

/// A rectangle of the screen in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Size against smoothness of a recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordQuality {
    Small,
    Balanced,
    High,
}

impl RecordQuality {
    pub const ALL: [Self; 3] = [Self::Small, Self::Balanced, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => tr("record.quality_small"),
            Self::Balanced => tr("record.quality_balanced"),
            Self::High => tr("record.quality_high"),
        }
    }

    /// x264's constant rate factor and preset, fast enough to keep up live.
    pub fn x264(self) -> (&'static str, &'static str) {
        match self {
            Self::Small => ("28", "veryfast"),
            Self::Balanced => ("23", "superfast"),
            Self::High => ("18", "ultrafast"),
        }
    }

    /// VP9's constant quality for WebM recordings.
    pub fn vp9_crf(self) -> &'static str {
        match self {
            Self::Small => "40",
            Self::Balanced => "34",
            Self::High => "28",
        }
    }
}

/// The file a screen recording is saved as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordFormat {
    Mkv,
    Mp4,
    Mov,
    Webm,
}

impl RecordFormat {
    pub const ALL: [Self; 4] = [Self::Mp4, Self::Mkv, Self::Webm, Self::Mov];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Mkv => "mkv",
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Webm => "webm",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mkv => "MKV",
            Self::Mp4 => "MP4",
            Self::Mov => "MOV",
            Self::Webm => "WebM",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Mkv => tr("record.format_mkv_hint"),
            Self::Mp4 => tr("download.container_mp4_hint"),
            Self::Mov => tr("record.format_mov_hint"),
            Self::Webm => tr("record.format_webm_hint"),
        }
    }
}

/// The image a screenshot is saved as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShotFormat {
    Png,
    Jpeg,
}

impl ShotFormat {
    pub const ALL: [Self; 2] = [Self::Png, Self::Jpeg];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
        }
    }
}

pub const MAX_SHOT_DELAY_S: u32 = 60;

/// The screenshot tool's choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenshotOptions {
    pub format: ShotFormat,
    pub area: RecordArea,
    pub region: ScreenRect,
    pub cursor: bool,
    /// Seconds to wait before the picture is taken.
    pub delay_s: u32,
    /// Minimize FMX first, so it is not in the picture.
    pub minimize: bool,
}

impl Default for ScreenshotOptions {
    fn default() -> Self {
        Self {
            format: ShotFormat::Png,
            area: RecordArea::Screen,
            region: RecordOptions::default().region,
            cursor: false,
            delay_s: 0,
            minimize: true,
        }
    }
}

pub const MIN_RECORD_SIDE: u32 = 16;
pub const MAX_RECORD_SIDE: u32 = 16_384;

/// The screen recorder's choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordOptions {
    /// Output size limit, independent of the captured screen or region.
    pub resolution: Resolution,
    pub fps: u32,
    pub audio: RecordAudio,
    pub area: RecordArea,
    pub region: ScreenRect,
    pub cursor: bool,
    pub quality: RecordQuality,
    pub format: RecordFormat,
    /// Wait three seconds before recording starts.
    pub countdown: bool,
    /// Minimize FMX, then wait three seconds before recording starts.
    pub minimize: bool,
}

impl Default for RecordOptions {
    fn default() -> Self {
        Self {
            resolution: Resolution::Original,
            fps: 30,
            audio: RecordAudio::Microphone,
            area: RecordArea::Screen,
            region: ScreenRect {
                x: 0,
                y: 0,
                width: 1280,
                height: 720,
            },
            cursor: true,
            quality: RecordQuality::Balanced,
            format: RecordFormat::Mkv,
            countdown: true,
            minimize: false,
        }
    }
}

/// Picture heights offered as the downloader's quality limit.
pub const DOWNLOAD_HEIGHTS: [u32; 7] = [4320, 2160, 1440, 1080, 720, 480, 360];
/// Longest subtitle-language list passed to yt-dlp.
pub const MAX_SUBTITLE_LANGS_LEN: usize = 64;
pub const MAX_RATE_LIMIT_KBPS: u32 = 1_000_000;

/// The video downloader's choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadOptions {
    pub kind: DownloadKind,
    /// Tallest picture to fetch; `None` takes the best available.
    pub max_height: Option<u32>,
    pub container: DownloadContainer,
    pub audio_format: DownloadAudio,
    /// For links to a playlist (or a video in one): fetch every item.
    pub playlist: bool,
    pub subtitles: bool,
    /// Subtitle languages for yt-dlp, e.g. `en` or `en,es`.
    pub subtitle_langs: String,
    /// Store the video's thumbnail as cover art.
    pub embed_thumbnail: bool,
    /// Store title, artist, description and chapters in the file.
    pub embed_metadata: bool,
    /// Download speed limit; `None` is unlimited.
    pub rate_limit_kbps: Option<u32>,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            kind: DownloadKind::Video,
            max_height: None,
            container: DownloadContainer::Mp4,
            audio_format: DownloadAudio::Mp3,
            playlist: false,
            subtitles: false,
            subtitle_langs: "en".to_owned(),
            embed_thumbnail: true,
            embed_metadata: true,
            rate_limit_kbps: None,
        }
    }
}

/// A pixel rectangle inside the video frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Default for Rect {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            w: 320,
            h: 180,
        }
    }
}

/// Advanced video encoder settings ("Output settings").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    pub encoder: VideoEncoder,
    pub hw: HwAccel,
    /// Fixed bitrate; `None` uses constant quality from [`Quality`].
    pub bitrate_kbps: Option<u32>,
    pub fps: Option<u32>,
    /// Decode on the GPU when ffmpeg can (`-hwaccel auto`).
    pub hw_decode: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            encoder: VideoEncoder::Auto,
            hw: HwAccel::Off,
            bitrate_kbps: None,
            fps: None,
            hw_decode: false,
        }
    }
}

/// Advanced audio settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Channels,
    pub volume_db: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            bitrate_kbps: None,
            sample_rate: None,
            channels: Channels::Original,
            volume_db: 0.0,
        }
    }
}

/// Per-task edits ("Clip / Option"): reset after each batch is queued.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    /// Clip window as typed by the user (`ss`, `mm:ss` or `hh:mm:ss[.fff]`).
    pub trim_start: String,
    pub trim_end: String,
    pub crop: Option<Rect>,
    pub rotation: Option<Rotation>,
    pub subtitle: Option<PathBuf>,
    pub watermark: Option<PathBuf>,
    pub watermark_corner: Option<Corner>,
}

impl Edit {
    pub fn has_trim(&self) -> bool {
        !self.trim_start.trim().is_empty() || !self.trim_end.trim().is_empty()
    }

    pub fn validate(&self, duration: Option<f64>) -> Result<(), crate::plan::PlanError> {
        crate::plan::validate_edit(self, duration)
    }
}

pub const MIN_SPEED: f32 = 0.25;
pub const MAX_SPEED: f32 = 4.0;
pub const MIN_TARGET_MB: u32 = 1;
pub const MAX_TARGET_MB: u32 = 100_000;
/// Which frames Extract Frames saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameMode {
    /// One every `frame_interval_s` seconds.
    Interval,
    /// Every frame of the video.
    All,
    /// Only at the moments in `frame_times`.
    Times,
}

impl FrameMode {
    pub const ALL: [Self; 3] = [Self::Interval, Self::All, Self::Times];

    pub fn label(self) -> &'static str {
        match self {
            Self::Interval => tr("frames.mode_interval"),
            Self::All => tr("frames.mode_all"),
            Self::Times => tr("frames.mode_times"),
        }
    }
}

/// Most moments one Extract Frames job saves.
pub const MAX_FRAME_TIMES: usize = 200;

/// Adds `secs` to sorted, distinct moments; a moment within a millisecond
/// of one already there counts as the same.
pub fn add_frame_time(times: &mut Vec<f64>, secs: f64) {
    if !secs.is_finite() || secs < 0.0 || times.len() >= MAX_FRAME_TIMES {
        return;
    }
    let secs = (secs * 1000.0).round() / 1000.0;
    if times.iter().any(|t| (t - secs).abs() < 0.001) {
        return;
    }
    let at = times.partition_point(|t| *t < secs);
    times.insert(at, secs);
}

/// Parses moments typed as `10, 14.5, 1:05` (commas, semicolons or spaces).
pub fn parse_frame_times(text: &str) -> Result<Vec<f64>, String> {
    let mut times = Vec::new();
    for word in text
        .split([',', ';', ' ', '\n'])
        .filter(|w| !w.trim().is_empty())
    {
        match parse_timestamp(word) {
            Ok(Some(t)) => add_frame_time(&mut times, t),
            _ => return Err(word.trim().to_owned()),
        }
    }
    Ok(times)
}

pub const MIN_FRAME_INTERVAL_S: f32 = 0.1;
pub const MAX_FRAME_INTERVAL_S: f32 = 3600.0;
pub const MIN_IMAGE_SIDE: u32 = 16;
pub const MAX_IMAGE_SIDE: u32 = 16_384;
pub const MAX_VIDEO_KBPS: u32 = 200_000;
pub const MAX_AUDIO_KBPS: u32 = 640;
pub const MAX_VOLUME_DB: f32 = 30.0;
pub const MIN_SPLIT_SECS: f32 = 1.0;
pub const MAX_SPLIT_PARTS: u32 = 999;
pub const MIN_PDF_DPI: u32 = 36;
pub const MAX_PDF_DPI: u32 = 1200;
pub const MAX_CSO_LEVEL: u32 = 9;
pub const MAX_RECORD_FPS: u32 = 120;
pub const SAMPLE_RATES: [u32; 6] = [8_000, 22_050, 32_000, 44_100, 48_000, 96_000];
pub const FRAME_RATES: [u32; 8] = [10, 15, 24, 25, 30, 50, 60, 120];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub quality: Quality,
    pub resolution: Resolution,
    pub video: VideoSettings,
    pub audio: AudioSettings,
    pub edit: Edit,
    /// Cut on keyframes without re-encoding (instant, slightly imprecise).
    pub stream_copy: bool,
    pub target_size_mb: u32,
    pub frame_interval_s: f32,
    pub frame_format: ImageFormat,
    pub frame_mode: FrameMode,
    /// Moments, in seconds, for [`FrameMode::Times`].
    pub frame_times: Vec<f64>,
    /// The Rotate tool's turn and flip.
    pub orientation: Orientation,
    pub speed: f32,
    pub join_audio_format: AudioFormat,
    /// Longest image side after resizing; `None` keeps the original size.
    pub image_max_side: Option<u32>,
    pub grayscale: bool,
    pub image_rotation: Option<Rotation>,
    pub split_by_parts: bool,
    pub split_secs: f32,
    pub split_parts: u32,
    pub delogo: Rect,
    pub mix_layout: MixLayout,
    pub pdf_dpi: u32,
    pub pdf_image_format: ImageFormat,
    /// DVD title number; 0 lets ffmpeg pick the main title.
    pub dvd_title: u32,
    pub disc_video_format: VideoFormat,
    pub cd_audio_format: AudioFormat,
    pub iso_label: String,
    pub cso_level: u32,
    pub record: RecordOptions,
    pub sound_record: RecordOptions,
    pub sound_format: AudioFormat,
    pub screenshot: ScreenshotOptions,
    pub download: DownloadOptions,
    pub join_layout: JoinLayout,
    /// Text appended to every output file name, e.g. `_converted`.
    pub name_tag: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            quality: Quality::Medium,
            resolution: Resolution::Original,
            video: VideoSettings::default(),
            audio: AudioSettings::default(),
            edit: Edit::default(),
            stream_copy: true,
            target_size_mb: 25,
            frame_interval_s: 1.0,
            frame_format: ImageFormat::Jpeg,
            frame_mode: FrameMode::Interval,
            frame_times: Vec::new(),
            orientation: Orientation::default(),
            speed: 1.5,
            join_audio_format: AudioFormat::Mp3,
            image_max_side: None,
            grayscale: false,
            image_rotation: None,
            split_by_parts: false,
            split_secs: 60.0,
            split_parts: 2,
            delogo: Rect::default(),
            mix_layout: MixLayout::SideBySide,
            pdf_dpi: 150,
            pdf_image_format: ImageFormat::Png,
            dvd_title: 0,
            disc_video_format: VideoFormat::Mp4,
            cd_audio_format: AudioFormat::Mp3,
            iso_label: "DISC".to_owned(),
            cso_level: 9,
            record: RecordOptions::default(),
            sound_record: RecordOptions::default(),
            sound_format: AudioFormat::Mp3,
            screenshot: ScreenshotOptions::default(),
            download: DownloadOptions::default(),
            join_layout: JoinLayout::Vertical,
            name_tag: String::new(),
        }
    }
}

impl Options {
    pub fn validate(&self, tool: crate::catalog::Tool) -> Result<(), crate::plan::PlanError> {
        crate::plan::validate_options(self, tool)
    }
}

/// Characters that may not appear in a file name on any common system.
const FORBIDDEN_NAME_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
pub const MAX_NAME_TAG_LEN: usize = 40;

/// Makes a user-typed file-name tag safe: no separators, control or reserved characters.
pub fn sanitize_name_tag(tag: &str) -> String {
    tag.chars()
        .filter(|c| !c.is_control() && !FORBIDDEN_NAME_CHARS.contains(c))
        .take(MAX_NAME_TAG_LEN)
        .collect::<String>()
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_owned()
}

/// A short exact time to type or edit, such as `3:41.6`, `0:10` or
/// `1:02:05.25`: milliseconds kept, trailing zeros dropped.
pub fn compact_timestamp(secs: f64) -> String {
    let millis = (secs.max(0.0) * 1000.0).round() as u64;
    let (s, ms) = (millis / 1000, millis % 1000);
    let clock = if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    };
    if ms == 0 {
        clock
    } else {
        let fraction = format!("{ms:03}");
        format!("{clock}.{}", fraction.trim_end_matches('0'))
    }
}

/// A short, readable time such as `1:05.5` or `1:02:05.0`, for display only.
pub fn display_time(secs: f64) -> String {
    let tenths = (secs.max(0.0) * 10.0).round() as u64;
    let (s, t) = (tenths / 10, tenths % 10);
    if s >= 3600 {
        format!("{}:{:02}:{:02}.{t}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}.{t}", s / 60, s % 60)
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
    fn frame_times_stay_sorted_and_distinct() {
        let mut times = Vec::new();
        for t in [14.0, 10.0, 14.0004, 12.5, -1.0, f64::NAN] {
            add_frame_time(&mut times, t);
        }
        assert_eq!(times, [10.0, 12.5, 14.0]);
        assert_eq!(
            parse_frame_times("10, 14.5; 1:05  2"),
            Ok(vec![2.0, 10.0, 14.5, 65.0])
        );
        assert_eq!(parse_frame_times("10, x"), Err("x".to_owned()));
    }

    #[test]
    fn compact_timestamps_round_trip() {
        assert_eq!(compact_timestamp(221.6), "3:41.6");
        assert_eq!(compact_timestamp(10.0), "0:10");
        assert_eq!(compact_timestamp(3725.25), "1:02:05.25");
        for secs in [0.0, 0.001, 59.999, 221.6, 3725.25] {
            assert_eq!(parse_timestamp(&compact_timestamp(secs)), Ok(Some(secs)));
        }
    }

    #[test]
    fn display_time_is_short_and_readable() {
        assert_eq!(display_time(0.0), "0:00.0");
        assert_eq!(display_time(65.46), "1:05.5");
        assert_eq!(display_time(3725.0), "1:02:05.0");
    }

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
    fn timestamps_round_trip() {
        for secs in [0.0, 5.25, 65.5, 3723.125] {
            assert_eq!(parse_timestamp(&compact_timestamp(secs)), Ok(Some(secs)));
        }
    }

    #[test]
    fn name_tags_are_sanitized() {
        assert_eq!(sanitize_name_tag("_hd"), "_hd");
        assert_eq!(sanitize_name_tag("../etc/x"), "etcx");
        assert_eq!(sanitize_name_tag("a:b*c?"), "abc");
        assert_eq!(sanitize_name_tag(&"x".repeat(100)).len(), MAX_NAME_TAG_LEN);
    }

    #[test]
    fn resolution_never_reports_original_height() {
        assert_eq!(Resolution::Original.max_height(), None);
        assert_eq!(Resolution::P720.max_height(), Some(720));
    }

    #[test]
    fn old_settings_files_still_load() {
        let opts: Options =
            serde_json::from_str(r#"{"quality":"High","trim_start":"1"}"#).expect("parse");
        assert_eq!(opts.quality, Quality::High);
        assert_eq!(opts.video, VideoSettings::default());
    }

    #[test]
    fn saved_orientation_values_never_overflow_and_match_export() {
        for turns in 0..=u8::MAX {
            for mirror in [false, true] {
                let orientation = Orientation { turns, mirror };
                let normal = Orientation {
                    turns: turns % 4,
                    mirror,
                };
                assert_eq!(orientation.rotate_cw(), normal.rotate_cw());
                assert_eq!(orientation.rotate_ccw(), normal.rotate_ccw());
                assert_eq!(
                    orientation.is_unchanged(),
                    orientation.video_filter().is_none()
                );
            }
        }
    }

    #[test]
    fn orientation_turns_and_flips() {
        let right = Orientation::from(Rotation::Clockwise);
        assert!(right.swaps_dimensions());
        assert!(!Orientation::from(Rotation::FlipVertical).swaps_dimensions());
        assert_eq!(right.transformed_size(1920, 1080), (1080, 1920));

        // Four turns, or two flips, come back to the start.
        let o = Orientation::default();
        assert!(
            o.rotate_cw()
                .rotate_cw()
                .rotate_cw()
                .rotate_cw()
                .is_unchanged()
        );
        assert!(o.flip_vertical().flip_vertical().is_unchanged());
        assert_eq!(
            o.rotate_ccw(),
            Orientation {
                turns: 3,
                mirror: false
            }
        );
        // Turning right then flipping looks like flipping then turning left.
        assert_eq!(
            o.rotate_cw().flip_horizontal(),
            o.flip_horizontal().rotate_ccw()
        );
        // A vertical flip is the old upside-down mirror.
        assert_eq!(o.flip_vertical(), Rotation::FlipVertical.into());
        assert_eq!(
            Orientation::from(Rotation::Clockwise)
                .video_filter()
                .as_deref(),
            Some("transpose=1")
        );
        assert_eq!(
            o.flip_horizontal().rotate_cw().video_filter().as_deref(),
            Some("hflip,transpose=1")
        );
        assert_eq!(o.video_filter(), None);
        // The top-left corner goes top-right after a turn right.
        assert_eq!(o.rotate_cw().map_point((0.0, 0.0)), (1.0, 0.0));
        assert_eq!(o.flip_horizontal().map_point((0.0, 0.0)), (1.0, 0.0));
    }

    #[test]
    fn edit_validation_catches_bad_and_empty_timestamps() {
        use crate::plan::PlanError;

        let mut edit = Edit::default();
        assert!(edit.validate(None).is_ok());

        edit.trim_start = "vmm".into();
        assert_eq!(
            edit.validate(None),
            Err(PlanError::BadTimestamp("vmm".into()))
        );

        edit.trim_start = "10".into();
        edit.trim_end = "5".into();
        assert_eq!(edit.validate(None), Err(PlanError::EmptyTrimWindow));

        edit.trim_end = "20".into();
        assert!(edit.validate(None).is_ok());

        // Stop after duration
        assert_eq!(edit.validate(Some(5.0)), Err(PlanError::EmptyTrimWindow));
    }

    #[test]
    fn options_validation_catches_invalid_tool_inputs() {
        use crate::catalog::Tool;
        use crate::plan::PlanError;

        let mut opts = Options::default();
        opts.edit.trim_start = "vmm".into();
        assert_eq!(
            opts.validate(Tool::Trim),
            Err(PlanError::BadTimestamp("vmm".into()))
        );

        opts.edit.trim_start.clear();
        assert_eq!(opts.validate(Tool::Crop), Err(PlanError::NoCropArea));

        opts.edit.crop = Some(Rect {
            x: 0,
            y: 0,
            w: 1,
            h: 100,
        });
        assert_eq!(opts.validate(Tool::Crop), Err(PlanError::InvalidRect));

        opts.edit.crop = Some(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 100,
        });
        assert!(opts.validate(Tool::Crop).is_ok());

        opts.speed = 0.0;
        assert!(matches!(
            opts.validate(Tool::Speed),
            Err(PlanError::InvalidValue(_))
        ));

        opts.speed = 1.0;
        opts.split_by_parts = true;
        opts.split_parts = 1;
        assert!(matches!(
            opts.validate(Tool::Split),
            Err(PlanError::InvalidValue(_))
        ));
    }
}
