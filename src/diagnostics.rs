//! Logging, the debug-logging switch, and the crash guard.
//!
//! FMX has no console window on Windows, so anything that goes wrong has to
//! end up in a file the user can find and send: `<data dir>/fmx/logs/fmx.log`,
//! with the run before kept as `fmx.previous.log`. Normal runs record warnings,
//! errors and a few facts about the run; debug logging (Settings → General,
//! `fmx --debug`, or `FMX_LOG=<filter>`) records everything FMX does. Messages
//! from eframe, winit and the graphics stack, which use the `log` crate, are
//! included.
//!
//! A panic, or a window that can't be created, writes `crash-<time>.txt` with
//! the details and a backtrace, and a dialog says where it is. The next start
//! mentions the report once.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Registry, fmt, reload};

const LOG_FILE: &str = "fmx.log";
const PREVIOUS_LOG_FILE: &str = "fmx.previous.log";
/// Exists while debug logging is on, so it applies from the very start of the next run.
const DEBUG_MARKER: &str = "debug-logging";
/// Holds the time of the newest crash report the user has been told about.
const SEEN_MARKER: &str = "crash-seen";
const CRASH_PREFIX: &str = "crash-";
/// Crash reports beyond this many are deleted, oldest first.
const MAX_CRASH_REPORTS: usize = 10;

const NORMAL_FILTER: &str = "warn,fmx=info";
/// Everything from FMX and its UI; the graphics stack's per-frame chatter is left out.
const DEBUG_FILTER: &str = "debug,naga=info,wgpu_core=info,wgpu_hal=info,calloop=info";

static FILTER: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
/// No dialogs: nobody is there to close them (the `--self-test` run in CI).
static QUIET: AtomicBool = AtomicBool::new(false);

/// Where logs and crash reports go.
pub fn logs_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("fmx").join("logs"))
}

/// Whether debug logging is on (it stays on across restarts).
pub fn debug_enabled() -> bool {
    logs_dir().is_some_and(|d| d.join(DEBUG_MARKER).exists())
}

/// Starts logging to stderr and to the log file. `force_debug` turns on debug
/// logging for this run only (the `--debug` flag); `quiet` reports problems
/// only in files, without dialogs; `fresh_log` starts a new log file, which only
/// the FMX that opens the window does: other starts add to its log.
pub fn init(force_debug: bool, quiet: bool, fresh_log: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
    let debug_on = force_debug || debug_enabled();
    let filter = EnvFilter::try_from_env("FMX_LOG").unwrap_or_else(|_| {
        EnvFilter::new(if debug_on {
            DEBUG_FILTER
        } else {
            NORMAL_FILTER
        })
    });
    let (filter, handle) = reload::Layer::new(filter);
    let _ = FILTER.set(handle);
    let file = logs_dir().and_then(|dir| open_log(&dir, fresh_log).ok());
    let file_layer = file.map(|f| fmt::layer().with_ansi(false).with_writer(Mutex::new(f)));
    let stderr_layer = fmt::layer().with_writer(std::io::stderr);
    // `try_init` also routes the `log` crate's records here.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
    install_panic_hook();
    tracing::info!(
        version = crate::update::VERSION,
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        debug_logging = debug_on,
        "FMX starting"
    );
}

/// Opens the log: a fresh one (keeping the previous run's), or the current one to add to.
fn open_log(dir: &Path, fresh: bool) -> std::io::Result<File> {
    fs::create_dir_all(dir)?;
    let current = dir.join(LOG_FILE);
    if fresh && current.exists() {
        let _ = fs::rename(&current, dir.join(PREVIOUS_LOG_FILE));
    }
    // Always appending, so lines from several FMX starts never overwrite each other.
    File::options().create(true).append(true).open(current)
}

/// Turns debug logging on or off, now and for later runs.
pub fn set_debug(on: bool) {
    if let Some(dir) = logs_dir() {
        let marker = dir.join(DEBUG_MARKER);
        let result = if on {
            fs::create_dir_all(&dir).and_then(|()| File::create(&marker).map(drop))
        } else {
            fs::remove_file(&marker).or_else(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(e),
            })
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "could not save the debug logging choice");
        }
    }
    if std::env::var_os("FMX_LOG").is_none()
        && let Some(handle) = FILTER.get()
    {
        let filter = EnvFilter::new(if on { DEBUG_FILTER } else { NORMAL_FILTER });
        if let Err(e) = handle.reload(filter) {
            tracing::warn!(error = %e, "could not change the log level");
        }
    }
    tracing::info!(on, "debug logging");
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(|| "unknown".to_owned(), ToString::to_string);
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_owned());
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(%message, %location, %thread, "FMX crashed");
        let details = format!(
            "Panic in thread '{thread}' at {location}:\n{message}\n\nBacktrace:\n{backtrace}"
        );
        let report = write_crash_report("FMX crashed", &details);
        default_hook(info);
        show_error(
            &format!("FMX ran into a problem and has to close.\n\n{message}"),
            report.as_deref(),
        );
    }));
}

/// Records that FMX could not start (for example, no usable graphics) and tells the user.
pub fn report_startup_failure(error: &dyn std::fmt::Display) {
    tracing::error!(%error, "FMX could not start");
    let report = write_crash_report("FMX could not start", &error.to_string());
    show_error(
        &format!("FMX could not start.\n\n{error}"),
        report.as_deref(),
    );
}

/// Writes a crash report and returns its path.
fn write_crash_report(title: &str, details: &str) -> Option<PathBuf> {
    let dir = logs_dir()?;
    fs::create_dir_all(&dir).ok()?;
    let millis = now_millis();
    let path = dir.join(format!("{CRASH_PREFIX}{millis}.txt"));
    let mut file = File::create(&path).ok()?;
    writeln!(
        file,
        "{title}\n\nFMX {}\nSystem: {} {}\nTime: {millis} (milliseconds since 1970, UTC)\nLog: {}\n\n{details}",
        crate::update::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        dir.join(LOG_FILE).display(),
    )
    .ok()?;
    prune_crash_reports(&dir);
    Some(path)
}

/// Crash reports, oldest first, with the time in their names.
fn crash_reports(dir: &Path) -> Vec<(u64, PathBuf)> {
    let mut reports: Vec<(u64, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let millis = name
                .strip_prefix(CRASH_PREFIX)?
                .strip_suffix(".txt")?
                .parse()
                .ok()?;
            Some((millis, entry.path()))
        })
        .collect();
    reports.sort();
    reports
}

fn prune_crash_reports(dir: &Path) {
    let reports = crash_reports(dir);
    let extra = reports.len().saturating_sub(MAX_CRASH_REPORTS);
    for (_, path) in &reports[..extra] {
        let _ = fs::remove_file(path);
    }
}

/// The newest crash report the user hasn't been told about yet, marked as told.
pub fn take_unseen_crash() -> Option<PathBuf> {
    newest_unseen(&logs_dir()?)
}

fn newest_unseen(dir: &Path) -> Option<PathBuf> {
    let (time, path) = crash_reports(dir).pop()?;
    let seen_marker = dir.join(SEEN_MARKER);
    let seen: u64 = fs::read_to_string(&seen_marker)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if time <= seen {
        return None;
    }
    let _ = fs::write(seen_marker, time.to_string());
    Some(path)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A native error dialog, since there may be no window (or no working one) left.
fn show_error(message: &str, report: Option<&Path>) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    let text = match report {
        Some(path) => format!("{message}\n\nA report was saved to:\n{}", path.display()),
        None => message.to_owned(),
    };
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("FMX")
        .set_description(text)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_reports_are_found_in_time_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in ["crash-20.txt", "crash-3.txt", "crash-x.txt", "fmx.log"] {
            fs::write(dir.path().join(name), "").expect("write");
        }
        let found: Vec<u64> = crash_reports(dir.path()).iter().map(|(s, _)| *s).collect();
        assert_eq!(found, vec![3, 20]);
    }

    #[test]
    fn each_crash_report_is_mentioned_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("crash-5.txt"), "").expect("write");
        assert!(newest_unseen(dir.path()).is_some());
        assert!(newest_unseen(dir.path()).is_none());
        fs::write(dir.path().join("crash-9.txt"), "").expect("write");
        assert!(newest_unseen(dir.path()).is_some_and(|p| p.ends_with("crash-9.txt")));
    }

    #[test]
    fn old_crash_reports_are_pruned() {
        let dir = tempfile::tempdir().expect("tempdir");
        for n in 0..MAX_CRASH_REPORTS + 3 {
            fs::write(dir.path().join(format!("crash-{n}.txt")), "").expect("write");
        }
        prune_crash_reports(dir.path());
        let left = crash_reports(dir.path());
        assert_eq!(left.len(), MAX_CRASH_REPORTS);
        assert_eq!(left[0].0, 3);
    }
}
