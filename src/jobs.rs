//! Job specs, output naming, and executing a job on a worker thread.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::catalog::{AudioFormat, InputArity, Tool, VideoFormat};
use crate::ffmpeg::{self, RunError};
use crate::i18n::{tr, trf};
use crate::imaging::{self, ImageError};
use crate::options::Options;
use crate::plan::{self, Input, PlanError};

/// Everything a worker needs to run one job; immutable once queued.
#[derive(Clone, Debug)]
pub struct JobSpec {
    pub tool: Tool,
    pub inputs: Vec<PathBuf>,
    /// Final file, or the destination directory for frame extraction.
    pub output: PathBuf,
    pub options: Options,
}

#[derive(Debug)]
pub enum JobError {
    FfmpegMissing,
    Plan(PlanError),
    Run(RunError),
    Image(ImageError),
    Io(std::io::Error),
}

impl JobError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Run(RunError::Cancelled))
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FfmpegMissing => f.write_str(tr("error.ffmpeg_missing")),
            Self::Plan(e) => e.fmt(f),
            Self::Run(e) => e.fmt(f),
            Self::Image(e) => e.fmt(f),
            Self::Io(e) => f.write_str(&trf("error.io", &[("error", e)])),
        }
    }
}

impl std::error::Error for JobError {}

/// File extension a tool produces for a given first input (`None` = directory).
pub fn output_ext(tool: Tool, first_input: &Path, opts: &Options) -> Option<&'static str> {
    Some(match tool {
        Tool::ToVideo(f) => f.ext(),
        Tool::ToAudio(f) => f.ext(),
        Tool::ToImage(f) => f.ext(),
        Tool::ImagesToPdf => "pdf",
        Tool::JoinVideo | Tool::Compress => "mp4",
        Tool::JoinAudio => opts.join_audio_format.ext(),
        Tool::Mux => "mkv",
        Tool::ExtractFrames => return None,
        Tool::Rotate | Tool::Speed => plan::same_format(first_input).ext(),
        // Copy-only tools keep the source container (Matroska holds anything else).
        Tool::Trim | Tool::RemoveAudio | Tool::Normalize => {
            static_ext(first_input).unwrap_or("mkv")
        }
    })
}

/// Returns the input's extension as a `'static` string when it's one we know.
fn static_ext(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let audio = AudioFormat::ALL.into_iter().map(AudioFormat::ext);
    let video = VideoFormat::ALL.into_iter().map(VideoFormat::ext);
    audio.chain(video).find(|known| *known == ext)
}

fn suffix(tool: Tool) -> &'static str {
    match tool {
        Tool::ToVideo(_) | Tool::ToAudio(_) | Tool::ToImage(_) | Tool::ImagesToPdf => "",
        Tool::JoinVideo | Tool::JoinAudio => "_joined",
        Tool::Mux => "_muxed",
        Tool::Trim => "_trim",
        Tool::Compress => "_small",
        Tool::RemoveAudio => "_noaudio",
        Tool::ExtractFrames => "_frames",
        Tool::Rotate => "_rotated",
        Tool::Speed => "_speed",
        Tool::Normalize => "_normalized",
    }
}

/// Picks `dir/stem{suffix}.ext`, adding ` (n)` until the name is free on disk
/// and not already claimed by another queued job.
pub fn allocate_output(
    dir: &Path,
    stem: &str,
    suffix: &str,
    ext: Option<&str>,
    claimed: &mut HashSet<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let name = |n: u32| {
        let counter = if n == 0 {
            String::new()
        } else {
            format!(" ({n})")
        };
        match ext {
            Some(ext) => format!("{stem}{suffix}{counter}.{ext}"),
            None => format!("{stem}{suffix}{counter}"),
        }
    };
    let path = (0..)
        .map(|n| dir.join(name(n)))
        .find(|p| !claimed.contains(p) && !exists(p))
        .unwrap_or_else(|| dir.join(name(0)));
    claimed.insert(path.clone());
    path
}

/// Groups selected files into job specs according to the tool's arity.
pub fn plan_jobs(
    tool: Tool,
    files: &[PathBuf],
    output_dir: Option<&Path>,
    opts: &Options,
    claimed: &mut HashSet<PathBuf>,
) -> Result<Vec<JobSpec>, PlanError> {
    let groups: Vec<Vec<PathBuf>> = match tool.arity() {
        InputArity::EachFile => files.iter().map(|f| vec![f.clone()]).collect(),
        InputArity::AllAtLeast(n) | InputArity::Exactly(n) if files.len() < n => {
            return Err(PlanError::WrongInputCount {
                needed: n,
                got: files.len(),
            });
        }
        InputArity::Exactly(n) if files.len() != n => {
            return Err(PlanError::WrongInputCount {
                needed: n,
                got: files.len(),
            });
        }
        _ => vec![files.to_vec()],
    };
    Ok(groups
        .into_iter()
        .filter_map(|inputs| {
            let first = inputs.first()?;
            let dir = output_dir
                .map(Path::to_path_buf)
                .or_else(|| first.parent().map(Path::to_path_buf))?;
            let stem = first.file_stem()?.to_string_lossy().into_owned();
            let ext = output_ext(tool, first, opts);
            let output = allocate_output(&dir, &stem, suffix(tool), ext, claimed, Path::exists);
            Some(JobSpec {
                tool,
                inputs,
                output,
                options: opts.clone(),
            })
        })
        .collect())
}

/// Runs a job to completion on the calling thread.
pub fn execute(
    spec: &JobSpec,
    ffmpeg_path: Option<&Path>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<(), JobError> {
    match spec.tool {
        Tool::ToImage(format) => {
            let input = spec
                .inputs
                .first()
                .ok_or(JobError::Plan(PlanError::WrongInputCount {
                    needed: 1,
                    got: 0,
                }))?;
            imaging::convert(input, &spec.output, format, &spec.options)
                .map_err(JobError::Image)?;
            on_progress(1.0);
            Ok(())
        }
        Tool::ImagesToPdf => {
            imaging::images_to_pdf(&spec.inputs, &spec.output, &spec.options, on_progress)
                .map_err(JobError::Image)
        }
        tool => {
            let ffmpeg = ffmpeg_path.ok_or(JobError::FfmpegMissing)?;
            let inputs = spec
                .inputs
                .iter()
                .map(|path| {
                    let info = ffmpeg::probe(ffmpeg, path).map_err(JobError::Run)?;
                    Ok(Input {
                        path: path.clone(),
                        info,
                    })
                })
                .collect::<Result<Vec<_>, JobError>>()?;
            let plan =
                plan::build(tool, &inputs, &spec.output, &spec.options).map_err(JobError::Plan)?;
            if tool == Tool::ExtractFrames {
                std::fs::create_dir_all(&spec.output).map_err(JobError::Io)?;
            }
            let result = ffmpeg::run(ffmpeg, &plan, cancel, on_progress).map_err(JobError::Run);
            if result.is_err() {
                remove_partial(&spec.output);
            }
            result
        }
    }
}

/// Deletes a half-written output so failed jobs leave nothing behind.
fn remove_partial(output: &Path) {
    let removed = if output.is_dir() {
        std::fs::remove_dir_all(output)
    } else {
        std::fs::remove_file(output)
    };
    if let Err(e) = removed
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %output.display(), error = %e, "could not remove partial output");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ImageFormat;

    #[test]
    fn allocation_skips_existing_and_claimed_names() {
        let mut claimed = HashSet::new();
        let taken = PathBuf::from("/out/clip.mp4");
        let first = allocate_output(
            Path::new("/out"),
            "clip",
            "",
            Some("mp4"),
            &mut claimed,
            |p| p == taken,
        );
        assert_eq!(first, PathBuf::from("/out/clip (1).mp4"));
        let second = allocate_output(
            Path::new("/out"),
            "clip",
            "",
            Some("mp4"),
            &mut claimed,
            |p| p == taken,
        );
        assert_eq!(second, PathBuf::from("/out/clip (2).mp4"));
    }

    #[test]
    fn each_file_tools_make_one_job_per_file() {
        let files = vec![PathBuf::from("/a/x.mkv"), PathBuf::from("/a/y.avi")];
        let jobs = plan_jobs(
            Tool::ToAudio(AudioFormat::Flac),
            &files,
            Some(Path::new("/o")),
            &Options::default(),
            &mut HashSet::new(),
        )
        .expect("jobs");
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].output, PathBuf::from("/o/x.flac"));
    }

    #[test]
    fn same_stem_inputs_get_distinct_outputs() {
        let files = vec![PathBuf::from("/a/x.png"), PathBuf::from("/b/x.bmp")];
        let jobs = plan_jobs(
            Tool::ToImage(ImageFormat::Webp),
            &files,
            Some(Path::new("/nonexistent-out")),
            &Options::default(),
            &mut HashSet::new(),
        )
        .expect("jobs");
        assert_ne!(jobs[0].output, jobs[1].output);
    }

    #[test]
    fn join_makes_one_job_and_checks_count() {
        let files = vec![PathBuf::from("/a/1.mp4"), PathBuf::from("/a/2.mp4")];
        let jobs = plan_jobs(
            Tool::JoinVideo,
            &files,
            None,
            &Options::default(),
            &mut HashSet::new(),
        )
        .expect("jobs");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].output, PathBuf::from("/a/1_joined.mp4"));
        let err = plan_jobs(
            Tool::Mux,
            &files[..1],
            None,
            &Options::default(),
            &mut HashSet::new(),
        );
        assert_eq!(
            err.err(),
            Some(PlanError::WrongInputCount { needed: 2, got: 1 })
        );
    }

    #[test]
    fn keep_format_tools_keep_known_extensions() {
        let opts = Options::default();
        assert_eq!(
            output_ext(Tool::Trim, Path::new("a.webm"), &opts),
            Some("webm")
        );
        assert_eq!(
            output_ext(Tool::Normalize, Path::new("a.flac"), &opts),
            Some("flac")
        );
        assert_eq!(
            output_ext(Tool::Trim, Path::new("a.weird"), &opts),
            Some("mkv")
        );
        assert_eq!(
            output_ext(Tool::ExtractFrames, Path::new("a.mp4"), &opts),
            None
        );
    }
}

/// End-to-end runs against a real ffmpeg. Skipped unless `MUXR_FFMPEG` points
/// at a binary: `MUXR_FFMPEG=/usr/bin/ffmpeg cargo test e2e -- --nocapture`.
#[cfg(test)]
mod e2e {
    use super::*;
    use crate::catalog::{AudioFormat, VideoFormat};
    use std::process::Command;

    fn ffmpeg() -> Option<PathBuf> {
        std::env::var_os("MUXR_FFMPEG").map(PathBuf::from)
    }

    fn generate(ffmpeg: &Path, out: &Path, extra: &[impl AsRef<std::ffi::OsStr>]) {
        let status = Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(extra)
            .arg(out)
            .status()
            .expect("run ffmpeg");
        assert!(status.success(), "could not generate {}", out.display());
    }

    fn av_source(video: &str, tone_hz: u32) -> Vec<String> {
        [
            "-f",
            "lavfi",
            "-i",
            video,
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency={tone_hz}"),
            "-t",
            "3",
            "-pix_fmt",
            "yuv420p",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    #[test]
    fn every_media_tool_produces_output() {
        let Some(ffmpeg) = ffmpeg() else {
            eprintln!("MUXR_FFMPEG not set; skipping");
            return;
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let video = dir.path().join("clip.mp4");
        let video2 = dir.path().join("other.mkv");
        let audio = dir.path().join("tone.wav");
        generate(
            &ffmpeg,
            &video,
            &av_source("testsrc=size=640x360:rate=25", 440),
        );
        generate(
            &ffmpeg,
            &video2,
            &av_source("testsrc2=size=320x240:rate=30", 660),
        );
        generate(
            &ffmpeg,
            &audio,
            &["-f", "lavfi", "-i", "sine=frequency=330:duration=2"],
        );

        let opts = Options {
            trim_start: "0:01".into(),
            trim_end: "0:02".into(),
            target_size_mb: 1,
            ..Options::default()
        };
        let mut cases: Vec<(Tool, Vec<PathBuf>)> = VideoFormat::ALL
            .into_iter()
            .map(|f| (Tool::ToVideo(f), vec![video.clone()]))
            .chain(
                AudioFormat::ALL
                    .into_iter()
                    .map(|f| (Tool::ToAudio(f), vec![video.clone()])),
            )
            .collect();
        cases.extend([
            (Tool::JoinVideo, vec![video.clone(), video2.clone()]),
            (Tool::JoinAudio, vec![audio.clone(), video.clone()]),
            (Tool::Mux, vec![video2.clone(), audio.clone()]),
            (Tool::Trim, vec![video.clone()]),
            (Tool::Compress, vec![video.clone()]),
            (Tool::RemoveAudio, vec![video.clone()]),
            (Tool::ExtractFrames, vec![video.clone()]),
            (Tool::Rotate, vec![video2.clone()]),
            (Tool::Speed, vec![video.clone()]),
            (Tool::Normalize, vec![audio.clone()]),
        ]);

        let out_dir = dir.path().join("out");
        std::fs::create_dir(&out_dir).expect("out dir");
        let mut claimed = HashSet::new();
        let mut failures = Vec::new();
        for (tool, files) in cases {
            let specs =
                plan_jobs(tool, &files, Some(&out_dir), &opts, &mut claimed).expect("plan jobs");
            for spec in specs {
                let started = std::time::Instant::now();
                let mut last = 0.0f32;
                let result = execute(&spec, Some(&ffmpeg), &AtomicBool::new(false), |p| last = p);
                let produced = if spec.output.is_dir() {
                    std::fs::read_dir(&spec.output).is_ok_and(|mut d| d.next().is_some())
                } else {
                    // The output must decode as media with at least one stream.
                    ffmpeg::probe(&ffmpeg, &spec.output)
                        .is_ok_and(|info| info.has_audio || info.has_video)
                };
                eprintln!(
                    "{:<24} {:>6.2}s progress={last:.2} {:?}",
                    format!("{tool:?}"),
                    started.elapsed().as_secs_f32(),
                    result.as_ref().err().map(ToString::to_string),
                );
                if result.is_err() || !produced || last < 1.0 {
                    failures.push(format!("{tool:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "failed tools: {failures:?}");
    }

    #[test]
    fn cancelling_stops_ffmpeg_and_removes_output() {
        let Some(ffmpeg) = ffmpeg() else {
            return;
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let video = dir.path().join("long.mp4");
        generate(
            &ffmpeg,
            &video,
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=1280x720:rate=30",
                "-t",
                "60",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
            ],
        );
        let spec = JobSpec {
            tool: Tool::ToVideo(VideoFormat::Webm),
            inputs: vec![video],
            output: dir.path().join("long.webm"),
            options: Options::default(),
        };
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let result = execute(&spec, Some(&ffmpeg), &cancel, |p| {
            if p > 0.0 {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        assert!(
            result.as_ref().is_err_and(JobError::is_cancelled),
            "{result:?}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
        assert!(!spec.output.exists(), "partial output left behind");
    }
}
