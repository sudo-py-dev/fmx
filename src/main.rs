//! Fmx — a lightweight, fast media converter.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod art;
mod catalog;
mod cso;
#[cfg(debug_assertions)]
mod devshot;
mod diagnostics;
mod dir;
mod disc;
mod docs;
mod download;
mod ebook;
mod exec;
mod externals;
mod ffmpeg;
mod i18n;
mod icons;
mod imaging;
mod instance;
mod jobs;
mod net;
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
mod shell;
mod sound;
mod theme;
mod update;
mod widgets;

use eframe::egui;

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

fn initial_size() -> [f32; 2] {
    #[cfg(debug_assertions)]
    if let Some(size) = devshot::window_size() {
        return size;
    }
    INITIAL_SIZE
}

/// Command-line switches; everything else is a file to open.
const DEBUG_FLAG: &str = "--debug";
/// Opens the window, draws a few frames and quits: a check that FMX starts here.
const SELF_TEST_FLAG: &str = "--self-test";
/// Add or remove the right-click menu, then quit (used by the Windows installer).
const REGISTER_SHELL_FLAG: &str = "--register-shell";
const UNREGISTER_SHELL_FLAG: &str = "--unregister-shell";
/// Set on the copy of FMX started with OpenGL after the default renderer failed.
const RENDERER_ENV: &str = "FMX_RENDERER";

/// Direct3D or Vulkan through wgpu on Windows, where OpenGL is often missing or
/// broken (virtual machines, remote desktops, basic display drivers); OpenGL
/// elsewhere. `FMX_RENDERER=glow` forces OpenGL.
#[cfg(windows)]
fn renderer() -> eframe::Renderer {
    if std::env::var(RENDERER_ENV).is_ok_and(|r| r == "glow") {
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    }
}

#[cfg(not(windows))]
fn renderer() -> eframe::Renderer {
    eframe::Renderer::Glow
}

fn main() {
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let debug = take_flag(&mut args, DEBUG_FLAG);
    let self_test = take_flag(&mut args, SELF_TEST_FLAG);
    let register_shell = take_flag(&mut args, REGISTER_SHELL_FLAG);
    let unregister_shell = take_flag(&mut args, UNREGISTER_SHELL_FLAG);
    let convert = take_value(&mut args, shell::CONVERT_FLAG);
    let shell_only = register_shell || unregister_shell;
    // Test and screenshot runs stay out of the way of a running FMX.
    let single_instance = !self_test
        && !shell_only
        && !(cfg!(debug_assertions) && std::env::var_os("FMX_SCREENSHOT").is_some());
    // Taken before logging starts, so only the FMX that opens a window starts a new log.
    let primary = !single_instance || instance::acquire();
    diagnostics::init(
        debug || self_test,
        self_test || shell_only,
        primary && !shell_only,
    );
    if let Ok(lang) = std::env::var("FMX_LANG").or_else(|_| std::env::var("Fmx_LANG")) {
        i18n::set_language(Some(&lang));
    }
    if shell_only {
        let done = if register_shell {
            shell::register()
        } else {
            shell::unregister()
        };
        if let Err(e) = done {
            tracing::error!(error = %e, "could not change the right-click menu");
            std::process::exit(1);
        }
        return;
    }

    // Files passed on the command line ("Open with", the right-click menu).
    let files: Vec<std::path::PathBuf> = args.iter().map(Into::into).collect();
    let request = match convert {
        Some(target) => instance::Request::Convert { target, files },
        None if files.is_empty() => instance::Request::Focus,
        None => instance::Request::Open(files),
    };
    if !primary {
        match instance::forward(&request) {
            Ok(()) => {
                tracing::info!(?request, "handed over to the running FMX");
                return;
            }
            Err(e) => tracing::warn!(error = %e, "could not reach the running FMX"),
        }
    }

    let renderer = renderer();
    tracing::info!(?renderer, self_test, "opening the window");
    let launch = app::Launch {
        request,
        self_test,
        single_instance,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(i18n::tr("app.name"))
            .with_app_id(APP_ID)
            .with_inner_size(initial_size())
            .with_min_inner_size(MIN_SIZE)
            .with_drag_and_drop(true)
            .with_icon(window_icon()),
        renderer,
        ..Default::default()
    };
    let result = eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| Ok(Box::new(app::FmxApp::new(cc, launch)))),
    );
    let error = match result {
        Ok(()) => {
            tracing::info!("FMX closed");
            return;
        }
        Err(error) => error,
    };
    match retry_with_opengl(renderer, &error) {
        // A self-test passes or fails with the copy that took over.
        Some(mut copy) if self_test => {
            let code = copy.wait().ok().and_then(|s| s.code()).unwrap_or(1);
            std::process::exit(code);
        }
        Some(_) => {}
        None => {
            diagnostics::report_startup_failure(&error);
            std::process::exit(1);
        }
    }
}

/// Removes `flag` and the value after it from `args`, returning the value.
fn take_value(args: &mut Vec<std::ffi::OsString>, flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    args.remove(at);
    (at < args.len()).then(|| args.remove(at).to_string_lossy().into_owned())
}

/// Removes `flag` from `args`, returning whether it was there.
fn take_flag(args: &mut Vec<std::ffi::OsString>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

/// After the default renderer failed, starts FMX again with OpenGL (a window
/// can only be opened once per process). Returns that copy, if it started.
fn retry_with_opengl(
    renderer: eframe::Renderer,
    error: &eframe::Error,
) -> Option<std::process::Child> {
    if renderer == eframe::Renderer::Glow || std::env::var_os(RENDERER_ENV).is_some() {
        return None;
    }
    tracing::warn!(%error, "the default renderer failed; trying OpenGL");
    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env(RENDERER_ENV, "glow")
            .spawn()
    });
    started
        .inspect_err(|e| tracing::error!(error = %e, "could not start FMX again with OpenGL"))
        .ok()
}
