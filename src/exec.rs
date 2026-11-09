//! Executes one job on the calling (worker) thread.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::capture;
use crate::catalog::{AudioFormat, EbookFormat, Tool};
use crate::cso;
use crate::disc;
use crate::docs;
use crate::download;
use crate::ebook;
use crate::externals::{External, Toolbox};
use crate::ffmpeg;
use crate::i18n::{tr, trf};
use crate::imaging;
use crate::jobs::{JobError, JobSpec};
use crate::office;
use crate::options::{RecordArea, RecordOptions, ScreenRect, ScreenshotOptions};
use crate::pdfgen;
use crate::plan::{self, BLURAY_PROTOCOL, CD_DEMUXER, Capture, DVD_DEMUXER, Input, PlanError};
use crate::process::{self, RunError, StopMode, Watch};
use crate::sheet;

/// Runs a job to completion, reporting progress in `0.0..=1.0`.
#[cfg(test)]
pub fn execute(
    spec: &JobSpec,
    toolbox: &Toolbox,
    cancel: &AtomicBool,
    on_progress: impl FnMut(f32),
) -> Result<(), JobError> {
    execute_with_detail(spec, toolbox, cancel, on_progress, |_| {})
}

/// Like [`execute`], also reporting a line of detail such as download speed.
pub fn execute_with_detail(
    spec: &JobSpec,
    toolbox: &Toolbox,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
    mut on_detail: impl FnMut(String),
) -> Result<(), JobError> {
    if spec.tool == Tool::MediaInfo {
        return Err(PlanError::UnsupportedTool.into());
    }
    if let Some(missing) = toolbox.missing(spec.tool.requires()) {
        return Err(JobError::Missing(missing));
    }
    if spec.tool == Tool::Download {
        // Downloads intentionally add files to an existing user-owned folder.
        let result = run_tool(spec, toolbox, cancel, &mut on_progress, &mut on_detail);
        if result.is_ok() {
            on_progress(1.0);
        }
        return result;
    }
    let parent = spec
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let staging = tempfile::Builder::new()
        .prefix(".fmx-job-")
        .tempdir_in(parent)
        .map_err(JobError::Io)?;
    let name = spec.output.file_name().ok_or(JobError::NoOutput)?;
    let mut staged = spec.clone();
    staged.output = staging.path().join(name);
    // Only the private staging folder is removed on failure or cancellation.
    run_tool(&staged, toolbox, cancel, &mut on_progress, &mut on_detail)?;
    if !staged.output.exists() {
        return Err(JobError::NoOutput);
    }
    // Include companion files produced by tools such as pdftohtml.
    for entry in std::fs::read_dir(staging.path()).map_err(JobError::Io)? {
        let entry = entry.map_err(JobError::Io)?;
        publish_output(&entry.path(), &parent.join(entry.file_name())).map_err(JobError::Io)?;
    }
    on_progress(1.0);
    Ok(())
}

/// Publishes without replacing an existing destination, including dangling links.
/// A publish error can leave completed files, but never deletes destination data.
fn publish_output(source: &Path, target: &Path) -> std::io::Result<()> {
    let kind = std::fs::symlink_metadata(source)?.file_type();
    if kind.is_file() {
        // Staging is on the same filesystem. Linking is atomic and fails if the
        // destination exists; unlike rename, it cannot overwrite another file.
        match std::fs::hard_link(source, target) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
            // FAT/exFAT and some network filesystems cannot create hard links.
            Err(_) => copy_output_exclusive(source, target),
        }
    } else if kind.is_dir() {
        std::fs::create_dir(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            publish_output(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "conversion produced a symlink or special file",
        ))
    }
}

fn copy_output_exclusive(source: &Path, target: &Path) -> std::io::Result<()> {
    let mut input = std::fs::File::open(source)?;
    // Exclusive creation also rejects dangling links and late collisions.
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()
}

fn need(toolbox: &Toolbox, ext: External) -> Result<&Path, JobError> {
    toolbox.get(ext).ok_or(JobError::Missing(ext))
}

fn first_input(spec: &JobSpec) -> Result<&Path, JobError> {
    spec.inputs
        .first()
        .map(PathBuf::as_path)
        .ok_or(JobError::Plan(PlanError::WrongInputCount {
            needed: 1,
            got: 0,
        }))
}

/// Records the screen until cancelled, which ends the recording normally.
fn record_screen(
    out: &Path,
    rec: &RecordOptions,
    ffmpeg: &Path,
    cancel: &AtomicBool,
    on_detail: &mut dyn FnMut(String),
) -> Result<(), JobError> {
    let devices = capture::detect(Some(ffmpeg));
    let sounds = devices.sound_inputs(rec.audio);
    #[cfg(target_os = "linux")]
    if capture::is_wayland() {
        use crate::screencast::{self, Cast};
        const GST_LAUNCH: &str = "gst-launch-1.0";
        if !in_path(GST_LAUNCH) {
            return Err(JobError::Recorder(tr("record.needs_gstreamer").to_owned()));
        }
        on_detail(tr("record.choose_screen").to_owned());
        let cursor = rec.cursor;
        let cast = ask_portal(cancel, move || Cast::start(cursor))?;
        countdown(record_wait(rec), rec.minimize, cancel, on_detail)?;
        let frames = cast
            .spawn_frames(Path::new(GST_LAUNCH), rec.fps)
            .map_err(|e| JobError::Run(RunError::Spawn(GST_LAUNCH.to_owned(), e)))?;
        let plan = plan::record(out, rec, &Capture::Stdin, &sounds);
        let result = process::run_fed(
            frames,
            screencast::interrupt,
            ffmpeg,
            &plan.args,
            cancel,
            recording_lines(Some(rec.fps), on_detail),
        );
        drop::<Cast>(cast);
        return result.map_err(JobError::Run);
    }
    let capture = screen_capture(rec.area, rec.region, &devices)?;
    countdown(record_wait(rec), rec.minimize, cancel, on_detail)?;
    let plan = plan::record(out, rec, &capture, &sounds);
    process::run(
        ffmpeg,
        &plan.args,
        plan.stop,
        cancel,
        recording_lines(Some(rec.fps), on_detail),
    )
    .map_err(JobError::Run)
}

/// Where the picture comes from outside Wayland, for the chosen area.
fn screen_capture(
    choice: RecordArea,
    region: ScreenRect,
    devices: &capture::Devices,
) -> Result<Capture, JobError> {
    let monitor = match choice {
        RecordArea::Monitor(i) => devices.monitors.get(i as usize),
        _ => None,
    };
    let area = match choice {
        RecordArea::Screen => None,
        RecordArea::Monitor(_) => monitor.and_then(|m| m.rect),
        RecordArea::Region => Some(region),
    };
    Ok(if cfg!(windows) {
        Capture::Desktop { area }
    } else if cfg!(target_os = "macos") {
        // AVFoundation captures one screen at a time: the chosen or the first.
        let device = monitor
            .or(devices.monitors.first())
            .and_then(|m| m.device.clone())
            .ok_or_else(|| JobError::Recorder(tr("record.no_screen").to_owned()))?;
        Capture::MacScreen { device, area }
    } else {
        Capture::X11 {
            display: std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_owned()),
            area,
        }
    })
}

/// Takes one picture of the screen after the chosen delay.
fn take_screenshot(
    out: &Path,
    shot: &ScreenshotOptions,
    ffmpeg: &Path,
    cancel: &AtomicBool,
    on_detail: &mut dyn FnMut(String),
) -> Result<(), JobError> {
    // Minimizing takes a moment; FMX must be gone from the picture.
    let wait = if shot.minimize {
        shot.delay_s.max(1)
    } else {
        shot.delay_s
    };
    #[cfg(target_os = "linux")]
    if capture::is_wayland() {
        countdown(wait, shot.minimize, cancel, on_detail)?;
        use crate::screencast;
        let interactive = shot.area == RecordArea::Region;
        let saved = match ask_portal(cancel, move || screencast::screenshot(interactive)) {
            // GNOME lets apps outside a sandbox take screenshots only through
            // its own capture tool.
            Err(JobError::Recorder(_)) if !interactive => {
                ask_portal(cancel, || screencast::screenshot(true))?
            }
            result => result?,
        };
        return keep_screenshot(&saved, out, ffmpeg, cancel);
    }
    let devices = capture::detect(Some(ffmpeg));
    let capture = screen_capture(shot.area, shot.region, &devices)?;
    countdown(wait, shot.minimize, cancel, on_detail)?;
    let plan = plan::screenshot(out, shot, &capture);
    process::run(ffmpeg, &plan.args, plan.stop, cancel, |_| {}).map_err(JobError::Run)
}

/// Moves the portal's PNG to `out`, converting it when another format was
/// chosen; the portal's copy goes, so the picture is not saved twice.
#[cfg(target_os = "linux")]
fn keep_screenshot(
    saved: &Path,
    out: &Path,
    ffmpeg: &Path,
    cancel: &AtomicBool,
) -> Result<(), JobError> {
    let same_format = saved
        .extension()
        .zip(out.extension())
        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));
    if same_format {
        if std::fs::rename(saved, out).is_err() {
            std::fs::copy(saved, out).map_err(JobError::Io)?;
            let _ = std::fs::remove_file(saved);
        }
        return Ok(());
    }
    let args: Vec<OsString> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(),
        "-loglevel".into(),
        "error".into(),
        "-i".into(),
        saved.into(),
        "-q:v".into(),
        "2".into(),
        out.into(),
    ];
    run_plain(ffmpeg, &args, cancel)?;
    let _ = std::fs::remove_file(saved);
    Ok(())
}

/// Whether a program can be found on `PATH`.
#[cfg(target_os = "linux")]
fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Feeds ffmpeg's progress lines to `on_detail` as "Recording 1:05 · 12.4 MB".
/// `fps` is the frame rate asked of a screen recording, to notice dropped frames.
fn recording_lines(fps: Option<u32>, on_detail: &mut dyn FnMut(String)) -> impl FnMut(&str) + '_ {
    let mut progress = RecordingProgress::new(fps);
    move |line| {
        if let Some(detail) = progress.update(line) {
            on_detail(detail);
        }
    }
}

/// Runs a desktop portal request (which may wait on the user in a system
/// dialog) on its own thread, giving up if the job is stopped meanwhile.
#[cfg(target_os = "linux")]
fn ask_portal<T: Send + 'static>(
    cancel: &AtomicBool,
    request: impl FnOnce() -> Result<T, crate::screencast::CastError> + Send + 'static,
) -> Result<T, JobError> {
    use crate::screencast::CastError;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(request());
    });
    let failed = |e: &str| JobError::Recorder(trf("error.screen_share", &[("error", &e)]));
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(JobError::Run(RunError::Cancelled));
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(CastError::Cancelled)) => return Err(JobError::Run(RunError::Cancelled)),
            Ok(Err(CastError::Failed(e))) => return Err(failed(&e)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(failed("")),
        }
    }
}

/// Set when a recording wants FMX's window out of the way.
static MINIMIZE_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Whether a recording asked to minimize the window since the last call.
pub fn take_minimize_request() -> bool {
    MINIMIZE_REQUESTED.swap(false, Ordering::Relaxed)
}

/// Seconds a recording waits before it starts: three to get ready, if
/// wanted or if FMX minimizes first.
fn record_wait(rec: &RecordOptions) -> u32 {
    if rec.countdown || rec.minimize { 3 } else { 0 }
}

/// Counts `seconds` down in the job's detail, minimizing the window first if
/// asked (the detail sent next wakes the window to do it).
fn countdown(
    seconds: u32,
    minimize: bool,
    cancel: &AtomicBool,
    on_detail: &mut dyn FnMut(String),
) -> Result<(), JobError> {
    if minimize {
        MINIMIZE_REQUESTED.store(true, Ordering::Relaxed);
    }
    for left in (1..=seconds).rev() {
        on_detail(trf("record.starting_in", &[("seconds", &left)]));
        for _ in 0..10 {
            if cancel.load(Ordering::Relaxed) {
                return Err(JobError::Run(RunError::Cancelled));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}

/// Seconds of frames compared at a time to tell whether ffmpeg keeps up.
const FRAME_RATE_WINDOW: Duration = Duration::from_secs(2);

/// Below this share of the asked frame rate, the recording is dropping frames.
const KEEPING_UP: f64 = 0.8;

struct RecordingProgress {
    /// When the recording's first second began, on the clock on the wall.
    /// ffmpeg's own time is that of the frames written, which trails behind
    /// while the encoder works and stutters when frames come unevenly.
    started: Option<Instant>,
    fps: Option<f64>,
    secs: f64,
    bytes: f64,
    /// Frames read from the screen: written, less repeats, plus discards.
    frames: Option<f64>,
    dup: f64,
    drop: f64,
    /// The frame count and time the current measuring window began.
    window: Option<(Instant, f64)>,
    behind: bool,
}

impl RecordingProgress {
    fn new(fps: Option<u32>) -> Self {
        Self {
            started: None,
            fps: fps.map(f64::from),
            secs: 0.0,
            bytes: 0.0,
            frames: None,
            dup: 0.0,
            drop: 0.0,
            window: None,
            behind: false,
        }
    }

    fn update(&mut self, line: &str) -> Option<String> {
        self.update_at(line, Instant::now())
    }

    fn update_at(&mut self, line: &str, now: Instant) -> Option<String> {
        let number = |prefix: &str| line.strip_prefix(prefix)?.trim().parse::<f64>().ok();
        if let Some(secs) = ffmpeg::progress_seconds(line) {
            self.secs = secs.max(0.0);
        } else if let Some(bytes) = number("total_size=") {
            self.bytes = bytes;
        } else if let Some(frames) = number("frame=") {
            self.frames = Some(frames);
        } else if let Some(dup) = number("dup_frames=") {
            self.dup = dup;
        } else if let Some(drop) = number("drop_frames=") {
            self.drop = drop;
        } else if line.starts_with("progress=") {
            let started = *self
                .started
                .get_or_insert_with(|| now - Duration::from_secs_f64(self.secs));
            self.check_frame_rate(now);
            let detail = trf(
                "record.recording",
                &[
                    ("time", &download::clock((now - started).as_secs_f64())),
                    ("size", &download::human_bytes(self.bytes)),
                ],
            );
            return Some(if self.behind {
                format!("{detail} · {}", tr("record.dropping_frames"))
            } else {
                detail
            });
        }
        None
    }

    /// Compares the frames read over the last window with the frame rate
    /// asked for; the verdict holds until the next window ends.
    fn check_frame_rate(&mut self, now: Instant) {
        let (Some(fps), Some(written)) = (self.fps, self.frames) else {
            return;
        };
        let read = written - self.dup + self.drop;
        match self.window {
            None => self.window = Some((now, read)),
            Some((since, at_start)) => {
                let span = (now - since).as_secs_f64();
                if span >= FRAME_RATE_WINDOW.as_secs_f64() {
                    self.behind = (read - at_start) / span < fps * KEEPING_UP;
                    self.window = Some((now, read));
                }
            }
        }
    }
}

/// Runs an external program that reports no progress.
fn run_plain(program: &Path, args: &[OsString], cancel: &AtomicBool) -> Result<(), JobError> {
    process::run(program, args, StopMode::Kill, cancel, |_| {}).map_err(JobError::Run)
}

fn run_tool(
    spec: &JobSpec,
    toolbox: &Toolbox,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32),
    on_detail: &mut dyn FnMut(String),
) -> Result<(), JobError> {
    let opts = &spec.options;
    let out = spec.output.as_path();
    let ffmpeg_path = toolbox.get(External::Ffmpeg);
    let run = |external, args: &[OsString]| run_plain(need(toolbox, external)?, args, cancel);
    match spec.tool {
        Tool::ToImage(format) => {
            imaging::convert(first_input(spec)?, out, format, opts, ffmpeg_path)
                .map_err(JobError::Image)
        }
        Tool::RemoveImageMetadata => {
            imaging::remove_metadata(first_input(spec)?, out).map_err(JobError::Image)
        }
        Tool::JoinImages => imaging::join(
            &spec.inputs,
            out,
            opts.join_layout,
            opts,
            ffmpeg_path,
            on_progress,
        )
        .map_err(JobError::Image),
        Tool::DiscToIso => {
            disc::copy_image(first_input(spec)?, out, cancel, on_progress).map_err(JobError::Cso)
        }
        Tool::PdfToExcel => pdf_to_excel(spec, toolbox, cancel),
        Tool::ToAudio(AudioFormat::Ape) => to_ape(spec, toolbox, cancel, on_progress),
        Tool::ImagesToPdf => {
            imaging::images_to_pdf(&spec.inputs, out, opts, ffmpeg_path, on_progress)
                .map_err(JobError::Image)
        }
        Tool::IsoToCso => {
            cso::compress(first_input(spec)?, out, opts.cso_level, cancel, on_progress)
                .map_err(JobError::Cso)
        }
        Tool::CsoToIso => {
            cso::decompress(first_input(spec)?, out, cancel, on_progress).map_err(JobError::Cso)
        }
        Tool::PdfToImages => {
            std::fs::create_dir_all(out).map_err(JobError::Io)?;
            let args =
                docs::pdf_to_images(first_input(spec)?, out, opts.pdf_dpi, opts.pdf_image_format);
            run(External::Pdftoppm, &args)
        }
        Tool::PdfToText => run(
            External::Pdftotext,
            &docs::pdf_to_text(first_input(spec)?, out),
        ),
        Tool::PdfToHtml => run(
            External::Pdftohtml,
            &docs::pdf_to_html(first_input(spec)?, out),
        ),
        Tool::PdfMerge => run(External::Pdfunite, &docs::pdf_merge(&spec.inputs, out)),
        Tool::PdfSplit => {
            std::fs::create_dir_all(out).map_err(JobError::Io)?;
            run(
                External::Pdfseparate,
                &docs::pdf_split(first_input(spec)?, out),
            )
        }
        Tool::PdfCompress => run(
            External::Ghostscript,
            &docs::pdf_compress(first_input(spec)?, out, opts.quality),
        ),
        Tool::OfficeToPdf => office_to_pdf(spec, toolbox, cancel),
        Tool::PdfToWord => pdf_to_word(spec, toolbox, cancel),
        Tool::ToEbook(format) => ebook_convert(spec, format, toolbox, cancel),
        Tool::FolderToIso => {
            let folder = first_input(spec)?;
            let is_dvd = folder.join("VIDEO_TS").is_dir();
            let args = docs::folder_to_iso(folder, out, &opts.iso_label, is_dvd);
            process::run_watching(
                need(toolbox, External::Xorriso)?,
                &args,
                StopMode::Kill,
                Watch::Stderr,
                cancel,
                |line| {
                    if let Some(p) = docs::xorriso_progress(line) {
                        on_progress(p);
                    }
                },
            )
            .map_err(JobError::Run)
        }
        Tool::Download => {
            let url = spec
                .url
                .as_deref()
                .filter(|u| docs::is_web_url(u))
                .ok_or(JobError::BadUrl)?;
            let ffmpeg = need(toolbox, External::Ffmpeg)?;
            let args = download::args(url, out, ffmpeg, &opts.download).map_err(|key| {
                JobError::Plan(crate::plan::PlanError::InvalidValue(tr(key).to_owned()))
            })?;
            std::fs::create_dir_all(out).map_err(JobError::Io)?;
            process::run(
                need(toolbox, External::YtDlp)?,
                &args,
                StopMode::Kill,
                cancel,
                |line| {
                    if let Some(progress) = download::parse_progress(line) {
                        if let Some(fraction) = progress.fraction {
                            on_progress(fraction);
                        }
                        on_detail(progress.detail);
                    }
                },
            )
            .map_err(|e| match e {
                RunError::Failed { .. } => JobError::DownloadFailed(e),
                e => JobError::Run(e),
            })
        }
        Tool::Screenshot => {
            take_screenshot(
                out,
                &opts.screenshot,
                need(toolbox, External::Ffmpeg)?,
                cancel,
                on_detail,
            )?;
            if out.is_file() {
                Ok(())
            } else {
                Err(JobError::NoOutput)
            }
        }
        Tool::RecordAudio => {
            let ffmpeg = need(toolbox, External::Ffmpeg)?;
            let rec = &spec.options.sound_record;
            let devices = capture::detect(Some(ffmpeg));
            let sounds = devices.sound_inputs(rec.audio);
            let expected = usize::from(rec.audio.microphone()) + usize::from(rec.audio.system());
            if sounds.is_empty() || sounds.len() != expected {
                return Err(JobError::Recorder(tr("record.no_audio").to_owned()));
            }
            countdown(record_wait(rec), false, cancel, on_detail)?;
            let plan = plan::record_audio(out, &spec.options, &sounds);
            process::run(
                ffmpeg,
                &plan.args,
                plan.stop,
                cancel,
                recording_lines(None, on_detail),
            )
            .map_err(JobError::Run)?;
            if out.is_file() {
                Ok(())
            } else {
                Err(JobError::NoOutput)
            }
        }
        Tool::Record => {
            record_screen(
                out,
                &opts.record,
                need(toolbox, External::Ffmpeg)?,
                cancel,
                on_detail,
            )?;
            if out.is_file() {
                Ok(())
            } else {
                Err(JobError::NoOutput)
            }
        }
        Tool::CdToAudio => rip_cd(spec, need(toolbox, External::Ffmpeg)?, cancel, on_progress),
        tool => {
            if tool == Tool::EditTags
                && let Some(cover) = &opts.media_tools.cover
            {
                crate::tagging::validate_cover(cover)?;
            }
            let ffmpeg = need(toolbox, External::Ffmpeg)?;
            let demuxer = match tool {
                Tool::DvdToVideo => Some(DVD_DEMUXER),
                Tool::BluRayToVideo => Some(BLURAY_PROTOCOL),
                _ => None,
            };
            let inputs = spec
                .inputs
                .iter()
                .map(|path| {
                    let probe_path = if tool == Tool::DvdToVideo {
                        plan::dvd_root(path)
                    } else {
                        path.clone()
                    };
                    let info =
                        ffmpeg::probe(ffmpeg, &probe_path, demuxer).map_err(JobError::Run)?;
                    Ok(Input {
                        path: path.clone(),
                        info,
                    })
                })
                .collect::<Result<Vec<_>, JobError>>()?;
            let plan = plan::build(tool, &inputs, out, opts)?;
            if matches!(tool, Tool::ExtractFrames | Tool::Split) {
                std::fs::create_dir_all(out).map_err(JobError::Io)?;
            }
            ffmpeg::run(ffmpeg, &plan, cancel, on_progress).map_err(JobError::Run)?;
            if tool == Tool::EditTags && ffmpeg::sibling_ffprobe(ffmpeg).is_some() {
                let saved = ffmpeg::probe(ffmpeg, out, None).map_err(JobError::Run)?;
                crate::tagging::verify_saved(&inputs[0].info, &saved, &opts.media_tools)?;
            }
            Ok(())
        }
    }
}

/// Renders a document to PDF natively. Old binary .doc/.ppt files need
/// LibreOffice, which is used only if it happens to be installed.
fn office_to_pdf(spec: &JobSpec, toolbox: &Toolbox, cancel: &AtomicBool) -> Result<(), JobError> {
    let input = first_input(spec)?;
    let ext = office::extension(input);
    if !office::NATIVE_EXTENSIONS.contains(&ext.as_str())
        && toolbox.get(External::Soffice).is_some()
    {
        return office(spec, toolbox, cancel);
    }
    let blocks = office::read_document(input).map_err(JobError::Office)?;
    pdfgen::render(&blocks, &spec.output).map_err(|e| match e {
        pdfgen::PdfError::Io(e) => JobError::Io(e),
        pdfgen::PdfError::Font => JobError::Io(std::io::Error::other("font")),
    })
}

/// Converts natively when FMX reads the input and writes the format (see
/// `ebook`), and with Calibre otherwise.
fn ebook_convert(
    spec: &JobSpec,
    format: EbookFormat,
    toolbox: &Toolbox,
    cancel: &AtomicBool,
) -> Result<(), JobError> {
    let input = first_input(spec)?;
    if format.is_native() && ebook::can_read(input) {
        let book = ebook::read(input).map_err(JobError::Office)?;
        let name = input
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        return ebook::write(&book, format, &name, &spec.output).map_err(JobError::Io);
    }
    run_plain(
        need(toolbox, External::EbookConvert)?,
        &docs::ebook_convert(input, &spec.output),
        cancel,
    )
}

/// Extracts laid-out PDF text for Word and Excel conversion.
fn pdf_text(spec: &JobSpec, toolbox: &Toolbox, cancel: &AtomicBool) -> Result<String, JobError> {
    let input = first_input(spec)?;
    let parent = spec.output.parent().unwrap_or(Path::new("."));
    let text_file = tempfile::Builder::new()
        .suffix(".txt")
        .tempfile_in(parent)
        .map_err(JobError::Io)?;
    run_plain(
        need(toolbox, External::Pdftotext)?,
        &docs::pdf_to_text(input, text_file.path()),
        cancel,
    )?;
    std::fs::read_to_string(text_file.path()).map_err(JobError::Io)
}

/// Lays out the PDF's text with Poppler and writes it as a Word document.
fn pdf_to_word(spec: &JobSpec, toolbox: &Toolbox, cancel: &AtomicBool) -> Result<(), JobError> {
    let text = pdf_text(spec, toolbox, cancel)?;
    office::write_docx(&office::text_pages(&text), &spec.output).map_err(JobError::Office)
}

/// Lays out the PDF's text with Poppler, then writes one sheet per page.
fn pdf_to_excel(spec: &JobSpec, toolbox: &Toolbox, cancel: &AtomicBool) -> Result<(), JobError> {
    let text = pdf_text(spec, toolbox, cancel)?;
    let parent = spec.output.parent().unwrap_or(Path::new("."));
    let pages = sheet::parse_pages(&text);
    let staging = tempfile::Builder::new()
        .suffix(".xlsx")
        .tempfile_in(parent)
        .map_err(JobError::Io)?;
    sheet::write_xlsx(&pages, staging.path(), |n| {
        crate::i18n::trf("sheet.page", &[("n", &n)])
    })
    .map_err(|e| JobError::Io(std::io::Error::other(e.to_string())))?;
    staging
        .persist(&spec.output)
        .map_err(|e| JobError::Io(e.error))?;
    Ok(())
}

/// Decodes to WAV with ffmpeg, then compresses with Monkey's Audio.
fn to_ape(
    spec: &JobSpec,
    toolbox: &Toolbox,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32),
) -> Result<(), JobError> {
    let ffmpeg = need(toolbox, External::Ffmpeg)?;
    let mac = need(toolbox, External::MonkeysAudio)?;
    let input = first_input(spec)?;
    let parent = spec.output.parent().unwrap_or(Path::new("."));
    let wav = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile_in(parent)
        .map_err(JobError::Io)?;
    let info = ffmpeg::probe(ffmpeg, input, None).map_err(JobError::Run)?;
    let plan = plan::build(
        Tool::ToAudio(AudioFormat::Wav),
        &[Input {
            path: input.to_path_buf(),
            info,
        }],
        wav.path(),
        &spec.options,
    )?;
    // Decoding is most of the work; compression is the last tenth.
    ffmpeg::run(ffmpeg, &plan, cancel, |p| on_progress(p * 0.9)).map_err(JobError::Run)?;
    let level = spec.options.quality.pick("-c4000", "-c2000", "-c1000");
    let args: Vec<OsString> = vec![wav.path().into(), spec.output.clone().into(), level.into()];
    run_plain(mac, &args, cancel)
}

fn office(spec: &JobSpec, toolbox: &Toolbox, cancel: &AtomicBool) -> Result<(), JobError> {
    let input = first_input(spec)?;
    let soffice = need(toolbox, External::Soffice)?;
    let parent = spec.output.parent().unwrap_or(Path::new("."));
    let staging = tempfile::tempdir_in(parent).map_err(JobError::Io)?;
    let profile = tempfile::tempdir().map_err(JobError::Io)?;
    let args = docs::office_to_pdf(input, staging.path(), profile.path());
    run_plain(soffice, &args, cancel)?;
    let ext = "pdf";
    let stem = input.file_stem().ok_or(JobError::NoOutput)?;
    let produced = staging.path().join(stem).with_extension(ext);
    if !produced.is_file() {
        return Err(JobError::NoOutput);
    }
    std::fs::rename(&produced, &spec.output).map_err(JobError::Io)
}

/// Rips each track of an audio CD into its own file.
fn rip_cd(
    spec: &JobSpec,
    ffmpeg: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32),
) -> Result<(), JobError> {
    let device = first_input(spec)?;
    let info = ffmpeg::probe(ffmpeg, device, Some(CD_DEMUXER)).map_err(JobError::Run)?;
    if info.chapters.is_empty() {
        return Err(PlanError::NoTracks.into());
    }
    std::fs::create_dir_all(&spec.output).map_err(JobError::Io)?;
    let total: f64 = info.chapters.iter().map(|(s, e)| e - s).sum();
    let mut done = 0.0;
    let ext = spec.options.cd_audio_format.ext();
    for (n, &track) in info.chapters.iter().enumerate() {
        let output = spec.output.join(format!("Track {:02}.{ext}", n + 1));
        let plan = plan::cd_track(device, track, &output, &spec.options);
        let length = track.1 - track.0;
        ffmpeg::run(ffmpeg, &plan, cancel, |p| {
            on_progress(((done + f64::from(p) * length) / total) as f32);
        })
        .map_err(JobError::Run)?;
        done += length;
    }
    Ok(())
}

/// End-to-end runs against real programs. Skipped unless `FMX_FFMPEG` points
/// at ffmpeg: `FMX_FFMPEG=/usr/bin/ffmpeg cargo test e2e -- --nocapture`.
/// Document/ISO tools run when their programs are on `PATH`.
#[cfg(test)]
mod e2e {
    use super::*;
    use crate::catalog::{AudioFormat, ImageFormat, VideoFormat};
    use crate::jobs::{Selection, plan_jobs};
    use crate::options::{Corner, FrameMode, Options, RecordAudio, Rect, ShotFormat};
    use image::GenericImageView;
    use std::collections::HashSet;
    use std::process::Command;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    #[test]
    fn reversal_rejects_compressed_audio_exceeding_memory_budget() {
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("constant.flac");
        let status = Command::new(ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=0.25|0.25:s=192000:d=1800",
                "-c:a",
                "flac",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(std::fs::metadata(&source).unwrap().len() < 10 * 1024 * 1024);
        for tool in [Tool::Reverse, Tool::TrimSilence] {
            let output = dir.path().join(format!("{tool:?}.mkv"));
            let input = Input {
                path: source.clone(),
                // Deliberately false metadata: the runtime limit must still work.
                info: crate::probe::MediaInfo {
                    has_audio: true,
                    duration_s: Some(0.01),
                    ..Default::default()
                },
            };
            let plan = plan::build(tool, &[input], &output, &Options::default()).unwrap();
            let started = Instant::now();
            let result = ffmpeg::run(ffmpeg, &plan, &AtomicBool::new(false), |_| {});
            assert!(
                matches!(result, Err(RunError::Failed { .. })),
                "{tool:?}: {result:?}"
            );
            assert!(started.elapsed() < Duration::from_secs(30));
        }
    }

    #[test]
    fn media_tools_execute_with_valid_streams_and_metadata() {
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).unwrap();
        let fx = fixture(ffmpeg);
        let out = fx.root.join("media-tools");
        std::fs::create_dir(&out).unwrap();
        let mut claimed = HashSet::new();
        let opts = Options::default();
        for (tool, source, video, audio) in [
            (Tool::Reverse, &fx.video, true, true),
            (Tool::Reverse, &fx.audio, false, true),
            (Tool::Fade, &fx.video, true, true),
            (Tool::Fade, &fx.audio, false, true),
            (Tool::Stabilize, &fx.video, true, true),
            (Tool::Denoise, &fx.video, true, true),
            (Tool::Sharpen, &fx.video, true, true),
            (Tool::AudioDenoise, &fx.video, false, true),
            (Tool::TrimSilence, &fx.audio, false, true),
            (Tool::Karaoke, &fx.video, false, true),
            (Tool::ThumbnailSheet, &fx.video, true, false),
            (Tool::WaveformVideo, &fx.audio, true, true),
        ] {
            let paths = run(
                &tb,
                tool,
                std::slice::from_ref(source),
                &opts,
                &out,
                &mut claimed,
            )
            .unwrap_or_else(|e| panic!("{tool:?}: {e}"));
            let info = ffmpeg::probe(ffmpeg, &paths[0], None).unwrap();
            assert_eq!((info.has_video, info.has_audio), (video, audio), "{tool:?}");
            if tool == Tool::ThumbnailSheet {
                let sheet = image::open(&paths[0]).unwrap();
                assert_eq!(sheet.width(), (240 + 4) * 4 + 4);
                assert_eq!(sheet.height(), (135 + 4) * 3 + 4);
                let pixels = sheet.to_rgb8();
                for row in 0..3 {
                    for col in 0..4 {
                        let tile = image::imageops::crop_imm(
                            &pixels,
                            4 + col * 244,
                            4 + row * 139,
                            240,
                            135,
                        );
                        assert!(
                            tile.pixels()
                                .any(|(_, _, pixel)| pixel.0.iter().any(|v| *v > 80)),
                            "empty tile {col}, {row}"
                        );
                    }
                }
            }
        }
        let silent_video = fx.root.join("silent.mp4");
        generate(
            ffmpeg,
            &silent_video,
            &["-i", fx.video.to_str().unwrap(), "-an", "-c:v", "copy"],
        );
        for tool in [Tool::Reverse, Tool::Fade] {
            let paths = run(
                &tb,
                tool,
                std::slice::from_ref(&silent_video),
                &opts,
                &out,
                &mut claimed,
            )
            .unwrap();
            let info = ffmpeg::probe(ffmpeg, &paths[0], None).unwrap();
            assert!(info.has_video && !info.has_audio);
        }
        let mut spectrum = opts.clone();
        spectrum.media_tools.spectrum = true;
        run(
            &tb,
            Tool::WaveformVideo,
            std::slice::from_ref(&fx.audio),
            &spectrum,
            &out,
            &mut claimed,
        )
        .unwrap();
        // Artwork survives an edit, is replaced on request, and is removable
        // in each supported container. Tags must round-trip Unicode and punctuation.
        let ffprobe = ffmpeg.with_file_name(if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        });
        for format in [AudioFormat::Mp3, AudioFormat::M4a, AudioFormat::Flac] {
            let mut tags = opts.clone();
            tags.media_tools.tag_format = format;
            tags.media_tools.title = "Title: שלום, café [1]".into();
            tags.media_tools.artist = "Artist".into();
            tags.media_tools.album = "Album".into();
            tags.media_tools.cover = Some(fx.logo.clone());
            let added = run(
                &tb,
                Tool::EditTags,
                std::slice::from_ref(&fx.audio),
                &tags,
                &out,
                &mut claimed,
            )
            .unwrap();
            let inspect = |path: &Path| {
                let r = Command::new(&ffprobe)
                    .args([
                        "-v",
                        "error",
                        "-show_format",
                        "-show_streams",
                        "-of",
                        "json",
                    ])
                    .arg(path)
                    .output()
                    .unwrap();
                assert!(r.status.success());
                serde_json::from_slice::<serde_json::Value>(&r.stdout).unwrap()
            };
            let has_cover = |v: &serde_json::Value| {
                v["streams"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["disposition"]["attached_pic"] == 1)
            };
            let first = inspect(&added[0]);
            let loaded = ffmpeg::probe(ffmpeg, &added[0], None)
                .unwrap()
                .tags
                .expect("ffprobe must load tags");
            assert_eq!(loaded.title, tags.media_tools.title);
            assert_eq!(loaded.artist, "Artist");
            assert_eq!(loaded.album, "Album");
            assert!(loaded.has_cover);
            assert_eq!(first["format"]["tags"]["title"], tags.media_tools.title);
            assert!(has_cover(&first), "{format:?}: no artwork");
            tags.media_tools.cover = None;
            tags.media_tools.title.clear();
            tags.media_tools.artist = "Edited artist".into();
            let kept = run(&tb, Tool::EditTags, &added, &tags, &out, &mut claimed).unwrap();
            let kept_info = inspect(&kept[0]);
            assert!(has_cover(&kept_info));
            assert_eq!(
                kept_info["format"]["tags"]["title"],
                "Title: שלום, café [1]"
            );
            assert_eq!(kept_info["format"]["tags"]["artist"], "Edited artist");
            let mut cleared_options = tags.clone();
            cleared_options.media_tools.tag_explicit = true;
            cleared_options.media_tools.tag_write = [true, false, false];
            let cleared = run(
                &tb,
                Tool::EditTags,
                &kept,
                &cleared_options,
                &out,
                &mut claimed,
            )
            .unwrap();
            let cleared_info = inspect(&cleared[0]);
            assert!(cleared_info["format"]["tags"].get("title").is_none());
            assert_eq!(cleared_info["format"]["tags"]["artist"], "Edited artist");
            assert_eq!(cleared_info["format"]["tags"]["album"], "Album");
            assert!(has_cover(&cleared_info));
            let audio_hashes = |path: &Path| {
                let r = Command::new(&ffprobe)
                    .args([
                        "-v",
                        "error",
                        "-select_streams",
                        "a:0",
                        "-show_packets",
                        "-show_data_hash",
                        "sha256",
                        "-of",
                        "json",
                    ])
                    .arg(path)
                    .output()
                    .unwrap();
                assert!(r.status.success());
                let json: serde_json::Value = serde_json::from_slice(&r.stdout).unwrap();
                json["packets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|packet| packet["data_hash"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                audio_hashes(&kept[0]),
                audio_hashes(&cleared[0]),
                "{format:?}: audio was re-encoded"
            );
            tags.media_tools.remove_cover = true;
            let removed = run(&tb, Tool::EditTags, &kept, &tags, &out, &mut claimed).unwrap();
            assert!(!has_cover(&inspect(&removed[0])));
        }
    }

    #[test]
    fn audio_reverse_fade_and_silence_have_expected_content() {
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("tone.wav");
        generate(
            ffmpeg,
            &source,
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=330:duration=3",
                "-c:a",
                "pcm_s16le",
            ],
        );
        let pcm = |path: &Path| {
            let r = Command::new(ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(path)
                .args(["-map", "0:a:0", "-f", "s16le", "-c:a", "pcm_s16le", "-"])
                .output()
                .unwrap();
            assert!(r.status.success());
            r.stdout
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>()
        };
        let mut claimed = HashSet::new();
        let opts = Options::default();
        let reversed = run(
            &tb,
            Tool::Reverse,
            std::slice::from_ref(&source),
            &opts,
            dir.path(),
            &mut claimed,
        )
        .unwrap();
        let original = pcm(&source);
        let mut expected = original.clone();
        expected.reverse();
        assert_eq!(pcm(&reversed[0]), expected);
        let faded = run(
            &tb,
            Tool::Fade,
            std::slice::from_ref(&source),
            &opts,
            dir.path(),
            &mut claimed,
        )
        .unwrap();
        let samples = pcm(&faded[0]);
        let rms = |slice: &[i16]| {
            (slice.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / slice.len() as f64).sqrt()
        };
        assert!(rms(&samples[..4410]) < rms(&samples[60000..64410]) * 0.15);
        assert!(rms(&samples[samples.len() - 4410..]) < rms(&samples[60000..64410]) * 0.15);
        let silence = dir.path().join("silence.wav");
        generate(
            ffmpeg,
            &silence,
            &[
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=if(between(t\\,0.4\\,0.9)+between(t\\,1.2\\,1.7)\\,0.3*sin(2*PI*440*t)\\,0):s=44100:d=2.1",
                "-c:a",
                "pcm_s16le",
            ],
        );
        let mut trim = opts.clone();
        trim.media_tools.silence_duration_s = 0.05;
        let trimmed = run(
            &tb,
            Tool::TrimSilence,
            &[silence],
            &trim,
            dir.path(),
            &mut claimed,
        )
        .unwrap();
        let audio = pcm(&trimmed[0]);
        let seconds = audio.len() as f64 / 44100.0;
        assert!((1.25..1.4).contains(&seconds), "{seconds}");
        // The 0.3-second pause in the middle must survive trimming.
        assert!(rms(&audio[25000..32000]) < 1.0);
    }

    #[test]
    fn vocal_reduction_scales_center_without_removing_stereo_detail() {
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("stereo.wav");
        generate(
            ffmpeg,
            &source,
            &[
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=0.1*sin(2*PI*440*t)+0.05*sin(2*PI*880*t)|0.1*sin(2*PI*440*t)-0.05*sin(2*PI*880*t):s=44100:d=0.3",
                "-c:a",
                "pcm_s16le",
            ],
        );
        let pcm = |path: &Path| {
            let r = Command::new(ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(path)
                .args(["-map", "0:a:0", "-f", "s16le", "-c:a", "pcm_s16le", "-"])
                .output()
                .unwrap();
            assert!(r.status.success());
            r.stdout
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect::<Vec<_>>()
        };
        let levels = |samples: &[i16]| {
            let frames = samples.as_chunks::<2>().0;
            let rms = |side: bool| {
                (frames
                    .iter()
                    .map(|[l, r]| {
                        let l = f64::from(*l);
                        let r = f64::from(*r);
                        if side {
                            (l - r).powi(2)
                        } else {
                            (l + r).powi(2)
                        }
                    })
                    .sum::<f64>()
                    / frames.len() as f64)
                    .sqrt()
            };
            (rms(false), rms(true))
        };
        let original = pcm(&source);
        let (center, side) = levels(&original);
        let mut claimed = HashSet::new();
        for amount in [0.0, 25.0, 50.0, 75.0, 100.0] {
            let mut opts = Options::default();
            opts.media_tools.vocal_reduction_percent = amount;
            let output = run(
                &tb,
                Tool::Karaoke,
                std::slice::from_ref(&source),
                &opts,
                dir.path(),
                &mut claimed,
            )
            .unwrap();
            let samples = pcm(&output[0]);
            assert_eq!(samples.len(), original.len());
            if amount == 0.0 {
                assert_eq!(samples, original);
            }
            let (new_center, new_side) = levels(&samples);
            assert!(
                (new_center / center - (1.0 - f64::from(amount) / 100.0)).abs() < 0.002,
                "{amount}: {new_center}"
            );
            assert!(
                (new_side / side - 1.0).abs() < 0.002,
                "{amount}: stereo detail changed"
            );
        }
        // Zero also keeps a mono recording's samples and channel layout intact.
        let mono = dir.path().join("mono.wav");
        generate(
            ffmpeg,
            &mono,
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.1",
                "-c:a",
                "pcm_s16le",
            ],
        );
        let mut opts = Options::default();
        opts.media_tools.vocal_reduction_percent = 0.0;
        let output = run(
            &tb,
            Tool::Karaoke,
            std::slice::from_ref(&mono),
            &opts,
            dir.path(),
            &mut claimed,
        )
        .unwrap();
        assert_eq!(pcm(&output[0]), pcm(&mono));
    }

    #[test]
    fn structured_media_inspection_reads_real_streams_without_creating_files() {
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).unwrap();
        let fx = fixture(ffmpeg);
        let list = || {
            let mut names = std::fs::read_dir(&fx.root)
                .unwrap()
                .map(|f| f.unwrap().file_name())
                .collect::<Vec<_>>();
            names.sort();
            names
        };
        let before = list();
        let info = ffmpeg::probe(ffmpeg, &fx.video, None).unwrap();
        assert!(info.has_video && info.has_audio);
        assert!(info.bitrate_bps.is_some());
        assert!(info.format.is_some());
        let video = info.streams.iter().find(|s| s.kind == "video").unwrap();
        assert!(video.codec.is_some());
        assert_eq!((video.width, video.height), (Some(640), Some(360)));
        let audio = info.streams.iter().find(|s| s.kind == "audio").unwrap();
        assert!(audio.codec.is_some() && audio.sample_rate.is_some() && audio.channels.is_some());
        assert_eq!(before, list());
    }

    #[test]
    fn photo_privacy_rejects_real_animated_png_and_webp() {
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).unwrap();
        let dir = tempfile::tempdir().unwrap();
        for (ext, encoder) in [("png", "apng"), ("webp", "libwebp_anim")] {
            if !tb.has_encoder(encoder) {
                continue;
            }
            let source = dir.path().join(format!("animation.{ext}"));
            generate(
                ffmpeg,
                &source,
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=32x32:rate=2:duration=1",
                    "-c:v",
                    encoder,
                    "-f",
                    if ext == "png" { "apng" } else { "webp" },
                ],
            );
            let original = std::fs::read(&source).unwrap();
            let output = dir.path().join("clean.png");
            assert!(
                imaging::remove_metadata(&source, &output).is_err(),
                "{ext}: animation must not be flattened"
            );
            assert!(!output.exists());
            assert_eq!(std::fs::read(&source).unwrap(), original);
        }
    }

    #[test]
    fn expanded_tags_round_trip_urls_dates_comments_and_custom_values() {
        use crate::tagging::{TagField, TagScope};
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).unwrap();
        let fx = fixture(ffmpeg);
        let out = fx.root.join("expanded-tags");
        std::fs::create_dir(&out).unwrap();
        let mut claimed = HashSet::new();
        let url = "https://www.youtube.com/watch?v=zIbT9_TKZKo";
        for format in [AudioFormat::Mp3, AudioFormat::M4a, AudioFormat::Flac] {
            let mut options = Options::default();
            options.media_tools.tag_format = format;
            options.media_tools.artist = "EyalGolanOfficial".into();
            options.media_tools.extra_tags = [
                ("date", "2026"),
                ("purl", url),
                ("comment", "Comment line 1\nComment line 2"),
                ("grouping", "Custom group"),
            ]
            .into_iter()
            .map(|(key, value)| TagField {
                scope: TagScope::File,
                key: key.into(),
                value: value.into(),
                write: true,
            })
            .collect();
            let saved = run(
                &tb,
                Tool::EditTags,
                std::slice::from_ref(&fx.audio),
                &options,
                &out,
                &mut claimed,
            )
            .unwrap();
            let tags = ffmpeg::probe(ffmpeg, &saved[0], None)
                .unwrap()
                .tags
                .unwrap();
            assert_eq!(tags.artist, "EyalGolanOfficial");
            for (key, value) in [
                ("date", "2026"),
                ("purl", url),
                ("comment", "Comment line 1\nComment line 2"),
                ("grouping", "Custom group"),
            ] {
                assert!(
                    tags.extra.iter().any(|f| f.scope == TagScope::File
                        && f.key.eq_ignore_ascii_case(key)
                        && f.value == value),
                    "{format:?}: {key}"
                );
            }
            // Clearing comment must preserve the URL, date and unedited custom tag.
            options.media_tools.extra_tags = vec![TagField {
                scope: TagScope::File,
                key: "comment".into(),
                value: String::new(),
                write: true,
            }];
            options.media_tools.artist.clear();
            let cleared = run(&tb, Tool::EditTags, &saved, &options, &out, &mut claimed).unwrap();
            let tags = ffmpeg::probe(ffmpeg, &cleared[0], None)
                .unwrap()
                .tags
                .unwrap();
            assert!(
                !tags
                    .extra
                    .iter()
                    .any(|f| f.scope == TagScope::File && f.key.eq_ignore_ascii_case("comment"))
            );
            assert!(
                tags.extra
                    .iter()
                    .any(|f| f.key.eq_ignore_ascii_case("purl") && f.value == url)
            );
            assert!(
                tags.extra
                    .iter()
                    .any(|f| f.key.eq_ignore_ascii_case("grouping") && f.value == "Custom group")
            );
        }
        // Artwork labels and global comments are independent, even when they share keys.
        let mut options = Options::default();
        options.media_tools.cover = Some(fx.logo.clone());
        options.media_tools.extra_tags = vec![
            TagField {
                scope: TagScope::File,
                key: "comment".into(),
                value: url.into(),
                write: true,
            },
            TagField {
                scope: TagScope::Cover(0),
                key: "title".into(),
                value: "Album cover".into(),
                write: true,
            },
            TagField {
                scope: TagScope::Cover(0),
                key: "comment".into(),
                value: "Cover (front)".into(),
                write: true,
            },
        ];
        let saved = run(
            &tb,
            Tool::EditTags,
            std::slice::from_ref(&fx.audio),
            &options,
            &out,
            &mut claimed,
        )
        .unwrap();
        let tags = ffmpeg::probe(ffmpeg, &saved[0], None)
            .unwrap()
            .tags
            .unwrap();
        assert!(tags.title.is_empty());
        assert!(
            tags.extra
                .iter()
                .any(|f| f.scope == TagScope::File && f.key == "comment" && f.value == url)
        );
        assert!(tags.extra.iter().any(|f| f.scope == TagScope::Cover(0)
            && f.key == "title"
            && f.value == "Album cover"));
        options.media_tools.cover = None;
        options.media_tools.extra_tags = vec![TagField {
            scope: TagScope::File,
            key: "comment".into(),
            write: true,
            ..Default::default()
        }];
        let cleared = run(&tb, Tool::EditTags, &saved, &options, &out, &mut claimed).unwrap();
        let tags = ffmpeg::probe(ffmpeg, &cleared[0], None)
            .unwrap()
            .tags
            .unwrap();
        assert!(tags.extra.iter().any(|f| f.scope == TagScope::Cover(0)
            && f.key == "comment"
            && f.value == "Cover (front)"));
    }

    #[test]
    fn unsupported_m4a_tags_with_cover_never_publish_a_damaged_output() {
        use crate::tagging::{TagField, TagScope};
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).unwrap();
        let fx = fixture(ffmpeg);
        let out = fx.root.join("unsupported.m4a");
        let mut options = Options::default();
        options.media_tools.tag_format = AudioFormat::M4a;
        options.media_tools.cover = Some(fx.logo.clone());
        options.media_tools.extra_tags = vec![TagField {
            scope: TagScope::File,
            key: "purl".into(),
            value: "https://example.invalid/never-open".into(),
            write: true,
        }];
        let original = std::fs::read(&fx.audio).unwrap();
        let spec = JobSpec {
            tool: Tool::EditTags,
            inputs: vec![fx.audio.clone()],
            output: out.clone(),
            url: None,
            options,
        };
        assert!(matches!(
            execute(&spec, &tb, &AtomicBool::new(false), |_| {}),
            Err(JobError::Plan(PlanError::MetadataNotSaved(_)))
        ));
        assert!(!out.exists());
        assert_eq!(std::fs::read(&fx.audio).unwrap(), original);
    }

    fn toolbox() -> Option<Toolbox> {
        std::env::var_os("FMX_FFMPEG").map(|p| Toolbox::detect(Some(Path::new(&p))))
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

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        video: PathBuf,
        video2: PathBuf,
        audio: PathBuf,
        logo: PathBuf,
        subs: PathBuf,
        images: Vec<PathBuf>,
    }

    fn fixture(ffmpeg: &Path) -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let video = root.join("clip.mp4");
        let video2 = root.join("other.mkv");
        let audio = root.join("tone.wav");
        let av = |src: &str, hz: &str| -> Vec<String> {
            let tone = format!("sine=frequency={hz}");
            [
                "-f",
                "lavfi",
                "-i",
                src,
                "-f",
                "lavfi",
                "-i",
                tone.as_str(),
                "-t",
                "3",
                "-pix_fmt",
                "yuv420p",
            ]
            .map(str::to_owned)
            .to_vec()
        };
        generate(ffmpeg, &video, &av("testsrc=size=640x360:rate=25", "440"));
        generate(ffmpeg, &video2, &av("testsrc2=size=320x240:rate=30", "660"));
        generate(
            ffmpeg,
            &audio,
            &["-f", "lavfi", "-i", "sine=frequency=330:duration=2"],
        );
        let logo = root.join("logo.png");
        image::RgbaImage::from_pixel(64, 32, image::Rgba([255, 0, 0, 160]))
            .save(&logo)
            .expect("logo");
        // A name with characters that need filtergraph escaping.
        let subs = root.join("it's: subs, [1].srt");
        std::fs::write(&subs, "1\n00:00:00,000 --> 00:00:02,000\nHello\n").expect("subs");
        let images = (0..2)
            .map(|i| {
                let p = root.join(format!("page{i}.png"));
                image::RgbImage::from_pixel(200, 120, image::Rgb([20, 80 * i as u8, 200]))
                    .save(&p)
                    .expect("img");
                p
            })
            .collect();
        Fixture {
            _dir: dir,
            root,
            video,
            video2,
            audio,
            logo,
            subs,
            images,
        }
    }

    /// Plans and runs one selection; returns the outputs or a failure label.
    fn run(
        toolbox: &Toolbox,
        tool: Tool,
        files: &[PathBuf],
        opts: &Options,
        out_dir: &Path,
        claimed: &mut HashSet<PathBuf>,
    ) -> Result<Vec<PathBuf>, String> {
        let sel = Selection {
            tool,
            files,
            url: "",
            output_dir: Some(out_dir),
            options: opts,
        };
        let specs = plan_jobs(&sel, claimed).map_err(|e| e.to_string())?;
        let mut outputs = Vec::new();
        for spec in specs {
            let started = Instant::now();
            let mut last = 0.0f32;
            let result = execute(&spec, toolbox, &AtomicBool::new(false), |p| last = p);
            eprintln!(
                "{:<28} {:>6.2}s {:?}",
                format!("{tool:?}"),
                started.elapsed().as_secs_f32(),
                result.as_ref().err().map(ToString::to_string)
            );
            result.map_err(|e| e.to_string())?;
            if last < 1.0 {
                return Err("progress did not reach 100%".into());
            }
            outputs.push(spec.output);
        }
        Ok(outputs)
    }

    fn produced_media(toolbox: &Toolbox, path: &Path) -> bool {
        if path.is_dir() {
            return std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_some());
        }
        let ffmpeg = toolbox.get(External::Ffmpeg).expect("ffmpeg");
        ffmpeg::probe(ffmpeg, path, None)
            .is_ok_and(|i| i.has_audio || i.has_video || i.has_subtitles)
    }

    #[test]
    fn matched_subtitle_batch_muxes_the_right_track() {
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let videos: Vec<_> = ["first", "second"]
            .iter()
            .map(|name| {
                let video = dir.path().join(format!("{name}.mp4"));
                generate(
                    ffmpeg,
                    &video,
                    &["-f", "lavfi", "-i", "color=size=64x64:rate=1", "-t", "1"],
                );
                std::fs::write(
                    dir.path().join(format!("{name}.srt")),
                    format!("1\n00:00:00,000 --> 00:00:00,900\n{name}\n"),
                )
                .unwrap();
                video
            })
            .collect();
        let mut opts = Options::default();
        opts.edit.match_subtitles = true;
        let outputs = run(
            &tb,
            Tool::AddSubtitles,
            &videos,
            &opts,
            dir.path(),
            &mut HashSet::new(),
        )
        .unwrap();
        assert_eq!(outputs.len(), 2);
        for (output, expected) in outputs.iter().zip(["first", "second"]) {
            let extracted = Command::new(ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(output)
                .args(["-map", "0:s:0", "-f", "srt", "-"])
                .output()
                .unwrap();
            assert!(extracted.status.success());
            assert!(String::from_utf8_lossy(&extracted.stdout).contains(expected));
        }
    }

    #[test]
    fn added_lossless_audio_formats_round_trip() {
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).expect("ffmpeg");
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        generate(
            ffmpeg,
            &source,
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.1",
                "-c:a",
                "pcm_s16le",
            ],
        );
        let pcm = |path: &Path| {
            let result = Command::new(ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(path)
                .args(["-f", "s16le", "-c:a", "pcm_s16le", "-"])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            result.stdout
        };
        let original = pcm(&source);
        assert!(!original.is_empty());
        let mut claimed = HashSet::new();
        for format in [AudioFormat::Alac, AudioFormat::Caf, AudioFormat::Au] {
            let outputs = run(
                &tb,
                Tool::ToAudio(format),
                std::slice::from_ref(&source),
                &Options::default(),
                dir.path(),
                &mut claimed,
            )
            .unwrap();
            assert_eq!(pcm(&outputs[0]), original, "{format:?} changed samples");
        }
    }

    #[test]
    fn rotation_exports_match_preview_for_every_turn_and_flip() {
        let Some(tb) = toolbox() else { return };
        let ffmpeg = tb.get(External::Ffmpeg).expect("ffmpeg");
        let dir = tempfile::tempdir().expect("tempdir");
        let picture = dir.path().join("corners.png");
        let colors = [
            [240u8, 20, 20],
            [20, 220, 20],
            [20, 20, 240],
            [240, 220, 20],
        ];
        let image = image::RgbImage::from_fn(96, 64, |x, y| {
            image::Rgb(colors[usize::from(x >= 48) + 2 * usize::from(y >= 32)])
        });
        image.save(&picture).expect("save corners");
        let source = dir.path().join("source.mp4");
        generate(
            ffmpeg,
            &source,
            &[
                "-loop",
                "1",
                "-i",
                picture.to_str().expect("path"),
                "-t",
                "0.2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        let mut claimed = HashSet::new();
        for turns in 0..4 {
            for mirror in [false, true] {
                let orientation = crate::options::Orientation { turns, mirror };
                if orientation.is_unchanged() {
                    continue;
                }
                let opts = Options {
                    orientation,
                    ..Options::default()
                };
                let outputs = run(
                    &tb,
                    Tool::Rotate,
                    std::slice::from_ref(&source),
                    &opts,
                    dir.path(),
                    &mut claimed,
                )
                .expect("rotate");
                let decoded = crate::process::command(ffmpeg)
                    .args(["-hide_banner", "-loglevel", "error", "-i"])
                    .arg(&outputs[0])
                    .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-"])
                    .output()
                    .expect("decode rotated video");
                assert!(decoded.status.success());
                let actual = image::load_from_memory(&decoded.stdout)
                    .expect("frame")
                    .to_rgb8();
                assert_eq!(actual.dimensions(), orientation.transformed_size(96, 64));
                for (index, expected) in colors.iter().enumerate() {
                    let uv = (
                        if index % 2 == 0 { 0.25 } else { 0.75 },
                        if index < 2 { 0.25 } else { 0.75 },
                    );
                    let (x, y) = orientation.map_point(uv);
                    let pixel = actual.get_pixel(
                        (x * actual.width() as f32) as u32,
                        (y * actual.height() as f32) as u32,
                    );
                    for channel in 0..3 {
                        assert!(
                            pixel[channel].abs_diff(expected[channel]) < 35,
                            "orientation {orientation:?}: corner {index}, pixel {pixel:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_media_tool_produces_output() {
        let Some(tb) = toolbox() else {
            eprintln!("FMX_FFMPEG not set; skipping");
            return;
        };
        let fx = fixture(tb.get(External::Ffmpeg).expect("ffmpeg"));
        let out_dir = fx.root.join("out");
        std::fs::create_dir(&out_dir).expect("out dir");
        let subbed = fx.root.join("subbed.mkv");
        let ffmpeg = tb.get(External::Ffmpeg).expect("ffmpeg");
        let video_arg = fx.video.to_string_lossy().into_owned();
        let subs_arg = fx.subs.to_string_lossy().into_owned();
        generate(
            ffmpeg,
            &subbed,
            &[
                "-i", &video_arg, "-i", &subs_arg, "-map", "0", "-map", "1", "-c", "copy",
            ],
        );

        let mut opts = Options::default();
        opts.edit.trim_start = "0:01".into();
        opts.edit.trim_end = "0:02".into();
        opts.target_size_mb = 1;
        opts.delogo = Rect {
            x: 10,
            y: 10,
            w: 100,
            h: 40,
        };
        let mut edited = Options::default();
        edited.edit.crop = Some(Rect {
            x: 0,
            y: 0,
            w: 320,
            h: 240,
        });
        edited.edit.subtitle = Some(fx.subs.clone());
        edited.edit.watermark = Some(fx.logo.clone());
        edited.edit.watermark_corner = Some(Corner::TopLeft);
        edited.video.fps = Some(15);
        edited.audio.volume_db = -3.0;

        let split_opts = Options {
            split_by_parts: true,
            split_parts: 2,
            ..Options::default()
        };
        let turned = Options {
            orientation: crate::options::Orientation::default()
                .rotate_cw()
                .flip_horizontal(),
            ..Options::default()
        };
        let mut louder = Options::default();
        louder.audio.volume_db = 6.0;
        let mut cases: Vec<(Tool, Vec<PathBuf>, &Options)> = VideoFormat::ALL
            .into_iter()
            .map(|f| (Tool::ToVideo(f), vec![fx.video.clone()], &opts))
            .chain(
                AudioFormat::ALL
                    .into_iter()
                    .filter(|f| {
                        tb.has_encoder(f.required_encoder())
                            && tb.missing(Tool::ToAudio(*f).requires()).is_none()
                    })
                    .map(|f| (Tool::ToAudio(f), vec![fx.video.clone()], &opts)),
            )
            .collect();
        cases.extend([
            (
                Tool::ToVideo(VideoFormat::Mp4),
                vec![fx.video.clone()],
                &edited,
            ),
            (
                Tool::ToVideo(VideoFormat::Gif),
                vec![fx.video.clone()],
                &edited,
            ),
            (
                Tool::JoinVideo,
                vec![fx.video.clone(), fx.video2.clone()],
                &opts,
            ),
            (
                Tool::JoinAudio,
                vec![fx.audio.clone(), fx.video.clone()],
                &opts,
            ),
            (
                Tool::AudioMix,
                vec![fx.audio.clone(), fx.video.clone()],
                &opts,
            ),
            (
                Tool::VideoMix,
                vec![fx.video.clone(), fx.video2.clone()],
                &opts,
            ),
            (Tool::Mux, vec![fx.video2.clone(), fx.audio.clone()], &opts),
            (Tool::Trim, vec![fx.video.clone()], &opts),
            (Tool::Split, vec![fx.video.clone()], &split_opts),
            (Tool::Compress, vec![fx.video.clone()], &opts),
            (Tool::RemoveAudio, vec![fx.video.clone()], &opts),
            (Tool::Delogo, vec![fx.video.clone()], &opts),
            (Tool::ExtractFrames, vec![fx.video.clone()], &opts),
            (Tool::Rotate, vec![fx.video2.clone()], &turned),
            (Tool::Speed, vec![fx.video.clone()], &opts),
            (Tool::Volume, vec![fx.audio.clone()], &louder),
            (Tool::Normalize, vec![fx.audio.clone()], &opts),
            (Tool::Repair, vec![fx.video.clone()], &opts),
            (Tool::Crop, vec![fx.video.clone()], &edited),
            (Tool::AddSubtitles, vec![fx.video.clone()], &edited),
            (Tool::ExtractSubtitles, vec![subbed.clone()], &opts),
        ]);
        let mut hw_decode = Options::default();
        hw_decode.video.hw_decode = true;
        cases.push((
            Tool::ToVideo(VideoFormat::Mkv),
            vec![fx.video.clone()],
            &hw_decode,
        ));
        for device in crate::catalog::Device::ALL {
            cases.push((Tool::ToDevice(device), vec![fx.video.clone()], &edited));
        }
        let mut vaapi = Options::default();
        vaapi.video.hw = crate::options::HwAccel::Vaapi;
        if Path::new("/dev/dri/renderD128").exists() && tb.has_encoder("h264_vaapi") {
            cases.push((
                Tool::ToVideo(VideoFormat::Mp4),
                vec![fx.video.clone()],
                &vaapi,
            ));
        }
        let mut claimed = HashSet::new();
        let mut failures = Vec::new();
        for (tool, files, options) in cases {
            match run(&tb, tool, &files, options, &out_dir, &mut claimed) {
                Ok(outputs) if outputs.iter().all(|o| produced_media(&tb, o)) => {}
                Ok(_) => failures.push(format!("{tool:?}: output is not valid media")),
                Err(e) => failures.push(format!("{tool:?}: {e}")),
            }
        }
        assert!(
            failures.is_empty(),
            "failed tools:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn document_image_and_disc_tools() {
        let Some(tb) = toolbox() else {
            return;
        };
        let fx = fixture(tb.get(External::Ffmpeg).expect("ffmpeg"));
        let out = fx.root.join("docs");
        std::fs::create_dir(&out).expect("dir");
        let opts = Options::default();
        let mut claimed = HashSet::new();
        let mut failures = Vec::new();
        let mut check = |tool: Tool, files: Vec<PathBuf>| -> Option<PathBuf> {
            if let Some(missing) = tb.missing(tool.requires()) {
                eprintln!("{tool:?}: skipped, {} not installed", missing.name());
                return None;
            }
            match run(&tb, tool, &files, &opts, &out, &mut claimed) {
                Ok(outputs) => {
                    let o = outputs.into_iter().next()?;
                    let ok = if o.is_dir() {
                        std::fs::read_dir(&o).is_ok_and(|mut d| d.next().is_some())
                    } else {
                        std::fs::metadata(&o).is_ok_and(|m| m.len() > 0)
                    };
                    if !ok {
                        failures.push(format!("{tool:?}: empty output"));
                    }
                    ok.then_some(o)
                }
                Err(e) => {
                    failures.push(format!("{tool:?}: {e}"));
                    None
                }
            }
        };
        for format in ImageFormat::ALL {
            check(Tool::ToImage(format), vec![fx.images[0].clone()]);
        }
        let pdf = check(Tool::ImagesToPdf, fx.images.clone()).expect("pdf");
        for tool in [
            Tool::PdfToImages,
            Tool::PdfToText,
            Tool::PdfToHtml,
            Tool::PdfSplit,
            Tool::PdfCompress,
        ] {
            check(tool, vec![pdf.clone()]);
        }
        check(Tool::PdfMerge, vec![pdf.clone(), pdf.clone()]);
        check(Tool::PdfToExcel, vec![pdf.clone()]);
        check(Tool::JoinImages, fx.images.clone());
        let disc = fx.root.join("disc");
        std::fs::create_dir(&disc).expect("disc");
        std::fs::copy(&fx.video, disc.join("clip.mp4")).expect("copy");
        if let Some(iso) = check(Tool::FolderToIso, vec![disc])
            && let Some(cso) = check(Tool::IsoToCso, vec![iso.clone()])
            && let Some(back) = check(Tool::CsoToIso, vec![cso])
        {
            assert_eq!(
                std::fs::read(&back).expect("back"),
                std::fs::read(&iso).expect("iso")
            );
            // The same image read back as a "disc" copies byte for byte.
            if let Some(copy) = check(Tool::DiscToIso, vec![iso.clone()]) {
                assert_eq!(
                    std::fs::read(&copy).expect("copy"),
                    std::fs::read(&iso).expect("iso")
                );
            }
        }
        assert!(
            failures.is_empty(),
            "failed tools:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn frames_at_chosen_moments_and_from_a_part() {
        let Some(tb) = toolbox() else {
            return;
        };
        let fx = fixture(tb.get(External::Ffmpeg).expect("ffmpeg"));
        let run = |opts: Options, name: &str| -> Vec<String> {
            let spec = JobSpec {
                tool: Tool::ExtractFrames,
                inputs: vec![fx.video.clone()],
                url: None,
                output: fx.root.join(name),
                options: opts,
            };
            let result = execute(&spec, &tb, &AtomicBool::new(false), |_| {});
            assert!(result.is_ok(), "{result:?}");
            let mut names: Vec<String> = std::fs::read_dir(&spec.output)
                .expect("read")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        // The clip is 3 s long: the moment past its end is left out.
        let times = run(
            Options {
                frame_mode: FrameMode::Times,
                frame_times: vec![0.5, 2.25, 9.0],
                frame_format: ImageFormat::Png,
                ..Options::default()
            },
            "times",
        );
        assert_eq!(times, ["frame_00m00.500s.png", "frame_00m02.250s.png"]);
        // Every frame from 1 s to 1.4 s of a 25 fps clip: ten frames.
        let mut part = Options {
            frame_mode: FrameMode::All,
            ..Options::default()
        };
        part.edit.trim_start = "1".into();
        part.edit.trim_end = "1.4".into();
        assert_eq!(run(part, "part").len(), 10);
    }

    #[test]
    fn cancelling_stops_ffmpeg_and_removes_output() {
        let Some(tb) = toolbox() else {
            return;
        };
        let ffmpeg = tb.get(External::Ffmpeg).expect("ffmpeg");
        let dir = tempfile::tempdir().expect("tempdir");
        let video = dir.path().join("long.mp4");
        generate(
            ffmpeg,
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
            url: None,
            output: dir.path().join("long.webm"),
            options: Options::default(),
        };
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let result = execute(&spec, &tb, &cancel, |p| {
            if p > 0.0 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(
            result.as_ref().is_err_and(JobError::is_cancelled),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(30));
        assert!(!spec.output.exists(), "partial output left behind");
    }

    #[test]
    fn recording_detail_shows_time_and_size() {
        let mut p = RecordingProgress::new(None);
        assert_eq!(p.update("out_time_us=65000000"), None);
        assert_eq!(p.update("total_size=12400000"), None);
        let detail = p.update("progress=continue").expect("detail");
        assert!(
            detail.contains("1:05") && detail.contains("12.4 MB"),
            "{detail}"
        );
    }

    #[test]
    fn recording_time_follows_the_wall_clock() {
        let mut p = RecordingProgress::new(None);
        let start = Instant::now();
        p.update_at("out_time_us=0", start);
        p.update_at("progress=continue", start);
        // ffmpeg's frames trail behind, but the clock does not wait for them.
        p.update_at("out_time_us=3000000", start + Duration::from_secs(10));
        let detail = p
            .update_at("progress=continue", start + Duration::from_secs(10))
            .expect("detail");
        assert!(detail.contains("0:10"), "{detail}");
    }

    #[test]
    fn recording_notices_dropped_frames() {
        let mut p = RecordingProgress::new(Some(30));
        let start = Instant::now();
        let report = |p: &mut RecordingProgress, secs: u64, frames: u32, dup: u32| {
            let at = start + Duration::from_secs(secs);
            p.update_at(&format!("frame={frames}"), at);
            p.update_at(&format!("dup_frames={dup}"), at);
            p.update_at("progress=continue", at).expect("detail")
        };
        let dropping = tr("record.dropping_frames");
        assert!(!report(&mut p, 0, 0, 0).contains(dropping));
        assert!(!report(&mut p, 2, 60, 0).contains(dropping));
        // Written at full rate, but most of them are repeats of old frames.
        assert!(report(&mut p, 4, 120, 40).contains(dropping));
        assert!(!report(&mut p, 6, 180, 40).contains(dropping));
    }

    /// May need a person to allow screenshots once: `cargo test -- --ignored screenshot`.
    #[test]
    #[ignore]
    fn screenshot_end_to_end() {
        let Some(tb) = toolbox() else {
            return;
        };
        let dir = tempfile::tempdir().expect("tempdir");
        for format in [ShotFormat::Png, ShotFormat::Jpeg] {
            let spec = JobSpec {
                tool: Tool::Screenshot,
                inputs: Vec::new(),
                url: None,
                output: dir.path().join(format!("shot.{}", format.ext())),
                options: Options {
                    screenshot: ScreenshotOptions {
                        format,
                        minimize: false,
                        ..ScreenshotOptions::default()
                    },
                    ..Options::default()
                },
            };
            let result = execute(&spec, &tb, &AtomicBool::new(false), |_| {});
            assert!(result.is_ok(), "{result:?}");
            let info = ffmpeg::probe(
                tb.get(External::Ffmpeg).expect("ffmpeg"),
                &spec.output,
                None,
            )
            .expect("probe");
            assert!(info.width.unwrap_or(0) > 100, "{format:?}");
        }
    }

    /// Needs a person to answer the system's dialog: `cargo test -- --ignored wayland`.
    #[test]
    #[ignore]
    fn wayland_recording_end_to_end() {
        let Some(tb) = toolbox() else {
            return;
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = JobSpec {
            tool: Tool::Record,
            inputs: Vec::new(),
            url: None,
            output: dir.path().join("rec.mkv"),
            options: Options {
                record: RecordOptions {
                    audio: RecordAudio::System,
                    countdown: false,
                    ..RecordOptions::default()
                },
                ..Options::default()
            },
        };
        let cancel = AtomicBool::new(false);
        let mut details = Vec::new();
        let result = std::thread::scope(|s| {
            let cancel = &cancel;
            s.spawn(move || {
                // Time to answer the dialog, then two seconds of recording.
                std::thread::sleep(Duration::from_secs(12));
                cancel.store(true, Ordering::Relaxed);
            });
            execute_with_detail(&spec, &tb, cancel, |_| {}, |d| details.push(d))
        });
        assert!(result.is_ok(), "{result:?}");
        assert!(
            details.iter().any(|d| d.contains("MB") || d.contains("KB")),
            "{details:?}"
        );
        let info = ffmpeg::probe(
            tb.get(External::Ffmpeg).expect("ffmpeg"),
            &spec.output,
            None,
        )
        .expect("probe");
        assert!(info.has_video && info.has_audio);
    }

    #[test]
    fn screen_recording_stops_gracefully() {
        let Some(tb) = toolbox() else {
            return;
        };
        if std::env::var_os("DISPLAY").is_none() || capture::is_wayland() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = JobSpec {
            tool: Tool::Record,
            inputs: Vec::new(),
            url: None,
            output: dir.path().join("rec.mkv"),
            options: Options {
                record: RecordOptions {
                    audio: RecordAudio::None,
                    fps: 10,
                    countdown: false,
                    ..RecordOptions::default()
                },
                ..Options::default()
            },
        };
        let cancel = AtomicBool::new(false);
        let result = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_secs(2));
                cancel.store(true, Ordering::Relaxed);
            });
            execute(&spec, &tb, &cancel, |_| {})
        });
        assert!(result.is_ok(), "{result:?}");
        let info = ffmpeg::probe(
            tb.get(External::Ffmpeg).expect("ffmpeg"),
            &spec.output,
            None,
        )
        .expect("probe");
        assert!(info.has_video);
    }
}

/// Office → PDF with no helper programs at all: the toolbox is empty.
#[cfg(test)]
mod office_e2e {
    use super::*;
    use crate::options::Options;
    use std::io::Write as _;

    fn zip_with(path: &Path, files: &[(&str, &str)]) {
        let file = std::fs::File::create(path).expect("create");
        let mut zip = zip::ZipWriter::new(file);
        for (name, body) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("entry");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("finish");
    }

    fn fixtures(dir: &Path) -> Vec<(PathBuf, &'static str)> {
        let docx = dir.join("report.docx");
        crate::office::write_docx(
            &[vec!["Docx heading text".into(), "Second paragraph".into()]],
            &docx,
        )
        .expect("docx");
        let xlsx = dir.join("sheet.xlsx");
        let mut wb = rust_xlsxwriter::Workbook::new();
        let ws = wb.add_worksheet();
        ws.write_string(0, 0, "Fruit").expect("cell");
        ws.write_string(1, 0, "Mango").expect("cell");
        ws.write_number(1, 1, 42.0).expect("cell");
        wb.save(&xlsx).expect("xlsx");
        let pptx = dir.join("deck.pptx");
        zip_with(
            &pptx,
            &[(
                "ppt/slides/slide1.xml",
                r#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>Slide title</a:t></a:r></a:p><a:p><a:r><a:t>Bullet one</a:t></a:r></a:p></p:sld>"#,
            )],
        );
        let odt = dir.join("letter.odt");
        zip_with(
            &odt,
            &[(
                "content.xml",
                r#"<office:document-content xmlns:office="o" xmlns:text="t"><office:body><office:text><text:h text:outline-level="1">Odt heading</text:h><text:p>Dear reader</text:p></office:text></office:body></office:document-content>"#,
            )],
        );
        let csv = dir.join("data.csv");
        std::fs::write(&csv, "name,qty\nwidget,7\n").expect("csv");
        let html = dir.join("page.html");
        std::fs::write(&html, "<h1>Html heading</h1><p>Body &amp; soul</p>").expect("html");
        let md = dir.join("notes.md");
        std::fs::write(&md, "# Notes\n\n- remember milk\n").expect("md");
        vec![
            (docx, "Docx heading text"),
            (xlsx, "Mango"),
            (pptx, "Slide title"),
            (odt, "Odt heading"),
            (csv, "widget"),
            (html, "Body & soul"),
            (md, "remember milk"),
        ]
    }

    #[test]
    fn office_documents_convert_without_helpers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let empty = Toolbox::default();
        let pdftotext = Toolbox::detect(None)
            .get(External::Pdftotext)
            .map(Path::to_path_buf);
        for (input, needle) in fixtures(dir.path()) {
            let output = input.with_extension("pdf");
            let spec = JobSpec {
                tool: Tool::OfficeToPdf,
                inputs: vec![input.clone()],
                url: None,
                output: output.clone(),
                options: Options::default(),
            };
            execute(&spec, &empty, &AtomicBool::new(false), |_| {})
                .unwrap_or_else(|e| panic!("{}: {e}", input.display()));
            let bytes = std::fs::read(&output).expect("pdf");
            assert!(bytes.starts_with(b"%PDF-"), "{}", input.display());
            if let Some(tool) = &pdftotext {
                let out = std::process::Command::new(tool)
                    .arg(&output)
                    .arg("-")
                    .output()
                    .expect("pdftotext");
                let text = String::from_utf8_lossy(&out.stdout);
                assert!(
                    text.contains(needle),
                    "{}: {needle:?} not in {text:?}",
                    input.display()
                );
            }
        }
    }

    #[test]
    fn legacy_doc_without_libreoffice_explains_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let input = dir.path().join("old.doc");
        std::fs::write(&input, b"\xd0\xcf\x11\xe0").expect("doc");
        let spec = JobSpec {
            tool: Tool::OfficeToPdf,
            inputs: vec![input],
            url: None,
            output: dir.path().join("old.pdf"),
            options: Options::default(),
        };
        let err = execute(&spec, &Toolbox::default(), &AtomicBool::new(false), |_| {})
            .expect_err("needs LibreOffice");
        assert!(err.to_string().contains("LibreOffice"), "{err}");
    }

    #[test]
    fn pdf_to_word_round_trip() {
        let Some(_) = Toolbox::detect(None).get(External::Pdftotext) else {
            return;
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = dir.path().join("in.pdf");
        crate::pdfgen::render(
            &[crate::pdfgen::Block::Paragraph(vec![
                crate::pdfgen::Span::plain("Round trip words"),
            ])],
            &pdf,
        )
        .expect("pdf");
        let docx = dir.path().join("out.docx");
        let spec = JobSpec {
            tool: Tool::PdfToWord,
            inputs: vec![pdf],
            url: None,
            output: docx.clone(),
            options: Options::default(),
        };
        execute(
            &spec,
            &Toolbox::detect(None),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("pdf to word");
        let text = format!(
            "{:?}",
            crate::office::read_document(&docx).expect("read docx")
        );
        assert!(text.contains("Round trip words"), "{text}");
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn exclusive_copy_publishes_and_refuses_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::write(&source, b"converted").unwrap();
        copy_output_exclusive(&source, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"converted");
        std::fs::write(&source, b"replacement").unwrap();
        assert!(copy_output_exclusive(&source, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"converted");
    }

    #[test]
    fn publishing_never_overwrites_existing_files_or_directories() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(&target, b"keep").unwrap();
        assert!(publish_output(&source, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        std::fs::remove_file(&target).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("unrelated"), b"keep").unwrap();
        assert!(publish_output(&source, &target).is_err());
        assert_eq!(std::fs::read(target.join("unrelated")).unwrap(), b"keep");
    }

    #[test]
    fn failed_conversion_preserves_destination_data() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out.png");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("unrelated"), b"keep").unwrap();
        let spec = JobSpec {
            tool: Tool::ToImage(crate::catalog::ImageFormat::Png),
            inputs: vec![dir.path().join("missing.png")],
            url: None,
            output: output.clone(),
            options: Default::default(),
        };
        assert!(execute(&spec, &Toolbox::default(), &AtomicBool::new(false), |_| {}).is_err());
        assert_eq!(std::fs::read(output.join("unrelated")).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn conversion_publishes_image_and_refuses_late_collision() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.png");
        image::DynamicImage::new_rgb8(2, 2).save(&input).unwrap();
        let output = dir.path().join("output.png");
        let spec = JobSpec {
            tool: Tool::ToImage(crate::catalog::ImageFormat::Png),
            inputs: vec![input],
            url: None,
            output: output.clone(),
            options: Default::default(),
        };
        execute(&spec, &Toolbox::default(), &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(image::open(&output).unwrap().width(), 2);
        std::fs::write(&output, b"keep").unwrap();
        assert!(execute(&spec, &Toolbox::default(), &AtomicBool::new(false), |_| {}).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn publishing_preserves_directory_contents() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("page.png"), b"image").unwrap();
        let target = dir.path().join("target");
        publish_output(&source, &target).unwrap();
        assert_eq!(std::fs::read(target.join("page.png")).unwrap(), b"image");
        assert!(publish_output(&source, &target).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn publishing_rejects_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::os::unix::fs::symlink("missing", &source).unwrap();
        assert!(publish_output(&source, &target).is_err());
        std::fs::remove_file(&source).unwrap();
        std::fs::write(&source, b"new").unwrap();
        std::os::unix::fs::symlink("other-missing", &target).unwrap();
        assert!(publish_output(&source, &target).is_err());
        assert!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[cfg(test)]
mod native_privacy_tests {
    use super::*;
    #[test]
    fn native_photo_privacy_runs_without_ffmpeg_and_ignores_unrelated_edits() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("photo.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([40, 50, 60, 120]))
            .save(&source)
            .unwrap();
        let output = dir.path().join("private.png");
        let original = std::fs::read(&source).unwrap();
        let tb = Toolbox::detect(Some(Path::new("/missing/fmx-test/ffmpeg")));
        assert!(tb.get(External::Ffmpeg).is_none());
        let mut options = crate::options::Options::default();
        options.edit.trim_start = "invalid unrelated edit".into();
        assert!(options.validate(Tool::RemoveImageMetadata).is_ok());
        let spec = JobSpec {
            tool: Tool::RemoveImageMetadata,
            inputs: vec![source.clone()],
            output: output.clone(),
            url: None,
            options,
        };
        execute(&spec, &tb, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(
            image::open(&output).unwrap().to_rgba8(),
            image::open(&source).unwrap().to_rgba8()
        );
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let inspector = JobSpec {
            tool: Tool::MediaInfo,
            output: dir.path().join("must-not-exist"),
            ..spec
        };
        assert!(matches!(
            execute(&inspector, &tb, &AtomicBool::new(false), |_| {}),
            Err(JobError::Plan(PlanError::UnsupportedTool))
        ));
        assert!(!inspector.output.exists());
    }
}
