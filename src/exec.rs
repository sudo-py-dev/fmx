//! Executes one job on the calling (worker) thread.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
    if let Some(missing) = toolbox.missing(spec.tool.requires()) {
        return Err(JobError::Missing(missing));
    }
    let result = run_tool(spec, toolbox, cancel, &mut on_progress, &mut on_detail);
    match &result {
        Ok(()) => on_progress(1.0),
        // Downloads write into a folder the user owns; never delete it.
        Err(_) if spec.tool != Tool::Download => remove_partial(&spec.output),
        Err(_) => {}
    }
    result
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
            recording_lines(on_detail),
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
        recording_lines(on_detail),
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
fn recording_lines(on_detail: &mut dyn FnMut(String)) -> impl FnMut(&str) + '_ {
    let mut progress = RecordingProgress::default();
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

#[derive(Default)]
struct RecordingProgress {
    secs: f64,
    bytes: f64,
}

impl RecordingProgress {
    fn update(&mut self, line: &str) -> Option<String> {
        if let Some(secs) = ffmpeg::progress_seconds(line) {
            self.secs = secs;
        } else if let Some(bytes) = line
            .strip_prefix("total_size=")
            .and_then(|b| b.parse::<f64>().ok())
        {
            self.bytes = bytes;
        } else if line.starts_with("progress=") {
            return Some(trf(
                "record.recording",
                &[
                    ("time", &download::clock(self.secs)),
                    ("size", &download::human_bytes(self.bytes)),
                ],
            ));
        }
        None
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
    match spec.tool {
        Tool::ToImage(format) => {
            imaging::convert(first_input(spec)?, out, format, opts, ffmpeg_path)
                .map_err(JobError::Image)
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
            run_plain(need(toolbox, External::Pdftoppm)?, &args, cancel)
        }
        Tool::PdfToText => run_plain(
            need(toolbox, External::Pdftotext)?,
            &docs::pdf_to_text(first_input(spec)?, out),
            cancel,
        ),
        Tool::PdfToHtml => run_plain(
            need(toolbox, External::Pdftohtml)?,
            &docs::pdf_to_html(first_input(spec)?, out),
            cancel,
        ),
        Tool::PdfMerge => run_plain(
            need(toolbox, External::Pdfunite)?,
            &docs::pdf_merge(&spec.inputs, out),
            cancel,
        ),
        Tool::PdfSplit => {
            std::fs::create_dir_all(out).map_err(JobError::Io)?;
            run_plain(
                need(toolbox, External::Pdfseparate)?,
                &docs::pdf_split(first_input(spec)?, out),
                cancel,
            )
        }
        Tool::PdfCompress => run_plain(
            need(toolbox, External::Ghostscript)?,
            &docs::pdf_compress(first_input(spec)?, out, opts.quality),
            cancel,
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
            std::fs::create_dir_all(out).map_err(JobError::Io)?;
            let args = download::args(url, out, ffmpeg, &opts.download);
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
            ffmpeg::run(ffmpeg, &plan, cancel, on_progress).map_err(JobError::Run)
        }
    }
}

/// Renders a document to PDF natively. Old binary .doc/.ppt files need
/// LibreOffice, which is used only if it happens to be installed.
fn office_to_pdf(spec: &JobSpec, toolbox: &Toolbox, cancel: &AtomicBool) -> Result<(), JobError> {
    let input = first_input(spec)?;
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
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

/// Lays out the PDF's text with Poppler and writes it as a Word document.
fn pdf_to_word(spec: &JobSpec, toolbox: &Toolbox, cancel: &AtomicBool) -> Result<(), JobError> {
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
    let text = std::fs::read_to_string(text_file.path()).map_err(JobError::Io)?;
    office::write_docx(&office::text_pages(&text), &spec.output).map_err(JobError::Office)
}

/// LibreOffice writes `<stem>.<ext>` into an output folder; convert into a
/// private folder beside the destination, then move the result into place.
/// Lays out the PDF's text with Poppler, then writes one sheet per page.
fn pdf_to_excel(spec: &JobSpec, toolbox: &Toolbox, cancel: &AtomicBool) -> Result<(), JobError> {
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
    let text = std::fs::read_to_string(text_file.path()).map_err(JobError::Io)?;
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

/// End-to-end runs against real programs. Skipped unless `FMX_FFMPEG` points
/// at ffmpeg: `FMX_FFMPEG=/usr/bin/ffmpeg cargo test e2e -- --nocapture`.
/// Document/ISO tools run when their programs are on `PATH`.
#[cfg(test)]
mod e2e {
    use super::*;
    use crate::catalog::{AudioFormat, ImageFormat, VideoFormat};
    use crate::jobs::{Selection, plan_jobs};
    use crate::options::{Corner, Options, RecordAudio, Rect, ShotFormat};
    use std::collections::HashSet;
    use std::process::Command;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

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
            (Tool::Rotate, vec![fx.video2.clone()], &opts),
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
        let mut p = RecordingProgress::default();
        assert_eq!(p.update("out_time_us=65000000"), None);
        assert_eq!(p.update("total_size=12400000"), None);
        let detail = p.update("progress=continue").expect("detail");
        assert!(
            detail.contains("1:05") && detail.contains("12.4 MB"),
            "{detail}"
        );
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
