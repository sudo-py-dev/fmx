//! Debug builds only: `FMX_SCREENSHOT=<file.png>` saves one frame of the
//! running app as a PNG and quits, for checking layouts (such as right to
//! left) without clicking through the UI. Files on the command line open the
//! task editor, and these also set the scene:
//! - `FMX_SCREENSHOT_SETTINGS=1` opens Settings;
//! - `FMX_SCREENSHOT_EXPAND=1` opens every collapsible section;
//! - `FMX_SCREENSHOT_ENQUEUE=1` adds the editor's files to the queue;
//! - `FMX_SCREENSHOT_SIZE=<width>x<height>` resizes the window;
//! - `FMX_SCREENSHOT_MENU=<id>` opens that drop-down (e.g. `after_done`);
//! - `FMX_SCREENSHOT_UPDATE=1` pretends a newer release was found.

use std::sync::atomic::{AtomicU32, Ordering};

use eframe::egui;

/// Frames to wait so fonts, layout and animations settle.
const SETTLE_FRAMES: u32 = 30;

static FRAME: AtomicU32 = AtomicU32::new(0);

/// Whether the screenshot should show the Settings window.
pub fn wants_settings() -> bool {
    std::env::var_os("FMX_SCREENSHOT_SETTINGS").is_some()
}

/// Whether collapsible sections should start open.
pub fn expand_sections() -> bool {
    std::env::var_os("FMX_SCREENSHOT_EXPAND").is_some()
}

/// Whether the files on the command line should go straight into the queue.
pub fn wants_enqueue() -> bool {
    std::env::var_os("FMX_SCREENSHOT_ENQUEUE").is_some()
}

/// Whether to pretend a newer release was found.
pub fn wants_update() -> bool {
    std::env::var_os("FMX_SCREENSHOT_UPDATE").is_some()
}

/// Whether the drop-down with this id should be shown open.
pub fn wants_menu(id: &str) -> bool {
    std::env::var("FMX_SCREENSHOT_MENU").is_ok_and(|wanted| wanted == id)
}

/// Call once per frame; does nothing unless `FMX_SCREENSHOT` is set.
pub fn tick(ctx: &egui::Context) {
    let Some(path) = std::env::var_os("FMX_SCREENSHOT") else {
        return;
    };
    let frame = FRAME.fetch_add(1, Ordering::Relaxed);
    ctx.request_repaint();
    if frame == 0
        && let Some((w, h)) = std::env::var("FMX_SCREENSHOT_SIZE")
            .ok()
            .and_then(|s| s.split_once('x').map(|(w, h)| (w.parse(), h.parse())))
            .and_then(|(w, h)| Some((w.ok()?, h.ok()?)))
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
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
