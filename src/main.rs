//! Fmx — a lightweight, fast media converter.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod art;
mod catalog;
mod cso;
mod disc;
mod docs;
mod download;
mod exec;
mod externals;
mod ffmpeg;
mod ffmpeg_setup;
mod i18n;
mod icons;
mod imaging;
mod jobs;
mod office;
mod options;
mod pdfgen;
mod plan;
mod preview;
mod probe;
mod process;
mod queue;
mod settings;
mod sheet;
mod theme;
mod update;
mod widgets;
mod ytdlp_setup;

use eframe::egui;
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "fmx";
const INITIAL_SIZE: [f32; 2] = [1120.0, 760.0];
const MIN_SIZE: [f32; 2] = [760.0, 520.0];

/// The app icon for the window and taskbar (from `packaging/make-icons.py`).
fn window_icon() -> egui::IconData {
    let png = include_bytes!("../packaging/icons/fmx-256.png");
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("bundled icon is a valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("FMX_LOG")
                .or_else(|_| EnvFilter::try_from_env("Fmx_LOG"))
                .unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();
    if let Ok(lang) = std::env::var("FMX_LANG").or_else(|_| std::env::var("Fmx_LANG")) {
        i18n::set_language(Some(&lang));
    }

    // Files passed on the command line (e.g. "Open with Fmx") go into the file list.
    let open_files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(i18n::tr("app.name"))
            .with_app_id(APP_ID)
            .with_inner_size(INITIAL_SIZE)
            .with_min_inner_size(MIN_SIZE)
            .with_drag_and_drop(true)
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(|cc| Ok(Box::new(app::FmxApp::new(cc, open_files)))),
    )
}
