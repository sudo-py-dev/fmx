//! Debug builds only: `FMX_SCREENSHOT=<file.png>` saves one frame of the
//! running app as a PNG and quits, for checking layouts (such as right to
//! left) without clicking through the UI. Files on the command line open the
//! task editor, and these also set the scene:
//! - `FMX_SCREENSHOT_SETTINGS=1` opens Settings (`=queue`, `=updates`,
//!   `=programs` or `=about` opens that section);
//! - `FMX_SCREENSHOT_EXPAND=1` opens every collapsible section;
//! - `FMX_SCREENSHOT_TOOL=record` opens that tool (`screenshot`, `frames`, `trim`);
//! - `FMX_SCREENSHOT_ENQUEUE=1` adds the editor's files to the queue
//!   (`FMX_SCREENSHOT_RUNNING=1` then shows the first two as running);
//! - `FMX_SCREENSHOT_SIZE=<width>x<height>` resizes the window;
//! - `FMX_SCREENSHOT_MENU=<id>` opens that drop-down (e.g. `after_done`);
//! - `FMX_SCREENSHOT_UPDATE=1` pretends a newer release was found
//!   (`=downloading:0.4` pretends it is 40 % downloaded).
//!
//! `FMX_DEBUG_PANIC=1` panics on the tenth frame, to try the crash guard.

use std::sync::atomic::{AtomicU32, Ordering};

use eframe::egui;

/// Frames to wait so fonts, layout and animations settle.
const SETTLE_FRAMES: u32 = 30;

static FRAME: AtomicU32 = AtomicU32::new(0);

/// The window size `FMX_SCREENSHOT_SIZE` asks for, set when the window opens.
/// A window manager that keeps the window bigger gets the UI zoomed instead,
/// so the layout is the one at that width.
pub fn window_size() -> Option<[f32; 2]> {
    let value = std::env::var("FMX_SCREENSHOT_SIZE").ok()?;
    let (w, h) = value.split_once('x')?;
    Some([w.parse().ok()?, h.parse().ok()?])
}

/// The Settings section the screenshot should show, if any (`1` = the first).
pub fn wants_settings() -> Option<String> {
    std::env::var("FMX_SCREENSHOT_SETTINGS").ok()
}

/// Whether collapsible sections should start open.
pub fn expand_sections() -> bool {
    std::env::var_os("FMX_SCREENSHOT_EXPAND").is_some()
}

/// The tool to open, if one was named.
pub fn wants_tool() -> Option<crate::catalog::Tool> {
    use crate::catalog::Tool;
    match std::env::var("FMX_SCREENSHOT_TOOL").ok()?.as_str() {
        "record" => Some(Tool::Record),
        "screenshot" => Some(Tool::Screenshot),
        "frames" => Some(Tool::ExtractFrames),
        "trim" => Some(Tool::Trim),
        _ => None,
    }
}

/// Whether the files on the command line should go straight into the queue.
pub fn wants_enqueue() -> bool {
    std::env::var_os("FMX_SCREENSHOT_ENQUEUE").is_some()
}

/// Whether queued jobs should look like they are running.
pub fn wants_running() -> bool {
    std::env::var_os("FMX_SCREENSHOT_RUNNING").is_some()
}

/// Whether to pretend a newer release was found: `None`, or how far its
/// download is (`Some(None)` when it isn't downloading).
pub fn wants_update() -> Option<Option<f32>> {
    let value = std::env::var("FMX_SCREENSHOT_UPDATE").ok()?;
    Some(
        value
            .strip_prefix("downloading:")
            .and_then(|f| f.parse().ok()),
    )
}

/// Whether the drop-down with this id should be shown open.
pub fn wants_menu(id: &str) -> bool {
    std::env::var("FMX_SCREENSHOT_MENU").is_ok_and(|wanted| wanted == id)
}

/// Call once per frame; does nothing unless `FMX_SCREENSHOT` is set.
pub fn tick(ctx: &egui::Context) {
    if std::env::var_os("FMX_DEBUG_PANIC").is_some() && FRAME.load(Ordering::Relaxed) == 10 {
        panic!("FMX_DEBUG_PANIC: a crash on purpose");
    }
    if std::env::var_os("FMX_DEBUG_PANIC").is_some() {
        FRAME.fetch_add(1, Ordering::Relaxed);
        ctx.request_repaint();
    }
    let Some(path) = std::env::var_os("FMX_SCREENSHOT") else {
        return;
    };
    let frame = FRAME.fetch_add(1, Ordering::Relaxed);
    ctx.request_repaint();
    // Until the window has its final size, keep the zoom matching it.
    if frame < SETTLE_FRAMES / 2
        && let Some([width, _]) = window_size()
    {
        let actual = ctx.content_rect().width() * ctx.zoom_factor();
        let zoom = (actual / width).max(1.0);
        if (zoom - ctx.zoom_factor()).abs() > 0.01 {
            ctx.set_zoom_factor(zoom);
        }
    }
    if frame != SETTLE_FRAMES {
        return;
    }
    let close = ctx.clone();
    let callback = egui::ScreenshotCallback::new(move |image| {
        let [width, height] = image.size.map(|n| n as u32);
        let rgba = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        match image::RgbaImage::from_raw(width, height, rgba).map(|img| img.save(&path)) {
            Some(Ok(())) => {}
            other => tracing::error!(?other, "could not save the screenshot"),
        }
        close.send_viewport_cmd(egui::ViewportCommand::Close);
        close.request_repaint();
    });
    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
}
