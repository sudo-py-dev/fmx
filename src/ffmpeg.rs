//! Spawn wrapper around the bundled `ffmpeg` and `ffprobe` binaries.
//!
//! The app never links libavcodec. Conversions are `ffmpeg` child processes
//! whose `-progress pipe:1` lines are parsed into a 0..=1 fraction; inspection
//! prefers `ffprobe -print_format json` and falls back to ffmpeg's `-i` banner.

use crate::i18n::tr;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::plan::{BLURAY_PROTOCOL, Plan, input_url};
use crate::probe::{MediaInfo, parse_ffmpeg_info, parse_ffprobe_json};
use crate::process::{self, KillOnDrop, RunError, command};

/// Upper bound on how much probe output is read, so a broken binary can't flood memory.
const MAX_PROBE_BYTES: u64 = 1 << 20;

/// Bound output and wall time independently, including a tool that never closes its pipe.
fn capture_probe(
    program: &Path,
    args: &[OsString],
    stderr: bool,
    timeout: Duration,
) -> Result<(Vec<u8>, std::process::ExitStatus), RunError> {
    let name = if stderr { "ffmpeg" } else { "ffprobe" };
    let spawn_error = |e| RunError::Spawn(name.into(), e);
    let mut child = command(program)
        .args(args)
        .stdout(if stderr {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stderr(if stderr {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .spawn()
        .map(KillOnDrop)
        .map_err(spawn_error)?;
    let reader: Box<dyn Read + Send> = if stderr {
        Box::new(
            child
                .0
                .stderr
                .take()
                .ok_or_else(|| spawn_error(std::io::Error::other("missing stderr pipe")))?,
        )
    } else {
        Box::new(
            child
                .0
                .stdout
                .take()
                .ok_or_else(|| spawn_error(std::io::Error::other("missing stdout pipe")))?,
        )
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = reader
            .take(MAX_PROBE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let start = Instant::now();
    let mut bytes = None;
    loop {
        if bytes.is_none() {
            match rx.try_recv() {
                Ok(result) => {
                    let output = result.map_err(spawn_error)?;
                    if output.len() as u64 > MAX_PROBE_BYTES {
                        return Err(RunError::Failed {
                            program: name.into(),
                            code: None,
                            log: tr("error.probe_limit").into(),
                        });
                    }
                    bytes = Some(output);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(spawn_error(std::io::Error::other(
                        "probe reader disconnected",
                    )));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(status) = child.0.try_wait().map_err(spawn_error)?
            && let Some(output) = bytes.take()
        {
            return Ok((output, status));
        }
        if start.elapsed() >= timeout {
            return Err(RunError::Failed {
                program: name.into(),
                code: None,
                log: tr("error.probe_timeout").into(),
            });
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Keeps file URLs and disc demuxer handling identical for both probes.
fn add_input(args: &mut Vec<OsString>, path: &Path, demuxer: Option<&str>) {
    if let Some(format) = demuxer.filter(|d| *d != BLURAY_PROTOCOL) {
        args.extend(["-f".into(), format.into()]);
    }
    if demuxer.is_none() {
        args.extend(["-protocol_whitelist".into(), "file,pipe".into()]);
    }
    args.extend(["-i".into(), input_url(path, demuxer)]);
}

/// Returns ffmpeg's human-readable description of an input (its `-i` banner).
/// `demuxer` forces an input format such as `dvdvideo` or `libcdio`.
pub fn describe(ffmpeg: &Path, path: &Path, demuxer: Option<&str>) -> Result<String, RunError> {
    let mut args: Vec<OsString> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-max_alloc".into(),
        "67108864".into(),
    ];
    add_input(&mut args, path, demuxer);
    let (bytes, _) = capture_probe(ffmpeg, &args, true, Duration::from_secs(10))?;
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
        "-v".into(),
        "error".into(),
        "-print_format".into(),
        "json".into(),
        "-show_format".into(),
        "-show_streams".into(),
        "-show_chapters".into(),
        "-max_alloc".into(),
        "67108864".into(),
    ];
    add_input(&mut args, path, demuxer);
    let (bytes, status) = capture_probe(ffprobe, &args, false, Duration::from_secs(10))?;
    let text = String::from_utf8_lossy(&bytes);
    if !status.success() {
        return Err(RunError::Failed {
            program: "ffprobe".into(),
            code: status.code(),
            log: tr("tags.unavailable").into(),
        });
    }
    parse_ffprobe_json(&text).ok_or_else(|| RunError::Failed {
        program: "ffprobe".into(),
        code: status.code(),
        log: tr("tags.unavailable").into(),
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
    process::run_limited(
        ffmpeg,
        &plan.args,
        plan.stop,
        plan.memory_limit,
        cancel,
        |line| {
            // The clock stops just short of the full length; the end marker means done.
            if line == "progress=end" {
                on_progress(1.0);
            } else if let (Some(done), Some(total)) = (progress_seconds(line), plan.expected_secs)
                && total > 0.0
            {
                on_progress((done / total).clamp(0.0, 1.0) as f32);
            }
        },
    )
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

#[cfg(all(test, unix))]
mod probe_security_tests {
    use super::*;
    #[test]
    fn hanging_probe_is_stopped_even_without_pipe_output() {
        let started = Instant::now();
        let result = capture_probe(
            Path::new("/bin/sh"),
            &["-c".into(), "exec sleep 5".into()],
            false,
            Duration::from_millis(50),
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn excessive_output_is_rejected() {
        let result = capture_probe(
            Path::new("/bin/sh"),
            &["-c".into(), "exec head -c 1048577 /dev/zero".into()],
            false,
            Duration::from_secs(2),
        );
        assert!(result.is_err());
    }
}
