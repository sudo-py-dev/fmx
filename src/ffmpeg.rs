//! Locating and driving the external ffmpeg binary.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::i18n::{tr, trf};
use crate::plan::Plan;
use crate::probe::{MediaInfo, parse_ffmpeg_info};

const BINARY: &str = if cfg!(windows) {
    "ffmpeg.exe"
} else {
    "ffmpeg"
};
/// How often a running job checks for cancellation.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Lines of ffmpeg's stderr kept for the error message.
const ERROR_TAIL_LINES: usize = 6;
/// Upper bound on how much probe output is read, so a broken binary can't flood memory.
const MAX_PROBE_BYTES: u64 = 1 << 20;

/// Finds ffmpeg: an explicit override, then next to our executable, then `PATH`.
pub fn locate(override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = override_path {
        return path.is_file().then(|| path.to_path_buf());
    }
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(BINARY)));
    let on_path = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(BINARY))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    beside_exe.into_iter().chain(on_path).find(|p| p.is_file())
}

#[derive(Debug)]
pub enum RunError {
    Spawn(std::io::Error),
    Cancelled,
    Failed { code: Option<i32>, log: String },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => f.write_str(&trf("error.spawn", &[("error", e)])),
            Self::Cancelled => f.write_str(tr("status.cancelled")),
            Self::Failed { code, log } => {
                let code = code.map_or_else(|| "?".to_owned(), |c| c.to_string());
                f.write_str(&trf(
                    "error.ffmpeg_failed",
                    &[("code", &code), ("log", log)],
                ))
            }
        }
    }
}

impl std::error::Error for RunError {}

fn command(ffmpeg: &Path) -> Command {
    let mut cmd = Command::new(ffmpeg);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Reads stream info for `path`. ffmpeg exits non-zero without an output file;
/// that is expected, so only spawn failures are errors.
pub fn probe(ffmpeg: &Path, path: &Path) -> Result<MediaInfo, RunError> {
    let mut url = std::ffi::OsString::from("file:");
    url.push(path.as_os_str());
    let child = command(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(RunError::Spawn)?;
    let mut text = String::new();
    let mut child = KillOnDrop(child);
    if let Some(stderr) = child.0.stderr.take() {
        // Non-UTF-8 bytes (odd file names) are fine to lose here.
        let mut bytes = Vec::new();
        stderr
            .take(MAX_PROBE_BYTES)
            .read_to_end(&mut bytes)
            .map_err(RunError::Spawn)?;
        text = String::from_utf8_lossy(&bytes).into_owned();
    }
    let _ = child.0.wait();
    Ok(parse_ffmpeg_info(&text))
}

/// Kills the child if we leave early (error, cancel, panic) so no ffmpeg is orphaned.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
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

fn spawn_line_reader<R: Read + Send + 'static>(source: R, tx: mpsc::Sender<String>) {
    thread::spawn(move || {
        for line in BufReader::new(source).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
}

/// Runs a plan, reporting progress in `0.0..=1.0` and honouring `cancel`.
pub fn run(
    ffmpeg: &Path,
    plan: &Plan,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<(), RunError> {
    tracing::debug!(args = ?plan.args, "running ffmpeg");
    let mut child = command(ffmpeg)
        .args(plan.args.iter().map(AsRef::<OsStr>::as_ref))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(KillOnDrop)
        .map_err(RunError::Spawn)?;

    let (progress_tx, progress_rx) = mpsc::channel();
    let (log_tx, log_rx) = mpsc::channel();
    if let Some(out) = child.0.stdout.take() {
        spawn_line_reader(out, progress_tx);
    }
    if let Some(err) = child.0.stderr.take() {
        spawn_line_reader(err, log_tx);
    }

    let mut tail = VecDeque::with_capacity(ERROR_TAIL_LINES);
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(RunError::Cancelled);
        }
        match progress_rx.recv_timeout(POLL_INTERVAL) {
            // The clock stops just short of the full length; the end marker means done.
            Ok(line) if line == "progress=end" => on_progress(1.0),
            Ok(line) => {
                if let (Some(done), Some(total)) = (progress_seconds(&line), plan.expected_secs)
                    && total > 0.0
                {
                    on_progress((done / total).clamp(0.0, 1.0) as f32);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // stdout closed: ffmpeg is exiting.
            Err(mpsc::RecvTimeoutError::Disconnected) => break child.0.wait(),
        }
        for line in log_rx.try_iter() {
            if tail.len() == ERROR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    }
    .map_err(RunError::Spawn)?;

    if status.success() {
        return Ok(());
    }
    // Collect what stderr wrote after stdout closed.
    tail.extend(log_rx.recv_timeout(POLL_INTERVAL));
    tail.extend(log_rx.try_iter());
    let log = tail
        .iter()
        .rev()
        .take(ERROR_TAIL_LINES)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    Err(RunError::Failed {
        code: status.code(),
        log,
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

    #[test]
    fn missing_override_is_not_found() {
        assert_eq!(locate(Some(Path::new("/definitely/not/here/ffmpeg"))), None);
    }
}
