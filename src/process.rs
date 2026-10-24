//! Running external programs with cancellation, line streaming, and
//! guaranteed cleanup of the child process.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::i18n::{tr, trf};

/// How often a running job checks for cancellation.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Lines of stderr kept for the error message.
const ERROR_TAIL_LINES: usize = 6;
/// How long a program asked to quit gracefully may take to finish its file.
const QUIT_GRACE: Duration = Duration::from_secs(15);

/// A `Command` that never inherits our stdin and opens no console window.
pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Starts a program that runs on its own (a file opener, a notification),
/// with its output discarded so it never floods our terminal. A background
/// thread waits for it so no zombie process is left behind.
pub fn spawn_detached<S: AsRef<std::ffi::OsStr>>(
    program: &str,
    args: impl IntoIterator<Item = S>,
) -> std::io::Result<()> {
    let mut child = command(Path::new(program))
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// What cancelling means for a running program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopMode {
    /// Kill it; the job counts as cancelled.
    Kill,
    /// Send `q` on stdin so ffmpeg finalizes its file; the job counts as done.
    Quit,
}

#[derive(Debug)]
pub enum RunError {
    Spawn(String, std::io::Error),
    Cancelled,
    Failed {
        program: String,
        code: Option<i32>,
        log: String,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(program, e) => {
                f.write_str(&trf("error.spawn", &[("program", program), ("error", e)]))
            }
            Self::Cancelled => f.write_str(tr("status.cancelled")),
            Self::Failed { program, code, log } => {
                let code = code.map_or_else(|| "?".to_owned(), |c| c.to_string());
                f.write_str(&trf(
                    "error.tool_failed",
                    &[("program", program), ("code", &code), ("log", log)],
                ))
            }
        }
    }
}

impl std::error::Error for RunError {}

/// Kills the child if we leave early (error, cancel, panic) so nothing is orphaned.
pub struct KillOnDrop(pub Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn spawn_line_reader<R: Read + Send + 'static>(source: R, tx: mpsc::Sender<String>) {
    thread::spawn(move || {
        // Some tools redraw progress with `\r`; treat it as a line break too.
        let mut reader = BufReader::new(source);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&buf);
                    for line in text.split(['\r', '\n']).filter(|l| !l.is_empty()) {
                        if tx.send(line.to_owned()).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
}

fn program_name(program: &Path) -> String {
    program.file_stem().map_or_else(
        || program.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Which output stream carries the progress lines a caller wants to see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    Stdout,
    Stderr,
}

/// Runs `program args…`, feeding each line of the watched stream to `on_line`,
/// until it exits or `cancel` is set.
pub fn run(
    program: &Path,
    args: &[OsString],
    stop: StopMode,
    cancel: &AtomicBool,
    on_line: impl FnMut(&str),
) -> Result<(), RunError> {
    run_watching(program, args, stop, Watch::Stdout, cancel, on_line)
}

/// Like [`run`], choosing which stream is watched. stderr lines always feed
/// the error tail as well.
pub fn run_watching(
    program: &Path,
    args: &[OsString],
    stop: StopMode,
    watch: Watch,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str),
) -> Result<(), RunError> {
    let name = program_name(program);
    tracing::debug!(program = %name, ?args, "running");
    let mut cmd = command(program);
    if stop == StopMode::Quit {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(KillOnDrop)
        .map_err(|e| RunError::Spawn(name.clone(), e))?;

    let (out_tx, out_rx) = mpsc::channel();
    let (log_tx, log_rx) = mpsc::channel();
    if let Some(out) = child.0.stdout.take() {
        spawn_line_reader(out, out_tx);
    }
    if let Some(err) = child.0.stderr.take() {
        spawn_line_reader(err, log_tx);
    }

    let mut tail = VecDeque::with_capacity(ERROR_TAIL_LINES + 1);
    let push_log = |tail: &mut VecDeque<String>, line: String| {
        if tail.len() == ERROR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    };
    let mut quit_sent: Option<Instant> = None;
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            match (stop, quit_sent) {
                (StopMode::Kill, _) => return Err(RunError::Cancelled),
                (StopMode::Quit, None) => {
                    if let Some(stdin) = child.0.stdin.as_mut() {
                        let _ = stdin.write_all(b"q\n").and_then(|()| stdin.flush());
                    }
                    quit_sent = Some(Instant::now());
                }
                (StopMode::Quit, Some(at)) if at.elapsed() > QUIT_GRACE => {
                    return Err(RunError::Cancelled);
                }
                _ => {}
            }
        }
        let (watched, other) = match watch {
            Watch::Stdout => (&out_rx, &log_rx),
            Watch::Stderr => (&log_rx, &out_rx),
        };
        match watched.recv_timeout(POLL_INTERVAL) {
            Ok(line) => {
                on_line(&line);
                if watch == Watch::Stderr {
                    push_log(&mut tail, line);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // The watched stream closed: the program is exiting.
            Err(mpsc::RecvTimeoutError::Disconnected) => break child.0.wait(),
        }
        for line in other.try_iter() {
            if watch == Watch::Stdout {
                push_log(&mut tail, line);
            }
        }
    }
    .map_err(|e| RunError::Spawn(name.clone(), e))?;

    // A graceful quit ends recordings; ffmpeg may report a non-zero code for it.
    if status.success() || quit_sent.is_some() {
        return Ok(());
    }
    // Collect what stderr wrote after stdout closed.
    if let Ok(line) = log_rx.recv_timeout(POLL_INTERVAL) {
        push_log(&mut tail, line);
    }
    for line in log_rx.try_iter() {
        push_log(&mut tail, line);
    }
    Err(RunError::Failed {
        program: name,
        code: status.code(),
        log: tail.into_iter().collect::<Vec<_>>().join("\n"),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<OsString> {
        vec!["-c".into(), script.into()]
    }

    #[test]
    fn streams_stdout_lines_including_carriage_returns() {
        let mut lines = Vec::new();
        run(
            Path::new("/bin/sh"),
            &sh("printf 'a\\rb\\nc\\n'"),
            StopMode::Kill,
            &AtomicBool::new(false),
            |l| lines.push(l.to_owned()),
        )
        .expect("run");
        assert_eq!(lines, ["a", "b", "c"]);
    }

    #[test]
    fn failure_reports_stderr_tail_and_code() {
        let err = run(
            Path::new("/bin/sh"),
            &sh("echo boom >&2; exit 3"),
            StopMode::Kill,
            &AtomicBool::new(false),
            |_| {},
        )
        .expect_err("must fail");
        match err {
            RunError::Failed { code, log, .. } => {
                assert_eq!(code, Some(3));
                assert!(log.contains("boom"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn cancel_kills_promptly() {
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        let err = run(
            Path::new("/bin/sh"),
            &sh("sleep 30"),
            StopMode::Kill,
            &cancel,
            |_| {},
        );
        assert!(matches!(err, Err(RunError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn quit_mode_finishes_successfully() {
        let cancel = AtomicBool::new(true);
        let result = run(
            Path::new("/bin/sh"),
            &sh("read line; echo got $line"),
            StopMode::Quit,
            &cancel,
            |_| {},
        );
        assert!(result.is_ok(), "{result:?}");
    }
}
