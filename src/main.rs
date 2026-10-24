//! Muxr — a lightweight, fast media converter.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod catalog;
mod cso;
mod docs;
mod exec;
mod externals;
mod ffmpeg;
mod i18n;
mod icons;
mod imaging;
mod jobs;
mod options;
mod plan;
mod probe;
mod process;
mod queue;
mod settings;

use eframe::egui;
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "muxr";
const INITIAL_SIZE: [f32; 2] = [1120.0, 760.0];
const MIN_SIZE: [f32; 2] = [760.0, 520.0];

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("MUXR_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();
    if let Ok(lang) = std::env::var("MUXR_LANG") {
        i18n::set_language(Some(&lang));
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(i18n::tr("app.name"))
            .with_app_id(APP_ID)
            .with_inner_size(INITIAL_SIZE)
            .with_min_inner_size(MIN_SIZE)
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(|cc| Ok(Box::new(app::MuxrApp::new(cc)))),
    )
}
