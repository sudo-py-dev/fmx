//! Driving ffmpeg: probing inputs and running plans with progress.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;

use crate::plan::{Plan, input_url};
use crate::probe::{MediaInfo, parse_ffmpeg_info};
use crate::process::{self, KillOnDrop, RunError, command};

/// Upper bound on how much probe output is read, so a broken binary can't flood memory.
const MAX_PROBE_BYTES: u64 = 1 << 20;

/// Returns ffmpeg's human-readable description of an input (its `-i` banner).
/// `demuxer` forces an input format such as `dvdvideo` or `libcdio`.
pub fn describe(ffmpeg: &Path, path: &Path, demuxer: Option<&str>) -> Result<String, RunError> {
    let mut args: Vec<OsString> = vec!["-hide_banner".into(), "-nostdin".into()];
    if let Some(format) = demuxer {
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

/// Reads stream info for `path`.
pub fn probe(ffmpeg: &Path, path: &Path, demuxer: Option<&str>) -> Result<MediaInfo, RunError> {
    describe(ffmpeg, path, demuxer).map(|text| parse_ffmpeg_info(&text))
}

/// Parses one `-progress` line into seconds of output written.
fn progress_seconds(line: &str) -> Option<f64> {
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
