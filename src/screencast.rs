//! Screen capture on Wayland, where programs may not read the screen
//! themselves: the XDG desktop portal asks which screen or window to share,
//! PipeWire delivers its frames, and GStreamer hands them to ffmpeg as a
//! YUV4MPEG stream. Screenshots come from the portal as a saved file.

use std::collections::HashMap;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

use zbus::blocking::{Connection, Proxy, proxy::Builder};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{self, ObjectPath, OwnedObjectPath, OwnedValue, Value};

use crate::process;

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SCREEN_CAST: &str = "org.freedesktop.portal.ScreenCast";
const SCREENSHOT: &str = "org.freedesktop.portal.Screenshot";
/// Source types the system dialog offers: monitors and windows.
const MONITOR_OR_WINDOW: u32 = 1 | 2;
const CURSOR_HIDDEN: u32 = 1;
const CURSOR_EMBEDDED: u32 = 2;

#[derive(Debug)]
pub enum CastError {
    /// The user closed the system's sharing dialog.
    Cancelled,
    Failed(String),
}

impl From<zbus::Error> for CastError {
    fn from(e: zbus::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

impl From<zvariant::Error> for CastError {
    fn from(e: zvariant::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

/// Whether this session is Wayland, where the portal is the way to record.
pub fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// A screen or window the user agreed to share. Sharing ends when it drops.
pub struct Cast {
    conn: Connection,
    session: OwnedObjectPath,
    remote: OwnedFd,
    node: u32,
}

impl Cast {
    /// Asks the user what to share (the system shows its own dialog) and
    /// blocks until they answer.
    pub fn start(cursor: bool) -> Result<Self, CastError> {
        let conn = Connection::session()?;
        let (session, node, remote) = {
            let mut portal = Portal::new(&conn, SCREEN_CAST)?;
            let session = portal.create_session()?;
            portal.select_sources(&session, cursor)?;
            let node = portal.start(&session)?;
            let remote: zvariant::OwnedFd = portal.proxy.call(
                "OpenPipeWireRemote",
                &(&session, HashMap::<&str, Value<'_>>::new()),
            )?;
            (session, node, remote)
        };
        Ok(Self {
            remote: remote.into(),
            conn,
            session,
            node,
        })
    }

    /// Starts GStreamer writing the shared picture to its stdout as YUV4MPEG
    /// frames at a steady `fps`; it finishes the stream when sent SIGINT.
    pub fn spawn_frames(&self, gst_launch: &Path, fps: u32) -> std::io::Result<Child> {
        let fd = self.remote.as_fd();
        let pipeline = format!(
            "pipewiresrc fd={fd} path={node} do-timestamp=true keepalive-time=1000 \
             ! videorate ! video/x-raw,framerate={fps}/1 \
             ! videoconvert ! video/x-raw,format=I420 ! y4menc ! fdsink fd=1",
            fd = std::os::fd::AsRawFd::as_raw_fd(&fd),
            node = self.node,
        );
        let mut cmd = process::command(gst_launch);
        cmd.args(["-q", "-e"])
            .args(pipeline.split_whitespace())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // GStreamer opens PipeWire through our connection, so it must inherit it.
        rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::empty())?;
        let child = cmd.spawn();
        rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::CLOEXEC)?;
        child
    }
}

impl Drop for Cast {
    fn drop(&mut self) {
        let closed = Proxy::new(
            &self.conn,
            DESTINATION,
            self.session.as_ref(),
            "org.freedesktop.portal.Session",
        )
        .and_then(|session| session.call_method("Close", &()));
        if let Err(err) = closed {
            tracing::debug!("screen cast session did not close: {err}");
        }
    }
}

/// Takes a screenshot of every screen, or of an area the user picks in the
/// system's dialog when `interactive`. Returns the file the portal saved.
pub fn screenshot(interactive: bool) -> Result<PathBuf, CastError> {
    let conn = Connection::session()?;
    let mut portal = Portal::new(&conn, SCREENSHOT)?;
    let token = portal.token();
    let options = HashMap::from([
        ("handle_token", Value::from(token.as_str())),
        ("interactive", Value::from(interactive)),
    ]);
    let results = portal.request("Screenshot", &token, &("", options))?;
    match results.get("uri").map(|v| &**v) {
        Some(Value::Str(uri)) => file_path(uri.as_str())
            .ok_or_else(|| CastError::Failed(format!("not a local file: {uri}"))),
        _ => Err(CastError::Failed("no screenshot".to_owned())),
    }
}

/// `file:///home/a/Shot%201.png` → `/home/a/Shot 1.png`.
fn file_path(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let encoded = uri.strip_prefix("file://")?.as_bytes();
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut i = 0;
    while i < encoded.len() {
        let hex = encoded
            .get(i + 1..i + 3)
            .filter(|_| encoded[i] == b'%')
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match hex {
            Some(byte) => {
                bytes.push(byte);
                i += 3;
            }
            None => {
                bytes.push(encoded[i]);
                i += 1;
            }
        }
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// Stops a frame pipeline gracefully, so its stream ends cleanly.
pub fn interrupt(child: &mut Child) {
    let pid = rustix::process::Pid::from_child(child);
    if rustix::process::kill_process(pid, rustix::process::Signal::INT).is_err() {
        let _ = child.kill();
    }
}

struct Portal<'a> {
    conn: &'a Connection,
    proxy: Proxy<'a>,
    /// Our bus name as the portal writes it in request paths.
    sender: String,
    requests: u32,
}

impl<'a> Portal<'a> {
    fn new(conn: &'a Connection, interface: &'a str) -> Result<Self, CastError> {
        let proxy = uncached(conn, PATH, interface)?;
        let sender = conn
            .unique_name()
            .map(|n| n.trim_start_matches(':').replace('.', "_"))
            .ok_or_else(|| CastError::Failed("no D-Bus name".to_owned()))?;
        Ok(Self {
            conn,
            proxy,
            sender,
            requests: 0,
        })
    }

    fn token(&mut self) -> String {
        self.requests += 1;
        format!("fmx{}_{}", std::process::id(), self.requests)
    }

    /// Calls a method that answers later through a Request object.
    fn request<B>(&mut self, method: &str, token: &str, body: &B) -> Result<Results, CastError>
    where
        B: serde::Serialize + zvariant::DynamicType,
    {
        let path = format!("{PATH}/request/{}/{token}", self.sender);
        let request = uncached(self.conn, &path, "org.freedesktop.portal.Request")?;
        // Listen before calling, so the answer cannot slip past.
        let mut responses = request.receive_signal("Response")?;
        self.proxy.call_method(method, body)?;
        let message = responses
            .next()
            .ok_or_else(|| CastError::Failed(format!("{method}: no answer")))?;
        let (code, results): (u32, Results) = message.body().deserialize()?;
        match code {
            0 => Ok(results),
            1 => Err(CastError::Cancelled),
            _ => Err(CastError::Failed(format!("{method} failed"))),
        }
    }

    fn create_session(&mut self) -> Result<OwnedObjectPath, CastError> {
        let token = self.token();
        let session_token = self.token();
        let options = HashMap::from([
            ("handle_token", Value::from(token.as_str())),
            ("session_handle_token", Value::from(session_token.as_str())),
        ]);
        let results = self.request("CreateSession", &token, &(options,))?;
        // The spec says a string; some portals send an object path.
        match results.get("session_handle").map(|v| &**v) {
            Some(Value::Str(s)) => Ok(ObjectPath::try_from(s.as_str())?.into()),
            Some(Value::ObjectPath(p)) => Ok(p.clone().into()),
            _ => Err(CastError::Failed("no screen cast session".to_owned())),
        }
    }

    fn select_sources(&mut self, session: &OwnedObjectPath, cursor: bool) -> Result<(), CastError> {
        let token = self.token();
        let mut options = HashMap::from([
            ("handle_token", Value::from(token.as_str())),
            ("types", Value::from(MONITOR_OR_WINDOW)),
            ("multiple", Value::from(false)),
        ]);
        let wanted = if cursor {
            CURSOR_EMBEDDED
        } else {
            CURSOR_HIDDEN
        };
        let modes: u32 = self.proxy.get_property("AvailableCursorModes").unwrap_or(0);
        if modes & wanted != 0 {
            options.insert("cursor_mode", Value::from(wanted));
        }
        self.request("SelectSources", &token, &(session, options))?;
        Ok(())
    }

    /// Shows the system's dialog; returns the PipeWire node of the choice.
    fn start(&mut self, session: &OwnedObjectPath) -> Result<u32, CastError> {
        let token = self.token();
        let options = HashMap::from([("handle_token", Value::from(token.as_str()))]);
        let results = self.request("Start", &token, &(session, "", options))?;
        // streams: a(ua{sv}), the node id first in each.
        if let Some(Value::Array(streams)) = results.get("streams").map(|v| &**v)
            && let Some(Value::Structure(stream)) = streams.iter().next()
            && let Some(Value::U32(node)) = stream.fields().first()
        {
            return Ok(*node);
        }
        Err(CastError::Failed("nothing was shared".to_owned()))
    }
}

type Results = HashMap<String, OwnedValue>;

fn uncached<'a>(conn: &Connection, path: &str, interface: &'a str) -> zbus::Result<Proxy<'a>> {
    Builder::new(conn)
        .destination(DESTINATION)?
        .path(path.to_owned())?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn file_uris_decode_to_paths() {
        assert_eq!(
            file_path("file:///home/a/Screenshot%20from%202026.png"),
            Some(PathBuf::from("/home/a/Screenshot from 2026.png"))
        );
        assert_eq!(file_path("https://x/y.png"), None);
    }

    /// Needs a person to take the picture: `cargo test -- --ignored portal_screenshot`.
    #[test]
    #[ignore]
    fn portal_screenshot_interactive() {
        let path = screenshot(true).expect("screenshot");
        assert!(path.is_file(), "{}", path.display());
        eprintln!("saved {}", path.display());
    }

    /// Needs a person to answer the system's dialog: `cargo test -- --ignored portal`.
    #[test]
    #[ignore]
    fn portal_shares_frames() {
        let cast = Cast::start(true).expect("shared");
        let mut frames = cast
            .spawn_frames(Path::new("gst-launch-1.0"), 10)
            .expect("gst-launch-1.0");
        let mut head = [0u8; 9];
        frames
            .stdout
            .take()
            .expect("stdout")
            .read_exact(&mut head)
            .expect("frames");
        interrupt(&mut frames);
        let _ = frames.wait();
        assert_eq!(&head, b"YUV4MPEG2");
    }
}
