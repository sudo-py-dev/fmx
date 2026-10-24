//! Pure translation of (tool, inputs, options) into ffmpeg arguments.
//! No I/O happens here, so every recipe is unit-testable.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::catalog::{AudioFormat, Tool, VideoFormat};
use crate::i18n::{tr, trf};
use crate::options::{self, Options, Quality, parse_timestamp};
use crate::probe::MediaInfo;

/// One input file together with what probing found out about it.
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
const FALLBACK_JOIN_SIZE: (u32, u32) = (1280, 720);

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
        self.flag("-i", file_url(path))
    }

    fn output(mut self, path: &Path) -> PendingPlan {
        self.0.push(file_url(path));
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
        }
    }
}

/// `file:` stops ffmpeg from treating names like `-x.mp4` or `http:x` specially.
fn file_url(path: &Path) -> OsString {
    let mut url = OsString::from("file:");
    url.push(path.as_os_str());
    url
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

/// Scale filter capping height without upscaling, keeping width even.
fn scale_filter(max_height: u32) -> String {
    format!("scale=-2:'min({max_height},ih)'")
}

/// Video encoder flags for a container format.
fn video_codec(format: VideoFormat, q: Quality) -> Vec<OsString> {
    let crf = |h: u8, m: u8, l: u8| q.pick(h, m, l).to_string();
    let qscale = q.pick("2", "5", "9");
    match format {
        VideoFormat::Mp4
        | VideoFormat::Mkv
        | VideoFormat::Mov
        | VideoFormat::Flv
        | VideoFormat::Ts => strings(&[
            "-c:v",
            "libx264",
            "-preset",
            "faster",
            "-crf",
            &crf(18, 23, 28),
            "-pix_fmt",
            "yuv420p",
        ]),
        VideoFormat::Mp4Hevc => strings(&[
            "-c:v",
            "libx265",
            "-preset",
            "fast",
            "-crf",
            &crf(22, 27, 31),
            "-pix_fmt",
            "yuv420p",
            "-tag:v",
            "hvc1",
        ]),
        VideoFormat::ThreeGp => strings(&[
            "-c:v",
            "libx264",
            "-preset",
            "faster",
            "-profile:v",
            "baseline",
            "-crf",
            &crf(20, 25, 30),
            "-pix_fmt",
            "yuv420p",
        ]),
        VideoFormat::Webm => strings(&[
            "-c:v",
            "libvpx-vp9",
            "-b:v",
            "0",
            "-crf",
            &crf(24, 32, 40),
            "-deadline",
            "good",
            "-cpu-used",
            "4",
            "-row-mt",
            "1",
        ]),
        VideoFormat::Avi => strings(&["-c:v", "mpeg4", "-q:v", qscale]),
        VideoFormat::Wmv => strings(&["-c:v", "wmv2", "-q:v", qscale]),
        VideoFormat::Mpg => strings(&["-c:v", "mpeg2video", "-q:v", qscale]),
        VideoFormat::Ogv => strings(&["-c:v", "libtheora", "-q:v", q.pick("9", "7", "5")]),
        VideoFormat::Gif => Vec::new(),
    }
}

/// Audio encoder flags that suit a video container.
fn audio_codec_for_video(format: VideoFormat, q: Quality) -> Vec<OsString> {
    let br = q.pick("192k", "160k", "128k");
    match format {
        VideoFormat::Webm => strings(&["-c:a", "libopus", "-b:a", q.pick("160k", "128k", "96k")]),
        VideoFormat::Avi => strings(&["-c:a", "libmp3lame", "-b:a", br]),
        VideoFormat::Wmv => strings(&["-c:a", "wmav2", "-b:a", br]),
        VideoFormat::Mpg => strings(&["-c:a", "mp2", "-b:a", q.pick("256k", "192k", "160k")]),
        VideoFormat::Ogv => strings(&["-c:a", "libvorbis", "-q:a", q.pick("7", "5", "3")]),
        VideoFormat::ThreeGp => strings(&["-c:a", "aac", "-b:a", "96k", "-ac", "1"]),
        VideoFormat::Gif => strings(&["-an"]),
        _ => strings(&["-c:a", "aac", "-b:a", br]),
    }
}

fn mp4_family(format: VideoFormat) -> bool {
    matches!(
        format,
        VideoFormat::Mp4 | VideoFormat::Mp4Hevc | VideoFormat::Mov | VideoFormat::ThreeGp
    )
}

/// Encoder flags for an audio-only output.
fn audio_codec(format: AudioFormat, q: Quality) -> Vec<OsString> {
    let br = q.pick("256k", "192k", "128k");
    match format {
        AudioFormat::Mp3 => {
            strings(&["-c:a", "libmp3lame", "-b:a", q.pick("320k", "192k", "128k")])
        }
        AudioFormat::Aac | AudioFormat::M4a => strings(&["-c:a", "aac", "-b:a", br]),
        AudioFormat::Flac => strings(&["-c:a", "flac"]),
        AudioFormat::Wav => strings(&["-c:a", "pcm_s16le"]),
        AudioFormat::Aiff => strings(&["-c:a", "pcm_s16be"]),
        AudioFormat::Ogg => strings(&["-c:a", "libvorbis", "-q:a", q.pick("8", "6", "4")]),
        AudioFormat::Opus => strings(&["-c:a", "libopus", "-b:a", q.pick("160k", "128k", "96k")]),
        AudioFormat::Wma => strings(&["-c:a", "wmav2", "-b:a", br]),
        AudioFormat::Ac3 => strings(&["-c:a", "ac3", "-b:a", q.pick("448k", "384k", "256k")]),
    }
}

/// Full encode flags (video + audio + container tweaks) for a video format.
fn video_encode(format: VideoFormat, opts: &Options, extra_vf: Option<&str>) -> Vec<OsString> {
    let mut filters: Vec<String> = extra_vf.map(str::to_owned).into_iter().collect();
    if format == VideoFormat::Gif {
        let fps = opts.quality.pick(15, 12, 10);
        let height = opts.resolution.max_height().unwrap_or(480);
        filters.push(format!(
            "fps={fps},{},split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer",
            scale_filter(height)
        ));
    } else if let Some(h) = opts.resolution.max_height() {
        filters.push(scale_filter(h));
    }
    let mut out = Vec::new();
    if !filters.is_empty() {
        out.extend(strings(&["-vf", &filters.join(",")]));
    }
    out.extend(video_codec(format, opts.quality));
    out.extend(audio_codec_for_video(format, opts.quality));
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

/// The container to re-encode into when an operation keeps the input's format.
pub fn same_format(path: &Path) -> VideoFormat {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(VideoFormat::from_ext)
        .filter(|f| *f != VideoFormat::Gif)
        .unwrap_or(VideoFormat::Mp4)
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

fn trim_window(opts: &Options, duration: Option<f64>) -> Result<(f64, Option<f64>), PlanError> {
    let parse = |text: &str| {
        parse_timestamp(text).map_err(|()| PlanError::BadTimestamp(text.trim().to_owned()))
    };
    let start = parse(&opts.trim_start)?.unwrap_or(0.0);
    let end = parse(&opts.trim_end)?;
    let stop = end.or(duration);
    if stop.is_some_and(|stop| stop <= start) {
        return Err(PlanError::EmptyTrimWindow);
    }
    Ok((start, end))
}

/// Builds the ffmpeg invocation for `tool`. `output` is the final file path
/// (or, for frame extraction, the destination directory).
pub fn build(
    tool: Tool,
    inputs: &[Input],
    output: &Path,
    opts: &Options,
) -> Result<Plan, PlanError> {
    match tool {
        Tool::ToVideo(format) => {
            let input = single(inputs)?;
            if !input.info.has_video {
                return Err(PlanError::NoVideoStream);
            }
            Ok(base()
                .input(&input.path)
                .extend(video_encode(format, opts, None))
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::ToAudio(format) => {
            let input = single(inputs)?;
            if !input.info.has_audio {
                return Err(PlanError::NoAudioStream);
            }
            Ok(base()
                .input(&input.path)
                .switch("-vn")
                .extend(audio_codec(format, opts.quality))
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::Trim => trim(single(inputs)?, output, opts),
        Tool::RemoveAudio => {
            let input = single(inputs)?;
            if !input.info.has_video {
                return Err(PlanError::NoVideoStream);
            }
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
            let (video, audio) = (&inputs[0], &inputs[1]);
            if !video.info.has_video {
                return Err(PlanError::NoVideoStream);
            }
            if !audio.info.has_audio {
                return Err(PlanError::NoAudioStream);
            }
            // Matroska accepts any codec pair, so both streams are copied untouched.
            let expected = match (video.info.duration_s, audio.info.duration_s) {
                (Some(v), Some(a)) => Some(v.min(a)),
                (v, a) => v.or(a),
            };
            Ok(base()
                .input(&video.path)
                .input(&audio.path)
                .flag("-map", "0:v:0")
                .flag("-map", "1:a:0")
                .flag("-c", "copy")
                .switch("-shortest")
                .output(output)
                .expect(expected))
        }
        Tool::Compress => {
            let input = single(inputs)?;
            if !input.info.has_video {
                return Err(PlanError::NoVideoStream);
            }
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
        Tool::ExtractFrames => {
            let input = single(inputs)?;
            if !input.info.has_video {
                return Err(PlanError::NoVideoStream);
            }
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
            let input = single(inputs)?;
            if !input.info.has_video {
                return Err(PlanError::NoVideoStream);
            }
            let format = same_format(&input.path);
            Ok(base()
                .input(&input.path)
                .extend(video_encode(
                    format,
                    opts,
                    Some(opts.rotation.video_filter()),
                ))
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::Speed => speed(single(inputs)?, output, opts),
        Tool::Normalize => {
            let input = single(inputs)?;
            if !input.info.has_audio {
                return Err(PlanError::NoAudioStream);
            }
            // Video is copied; ffmpeg picks the container's default audio encoder.
            Ok(base()
                .input(&input.path)
                .flag("-af", "loudnorm=I=-16:TP=-1.5:LRA=11")
                .flag("-c:v", "copy")
                .output(output)
                .expect(input.info.duration_s))
        }
        Tool::JoinVideo => join_video(inputs, output, opts),
        Tool::JoinAudio => join_audio(inputs, output, opts),
        Tool::ToImage(_) | Tool::ImagesToPdf => Err(PlanError::UnsupportedTool),
    }
}

fn trim(input: &Input, output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    let (start, end) = trim_window(opts, input.info.duration_s)?;
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
        args.extend(video_encode(same_format(&input.path), opts, None))
    } else {
        args
    };
    let expected = end.or(input.info.duration_s).map(|stop| stop - start);
    Ok(args.output(output).expect(expected))
}

fn speed(input: &Input, output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    let factor = f64::from(opts.speed.clamp(options::MIN_SPEED, options::MAX_SPEED));
    let mut args = base().input(&input.path);
    if input.info.has_audio {
        args = args.flag("-filter:a", atempo_chain(factor));
    }
    if input.info.has_video {
        let setpts = format!("setpts=PTS/{factor}");
        args = args.extend(video_encode(same_format(&input.path), opts, Some(&setpts)));
    }
    let expected = input.info.duration_s.map(|d| d / factor);
    Ok(args.output(output).expect(expected))
}

fn total_duration(inputs: &[Input]) -> Option<f64> {
    inputs.iter().map(|i| i.info.duration_s).sum()
}

fn join_video(inputs: &[Input], output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    require_count(inputs, 2, false)?;
    if inputs.iter().any(|i| !i.info.has_video) {
        return Err(PlanError::NoVideoStream);
    }
    let with_audio = inputs.iter().all(|i| i.info.has_audio);
    let first = &inputs[0].info;
    let (src_w, src_h) = match (first.width, first.height) {
        (Some(w), Some(h)) => (w, h),
        _ => FALLBACK_JOIN_SIZE,
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
        .extend(video_codec(format, opts.quality))
        .extend(if with_audio {
            audio_codec_for_video(format, opts.quality)
        } else {
            strings(&["-an"])
        })
        .extend(strings(&["-movflags", "+faststart"]))
        .output(output)
        .expect(total_duration(inputs)))
}

fn join_audio(inputs: &[Input], output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    require_count(inputs, 2, false)?;
    if inputs.iter().any(|i| !i.info.has_audio) {
        return Err(PlanError::NoAudioStream);
    }
    let mut graph = String::new();
    for i in 0..inputs.len() {
        graph.push_str(&format!(
            "[{i}:a:0]aresample={JOIN_SAMPLE_RATE},aformat=channel_layouts=stereo[a{i}];"
        ));
    }
    let pads: String = (0..inputs.len()).map(|i| format!("[a{i}]")).collect();
    graph.push_str(&format!("{pads}concat=n={}:v=0:a=1[a]", inputs.len()));
    Ok(inputs
        .iter()
        .fold(base(), |args, i| args.input(&i.path))
        .flag("-filter_complex", graph)
        .flag("-map", "[a]")
        .extend(audio_codec(opts.join_audio_format, opts.quality))
        .output(output)
        .expect(total_duration(inputs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(video: bool, audio: bool, secs: f64) -> MediaInfo {
        MediaInfo {
            duration_s: Some(secs),
            has_video: video,
            has_audio: audio,
            width: video.then_some(1920),
            height: video.then_some(1080),
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

    #[test]
    fn mp4_conversion_uses_x264_aac_faststart() {
        let inputs = [input("a.avi", media(true, true, 10.0))];
        let plan = build(
            Tool::ToVideo(VideoFormat::Mp4),
            &inputs,
            Path::new("/out/a.mp4"),
            &Options::default(),
        )
        .expect("plan");
        let cmd = joined(&plan);
        assert!(cmd.contains("-i file:/in/a.avi"));
        assert!(cmd.contains("-c:v libx264") && cmd.contains("-c:a aac"));
        assert!(cmd.contains("+faststart"));
        assert!(cmd.ends_with("file:/out/a.mp4"));
        assert_eq!(plan.expected_secs, Some(10.0));
    }

    #[test]
    fn resolution_adds_non_upscaling_scale() {
        let opts = Options {
            resolution: crate::options::Resolution::P720,
            ..Options::default()
        };
        let inputs = [input("a.mkv", media(true, true, 1.0))];
        let plan = build(
            Tool::ToVideo(VideoFormat::Webm),
            &inputs,
            Path::new("/o.webm"),
            &opts,
        )
        .expect("plan");
        assert!(joined(&plan).contains("scale=-2:'min(720,ih)'"));
    }

    #[test]
    fn audio_from_audio_only_input_is_rejected_for_video() {
        let inputs = [input("a.mp3", media(false, true, 1.0))];
        let err = build(
            Tool::ToVideo(VideoFormat::Mp4),
            &inputs,
            Path::new("/o.mp4"),
            &Options::default(),
        );
        assert_eq!(err, Err(PlanError::NoVideoStream));
    }

    #[test]
    fn audio_conversion_drops_video() {
        let inputs = [input("a.mp4", media(true, true, 5.0))];
        let plan = build(
            Tool::ToAudio(AudioFormat::Mp3),
            &inputs,
            Path::new("/o.mp3"),
            &Options::default(),
        )
        .expect("plan");
        let cmd = joined(&plan);
        assert!(cmd.contains("-vn") && cmd.contains("libmp3lame"));
    }

    #[test]
    fn trim_copy_uses_input_seeking() {
        let opts = Options {
            trim_start: "0:10".into(),
            trim_end: "0:25".into(),
            ..Options::default()
        };
        let inputs = [input("a.mp4", media(true, true, 60.0))];
        let plan = build(Tool::Trim, &inputs, Path::new("/o.mp4"), &opts).expect("plan");
        let cmd = joined(&plan);
        assert!(cmd.find("-ss 10.000").expect("ss") < cmd.find("-i ").expect("i"));
        assert!(cmd.contains("-c copy"));
        assert_eq!(plan.expected_secs, Some(15.0));
    }

    #[test]
    fn trim_rejects_bad_and_empty_windows() {
        let inputs = [input("a.mp4", media(true, true, 60.0))];
        let bad = Options {
            trim_start: "x".into(),
            ..Options::default()
        };
        assert!(matches!(
            build(Tool::Trim, &inputs, Path::new("/o"), &bad),
            Err(PlanError::BadTimestamp(_))
        ));
        let empty = Options {
            trim_start: "30".into(),
            trim_end: "10".into(),
            ..Options::default()
        };
        assert_eq!(
            build(Tool::Trim, &inputs, Path::new("/o"), &empty),
            Err(PlanError::EmptyTrimWindow)
        );
        let past_end = Options {
            trim_start: "90".into(),
            ..Options::default()
        };
        assert_eq!(
            build(Tool::Trim, &inputs, Path::new("/o"), &past_end),
            Err(PlanError::EmptyTrimWindow)
        );
    }

    #[test]
    fn atempo_stages_stay_in_range() {
        assert_eq!(atempo_chain(1.5), "atempo=1.5");
        assert_eq!(atempo_chain(4.0), "atempo=2,atempo=2");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.5");
    }

    #[test]
    fn target_bitrate_math() {
        // 10 MiB over 80 s ≈ 1.006 Mb/s total, minus 128 kb/s audio.
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
        let plan = build(
            Tool::JoinVideo,
            &inputs,
            Path::new("/j.mp4"),
            &Options::default(),
        )
        .expect("plan");
        let cmd = joined(&plan);
        assert!(cmd.contains("[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"));
        assert!(cmd.contains("scale=1920:1080"));
        assert_eq!(plan.expected_secs, Some(7.0));
    }

    #[test]
    fn join_video_without_audio_everywhere_drops_audio() {
        let inputs = [
            input("a.mp4", media(true, false, 3.0)),
            input("b.mp4", media(true, true, 4.0)),
        ];
        let cmd = joined(
            &build(
                Tool::JoinVideo,
                &inputs,
                Path::new("/j.mp4"),
                &Options::default(),
            )
            .expect("plan"),
        );
        assert!(cmd.contains("concat=n=2:v=1:a=0[v]") && cmd.contains("-an"));
    }

    #[test]
    fn join_requires_two_inputs() {
        let inputs = [input("a.mp3", media(false, true, 3.0))];
        assert_eq!(
            build(
                Tool::JoinAudio,
                &inputs,
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
        let plan =
            build(Tool::Mux, &inputs, Path::new("/m.mkv"), &Options::default()).expect("plan");
        let cmd = joined(&plan);
        assert!(cmd.contains("-map 0:v:0 -map 1:a:0 -c copy -shortest"));
        assert_eq!(plan.expected_secs, Some(8.0));
    }

    #[test]
    fn output_names_starting_with_dash_are_not_options() {
        let inputs = [input("-rf.mp4", media(true, true, 1.0))];
        let plan = build(
            Tool::RemoveAudio,
            &inputs,
            Path::new("-x.mp4"),
            &Options::default(),
        )
        .expect("plan");
        assert!(plan.args.iter().all(|a| a != "-x.mp4"));
    }
}
