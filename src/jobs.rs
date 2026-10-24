//! Job specs, their errors, and turning a selection into jobs with
//! collision-free output names.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::{InputArity, Tool};
use crate::cso::CsoError;
use crate::docs;
use crate::externals::External;
use crate::i18n::{tr, trf};
use crate::imaging::ImageError;
use crate::options::{self, Options};
use crate::plan::{self, PlanError};
use crate::process::RunError;

/// Everything a worker needs to run one job; immutable once queued.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub tool: Tool,
    pub inputs: Vec<PathBuf>,
    /// Source address for the downloader.
    #[serde(default)]
    pub url: Option<String>,
    /// Final file, or the destination directory for multi-file tools.
    pub output: PathBuf,
    pub options: Options,
}

#[derive(Debug)]
pub enum JobError {
    Missing(External),
    Plan(PlanError),
    Run(RunError),
    /// yt-dlp failed; most often the site changed and yt-dlp needs updating.
    DownloadFailed(RunError),
    Image(ImageError),
    Cso(CsoError),
    Office(crate::office::OfficeError),
    Io(std::io::Error),
    BadUrl,
    NoOutput,
}

impl JobError {
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Run(RunError::Cancelled) | Self::Cso(CsoError::Cancelled)
        )
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(ext) => {
                f.write_str(&trf("error.missing_program", &[("program", &ext.name())]))
            }
            Self::Plan(e) => e.fmt(f),
            Self::Run(e) => e.fmt(f),
            // The advice comes first: the queue shows only the first line.
            Self::DownloadFailed(e) => write!(f, "{}\n{e}", tr("error.download_failed")),
            Self::Image(e) => e.fmt(f),
            Self::Cso(e) => e.fmt(f),
            Self::Office(e) => e.fmt(f),
            Self::Io(e) => f.write_str(&trf("error.io", &[("error", e)])),
            Self::BadUrl => f.write_str(tr("error.bad_url")),
            Self::NoOutput => f.write_str(tr("error.no_output")),
        }
    }
}

impl std::error::Error for JobError {}

impl From<PlanError> for JobError {
    fn from(e: PlanError) -> Self {
        Self::Plan(e)
    }
}

/// Whether a tool writes one file (with this extension) or a folder of files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    File(&'static str),
    Dir,
    /// The tool names its own files inside the output folder (downloads).
    Into,
}

pub fn output_kind(tool: Tool, first_input: Option<&Path>, opts: &Options) -> OutputKind {
    let first = first_input.unwrap_or(Path::new(""));
    OutputKind::File(match tool {
        Tool::ToVideo(f) => f.ext(),
        Tool::ToDevice(_) => "mp4",
        Tool::ToAudio(f) => f.ext(),
        Tool::JoinImages => "png",
        Tool::PdfToExcel => "xlsx",
        Tool::BluRayToVideo => opts.disc_video_format.ext(),
        Tool::DiscToIso => "iso",
        Tool::AddSubtitles => "mkv",
        Tool::ExtractSubtitles => "srt",
        Tool::Crop => plan::same_format(first).ext(),
        Tool::ToImage(f) => f.ext(),
        Tool::ToEbook(f) => f.ext(),
        Tool::ImagesToPdf | Tool::PdfMerge | Tool::PdfCompress | Tool::OfficeToPdf => "pdf",
        Tool::PdfToText => "txt",
        Tool::PdfToHtml => "html",
        Tool::PdfToWord => "docx",
        Tool::JoinVideo | Tool::Compress | Tool::VideoMix => "mp4",
        Tool::JoinAudio | Tool::AudioMix => opts.join_audio_format.ext(),
        Tool::Mux | Tool::Repair | Tool::Record => "mkv",
        Tool::DvdToVideo => opts.disc_video_format.ext(),
        Tool::FolderToIso | Tool::CsoToIso => "iso",
        Tool::IsoToCso => "cso",
        Tool::Rotate | Tool::Speed | Tool::Delogo => plan::same_format(first).ext(),
        // Copy-only tools keep the source container (Matroska holds anything else).
        Tool::Trim | Tool::RemoveAudio | Tool::Normalize | Tool::Volume => {
            plan::copy_container_ext(first)
        }
        Tool::ExtractFrames
        | Tool::Split
        | Tool::PdfToImages
        | Tool::PdfSplit
        | Tool::CdToAudio => {
            return OutputKind::Dir;
        }
        Tool::Download => return OutputKind::Into,
    })
}

fn suffix(tool: Tool) -> &'static str {
    match tool {
        Tool::JoinVideo | Tool::JoinAudio | Tool::PdfMerge => "_joined",
        Tool::Mux => "_muxed",
        Tool::AudioMix | Tool::VideoMix => "_mix",
        Tool::Trim => "_trim",
        Tool::Split => "_parts",
        Tool::Compress | Tool::PdfCompress => "_small",
        Tool::RemoveAudio => "_noaudio",
        Tool::Delogo => "_delogo",
        Tool::Crop => "_crop",
        Tool::JoinImages => "_joined",
        Tool::AddSubtitles => "_subs",
        Tool::ExtractFrames => "_frames",
        Tool::PdfToImages | Tool::PdfSplit => "_pages",
        Tool::CdToAudio => "_tracks",
        Tool::Rotate => "_rotated",
        Tool::Speed => "_speed",
        Tool::Volume => "_volume",
        Tool::Normalize => "_normalized",
        Tool::Repair => "_fixed",
        _ => "",
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

/// Where tools without a source file (downloads, recordings) save by default.
fn default_dir(sub: &str) -> PathBuf {
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let preferred = home.join(sub);
    if preferred.is_dir() { preferred } else { home }
}

/// What the user selected for one batch.
pub struct Selection<'a> {
    pub tool: Tool,
    pub files: &'a [PathBuf],
    pub url: &'a str,
    pub output_dir: Option<&'a Path>,
    pub options: &'a Options,
}

/// Groups the selection into job specs according to the tool's arity.
pub fn plan_jobs(
    sel: &Selection<'_>,
    claimed: &mut HashSet<PathBuf>,
) -> Result<Vec<JobSpec>, JobError> {
    let tool = sel.tool;
    let spec = |inputs: Vec<PathBuf>, url: Option<String>, output: PathBuf| JobSpec {
        tool,
        inputs,
        url,
        output,
        options: sel.options.clone(),
    };
    match tool {
        Tool::Download => {
            if !docs::is_web_url(sel.url) {
                return Err(JobError::BadUrl);
            }
            let dir = sel
                .output_dir
                .map_or_else(|| default_dir("Downloads"), Path::to_path_buf);
            return Ok(vec![spec(Vec::new(), Some(sel.url.trim().to_owned()), dir)]);
        }
        Tool::Record => {
            let dir = sel
                .output_dir
                .map_or_else(|| default_dir("Videos"), Path::to_path_buf);
            let output = allocate_output(
                &dir,
                "screen_recording",
                "",
                Some("mkv"),
                claimed,
                Path::exists,
            );
            return Ok(vec![spec(Vec::new(), None, output)]);
        }
        _ => {}
    }
    // Absolute paths can never be mistaken for command-line options.
    let files: Vec<PathBuf> = sel
        .files
        .iter()
        .map(|f| std::path::absolute(f).unwrap_or_else(|_| f.clone()))
        .collect();
    let count_error = |needed| PlanError::WrongInputCount {
        needed,
        got: files.len(),
    };
    let groups: Vec<Vec<PathBuf>> = match tool.arity() {
        InputArity::EachFile if files.is_empty() => return Err(count_error(1).into()),
        InputArity::EachFile => files.iter().map(|f| vec![f.clone()]).collect(),
        InputArity::AllAtLeast(n) if files.len() < n => return Err(count_error(n).into()),
        InputArity::Exactly(n) if files.len() != n => return Err(count_error(n).into()),
        _ => vec![files.clone()],
    };
    Ok(groups
        .into_iter()
        .filter_map(|inputs| {
            let first = inputs.first()?;
            let dir = sel
                .output_dir
                .map(Path::to_path_buf)
                .or_else(|| first.parent().map(Path::to_path_buf))?;
            let stem = match first.file_stem() {
                Some(s) => s.to_string_lossy().into_owned(),
                // Devices and roots such as `/dev/cdrom` or `/` have no usable stem.
                None => "disc".to_owned(),
            };
            let ext = match output_kind(tool, Some(first), sel.options) {
                OutputKind::File(ext) => Some(ext),
                OutputKind::Dir | OutputKind::Into => None,
            };
            let tag = format!(
                "{}{}",
                suffix(tool),
                options::sanitize_name_tag(&sel.options.name_tag)
            );
            let output = allocate_output(&dir, &stem, &tag, ext, claimed, Path::exists);
            Some(spec(inputs, None, output))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{AudioFormat, ImageFormat};

    fn select<'a>(
        tool: Tool,
        files: &'a [PathBuf],
        out: Option<&'a Path>,
        opts: &'a Options,
    ) -> Selection<'a> {
        Selection {
            tool,
            files,
            url: "",
            output_dir: out,
            options: opts,
        }
    }

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
        let opts = Options::default();
        let jobs = plan_jobs(
            &select(
                Tool::ToAudio(AudioFormat::Flac),
                &files,
                Some(Path::new("/o")),
                &opts,
            ),
            &mut HashSet::new(),
        )
        .expect("jobs");
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].output, PathBuf::from("/o/x.flac"));
    }

    #[test]
    fn same_stem_inputs_get_distinct_outputs() {
        let files = vec![PathBuf::from("/a/x.png"), PathBuf::from("/b/x.bmp")];
        let opts = Options::default();
        let jobs = plan_jobs(
            &select(
                Tool::ToImage(ImageFormat::Webp),
                &files,
                Some(Path::new("/nonexistent-out")),
                &opts,
            ),
            &mut HashSet::new(),
        )
        .expect("jobs");
        assert_ne!(jobs[0].output, jobs[1].output);
    }

    #[test]
    fn join_makes_one_job_and_checks_count() {
        let files = vec![PathBuf::from("/a/1.mp4"), PathBuf::from("/a/2.mp4")];
        let opts = Options::default();
        let jobs = plan_jobs(
            &select(Tool::JoinVideo, &files, None, &opts),
            &mut HashSet::new(),
        )
        .expect("jobs");
        assert_eq!(jobs.len(), 1);
        // Inputs are made absolute, which on Windows adds the current drive.
        let expected = std::path::absolute("/a/1_joined.mp4").expect("absolute");
        assert_eq!(jobs[0].output, expected);
        let err = plan_jobs(
            &select(Tool::Mux, &files[..1], None, &opts),
            &mut HashSet::new(),
        );
        assert!(matches!(
            err,
            Err(JobError::Plan(PlanError::WrongInputCount {
                needed: 2,
                got: 1
            }))
        ));
    }

    #[test]
    fn name_tag_is_appended_safely() {
        let files = vec![PathBuf::from("/a/x.mkv")];
        let opts = Options {
            name_tag: "_hd/../".into(),
            ..Options::default()
        };
        let jobs = plan_jobs(
            &select(
                Tool::ToAudio(AudioFormat::Mp3),
                &files,
                Some(Path::new("/o")),
                &opts,
            ),
            &mut HashSet::new(),
        )
        .expect("jobs");
        assert_eq!(jobs[0].output, PathBuf::from("/o/x_hd.mp3"));
    }

    #[test]
    fn downloads_require_web_urls() {
        let opts = Options::default();
        let mut sel = select(Tool::Download, &[], Some(Path::new("/dl")), &opts);
        sel.url = "ytsearch:x";
        assert!(matches!(
            plan_jobs(&sel, &mut HashSet::new()),
            Err(JobError::BadUrl)
        ));
        sel.url = " https://example.com/v ";
        let jobs = plan_jobs(&sel, &mut HashSet::new()).expect("jobs");
        assert_eq!(jobs[0].url.as_deref(), Some("https://example.com/v"));
        assert_eq!(jobs[0].output, PathBuf::from("/dl"));
    }

    #[test]
    fn device_inputs_get_a_fallback_stem() {
        let files = vec![PathBuf::from("/")];
        let opts = Options::default();
        let jobs = plan_jobs(
            &select(Tool::CdToAudio, &files, Some(Path::new("/o")), &opts),
            &mut HashSet::new(),
        )
        .expect("jobs");
        assert_eq!(jobs[0].output, PathBuf::from("/o/disc_tracks"));
    }

    #[test]
    fn output_kinds() {
        let opts = Options::default();
        assert_eq!(
            output_kind(Tool::Trim, Some(Path::new("a.webm")), &opts),
            OutputKind::File("webm")
        );
        assert_eq!(
            output_kind(Tool::Normalize, Some(Path::new("a.flac")), &opts),
            OutputKind::File("flac")
        );
        assert_eq!(
            output_kind(Tool::Trim, Some(Path::new("a.weird")), &opts),
            OutputKind::File("mkv")
        );
        assert_eq!(
            output_kind(Tool::ExtractFrames, Some(Path::new("a.mp4")), &opts),
            OutputKind::Dir
        );
        assert_eq!(output_kind(Tool::Download, None, &opts), OutputKind::Into);
    }

    #[test]
    fn specs_round_trip_through_json() {
        let files = vec![PathBuf::from("/a/x.mkv")];
        let opts = Options::default();
        let jobs = plan_jobs(
            &select(Tool::Trim, &files, None, &opts),
            &mut HashSet::new(),
        )
        .expect("jobs");
        let text = serde_json::to_string(&jobs).expect("serialize");
        let back: Vec<JobSpec> = serde_json::from_str(&text).expect("parse");
        assert_eq!(back[0].output, jobs[0].output);
    }
}
