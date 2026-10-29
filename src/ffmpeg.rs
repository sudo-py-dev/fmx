//! Spawn wrapper around the bundled `ffmpeg` and `ffprobe` binaries.
//!
//! The app never links libavcodec. Conversions are `ffmpeg` child processes
//! whose `-progress pipe:1` lines are parsed into a 0..=1 fraction; inspection
//! prefers `ffprobe -print_format json` and falls back to ffmpeg's `-i` banner.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;

use crate::plan::{BLURAY_PROTOCOL, Plan, input_url};
use crate::probe::{MediaInfo, parse_ffmpeg_info, parse_ffprobe_json};
use crate::process::{self, KillOnDrop, RunError, command};

/// Upper bound on how much probe output is read, so a broken binary can't flood memory.
const MAX_PROBE_BYTES: u64 = 1 << 20;

/// Returns ffmpeg's human-readable description of an input (its `-i` banner).
/// `demuxer` forces an input format such as `dvdvideo` or `libcdio`.
pub fn describe(ffmpeg: &Path, path: &Path, demuxer: Option<&str>) -> Result<String, RunError> {
    let mut args: Vec<OsString> = vec!["-hide_banner".into(), "-nostdin".into()];
    if let Some(format) = demuxer.filter(|d| *d != BLURAY_PROTOCOL) {
        args.extend(["-f".into(), format.into()]);
    }
    args.extend(["-i".into(), input_url(path, demuxer)]);
    let spawn_err = |e| RunError::Spawn("ffmpeg".into(), e);
    let mut child = command(ffmpeg)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map(KillOnDrop)
        .map_err(spawn_err)?;
    let mut bytes = Vec::new();
    if let Some(stderr) = child.0.stderr.take() {
        // Dropping the pipe after the cap makes a runaway writer exit.
        stderr
            .take(MAX_PROBE_BYTES)
            .read_to_end(&mut bytes)
            .map_err(spawn_err)?;
    }
    // ffmpeg exits non-zero without an output file; that is expected here.
    let _ = child.0.wait();
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `ffprobe` sitting next to `ffmpeg`, if the file exists.
pub fn sibling_ffprobe(ffmpeg: &Path) -> Option<PathBuf> {
    let dir = ffmpeg.parent()?;
    let name = if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    };
    let path = dir.join(name);
    path.is_file().then_some(path)
}

fn probe_with_ffprobe(
    ffprobe: &Path,
    path: &Path,
    demuxer: Option<&str>,
) -> Result<MediaInfo, RunError> {
    let mut args: Vec<OsString> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-v".into(),
        "error".into(),
        "-print_format".into(),
        "json".into(),
        "-show_format".into(),
        "-show_streams".into(),
        "-show_chapters".into(),
    ];
    if let Some(format) = demuxer.filter(|d| *d != BLURAY_PROTOCOL) {
        args.extend(["-f".into(), format.into()]);
    }
    args.extend(["-i".into(), input_url(path, demuxer)]);
    let spawn_err = |e| RunError::Spawn("ffprobe".into(), e);
    let mut child = command(ffprobe)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map(KillOnDrop)
        .map_err(spawn_err)?;
    let mut bytes = Vec::new();
    if let Some(stdout) = child.0.stdout.take() {
        stdout
            .take(MAX_PROBE_BYTES)
            .read_to_end(&mut bytes)
            .map_err(spawn_err)?;
    }
    let status = child.0.wait().map_err(spawn_err)?;
    let text = String::from_utf8_lossy(&bytes);
    if !status.success() {
        return Err(RunError::Failed {
            program: "ffprobe".into(),
            code: status.code(),
            log: text.into_owned(),
        });
    }
    parse_ffprobe_json(&text).ok_or_else(|| RunError::Failed {
        program: "ffprobe".into(),
        code: status.code(),
        log: text.into_owned(),
    })
}

/// Reads stream info for `path`, using ffprobe when it sits next to ffmpeg.
pub fn probe(ffmpeg: &Path, path: &Path, demuxer: Option<&str>) -> Result<MediaInfo, RunError> {
    if let Some(ffprobe) = sibling_ffprobe(ffmpeg)
        && let Ok(info) = probe_with_ffprobe(&ffprobe, path, demuxer)
    {
        return Ok(info);
    }
    describe(ffmpeg, path, demuxer).map(|text| parse_ffmpeg_info(&text))
}

/// Parses one `-progress` line into seconds of output written.
pub fn progress_seconds(line: &str) -> Option<f64> {
    // `out_time_us` and the misnamed `out_time_ms` both carry microseconds.
    let (key, value) = line.split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" => value.trim().parse::<f64>().ok().map(|us| us / 1e6),
        _ => None,
    }
}

/// Runs a plan, reporting progress in `0.0..=1.0` and honouring `cancel`.
pub fn run(
    ffmpeg: &Path,
    plan: &Plan,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<(), RunError> {
    process::run(ffmpeg, &plan.args, plan.stop, cancel, |line| {
        // The clock stops just short of the full length; the end marker means done.
        if line == "progress=end" {
            on_progress(1.0);
        } else if let (Some(done), Some(total)) = (progress_seconds(line), plan.expected_secs)
            && total > 0.0
        {
            on_progress((done / total).clamp(0.0, 1.0) as f32);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_parse_microseconds() {
        assert_eq!(progress_seconds("out_time_us=2500000"), Some(2.5));
        assert_eq!(progress_seconds("out_time_ms=1000000"), Some(1.0));
        assert_eq!(progress_seconds("out_time=00:00:02.50"), None);
        assert_eq!(progress_seconds("out_time_us=N/A"), None);
        assert_eq!(progress_seconds("progress=end"), None);
    }
}
