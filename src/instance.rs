//! One FMX window at a time.
//!
//! The first FMX holds a lock file. Any later one (a second click on the
//! icon, "Open with", or a right-click conversion, which Windows starts once
//! per selected file) leaves its request in an inbox folder and exits; the
//! running FMX picks it up, waits a moment for the rest of a multi-file
//! selection, and merges what came together, so five images "to PDF" still
//! make one PDF.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use serde::{Deserialize, Serialize};

const LOCK_FILE: &str = "instance.lock";
const INBOX: &str = "inbox";
/// How often the running FMX looks in its inbox.
const POLL: Duration = Duration::from_millis(250);
/// A selection's requests arrive this close together.
const SETTLE: Duration = Duration::from_millis(400);
const MAX_SETTLE: Duration = Duration::from_secs(3);
/// Requests older than this were left by an FMX that closed; they are dropped.
const STALE: Duration = Duration::from_secs(30);

/// Held for as long as this FMX runs.
static LOCK: OnceLock<File> = OnceLock::new();

/// What a start of FMX asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    /// Just bring the window forward.
    Focus,
    /// Add files to the editor.
    Open(Vec<PathBuf>),
    /// Queue files for a right-click conversion (see `shell::tool_for`) and start.
    Convert { target: String, files: Vec<PathBuf> },
}

fn dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("fmx"))
}

/// Whether this is the only FMX running. If it is, it stays so until it exits.
pub fn acquire() -> bool {
    let Some(dir) = dir() else {
        return true;
    };
    let lock = fs::create_dir_all(&dir).and_then(|()| {
        File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK_FILE))
    });
    match lock {
        Ok(file) => match file.try_lock() {
            Ok(()) => {
                let _ = LOCK.set(file);
                true
            }
            Err(fs::TryLockError::WouldBlock) => false,
            // Without working locks, every FMX simply runs on its own.
            Err(fs::TryLockError::Error(e)) => {
                tracing::warn!(error = %e, "no single-instance lock");
                true
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "no single-instance lock");
            true
        }
    }
}

/// Leaves `request` for the running FMX.
pub fn forward(request: &Request) -> io::Result<()> {
    let inbox = dir()
        .ok_or_else(|| io::Error::other("no data folder"))?
        .join(INBOX);
    fs::create_dir_all(&inbox)?;
    let name = format!("{}-{}", now_millis(), std::process::id());
    // Written under another name first, so a half-written request is never read.
    let partial = inbox.join(format!("{name}.part"));
    fs::write(&partial, serde_json::to_vec(request)?)?;
    fs::rename(partial, inbox.join(format!("{name}.json")))
}

/// Watches the inbox on a background thread; merged requests arrive on the
/// returned channel, each with a repaint so the UI handles it at once.
pub fn watch(ctx: egui::Context) -> mpsc::Receiver<Request> {
    let (tx, rx) = mpsc::channel();
    let Some(inbox) = dir().map(|d| d.join(INBOX)) else {
        return rx;
    };
    let spawned = std::thread::Builder::new()
        .name("fmx-inbox".into())
        .spawn(move || {
            loop {
                let mut requests = take(&inbox);
                if requests.is_empty() {
                    std::thread::sleep(POLL);
                    continue;
                }
                let started = Instant::now();
                while started.elapsed() < MAX_SETTLE {
                    std::thread::sleep(SETTLE);
                    let more = take(&inbox);
                    if more.is_empty() {
                        break;
                    }
                    requests.extend(more);
                }
                for request in merge(requests) {
                    if tx.send(request).is_err() {
                        return;
                    }
                }
                ctx.request_repaint();
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not watch for requests from other FMX starts");
    }
    rx
}

/// Reads and removes the waiting requests, oldest first.
fn take(inbox: &Path) -> Vec<Request> {
    let mut entries: Vec<(u64, PathBuf)> = fs::read_dir(inbox)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_str()?;
            let millis = name.split('-').next()?.parse().ok()?;
            (path.extension()? == "json").then_some((millis, path))
        })
        .collect();
    entries.sort();
    let stale_before = now_millis().saturating_sub(STALE.as_millis() as u64);
    entries
        .into_iter()
        .filter_map(|(millis, path)| {
            let bytes = fs::read(&path).ok();
            let _ = fs::remove_file(&path);
            if millis < stale_before {
                return None;
            }
            let request = serde_json::from_slice(&bytes?);
            if let Err(e) = &request {
                tracing::warn!(error = %e, "unreadable request from another FMX start");
            }
            request.ok()
        })
        .collect()
}

/// Joins requests that came together: all files to open, and the files of
/// each conversion target, each set in name order.
fn merge(requests: Vec<Request>) -> Vec<Request> {
    let mut focus = false;
    let mut open: Vec<PathBuf> = Vec::new();
    let mut convert: Vec<(String, Vec<PathBuf>)> = Vec::new();
    for request in requests {
        match request {
            Request::Focus => focus = true,
            Request::Open(files) => open.extend(files),
            Request::Convert { target, files } => {
                match convert.iter_mut().find(|(t, _)| *t == target) {
                    Some((_, all)) => all.extend(files),
                    None => convert.push((target, files)),
                }
            }
        }
    }
    let mut merged = Vec::new();
    if !open.is_empty() {
        open.sort();
        open.dedup();
        merged.push(Request::Open(open));
    }
    for (target, mut files) in convert {
        files.sort();
        files.dedup();
        merged.push(Request::Convert { target, files });
    }
    if merged.is_empty() && focus {
        merged.push(Request::Focus);
    }
    merged
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(target: &str, files: &[&str]) -> Request {
        Request::Convert {
            target: target.to_owned(),
            files: files.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn a_selection_becomes_one_request_per_target() {
        let merged = merge(vec![
            convert("pdf", &["b.jpg"]),
            Request::Focus,
            convert("pdf", &["a.jpg"]),
            convert("mp4", &["clip.mov"]),
            convert("pdf", &["a.jpg"]),
        ]);
        assert_eq!(
            merged,
            vec![
                convert("pdf", &["a.jpg", "b.jpg"]),
                convert("mp4", &["clip.mov"])
            ]
        );
        assert_eq!(
            merge(vec![Request::Focus, Request::Focus]),
            vec![Request::Focus]
        );
    }

    #[test]
    fn requests_survive_the_inbox() {
        let dir = tempfile::tempdir().expect("tempdir");
        let request = convert("mp3", &["song.flac"]);
        let name = format!("{}-1.json", now_millis());
        fs::write(
            dir.path().join(name),
            serde_json::to_vec(&request).expect("json"),
        )
        .expect("write");
        fs::write(dir.path().join("1-2.json"), b"{}").expect("write"); // stale
        fs::write(dir.path().join("9-3.part"), b"half").expect("write"); // unfinished
        assert_eq!(take(dir.path()), vec![request]);
        assert!(take(dir.path()).is_empty());
        assert!(dir.path().join("9-3.part").exists());
    }
}
