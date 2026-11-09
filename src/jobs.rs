//! Job specs, their errors, and turning a selection into jobs with
//! collision-free output names.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
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

impl JobSpec {
    pub fn validate(&self) -> Result<(), JobError> {
        self.options.validate(self.tool)?;
        if self.tool == Tool::AddSubtitles && self.options.edit.subtitle.is_none() {
            return Err(PlanError::NoSubtitleFile.into());
        }
        if self.tool == Tool::Download {
            let url = self.url.as_deref().ok_or(JobError::BadUrl)?;
            if !docs::is_web_url(url) {
                return Err(JobError::BadUrl);
            }
        } else {
            let count = self.inputs.len();
            let arity = self.tool.arity();
            let count_error =
                |needed| JobError::Plan(PlanError::WrongInputCount { needed, got: count });
            match arity {
                InputArity::EachFile if count != 1 => return Err(count_error(1)),
                InputArity::AllAtLeast(n) if count < n => return Err(count_error(n)),
                InputArity::Exactly(n) if count != n => return Err(count_error(n)),
                _ => {}
            }
            for input in &self.inputs {
                let s = input.to_string_lossy();
                if s.is_empty() || s.contains('\0') || s.chars().any(|c| c.is_control()) {
                    return Err(JobError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "invalid input path",
                    )));
                }
            }
        }
        let out_str = self.output.to_string_lossy();
        if out_str.is_empty() || out_str.contains('\0') || out_str.chars().any(|c| c.is_control()) {
            return Err(JobError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid output path",
            )));
        }
        Ok(())
    }
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
    SubtitleMatchMissing(PathBuf),
    SubtitleMatchAmbiguous(PathBuf),
    NoOutput,
    /// The screen recorder could not start; the message says why.
    Recorder(String),
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
            Self::SubtitleMatchMissing(path) => f.write_str(&trf(
                "error.subtitle_match_missing",
                &[("file", &path.display())],
            )),
            Self::SubtitleMatchAmbiguous(path) => f.write_str(&trf(
                "error.subtitle_match_ambiguous",
                &[("file", &path.display())],
            )),
            Self::NoOutput => f.write_str(tr("error.no_output")),
            Self::Recorder(e) => f.write_str(e),
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
    /// Inspection has no destination and cannot be queued.
    None,
    File(&'static str),
    Dir,
    /// The tool names its own files inside the output folder (downloads).
    Into,
}

/// Determines whether the tool writes a file, a directory, or into a directory.
/// `first_input` supplies the source format for tools that preserve it.
pub fn output_kind(tool: Tool, first_input: Option<&Path>, opts: &Options) -> OutputKind {
    let first = first_input.unwrap_or(Path::new(""));
    OutputKind::File(match tool {
        Tool::ToVideo(f) => f.ext(),
        Tool::Reverse | Tool::Fade => {
            let ext = first
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if crate::catalog::AUDIO_EXTENSIONS
                .iter()
                .any(|a| a.eq_ignore_ascii_case(ext))
            {
                "flac"
            } else {
                "mkv"
            }
        }
        Tool::Stabilize | Tool::Denoise | Tool::Sharpen | Tool::WaveformVideo => "mp4",
        Tool::AudioDenoise | Tool::TrimSilence | Tool::Karaoke => "flac",
        Tool::ThumbnailSheet => "png",
        Tool::EditTags => opts.media_tools.tag_format.ext(),
        Tool::ToDevice(_) => "mp4",
        Tool::ToAudio(f) => f.ext(),
        Tool::RemoveImageMetadata => "png",
        Tool::MediaInfo => return OutputKind::None,
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
        Tool::JoinVideo | Tool::Slideshow | Tool::Compress | Tool::VideoMix => "mp4",
        Tool::JoinAudio | Tool::AudioMix => opts.join_audio_format.ext(),
        Tool::Mux | Tool::Repair => "mkv",
        Tool::Record => opts.record.format.ext(),
        Tool::RecordAudio => opts.sound_format.ext(),
        Tool::Screenshot => opts.screenshot.format.ext(),
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
        Tool::Slideshow => "_slideshow",
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
        Tool::Reverse => "_reverse",
        Tool::Fade => "_fade",
        Tool::Stabilize => "_stabilize",
        Tool::Denoise => "_denoise",
        Tool::Sharpen => "_sharpen",
        Tool::AudioDenoise => "_audio_denoise",
        Tool::TrimSilence => "_trim_silence",
        Tool::Karaoke => "_karaoke",
        Tool::ThumbnailSheet => "_thumbnail_sheet",
        Tool::WaveformVideo => "_waveform_video",
        Tool::EditTags => "_edit_tags",
        Tool::RemoveImageMetadata => "_private",
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
    OutputAllocator::new(claimed).allocate(dir, stem, suffix, ext, exists)
}

/// Remembers collision searches for one batch. Claims are retained for the
/// entire batch, and every new candidate is still checked against disk.
pub struct OutputAllocator<'a> {
    claimed: &'a mut HashSet<PathBuf>,
    next_suffix: HashMap<(PathBuf, Option<String>), u64>,
}

impl<'a> OutputAllocator<'a> {
    pub fn new(claimed: &'a mut HashSet<PathBuf>) -> Self {
        Self {
            claimed,
            next_suffix: HashMap::new(),
        }
    }

    pub fn allocate(
        &mut self,
        dir: &Path,
        stem: &str,
        suffix: &str,
        ext: Option<&str>,
        exists: impl Fn(&Path) -> bool,
    ) -> PathBuf {
        let base = format!("{stem}{suffix}");
        let next = self
            .next_suffix
            .entry((dir.join(&base), ext.map(str::to_owned)))
            .or_default();
        loop {
            let counter = if *next == 0 {
                String::new()
            } else {
                format!(" ({next})")
            };
            let name = match ext {
                Some(ext) => format!("{base}{counter}.{ext}"),
                None => format!("{base}{counter}"),
            };
            let path = dir.join(name);
            *next += 1;
            if !self.claimed.contains(&path) && !exists(&path) {
                self.claimed.insert(path.clone());
                return path;
            }
        }
    }
}

/// Sortable local capture time, with milliseconds and filesystem-safe separators.
/// `allocate_output` also handles captures queued in the same millisecond.
fn capture_stem(prefix: &str, at: chrono::DateTime<chrono::Local>) -> String {
    format!("{prefix}_{}", at.format("%Y-%m-%d_%H-%M-%S%.3f"))
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

/// Match exact stems in each video's directory. Multiple formats with the same
/// stem are ambiguous, so never pick a track based on directory iteration order.
fn match_subtitles(files: &[PathBuf]) -> Result<Vec<PathBuf>, JobError> {
    let mut directories: HashMap<PathBuf, HashMap<OsString, Vec<PathBuf>>> = HashMap::new();
    let mut matches = Vec::with_capacity(files.len());
    for video in files {
        let dir = video.parent().unwrap_or(Path::new("."));
        let subtitles = match directories.entry(dir.to_path_buf()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut subtitles: HashMap<OsString, Vec<PathBuf>> = HashMap::new();
                for entry in std::fs::read_dir(dir).map_err(JobError::Io)? {
                    let entry = entry.map_err(JobError::Io)?;
                    let path = entry.path();
                    if path.is_file()
                        && path
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| {
                                ["srt", "ass", "ssa", "vtt"]
                                    .iter()
                                    .any(|supported| supported.eq_ignore_ascii_case(ext))
                            })
                        && let Some(stem) = path.file_stem()
                    {
                        subtitles.entry(stem.to_os_string()).or_default().push(path);
                    }
                }
                entry.insert(subtitles)
            }
        };
        let candidates = video.file_stem().and_then(|stem| subtitles.get(stem));
        match candidates.map(Vec::as_slice) {
            Some([subtitle]) => matches.push(subtitle.clone()),
            Some([_, _, ..]) => return Err(JobError::SubtitleMatchAmbiguous(video.clone())),
            _ => return Err(JobError::SubtitleMatchMissing(video.clone())),
        }
    }
    Ok(matches)
}

/// Groups the selection into job specs according to the tool's arity.
///
/// Allocated output paths are added to `claimed` to avoid name collisions.
/// Recordings use the Music or Videos folder by default when it exists, or the
/// home directory otherwise (or the current directory if no home is found).
/// A download uses its destination directory directly.
///
/// Returns an error for invalid options, a non-web download URL, or an invalid
/// input count. Input groups without a usable output directory are omitted.
pub fn plan_jobs(
    sel: &Selection<'_>,
    claimed: &mut HashSet<PathBuf>,
) -> Result<Vec<JobSpec>, JobError> {
    if sel.tool == Tool::MediaInfo {
        return Err(PlanError::UnsupportedTool.into());
    }
    sel.options.validate(sel.tool)?;
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
        Tool::Screenshot => {
            let dir = sel
                .output_dir
                .map_or_else(|| default_dir("Pictures"), Path::to_path_buf);
            let output = allocate_output(
                &dir,
                &capture_stem("screenshot", chrono::Local::now()),
                "",
                Some(sel.options.screenshot.format.ext()),
                claimed,
                Path::exists,
            );
            return Ok(vec![spec(Vec::new(), None, output)]);
        }
        Tool::Record | Tool::RecordAudio => {
            let audio = sel.tool == Tool::RecordAudio;
            let dir = sel.output_dir.map_or_else(
                || default_dir(if audio { "Music" } else { "Videos" }),
                Path::to_path_buf,
            );
            let output = allocate_output(
                &dir,
                &capture_stem(
                    if audio {
                        "sound_recording"
                    } else {
                        "screen_recording"
                    },
                    chrono::Local::now(),
                ),
                "",
                Some(if audio {
                    sel.options.sound_format.ext()
                } else {
                    sel.options.record.format.ext()
                }),
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
    // Resolve the entire batch before claiming outputs or queueing any jobs.
    let subtitles = if tool == Tool::AddSubtitles && sel.options.edit.match_subtitles {
        Some(match_subtitles(&files)?)
    } else {
        None
    };
    let mut allocator = OutputAllocator::new(claimed);
    Ok(groups
        .into_iter()
        .enumerate()
        .filter_map(|(index, inputs)| {
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
                OutputKind::None => return None,
            };
            let tag = format!(
                "{}{}",
                suffix(tool),
                options::sanitize_name_tag(&sel.options.name_tag)
            );
            let output = allocator.allocate(&dir, &stem, &tag, ext, Path::exists);
            let mut job = spec(inputs, None, output);
            if let Some(subtitles) = &subtitles {
                job.options.edit.subtitle = Some(subtitles[index].clone());
                job.options.edit.match_subtitles = false;
            }
            Some(job)
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
    fn sound_recorder_allocates_audio_without_source_files() {
        let opts = Options {
            sound_format: AudioFormat::Flac,
            ..Options::default()
        };
        let jobs = plan_jobs(
            &select(Tool::RecordAudio, &[], Some(Path::new("/tmp/audio")), &opts),
            &mut HashSet::new(),
        )
        .unwrap();
        assert_eq!(jobs.len(), 1);
        assert!(jobs[0].inputs.is_empty());
        assert_eq!(jobs[0].output.extension().unwrap(), "flac");
        assert!(
            jobs[0]
                .output
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sound_recording_")
        );
        jobs[0].validate().unwrap();
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
    fn batch_allocation_does_not_rescan_collisions() {
        use std::cell::Cell;

        let mut claimed = HashSet::new();
        let mut allocator = OutputAllocator::new(&mut claimed);
        let dir = Path::new("/out");
        let probes = Cell::new(0);
        let existing: HashSet<_> = std::iter::once(dir.join("clip.mp4"))
            .chain((1..1000).map(|n| dir.join(format!("clip ({n}).mp4"))))
            .collect();
        for n in 1000..2000 {
            let output = allocator.allocate(dir, "clip", "", Some("mp4"), |p| {
                probes.set(probes.get() + 1);
                existing.contains(p)
            });
            assert_eq!(output, dir.join(format!("clip ({n}).mp4")));
        }
        assert_eq!(probes.get(), 2000);
        assert_eq!(claimed.len(), 1000);
    }

    #[test]
    fn batch_allocation_checks_overlapping_families_and_extensions() {
        let dir = Path::new("/out");
        let mut claimed = HashSet::from([dir.join("clip (2).mp4")]);
        let mut allocator = OutputAllocator::new(&mut claimed);
        let mut allocate =
            |stem, suffix, ext| allocator.allocate(dir, stem, suffix, ext, |_| false);
        assert_eq!(
            allocate("clip (1)", "", Some("mp4")),
            dir.join("clip (1).mp4")
        );
        assert_eq!(allocate("clip", "", Some("mp4")), dir.join("clip.mp4"));
        assert_eq!(allocate("cli", "p", Some("mp4")), dir.join("clip (3).mp4"));
        assert_eq!(allocate("clip", "", Some("png")), dir.join("clip.png"));
        assert_eq!(allocate("clip", "", None), dir.join("clip"));
        assert_eq!(
            allocate("clip (3)", "", Some("mp4")),
            dir.join("clip (3) (1).mp4")
        );
        assert_eq!(allocate("clip", "", Some("mp4")), dir.join("clip (4).mp4"));
        assert_eq!(
            allocator.allocate(Path::new("/other"), "clip", "", Some("mp4"), |_| false),
            Path::new("/other/clip.mp4")
        );
    }

    #[test]
    fn capture_names_include_time_and_handle_collisions() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-30T09:05:07.123+02:00")
            .unwrap()
            .with_timezone(&chrono::Local);
        for prefix in ["screen_recording", "screenshot"] {
            let stem = capture_stem(prefix, at);
            assert_eq!(
                stem,
                format!("{prefix}_{}", at.format("%Y-%m-%d_%H-%M-%S%.3f"))
            );
            assert!(stem.ends_with(".123"));
            assert!(!stem.contains(':'));
            let mut claimed = HashSet::new();
            let dir = Path::new("/captures");
            let existing = dir.join(format!("{stem}.mp4"));
            let first =
                allocate_output(dir, &stem, "", Some("mp4"), &mut claimed, |p| p == existing);
            let second =
                allocate_output(dir, &stem, "", Some("mp4"), &mut claimed, |p| p == existing);
            assert_eq!(first, dir.join(format!("{stem} (1).mp4")));
            assert_eq!(second, dir.join(format!("{stem} (2).mp4")));
        }
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
    fn subtitle_matching_resolves_each_job_and_survives_serialization() {
        let root = tempfile::tempdir().unwrap();
        let subdir = root.path().join("season");
        std::fs::create_dir(&subdir).unwrap();
        let videos = vec![root.path().join("one.mp4"), subdir.join("two.mkv")];
        let subtitles = [root.path().join("one.SRT"), subdir.join("two.ass")];
        for path in videos.iter().chain(subtitles.iter()) {
            std::fs::write(path, b"").unwrap();
        }
        let mut opts = Options::default();
        opts.edit.match_subtitles = true;
        opts.edit.subtitle = Some(root.path().join("manual.srt"));
        let jobs = plan_jobs(
            &select(Tool::AddSubtitles, &videos, None, &opts),
            &mut HashSet::new(),
        )
        .unwrap();
        let restored: Vec<JobSpec> =
            serde_json::from_str(&serde_json::to_string(&jobs).unwrap()).unwrap();
        assert_eq!(restored.len(), 2);
        for (job, subtitle) in restored.iter().zip(subtitles) {
            assert_eq!(job.options.edit.subtitle.as_ref(), Some(&subtitle));
            assert!(!job.options.edit.match_subtitles);
            job.validate().unwrap();
            let input = crate::plan::Input {
                path: job.inputs[0].clone(),
                info: crate::probe::MediaInfo {
                    has_video: true,
                    ..Default::default()
                },
            };
            let plan = crate::plan::build(job.tool, &[input], &job.output, &job.options).unwrap();
            assert!(plan.args.contains(&crate::plan::input_url(&subtitle, None)));
        }
    }

    #[test]
    fn subtitle_matching_rejects_missing_and_ambiguous_without_claiming_outputs() {
        let root = tempfile::tempdir().unwrap();
        let videos = vec![root.path().join("one.mp4"), root.path().join("two.mp4")];
        std::fs::write(root.path().join("one.srt"), b"").unwrap();
        // A different case and a language suffix do not match the stem.
        std::fs::write(root.path().join("Two.srt"), b"").unwrap();
        std::fs::write(root.path().join("two.en.srt"), b"").unwrap();
        let mut opts = Options::default();
        opts.edit.match_subtitles = true;
        let mut claimed = HashSet::new();
        assert!(
            matches!(plan_jobs(&select(Tool::AddSubtitles, &videos, None, &opts), &mut claimed), Err(JobError::SubtitleMatchMissing(path)) if path == videos[1])
        );
        assert!(claimed.is_empty());
        // Remove the case-mismatch fixture first: on case-insensitive filesystems,
        // writing `two.srt` over `Two.srt` preserves the original directory name.
        std::fs::remove_file(root.path().join("Two.srt")).unwrap();
        std::fs::write(root.path().join("two.srt"), b"").unwrap();
        std::fs::write(root.path().join("two.vtt"), b"").unwrap();
        assert!(
            matches!(plan_jobs(&select(Tool::AddSubtitles, &videos, None, &opts), &mut claimed), Err(JobError::SubtitleMatchAmbiguous(path)) if path == videos[1])
        );
        assert!(claimed.is_empty());
    }

    #[test]
    fn manual_subtitle_is_shared_by_all_jobs() {
        let videos = vec![PathBuf::from("/a/one.mp4"), PathBuf::from("/a/two.mp4")];
        let mut opts = Options::default();
        opts.edit.subtitle = Some(PathBuf::from("/subs.srt"));
        let jobs = plan_jobs(
            &select(Tool::AddSubtitles, &videos, None, &opts),
            &mut HashSet::new(),
        )
        .unwrap();
        assert_eq!(jobs.len(), 2);
        assert!(
            jobs.iter()
                .all(|job| job.options.edit.subtitle == opts.edit.subtitle)
        );
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

    #[test]
    fn plan_jobs_rejects_invalid_inputs_and_options() {
        let files = vec![PathBuf::from("/a/x.mkv")];
        let mut opts = Options::default();
        opts.edit.trim_start = "vmm".into();
        let err = plan_jobs(
            &select(Tool::Trim, &files, None, &opts),
            &mut HashSet::new(),
        );
        assert!(matches!(
            err,
            Err(JobError::Plan(PlanError::BadTimestamp(_)))
        ));
    }

    #[test]
    fn job_spec_validate_catches_evil_inputs() {
        let mut spec = JobSpec {
            tool: Tool::Trim,
            inputs: vec![PathBuf::from("/a/x.mkv")],
            url: None,
            output: PathBuf::from("/a/x_trim.mkv"),
            options: Options::default(),
        };
        assert!(spec.validate().is_ok());

        spec.options.edit.trim_start = "vmm".into();
        assert!(spec.validate().is_err());

        spec.options.edit.trim_start.clear();
        spec.inputs = vec![PathBuf::from("/a/x\0evil.mkv")];
        assert!(spec.validate().is_err());

        spec.inputs = vec![PathBuf::from("/a/x.mkv")];
        spec.output = PathBuf::from("/a/x\0out.mkv");
        assert!(spec.validate().is_err());

        spec.output = PathBuf::from("/a/out.mkv");
        spec.tool = Tool::Download;
        spec.url = Some("not-a-url".into());
        assert!(spec.validate().is_err());

        spec.url = Some("https://example.com/video.mp4".into());
        assert!(spec.validate().is_ok());
    }
}

#[cfg(test)]
mod inspection_tests {
    use super::*;
    #[test]
    fn inspection_has_no_output_and_cannot_allocate_a_job() {
        let opts = Options::default();
        assert_eq!(output_kind(Tool::MediaInfo, None, &opts), OutputKind::None);
        let paths = [PathBuf::from("clip.mp4")];
        let selection = Selection {
            tool: Tool::MediaInfo,
            files: &paths,
            url: "",
            output_dir: None,
            options: &opts,
        };
        let mut claimed = HashSet::new();
        assert!(matches!(
            plan_jobs(&selection, &mut claimed),
            Err(JobError::Plan(PlanError::UnsupportedTool))
        ));
        assert!(claimed.is_empty());
        assert_eq!(
            output_kind(
                Tool::RemoveImageMetadata,
                Some(Path::new("photo.jpg")),
                &opts
            ),
            OutputKind::File("png")
        );
        assert!(Tool::RemoveImageMetadata.requires().is_empty());
    }
}
