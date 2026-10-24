//! Pure translation of (tool, inputs, options) into ffmpeg arguments.
//! No I/O happens here, so every recipe is unit-testable.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::catalog::{AudioFormat, Device, Tool, VideoEncoder, VideoFormat};
use crate::i18n::{tr, trf};
use crate::options::{
    self, Corner, Edit, HwAccel, MixLayout, Options, Quality, Rect, parse_timestamp,
};
use crate::probe::MediaInfo;
use crate::process::StopMode;

/// One input together with what probing found out about it.
#[derive(Clone, Debug)]
pub struct Input {
    pub path: PathBuf,
    pub info: MediaInfo,
}

/// A ready-to-run ffmpeg invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub args: Vec<OsString>,
    /// Length of the output, used to turn ffmpeg's clock into a percentage.
    pub expected_secs: Option<f64>,
    pub stop: StopMode,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlanError {
    WrongInputCount { needed: usize, got: usize },
    NoVideoStream,
    NoAudioStream,
    BadTimestamp(String),
    EmptyTrimWindow,
    UnknownDuration,
    TargetTooSmall,
    InvalidRect,
    NoCropArea,
    NoTracks,
    NoSubtitleFile,
    NoSubtitleStream,
    UnsupportedTool,
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongInputCount { needed, got } => f.write_str(&trf(
                "error.wrong_input_count",
                &[("needed", needed), ("got", got)],
            )),
            Self::NoVideoStream => f.write_str(tr("error.no_video")),
            Self::NoAudioStream => f.write_str(tr("error.no_audio")),
            Self::BadTimestamp(t) => f.write_str(&trf("error.bad_timestamp", &[("value", t)])),
            Self::EmptyTrimWindow => f.write_str(tr("error.empty_trim")),
            Self::UnknownDuration => f.write_str(tr("error.unknown_duration")),
            Self::TargetTooSmall => f.write_str(tr("error.target_too_small")),
            Self::InvalidRect => f.write_str(tr("error.invalid_rect")),
            Self::NoTracks => f.write_str(tr("error.no_tracks")),
            Self::NoCropArea => f.write_str(tr("error.no_crop")),
            Self::NoSubtitleFile => f.write_str(tr("error.no_subtitle_file")),
            Self::NoSubtitleStream => f.write_str(tr("error.no_subtitle_stream")),
            Self::UnsupportedTool => f.write_str(tr("error.unsupported_tool")),
        }
    }
}

impl std::error::Error for PlanError {}

/// Bits per megabyte for target-size math (MiB, matching file managers).
const BITS_PER_MB: f64 = 8.0 * 1024.0 * 1024.0;
/// Fraction of the target size left for the container after muxing overhead.
const CONTAINER_HEADROOM: f64 = 0.96;
const COMPRESS_AUDIO_BPS: f64 = 128_000.0;
const MIN_VIDEO_BPS: f64 = 64_000.0;
const JOIN_FPS: u32 = 30;
const JOIN_SAMPLE_RATE: u32 = 48_000;
const FALLBACK_SIZE: (u32, u32) = (1280, 720);
/// Default height for animated GIF/WebP when no resolution is chosen.
const ANIMATION_HEIGHT: u32 = 480;
/// iPhone ringtones may not exceed 40 seconds.
const RINGTONE_MAX_SECS: f64 = 40.0;
const VAAPI_DEVICE: &str = "/dev/dri/renderD128";

/// Demuxers that take a device or disc path rather than a file URL.
pub const DVD_DEMUXER: &str = "dvdvideo";
pub const CD_DEMUXER: &str = "libcdio";
/// Blu-ray is read through ffmpeg's `bluray:` protocol rather than a demuxer.
pub const BLURAY_PROTOCOL: &str = "bluray";

/// `file:` stops ffmpeg from treating names like `-x.mp4` or `http:x` specially.
/// Disc/device demuxers read the raw path.
pub fn input_url(path: &Path, demuxer: Option<&str>) -> OsString {
    if demuxer == Some(BLURAY_PROTOCOL) {
        let mut url = OsString::from("bluray:");
        url.push(path.as_os_str());
        return url;
    }
    if demuxer.is_some() {
        return path.as_os_str().to_owned();
    }
    let mut url = OsString::from("file:");
    url.push(path.as_os_str());
    url
}

/// Small builder so recipes read as a sequence of flags.
#[derive(Default)]
struct Args(Vec<OsString>);

impl Args {
    fn flag(mut self, flag: &str, value: impl Into<OsString>) -> Self {
        self.0.push(flag.into());
        self.0.push(value.into());
        self
    }

    fn switch(mut self, flag: &str) -> Self {
        self.0.push(flag.into());
        self
    }

    fn extend(mut self, items: Vec<OsString>) -> Self {
        self.0.extend(items);
        self
    }

    fn input(self, path: &Path) -> Self {
        self.flag("-i", input_url(path, None))
    }

    fn output(mut self, path: &Path) -> PendingPlan {
        self.0.push(input_url(path, None));
        PendingPlan(self.0)
    }
}

/// Arguments with the output attached, awaiting the expected duration.
struct PendingPlan(Vec<OsString>);

impl PendingPlan {
    fn expect(self, secs: Option<f64>) -> Plan {
        Plan {
            args: self.0,
            expected_secs: secs,
            stop: StopMode::Kill,
        }
    }
}

/// Flags shared by every run: quiet logs, machine-readable progress on stdout.
fn base() -> Args {
    Args::default()
        .switch("-hide_banner")
        .switch("-nostdin")
        .switch("-y")
        .flag("-loglevel", "error")
        .flag("-progress", "pipe:1")
        .switch("-nostats")
}

fn strings(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// Escapes a value for an ffmpeg filter option, then for the filtergraph
/// (the two quoting levels described in the ffmpeg-filters manual).
pub fn escape_filter_value(value: &str) -> String {
    let escape = |text: &str, special: &[char]| {
        text.chars().fold(String::new(), |mut out, c| {
            if special.contains(&c) {
                out.push('\\');
            }
            out.push(c);
            out
        })
    };
    let option_level = escape(value, &['\\', '\'', ':']);
    escape(&option_level, &['\\', '\'', '[', ']', ',', ';'])
}

/// Scale filter capping height without upscaling, keeping width even.
fn scale_filter(max_height: u32) -> String {
    format!("scale=-2:'min({max_height},ih)'")
}

fn crop_filter(rect: Rect) -> String {
    format!("crop={}:{}:{}:{}", rect.w & !1, rect.h & !1, rect.x, rect.y)
}

fn check_rect(rect: Rect, info: &MediaInfo) -> Result<(), PlanError> {
    let fits = |pos: u32, len: u32, limit: Option<u32>| {
        len >= 2 && limit.is_none_or(|limit| pos.saturating_add(len) <= limit)
    };
    if fits(rect.x, rect.w, info.width) && fits(rect.y, rect.h, info.height) {
        Ok(())
    } else {
        Err(PlanError::InvalidRect)
    }
}

/// The concrete video codec for a container and the user's choice.
fn effective_encoder(format: VideoFormat, chosen: VideoEncoder) -> VideoEncoder {
    if chosen != VideoEncoder::Auto && format.encoder_choices().contains(&chosen) {
        chosen
    } else {
        format.default_encoder()
    }
}

fn hardware_encoder(encoder: VideoEncoder, hw: HwAccel) -> Option<String> {
    let family = match encoder {
        VideoEncoder::H264 => "h264",
        VideoEncoder::H265 => "hevc",
        VideoEncoder::Av1 => "av1",
        _ => return None,
    };
    hw.suffix().map(|suffix| format!("{family}_{suffix}"))
}

fn uses_vaapi(format: VideoFormat, opts: &Options) -> bool {
    opts.video.hw == HwAccel::Vaapi
        && effective_encoder(format, opts.video.encoder).supports_hardware()
}

fn mp4_family(format: VideoFormat) -> bool {
    matches!(
        format,
        VideoFormat::Mp4 | VideoFormat::Mp4Hevc | VideoFormat::Mov | VideoFormat::ThreeGp
    )
}

/// Video encoder flags for a container format and the output settings.
fn video_codec(format: VideoFormat, opts: &Options) -> Vec<OsString> {
    let q = opts.quality;
    let settings = &opts.video;
    let rate = settings
        .bitrate_kbps
        .map(|k| format!("{}k", k.clamp(1, options::MAX_VIDEO_KBPS)));
    let encoder = effective_encoder(format, settings.encoder);
    let rate_or = |quality_flags: &[&str]| match &rate {
        Some(r) => strings(&["-b:v", r]),
        None => strings(quality_flags),
    };

    let mut out = if let Some(name) = hardware_encoder(encoder, settings.hw) {
        let mut v = strings(&["-c:v", &name]);
        v.extend(match settings.hw {
            HwAccel::Nvenc => rate_or(&[
                "-preset",
                "p4",
                "-rc",
                "vbr",
                "-cq",
                q.pick("19", "25", "31"),
                "-b:v",
                "0",
            ]),
            HwAccel::Qsv => rate_or(&["-global_quality", q.pick("20", "25", "30")]),
            HwAccel::Vaapi => rate_or(&["-rc_mode", "CQP", "-qp", q.pick("20", "25", "30")]),
            HwAccel::Off => Vec::new(),
        });
        v
    } else {
        software_video_codec(format, encoder, q, &rate_or)
    };
    if encoder == VideoEncoder::H265 && mp4_family(format) {
        out.extend(strings(&["-tag:v", "hvc1"]));
    }
    if let Some(fps) = settings.fps {
        out.extend(strings(&["-r", &fps.to_string()]));
    }
    out
}

fn software_video_codec(
    format: VideoFormat,
    encoder: VideoEncoder,
    q: Quality,
    rate_or: &dyn Fn(&[&str]) -> Vec<OsString>,
) -> Vec<OsString> {
    let qscale = q.pick("2", "5", "9");
    let mut out = match encoder {
        VideoEncoder::H264 => {
            let mut v = strings(&["-c:v", "libx264", "-preset", "faster"]);
            if format == VideoFormat::ThreeGp {
                v.extend(strings(&["-profile:v", "baseline"]));
            }
            v.extend(rate_or(&["-crf", q.pick("18", "23", "28")]));
            v
        }
        VideoEncoder::H265 => {
            let mut v = strings(&["-c:v", "libx265", "-preset", "fast"]);
            v.extend(rate_or(&["-crf", q.pick("22", "27", "31")]));
            v
        }
        VideoEncoder::Av1 => {
            let mut v = strings(&["-c:v", "libsvtav1", "-preset", "8"]);
            v.extend(rate_or(&["-crf", q.pick("28", "35", "42")]));
            v
        }
        VideoEncoder::Vp9 => {
            let mut v = strings(&[
                "-c:v",
                "libvpx-vp9",
                "-deadline",
                "good",
                "-cpu-used",
                "4",
                "-row-mt",
                "1",
            ]);
            v.extend(rate_or(&["-b:v", "0", "-crf", q.pick("24", "32", "40")]));
            v
        }
        VideoEncoder::Auto | VideoEncoder::Legacy => {
            let (codec, quality): (&str, &[&str]) = match format {
                VideoFormat::Avi => ("mpeg4", &["-q:v", qscale]),
                VideoFormat::Wmv => ("wmv2", &["-q:v", qscale]),
                VideoFormat::Mpg | VideoFormat::Vob => ("mpeg2video", &["-q:v", qscale]),
                VideoFormat::Ogv => ("libtheora", &["-q:v", q.pick("9", "7", "5")]),
                VideoFormat::Swf => ("flv", &["-q:v", qscale]),
                VideoFormat::Webp => (
                    "libwebp_anim",
                    &[
                        "-lossless",
                        "0",
                        "-q:v",
                        q.pick("85", "75", "60"),
                        "-loop",
                        "0",
                    ],
                ),
                VideoFormat::Gif => ("gif", &[]),
                _ => ("libx264", &["-crf", "23"]),
            };
            let mut v = strings(&["-c:v", codec]);
            v.extend(if quality.is_empty() {
                Vec::new()
            } else {
                rate_or(quality)
            });
            return v;
        }
    };
    out.extend(strings(&["-pix_fmt", "yuv420p"]));
    out
}

/// Audio bitrate: the user's override or the quality default.
fn audio_rate(opts: &Options, high: &str, medium: &str, low: &str) -> String {
    opts.audio.bitrate_kbps.map_or_else(
        || opts.quality.pick(high, medium, low).to_owned(),
        |k| format!("{}k", k.clamp(8, options::MAX_AUDIO_KBPS)),
    )
}

/// Sample-rate, channel, and volume flags from the advanced audio settings.
fn audio_tail(opts: &Options) -> Vec<OsString> {
    let mut out = Vec::new();
    if let Some(rate) = opts.audio.sample_rate {
        out.extend(strings(&["-ar", &rate.to_string()]));
    }
    if let Some(n) = opts.audio.channels.count() {
        out.extend(strings(&["-ac", &n.to_string()]));
    }
    let gain = opts
        .audio
        .volume_db
        .clamp(-options::MAX_VOLUME_DB, options::MAX_VOLUME_DB);
    if gain.abs() >= 0.1 {
        out.extend(strings(&["-af", &format!("volume={gain:.1}dB")]));
    }
    out
}

/// Audio encoder flags that suit a video container.
fn audio_codec_for_video(format: VideoFormat, opts: &Options) -> Vec<OsString> {
    let br = audio_rate(opts, "192k", "160k", "128k");
    let mut out = match format {
        VideoFormat::Gif | VideoFormat::Webp => return strings(&["-an"]),
        VideoFormat::Webm => strings(&[
            "-c:a",
            "libopus",
            "-b:a",
            &audio_rate(opts, "160k", "128k", "96k"),
        ]),
        VideoFormat::Avi => strings(&["-c:a", "libmp3lame", "-b:a", &br]),
        VideoFormat::Swf => strings(&["-c:a", "libmp3lame", "-b:a", &br, "-ar", "44100"]),
        VideoFormat::Wmv => strings(&["-c:a", "wmav2", "-b:a", &br]),
        VideoFormat::Mpg => strings(&[
            "-c:a",
            "mp2",
            "-b:a",
            &audio_rate(opts, "256k", "192k", "160k"),
        ]),
        VideoFormat::Vob => strings(&[
            "-c:a",
            "ac3",
            "-b:a",
            &audio_rate(opts, "384k", "256k", "192k"),
        ]),
        VideoFormat::Ogv => strings(&[
            "-c:a",
            "libvorbis",
            "-q:a",
            opts.quality.pick("7", "5", "3"),
        ]),
        VideoFormat::ThreeGp => strings(&[
            "-c:a",
            "aac",
            "-b:a",
            &audio_rate(opts, "96k", "96k", "64k"),
            "-ac",
            "1",
        ]),
        _ => strings(&["-c:a", "aac", "-b:a", &br]),
    };
    out.extend(audio_tail(opts));
    out
}

/// Encoder flags for an audio-only output.
fn audio_codec(format: AudioFormat, opts: &Options) -> Vec<OsString> {
    let br = audio_rate(opts, "256k", "192k", "128k");
    let mut out = match format {
        AudioFormat::Mp3 => strings(&[
            "-c:a",
            "libmp3lame",
            "-b:a",
            &audio_rate(opts, "320k", "192k", "128k"),
        ]),
        AudioFormat::Aac | AudioFormat::M4a | AudioFormat::Mka => {
            strings(&["-c:a", "aac", "-b:a", &br])
        }
        AudioFormat::M4r => strings(&["-c:a", "aac", "-b:a", &br, "-f", "ipod"]),
        AudioFormat::Flac => strings(&["-c:a", "flac"]),
        // APE is encoded from this WAV by Monkey's Audio afterwards.
        AudioFormat::Wav | AudioFormat::Ape => strings(&["-c:a", "pcm_s16le"]),
        AudioFormat::Aiff => strings(&["-c:a", "pcm_s16be"]),
        AudioFormat::Wv => strings(&["-c:a", "wavpack"]),
        AudioFormat::Ogg => strings(&[
            "-c:a",
            "libvorbis",
            "-q:a",
            opts.quality.pick("8", "6", "4"),
        ]),
        AudioFormat::Opus => strings(&[
            "-c:a",
            "libopus",
            "-b:a",
            &audio_rate(opts, "160k", "128k", "96k"),
        ]),
        AudioFormat::Wma => strings(&["-c:a", "wmav2", "-b:a", &br]),
        AudioFormat::Ac3 => strings(&[
            "-c:a",
            "ac3",
            "-b:a",
            &audio_rate(opts, "448k", "384k", "256k"),
        ]),
        AudioFormat::Mp2 => strings(&["-c:a", "mp2", "-b:a", &br]),
        AudioFormat::Mmf => strings(&["-c:a", "adpcm_yamaha", "-ar", "22050", "-ac", "1"]),
        AudioFormat::Amr => strings(&[
            "-c:a",
            "libopencore_amrnb",
            "-ar",
            "8000",
            "-ac",
            "1",
            "-b:a",
            "12.2k",
        ]),
    };
    out.extend(audio_tail(opts));
    out
}

/// The video filter graph of a conversion: a linear chain, an optional
/// watermark overlay, and a format-specific tail.
struct VideoGraph {
    chain: Vec<String>,
    watermark: Option<(usize, Corner)>,
    tail: Option<String>,
}

impl VideoGraph {
    fn new(
        format: VideoFormat,
        opts: &Options,
        edit: &Edit,
        extra: &[String],
        watermark_input: Option<usize>,
    ) -> Self {
        let mut chain: Vec<String> = edit.crop.map(crop_filter).into_iter().collect();
        chain.extend(edit.rotation.map(|r| r.video_filter().to_owned()));
        chain.extend(extra.iter().cloned());
        if let Some(subs) = &edit.subtitle {
            chain.push(format!(
                "subtitles=filename={}",
                escape_filter_value(&subs.to_string_lossy())
            ));
        }
        let height = match (opts.resolution.max_height(), format.is_animation()) {
            (Some(h), _) => Some(h),
            (None, true) => Some(ANIMATION_HEIGHT),
            (None, false) => None,
        };
        chain.extend(height.map(scale_filter));
        let fps = opts.video.fps.unwrap_or(opts.quality.pick(15, 12, 10));
        let tail = match format {
            VideoFormat::Gif => Some(format!(
                "fps={fps},split[pa][pb];[pa]palettegen=stats_mode=diff[pp];[pb][pp]paletteuse=dither=bayer"
            )),
            VideoFormat::Webp => Some(format!("fps={fps}")),
            _ if uses_vaapi(format, opts) => Some("format=nv12,hwupload".to_owned()),
            _ => None,
        };
        let watermark =
            watermark_input.map(|i| (i, edit.watermark_corner.unwrap_or(Corner::BottomRight)));
        Self {
            chain,
            watermark,
            tail,
        }
    }

    fn args(self) -> Vec<OsString> {
        let Some((index, corner)) = self.watermark else {
            let all: Vec<String> = self.chain.into_iter().chain(self.tail).collect();
            return if all.is_empty() {
                Vec::new()
            } else {
                strings(&["-vf", &all.join(",")])
            };
        };
        let pre = if self.chain.is_empty() {
            "null".to_owned()
        } else {
            self.chain.join(",")
        };
        let mut graph = format!(
            "[0:v:0]{pre}[base];[{index}:v]format=rgba[wm];[base][wm]overlay={}",
            corner.overlay_position()
        );
        if let Some(tail) = self.tail {
            graph.push_str(&format!("[ov];[ov]{tail}"));
        }
        graph.push_str("[v]");
        strings(&["-filter_complex", &graph, "-map", "[v]", "-map", "0:a:0?"])
    }
}

/// Full encode flags (filters + video + audio + container tweaks).
fn video_encode(
    format: VideoFormat,
    opts: &Options,
    edit: &Edit,
    extra: &[String],
    watermark_input: Option<usize>,
) -> Vec<OsString> {
    let mut out = VideoGraph::new(format, opts, edit, extra, watermark_input).args();
    out.extend(video_codec(format, opts));
    out.extend(audio_codec_for_video(format, opts));
    if mp4_family(format) {
        out.extend(strings(&["-movflags", "+faststart"]));
    }
    out
}

fn require_count(inputs: &[Input], needed: usize, exact: bool) -> Result<(), PlanError> {
    let got = inputs.len();
    if got < needed || (exact && got != needed) {
        return Err(PlanError::WrongInputCount { needed, got });
    }
    Ok(())
}

fn single(inputs: &[Input]) -> Result<&Input, PlanError> {
    require_count(inputs, 1, true)?;
    Ok(&inputs[0])
}

fn with_video(input: &Input) -> Result<&Input, PlanError> {
    if input.info.has_video {
        Ok(input)
    } else {
        Err(PlanError::NoVideoStream)
    }
}

fn with_audio(input: &Input) -> Result<&Input, PlanError> {
    if input.info.has_audio {
        Ok(input)
    } else {
        Err(PlanError::NoAudioStream)
    }
}

/// The container to re-encode into when an operation keeps the input's format.
pub fn same_format(path: &Path) -> VideoFormat {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(VideoFormat::from_ext)
        .filter(|f| !f.is_animation())
        .unwrap_or(VideoFormat::Mp4)
}

/// A container extension ffmpeg can mux for this input, used by copy-only tools.
pub fn copy_container_ext(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let audio = AudioFormat::ALL.into_iter().map(AudioFormat::ext);
    let video = VideoFormat::ALL.into_iter().map(VideoFormat::ext);
    audio
        .chain(video)
        .find(|known| *known == ext)
        .unwrap_or("mkv")
}

/// Splits a speed factor into `atempo` stages, each within ffmpeg's 0.5–2.0 range.
pub fn atempo_chain(speed: f64) -> String {
    let mut remaining = speed;
    let mut stages = Vec::new();
    while remaining > 2.0 {
        stages.push(2.0);
        remaining /= 2.0;
    }
    while remaining < 0.5 {
        stages.push(0.5);
        remaining /= 0.5;
    }
    stages.push(remaining);
    stages
        .iter()
        .map(|s| format!("atempo={s}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Video bitrate (bits/s) needed to hit a file size, if achievable.
pub fn target_video_bps(target_mb: u32, duration_s: f64, has_audio: bool) -> Option<f64> {
    if duration_s <= 0.0 {
        return None;
    }
    let total_bps = f64::from(target_mb) * BITS_PER_MB * CONTAINER_HEADROOM / duration_s;
    let audio = if has_audio { COMPRESS_AUDIO_BPS } else { 0.0 };
    let video = total_bps - audio;
    (video >= MIN_VIDEO_BPS).then_some(video)
}

/// Parses a clip window; `end` is `None` when it runs to the end of the file.
fn trim_window(edit: &Edit, duration: Option<f64>) -> Result<(f64, Option<f64>), PlanError> {
    let parse = |text: &str| {
        parse_timestamp(text).map_err(|()| PlanError::BadTimestamp(text.trim().to_owned()))
    };
    let start = parse(&edit.trim_start)?.unwrap_or(0.0);
    let end = parse(&edit.trim_end)?;
    let stop = end.or(duration);
    if stop.is_some_and(|stop| stop <= start) {
        return Err(PlanError::EmptyTrimWindow);
    }
    Ok((start, end))
}

/// Adds the main input (with clip seeking) and any watermark input.
/// Returns the args, the expected output length, and the watermark's input index.
fn conversion_inputs(
    input: &Input,
    edit: &Edit,
    opts: &Options,
) -> Result<(Args, Option<f64>, Option<usize>), PlanError> {
    let mut args = base();
    if opts.video.hw_decode && input.info.has_video {
        args = args.flag("-hwaccel", "auto");
    }
    let mut expected = input.info.duration_s;
    if edit.has_trim() {
        let (start, end) = trim_window(edit, input.info.duration_s)?;
        args = args.flag("-ss", format!("{start:.3}"));
        if let Some(end) = end {
            args = args.flag("-to", format!("{end:.3}"));
        }
        expected = end.or(input.info.duration_s).map(|stop| stop - start);
    }
    args = args.input(&input.path);
    let watermark = match &edit.watermark {
        Some(wm) => {
            args = args.input(wm);
            Some(1)
        }
        None => None,
    };
    Ok((args, expected, watermark))
}

/// Builds the ffmpeg invocation for `tool`. `output` is the final file path
/// (or, for tools that write many files, the destination directory).
pub fn build(
    tool: Tool,
    inputs: &[Input],
    output: &Path,
    opts: &Options,
) -> Result<Plan, PlanError> {
    let none = Edit::default();
    match tool {
        Tool::ToVideo(format) => {
            let input = with_video(single(inputs)?)?;
            let edit = &opts.edit;
            if let Some(rect) = edit.crop {
                check_rect(rect, &input.info)?;
            }
            let (mut args, expected, wm) = conversion_inputs(input, edit, opts)?;
            if uses_vaapi(format, opts) {
                args = args.flag("-vaapi_device", VAAPI_DEVICE);
            }
            Ok(args
                .extend(video_encode(format, opts, edit, &[], wm))
                .output(output)
                .expect(expected))
        }
        Tool::ToAudio(format) => {
            let input = with_audio(single(inputs)?)?;
            let edit = Edit {
                watermark: None,
                ..opts.edit.clone()
            };
            let (mut args, mut expected, _) = conversion_inputs(input, &edit, opts)?;
            args = args.switch("-vn").extend(audio_codec(format, opts));
            if format == AudioFormat::M4r && expected.is_none_or(|d| d > RINGTONE_MAX_SECS) {
                args = args.flag("-t", RINGTONE_MAX_SECS.to_string());
                expected = Some(expected.map_or(RINGTONE_MAX_SECS, |d| d.min(RINGTONE_MAX_SECS)));
            }
            Ok(args.output(output).expect(expected))
        }
        Tool::Trim => trim(single(inputs)?, output, opts),
        Tool::RemoveAudio => {
            let input = with_video(single(inputs)?)?;
            Ok(base()
                .input(&input.path)
                .flag("-map", "0")
                .flag("-map", "-0:a")
                .flag("-c", "copy")
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::Mux => {
            require_count(inputs, 2, true)?;
            let (video, audio) = (with_video(&inputs[0])?, with_audio(&inputs[1])?);
            // Matroska accepts any codec pair, so both streams are copied untouched.
            Ok(base()
                .input(&video.path)
                .input(&audio.path)
                .flag("-map", "0:v:0")
                .flag("-map", "1:a:0")
                .flag("-c", "copy")
                .switch("-shortest")
                .output(output)
                .expect(shortest(inputs)))
        }
        Tool::Compress => compress(with_video(single(inputs)?)?, output, opts),
        Tool::ExtractFrames => {
            let input = with_video(single(inputs)?)?;
            let interval = opts
                .frame_interval_s
                .clamp(options::MIN_FRAME_INTERVAL_S, options::MAX_FRAME_INTERVAL_S);
            let pattern = output.join(format!("frame_%05d.{}", opts.frame_format.ext()));
            Ok(base()
                .input(&input.path)
                .flag("-vf", format!("fps=1/{interval}"))
                .flag("-q:v", "2")
                .output(&pattern)
                .expect(input.info.duration_s))
        }
        Tool::Rotate => {
            let input = with_video(single(inputs)?)?;
            let filter = opts.rotation.video_filter().to_owned();
            Ok(base()
                .input(&input.path)
                .extend(video_encode(
                    same_format(&input.path),
                    opts,
                    &none,
                    &[filter],
                    None,
                ))
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::Delogo => {
            let input = with_video(single(inputs)?)?;
            let r = opts.delogo;
            check_rect(r, &input.info)?;
            // delogo needs a 1-pixel border of real picture around the band.
            let (x, y) = (r.x.max(1), r.y.max(1));
            let w =
                r.w.min(input.info.width.map_or(r.w, |fw| fw.saturating_sub(x + 1)));
            let h =
                r.h.min(input.info.height.map_or(r.h, |fh| fh.saturating_sub(y + 1)));
            let filter = format!("delogo=x={x}:y={y}:w={w}:h={h}");
            Ok(base()
                .input(&input.path)
                .extend(video_encode(
                    same_format(&input.path),
                    opts,
                    &none,
                    &[filter],
                    None,
                ))
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::Speed => speed(single(inputs)?, output, opts),
        Tool::Normalize => {
            let input = with_audio(single(inputs)?)?;
            // Video is copied; ffmpeg picks the container's default audio encoder.
            Ok(base()
                .input(&input.path)
                .flag("-af", "loudnorm=I=-16:TP=-1.5:LRA=11")
                .flag("-c:v", "copy")
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::Volume => {
            let input = with_audio(single(inputs)?)?;
            let gain = opts
                .audio
                .volume_db
                .clamp(-options::MAX_VOLUME_DB, options::MAX_VOLUME_DB);
            Ok(base()
                .input(&input.path)
                .flag("-af", format!("volume={gain:.1}dB"))
                .flag("-c:v", "copy")
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::Repair => {
            let input = single(inputs)?;
            Ok(base()
                .flag("-fflags", "+genpts+discardcorrupt")
                .flag("-err_detect", "ignore_err")
                .input(&input.path)
                .flag("-map", "0")
                .flag("-c", "copy")
                .switch("-ignore_unknown")
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::Split => split(single(inputs)?, output, opts),
        Tool::JoinVideo => join_video(inputs, output, opts),
        Tool::JoinAudio => join_audio(inputs, output, opts),
        Tool::AudioMix => audio_mix(inputs, output, opts),
        Tool::VideoMix => video_mix(inputs, output, opts),
        Tool::DvdToVideo => dvd(single(inputs)?, output, opts),
        Tool::BluRayToVideo => {
            let input = single(inputs)?;
            let format = opts.disc_video_format;
            Ok(base()
                .flag("-i", input_url(&input.path, Some(BLURAY_PROTOCOL)))
                .flag("-map", "0:v:0")
                .flag("-map", "0:a:0?")
                .extend(video_encode(format, opts, &Edit::default(), &[], None))
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::ToDevice(device) => to_device(device, with_video(single(inputs)?)?, output, opts),
        Tool::Crop => {
            let input = with_video(single(inputs)?)?;
            let rect = opts.edit.crop.ok_or(PlanError::NoCropArea)?;
            check_rect(rect, &input.info)?;
            Ok(base()
                .input(&input.path)
                .extend(video_encode(
                    same_format(&input.path),
                    opts,
                    &none,
                    &[crop_filter(rect)],
                    None,
                ))
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::AddSubtitles => {
            let input = with_video(single(inputs)?)?;
            let subs = opts
                .edit
                .subtitle
                .as_ref()
                .ok_or(PlanError::NoSubtitleFile)?;
            // Matroska stores SRT, ASS and WebVTT tracks as they are.
            Ok(base()
                .input(&input.path)
                .input(subs)
                .flag("-map", "0")
                .flag("-map", "1:0")
                .flag("-c", "copy")
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::ExtractSubtitles => {
            let input = single(inputs)?;
            if !input.info.has_subtitles {
                return Err(PlanError::NoSubtitleStream);
            }
            Ok(base()
                .input(&input.path)
                .flag("-map", "0:s:0")
                .flag("-c:s", "srt")
                .output(output)
                .expect(input.info.duration_s))
        }
        _ => Err(PlanError::UnsupportedTool),
    }
}

fn shortest(inputs: &[Input]) -> Option<f64> {
    inputs
        .iter()
        .filter_map(|i| i.info.duration_s)
        .reduce(f64::min)
}

fn longest(inputs: &[Input]) -> Option<f64> {
    inputs
        .iter()
        .filter_map(|i| i.info.duration_s)
        .reduce(f64::max)
}

fn total_duration(inputs: &[Input]) -> Option<f64> {
    inputs.iter().map(|i| i.info.duration_s).sum()
}

fn trim(input: &Input, output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    let (start, end) = trim_window(&opts.edit, input.info.duration_s)?;
    let mut args = base().flag("-ss", format!("{start:.3}"));
    if let Some(end) = end {
        args = args.flag("-to", format!("{end:.3}"));
    }
    args = args.input(&input.path);
    args = if opts.stream_copy {
        args.flag("-map", "0")
            .flag("-c", "copy")
            .flag("-avoid_negative_ts", "make_zero")
    } else if input.info.has_video {
        args.extend(video_encode(
            same_format(&input.path),
            opts,
            &Edit::default(),
            &[],
            None,
        ))
    } else {
        args
    };
    let expected = end.or(input.info.duration_s).map(|stop| stop - start);
    Ok(args.output(output).expect(expected))
}

fn compress(input: &Input, output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    let duration = input.info.duration_s.ok_or(PlanError::UnknownDuration)?;
    let target = opts
        .target_size_mb
        .clamp(options::MIN_TARGET_MB, options::MAX_TARGET_MB);
    let bps = target_video_bps(target, duration, input.info.has_audio)
        .ok_or(PlanError::TargetTooSmall)?;
    let mut args = base().input(&input.path);
    if let Some(h) = opts.resolution.max_height() {
        args = args.flag("-vf", scale_filter(h));
    }
    let rate = |factor: f64| format!("{}", (bps * factor) as u64);
    Ok(args
        .extend(strings(&[
            "-c:v",
            "libx264",
            "-preset",
            "faster",
            "-pix_fmt",
            "yuv420p",
            "-b:v",
            &rate(1.0),
            "-maxrate",
            &rate(1.3),
            "-bufsize",
            &rate(2.0),
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-movflags",
            "+faststart",
        ]))
        .output(output)
        .expect(Some(duration)))
}

fn speed(input: &Input, output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    let factor = f64::from(opts.speed.clamp(options::MIN_SPEED, options::MAX_SPEED));
    let mut args = base().input(&input.path);
    if input.info.has_audio {
        args = args.flag("-filter:a", atempo_chain(factor));
    }
    if input.info.has_video {
        let setpts = format!("setpts=PTS/{factor}");
        // The atempo filter above is the only audio filter allowed on this stream.
        let mut no_gain = opts.clone();
        no_gain.audio.volume_db = 0.0;
        args = args.extend(video_encode(
            same_format(&input.path),
            &no_gain,
            &Edit::default(),
            &[setpts],
            None,
        ));
    }
    let expected = input.info.duration_s.map(|d| d / factor);
    Ok(args.output(output).expect(expected))
}

fn split(input: &Input, output_dir: &Path, opts: &Options) -> Result<Plan, PlanError> {
    let duration = input.info.duration_s.ok_or(PlanError::UnknownDuration)?;
    let segment = if opts.split_by_parts {
        let parts = opts.split_parts.clamp(2, options::MAX_SPLIT_PARTS);
        duration / f64::from(parts)
    } else {
        f64::from(opts.split_secs)
    }
    .max(f64::from(options::MIN_SPLIT_SECS));
    let pattern = output_dir.join(format!("part_%03d.{}", copy_container_ext(&input.path)));
    Ok(base()
        .input(&input.path)
        .flag("-map", "0")
        .flag("-c", "copy")
        .flag("-f", "segment")
        .flag("-segment_time", format!("{segment:.3}"))
        .flag("-reset_timestamps", "1")
        .output(&pattern)
        .expect(Some(duration)))
}

fn join_video(inputs: &[Input], output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    require_count(inputs, 2, false)?;
    inputs.iter().try_for_each(|i| with_video(i).map(drop))?;
    let with_audio = inputs.iter().all(|i| i.info.has_audio);
    let first = &inputs[0].info;
    let (src_w, src_h) = match (first.width, first.height) {
        (Some(w), Some(h)) => (w, h),
        _ => FALLBACK_SIZE,
    };
    let height = opts
        .resolution
        .max_height()
        .map_or(src_h, |cap| cap.min(src_h));
    let width = (u64::from(src_w) * u64::from(height) / u64::from(src_h.max(1))) as u32;
    let (w, h) = (width & !1, height & !1);

    let mut graph = String::new();
    let mut pads = String::new();
    for i in 0..inputs.len() {
        graph.push_str(&format!(
            "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={JOIN_FPS},format=yuv420p[v{i}];"
        ));
        pads.push_str(&format!("[v{i}]"));
        if with_audio {
            graph.push_str(&format!(
                "[{i}:a:0]aresample={JOIN_SAMPLE_RATE},aformat=channel_layouts=stereo[a{i}];"
            ));
            pads.push_str(&format!("[a{i}]"));
        }
    }
    let n = inputs.len();
    let a = u8::from(with_audio);
    graph.push_str(&format!("{pads}concat=n={n}:v=1:a={a}[v]"));
    if with_audio {
        graph.push_str("[a]");
    }

    let mut args = inputs.iter().fold(base(), |args, i| args.input(&i.path));
    args = args.flag("-filter_complex", graph).flag("-map", "[v]");
    if with_audio {
        args = args.flag("-map", "[a]");
    }
    let format = VideoFormat::Mp4;
    Ok(args
        .extend(video_codec(format, opts))
        .extend(if with_audio {
            audio_codec_for_video(format, opts)
        } else {
            strings(&["-an"])
        })
        .extend(strings(&["-movflags", "+faststart"]))
        .output(output)
        .expect(total_duration(inputs)))
}

fn normalized_audio_inputs(inputs: &[Input]) -> Result<String, PlanError> {
    inputs.iter().try_for_each(|i| with_audio(i).map(drop))?;
    Ok((0..inputs.len())
        .map(|i| {
            format!("[{i}:a:0]aresample={JOIN_SAMPLE_RATE},aformat=channel_layouts=stereo[a{i}];")
        })
        .collect())
}

fn join_audio(inputs: &[Input], output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    require_count(inputs, 2, false)?;
    let mut graph = normalized_audio_inputs(inputs)?;
    let pads: String = (0..inputs.len()).map(|i| format!("[a{i}]")).collect();
    graph.push_str(&format!("{pads}concat=n={}:v=0:a=1[a]", inputs.len()));
    Ok(inputs
        .iter()
        .fold(base(), |args, i| args.input(&i.path))
        .flag("-filter_complex", graph)
        .flag("-map", "[a]")
        .extend(audio_codec(opts.join_audio_format, opts))
        .output(output)
        .expect(total_duration(inputs)))
}

fn audio_mix(inputs: &[Input], output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    require_count(inputs, 2, false)?;
    let mut graph = normalized_audio_inputs(inputs)?;
    let pads: String = (0..inputs.len()).map(|i| format!("[a{i}]")).collect();
    graph.push_str(&format!(
        "{pads}amix=inputs={}:duration=longest:normalize=0[a]",
        inputs.len()
    ));
    Ok(inputs
        .iter()
        .fold(base(), |args, i| args.input(&i.path))
        .flag("-filter_complex", graph)
        .flag("-map", "[a]")
        .extend(audio_codec(opts.join_audio_format, opts))
        .output(output)
        .expect(longest(inputs)))
}

fn video_mix(inputs: &[Input], output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    require_count(inputs, 2, true)?;
    let (a, b) = (with_video(&inputs[0])?, with_video(&inputs[1])?);
    let dim = |pick: fn(&MediaInfo) -> Option<u32>, fallback: u32| {
        let smallest = pick(&a.info)
            .unwrap_or(fallback)
            .min(pick(&b.info).unwrap_or(fallback));
        opts.resolution
            .max_height()
            .map_or(smallest, |cap| cap.min(smallest))
            & !1
    };
    let graph = match opts.mix_layout {
        MixLayout::SideBySide => {
            let h = dim(|i| i.height, FALLBACK_SIZE.1);
            format!(
                "[0:v]scale=-2:{h},setsar=1[l];[1:v]scale=-2:{h},setsar=1[r];[l][r]hstack=inputs=2:shortest=1[v]"
            )
        }
        MixLayout::TopBottom => {
            let w = dim(|i| i.width, FALLBACK_SIZE.0);
            format!(
                "[0:v]scale={w}:-2,setsar=1[t];[1:v]scale={w}:-2,setsar=1[b];[t][b]vstack=inputs=2:shortest=1[v]"
            )
        }
        MixLayout::PictureInPicture => {
            "[1:v]scale=trunc(iw/8)*2:-2[s];[0:v][s]overlay=W-w-16:H-h-16:shortest=1[v]".to_owned()
        }
    };
    let format = VideoFormat::Mp4;
    Ok(base()
        .input(&a.path)
        .input(&b.path)
        .flag("-filter_complex", graph)
        .flag("-map", "[v]")
        .flag("-map", "0:a:0?")
        .extend(video_codec(format, opts))
        .extend(audio_codec_for_video(format, opts))
        .extend(strings(&["-movflags", "+faststart"]))
        .output(output)
        .expect(shortest(inputs)))
}

/// libdvdnav wants the disc root, not its `VIDEO_TS` folder.
pub fn dvd_root(path: &Path) -> PathBuf {
    let is_video_ts = path
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("VIDEO_TS"));
    match path.parent() {
        Some(parent) if is_video_ts => parent.to_path_buf(),
        _ => path.to_path_buf(),
    }
}

/// Scales into a box without upscaling, keeping both sides even.
fn fit_filter(max_width: u32, max_height: u32) -> String {
    format!(
        "scale='min({max_width},iw)':'min({max_height},ih)':force_original_aspect_ratio=decrease,\
         scale=trunc(iw/2)*2:trunc(ih/2)*2"
    )
}

/// MP4 tuned for a device: size box, H.264 profile/level, bitrate and frame-rate caps.
fn to_device(
    device: Device,
    input: &Input,
    output: &Path,
    opts: &Options,
) -> Result<Plan, PlanError> {
    let profile = device.profile();
    let edit = &opts.edit;
    if let Some(rect) = edit.crop {
        check_rect(rect, &input.info)?;
    }
    let (args, expected, wm) = conversion_inputs(input, edit, opts)?;
    // The device box replaces the resolution choice.
    let mut boxed = opts.clone();
    boxed.resolution = options::Resolution::Original;
    let filters = VideoGraph::new(
        VideoFormat::Mp4,
        &boxed,
        edit,
        &[fit_filter(profile.max_width, profile.max_height)],
        wm,
    )
    .args();
    let kbps = profile.max_kbps;
    let mut codec = strings(&[
        "-c:v",
        "libx264",
        "-preset",
        "faster",
        "-profile:v",
        profile.profile,
        "-level",
        profile.level,
        "-crf",
        opts.quality.pick("19", "23", "27"),
        "-maxrate",
        &format!("{kbps}k"),
        "-bufsize",
        &format!("{}k", kbps * 2),
        "-pix_fmt",
        "yuv420p",
    ]);
    if let (Some(cap), Some(fps)) = (profile.max_fps, input.info.fps)
        && fps > f64::from(cap) + 0.5
    {
        codec.extend(strings(&["-r", &cap.to_string()]));
    }
    let audio = format!("{}k", profile.audio_kbps);
    Ok(args
        .extend(filters)
        .extend(codec)
        .extend(strings(&[
            "-c:a",
            "aac",
            "-b:a",
            &audio,
            "-ac",
            "2",
            "-movflags",
            "+faststart",
        ]))
        .output(output)
        .expect(expected))
}

fn dvd(input: &Input, output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    let mut args = base().flag("-f", DVD_DEMUXER);
    if opts.dvd_title > 0 {
        args = args.flag("-title", opts.dvd_title.to_string());
    }
    let format = opts.disc_video_format;
    Ok(args
        .flag("-i", input_url(&dvd_root(&input.path), Some(DVD_DEMUXER)))
        .flag("-map", "0:v:0")
        .flag("-map", "0:a:0?")
        .extend(video_encode(format, opts, &Edit::default(), &[], None))
        .output(output)
        .expect(input.info.duration_s))
}

/// One audio-CD track as a standalone file.
pub fn cd_track(device: &Path, track: (f64, f64), output: &Path, opts: &Options) -> Plan {
    let (start, end) = track;
    base()
        .flag("-f", CD_DEMUXER)
        .flag("-ss", format!("{start:.3}"))
        .flag("-to", format!("{end:.3}"))
        .flag("-i", input_url(device, Some(CD_DEMUXER)))
        .switch("-vn")
        .extend(audio_codec(opts.cd_audio_format, opts))
        .output(output)
        .expect(Some(end - start))
}

/// Screen capture until stopped. `display` is the X11 display on Linux.
pub fn record(output: &Path, opts: &Options, display: &str) -> Plan {
    let fps = opts
        .record_fps
        .clamp(1, options::MAX_RECORD_FPS)
        .to_string();
    let mut args = base();
    // Stopping sends `q` on stdin, so ffmpeg must keep reading it.
    args.0.retain(|a| a != "-nostdin");
    args = args.flag("-thread_queue_size", "1024");
    args = if cfg!(windows) {
        args.flag("-f", "gdigrab")
            .flag("-framerate", fps.as_str())
            .flag("-i", "desktop")
    } else if cfg!(target_os = "macos") {
        let device = if opts.record_audio { "1:0" } else { "1:none" };
        args.flag("-f", "avfoundation")
            .flag("-framerate", fps.as_str())
            .flag("-capture_cursor", "1")
            .flag("-i", device)
    } else {
        args.flag("-f", "x11grab")
            .flag("-framerate", fps.as_str())
            .flag("-i", display)
    };
    let pulse_audio = opts.record_audio && !cfg!(windows) && !cfg!(target_os = "macos");
    if pulse_audio {
        args = args
            .flag("-thread_queue_size", "1024")
            .flag("-f", "pulse")
            .flag("-i", "default");
    }
    args = args.extend(strings(&[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "23",
        "-pix_fmt",
        "yuv420p",
    ]));
    args = if opts.record_audio && !cfg!(windows) {
        args.extend(strings(&["-c:a", "aac", "-b:a", "160k"]))
    } else {
        args.switch("-an")
    };
    Plan {
        stop: StopMode::Quit,
        ..args.output(output).expect(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Resolution;

    fn media(video: bool, audio: bool, secs: f64) -> MediaInfo {
        MediaInfo {
            duration_s: Some(secs),
            has_video: video,
            has_audio: audio,
            width: video.then_some(1920),
            height: video.then_some(1080),
            chapters: Vec::new(),
            has_subtitles: false,
            fps: Some(60.0),
        }
    }

    fn input(name: &str, info: MediaInfo) -> Input {
        Input {
            path: PathBuf::from(format!("/in/{name}")),
            info,
        }
    }

    fn joined(plan: &Plan) -> String {
        plan.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn plan(tool: Tool, inputs: &[Input], opts: &Options) -> String {
        joined(&build(tool, inputs, Path::new("/out/o"), opts).expect("plan"))
    }

    #[test]
    fn mp4_conversion_uses_x264_aac_faststart() {
        let inputs = [input("a.avi", media(true, true, 10.0))];
        let p = build(
            Tool::ToVideo(VideoFormat::Mp4),
            &inputs,
            Path::new("/out/a.mp4"),
            &Options::default(),
        )
        .expect("plan");
        let cmd = joined(&p);
        assert!(cmd.contains("-i file:/in/a.avi"));
        assert!(cmd.contains("-c:v libx264") && cmd.contains("-c:a aac"));
        assert!(cmd.contains("+faststart"));
        assert!(cmd.ends_with("file:/out/a.mp4"));
        assert_eq!(p.expected_secs, Some(10.0));
    }

    #[test]
    fn resolution_adds_non_upscaling_scale() {
        let opts = Options {
            resolution: Resolution::P720,
            ..Options::default()
        };
        let cmd = plan(
            Tool::ToVideo(VideoFormat::Webm),
            &[input("a.mkv", media(true, true, 1.0))],
            &opts,
        );
        assert!(cmd.contains("scale=-2:'min(720,ih)'"));
    }

    #[test]
    fn encoder_choice_and_hardware_are_honoured() {
        let mut opts = Options::default();
        opts.video.encoder = VideoEncoder::H265;
        opts.video.hw = HwAccel::Nvenc;
        let cmd = plan(
            Tool::ToVideo(VideoFormat::Mp4),
            &[input("a.mkv", media(true, true, 1.0))],
            &opts,
        );
        assert!(
            cmd.contains("-c:v hevc_nvenc") && cmd.contains("-tag:v hvc1"),
            "{cmd}"
        );

        opts.video.hw = HwAccel::Vaapi;
        let cmd = plan(
            Tool::ToVideo(VideoFormat::Mp4),
            &[input("a.mkv", media(true, true, 1.0))],
            &opts,
        );
        assert!(cmd.contains("-vaapi_device /dev/dri/renderD128"));
        assert!(cmd.contains("format=nv12,hwupload"));

        // AVI cannot hold H.265; the choice falls back to the container default.
        let cmd = plan(
            Tool::ToVideo(VideoFormat::Avi),
            &[input("a.mkv", media(true, true, 1.0))],
            &opts,
        );
        assert!(cmd.contains("-c:v mpeg4"));
    }

    #[test]
    fn bitrate_fps_and_audio_settings_apply() {
        let mut opts = Options::default();
        opts.video.bitrate_kbps = Some(2500);
        opts.video.fps = Some(25);
        opts.audio.bitrate_kbps = Some(96);
        opts.audio.sample_rate = Some(44_100);
        opts.audio.channels = crate::options::Channels::Mono;
        opts.audio.volume_db = 3.0;
        let cmd = plan(
            Tool::ToVideo(VideoFormat::Mkv),
            &[input("a.mp4", media(true, true, 1.0))],
            &opts,
        );
        for part in [
            "-b:v 2500k",
            "-r 25",
            "-b:a 96k",
            "-ar 44100",
            "-ac 1",
            "-af volume=3.0dB",
        ] {
            assert!(cmd.contains(part), "missing {part}: {cmd}");
        }
        assert!(!cmd.contains("-crf"));
    }

    #[test]
    fn edits_build_filter_chain_and_clip() {
        let edit = Edit {
            trim_start: "5".into(),
            trim_end: "15".into(),
            crop: Some(Rect {
                x: 10,
                y: 20,
                w: 641,
                h: 360,
            }),
            rotation: Some(crate::options::Rotation::Clockwise),
            subtitle: Some(PathBuf::from("/s/it's: a,b.srt")),
            ..Edit::default()
        };
        let opts = Options {
            edit,
            ..Options::default()
        };
        let p = build(
            Tool::ToVideo(VideoFormat::Mp4),
            &[input("a.mp4", media(true, true, 60.0))],
            Path::new("/o.mp4"),
            &opts,
        )
        .expect("plan");
        let cmd = joined(&p);
        assert!(cmd.contains("-ss 5.000 -to 15.000 -i"));
        assert!(
            cmd.contains(
                "-vf crop=640:360:10:20,transpose=1,subtitles=filename=/s/it\\\\\\'s\\\\: a\\,b.srt"
            ),
            "{cmd}"
        );
        assert_eq!(p.expected_secs, Some(10.0));
    }

    #[test]
    fn crop_outside_frame_is_rejected() {
        let mut opts = Options::default();
        opts.edit.crop = Some(Rect {
            x: 1900,
            y: 0,
            w: 100,
            h: 100,
        });
        let err = build(
            Tool::ToVideo(VideoFormat::Mp4),
            &[input("a.mp4", media(true, true, 1.0))],
            Path::new("/o"),
            &opts,
        );
        assert_eq!(err, Err(PlanError::InvalidRect));
    }

    #[test]
    fn watermark_uses_overlay_graph() {
        let mut opts = Options::default();
        opts.edit.watermark = Some(PathBuf::from("/logo.png"));
        opts.edit.watermark_corner = Some(Corner::TopRight);
        let cmd = plan(
            Tool::ToVideo(VideoFormat::Gif),
            &[input("a.mp4", media(true, true, 1.0))],
            &opts,
        );
        assert!(cmd.contains("-i file:/logo.png"));
        assert!(
            cmd.contains("[base][wm]overlay=W-w-16:16[ov];[ov]fps=12,split"),
            "{cmd}"
        );
        assert!(cmd.contains("-map [v]") && cmd.contains("-an"));
    }

    #[test]
    fn audio_only_input_is_rejected_for_video() {
        let err = build(
            Tool::ToVideo(VideoFormat::Mp4),
            &[input("a.mp3", media(false, true, 1.0))],
            Path::new("/o.mp4"),
            &Options::default(),
        );
        assert_eq!(err, Err(PlanError::NoVideoStream));
    }

    #[test]
    fn audio_conversion_drops_video_and_caps_ringtones() {
        let cmd = plan(
            Tool::ToAudio(AudioFormat::Mp3),
            &[input("a.mp4", media(true, true, 5.0))],
            &Options::default(),
        );
        assert!(cmd.contains("-vn") && cmd.contains("libmp3lame"));
        let p = build(
            Tool::ToAudio(AudioFormat::M4r),
            &[input("a.mp3", media(false, true, 200.0))],
            Path::new("/o.m4r"),
            &Options::default(),
        )
        .expect("plan");
        assert!(joined(&p).contains("-f ipod") && joined(&p).contains("-t 40"));
        assert_eq!(p.expected_secs, Some(40.0));
    }

    #[test]
    fn trim_copy_uses_input_seeking() {
        let mut opts = Options::default();
        opts.edit.trim_start = "0:10".into();
        opts.edit.trim_end = "0:25".into();
        let p = build(
            Tool::Trim,
            &[input("a.mp4", media(true, true, 60.0))],
            Path::new("/o.mp4"),
            &opts,
        )
        .expect("plan");
        let cmd = joined(&p);
        assert!(cmd.find("-ss 10.000").expect("ss") < cmd.find("-i ").expect("i"));
        assert!(cmd.contains("-c copy"));
        assert_eq!(p.expected_secs, Some(15.0));
    }

    #[test]
    fn trim_rejects_bad_and_empty_windows() {
        let inputs = [input("a.mp4", media(true, true, 60.0))];
        let with = |s: &str, e: &str| {
            let mut o = Options::default();
            o.edit.trim_start = s.into();
            o.edit.trim_end = e.into();
            build(Tool::Trim, &inputs, Path::new("/o"), &o)
        };
        assert!(matches!(with("x", ""), Err(PlanError::BadTimestamp(_))));
        assert_eq!(with("30", "10"), Err(PlanError::EmptyTrimWindow));
        assert_eq!(with("90", ""), Err(PlanError::EmptyTrimWindow));
    }

    #[test]
    fn atempo_stages_stay_in_range() {
        assert_eq!(atempo_chain(1.5), "atempo=1.5");
        assert_eq!(atempo_chain(4.0), "atempo=2,atempo=2");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.5");
    }

    #[test]
    fn target_bitrate_math() {
        let bps = target_video_bps(10, 80.0, true).expect("feasible");
        assert!((bps - 878_778.0).abs() < 1_000.0, "{bps}");
        assert_eq!(target_video_bps(1, 3600.0, true), None);
        assert_eq!(target_video_bps(10, 0.0, false), None);
    }

    #[test]
    fn join_video_builds_concat_graph() {
        let inputs = [
            input("a.mp4", media(true, true, 3.0)),
            input("b.mov", media(true, true, 4.0)),
        ];
        let p = build(
            Tool::JoinVideo,
            &inputs,
            Path::new("/j.mp4"),
            &Options::default(),
        )
        .expect("plan");
        let cmd = joined(&p);
        assert!(cmd.contains("[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"));
        assert!(cmd.contains("scale=1920:1080"));
        assert_eq!(p.expected_secs, Some(7.0));
    }

    #[test]
    fn join_video_without_audio_everywhere_drops_audio() {
        let inputs = [
            input("a.mp4", media(true, false, 3.0)),
            input("b.mp4", media(true, true, 4.0)),
        ];
        let cmd = plan(Tool::JoinVideo, &inputs, &Options::default());
        assert!(cmd.contains("concat=n=2:v=1:a=0[v]") && cmd.contains("-an"));
    }

    #[test]
    fn join_requires_two_inputs() {
        assert_eq!(
            build(
                Tool::JoinAudio,
                &[input("a.mp3", media(false, true, 3.0))],
                Path::new("/j.mp3"),
                &Options::default()
            ),
            Err(PlanError::WrongInputCount { needed: 2, got: 1 })
        );
    }

    #[test]
    fn mux_copies_streams_and_uses_shortest() {
        let inputs = [
            input("v.mp4", media(true, false, 10.0)),
            input("a.mp3", media(false, true, 8.0)),
        ];
        let p = build(Tool::Mux, &inputs, Path::new("/m.mkv"), &Options::default()).expect("plan");
        assert!(joined(&p).contains("-map 0:v:0 -map 1:a:0 -c copy -shortest"));
        assert_eq!(p.expected_secs, Some(8.0));
    }

    #[test]
    fn mixers_and_split() {
        let two_audio = [
            input("a.mp3", media(false, true, 3.0)),
            input("b.wav", media(false, true, 5.0)),
        ];
        assert!(
            plan(Tool::AudioMix, &two_audio, &Options::default())
                .contains("amix=inputs=2:duration=longest")
        );
        let two_video = [
            input("a.mp4", media(true, true, 3.0)),
            input("b.mp4", media(true, true, 5.0)),
        ];
        assert!(plan(Tool::VideoMix, &two_video, &Options::default()).contains("hstack=inputs=2"));
        let opts = Options {
            split_by_parts: true,
            split_parts: 4,
            ..Options::default()
        };
        let cmd = plan(
            Tool::Split,
            &[input("a.webm", media(true, true, 100.0))],
            &opts,
        );
        assert!(
            cmd.contains("-segment_time 25.000") && cmd.ends_with("part_%03d.webm"),
            "{cmd}"
        );
    }

    #[test]
    fn delogo_keeps_band_inside_frame() {
        let opts = Options {
            delogo: Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 100,
            },
            ..Options::default()
        };
        let cmd = plan(
            Tool::Delogo,
            &[input("a.mp4", media(true, true, 1.0))],
            &opts,
        );
        assert!(cmd.contains("delogo=x=1:y=1:w=1918:h=100"), "{cmd}");
    }

    #[test]
    fn dvd_and_cd_use_raw_device_paths() {
        assert_eq!(
            dvd_root(Path::new("/media/DISC/VIDEO_TS")),
            PathBuf::from("/media/DISC")
        );
        let opts = Options {
            dvd_title: 2,
            ..Options::default()
        };
        let cmd = plan(
            Tool::DvdToVideo,
            &[input("VIDEO_TS", media(true, true, 100.0))],
            &opts,
        );
        assert!(cmd.contains("-f dvdvideo -title 2 -i /in"), "{cmd}");
        let cd = joined(&cd_track(
            Path::new("/dev/cdrom"),
            (10.0, 70.5),
            Path::new("/o/t.mp3"),
            &Options::default(),
        ));
        assert!(cd.contains("-f libcdio -ss 10.000 -to 70.500 -i /dev/cdrom"));
    }

    #[test]
    fn device_profiles_box_and_cap() {
        let cmd = plan(
            Tool::ToDevice(Device::Psp),
            &[input("a.mkv", media(true, true, 9.0))],
            &Options::default(),
        );
        assert!(cmd.contains("scale='min(480,iw)':'min(272,ih)'"), "{cmd}");
        assert!(cmd.contains("-profile:v main -level 3.0") && cmd.contains("-maxrate 768k"));
        assert!(cmd.contains("-r 30"), "60 fps source is capped: {cmd}");
        let iphone = plan(
            Tool::ToDevice(Device::Iphone),
            &[input("a.mkv", media(true, true, 9.0))],
            &Options::default(),
        );
        assert!(!iphone.contains("-r "), "no cap for iPhone");
    }

    #[test]
    fn hardware_decoding_goes_before_input() {
        let mut opts = Options::default();
        opts.video.hw_decode = true;
        let cmd = plan(
            Tool::ToVideo(VideoFormat::Mp4),
            &[input("a.mkv", media(true, true, 1.0))],
            &opts,
        );
        assert!(cmd.find("-hwaccel auto").expect("hwaccel") < cmd.find("-i ").expect("input"));
    }

    #[test]
    fn crop_and_subtitle_tools() {
        let inputs = [input("a.mp4", media(true, true, 5.0))];
        assert_eq!(
            build(Tool::Crop, &inputs, Path::new("/o"), &Options::default()),
            Err(PlanError::NoCropArea)
        );
        let mut opts = Options::default();
        opts.edit.crop = Some(Rect {
            x: 0,
            y: 0,
            w: 640,
            h: 360,
        });
        assert!(plan(Tool::Crop, &inputs, &opts).contains("crop=640:360:0:0"));
        assert_eq!(
            build(
                Tool::AddSubtitles,
                &inputs,
                Path::new("/o"),
                &Options::default()
            ),
            Err(PlanError::NoSubtitleFile)
        );
        opts.edit.subtitle = Some(PathBuf::from("/s.srt"));
        assert!(plan(Tool::AddSubtitles, &inputs, &opts).contains("-map 0 -map 1:0 -c copy"));
        assert_eq!(
            build(Tool::ExtractSubtitles, &inputs, Path::new("/o"), &opts),
            Err(PlanError::NoSubtitleStream)
        );
    }

    #[test]
    fn bluray_uses_protocol_url() {
        let cmd = plan(
            Tool::BluRayToVideo,
            &[input("disc", media(true, true, 100.0))],
            &Options::default(),
        );
        assert!(cmd.contains("-i bluray:/in/disc"), "{cmd}");
    }

    #[test]
    fn recording_stops_gracefully() {
        let p = record(Path::new("/o/rec.mkv"), &Options::default(), ":0");
        assert_eq!(p.stop, StopMode::Quit);
        assert_eq!(p.expected_secs, None);
        assert!(p.args.iter().all(|a| a != "-nostdin"));
    }

    #[test]
    fn filter_escaping_handles_both_levels() {
        assert_eq!(escape_filter_value("C:\\x"), "C\\\\:\\\\\\\\x");
        assert_eq!(escape_filter_value("a,b[c]"), "a\\,b\\[c\\]");
    }

    #[test]
    fn output_names_starting_with_dash_are_not_options() {
        let p = build(
            Tool::RemoveAudio,
            &[input("-rf.mp4", media(true, true, 1.0))],
            Path::new("-x.mp4"),
            &Options::default(),
        )
        .expect("plan");
        assert!(p.args.iter().all(|a| a != "-x.mp4"));
    }
}
