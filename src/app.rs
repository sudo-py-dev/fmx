//! The egui front end: tool sidebar, task editor, and job queue.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, ComboBox, CornerRadius, DragValue, Id, Key, KeyboardShortcut, Label, Margin,
    Modifiers, RichText, ScrollArea, Slider, Stroke, StrokeKind, TextEdit,
};

use crate::art;
use crate::capture;
use crate::catalog::{
    self, AudioFormat, Category, ImageFormat, InputArity, InputKind, Tool, VideoFormat,
};
use crate::diagnostics;
use crate::dir;
use crate::docs;
use crate::download;
use crate::exec;
use crate::externals::{External, Toolbox};
use crate::ffmpeg;
use crate::i18n::{self, tr, trf};
use crate::icons;
use crate::instance;
use crate::jobs::{JobSpec, Selection, allocate_output, plan_jobs};
use crate::options::{
    self, Channels, Corner, DownloadKind, Edit, FrameMode, HwAccel, JoinLayout, MixLayout, Options,
    Quality, Rect, Resolution, Rotation,
};
use crate::preview::{Mode as PreviewMode, Preview};
use crate::process::spawn_detached;
use crate::queue::{Job, JobId, JobState, Notifier, Queue};
use crate::settings::{MAX_PARALLEL_JOBS, STORAGE_KEY, Settings};
use crate::shell;
use crate::sound;
use crate::theme::{self, MOTION, ThemeChoice};
use crate::update;
use crate::widgets::{self, Kind, Size, ToastKind, Toasts};

const ADD_FILES: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const ENQUEUE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
/// FMX's GitHub page, for the links in Settings → About.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const SIDEBAR_WIDTH: f32 = 230.0;
/// The height of the header bar's row, matched by the sidebar's title row.
const HEADER_HEIGHT: f32 = 30.0;
const QUEUE_HEIGHT: f32 = 180.0;
const PROGRESS_WIDTH: f32 = 220.0;
/// The task editor's content is kept to a usable width on very wide windows.
const EDITOR_MAX_WIDTH: f32 = 1320.0;
/// Below this width the editor stacks its two panels and scrolls.
const TWO_COLUMN_MIN_WIDTH: f32 = 760.0;
const PANEL_GAP: f32 = 14.0;
/// Share of the width the file panel gets, beside settings or a video preview.
const FILES_SHARE: f32 = 0.46;
const FILES_SHARE_BESIDE_PREVIEW: f32 = 0.34;
const MIN_DROP_HEIGHT: f32 = 120.0;
/// Below this panel width "Add folder" and "Clear list" show only their icon.
const COMPACT_BUTTONS_WIDTH: f32 = 460.0;
/// Sizes used when the panels are stacked on narrow windows.
const STACKED_DROP_HEIGHT: f32 = 170.0;
const STACKED_LIST_HEIGHT: f32 = 260.0;
const STACKED_PREVIEW_HEIGHT: f32 = 320.0;
/// Room left under the inline video for its timeline and controls; the
/// options below it scroll into view.
const PREVIEW_RESERVE_CLIP: f32 = 190.0;
const PREVIEW_RESERVE_AREA: f32 = 160.0;
const MIN_PREVIEW_HEIGHT: f32 = 200.0;
/// How long typing must pause before a pasted link is looked up.
const LINK_LOOKUP_DELAY: Duration = Duration::from_millis(700);
const THUMBNAIL_WIDTH: f32 = 200.0;
const FILE_TILE: f32 = 32.0;
/// Only this many files of one batch are probed for their length.
const MAX_PROBED_FILES: usize = 200;
/// How many extensions the drop zone lists before "…".
const SHOWN_EXTENSIONS: usize = 10;
const SHUTDOWN_DELAY: Duration = Duration::from_secs(60);
const SETTINGS_WIDTH: f32 = 820.0;
const SETTINGS_HEIGHT: f32 = 540.0;
const SETTINGS_NAV_WIDTH: f32 = 190.0;
/// Space kept free around the Settings window on small screens.
const SETTINGS_SCREEN_MARGIN: f32 = 80.0;
/// Task-list files larger than this are refused.
const MAX_TASK_LIST_BYTES: u64 = 10 << 20;
/// Folder scans stop after this many files.
const MAX_SCAN_FILES: usize = 10_000;
/// Frames a `--self-test` run draws before closing.
const SELF_TEST_FRAMES: u32 = 30;
/// Progress below this is too early to estimate time left.
const MIN_PROGRESS_FOR_ETA: f32 = 0.03;

/// What a pasted link points to, fetched with yt-dlp before downloading.
struct LinkPreview {
    url: String,
    playlist: bool,
    state: LinkState,
    thumbnail: Option<egui::TextureHandle>,
    thumbnail_rx: Option<mpsc::Receiver<Result<image::RgbaImage, String>>>,
}

enum LinkState {
    Loading(mpsc::Receiver<Result<download::LinkInfo, String>>),
    Ready(download::LinkInfo),
    Failed(String),
}

/// Where FMX is with its own updates.
enum UpdateState {
    Idle,
    Checking {
        /// Started from Settings, so the result is always shown.
        manual: bool,
        rx: mpsc::Receiver<Result<Option<update::Release>, String>>,
    },
    UpToDate,
    Available(update::Release),
    Downloading {
        release: update::Release,
        /// Progress as `f32` bits.
        progress: Arc<AtomicU32>,
        cancel: Arc<AtomicBool>,
        rx: mpsc::Receiver<Result<update::Installed, String>>,
    },
    Installed(update::Release, update::Installed),
    Failed(String),
}

/// Seconds since 1970, for spacing out update checks.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A section of the Settings window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsTab {
    General,
    Queue,
    Updates,
    Programs,
    About,
}

impl SettingsTab {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Queue,
        Self::Updates,
        Self::Programs,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => tr("settings.section_general_title"),
            Self::Queue => tr("settings.section_queue_title"),
            Self::Updates => tr("settings.section_updates_title"),
            Self::Programs => tr("settings.section_programs_title"),
            Self::About => tr("settings.section_about_title"),
        }
    }

    fn art(self) -> art::Art {
        let (glyph, hue) = match self {
            Self::General => (art::glyph::SLIDERS, art::hue::BLUE),
            Self::Queue => (art::glyph::STACK, art::hue::VIOLET),
            Self::Updates => (art::glyph::DOWNLOAD, art::hue::GREEN),
            Self::Programs => (art::glyph::WRENCH, art::hue::AMBER),
            Self::About => (art::glyph::INFO, art::hue::SLATE),
        };
        art::Art { glyph, hue }
    }
}

/// What happens once the whole queue has finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterDone {
    Nothing,
    OpenFolder,
    Shutdown,
}

impl AfterDone {
    const ALL: [Self; 3] = [Self::Nothing, Self::OpenFolder, Self::Shutdown];

    fn label(self) -> &'static str {
        match self {
            Self::Nothing => tr("after.nothing"),
            Self::OpenFolder => tr("after.open_folder"),
            Self::Shutdown => tr("after.shutdown"),
        }
    }
}

pub struct FmxApp {
    settings: Settings,
    queue: Queue,
    toolbox: Toolbox,
    tool: Tool,
    files: Vec<PathBuf>,
    url: String,
    toasts: Toasts,
    show_settings: bool,
    settings_tab: SettingsTab,
    media_info: Option<(String, String)>,
    preview: Option<Preview>,
    /// The category shown in the tool grid.
    category: Category,
    /// Whether the selected tool's editor is open (otherwise the tool grid shows).
    in_editor: bool,
    search: String,
    /// Put the keyboard in the web-address box on the next frame.
    focus_url: bool,
    /// When the web address last changed, to wait for typing to pause.
    url_edited: Option<Instant>,
    link: Option<LinkPreview>,
    /// What the screen recorder can capture, found in the background when
    /// the recorder first opens.
    capture_devices: Option<capture::Devices>,
    capture_rx: Option<mpsc::Receiver<capture::Devices>>,
    /// Moments typed for Extract Frames, before they are added.
    frame_times_text: String,
    /// The typed moment that could not be read, if any.
    frame_times_error: Option<String>,
    /// Which added file the inline preview (Trim, Crop, Remove logo) shows.
    preview_index: usize,
    /// Size and length of added files, for the file list.
    details: HashMap<PathBuf, FileDetails>,
    probe_tx: mpsc::Sender<(PathBuf, Option<f64>)>,
    probe_rx: mpsc::Receiver<(PathBuf, Option<f64>)>,
    after_done: AfterDone,
    /// The theme last given to the window frame.
    window_theme: Option<egui::Theme>,
    was_busy: bool,
    /// Jobs done before the queue last started, so only this run's count.
    done_before_run: usize,
    /// "Stop all" was pressed in this run: no sound, notice or shutdown after it.
    run_stopped: bool,
    shutdown_at: Option<Instant>,
    notifier: Notifier,
    update: UpdateState,
    /// Debug logging, as switched in Settings (see `diagnostics`).
    debug_logging: bool,
    /// Frames left before a `--self-test` run closes itself.
    self_test_frames: Option<u32>,
    /// Requests from later starts of FMX (see `instance`).
    requests: Option<mpsc::Receiver<instance::Request>>,
    /// The right-click menu, as switched in Settings (see `shell`).
    shell_menu: bool,
}

/// How FMX was started.
pub struct Launch {
    /// What the command line asked for.
    pub request: instance::Request,
    /// Close the window once a few frames were drawn (`--self-test`).
    pub self_test: bool,
    /// This is the one running FMX; later starts hand their requests to it.
    pub single_instance: bool,
}

impl FmxApp {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: Launch) -> Self {
        let Launch {
            request,
            self_test,
            single_instance,
        } = launch;
        icons::install(&cc.egui_ctx);
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        settings.sanitize();
        theme::apply(&cc.egui_ctx, settings.theme, settings.reduce_motion);
        #[cfg(target_os = "linux")]
        theme::watch_desktop_theme(&cc.egui_ctx);
        // Clip/crop/subtitle edits belong to one batch, not to the next session.
        settings.options.edit = Edit::default();
        // FMX_LANG (applied in main) wins unless a language was chosen in Settings.
        if settings.language.is_some() || std::env::var_os("FMX_LANG").is_none() {
            i18n::set_language(settings.language.as_deref());
        }
        let ctx = cc.egui_ctx.clone();
        let toolbox = Toolbox::detect(settings.ffmpeg_path.as_deref());
        if toolbox.get(External::Ffmpeg).is_none() {
            tracing::warn!("ffmpeg not found; media tools are disabled");
        }
        let (probe_tx, probe_rx) = mpsc::channel();
        let mut app = Self {
            tool: settings.last_tool,
            queue: Queue::new(),
            toolbox,
            files: Vec::new(),
            url: String::new(),
            toasts: Toasts::default(),
            show_settings: false,
            settings_tab: SettingsTab::General,
            media_info: None,
            preview: None,
            category: settings.last_tool.category(),
            in_editor: false,
            search: String::new(),
            focus_url: false,
            url_edited: None,
            link: None,
            capture_devices: None,
            capture_rx: None,
            frame_times_text: String::new(),
            frame_times_error: None,
            preview_index: 0,
            details: HashMap::new(),
            probe_tx,
            probe_rx,
            after_done: AfterDone::Nothing,
            window_theme: None,
            was_busy: false,
            done_before_run: 0,
            run_stopped: false,
            shutdown_at: None,
            notifier: Arc::new(move || ctx.request_repaint()),
            update: UpdateState::Idle,
            debug_logging: diagnostics::debug_enabled(),
            self_test_frames: None,
            requests: None,
            shell_menu: shell::is_registered(),
            settings,
        };
        let due = now_secs().saturating_sub(app.settings.last_update_check)
            >= update::CHECK_INTERVAL_SECS;
        if self_test {
            app.self_test_frames = Some(SELF_TEST_FRAMES);
        } else {
            if app.settings.check_updates && due {
                app.start_update_check(&cc.egui_ctx, false);
            }
            if let Some(report) = diagnostics::take_unseen_crash() {
                tracing::info!(report = %report.display(), "the last run crashed");
                app.toasts.push(ToastKind::Error, tr("crash.last_time"));
            }
        }
        #[cfg(debug_assertions)]
        {
            if let Some(downloaded) = crate::devshot::wants_update() {
                let release = update::Release::sample();
                app.update = match downloaded {
                    None => UpdateState::Available(release),
                    Some(fraction) => {
                        let (tx, rx) = mpsc::channel();
                        // Kept open, so the pretend download never ends.
                        std::mem::forget(tx);
                        UpdateState::Downloading {
                            release,
                            progress: Arc::new(AtomicU32::new(fraction.to_bits())),
                            cancel: Arc::new(AtomicBool::new(false)),
                            rx,
                        }
                    }
                };
                app.settings_tab = SettingsTab::Updates;
            }
            if let Some(tool) = crate::devshot::wants_tool() {
                app.select_tool(tool);
            }
            if let Some(section) = crate::devshot::wants_settings() {
                app.show_settings = true;
                app.settings_tab = match section.as_str() {
                    "queue" => SettingsTab::Queue,
                    "updates" => SettingsTab::Updates,
                    "programs" => SettingsTab::Programs,
                    "about" => SettingsTab::About,
                    _ => app.settings_tab,
                };
            }
        }
        if !self_test && shell::supported() {
            app.setup_shell_menu();
        }
        if single_instance {
            app.requests = Some(instance::watch(cc.egui_ctx.clone()));
            // Through the inbox, so it joins the rest of a multi-file selection.
            if request != instance::Request::Focus
                && let Err(e) = instance::forward(&request)
            {
                tracing::warn!(error = %e, "could not queue the start request");
                app.handle_request(&cc.egui_ctx, request);
            }
        } else {
            app.handle_request(&cc.egui_ctx, request);
        }
        #[cfg(debug_assertions)]
        if crate::devshot::wants_enqueue() && app.can_enqueue() {
            app.enqueue();
            if crate::devshot::wants_running() {
                // A download's own detail line, and a step of unknown length.
                let done = trf(
                    "download.progress_of",
                    &[("done", &"3.1 MB"), ("total", &"7.8 MB")],
                );
                let left = trf("download.left", &[("time", &"0:08")]);
                let detail = format!("{done} · 551.9 KB/s · {left}");
                app.queue.pretend_running(0, Some(0.42), Some(detail));
                app.queue
                    .pretend_running(1, None, Some(tr("download.step_merge").to_owned()));
            }
        }
        app
    }

    /// Acts on a start of FMX: this one's command line, or a later one's.
    fn handle_request(&mut self, ctx: &egui::Context, request: instance::Request) {
        tracing::info!(?request, "request");
        match request {
            instance::Request::Focus => {}
            instance::Request::Open(files) => {
                // Pick a tool for the files when the current one doesn't take them.
                if let Some(tool) = files
                    .first()
                    .filter(|f| !self.accepts(f))
                    .and_then(|f| shell::default_tool(f))
                {
                    self.select_tool(tool);
                }
                self.add_files(files);
                self.in_editor = !self.files.is_empty();
            }
            instance::Request::Convert { target, files } => self.quick_convert(&target, files),
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// A right-click conversion: queues `files` with the current settings and
    /// starts at once, leaving whatever is open in the editor alone.
    fn quick_convert(&mut self, target: &str, files: Vec<PathBuf>) {
        let mut by_tool: Vec<(Tool, Vec<PathBuf>)> = Vec::new();
        let mut skipped = 0usize;
        for file in files {
            match shell::tool_for(target, &file).filter(|_| file.is_file()) {
                Some(tool) => match by_tool.iter_mut().find(|(t, _)| *t == tool) {
                    Some((_, list)) => list.push(file),
                    None => by_tool.push((tool, vec![file])),
                },
                None => skipped += 1,
            }
        }
        // No clip, crop or watermark: those were set for some other files.
        let options = Options {
            edit: Edit::default(),
            ..self.settings.options.clone()
        };
        let mut claimed: HashSet<PathBuf> = self.queue.claimed_outputs().cloned().collect();
        let mut queued = 0usize;
        for (tool, files) in by_tool {
            if let Some(reason) = self.unavailable_reason(tool) {
                self.toasts.push(ToastKind::Error, reason);
                continue;
            }
            let selection = Selection {
                tool,
                files: &files,
                url: "",
                output_dir: self.settings.output_dir.as_deref(),
                options: &options,
            };
            match plan_jobs(&selection, &mut claimed) {
                Ok(specs) => {
                    queued += specs.len();
                    for spec in specs {
                        self.queue.push_titled(spec, None);
                    }
                }
                Err(e) => self.toasts.push(ToastKind::Error, e.to_string()),
            }
        }
        if queued > 0 {
            self.queue.is_running = true;
            self.toasts.push(
                ToastKind::Success,
                trf("notice.queued", &[("count", &queued)]),
            );
        }
        if skipped > 0 {
            self.toasts.push(
                ToastKind::Error,
                trf("notice.skipped", &[("count", &skipped)]),
            );
        }
    }

    /// Adds the right-click menu on the first start, and keeps it pointing at
    /// this FMX in its language. In the background: `reg.exe` takes a moment.
    /// On Windows the installer asks instead (or Settings, for the zip).
    fn setup_shell_menu(&mut self) {
        let first_time = !self.settings.shell_menu_setup && !cfg!(windows);
        self.settings.shell_menu_setup = true;
        if first_time {
            self.shell_menu = true;
        }
        std::thread::spawn(move || {
            if first_time {
                if let Err(e) = shell::register() {
                    tracing::warn!(error = %e, "could not add the right-click menu");
                }
            } else {
                shell::refresh();
            }
        });
    }

    fn select_tool(&mut self, tool: Tool) {
        if tool.input_kind() != self.tool.input_kind() {
            self.files.clear();
        }
        if tool != self.tool {
            self.preview = None;
            self.preview_index = 0;
        }
        self.tool = tool;
        self.settings.last_tool = tool;
        self.category = tool.category();
        self.in_editor = true;
        self.focus_url = tool.input_kind() == InputKind::Url;
    }

    fn open_category(&mut self, category: Category) {
        self.category = category;
        self.in_editor = false;
        self.search.clear();
    }

    /// Why a tool can't run here, if it can't.
    fn unavailable_reason(&self, tool: Tool) -> Option<String> {
        if let Some(missing) = self.toolbox.missing(tool.requires()) {
            return Some(trf("availability.missing", &[("program", &missing.name())]));
        }
        let encoder = tool.required_encoder()?;
        (!self.toolbox.has_encoder(encoder))
            .then(|| trf("availability.no_encoder", &[("encoder", &encoder)]))
    }

    fn accepts(&self, path: &Path) -> bool {
        let kind = self.tool.input_kind();
        if kind.picks_folders() && path.is_dir() {
            return true;
        }
        let exts = kind.extensions();
        path.is_file()
            && (exts.is_empty()
                || path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str())))
    }

    fn add_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let mut known: HashSet<PathBuf> = self.files.iter().cloned().collect();
        let mut skipped = 0usize;
        let mut added = Vec::new();
        for path in paths {
            if !self.accepts(&path) {
                skipped += 1;
            } else if known.insert(path.clone()) {
                added.push(path.clone());
                self.files.push(path);
            }
        }
        self.probe_details(added);
        if skipped > 0 {
            self.toasts.push(
                ToastKind::Error,
                trf("notice.skipped", &[("count", &skipped)]),
            );
        }
    }

    /// Records the size of new files and finds media lengths in the background.
    fn probe_details(&mut self, added: Vec<PathBuf>) {
        for path in &added {
            let bytes = std::fs::metadata(path)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len());
            self.details.entry(path.clone()).or_insert(FileDetails {
                bytes,
                duration: None,
            });
        }
        let Some(ffmpeg) = self.toolbox.get(External::Ffmpeg) else {
            return;
        };
        if self.tool.input_kind() != InputKind::Media || added.is_empty() {
            return;
        }
        let ffmpeg = ffmpeg.to_path_buf();
        let (tx, notify) = (self.probe_tx.clone(), Arc::clone(&self.notifier));
        std::thread::spawn(move || {
            for path in added.into_iter().take(MAX_PROBED_FILES) {
                let duration = ffmpeg::probe(&ffmpeg, &path, None)
                    .ok()
                    .and_then(|i| i.duration_s);
                if tx.send((path, duration)).is_err() {
                    return;
                }
                notify();
            }
        });
    }

    fn poll_details(&mut self) {
        while let Ok((path, duration)) = self.probe_rx.try_recv() {
            if let Some(d) = self.details.get_mut(&path) {
                d.duration = duration;
            }
        }
    }

    fn pick_files(&mut self) {
        let kind = self.tool.input_kind();
        let dialog = rfd::FileDialog::new();
        let picked = if kind.picks_folders() {
            dialog.pick_folders()
        } else {
            let exts = kind.extensions();
            let dialog = if exts.is_empty() {
                dialog
            } else {
                dialog.add_filter(tr("filter.supported"), exts)
            };
            dialog.add_filter(tr("filter.all"), &["*"]).pick_files()
        };
        if let Some(paths) = picked {
            self.add_files(paths);
        }
    }

    /// Adds every matching file under a folder, recursively.
    fn add_folder(&mut self) {
        let Some(root) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let mut found = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                // file_type() does not follow symlinks, so link loops are skipped.
                match entry.file_type() {
                    Ok(t) if t.is_dir() => stack.push(path),
                    Ok(t) if t.is_file() && self.accepts(&path) => found.push(path),
                    _ => {}
                }
            }
            if found.len() >= MAX_SCAN_FILES {
                break;
            }
        }
        found.sort();
        self.add_files(found);
    }

    fn enqueue(&mut self) {
        let mut claimed: HashSet<PathBuf> = self.queue.claimed_outputs().cloned().collect();
        let device = [PathBuf::from(self.settings.cd_device.trim())];
        let files: &[PathBuf] = if self.tool.input_kind() == InputKind::Device {
            &device
        } else {
            &self.files
        };
        let selection = Selection {
            tool: self.tool,
            files,
            url: &self.url,
            output_dir: self.settings.output_dir.as_deref(),
            options: &self.settings.options,
        };
        match plan_jobs(&selection, &mut claimed) {
            Ok(specs) => {
                let count = specs.len();
                // Downloads are listed under the video's title once it is known.
                let title = match self.link.as_ref().map(|l| (&l.url, &l.state)) {
                    Some((url, LinkState::Ready(info)))
                        if self.tool == Tool::Download && *url == self.url.trim() =>
                    {
                        Some(info.title.clone()).filter(|t| !t.is_empty())
                    }
                    _ => None,
                };
                specs
                    .into_iter()
                    .for_each(|s| self.queue.push_titled(s, title.clone()));
                self.files.clear();
                self.url.clear();
                self.settings.options.edit = Edit::default();
                // Back to the tools, where the queue shows the new jobs.
                self.in_editor = false;
                // "Start recording" and "Download" mean now; everything else waits for Start.
                if self.settings.start_on_add
                    || matches!(self.tool, Tool::Record | Tool::Screenshot | Tool::Download)
                {
                    self.queue.is_running = true;
                }
                let key = if self.queue.is_running {
                    "notice.queued"
                } else {
                    "notice.queued_press_start"
                };
                self.toasts
                    .push(ToastKind::Success, trf(key, &[("count", &count)]));
            }
            Err(e) => self.toasts.push(ToastKind::Error, e.to_string()),
        }
    }

    fn can_enqueue(&self) -> bool {
        self.blocker().is_none()
    }

    /// What is still missing before the tool can run, in plain words.
    fn blocker(&self) -> Option<String> {
        self.unavailable_reason(self.tool)
            .or_else(|| self.input_missing())
            .or_else(|| self.options_error())
    }

    fn options_error(&self) -> Option<String> {
        self.settings
            .options
            .validate(self.tool)
            .err()
            .map(|e| e.to_string())
    }

    /// What the first step still needs, if anything.
    fn input_missing(&self) -> Option<String> {
        let kind = self.tool.input_kind();
        let count = self.files.len();
        match kind {
            InputKind::Url => {
                let trimmed = self.url.trim();
                if trimmed.is_empty() {
                    Some(tr("need.url").to_owned())
                } else if !docs::is_web_url(trimmed) {
                    Some(tr("error.bad_url").to_owned())
                } else {
                    None
                }
            }
            InputKind::Device => self
                .settings
                .cd_device
                .trim()
                .is_empty()
                .then(|| tr("need.device").to_owned()),
            InputKind::Capture => None,
            _ => {
                if let Some(missing) = self.files.iter().find(|p| !p.exists()) {
                    return Some(trf(
                        "error.io",
                        &[("error", &format!("file not found: {}", missing.display()))],
                    ));
                }
                match self.tool.arity() {
                    InputArity::Exactly(n) if count != n => {
                        Some(trf("need.exactly", &[("count", &n), ("got", &count)]))
                    }
                    InputArity::AllAtLeast(n) if n > 1 && count < n => {
                        Some(trf("need.at_least", &[("count", &n)]))
                    }
                    _ if count == 0 && kind.picks_folders() => Some(tr("need.folder").to_owned()),
                    _ if count == 0 => Some(tr("need.file").to_owned()),
                    _ => None,
                }
            }
        }
    }

    /// How many jobs the main action will add.
    fn job_count(&self) -> usize {
        match (self.tool.input_kind(), self.tool.arity()) {
            (InputKind::Url | InputKind::Device | InputKind::Capture, _) => 1,
            (_, InputArity::EachFile) => self.files.len(),
            _ => 1,
        }
    }

    /// One sentence telling the user what pressing the main button will do.
    fn ready_summary(&self) -> String {
        let place = match (self.settings.output_dir.as_deref(), self.tool) {
            (Some(dir), _) => trf("ready.in_folder", &[("folder", &file_name(dir))]),
            // Downloads have no source file; they go to the Downloads folder.
            (None, Tool::Download) => tr("ready.in_downloads").to_owned(),
            (None, _) => tr("ready.beside_source").to_owned(),
        };
        match self.job_count() {
            1 => trf("ready.summary_one", &[("where", &place)]),
            count => trf("ready.summary", &[("count", &count), ("where", &place)]),
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.add_files(dropped);
            if !self.files.is_empty() {
                self.in_editor = true;
            }
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ADD_FILES)) {
            self.pick_files();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ENQUEUE)) && self.can_enqueue() {
            self.enqueue();
        }
        // Escape leaves a tool, unless it is closing something else first.
        if self.in_editor
            && !self.show_settings
            && self.preview.is_none()
            && self.media_info.is_none()
            && !egui::Popup::is_any_open(ctx)
            && ctx.memory(|m| m.focused().is_none())
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.in_editor = false;
        }
        // In the downloader, Ctrl+V works even when the address box isn't selected.
        if self.in_editor
            && self.tool.input_kind() == InputKind::Url
            && ctx.memory(|m| m.focused().is_none())
            && let Some(text) = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Paste(text) => Some(text.clone()),
                    _ => None,
                })
            })
        {
            self.url = text.trim().to_owned();
        }
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        theme::apply(ctx, self.settings.theme, self.settings.reduce_motion);
    }

    /// Tells the window frame (the title bar and its buttons, where the
    /// system draws them) which theme the app is showing, so they match.
    fn sync_window_theme(&mut self, ctx: &egui::Context) {
        // "System" follows the desktop as it changes (see theme.rs).
        let preference = self.settings.theme.preference();
        if ctx.options(|o| o.theme_preference) != preference {
            ctx.set_theme(preference);
        }
        let theme = ctx.theme();
        if self.window_theme != Some(theme) {
            self.window_theme = Some(theme);
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match theme {
                egui::Theme::Dark => egui::SystemTheme::Dark,
                egui::Theme::Light => egui::SystemTheme::Light,
            }));
        }
    }

    // ── Task lists ──────────────────────────────────────────────────────

    fn save_task_list(&mut self) {
        let specs: Vec<&JobSpec> = self
            .queue
            .jobs()
            .iter()
            .filter(|j| !matches!(j.state, JobState::Done { .. }))
            .map(|j| &j.spec)
            .collect();
        let Some(path) = rfd::FileDialog::new()
            .add_filter(tr("filter.task_list"), &["json"])
            .set_file_name("fmx-tasks.json")
            .save_file()
        else {
            return;
        };
        let result = serde_json::to_vec_pretty(&specs)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&path, bytes));
        match result {
            Ok(()) => self.toasts.push(
                ToastKind::Success,
                trf("notice.list_saved", &[("count", &specs.len())]),
            ),
            Err(e) => self
                .toasts
                .push(ToastKind::Error, trf("error.io", &[("error", &e)])),
        }
    }

    fn load_task_list(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(tr("filter.task_list"), &["json"])
            .pick_file()
        else {
            return;
        };
        let read = std::fs::metadata(&path).and_then(|m| {
            if m.len() > MAX_TASK_LIST_BYTES {
                Err(std::io::Error::other(tr("error.list_too_big")))
            } else {
                std::fs::read(&path)
            }
        });
        let parsed = read
            .map_err(|e| trf("error.io", &[("error", &e)]))
            .and_then(|bytes| {
                serde_json::from_slice::<Vec<JobSpec>>(&bytes)
                    .map_err(|_| tr("error.bad_list").to_owned())
            });
        match parsed {
            Ok(specs) => {
                for spec in &specs {
                    if let Err(e) = spec.validate() {
                        self.toasts.push(ToastKind::Error, e.to_string());
                        return;
                    }
                }
                let mut claimed: HashSet<PathBuf> = self.queue.claimed_outputs().cloned().collect();
                let count = specs.len();
                for mut spec in specs {
                    spec.output = fresh_output(&spec, &mut claimed);
                    self.queue.push(spec);
                }
                self.toasts.push(
                    ToastKind::Success,
                    trf("notice.queued", &[("count", &count)]),
                );
            }
            Err(message) => self.toasts.push(ToastKind::Error, message),
        }
    }

    // ── After-done actions ──────────────────────────────────────────────

    fn check_after_done(&mut self, ctx: &egui::Context) {
        let counts = self.queue.counts();
        let busy = counts.running + counts.queued > 0 && self.queue.is_running;
        let just_finished = self.was_busy && !busy && self.queue.is_idle();
        if busy && !self.was_busy {
            self.done_before_run = counts.done;
            self.run_stopped = false;
        }
        self.was_busy = busy;
        // Only a run that finished something, and wasn't stopped, ends with the
        // notice, the sound and the "when done" action.
        let finished_something = counts.done > self.done_before_run;
        if just_finished && finished_something && !self.run_stopped {
            self.toasts.push(ToastKind::Success, tr("notice.all_done"));
            if self.settings.notify_when_done {
                desktop_notification(tr("app.name"), tr("notice.all_done"));
            }
            sound::play(
                self.settings.completion_sound.clone(),
                self.toolbox.get(External::Ffmpeg).map(Path::to_path_buf),
            );
            match self.after_done {
                AfterDone::Nothing => {}
                AfterDone::OpenFolder => {
                    if let Some(job) = self
                        .queue
                        .jobs()
                        .iter()
                        .rev()
                        .find(|j| matches!(j.state, JobState::Done { .. }))
                    {
                        reveal(&job.spec.output);
                    }
                }
                AfterDone::Shutdown => self.shutdown_at = Some(Instant::now() + SHUTDOWN_DELAY),
            }
        }
        let Some(at) = self.shutdown_at else { return };
        let left = at.saturating_duration_since(Instant::now());
        if left.is_zero() {
            self.shutdown_at = None;
            self.after_done = AfterDone::Nothing;
            power_off();
            return;
        }
        let mut cancel = false;
        egui::Window::new(tr("shutdown.title"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                dir::vertical(ui, |ui| {
                    ui.label(trf("shutdown.countdown", &[("secs", &left.as_secs())]));
                    ui.add_space(8.0);
                    cancel = widgets::button(
                        ui,
                        Some(icons::X),
                        tr("action.cancel"),
                        Kind::Primary,
                        Size::Regular,
                    )
                    .clicked();
                });
            });
        if cancel {
            self.shutdown_at = None;
            self.after_done = AfterDone::Nothing;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    // ── Toolbar ─────────────────────────────────────────────────────────

    /// Just the app's name and Settings: queue controls live on the queue,
    /// and the output folder sits beside the button that adds jobs.
    /// Start or Pause, then the buttons for what is in the queue right now:
    /// a button only shows when it has something to act on.
    fn queue_controls(&mut self, ui: &mut egui::Ui) {
        use art::hue;
        let counts = self.queue.counts();
        if self.queue.is_running {
            if widgets::button_tinted(
                ui,
                Some(icons::PAUSE),
                Some(hue::AMBER),
                tr("action.pause"),
                Kind::Secondary,
                Size::Small,
            )
            .on_hover_text(tr("hint.pause"))
            .clicked()
            {
                self.queue.is_running = false;
            }
        } else if counts.queued > 0
            && widgets::button(
                ui,
                Some(icons::PLAY),
                tr("action.start"),
                Kind::Primary,
                Size::Small,
            )
            .on_hover_text(tr("hint.start"))
            .clicked()
        {
            self.queue.is_running = true;
        }
        if counts.running + counts.queued > 0
            && widgets::button(
                ui,
                Some(icons::STOP),
                tr("action.stop_all"),
                Kind::Danger,
                Size::Small,
            )
            .on_hover_text(tr("hint.stop_all"))
            .clicked()
        {
            self.queue.cancel_all();
            self.run_stopped = true;
        }
        if counts.failed > 0
            && widgets::button(
                ui,
                Some(icons::ARROW_CLOCKWISE),
                tr("action.retry"),
                Kind::Ghost,
                Size::Small,
            )
            .clicked()
        {
            self.queue.retry_failed();
        }
        if counts.done + counts.failed > 0
            && widgets::button(
                ui,
                Some(icons::BROOM),
                tr("action.clear_finished"),
                Kind::Ghost,
                Size::Small,
            )
            .clicked()
        {
            self.queue.clear_finished();
        }
    }

    /// The queue header's end: what to do when it finishes, and a menu for
    /// saving and loading the list.
    fn queue_menu(&mut self, ui: &mut egui::Ui) {
        // Pinned to the end, the row runs backwards: add the items in reverse.
        // Otherwise they go together on the next line.
        let pinned = ui.layout().main_dir() == dir::trailing(Align::Center).main_dir();
        if !pinned {
            ui.end_row();
        }
        self.queue_menu_items(ui, pinned);
    }

    fn queue_menu_items(&mut self, ui: &mut egui::Ui, pinned: bool) {
        let has_jobs = !self.queue.jobs().is_empty();
        if pinned {
            self.more_menu(ui, has_jobs);
        }
        if has_jobs {
            if !pinned {
                ui.label(RichText::new(tr("after.label")).weak());
            }
            combo(
                ui,
                "after_done",
                &mut self.after_done,
                &AfterDone::ALL,
                |a| a.label().to_owned(),
            );
            if pinned {
                ui.label(RichText::new(tr("after.label")).weak());
            }
        }
        if !pinned {
            self.more_menu(ui, has_jobs);
        }
    }

    /// The ⋮ button with Save list (when there is one) and Load list.
    fn more_menu(&mut self, ui: &mut egui::Ui, can_save: bool) {
        let more = widgets::button_tinted(
            ui,
            Some(icons::DOTS_THREE_VERTICAL),
            Some(art::hue::SLATE),
            tr("action.more"),
            Kind::Ghost,
            Size::Regular,
        );
        let mut save = false;
        let mut load = false;
        egui::Popup::menu(&more).show(|ui| {
            dir::menu(ui, |ui| {
                save = can_save
                    && ui
                        .button(icons::label(icons::FLOPPY_DISK, tr("action.save_list")))
                        .clicked();
                load = ui
                    .button(icons::label(icons::FOLDER_OPEN, tr("action.load_list")))
                    .clicked();
            });
        });
        if save {
            self.save_task_list();
        }
        if load {
            self.load_task_list();
        }
    }

    /// The Settings button in the sidebar's header, like the menu button in
    /// GNOME Settings, with a dot when an update is waiting there.
    fn settings_button(&mut self, ui: &mut egui::Ui) {
        let news = self.update_news();
        let settings = widgets::button(ui, Some(icons::GEAR), "", Kind::Ghost, Size::Regular);
        if news {
            // A dot on the button's top end corner: an update is waiting in Settings.
            let r = settings.rect;
            let x = dir::mirror_x(r, r.max.x - 9.0, 0.0);
            let p = theme::palette(ui);
            ui.painter()
                .circle_filled(egui::pos2(x, r.min.y + 7.0), 4.0, p.accent);
        }
        if settings
            .on_hover_text_at_pointer(if news {
                tr("update.available_hint")
            } else {
                tr("action.settings")
            })
            .clicked()
        {
            self.show_settings = !self.show_settings;
            if news {
                self.settings_tab = SettingsTab::Updates;
            }
        }
    }

    /// "Change…" (and "Reset" once changed) beside the sentence that says
    /// where the files will be saved.
    /// How wide [`Self::output_dir_picker`] draws its buttons.
    fn output_dir_picker_width(&self, ui: &egui::Ui) -> f32 {
        let change =
            widgets::button_width(ui, Some(icons::FOLDER), tr("action.change"), Size::Small);
        match self.settings.output_dir {
            Some(_) => {
                change
                    + ui.spacing().item_spacing.x
                    + widgets::button_width(ui, Some(icons::X), tr("action.reset"), Size::Small)
            }
            None => change,
        }
    }

    fn output_dir_picker(&mut self, ui: &mut egui::Ui) {
        if widgets::button(
            ui,
            Some(icons::FOLDER),
            tr("action.change"),
            Kind::Ghost,
            Size::Small,
        )
        .on_hover_text(tr("hint.output_change"))
        .clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            self.settings.output_dir = Some(dir);
        }
        if self.settings.output_dir.is_some()
            && widgets::button(
                ui,
                Some(icons::X),
                tr("action.reset"),
                Kind::Ghost,
                Size::Small,
            )
            .on_hover_text(tr("hint.output_reset"))
            .clicked()
        {
            self.settings.output_dir = None;
        }
    }

    // ── Sidebar ─────────────────────────────────────────────────────────

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(HEADER_HEIGHT);
            app_title(ui);
            ui.with_layout(dir::trailing(Align::Center), |ui| {
                self.settings_button(ui);
            });
        });
        ui.add_space(10.0);
        // Searching shows the matching tools from every category, and leaves
        // an open tool to show them.
        let search = ui.add(
            TextEdit::singleline(&mut self.search)
                .hint_text(icons::label(icons::MAGNIFYING_GLASS, tr("grid.search")))
                .align(egui::Align2([dir::start(), Align::Center]))
                .margin(Margin::symmetric(10, 7))
                .desired_width(f32::INFINITY),
        );
        if search.changed() && !self.search.trim().is_empty() {
            self.in_editor = false;
        }
        ui.add_space(10.0);
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                widgets::nav_list(ui, "categories", |ui| {
                    let mut chosen = None;
                    for (i, category) in Category::ALL.into_iter().enumerate() {
                        let selected = self.category == category && self.search.is_empty();
                        let item = widgets::nav_item(
                            ui,
                            Some(art::category(category)),
                            category.label(),
                            selected,
                            false,
                            None,
                        );
                        if selected {
                            chosen = Some((i, item.rect));
                        }
                        if item.clicked() {
                            self.open_category(category);
                        }
                    }
                    chosen
                });
            });
    }

    // ── Tool grid ───────────────────────────────────────────────────────

    fn tool_grid(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let searching = !self.search.trim().is_empty();
        ui.horizontal(|ui| {
            if searching {
                ui.label(RichText::new(tr("grid.results")).heading().strong());
            } else {
                widgets::icon_tile(ui, art::category(self.category), 40.0, true);
                ui.add_space(4.0);
                ui.label(RichText::new(self.category.label()).heading().strong());
            }
        });
        ui.add_space(12.0);
        if self.toolbox.get(External::Ffmpeg).is_none() {
            widgets::chip(
                ui,
                icons::WARNING,
                &trf(
                    "availability.missing",
                    &[("program", &External::Ffmpeg.name())],
                ),
                p.warning,
            );
            ui.add_space(12.0);
        }
        let needle = self.search.trim().to_lowercase();
        let tools: Vec<Tool> = if searching {
            Category::ALL
                .into_iter()
                .flat_map(Category::tools)
                .filter(|t| {
                    t.label().to_lowercase().contains(&needle)
                        || t.description().to_lowercase().contains(&needle)
                })
                .collect()
        } else {
            self.category.tools()
        };
        if tools.is_empty() {
            widgets::empty_state(ui, icons::MAGNIFYING_GLASS, tr("grid.no_results"), 160.0);
            return;
        }
        let mut picked = None;
        // As many columns as fit, the cards stretched to fill the row exactly.
        let gap = 12.0;
        let available = ui.available_width();
        let columns = ((available + gap) / (widgets::CARD_SIZE.x + gap))
            .floor()
            .max(1.0);
        let card_width = ((available - gap * (columns - 1.0)) / columns)
            .min(widgets::CARD_MAX_WIDTH)
            .floor();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
            for tool in tools {
                let reason = self.unavailable_reason(tool);
                let subtitle = if searching {
                    tool.category().label().to_owned()
                } else {
                    card_detail(tool)
                };
                let hover = match &reason {
                    Some(r) => format!("{}\n\n{} {r}", tool.description(), icons::WARNING),
                    None => tool.description().to_owned(),
                };
                if widgets::tool_card(
                    ui,
                    card_width,
                    art::tool(tool),
                    tool.label(),
                    &subtitle,
                    reason.is_some(),
                )
                .on_hover_text(hover)
                .clicked()
                {
                    picked = Some(tool);
                }
            }
        });
        if let Some(tool) = picked {
            self.select_tool(tool);
        }
        ui.add_space(8.0);
        ui.label(RichText::new(tr("grid.hint")).color(p.text_weak));
    }

    // ── Task editor ─────────────────────────────────────────────────────

    /// The tool window: files on the left, settings (or the video) on the right,
    /// sized to the window so nothing scrolls except long lists.
    fn task_editor(&mut self, ui: &mut egui::Ui) {
        self.editor_header(ui);
        ui.add_space(12.0);
        let (title, help) = self.input_heading();
        let input_done = self.input_missing().is_none();
        let size = ui.available_size();
        match self.settings_heading() {
            None => step_panel(ui, size, 1, input_done, title, &help, |ui| {
                self.input_ui(ui, true);
            }),
            Some((settings_title, settings_help)) if size.x >= TWO_COLUMN_MIN_WIDTH => {
                let preview = self.preview_mode();
                let share = if preview.is_some() {
                    FILES_SHARE_BESIDE_PREVIEW
                } else {
                    FILES_SHARE
                };
                let left = ((size.x - PANEL_GAP) * share).floor();
                let right = size.x - PANEL_GAP - left;
                ui.horizontal_top(|ui| {
                    let files = egui::vec2(left, size.y);
                    step_panel(ui, files, 1, input_done, title, &help, |ui| {
                        self.input_ui(ui, true);
                    });
                    ui.add_space(PANEL_GAP - ui.spacing().item_spacing.x);
                    let settings = egui::vec2(right, size.y);
                    step_panel(
                        ui,
                        settings,
                        2,
                        false,
                        settings_title,
                        settings_help,
                        |ui| {
                            let reserve = match preview {
                                Some(mode) if mode.clip => PREVIEW_RESERVE_CLIP,
                                _ => PREVIEW_RESERVE_AREA,
                            };
                            let preview_height =
                                (ui.available_height() - reserve).max(MIN_PREVIEW_HEIGHT);
                            theme::page_scroll()
                                .id_salt("settings_panel")
                                .auto_shrink([false; 2])
                                .show(ui, |ui| self.settings_step(ui, preview_height));
                        },
                    );
                });
            }
            Some((settings_title, settings_help)) => {
                theme::page_scroll()
                    .id_salt("editor_stacked")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        step(ui, 1, input_done, title, &help, |ui| {
                            self.input_ui(ui, false)
                        });
                        ui.add_space(12.0);
                        step(ui, 2, false, settings_title, settings_help, |ui| {
                            self.settings_step(ui, STACKED_PREVIEW_HEIGHT);
                        });
                    });
            }
        }
    }

    /// A GNOME-style page header: a back button at the start edge and the
    /// tool's name centered over the page, with its purpose below.
    fn editor_header(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let (row, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), theme::BUTTON_HEIGHT),
            egui::Sense::hover(),
        );
        ui.painter().text(
            row.center(),
            egui::Align2::CENTER_CENTER,
            self.tool_title(),
            egui::FontId::proportional(17.0),
            p.text,
        );
        // GNOME's thin back chevron at the start edge, pointing that way.
        let back = if dir::is_rtl() {
            icons::CARET_RIGHT
        } else {
            icons::CARET_LEFT
        };
        let mut start = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(dir::leading(Align::Center)),
        );
        if widgets::button(&mut start, Some(back), "", Kind::Ghost, Size::Regular)
            .on_hover_text(trf(
                "hint.back_to",
                &[("category", &self.tool.category().label())],
            ))
            .clicked()
        {
            self.in_editor = false;
        }
        ui.vertical_centered(|ui| {
            ui.add(
                egui::Label::new(RichText::new(self.tool.description()).color(p.text_weak)).wrap(),
            );
        });
        if let Some(reason) = self.unavailable_reason(self.tool) {
            ui.add_space(4.0);
            ui.vertical_centered(|ui| widgets::chip(ui, icons::WARNING, &reason, p.warning));
        }
    }

    /// The first step's controls. With `fill`, the file list or drop zone takes
    /// all the height that is left.
    fn input_ui(&mut self, ui: &mut egui::Ui, fill: bool) {
        let p = theme::palette(ui);
        match self.tool.input_kind() {
            InputKind::Url => self.url_input(ui),
            InputKind::Device => {
                ui.horizontal(|ui| {
                    ui.label(tr("input.device"));
                    ui.add(TextEdit::singleline(&mut self.settings.cd_device).desired_width(220.0));
                });
            }
            InputKind::Capture => {
                let screenshot = self.tool == Tool::Screenshot;
                ui.label(tr(if screenshot {
                    "shot.input"
                } else {
                    "input.capture"
                }));
                let note = if capture::system_picks_area() {
                    Some(tr(if screenshot {
                        "shot.system_picks"
                    } else {
                        "record.system_picks"
                    }))
                } else if cfg!(target_os = "macos") {
                    Some(tr("record.mac_permission"))
                } else {
                    None
                };
                if let Some(note) = note {
                    ui.add_space(4.0);
                    ui.colored_label(p.text_weak, note);
                }
            }
            _ => self.file_list(ui, fill),
        }
    }

    /// The first step's title and a sentence on what to add.
    fn input_heading(&self) -> (&'static str, String) {
        let kind = self.tool.input_kind();
        match kind {
            InputKind::Url => (tr("step.url"), tr("hint.download_rights").to_owned()),
            InputKind::Device => (tr("step.device"), tr("help.device").to_owned()),
            InputKind::Capture if self.tool == Tool::Screenshot => (tr("shot.step"), String::new()),
            InputKind::Capture => (tr("step.capture"), String::new()),
            _ => {
                let title = if kind.picks_folders() {
                    tr("step.folders")
                } else {
                    tr("step.files")
                };
                let help = match self.tool.arity() {
                    InputArity::Exactly(_) if self.tool == Tool::Mux => {
                        tr("hint.mux_order").to_owned()
                    }
                    InputArity::Exactly(_) => tr("hint.mix_order").to_owned(),
                    InputArity::AllAtLeast(n) if n > 1 => trf("help.combine", &[("count", &n)]),
                    InputArity::AllAtLeast(_) => tr("help.in_order").to_owned(),
                    _ if kind.picks_folders() => tr("help.each_folder").to_owned(),
                    _ => tr("help.each_file").to_owned(),
                };
                (title, help)
            }
        }
    }

    /// The second step's title and help, or `None` when the tool has nothing to set.
    fn settings_heading(&self) -> Option<(&'static str, &'static str)> {
        match self.tool {
            Tool::Trim => Some((tr("step.trim"), tr("help.trim"))),
            Tool::Crop => Some((tr("step.crop"), tr("help.crop"))),
            Tool::Delogo => Some((tr("step.delogo"), tr("help.delogo"))),
            Tool::Rotate => Some((tr("step.rotate"), tr("help.rotate"))),
            tool if has_options(tool) => Some((tr("step.settings"), tr("help.settings"))),
            _ => None,
        }
    }

    fn settings_step(&mut self, ui: &mut egui::Ui, preview_height: f32) {
        if let Some(mode) = self.preview_mode() {
            self.inline_preview(ui, mode, preview_height);
            ui.add_space(12.0);
        }
        if self.tool == Tool::Trim {
            trim_settings(ui, &mut self.settings.options);
        } else {
            pref_group(ui, None, |ui| self.options_ui(ui));
        }
        if let Tool::ToVideo(format) = self.tool {
            ui.add_space(4.0);
            self.advanced_video(ui, format);
            self.edit_panel(ui, true);
        } else if let Tool::ToDevice(_) = self.tool {
            ui.add_space(4.0);
            self.edit_panel(ui, true);
        } else if let Tool::ToAudio(format) = self.tool {
            ui.add_space(4.0);
            self.advanced_audio(ui, !format.is_lossless());
            self.edit_panel(ui, false);
        }
    }

    /// How the inline preview works for the current tool, if it has one.
    fn preview_mode(&self) -> Option<PreviewMode> {
        match self.tool {
            Tool::ExtractFrames if self.settings.options.frame_mode == FrameMode::Times => {
                Some(PreviewMode::MARKS)
            }
            Tool::ExtractFrames => Some(PreviewMode::CLIP),
            tool => inline_preview_mode(tool),
        }
    }

    /// The video itself, for tools whose main job is picking a part of it.
    fn inline_preview(&mut self, ui: &mut egui::Ui, mode: PreviewMode, max_height: f32) {
        let p = theme::palette(ui);
        let Some(ffmpeg) = self.toolbox.get(External::Ffmpeg) else {
            return;
        };
        if self.files.is_empty() {
            egui::Frame::new()
                .fill(p.sunken)
                .corner_radius(CornerRadius::same(10))
                .show(ui, |ui| {
                    widgets::empty_state(
                        ui,
                        icons::EYE,
                        tr("preview.add_file_first"),
                        max_height.min(240.0),
                    );
                });
            return;
        }
        self.preview_index = self.preview_index.min(self.files.len() - 1);
        if self.files.len() > 1 {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("preview.showing")).color(p.text_weak));
                let choices: Vec<usize> = (0..self.files.len()).collect();
                let files = &self.files;
                combo(ui, "preview_file", &mut self.preview_index, &choices, |i| {
                    file_name(&files[i])
                });
                ui.label(RichText::new(icons::INFO).color(p.text_weak))
                    .on_hover_text(tr("preview.same_for_all"));
            });
            ui.add_space(4.0);
        }
        let path = &self.files[self.preview_index];
        if self.preview.as_ref().is_none_or(|v| v.path() != path) {
            self.preview = Some(Preview::open(path.clone(), ffmpeg.to_path_buf(), ui.ctx()));
        }
        let Some(preview) = self.preview.as_mut() else {
            return;
        };
        let opts = &mut self.settings.options;
        if self.tool == Tool::Delogo {
            let mut area = Some(opts.delogo);
            preview.inline(
                ui,
                &mut opts.edit,
                &mut area,
                &mut opts.frame_times,
                mode,
                max_height,
                None,
            );
            if let Some(area) = area {
                opts.delogo = area;
            }
        } else if self.tool == Tool::Rotate {
            let mut crop = opts.edit.crop.take();
            preview.inline(
                ui,
                &mut opts.edit,
                &mut crop,
                &mut opts.frame_times,
                mode,
                max_height,
                Some(&mut opts.orientation),
            );
            opts.edit.crop = crop;
        } else {
            let mut turned = opts.edit.rotation.map(options::Orientation::from);
            let mut crop = opts.edit.crop.take();
            preview.inline(
                ui,
                &mut opts.edit,
                &mut crop,
                &mut opts.frame_times,
                mode,
                max_height,
                turned.as_mut(),
            );
            opts.edit.crop = crop;
        }
    }

    /// The editor's sticky footer: what will happen, or what is still missing,
    /// next to the main action.
    fn action_bar(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let blocker = self.blocker();
        ui.horizontal(|ui| {
            ui.set_min_height(44.0);
            ui.with_layout(dir::trailing(Align::Center), |ui| {
                let (icon, label) = self.primary_action();
                if ui
                    .add_enabled_ui(blocker.is_none(), |ui| {
                        widgets::button(ui, Some(icon), label, Kind::Primary, Size::Large)
                    })
                    .inner
                    .on_hover_text(ui.ctx().format_shortcut(&ENQUEUE))
                    .clicked()
                {
                    self.enqueue();
                }
                ui.add_space(10.0);
                ui.with_layout(dir::leading(Align::Center), |ui| {
                    let (glyph, color, text, text_color) = match &blocker {
                        Some(reason) => (icons::INFO, p.text_weak, reason.clone(), p.text_weak),
                        None => (icons::CHECK_CIRCLE, p.success, self.ready_summary(), p.text),
                    };
                    ui.label(RichText::new(glyph).size(20.0).color(color));
                    // The sentence wraps in the room the folder buttons leave.
                    let picker = if blocker.is_none() {
                        self.output_dir_picker_width(ui) + ui.spacing().item_spacing.x
                    } else {
                        0.0
                    };
                    ui.scope(|ui| {
                        ui.set_max_width((ui.available_width() - picker).max(80.0));
                        ui.add(Label::new(RichText::new(text).color(text_color)).wrap());
                    });
                    if blocker.is_none() {
                        self.output_dir_picker(ui);
                    }
                });
            });
        });
    }

    fn primary_action(&self) -> (&'static str, &'static str) {
        match self.tool {
            Tool::Record => (icons::RECORD, tr("action.start_recording")),
            Tool::Screenshot => (icons::CAMERA, tr("action.take_screenshot")),
            Tool::Download => (icons::DOWNLOAD, tr("action.download")),
            _ => (icons::LIST_PLUS, tr("action.enqueue")),
        }
    }

    fn tool_title(&self) -> String {
        match self.tool {
            Tool::ToVideo(_) | Tool::ToAudio(_) | Tool::ToImage(_) => {
                trf("title.convert_to", &[("format", &self.tool.label())])
            }
            tool => tool.label().to_owned(),
        }
    }

    /// The web-address box with a Paste button beside it.
    fn url_input(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(dir::trailing(Align::Center), |ui| {
                let paste = widgets::button(
                    ui,
                    Some(icons::CLIPBOARD),
                    tr("action.paste"),
                    Kind::Secondary,
                    Size::Regular,
                )
                .on_hover_text(tr("hint.paste_url"))
                .clicked();
                let trimmed = self.url.trim();
                let invalid_url = !trimmed.is_empty() && !docs::is_web_url(trimmed);
                let mut edit = ui.add(
                    TextEdit::singleline(&mut self.url)
                        .hint_text("https://…")
                        .font(egui::FontId::proportional(16.0))
                        .margin(Margin::symmetric(10, 8))
                        .desired_width(f32::INFINITY),
                );
                if invalid_url {
                    let p = theme::palette(ui);
                    ui.painter().rect_stroke(
                        edit.rect,
                        CornerRadius::same(theme::RADIUS),
                        Stroke::new(1.5, p.danger),
                        StrokeKind::Inside,
                    );
                    edit = edit.on_hover_text(tr("error.bad_url"));
                }
                if edit.changed() {
                    self.url_edited = Some(Instant::now());
                }
                if paste {
                    // Replace whatever was there with the clipboard's address.
                    self.url.clear();
                    edit.request_focus();
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                } else if std::mem::take(&mut self.focus_url) {
                    edit.request_focus();
                }
            });
        });
        ui.add_space(12.0);
        self.link_card(ui);
    }

    // ── Link details (video downloader) ─────────────────────────────────

    /// Looks up a pasted link once typing pauses, and collects the answers.
    fn poll_link(&mut self, ctx: &egui::Context) {
        if !self.in_editor || self.tool != Tool::Download {
            return;
        }
        let url = self.url.trim().to_owned();
        let playlist = self.settings.options.download.playlist;
        let Some(ytdlp) = self.toolbox.get(External::YtDlp).map(Path::to_path_buf) else {
            self.link = None;
            return;
        };
        if !docs::is_web_url(&url) {
            self.link = None;
            return;
        }
        let current = self
            .link
            .as_ref()
            .is_some_and(|l| l.url == url && l.playlist == playlist);
        if !current {
            let waited = self.url_edited.map_or(LINK_LOOKUP_DELAY, |t| t.elapsed());
            if waited < LINK_LOOKUP_DELAY {
                ctx.request_repaint_after(LINK_LOOKUP_DELAY - waited);
                return;
            }
            let (tx, rx) = mpsc::channel();
            let (thread_url, thread_ctx) = (url.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(download::fetch_info(&ytdlp, &thread_url, playlist));
                thread_ctx.request_repaint();
            });
            self.link = Some(LinkPreview {
                url,
                playlist,
                state: LinkState::Loading(rx),
                thumbnail: None,
                thumbnail_rx: None,
            });
            return;
        }
        let Some(link) = &mut self.link else { return };
        if let LinkState::Loading(rx) = &link.state {
            match rx.try_recv() {
                Ok(Ok(info)) => {
                    if let Some(thumb) = info.thumbnail.clone() {
                        let (tx, rx) = mpsc::channel();
                        let thread_ctx = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(download::fetch_thumbnail(&thumb));
                            thread_ctx.request_repaint();
                        });
                        link.thumbnail_rx = Some(rx);
                    }
                    link.state = LinkState::Ready(info);
                }
                Ok(Err(message)) => link.state = LinkState::Failed(message),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    link.state = LinkState::Failed(tr("download.info_failed").to_owned());
                }
            }
        }
        if let Some(rx) = &link.thumbnail_rx
            && let Ok(result) = rx.try_recv()
        {
            link.thumbnail_rx = None;
            match result {
                Ok(image) => {
                    let size = [image.width() as usize, image.height() as usize];
                    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                    link.thumbnail = Some(ctx.load_texture(
                        "link_thumbnail",
                        pixels,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                Err(e) => tracing::debug!(error = %e, "thumbnail unavailable"),
            }
        }
    }

    /// The link's thumbnail, title, channel, length and best quality.
    fn link_card(&self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let Some(link) = &self.link else {
            if self.toolbox.get(External::YtDlp).is_some() {
                ui.label(RichText::new(tr("download.paste_hint")).color(p.text_weak));
            }
            return;
        };
        egui::Frame::new()
            .fill(p.sunken)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                match &link.state {
                    LinkState::Loading(_) => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label(RichText::new(tr("download.looking_up")).color(p.text_weak));
                        });
                    }
                    LinkState::Failed(message) => {
                        widgets::chip(ui, icons::WARNING, tr("download.lookup_failed"), p.warning);
                        ui.add(
                            Label::new(RichText::new(message).small().color(p.text_weak)).wrap(),
                        );
                    }
                    LinkState::Ready(info) => {
                        let thumb_width = (ui.available_width() * 0.42).min(THUMBNAIL_WIDTH);
                        let thumb = egui::vec2(thumb_width, thumb_width * 9.0 / 16.0);
                        ui.horizontal_top(|ui| {
                            let (rect, _) = ui.allocate_exact_size(thumb, egui::Sense::hover());
                            match &link.thumbnail {
                                Some(texture) => {
                                    ui.painter().image(
                                        texture.id(),
                                        rect,
                                        egui::Rect::from_min_max(
                                            egui::Pos2::ZERO,
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::WHITE,
                                    );
                                }
                                None => {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(6),
                                        p.control,
                                    );
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        icons::PLAY,
                                        egui::FontId::proportional(26.0),
                                        p.text_weak,
                                    );
                                }
                            }
                            dir::vertical(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 3.0;
                                ui.add(Label::new(RichText::new(&info.title).strong()).wrap());
                                let by: Vec<&str> = [info.channel.as_deref(), info.site.as_deref()]
                                    .into_iter()
                                    .flatten()
                                    .collect();
                                if !by.is_empty() {
                                    ui.label(
                                        RichText::new(by.join(" · ")).small().color(p.text_weak),
                                    );
                                }
                                ui.horizontal_wrapped(|ui| {
                                    if let Some(count) = info.playlist_count {
                                        widgets::chip(
                                            ui,
                                            icons::LIST_PLUS,
                                            &trf("download.playlist_count", &[("count", &count)]),
                                            p.accent,
                                        );
                                    }
                                    if let Some(secs) = info.duration {
                                        widgets::chip(
                                            ui,
                                            icons::PLAY,
                                            &download::clock(secs),
                                            p.text_weak,
                                        );
                                    }
                                    if let Some(h) = info.max_height {
                                        widgets::chip(
                                            ui,
                                            icons::EYE,
                                            &trf("download.up_to", &[("height", &h)]),
                                            p.success,
                                        );
                                    }
                                });
                            });
                        });
                    }
                }
            });
    }

    /// The add buttons, then the files, or a drop zone while there are none.
    /// With `fill`, the list or drop zone uses all the remaining height.
    fn file_list(&mut self, ui: &mut egui::Ui, fill: bool) {
        let kind = self.tool.input_kind();
        let p = theme::palette(ui);
        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        let roles = slot_roles(self.tool);
        self.add_buttons(ui);
        ui.add_space(8.0);
        let (drop_height, list_height) = if fill {
            let left = (ui.available_height() - 6.0).max(MIN_DROP_HEIGHT);
            (left, left)
        } else {
            (STACKED_DROP_HEIGHT, STACKED_LIST_HEIGHT)
        };
        if self.files.is_empty() && roles.is_none() {
            let (icon, title) = if kind.picks_folders() {
                (icons::FOLDER_PLUS, tr("drop.title_folders"))
            } else {
                (icons::FILE_PLUS, tr("drop.title"))
            };
            if widgets::drop_zone(
                ui,
                icon,
                title,
                tr("drop.subtitle"),
                &formats_line(kind),
                hovering,
                drop_height,
            )
            .on_hover_text(ui.ctx().format_shortcut(&ADD_FILES))
            .clicked()
            {
                self.pick_files();
            }
            return;
        }

        let glow = ui
            .ctx()
            .animate_bool_with_time(Id::unique("drop_glow"), hovering, MOTION);
        let probeable = kind == InputKind::Media && self.toolbox.get(External::Ffmpeg).is_some();
        let inline = self.preview_mode().is_some();
        let row = RowContext {
            count: self.files.len(),
            ordered: !matches!(self.tool.arity(), InputArity::EachFile),
            probeable,
            eye: probeable && (previewable(self.tool) || (inline && self.files.len() > 1)),
            selected: (inline && self.files.len() > 1).then_some(self.preview_index),
        };
        let mut action = None;
        egui::Frame::new()
            .stroke(Stroke::new(1.5, p.accent.gamma_multiply(glow)))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(2))
            .show(ui, |ui| {
                theme::page_scroll()
                    .id_salt("file_list")
                    .max_height(list_height)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        for (i, path) in self.files.iter().enumerate() {
                            let role = roles.and_then(|r| r.get(i).copied());
                            if let Some(a) = self.file_row(ui, i, path, role, &row) {
                                action = Some(a);
                            }
                        }
                        if let Some(roles) = roles {
                            for (i, role) in roles.iter().enumerate().skip(self.files.len()) {
                                if empty_slot(ui, i + 1, role) {
                                    action = Some(FileAction::Add);
                                }
                            }
                        }
                    });
            });
        match action {
            Some(FileAction::Remove(i)) => {
                self.files.remove(i);
            }
            Some(FileAction::Swap(a, b)) => self.files.swap(a, b),
            Some(FileAction::Info(i)) => self.show_media_info(i),
            Some(FileAction::Preview(i)) if inline => self.preview_index = i,
            Some(FileAction::Preview(i)) => self.open_preview(i, ui.ctx()),
            Some(FileAction::Add) => self.pick_files(),
            None => {}
        }
    }

    /// "Add more", "Add folder", "Clear list" and a short summary of the list.
    fn add_buttons(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let kind = self.tool.input_kind();
        let has_files = !self.files.is_empty();
        // In a narrow file panel the secondary buttons show only their icon.
        let compact = ui.available_width() < COMPACT_BUTTONS_WIDTH;
        let text = |label: &'static str| if compact { "" } else { label };
        ui.horizontal(|ui| {
            {
                let label = match (kind.picks_folders(), has_files) {
                    (true, false) => tr("action.add_folders"),
                    (true, true) => tr("action.add_more_folders"),
                    (false, false) => tr("action.add_files"),
                    (false, true) => tr("action.add_more"),
                };
                if widgets::button(ui, Some(icons::PLUS), label, Kind::Secondary, Size::Regular)
                    .on_hover_text(ui.ctx().format_shortcut(&ADD_FILES))
                    .clicked()
                {
                    self.pick_files();
                }
            }
            if kind == InputKind::Disc
                && widgets::button(
                    ui,
                    Some(icons::DISC),
                    tr("action.add_iso"),
                    Kind::Secondary,
                    Size::Regular,
                )
                .clicked()
                && let Some(paths) = rfd::FileDialog::new()
                    .add_filter("ISO", &["iso"])
                    .pick_files()
            {
                self.add_files(paths);
            }
            if !kind.picks_folders()
                && widgets::button(
                    ui,
                    Some(icons::FOLDER_PLUS),
                    text(tr("action.add_folder_scan")),
                    Kind::Ghost,
                    Size::Regular,
                )
                .on_hover_text(format!(
                    "{}\n{}",
                    tr("action.add_folder_scan"),
                    tr("hint.add_folder_scan")
                ))
                .clicked()
            {
                self.add_folder();
            }
            if has_files
                && widgets::button(
                    ui,
                    Some(icons::BROOM),
                    text(tr("action.clear")),
                    Kind::Ghost,
                    Size::Regular,
                )
                .on_hover_text(tr("action.clear"))
                .clicked()
            {
                self.files.clear();
            }
        });
        if has_files {
            ui.label(
                RichText::new(self.list_summary())
                    .small()
                    .color(p.text_weak),
            );
        }
    }

    /// "3 files · 12:30 in total" (the length only once every file is known).
    fn list_summary(&self) -> String {
        let count = trf("files.count", &[("count", &self.files.len())]);
        let total: Option<f64> = self
            .files
            .iter()
            .map(|f| self.details.get(f).and_then(|d| d.duration))
            .sum();
        match total {
            Some(secs) if self.files.len() > 1 => format!(
                "{count} · {}",
                trf(
                    "files.total_length",
                    &[("length", &clock(Duration::from_secs_f64(secs)))]
                )
            ),
            _ => count,
        }
    }

    /// One file in the list: its kind, name, size and length, and its actions.
    fn file_row(
        &self,
        ui: &mut egui::Ui,
        i: usize,
        path: &Path,
        role: Option<&str>,
        row: &RowContext,
    ) -> Option<FileAction> {
        let p = theme::palette(ui);
        let mut action = None;
        let fill = if row.selected == Some(i) {
            p.sunken.lerp_to_gamma(p.accent, 0.14)
        } else {
            p.sunken
        };
        egui::Frame::new()
            .fill(fill)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if row.ordered {
                        position_badge(ui, i + 1);
                    }
                    widgets::icon_tile(
                        ui,
                        file_art(path, self.tool.input_kind()),
                        FILE_TILE,
                        false,
                    );
                    ui.with_layout(dir::trailing(Align::Center), |ui| {
                        let mut small = |icon, hint: &str, enabled, then: FileAction| {
                            if ui
                                .add_enabled_ui(enabled, |ui| {
                                    widgets::button(ui, Some(icon), "", Kind::Ghost, Size::Small)
                                })
                                .inner
                                .on_hover_text(hint)
                                .on_disabled_hover_text(hint)
                                .clicked()
                            {
                                action = Some(then);
                            }
                        };
                        small(icons::X, tr("action.remove"), true, FileAction::Remove(i));
                        if row.ordered {
                            small(
                                icons::CARET_DOWN,
                                tr("action.move_down"),
                                i + 1 < row.count,
                                FileAction::Swap(i, i + 1),
                            );
                            small(
                                icons::CARET_UP,
                                tr("action.move_up"),
                                i > 0,
                                FileAction::Swap(i, i.saturating_sub(1)),
                            );
                        }
                        if row.probeable {
                            small(
                                icons::INFO,
                                tr("action.media_info"),
                                true,
                                FileAction::Info(i),
                            );
                        }
                        if row.eye {
                            let hint = if row.selected.is_some() {
                                tr("action.show_in_preview")
                            } else {
                                tr("action.preview")
                            };
                            small(icons::EYE, hint, true, FileAction::Preview(i));
                        }
                        ui.with_layout(dir::leading(Align::Center), |ui| {
                            dir::vertical(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                if let Some(role) = role {
                                    ui.label(RichText::new(role).small().strong().color(p.accent));
                                }
                                ui.add(
                                    Label::new(RichText::new(file_name(path)).strong()).truncate(),
                                )
                                .on_hover_text(path.display().to_string());
                                let detail = self.file_detail(path);
                                if !detail.is_empty() {
                                    ui.label(RichText::new(detail).small().color(p.text_weak));
                                }
                            });
                        });
                    });
                });
            });
        action
    }

    /// "12.4 MB · 3:24" for a file, as far as it is known.
    fn file_detail(&self, path: &Path) -> String {
        let Some(details) = self.details.get(path) else {
            return String::new();
        };
        let mut parts = Vec::new();
        if let Some(bytes) = details.bytes {
            parts.push(human_size(bytes));
        }
        if let Some(secs) = details.duration {
            parts.push(clock(Duration::from_secs_f64(secs)));
        }
        parts.join(" · ")
    }

    fn open_preview(&mut self, index: usize, ctx: &egui::Context) {
        if let (Some(ffmpeg), Some(path)) =
            (self.toolbox.get(External::Ffmpeg), self.files.get(index))
        {
            self.preview = Some(Preview::open(path.clone(), ffmpeg.to_path_buf(), ctx));
        }
    }

    fn preview_window(&mut self, ctx: &egui::Context) {
        if self.preview_mode().is_some() {
            return;
        }
        let Some(preview) = &mut self.preview else {
            return;
        };
        let opts = &mut self.settings.options;
        let mut crop = opts.edit.crop.take();
        let open = preview.show(ctx, &mut opts.edit, &mut crop);
        opts.edit.crop = crop;
        if !open {
            self.preview = None;
        }
    }

    fn show_media_info(&mut self, index: usize) {
        let (Some(ffmpeg), Some(path)) =
            (self.toolbox.get(External::Ffmpeg), self.files.get(index))
        else {
            return;
        };
        let text = match ffmpeg::describe(ffmpeg, path, None) {
            Ok(text) => text
                .lines()
                .filter(|l| !l.contains("At least one output file") && !l.starts_with("Exiting"))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => e.to_string(),
        };
        self.media_info = Some((file_name(path), text));
    }

    /// Starts finding the recorder's devices, or picks up what was found.
    fn poll_capture_devices(&mut self, ctx: &egui::Context) {
        if self.capture_devices.is_some() {
            return;
        }
        if let Some(rx) = &self.capture_rx {
            if let Ok(devices) = rx.try_recv() {
                self.capture_devices = Some(devices);
                self.capture_rx = None;
            }
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ffmpeg = self.toolbox.get(External::Ffmpeg).map(Path::to_path_buf);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(capture::detect(ffmpeg.as_deref()));
            ctx.request_repaint();
        });
        self.capture_rx = Some(rx);
    }

    fn options_ui(&mut self, ui: &mut egui::Ui) {
        let tool = self.tool;
        if matches!(tool, Tool::Record | Tool::Screenshot) {
            self.poll_capture_devices(ui.ctx());
        }
        let opts = &mut self.settings.options;
        match tool {
            Tool::ToVideo(_) | Tool::JoinVideo | Tool::VideoMix => {
                quality_row(ui, &mut opts.quality);
                resolution_row(ui, &mut opts.resolution);
            }
            Tool::ToAudio(_) | Tool::Rotate | Tool::Speed | Tool::Delogo | Tool::PdfCompress => {
                quality_row(ui, &mut opts.quality);
            }
            Tool::ToImage(_) | Tool::ImagesToPdf | Tool::JoinImages => {
                if tool == Tool::JoinImages {
                    row(ui, tr("option.layout"), |ui| {
                        let choices = JoinLayout::ALL.map(|l| (l, l.label()));
                        widgets::segmented(ui, &mut opts.join_layout, &choices);
                    });
                }
                quality_row(ui, &mut opts.quality);
                image_rows(ui, opts);
                watermark_row(ui, &mut opts.edit);
            }
            Tool::ToDevice(device) => {
                let p = device.profile();
                row(ui, tr("option.profile"), |ui| {
                    ui.label(trf(
                        "device.summary",
                        &[
                            ("w", &p.max_width),
                            ("h", &p.max_height),
                            ("profile", &p.profile),
                            ("kbps", &p.max_kbps),
                        ],
                    ));
                });
                quality_row(ui, &mut opts.quality);
            }
            Tool::BluRayToVideo => {
                row(ui, tr("option.output_format"), |ui| {
                    combo(
                        ui,
                        "bd_format",
                        &mut opts.disc_video_format,
                        &[
                            VideoFormat::Mp4,
                            VideoFormat::Mkv,
                            VideoFormat::Avi,
                            VideoFormat::Webm,
                            VideoFormat::Mpg,
                        ],
                        |f| f.label().to_owned(),
                    );
                });
                quality_row(ui, &mut opts.quality);
                resolution_row(ui, &mut opts.resolution);
            }
            Tool::Crop => {
                row_hint(ui, tr("option.crop"), tr("hint.rect_exact"), |ui| {
                    let mut rect = opts.edit.crop.unwrap_or_default();
                    rect_editor(ui, &mut rect);
                    opts.edit.crop = Some(rect);
                });
                quality_row(ui, &mut opts.quality);
            }
            Tool::AddSubtitles => {
                row(ui, tr("option.subtitle_file"), |ui| {
                    file_slot(ui, &mut opts.edit.subtitle, &["srt", "ass", "ssa", "vtt"]);
                });
            }
            Tool::JoinAudio | Tool::AudioMix => {
                quality_row(ui, &mut opts.quality);
                row(ui, tr("option.output_format"), |ui| {
                    combo(
                        ui,
                        "join_audio",
                        &mut opts.join_audio_format,
                        &AudioFormat::ALL,
                        |f| f.label().to_owned(),
                    );
                });
            }
            Tool::Split => {
                row(ui, tr("option.split_mode"), |ui| {
                    widgets::segmented(
                        ui,
                        &mut opts.split_by_parts,
                        &[
                            (false, tr("option.split_by_time")),
                            (true, tr("option.split_by_parts")),
                        ],
                    );
                });
                if opts.split_by_parts {
                    row(ui, tr("option.parts"), |ui| {
                        ui.add(
                            DragValue::new(&mut opts.split_parts)
                                .range(2..=options::MAX_SPLIT_PARTS),
                        );
                    });
                } else {
                    row(ui, tr("option.segment_length"), |ui| {
                        ui.add(
                            DragValue::new(&mut opts.split_secs)
                                .range(options::MIN_SPLIT_SECS..=86_400.0)
                                .suffix(" s"),
                        );
                    });
                }
            }
            Tool::Compress => {
                row_hint(ui, tr("option.target_size"), tr("hint.target_size"), |ui| {
                    ui.add(
                        DragValue::new(&mut opts.target_size_mb)
                            .range(options::MIN_TARGET_MB..=options::MAX_TARGET_MB)
                            .suffix(" MB"),
                    );
                });
                resolution_row(ui, &mut opts.resolution);
            }
            Tool::ExtractFrames => {
                let hint = match opts.frame_mode {
                    FrameMode::Times => tr("frames.mode_times_hint"),
                    _ => tr("frames.range_hint"),
                };
                row_hint(ui, tr("frames.which"), hint, |ui| {
                    let choices = FrameMode::ALL.map(|m| (m, m.label()));
                    widgets::segmented(ui, &mut opts.frame_mode, &choices);
                });
                match opts.frame_mode {
                    FrameMode::Interval => {
                        row(ui, tr("option.interval"), |ui| {
                            ui.add(
                                DragValue::new(&mut opts.frame_interval_s)
                                    .range(
                                        options::MIN_FRAME_INTERVAL_S
                                            ..=options::MAX_FRAME_INTERVAL_S,
                                    )
                                    .speed(0.1)
                                    .suffix(" s"),
                            );
                        });
                    }
                    FrameMode::All => {}
                    FrameMode::Times => frame_times_rows(
                        ui,
                        &mut opts.frame_times,
                        &mut self.frame_times_text,
                        &mut self.frame_times_error,
                    ),
                }
                row(ui, tr("option.output_format"), |ui| {
                    combo(
                        ui,
                        "frame_format",
                        &mut opts.frame_format,
                        &[
                            ImageFormat::Jpeg,
                            ImageFormat::Png,
                            ImageFormat::Webp,
                            ImageFormat::Bmp,
                        ],
                        |f| f.label().to_owned(),
                    );
                });
            }
            Tool::Volume => volume_row(ui, &mut opts.audio.volume_db),
            Tool::PdfToImages => {
                row(ui, tr("option.dpi"), |ui| {
                    ui.add(
                        DragValue::new(&mut opts.pdf_dpi)
                            .range(options::MIN_PDF_DPI..=options::MAX_PDF_DPI),
                    );
                });
                row(ui, tr("option.output_format"), |ui| {
                    combo(
                        ui,
                        "pdf_format",
                        &mut opts.pdf_image_format,
                        &[ImageFormat::Png, ImageFormat::Jpeg],
                        |f| f.label().to_owned(),
                    );
                });
            }
            Tool::DvdToVideo => {
                row(ui, tr("option.dvd_title"), |ui| {
                    ui.add(DragValue::new(&mut opts.dvd_title).range(0..=99))
                        .on_hover_text(tr("hint.dvd_title"));
                });
                row(ui, tr("option.output_format"), |ui| {
                    combo(
                        ui,
                        "disc_format",
                        &mut opts.disc_video_format,
                        &[
                            VideoFormat::Mp4,
                            VideoFormat::Mkv,
                            VideoFormat::Avi,
                            VideoFormat::Webm,
                            VideoFormat::Mpg,
                        ],
                        |f| f.label().to_owned(),
                    );
                });
                quality_row(ui, &mut opts.quality);
                resolution_row(ui, &mut opts.resolution);
            }
            Tool::CdToAudio => {
                row(ui, tr("option.output_format"), |ui| {
                    combo(
                        ui,
                        "cd_format",
                        &mut opts.cd_audio_format,
                        &[
                            AudioFormat::Mp3,
                            AudioFormat::Flac,
                            AudioFormat::Wav,
                            AudioFormat::Ogg,
                            AudioFormat::M4a,
                            AudioFormat::Opus,
                        ],
                        |f| f.label().to_owned(),
                    );
                });
                quality_row(ui, &mut opts.quality);
            }
            Tool::FolderToIso => {
                row(ui, tr("option.volume_label"), |ui| {
                    ui.add(
                        TextEdit::singleline(&mut opts.iso_label)
                            .char_limit(32)
                            .desired_width(160.0),
                    );
                });
            }
            Tool::IsoToCso => {
                row(ui, tr("option.compression"), |ui| {
                    ui.add(Slider::new(&mut opts.cso_level, 1..=options::MAX_CSO_LEVEL));
                });
            }
            Tool::Download => {
                let best = match self.link.as_ref().map(|l| &l.state) {
                    Some(LinkState::Ready(info)) => info.max_height,
                    _ => None,
                };
                download_rows(ui, &mut opts.download, best);
            }
            Tool::Record => {
                record_rows(ui, &mut opts.record, self.capture_devices.as_ref());
            }
            Tool::Screenshot => {
                screenshot_rows(ui, &mut opts.screenshot, self.capture_devices.as_ref());
            }
            // The tools `has_options` lists have nothing to set.
            _ => {}
        }
        if tool == Tool::Speed {
            row_hint(ui, tr("option.speed"), tr("hint.speed"), |ui| {
                ui.add(
                    Slider::new(&mut opts.speed, options::MIN_SPEED..=options::MAX_SPEED)
                        .suffix("×")
                        .step_by(0.05),
                );
            });
        }
        if tool == Tool::VideoMix {
            row(ui, tr("option.layout"), |ui| {
                let choices = MixLayout::ALL.map(|l| (l, l.label()));
                widgets::segmented(ui, &mut opts.mix_layout, &choices);
            });
        }
        if tool == Tool::Delogo {
            row_hint(ui, tr("option.logo_area"), tr("hint.rect_exact"), |ui| {
                rect_editor(ui, &mut opts.delogo)
            });
        }
    }

    fn advanced_video(&mut self, ui: &mut egui::Ui, format: VideoFormat) {
        let toolbox = &self.toolbox;
        let opts = &mut self.settings.options;
        widgets::collapsible_section(
            ui,
            "adv_video",
            Some(icons::SLIDERS),
            tr("section.output_settings"),
            None,
            |ui| {
                pref_group(ui, None, |ui| {
                    let choices = format.encoder_choices();
                    if choices.len() > 1 {
                        row(ui, tr("option.codec"), |ui| {
                            combo(ui, "codec", &mut opts.video.encoder, choices, |e| {
                                e.label().to_owned()
                            });
                        });
                        row(ui, tr("option.hardware"), |ui| {
                            let available: Vec<HwAccel> = HwAccel::ALL
                                .into_iter()
                                .filter(|hw| {
                                    hw.suffix()
                                        .is_none_or(|s| toolbox.has_encoder(&format!("h264_{s}")))
                                })
                                .collect();
                            combo(ui, "hw", &mut opts.video.hw, &available, |h| {
                                h.label().to_owned()
                            });
                        });
                    }
                    row(ui, tr("option.video_bitrate"), |ui| {
                        optional_number(
                            ui,
                            &mut opts.video.bitrate_kbps,
                            4000,
                            100..=options::MAX_VIDEO_KBPS,
                            " kb/s",
                            tr("option.auto_quality"),
                        );
                    });
                    row(ui, tr("option.hw_decode_on"), |ui| {
                        widgets::switch(ui, &mut opts.video.hw_decode);
                    });
                    row(ui, tr("option.fps"), |ui| {
                        let mut choices: Vec<Option<u32>> = vec![None];
                        choices.extend(options::FRAME_RATES.map(Some));
                        combo(ui, "fps", &mut opts.video.fps, &choices, |f| {
                            f.map_or_else(|| tr("option.original").to_owned(), |n| n.to_string())
                        });
                    });
                });
                if !format.is_animation() {
                    ui.add_space(8.0);
                    audio_grid(ui, &mut opts.audio, true);
                }
            },
        );
    }

    fn advanced_audio(&mut self, ui: &mut egui::Ui, lossy: bool) {
        let opts = &mut self.settings.options;
        widgets::collapsible_section(
            ui,
            "adv_audio",
            Some(icons::SLIDERS),
            tr("section.output_settings"),
            None,
            |ui| audio_grid(ui, &mut opts.audio, lossy),
        );
    }

    fn edit_panel(&mut self, ui: &mut egui::Ui, video: bool) {
        let edit = &mut self.settings.options.edit;
        widgets::collapsible_section(
            ui,
            "edit_panel",
            Some(icons::SCISSORS),
            tr("section.edit"),
            None,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::INFO).color(theme::palette(ui).accent));
                    ui.label(RichText::new(tr("hint.edit_applies")).weak());
                });
                ui.add_space(6.0);
                pref_group(ui, None, |ui| {
                    trim_rows(ui, edit);
                    if !video {
                        return;
                    }
                    row(ui, tr("option.crop"), |ui| {
                        let mut on = edit.crop.is_some();
                        dir::checkbox(ui, &mut on, "");
                        let mut rect = edit.crop.unwrap_or_default();
                        if on {
                            rect_editor(ui, &mut rect);
                        }
                        edit.crop = on.then_some(rect);
                    });
                    row(ui, tr("option.rotation"), |ui| {
                        let mut choices: Vec<Option<Rotation>> = vec![None];
                        choices.extend(Rotation::ALL.map(Some));
                        combo(ui, "edit_rotation", &mut edit.rotation, &choices, |r| {
                            r.map_or_else(
                                || tr("option.none_short").to_owned(),
                                |r| r.label().to_owned(),
                            )
                        });
                    });
                    row(ui, tr("option.subtitles"), |ui| {
                        file_slot(ui, &mut edit.subtitle, &["srt", "ass", "ssa", "vtt"]);
                    });
                    row(ui, tr("option.watermark"), |ui| {
                        file_slot(
                            ui,
                            &mut edit.watermark,
                            &["png", "jpg", "jpeg", "webp", "bmp"],
                        );
                        if edit.watermark.is_some() {
                            let mut corner = edit.watermark_corner.unwrap_or(Corner::BottomRight);
                            combo(ui, "wm_corner", &mut corner, &Corner::ALL, |c| {
                                c.label().to_owned()
                            });
                            edit.watermark_corner = Some(corner);
                        }
                    });
                });
            },
        );
    }

    // ── Queue ───────────────────────────────────────────────────────────

    fn queue_panel(&mut self, ui: &mut egui::Ui) {
        let counts = self.queue.counts();
        let p = theme::palette(ui);
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("queue.title")).strong().size(16.0));
            let total = self.queue.jobs().len();
            ui.label(
                RichText::new(trf(
                    "queue.summary",
                    &[("done", &counts.done), ("total", &total)],
                ))
                .weak(),
            );
            if total == 0 {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(icons::label(icons::LIST_PLUS, tr("queue.empty")))
                        .color(p.text_weak),
                );
            } else {
                ui.add_space(8.0);
                self.queue_controls(ui);
            }
            let mut width = widgets::button_width(
                ui,
                Some(icons::DOTS_THREE_VERTICAL),
                tr("action.more"),
                Size::Regular,
            );
            if total > 0 {
                width += ui.spacing().item_spacing.x
                    + labelled_combo_width(ui, tr("after.label"), self.after_done.label());
            }
            dir::at_end(ui, width, |ui| self.queue_menu(ui));
        });
        if self.queue.jobs().is_empty() {
            return;
        }
        ui.add_space(4.0);
        let mut cancel = None;
        let mut remove = None;
        theme::page_scroll().auto_shrink([false; 2]).show(ui, |ui| {
            pref_group(ui, None, |ui| {
                for job in self.queue.jobs() {
                    job_row(ui, job, &mut cancel, &mut remove);
                }
            });
        });
        if let Some(id) = cancel {
            self.queue.cancel(id);
        }
        if let Some(id) = remove {
            self.queue.remove(id);
        }
    }

    // ── Windows ─────────────────────────────────────────────────────────

    /// Settings as a GNOME-style dialog: it fades and grows in over a
    /// dimmed window, and fades back out when closed.
    fn settings_window(&mut self, ctx: &egui::Context) {
        let time = if ctx.global_style().animation_time == 0.0 {
            0.0
        } else {
            theme::MOTION
        };
        let shown = ctx.animate_bool_with_time(
            egui::Id::unique("settings_dialog_shown"),
            self.show_settings,
            time,
        );
        if shown == 0.0 {
            return;
        }
        let eased = egui::emath::easing::cubic_out(shown);
        let screen = ctx.content_rect();
        let size = egui::vec2(
            SETTINGS_WIDTH.min(screen.width() - SETTINGS_SCREEN_MARGIN),
            SETTINGS_HEIGHT.min(screen.height() - SETTINGS_SCREEN_MARGIN),
        )
        .max(egui::vec2(480.0, 320.0));
        let dark = ctx.global_style().visuals.dark_mode;
        let modal = egui::Modal::new(egui::Id::unique("settings_dialog"))
            .area(egui::Modal::default_area(egui::Id::unique("settings_dialog")).fade_in(false))
            .frame(egui::Frame::NONE)
            .backdrop_color(
                egui::Color32::from_black_alpha(if dark { 110 } else { 70 }).gamma_multiply(eased),
            )
            .show(ctx, |ui| {
                let p = theme::palette(ui);
                ui.multiply_opacity(eased);
                // Grows from 94 % around the window's center.
                let center = screen.center().to_vec2();
                let scale = 0.94 + 0.06 * eased;
                let transform = egui::emath::TSTransform::from_translation(center)
                    * egui::emath::TSTransform::from_scaling(scale)
                    * egui::emath::TSTransform::from_translation(-center);
                ui.with_visual_transform(transform, |ui| {
                    egui::Frame::new()
                        .fill(p.panel)
                        .stroke(ui.visuals().window_stroke)
                        .shadow(ui.visuals().window_shadow)
                        .corner_radius(ui.visuals().window_corner_radius)
                        .inner_margin(Margin::symmetric(16, 10))
                        .show(ui, |ui| {
                            ui.set_min_size(size);
                            ui.set_max_size(size);
                            dir::vertical(ui, |ui| {
                                if self.settings_header(ui) {
                                    self.show_settings = false;
                                }
                                ui.add_space(8.0);
                                self.settings_contents(ui, ctx);
                            });
                        });
                });
            });
        if self.show_settings
            && modal.is_top_modal
            && !modal.any_popup_open
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.show_settings = false;
        }
    }

    /// The dialog's centered title and round close button, like a libadwaita
    /// header bar. Returns `true` when the close button was clicked.
    fn settings_header(&mut self, ui: &mut egui::Ui) -> bool {
        let p = theme::palette(ui);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::hover());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            tr("settings.title"),
            egui::FontId::proportional(15.5),
            p.text,
        );
        let close = egui::Rect::from_center_size(
            egui::pos2(
                dir::mirror_x(rect, rect.max.x - 24.0, 24.0) + 12.0,
                rect.center().y,
            ),
            egui::Vec2::splat(24.0),
        );
        let response = ui
            .interact(close, ui.scope_id().with("close"), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tr("action.close"));
        let hover = ui
            .ctx()
            .animate_bool_with_time(response.id, response.hovered(), theme::MOTION);
        ui.painter().circle_filled(
            close.center(),
            12.0,
            p.control.lerp_to_gamma(p.control_hover, hover),
        );
        ui.painter().text(
            close.center(),
            egui::Align2::CENTER_CENTER,
            icons::X,
            egui::FontId::proportional(13.0),
            p.text,
        );
        response.clicked()
    }

    fn settings_contents(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal_top(|ui| {
            // Section list on the start edge, like most apps' preferences.
            dir::vertical(ui, |ui| {
                ui.set_width(SETTINGS_NAV_WIDTH);
                ui.spacing_mut().item_spacing.y = 3.0;
                widgets::nav_list(ui, "settings_sections", |ui| {
                    let mut chosen = None;
                    for (i, tab) in SettingsTab::ALL.into_iter().enumerate() {
                        let news = tab == SettingsTab::Updates && self.update_news();
                        let selected = self.settings_tab == tab;
                        let item = widgets::nav_item(
                            ui,
                            Some(tab.art()),
                            tab.label(),
                            selected,
                            false,
                            news.then_some(1),
                        );
                        if selected {
                            chosen = Some((i, item.rect));
                        }
                        if item.clicked() {
                            self.settings_tab = tab;
                        }
                    }
                    chosen
                });
            });
            ui.add_space(12.0);
            let right_width = ui.available_width();
            dir::vertical(ui, |ui| {
                ui.set_width(right_width);
                ui.set_max_width(right_width);
                // Each section fades and rises in.
                let tab = self.settings_tab as u8;
                widgets::navigation(ui, "settings", 0, tab.into(), tab, |ui, _| {
                    theme::page_scroll()
                        .id_salt(("settings_page", tab))
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.add_space(4.0);
                            self.settings_page(ui, ctx);
                            ui.add_space(8.0);
                        });
                });
            });
        });
    }

    /// The controls of the selected Settings section, as GNOME-style
    /// groups of rows: what a setting is on the start edge, its control on
    /// the end edge.
    fn settings_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.settings_tab {
            SettingsTab::General => {
                let mut theme_changed = false;
                pref_group(ui, Some((tr("settings.group_appearance"), "")), |ui| {
                    widgets::pref_row(ui, tr("settings.language"), "", |ui| {
                        self.language_picker(ui);
                    });
                    widgets::pref_row(ui, tr("settings.theme"), "", |ui| {
                        let choices = ThemeChoice::ALL.map(|t| (t, t.label()));
                        theme_changed |= widgets::segmented(ui, &mut self.settings.theme, &choices);
                    });
                    widgets::pref_row(ui, tr("settings.reduce_motion"), "", |ui| {
                        theme_changed |=
                            widgets::switch(ui, &mut self.settings.reduce_motion).changed();
                    });
                });
                ui.add_space(GROUP_GAP);
                pref_group(ui, Some((tr("settings.group_files"), "")), |ui| {
                    widgets::pref_row(
                        ui,
                        tr("settings.name_tag"),
                        tr("settings.name_tag_hint"),
                        |ui| {
                            ui.add(
                                TextEdit::singleline(&mut self.settings.options.name_tag)
                                    .hint_text("_converted")
                                    .char_limit(options::MAX_NAME_TAG_LEN)
                                    .desired_width(150.0),
                            );
                        },
                    );
                });
                ui.add_space(GROUP_GAP);
                pref_group(ui, Some((tr("settings.group_advanced"), "")), |ui| {
                    if shell::supported() {
                        widgets::pref_row(
                            ui,
                            tr("settings.shell_label"),
                            tr("settings.shell_short"),
                            |ui| {
                                if widgets::switch(ui, &mut self.shell_menu)
                                    .on_hover_text(tr("settings.shell_hint"))
                                    .changed()
                                {
                                    self.toggle_shell_menu();
                                }
                            },
                        );
                    }
                    widgets::pref_row(
                        ui,
                        tr("settings.debug_logging"),
                        tr("settings.debug_short"),
                        |ui| {
                            if widgets::button(
                                ui,
                                Some(icons::FOLDER_OPEN),
                                "",
                                Kind::Ghost,
                                Size::Regular,
                            )
                            .on_hover_text(tr("settings.open_logs"))
                            .clicked()
                                && let Some(dir) = diagnostics::logs_dir()
                            {
                                let _ = std::fs::create_dir_all(&dir);
                                open_path(&dir);
                            }
                            if widgets::switch(ui, &mut self.debug_logging)
                                .on_hover_text(tr("settings.debug_hint"))
                                .changed()
                            {
                                diagnostics::set_debug(self.debug_logging);
                            }
                        },
                    );
                });
                if theme_changed {
                    self.apply_theme(ctx);
                }
            }
            SettingsTab::Queue => {
                pref_group(ui, Some((tr("settings.group_running"), "")), |ui| {
                    widgets::pref_row(
                        ui,
                        tr("settings.parallel"),
                        tr("settings.parallel_hint"),
                        |ui| {
                            ui.add(
                                DragValue::new(&mut self.settings.parallel_jobs)
                                    .range(1..=MAX_PARALLEL_JOBS),
                            );
                        },
                    );
                    widgets::pref_row(ui, tr("settings.auto_start"), "", |ui| {
                        widgets::switch(ui, &mut self.settings.start_on_add);
                    });
                });
                ui.add_space(GROUP_GAP);
                pref_group(ui, Some((tr("settings.group_finished"), "")), |ui| {
                    widgets::pref_row(ui, tr("settings.notify"), "", |ui| {
                        widgets::switch(ui, &mut self.settings.notify_when_done);
                    });
                    // A custom sound names its file under the title.
                    let file = match &self.settings.completion_sound {
                        crate::settings::CompletionSound::Custom(path) => path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("…")
                            .to_owned(),
                        _ => String::new(),
                    };
                    widgets::pref_row(ui, tr("settings.sound_label"), &file, |ui| {
                        self.sound_picker(ui);
                    });
                });
            }
            SettingsTab::Updates => self.update_settings(ui),
            SettingsTab::Programs => self.programs_settings(ui),
            SettingsTab::About => about_page(ui),
        }
    }

    /// Adds or removes the right-click menu after its switch was flipped.
    fn toggle_shell_menu(&mut self) {
        let done = if self.shell_menu {
            shell::register()
        } else {
            shell::unregister()
        };
        if let Err(e) = done {
            self.shell_menu = shell::is_registered();
            self.toasts.push(
                ToastKind::Error,
                trf("settings.shell_failed", &[("error", &e)]),
            );
        }
    }

    /// The completion sound's drop-down, then Play, then Change for a custom file.
    fn sound_picker(&mut self, ui: &mut egui::Ui) {
        use crate::settings::CompletionSound;
        let label_for = |s: &CompletionSound| match s {
            CompletionSound::Off => tr("settings.sound_off"),
            CompletionSound::Default => tr("settings.sound_default"),
            CompletionSound::Bell => tr("settings.sound_bell"),
            CompletionSound::Victory => tr("settings.sound_victory"),
            CompletionSound::Custom(_) => tr("settings.sound_custom"),
        };
        ComboBox::from_id_salt("completion_sound")
            .selected_text(label_for(&self.settings.completion_sound))
            .show_ui(ui, |ui| {
                dir::menu(ui, |ui| {
                    for choice in [
                        CompletionSound::Default,
                        CompletionSound::Bell,
                        CompletionSound::Victory,
                        CompletionSound::Custom(std::path::PathBuf::new()),
                        CompletionSound::Off,
                    ] {
                        let is_custom_variant = matches!(choice, CompletionSound::Custom(_));
                        let text = label_for(&choice);
                        let currently_custom =
                            matches!(self.settings.completion_sound, CompletionSound::Custom(_));
                        let selected = if is_custom_variant {
                            currently_custom
                        } else {
                            self.settings.completion_sound == choice
                        };
                        if ui.selectable_label(selected, text).clicked() {
                            if !is_custom_variant {
                                self.settings.completion_sound = choice;
                            } else if !currently_custom && let Some(file) = pick_sound_file() {
                                // Picking "Custom file…" asks for the file right away.
                                self.settings.completion_sound = CompletionSound::Custom(file);
                            }
                        }
                    }
                });
            });
        if self.settings.completion_sound != CompletionSound::Off
            && widgets::button(ui, Some(icons::PLAY), "", Kind::Ghost, Size::Regular)
                .on_hover_text(tr("settings.sound_test"))
                .clicked()
        {
            sound::play(
                self.settings.completion_sound.clone(),
                self.toolbox.get(External::Ffmpeg).map(Path::to_path_buf),
            );
        }
        if let CompletionSound::Custom(path) = &mut self.settings.completion_sound
            && widgets::button(ui, Some(icons::FOLDER_OPEN), "", Kind::Ghost, Size::Regular)
                .on_hover_text(path.display().to_string())
                .clicked()
            && let Some(file) = pick_sound_file()
        {
            *path = file;
        }
    }

    fn language_picker(&mut self, ui: &mut egui::Ui) {
        let names = i18n::languages();
        let label = |code: &Option<String>| match code {
            None => tr("settings.system_language").to_owned(),
            Some(code) => names
                .iter()
                .find(|(k, _)| k == code)
                .map_or_else(|| code.clone(), |(_, name)| (*name).to_owned()),
        };
        let mut current = self.settings.language.clone();
        ComboBox::from_id_salt("language")
            .selected_text(label(&current))
            .show_ui(ui, |ui| {
                dir::menu(ui, |ui| {
                    ui.selectable_value(&mut current, None, label(&None));
                    for (code, _) in &names {
                        let choice = Some((*code).to_owned());
                        let text = label(&choice);
                        ui.selectable_value(&mut current, choice, text);
                    }
                });
            });
        if current != self.settings.language {
            i18n::set_language(current.as_deref());
            self.settings.language = current;
        }
    }

    /// ffmpeg and yt-dlp, then the optional helper programs, each a row
    /// with whether it was found.
    fn programs_settings(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let status = |ui: &mut egui::Ui, found: bool| {
            if found {
                widgets::chip(
                    ui,
                    icons::CHECK_CIRCLE,
                    tr("settings.ffmpeg_found"),
                    p.success,
                );
            } else {
                widgets::chip(ui, icons::X_CIRCLE, tr("settings.ffmpeg_missing"), p.danger);
            }
        };
        let path_of = |ext| {
            self.toolbox
                .get(ext)
                .map(|path: &Path| path.display().to_string())
        };
        let ffmpeg = path_of(External::Ffmpeg);
        let ytdlp = path_of(External::YtDlp);
        pref_group(ui, Some((tr("settings.group_core"), "")), |ui| {
            let sub = ffmpeg.clone().unwrap_or_default();
            widgets::pref_row(ui, tr("settings.ffmpeg"), &sub, |ui| {
                status(ui, ffmpeg.is_some());
                if self.settings.ffmpeg_path.is_some()
                    && widgets::button(ui, None, tr("action.auto_detect"), Kind::Ghost, Size::Small)
                        .clicked()
                {
                    self.settings.ffmpeg_path = None;
                    self.toolbox = Toolbox::detect(None);
                }
                if widgets::button(ui, Some(icons::FOLDER_OPEN), "", Kind::Ghost, Size::Regular)
                    .on_hover_text(tr("action.browse"))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    self.settings.ffmpeg_path = Some(path);
                    self.toolbox = Toolbox::detect(self.settings.ffmpeg_path.as_deref());
                }
            });
            let sub = ytdlp
                .clone()
                .unwrap_or_else(|| tr("settings.ytdlp_hint").to_owned());
            widgets::pref_row(ui, "yt-dlp", &sub, |ui| status(ui, ytdlp.is_some()))
                .on_hover_text(tr("settings.ytdlp_hint"));
        });
        ui.add_space(GROUP_GAP);
        let mut rescan = false;
        pref_group(
            ui,
            Some((tr("settings.programs"), tr("settings.programs_hint"))),
            |ui| {
                let mut seen = HashSet::new();
                let shown_above = [External::Ffmpeg, External::YtDlp];
                for ext in External::ALL
                    .into_iter()
                    .filter(|e| !shown_above.contains(e))
                {
                    if !seen.insert(ext.name()) {
                        continue;
                    }
                    let found = self.toolbox.get(ext).is_some();
                    widgets::pref_row(ui, ext.name(), "", |ui| {
                        if found {
                            widgets::chip(
                                ui,
                                icons::CHECK_CIRCLE,
                                tr("settings.program_found"),
                                p.success,
                            );
                        } else {
                            widgets::chip(
                                ui,
                                icons::X_CIRCLE,
                                tr("settings.program_missing"),
                                p.text_weak,
                            );
                        }
                    });
                }
            },
        );
        ui.add_space(10.0);
        ui.with_layout(dir::leading(Align::Center), |ui| {
            rescan = widgets::button(
                ui,
                Some(icons::ARROW_CLOCKWISE),
                tr("action.rescan"),
                Kind::Secondary,
                Size::Small,
            )
            .clicked();
        });
        if rescan {
            self.toolbox = Toolbox::detect(self.settings.ffmpeg_path.as_deref());
        }
    }

    // ── Updates ─────────────────────────────────────────────────────────

    fn start_update_check(&mut self, ctx: &egui::Context, manual: bool) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::check().map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
        self.settings.last_update_check = now_secs();
        self.update = UpdateState::Checking { manual, rx };
    }

    fn start_update_download(&mut self, release: update::Release, ctx: &egui::Context) {
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (thread_release, thread_progress, thread_cancel, ctx) = (
            release.clone(),
            Arc::clone(&progress),
            Arc::clone(&cancel),
            ctx.clone(),
        );
        std::thread::spawn(move || {
            // Installing waits for the package manager on Linux, so it runs here too.
            let result = update::download(&thread_release, &thread_cancel, |p| {
                thread_progress.store(p.to_bits(), Ordering::Relaxed);
                ctx.request_repaint();
            })
            .and_then(|file| update::install(&thread_release, &file))
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.update = UpdateState::Downloading {
            release,
            progress,
            cancel,
            rx,
        };
    }

    fn poll_update(&mut self, ctx: &egui::Context) {
        let next = match &self.update {
            UpdateState::Checking { manual, rx } => match rx.try_recv() {
                Ok(Ok(Some(release))) => {
                    let skipped = self.settings.skipped_version.as_deref()
                        == Some(release.version.to_string().as_str());
                    if skipped && !manual {
                        UpdateState::Idle
                    } else {
                        UpdateState::Available(release)
                    }
                }
                Ok(Ok(None)) if *manual => UpdateState::UpToDate,
                Ok(Err(message)) if *manual => UpdateState::Failed(message),
                Ok(result) => {
                    if let Err(message) = result {
                        tracing::info!(%message, "update check failed");
                    }
                    UpdateState::Idle
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => UpdateState::Idle,
            },
            UpdateState::Downloading { release, rx, .. } => match rx.try_recv() {
                Ok(Ok(outcome)) => {
                    if outcome == update::Installed::QuitForInstaller {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    UpdateState::Installed(release.clone(), outcome)
                }
                Ok(Err(message)) => UpdateState::Failed(message),
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => UpdateState::Idle,
            },
            _ => return,
        };
        self.update = next;
    }

    /// Whether there is news about an update that Settings → Updates shows.
    fn update_news(&self) -> bool {
        matches!(
            self.update,
            UpdateState::Available(_)
                | UpdateState::Downloading { .. }
                | UpdateState::Installed(..)
                | UpdateState::Failed(_)
        )
    }

    /// The update card in Settings → Updates while an update is available or
    /// underway: what is happening on top, what you can do about it below.
    fn update_card(&mut self, ui: &mut egui::Ui) {
        let (glyph, hue, title, detail) = match &self.update {
            UpdateState::Available(release) => (
                art::glyph::DOWNLOAD,
                art::hue::BLUE,
                trf("update.available_title", &[("version", &release.version)]),
                Some(trf(
                    "update.current_version",
                    &[("current", &update::VERSION)],
                )),
            ),
            UpdateState::Downloading { release, .. } => (
                art::glyph::DOWNLOAD,
                art::hue::BLUE,
                trf("update.downloading", &[("version", &release.version)]),
                None,
            ),
            UpdateState::Installed(release, outcome) => {
                let key = match outcome {
                    update::Installed::Restart => "update.restart",
                    update::Installed::FinishInInstaller => "update.finish_in_installer",
                    update::Installed::QuitForInstaller => "update.installer_running",
                };
                (
                    art::glyph::SPARKLE,
                    art::hue::GREEN,
                    trf(key, &[("version", &release.version)]),
                    None,
                )
            }
            UpdateState::Failed(message) => (
                icons::WARNING,
                art::hue::RED,
                tr("update.failed_title").to_owned(),
                Some(message.clone()),
            ),
            _ => return,
        };
        let p = theme::palette(ui);
        let mut action = None;
        egui::Frame::new()
            .fill(p.surface.lerp_to_gamma(hue, 0.08))
            .stroke(Stroke::new(1.0, hue.gamma_multiply(0.45)))
            .corner_radius(CornerRadius::same(theme::RADIUS + 4))
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    widgets::icon_tile(ui, art::Art { glyph, hue }, 40.0, true);
                    ui.add_space(4.0);
                    dir::vertical(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add(egui::Label::new(RichText::new(&title).size(15.5).strong()).wrap());
                        if let Some(detail) = &detail {
                            ui.add(
                                egui::Label::new(RichText::new(detail).small().color(p.text_weak))
                                    .wrap(),
                            );
                        }
                    });
                });
                ui.add_space(12.0);
                action = self.update_card_actions(ui);
                // The release's notes, right here instead of on GitHub.
                if let UpdateState::Available(release) = &self.update
                    && !release.notes.is_empty()
                {
                    ui.add_space(8.0);
                    widgets::collapsible_section(
                        ui,
                        ("update_notes", release.version.to_string()),
                        Some(icons::SPARKLE),
                        tr("update.whats_new"),
                        None,
                        |ui| release_notes(ui, &release.notes),
                    );
                }
            });
        match action {
            Some(UpdateAction::Download(release)) => {
                self.start_update_download(release, ui.ctx());
            }
            Some(UpdateAction::Retry) => self.start_update_check(ui.ctx(), true),
            Some(UpdateAction::Skip(version)) => {
                self.settings.skipped_version = Some(version);
                self.update = UpdateState::Idle;
            }
            Some(UpdateAction::Restart) => match update::relaunch() {
                Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.update = UpdateState::Failed(e.to_string()),
            },
            None => {}
        }
    }

    /// The update card's buttons, or its progress bar while downloading.
    fn update_card_actions(&self, ui: &mut egui::Ui) -> Option<UpdateAction> {
        let button = |ui: &mut egui::Ui, icon: Option<&str>, text: &str, kind| {
            widgets::button(ui, icon, text, kind, Size::Regular).clicked()
        };
        let mut action = None;
        ui.horizontal(|ui| match &self.update {
            UpdateState::Available(release) => {
                if button(
                    ui,
                    Some(icons::DOWNLOAD),
                    tr("update.install"),
                    Kind::Primary,
                ) {
                    action = Some(UpdateAction::Download(release.clone()));
                }
                // With no notes to show here, "What's new" opens the release page.
                if release.notes.is_empty()
                    && button(
                        ui,
                        Some(icons::ARROW_SQUARE_OUT),
                        tr("update.whats_new"),
                        Kind::Secondary,
                    )
                {
                    open_url(&release.page);
                }
                ui.with_layout(dir::trailing(Align::Center), |ui| {
                    if button(ui, None, tr("update.skip"), Kind::Ghost) {
                        action = Some(UpdateAction::Skip(release.version.to_string()));
                    }
                });
            }
            UpdateState::Downloading {
                progress, cancel, ..
            } => {
                let fraction = f32::from_bits(progress.load(Ordering::Relaxed));
                ui.with_layout(dir::trailing(Align::Center), |ui| {
                    if button(ui, Some(icons::X), tr("action.cancel"), Kind::Ghost) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    let width = ui.available_width() - ui.spacing().item_spacing.x;
                    widgets::progress(
                        ui,
                        Id::unique("update_download"),
                        fraction,
                        format!("{:.0}%", fraction * 100.0),
                        width,
                    );
                });
            }
            UpdateState::Installed(_, update::Installed::Restart) => {
                action = button(
                    ui,
                    Some(icons::ARROW_CLOCKWISE),
                    tr("update.restart_now"),
                    Kind::Primary,
                )
                .then_some(UpdateAction::Restart);
            }
            UpdateState::Failed(_) => {
                action = button(
                    ui,
                    Some(icons::ARROW_CLOCKWISE),
                    tr("update.try_again"),
                    Kind::Secondary,
                )
                .then_some(UpdateAction::Retry);
            }
            _ => {}
        });
        action
    }

    /// The Updates section of Settings.
    fn update_settings(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let mut check_now = false;
        let version = format!("{} {}", tr("update.version_label"), update::VERSION);
        let status = match &self.update {
            UpdateState::Checking { .. } => format!("{version} · {}", tr("update.checking")),
            UpdateState::UpToDate => format!("{version} · {}", tr("update.up_to_date")),
            // The card below has the details and the buttons.
            _ => version,
        };
        pref_group(ui, None, |ui| {
            widgets::pref_row(ui, "FMX (Free Media Xpress)", &status, |ui| {
                let busy = matches!(
                    self.update,
                    UpdateState::Checking { .. } | UpdateState::Downloading { .. }
                );
                check_now = ui
                    .add_enabled_ui(!busy, |ui| {
                        widgets::button(
                            ui,
                            Some(icons::ARROW_CLOCKWISE),
                            tr("update.check_now"),
                            Kind::Secondary,
                            Size::Small,
                        )
                    })
                    .inner
                    .clicked();
            });
            widgets::pref_row(ui, tr("update.automatic"), "", |ui| {
                widgets::switch(ui, &mut self.settings.check_updates);
            });
        });
        ui.add_space(14.0);
        self.update_card(ui);
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(
                RichText::new(tr("update.safety_hint"))
                    .small()
                    .color(p.text_weak),
            )
            .wrap(),
        );
        if check_now {
            self.start_update_check(ui.ctx(), true);
        }
    }

    /// yt-dlp's status in Settings.
    fn media_info_window(&mut self, ctx: &egui::Context) {
        let Some((title, text)) = &self.media_info else {
            return;
        };
        let mut open = true;
        egui::Window::new(icons::label(
            icons::INFO,
            &trf("info.title", &[("name", title)]),
        ))
        .open(&mut open)
        .default_size([600.0, 380.0])
        .show(ctx, |ui| {
            ScrollArea::both().show(ui, |ui| {
                ui.label(RichText::new(text.as_str()).monospace());
            });
        });
        if !open {
            self.media_info = None;
        }
    }
}

/// What the update card asked for.
enum UpdateAction {
    Download(update::Release),
    Retry,
    Skip(String),
    Restart,
}

enum FileAction {
    Remove(usize),
    Swap(usize, usize),
    Info(usize),
    Preview(usize),
    Add,
}

/// What is known about an added file.
struct FileDetails {
    bytes: Option<u64>,
    /// Length in seconds, once probed.
    duration: Option<f64>,
}

/// What every row of one file list shows.
struct RowContext {
    count: usize,
    /// Order matters, so rows get numbers and move buttons.
    ordered: bool,
    probeable: bool,
    /// Show the preview (eye) button.
    eye: bool,
    /// The row shown in the inline preview.
    selected: Option<usize>,
}

/// Tools whose clip and crop can be set in the preview window.
fn previewable(tool: Tool) -> bool {
    matches!(tool, Tool::ToVideo(_) | Tool::ToDevice(_))
}

/// Tools that show the video inside the editor, and what can be picked on it.
fn inline_preview_mode(tool: Tool) -> Option<PreviewMode> {
    match tool {
        Tool::Trim => Some(PreviewMode::CLIP),
        Tool::Crop | Tool::Delogo => Some(PreviewMode::AREA),
        Tool::Rotate => Some(PreviewMode::ROTATION),
        _ => None,
    }
}

/// Whether the tool has any settings to show.
fn has_options(tool: Tool) -> bool {
    !matches!(
        tool,
        Tool::Mux
            | Tool::RemoveAudio
            | Tool::Normalize
            | Tool::Repair
            | Tool::PdfToText
            | Tool::PdfToHtml
            | Tool::PdfToWord
            | Tool::OfficeToPdf
            | Tool::PdfMerge
            | Tool::PdfSplit
            | Tool::CsoToIso
            | Tool::PdfToExcel
            | Tool::DiscToIso
            | Tool::ExtractSubtitles
            | Tool::ToEbook(_)
    )
}

/// Names for the two inputs of tools that take exactly two files.
fn slot_roles(tool: Tool) -> Option<[&'static str; 2]> {
    match tool {
        Tool::Mux => Some([tr("role.video"), tr("role.audio")]),
        Tool::VideoMix => Some([tr("role.main_video"), tr("role.second_video")]),
        _ => None,
    }
}

/// One numbered step of the task editor: a card with a badge, a title and a
/// short explanation above its controls.
fn step(
    ui: &mut egui::Ui,
    number: usize,
    done: bool,
    title: &str,
    help: &str,
    content: impl FnOnce(&mut egui::Ui),
) {
    card(ui, |ui| {
        step_header(ui, number, done, title, help);
        ui.add_space(12.0);
        content(ui);
    });
}

/// Like [`step`], as a card of exactly `size` for one side of the editor.
fn step_panel(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    number: usize,
    done: bool,
    title: &str,
    help: &str,
    content: impl FnOnce(&mut egui::Ui),
) {
    let margin = 14;
    // The frame's margin and stroke on both sides.
    let inner = size - egui::Vec2::splat(f32::from(margin) * 2.0 + 2.0);
    ui.allocate_ui_with_layout(size, dir::column(), |ui| {
        card_frame(ui)
            .inner_margin(Margin::same(margin))
            .show(ui, |ui| {
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                step_header(ui, number, done, title, help);
                ui.add_space(10.0);
                content(ui);
            });
    });
}

/// A step's badge, title and one-line explanation.
fn step_header(ui: &mut egui::Ui, number: usize, done: bool, title: &str, help: &str) {
    let p = theme::palette(ui);
    ui.horizontal(|ui| {
        widgets::step_badge(ui, number, done);
        ui.add_space(4.0);
        dir::vertical(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).strong().size(16.5));
            if !help.is_empty() {
                ui.label(RichText::new(help).color(p.text_weak));
            }
        });
    });
}

/// Settings → About, like GNOME's About windows: the app's icon, name and
/// version, then links to its page and its licenses.
fn about_page(ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    let repo = REPOSITORY;
    ui.vertical_centered(|ui| {
        ui.add_space(8.0);
        widgets::icon_tile(ui, art::LOGO, 72.0, true);
        ui.add_space(10.0);
        ui.label(RichText::new(tr("app.name")).size(22.0).strong());
        ui.label(RichText::new("Free Media Xpress").color(p.text_weak));
        ui.add_space(6.0);
        // The version in an accent pill, only as wide as its text.
        let version = ui.painter().layout_no_wrap(
            update::VERSION.to_owned(),
            egui::FontId::proportional(12.5),
            p.accent,
        );
        let (pill, _) =
            ui.allocate_exact_size(version.size() + egui::vec2(20.0, 6.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(pill, CornerRadius::same(255), p.accent.gamma_multiply(0.14));
        ui.painter()
            .galley(pill.center() - version.size() / 2.0, version, p.accent);
        ui.add_space(10.0);
        ui.add(Label::new(RichText::new(tr("about.description")).color(p.text_weak)).wrap());
    });
    ui.add_space(GROUP_GAP);
    pref_group(ui, None, |ui| {
        link_row(ui, tr("about.website"), "github.com/sudo-py-dev/fmx", repo);
        link_row(ui, tr("about.report"), "", &format!("{repo}/issues"));
        link_row(ui, tr("about.releases"), "", &format!("{repo}/releases"));
    });
    ui.add_space(GROUP_GAP);
    pref_group(
        ui,
        Some((tr("about.license_title"), tr("about.license"))),
        |ui| {
            link_row(
                ui,
                "MIT License",
                "",
                &format!("{repo}/blob/master/LICENSE-MIT"),
            );
            link_row(
                ui,
                "Apache License 2.0",
                "",
                &format!("{repo}/blob/master/LICENSE-APACHE"),
            );
            link_row(
                ui,
                tr("about.third_party"),
                tr("about.third_party_hint"),
                &format!("{repo}/blob/master/packaging/THIRD-PARTY-NOTICES.md"),
            );
        },
    );
    ui.add_space(8.0);
}

/// A [`widgets::pref_row`] that opens `url` in the browser when clicked,
/// with the "opens outside" arrow on its end edge.
fn link_row(ui: &mut egui::Ui, title: &str, subtitle: &str, url: &str) {
    let p = theme::palette(ui);
    let row = widgets::pref_row(ui, title, subtitle, |ui| {
        ui.label(
            RichText::new(icons::ARROW_SQUARE_OUT)
                .size(16.0)
                .color(p.text_weak),
        );
    });
    let click = ui
        .interact(row.rect, row.id.with("link"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(url);
    let hover = ui
        .ctx()
        .animate_bool_with_time(click.id, click.hovered(), theme::MOTION);
    if hover > 0.0 {
        ui.painter().rect_filled(
            row.rect.shrink(1.0),
            CornerRadius::same(theme::RADIUS),
            p.text.gamma_multiply(0.04 * hover),
        );
    }
    if click.clicked() {
        open_url(url);
    }
}

/// A group title in bold. The bundled fonts have no bold weight, so the
/// text is drawn twice, a fraction of a point apart.
fn group_title(ui: &mut egui::Ui, title: &str) {
    const EMBOLDEN: f32 = 0.6;
    let color = theme::palette(ui).text;
    let galley =
        ui.painter()
            .layout_no_wrap(title.to_owned(), egui::FontId::proportional(15.0), color);
    let size = galley.size() + egui::vec2(EMBOLDEN, 0.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, title));
    ui.painter().galley(rect.min, galley.clone(), color);
    ui.painter()
        .galley(rect.min + egui::vec2(EMBOLDEN, 0.0), galley, color);
}

/// Space between preferences groups, as on libadwaita's preferences pages.
const GROUP_GAP: f32 = 12.0;

/// A libadwaita preferences group: an optional title and description,
/// then one card of [`widgets::pref_row`]s split by hairlines.
fn pref_group(ui: &mut egui::Ui, heading: Option<(&str, &str)>, rows: impl FnOnce(&mut egui::Ui)) {
    // The title sits close to its card; the page's spacing keeps groups apart.
    dir::vertical(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        if let Some((title, description)) = heading {
            let p = theme::palette(ui);
            // libadwaita's group title: bold, a little inset from the card's edge.
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                group_title(ui, title);
            });
            if !description.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.add(
                        Label::new(RichText::new(description).small().color(p.text_weak)).wrap(),
                    );
                });
            }
            ui.add_space(6.0);
        }
        card_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            rows(ui);
        });
    });
}

/// A small circled position number for ordered lists.
fn position_badge(ui: &mut egui::Ui, number: usize) {
    let p = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(22.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 11.0, p.control);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        number.to_string(),
        egui::FontId::proportional(12.5),
        p.text_weak,
    );
}

/// A placeholder row for a named input that has not been added yet.
/// Returns `true` when its button was clicked.
fn empty_slot(ui: &mut egui::Ui, number: usize, role: &str) -> bool {
    let p = theme::palette(ui);
    let mut clicked = false;
    egui::Frame::new()
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                position_badge(ui, number);
                dir::vertical(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(role).small().strong().color(p.accent));
                    ui.label(RichText::new(tr("slot.empty")).color(p.text_weak));
                });
                ui.with_layout(dir::trailing(Align::Center), |ui| {
                    clicked = widgets::button(
                        ui,
                        Some(icons::PLUS),
                        tr("action.choose"),
                        Kind::Secondary,
                        Size::Small,
                    )
                    .clicked();
                });
            });
        });
    clicked
}
/// About how wide `label` and then a drop-down showing `selected` are, as
/// [`combo`] draws them.
fn labelled_combo_width(ui: &egui::Ui, label: &str, selected: &str) -> f32 {
    let text = |s: &str, style: egui::TextStyle| {
        ui.painter()
            .layout_no_wrap(
                s.to_owned(),
                style.resolve(ui.style()),
                egui::Color32::PLACEHOLDER,
            )
            .size()
            .x
    };
    let spacing = ui.spacing();
    let menu = (text(selected, egui::TextStyle::Button)
        + spacing.icon_width
        + spacing.icon_spacing
        + spacing.button_padding.x * 2.0)
        .max(spacing.combo_width);
    text(label, egui::TextStyle::Body) + spacing.item_spacing.x + menu
}

/// Lays out `content` at most `max_width` wide, centered in the available space.
fn centered(ui: &mut egui::Ui, max_width: f32, content: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width().min(max_width);
    let pad = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.add_space(pad);
        dir::vertical(ui, |ui| {
            ui.set_width(width);
            content(ui);
        });
    });
}

/// "Supported: MP4, MKV, …" for the drop zone.
fn formats_line(kind: InputKind) -> String {
    let exts = kind.extensions();
    if exts.is_empty() {
        return String::new();
    }
    let mut list = exts
        .iter()
        .take(SHOWN_EXTENSIONS)
        .map(|e| e.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(", ");
    if exts.len() > SHOWN_EXTENSIONS {
        list.push_str(", …");
    }
    trf("drop.formats", &[("list", &list)])
}

/// A colored icon for a file, chosen by its type.
fn file_art(path: &Path, kind: InputKind) -> art::Art {
    use art::{glyph, hue};
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let ext = ext.as_str();
    let (glyph, hue) = match (kind, ext) {
        (InputKind::Folder, _) => (glyph::HARD_DRIVE, hue::SLATE),
        (InputKind::Disc, _) | (_, "iso" | "cso") => (glyph::DISC, hue::VIOLET),
        _ if catalog::AUDIO_EXTENSIONS.contains(&ext) => (glyph::FILE_AUDIO, hue::PINK),
        (InputKind::Media, _) => (glyph::FILE_VIDEO, hue::BLUE),
        (InputKind::Images, _) => (glyph::FILE_IMAGE, hue::GREEN),
        (_, "pdf") => (glyph::FILE_PDF, hue::RED),
        (_, "docx" | "doc" | "odt" | "rtf") => (glyph::FILE_DOC, hue::INDIGO),
        (_, "xlsx" | "xlsm" | "xls" | "ods" | "csv" | "tsv") => (glyph::EXCEL, hue::EMERALD),
        (_, "pptx" | "ppt" | "odp") => (glyph::FILE_DOC, hue::ORANGE),
        (_, "html" | "htm") => (glyph::FILE_HTML, hue::ORANGE),
        (_, "txt" | "md") => (glyph::FILE_TXT, hue::SLATE),
        (InputKind::Ebooks, _) => (glyph::BOOK, hue::AMBER),
        _ => (glyph::FILE_TEXT, hue::SLATE),
    };
    art::Art { glyph, hue }
}

/// A file size such as `12.4 MB` (decimal units, like file managers).
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Shows release notes written in simple Markdown: headings, bullet lists
/// and paragraphs, with emphasis, code marks and link targets left out.
fn release_notes(ui: &mut egui::Ui, notes: &str) {
    let p = theme::palette(ui);
    ui.spacing_mut().item_spacing.y = 4.0;
    for line in notes.lines() {
        let line = line.trim();
        if line.is_empty() {
            ui.add_space(4.0);
        } else if let Some(heading) = line.strip_prefix('#') {
            ui.add_space(4.0);
            let heading = plain_markdown(heading.trim_start_matches('#').trim());
            ui.label(RichText::new(heading).strong().size(15.0));
        } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            ui.horizontal_top(|ui| {
                ui.label(RichText::new("•").color(p.accent));
                ui.add(egui::Label::new(plain_markdown(item)).wrap());
            });
        } else {
            ui.add(egui::Label::new(RichText::new(plain_markdown(line)).color(p.text_weak)).wrap());
        }
    }
}

/// Markdown text without its marks: `**bold**`, `` `code` `` and
/// `[text](link)` become plain words.
fn plain_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find("](").and_then(|mid| {
            after[mid + 2..]
                .find(')')
                .map(|close| (mid, mid + 2 + close))
        }) {
            Some((mid, close)) => {
                out.push_str(&after[..mid]);
                rest = &after[close + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "").replace("__", "").replace('`', "")
}

/// The app's icon and name, at the top of the sidebar or header bar.
fn app_title(ui: &mut egui::Ui) {
    widgets::icon_tile(ui, art::LOGO, 26.0, true);
    ui.label(RichText::new(tr("app.name")).size(16.0).strong());
}

/// A libadwaita card: white (lit in dark mode), rounded, softly shadowed.
fn card_frame(ui: &egui::Ui) -> egui::Frame {
    let p = theme::palette(ui);
    let dark = ui.visuals().dark_mode;
    egui::Frame::new()
        .fill(p.surface)
        .stroke(widgets::card_stroke(&p, dark))
        .shadow(widgets::card_shadow(dark))
        .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
}

/// A rounded surface that groups related controls.
fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    card_frame(ui)
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}

/// The second line on a tool card: the file extension or a device's screen box.
/// Empty when it would only repeat the name, as ".mp4" under "MP4" or
/// "MP4 (H.265)" does; ".jxl" under "JPEG XL" stays.
fn card_detail(tool: Tool) -> String {
    let ext = match tool {
        Tool::ToVideo(f) => f.ext(),
        Tool::ToAudio(f) => f.ext(),
        Tool::ToImage(f) => f.ext(),
        _ => "",
    };
    if !ext.is_empty() {
        let first_word = tool.label().split_whitespace().next().unwrap_or_default();
        return if first_word.eq_ignore_ascii_case(ext) {
            String::new()
        } else {
            format!(".{ext}")
        };
    }
    match tool {
        Tool::ToDevice(d) => {
            let p = d.profile();
            format!("{}×{} · H.264", p.max_width, p.max_height)
        }
        _ => String::new(),
    }
}

/// Gives a loaded job an output name that doesn't overwrite anything.
fn fresh_output(spec: &JobSpec, claimed: &mut HashSet<PathBuf>) -> PathBuf {
    if spec.tool == Tool::Download {
        return spec.output.clone();
    }
    let dir = spec.output.parent().unwrap_or(Path::new("."));
    let stem = spec
        .output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = spec
        .output
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    allocate_output(dir, &stem, "", ext.as_deref(), claimed, Path::exists)
}

fn job_source(spec: &JobSpec) -> String {
    if let Some(url) = &spec.url {
        return url.clone();
    }
    let first = spec
        .inputs
        .first()
        .map(|p| file_name(p))
        .unwrap_or_else(|| file_name(&spec.output));
    match spec.inputs.len() {
        0 | 1 => first,
        n => trf("queue.more_files", &[("name", &first), ("count", &(n - 1))]),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Formats a duration as `m:ss` or `h:mm:ss`.
fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Estimated time left from elapsed time and progress.
fn time_left(elapsed: Duration, progress: f32) -> Option<Duration> {
    (MIN_PROGRESS_FOR_ETA..1.0)
        .contains(&progress)
        .then(|| elapsed.mul_f32((1.0 - progress) / progress))
}

/// One labelled row of a two-column options grid.
/// One option of a tool: a GNOME boxed-list row with its controls at the
/// end. Rows go inside a [`pref_group`].
fn row(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    widgets::pref_row(ui, label, "", content);
}

/// The screen recorder's options.
/// The moments Extract Frames saves, in one row: a removable chip for each,
/// then a field to type more (Enter adds them).
fn frame_times_rows(
    ui: &mut egui::Ui,
    times: &mut Vec<f64>,
    text: &mut String,
    error: &mut Option<String>,
) {
    let p = theme::palette(ui);
    widgets::pref_block(ui, tr("frames.moments"), |ui| {
        dir::vertical(ui, |ui| {
            let mut remove = None;
            ui.horizontal_wrapped(|ui| {
                for (i, t) in times.iter().enumerate() {
                    if widgets::button(
                        ui,
                        Some(icons::X),
                        &options::display_time(*t),
                        Kind::Secondary,
                        Size::Small,
                    )
                    .on_hover_text(tr("frames.remove"))
                    .clicked()
                    {
                        remove = Some(i);
                    }
                }
                let field = ui.add(
                    TextEdit::singleline(text)
                        .hint_text(tr("frames.type_hint"))
                        .desired_width(150.0),
                );
                if field.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && !text.trim().is_empty()
                {
                    match options::parse_frame_times(text) {
                        Ok(typed) => {
                            for t in typed {
                                options::add_frame_time(times, t);
                            }
                            text.clear();
                            *error = None;
                        }
                        Err(bad) => *error = Some(bad),
                    }
                }
                if times.len() > 1
                    && widgets::button(ui, None, tr("frames.clear"), Kind::Ghost, Size::Small)
                        .clicked()
                {
                    times.clear();
                }
            });
            if let Some(i) = remove {
                times.remove(i);
            }
            match error {
                Some(bad) => {
                    ui.colored_label(p.danger, trf("frames.bad_time", &[("time", bad)]));
                }
                None if times.is_empty() => {
                    ui.label(RichText::new(tr("preview.marks_hint")).color(p.text_weak));
                }
                None => {}
            }
        });
    });
}

/// What part of the screen to capture; `devices` is `None` while still being
/// found. With `system_dialog`, a custom area is drawn in the system's own
/// dialog (Wayland) instead of typed in.
fn area_rows(
    ui: &mut egui::Ui,
    id: &str,
    area: &mut options::RecordArea,
    region: &mut options::ScreenRect,
    devices: Option<&capture::Devices>,
    system_dialog: bool,
) {
    use options::RecordArea;
    let monitors = devices.map_or(&[][..], |d| &d.monitors);
    let mut areas = vec![RecordArea::Screen];
    areas.extend((0..monitors.len() as u32).map(RecordArea::Monitor));
    areas.push(RecordArea::Region);
    if (devices.is_some() || system_dialog) && !areas.contains(area) {
        *area = RecordArea::Screen;
    }
    row(ui, tr("record.area"), |ui| {
        combo(ui, id, area, &areas, |a| match a {
            RecordArea::Screen => tr("record.area_screen").to_owned(),
            RecordArea::Monitor(i) => monitors.get(i as usize).map_or_else(
                || format!("{} {}", tr("record.screen"), i + 1),
                |m| match m.rect {
                    Some(r) => format!("{} ({}×{})", m.name, r.width, r.height),
                    None => m.name.clone(),
                },
            ),
            RecordArea::Region if system_dialog => tr("shot.area_pick").to_owned(),
            RecordArea::Region => tr("record.area_region").to_owned(),
        });
    });
    if *area == RecordArea::Region && !system_dialog {
        let side = options::MIN_RECORD_SIDE..=options::MAX_RECORD_SIDE;
        row(ui, tr("record.position"), |ui| {
            ui.add(
                DragValue::new(&mut region.x)
                    .range(0..=options::MAX_RECORD_SIDE)
                    .prefix("x "),
            );
            ui.add(
                DragValue::new(&mut region.y)
                    .range(0..=options::MAX_RECORD_SIDE)
                    .prefix("y "),
            );
        });
        row(ui, tr("record.size"), |ui| {
            ui.add(
                DragValue::new(&mut region.width)
                    .range(side.clone())
                    .suffix(" px"),
            );
            ui.label("×");
            ui.add(DragValue::new(&mut region.height).range(side).suffix(" px"));
        });
    }
}

/// The screenshot tool's options.
fn screenshot_rows(
    ui: &mut egui::Ui,
    s: &mut options::ScreenshotOptions,
    devices: Option<&capture::Devices>,
) {
    let wayland = capture::system_picks_area();
    area_rows(
        ui,
        "shot_area",
        &mut s.area,
        &mut s.region,
        devices,
        wayland,
    );
    row(ui, tr("option.output_format"), |ui| {
        let choices = options::ShotFormat::ALL.map(|f| (f, f.label()));
        widgets::segmented(ui, &mut s.format, &choices);
    });
    row(ui, tr("shot.delay"), |ui| {
        ui.add(
            DragValue::new(&mut s.delay_s)
                .range(0..=options::MAX_SHOT_DELAY_S)
                .suffix(tr("shot.seconds_suffix")),
        );
    });
    // The portal draws the pointer as the system decides.
    if !wayland {
        switch_row(ui, tr("record.cursor"), &mut s.cursor);
    }
    switch_row(ui, tr("shot.minimize"), &mut s.minimize);
}

/// The screen recorder's options; `devices` is `None` while still being found.
fn record_rows(
    ui: &mut egui::Ui,
    r: &mut options::RecordOptions,
    devices: Option<&capture::Devices>,
) {
    use options::{RecordAudio, RecordQuality};
    if !capture::system_picks_area() {
        area_rows(
            ui,
            "record_area",
            &mut r.area,
            &mut r.region,
            devices,
            false,
        );
    }
    let audio_choices = match devices {
        Some(d) => {
            let choices = d.audio_choices();
            if !choices.contains(&r.audio) {
                r.audio = *choices.get(1).unwrap_or(&RecordAudio::None);
            }
            choices
        }
        None => vec![r.audio],
    };
    // Windows and macOS need a loopback device to record what they play.
    let loopback_hint = match devices {
        Some(d) if d.loopback.is_none() && cfg!(windows) => tr("record.loopback_windows"),
        Some(d) if d.loopback.is_none() && cfg!(target_os = "macos") => tr("record.loopback_mac"),
        _ => "",
    };
    let audio_row = |ui: &mut egui::Ui| {
        combo(ui, "record_audio", &mut r.audio, &audio_choices, |a| {
            a.label().to_owned()
        });
    };
    if loopback_hint.is_empty() {
        row(ui, tr("option.audio"), audio_row);
    } else {
        row_hint(ui, tr("option.audio"), loopback_hint, audio_row);
    }
    row_hint(ui, tr("option.output_format"), r.format.hint(), |ui| {
        let choices = options::RecordFormat::ALL.map(|f| (f, f.label()));
        widgets::segmented(ui, &mut r.format, &choices);
    });
    row(ui, tr("option.quality"), |ui| {
        let choices = RecordQuality::ALL.map(|q| (q, q.label()));
        widgets::segmented(ui, &mut r.quality, &choices);
    });
    row(ui, tr("option.fps"), |ui| {
        ui.add(DragValue::new(&mut r.fps).range(1..=options::MAX_RECORD_FPS));
    });
    switch_row(ui, tr("record.cursor"), &mut r.cursor);
    switch_row(ui, tr("record.countdown"), &mut r.countdown);
    switch_row(ui, tr("record.minimize"), &mut r.minimize);
}

/// The video downloader's options. `best` is the link's tallest picture, if known.
fn download_rows(ui: &mut egui::Ui, d: &mut options::DownloadOptions, best: Option<u32>) {
    row(ui, tr("option.download_kind"), |ui| {
        let choices = DownloadKind::ALL.map(|k| (k, k.label()));
        widgets::segmented(ui, &mut d.kind, &choices);
    });
    if d.kind == DownloadKind::Video {
        let hint = best.map_or_else(
            || tr("download.quality_hint").to_owned(),
            |h| trf("download.quality_hint_best", &[("height", &h)]),
        );
        row_hint(ui, tr("option.quality"), &hint, |ui| {
            // Only heights the link offers, once it is known.
            if let (Some(h), Some(best)) = (d.max_height, best)
                && h > best
            {
                d.max_height = None;
            }
            let mut choices: Vec<Option<u32>> = vec![None];
            choices.extend(
                options::DOWNLOAD_HEIGHTS
                    .into_iter()
                    .filter(|&h| best.is_none_or(|best| h <= best))
                    .map(Some),
            );
            combo(
                ui,
                "download_height",
                &mut d.max_height,
                &choices,
                |h| match h {
                    None => tr("download.best").to_owned(),
                    Some(h) => height_label(h),
                },
            );
        });
        let format_hint = match d.container {
            options::DownloadContainer::Mp4 => tr("download.container_mp4_hint"),
            options::DownloadContainer::Mkv => tr("download.container_mkv_hint"),
            options::DownloadContainer::Webm => tr("download.container_webm_hint"),
        };
        row_hint(ui, tr("option.output_format"), format_hint, |ui| {
            let choices = options::DownloadContainer::ALL.map(|c| (c, c.label()));
            widgets::segmented(ui, &mut d.container, &choices);
        });
    } else {
        row(ui, tr("option.output_format"), |ui| {
            combo(
                ui,
                "download_audio",
                &mut d.audio_format,
                &options::DownloadAudio::ALL,
                |a| a.label().to_owned(),
            );
        });
    }
    row_hint(
        ui,
        tr("download.playlist"),
        tr("download.playlist_hint"),
        |ui| {
            widgets::switch(ui, &mut d.playlist);
        },
    );
    if d.kind == DownloadKind::Video {
        row(ui, tr("download.subtitles"), |ui| {
            widgets::switch(ui, &mut d.subtitles);
            if d.subtitles {
                ui.add(
                    TextEdit::singleline(&mut d.subtitle_langs)
                        .hint_text("en")
                        .char_limit(options::MAX_SUBTITLE_LANGS_LEN)
                        .desired_width(90.0),
                )
                .on_hover_text(tr("download.subtitle_langs_hint"));
            }
        });
    }
    let cover = download::supports_cover(d);
    row(ui, tr("download.cover"), |ui| {
        ui.add_enabled_ui(cover, |ui| widgets::switch(ui, &mut d.embed_thumbnail))
            .response
            .on_disabled_hover_text(tr("download.cover_unsupported"));
    });
    switch_row(ui, tr("download.metadata"), &mut d.embed_metadata);
    row_hint(
        ui,
        tr("download.speed_label"),
        tr("download.speed_hint"),
        |ui| {
            optional_number(
                ui,
                &mut d.rate_limit_kbps,
                2000,
                50..=options::MAX_RATE_LIMIT_KBPS,
                " KB/s",
                tr("download.unlimited"),
            );
        },
    );
}

/// `1080p (Full HD)` and the like.
fn height_label(height: u32) -> String {
    let name = match height {
        4320 => " (8K)",
        2160 => " (4K)",
        1440 => " (2K)",
        1080 => " (Full HD)",
        720 => " (HD)",
        _ => "",
    };
    format!("{height}p{name}")
}

/// A grid row with a short explanation under the control.
/// An on/off option: a [`row`] ending in a switch, as in GNOME.
fn switch_row(ui: &mut egui::Ui, label: &str, on: &mut bool) {
    row(ui, label, |ui| {
        widgets::switch(ui, on);
    });
}

/// A [`row`] with a line of explanation under its title.
fn row_hint(ui: &mut egui::Ui, label: &str, hint: &str, content: impl FnOnce(&mut egui::Ui)) {
    widgets::pref_row(ui, label, hint, content);
}

/// Trim's options as a GNOME boxed list; its in and out points are set
/// under the video.
fn trim_settings(ui: &mut egui::Ui, opts: &mut Options) {
    pref_group(ui, None, |ui| {
        widgets::pref_row(ui, tr("option.stream_copy"), tr("hint.stream_copy"), |ui| {
            widgets::switch(ui, &mut opts.stream_copy);
        });
        // Re-encoding settings matter only without a fast cut.
        if !opts.stream_copy {
            widgets::pref_row(ui, tr("option.quality"), tr("hint.quality"), |ui| {
                let choices = Quality::ALL.map(|v| (v, v.label()));
                widgets::segmented(ui, &mut opts.quality, &choices);
            });
            widgets::pref_row(ui, tr("option.resolution"), tr("hint.resolution"), |ui| {
                combo(
                    ui,
                    "resolution",
                    &mut opts.resolution,
                    &Resolution::ALL,
                    |v| v.label(),
                );
            });
        }
    });
}

fn quality_row(ui: &mut egui::Ui, q: &mut Quality) {
    row_hint(ui, tr("option.quality"), tr("hint.quality"), |ui| {
        let choices = Quality::ALL.map(|v| (v, v.label()));
        widgets::segmented(ui, q, &choices);
    });
}

fn resolution_row(ui: &mut egui::Ui, r: &mut Resolution) {
    row_hint(ui, tr("option.resolution"), tr("hint.resolution"), |ui| {
        combo(ui, "resolution", r, &Resolution::ALL, |v| v.label())
    });
}

fn volume_row(ui: &mut egui::Ui, db: &mut f32) {
    row(ui, tr("option.volume"), |ui| {
        ui.add(
            Slider::new(db, -options::MAX_VOLUME_DB..=options::MAX_VOLUME_DB)
                .suffix(" dB")
                .step_by(0.5),
        );
    });
}

fn time_input(
    ui: &mut egui::Ui,
    text: &mut String,
    desired_width: f32,
    hint_text: &str,
    other_time: Option<(&str, bool)>,
) {
    let p = theme::palette(ui);
    let trimmed = text.trim().to_owned();
    let is_malformed = !trimmed.is_empty()
        && (trimmed.len() > 32
            || trimmed.chars().any(|c| c.is_control())
            || options::parse_timestamp(&trimmed).is_err());

    let is_empty_window = if !is_malformed && !trimmed.is_empty() {
        if let Some((other_str, is_other_start)) = other_time {
            let other_trimmed = other_str.trim();
            if !other_trimmed.is_empty() {
                if let (Ok(Some(my_val)), Ok(Some(other_val))) = (
                    options::parse_timestamp(&trimmed),
                    options::parse_timestamp(other_trimmed),
                ) {
                    if is_other_start {
                        my_val <= other_val
                    } else {
                        my_val >= other_val
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    let mut widget = TextEdit::singleline(text).desired_width(desired_width);
    if !hint_text.is_empty() {
        widget = widget.hint_text(hint_text);
    }
    let response = ui.add(widget);
    if is_malformed {
        ui.painter().rect_stroke(
            response.rect,
            CornerRadius::same(theme::RADIUS),
            Stroke::new(1.5, p.danger),
            StrokeKind::Inside,
        );
        let _ = response.on_hover_text(trf("error.bad_timestamp", &[("value", &trimmed)]));
    } else if is_empty_window {
        ui.painter().rect_stroke(
            response.rect,
            CornerRadius::same(theme::RADIUS),
            Stroke::new(1.5, p.danger),
            StrokeKind::Inside,
        );
        let _ = response.on_hover_text(tr("error.empty_trim"));
    }
}

fn trim_rows(ui: &mut egui::Ui, edit: &mut Edit) {
    row(ui, tr("option.start"), |ui| {
        let end_clone = edit.trim_end.clone();
        time_input(
            ui,
            &mut edit.trim_start,
            140.0,
            tr("hint.timestamp"),
            Some((&end_clone, false)),
        );
    });
    row(ui, tr("option.end"), |ui| {
        let start_clone = edit.trim_start.clone();
        time_input(
            ui,
            &mut edit.trim_end,
            140.0,
            tr("hint.timestamp"),
            Some((&start_clone, true)),
        );
    });
}

fn image_rows(ui: &mut egui::Ui, opts: &mut options::Options) {
    row(ui, tr("option.max_side"), |ui| {
        optional_number(
            ui,
            &mut opts.image_max_side,
            1920,
            options::MIN_IMAGE_SIDE..=options::MAX_IMAGE_SIDE,
            " px",
            tr("option.original"),
        );
    });
    row(ui, tr("option.rotation"), |ui| {
        let mut choices: Vec<Option<Rotation>> = vec![None];
        choices.extend(Rotation::ALL.map(Some));
        combo(
            ui,
            "image_rotation",
            &mut opts.image_rotation,
            &choices,
            |r| {
                r.map_or_else(
                    || tr("option.none_short").to_owned(),
                    |r| r.label().to_owned(),
                )
            },
        );
    });
    switch_row(ui, tr("option.grayscale"), &mut opts.grayscale);
}

fn audio_grid(ui: &mut egui::Ui, audio: &mut options::AudioSettings, lossy: bool) {
    pref_group(ui, None, |ui| {
        if lossy {
            row(ui, tr("option.audio_bitrate"), |ui| {
                optional_number(
                    ui,
                    &mut audio.bitrate_kbps,
                    192,
                    8..=options::MAX_AUDIO_KBPS,
                    " kb/s",
                    tr("option.auto_quality"),
                );
            });
        }
        row(ui, tr("option.sample_rate"), |ui| {
            let mut choices: Vec<Option<u32>> = vec![None];
            choices.extend(options::SAMPLE_RATES.map(Some));
            combo(ui, "sample_rate", &mut audio.sample_rate, &choices, |r| {
                r.map_or_else(|| tr("option.original").to_owned(), |hz| format!("{hz} Hz"))
            });
        });
        row(ui, tr("option.channels"), |ui| {
            combo(ui, "channels", &mut audio.channels, &Channels::ALL, |c| {
                c.label().to_owned()
            });
        });
        volume_row(ui, &mut audio.volume_db);
    });
}

/// A checkbox that enables a numeric field; unchecked means `None`.
fn optional_number(
    ui: &mut egui::Ui,
    value: &mut Option<u32>,
    default: u32,
    range: std::ops::RangeInclusive<u32>,
    suffix: &str,
    off_label: &str,
) {
    let mut on = value.is_some();
    dir::checkbox(ui, &mut on, "");
    let mut number = value.unwrap_or(default);
    if on {
        ui.add(DragValue::new(&mut number).range(range).suffix(suffix));
    } else {
        ui.label(RichText::new(off_label).weak());
    }
    *value = on.then_some(number);
}

fn rect_editor(ui: &mut egui::Ui, rect: &mut Rect) {
    for (label, value) in [
        ("x", &mut rect.x),
        ("y", &mut rect.y),
        ("w", &mut rect.w),
        ("h", &mut rect.h),
    ] {
        ui.label(RichText::new(label).weak());
        ui.add(DragValue::new(value).range(0..=16_384));
    }
}

fn watermark_row(ui: &mut egui::Ui, edit: &mut Edit) {
    row(ui, tr("option.watermark"), |ui| {
        file_slot(
            ui,
            &mut edit.watermark,
            &["png", "jpg", "jpeg", "webp", "bmp"],
        );
        if edit.watermark.is_some() {
            let mut corner = edit.watermark_corner.unwrap_or(Corner::BottomRight);
            combo(ui, "image_wm_corner", &mut corner, &Corner::ALL, |c| {
                c.label().to_owned()
            });
            edit.watermark_corner = Some(corner);
        }
    });
}

/// A file picker slot showing the chosen file with a clear button.
fn file_slot(ui: &mut egui::Ui, slot: &mut Option<PathBuf>, exts: &[&str]) {
    if widgets::button(
        ui,
        Some(icons::FOLDER_OPEN),
        tr("action.choose"),
        Kind::Secondary,
        Size::Small,
    )
    .clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter(tr("filter.supported"), exts)
            .pick_file()
    {
        *slot = Some(path);
    }
    if let Some(path) = slot.as_ref() {
        ui.label(file_name(path))
            .on_hover_text(path.display().to_string());
        if widgets::button(ui, Some(icons::X), "", Kind::Ghost, Size::Small)
            .on_hover_text(tr("action.remove"))
            .clicked()
        {
            *slot = None;
        }
    }
}

fn combo<T: PartialEq + Clone>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    choices: &[T],
    label: impl Fn(T) -> String,
) {
    #[cfg(debug_assertions)]
    if crate::devshot::wants_menu(id) {
        let button = ui.make_persistent_id(egui::IdSalt::new(id));
        egui::Popup::open_id(ui.ctx(), button.with("popup"));
    }
    ComboBox::from_id_salt(id)
        .selected_text(label(value.clone()))
        .show_ui(ui, |ui| {
            dir::menu(ui, |ui| {
                for choice in choices {
                    ui.selectable_value(value, choice.clone(), label(choice.clone()));
                }
            });
        });
}

/// One job of the queue as a list row: its tool's tile, what it works on,
/// the tool and how it is going, then its progress and actions.
fn job_row(ui: &mut egui::Ui, job: &Job, cancel: &mut Option<JobId>, remove: &mut Option<JobId>) {
    let p = theme::palette(ui);
    let title = job.title.clone().unwrap_or_else(|| job_source(&job.spec));
    let (status, color) = job_status(job, &p);
    let subtitle = format!("{} · {status}", job.spec.tool.label());
    let row = widgets::list_row(
        ui,
        art::tool(job.spec.tool),
        &title,
        &subtitle,
        color,
        |ui| {
            job_controls(ui, job, cancel, remove);
        },
    );
    let details = match &job.state {
        JobState::Failed(message) => format!("{}\n\n{message}", job.spec.output.display()),
        _ => job.spec.output.display().to_string(),
    };
    row.on_hover_text(details);
    if matches!(job.state, JobState::Running { .. }) {
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
}

/// The subtitle's status for a job, and its color.
fn job_status(job: &Job, p: &theme::Palette) -> (String, egui::Color32) {
    match &job.state {
        JobState::Queued => (tr("status.queued").to_owned(), p.text_weak),
        // The recorder's own detail: the countdown, then time and size.
        JobState::Running { .. } if job.spec.tool == Tool::Record => (
            job.detail.clone().unwrap_or_else(|| {
                format!(
                    "{} {}",
                    tr("status.recording"),
                    clock(job.elapsed().unwrap_or_default())
                )
            }),
            p.danger,
        ),
        // A tool's own detail (download size, speed, time left) beats our estimate.
        JobState::Running { .. } if job.detail.is_some() => {
            (job.detail.clone().unwrap_or_default(), p.text_weak)
        }
        JobState::Running {
            progress: Some(fraction),
        } => {
            let left = job.elapsed().and_then(|e| time_left(e, *fraction));
            let text = left.map_or_else(
                || tr("status.running").to_owned(),
                |left| trf("queue.time_left", &[("left", &clock(left))]),
            );
            (text, p.text_weak)
        }
        JobState::Running { progress: None } => (tr("status.running").to_owned(), p.text_weak),
        JobState::Done { elapsed } => (
            trf(
                "status.done",
                &[("secs", &format!("{:.1}", elapsed.as_secs_f32()))],
            ),
            p.success,
        ),
        JobState::Failed(message) => (
            message.lines().next().unwrap_or_default().to_owned(),
            p.danger,
        ),
        JobState::Cancelled => (tr("status.cancelled").to_owned(), p.text_weak),
    }
}

/// A job's progress and its actions, as small icon buttons.
fn job_controls(
    ui: &mut egui::Ui,
    job: &Job,
    cancel: &mut Option<JobId>,
    remove: &mut Option<JobId>,
) {
    let icon = |ui: &mut egui::Ui, icon: &str, hint: &str| {
        widgets::button(ui, Some(icon), "", Kind::Ghost, Size::Small)
            .on_hover_text(hint)
            .clicked()
    };
    match &job.state {
        JobState::Running {
            progress: Some(fraction),
        } if job.spec.tool != Tool::Record => {
            let bar_id = Id::unique(("job_progress", job.id));
            let percent = format!("{:.0}%", fraction * 100.0);
            widgets::progress(ui, bar_id, *fraction, percent, PROGRESS_WIDTH);
        }
        JobState::Running { progress: None } if job.spec.tool != Tool::Record => {
            widgets::busy(ui, "", PROGRESS_WIDTH);
        }
        _ => {}
    }
    match &job.state {
        JobState::Running { .. } if job.spec.tool == Tool::Record => {
            if widgets::button(
                ui,
                Some(icons::STOP),
                tr("action.stop_recording"),
                Kind::Danger,
                Size::Small,
            )
            .clicked()
            {
                *cancel = Some(job.id);
            }
        }
        JobState::Queued | JobState::Running { .. } => {
            if icon(ui, icons::X, tr("action.cancel")) {
                *cancel = Some(job.id);
            }
        }
        JobState::Done { .. } => {
            if job.spec.output.is_file() && icon(ui, icons::ARROW_SQUARE_OUT, tr("action.open")) {
                open_path(&job.spec.output);
            }
            if icon(ui, icons::FOLDER, tr("action.open_folder")) {
                reveal(&job.spec.output);
            }
            if icon(ui, icons::X, tr("action.remove")) {
                *remove = Some(job.id);
            }
        }
        JobState::Failed(_) | JobState::Cancelled => {
            if icon(ui, icons::X, tr("action.remove")) {
                *remove = Some(job.id);
            }
        }
    }
}

fn opener() -> &'static str {
    if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Opens a file with its default application.
fn open_path(path: &Path) {
    if let Err(e) = spawn_detached(opener(), [path]) {
        tracing::warn!(error = %e, "could not open file");
    }
}

/// Opens a web page in the default browser.
fn open_url(url: &str) {
    if let Err(e) = spawn_detached(opener(), [url]) {
        tracing::warn!(error = %e, "could not open web page");
    }
}

/// Opens the folder containing `path` in the system file manager.
fn reveal(path: &Path) {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    open_path(dir);
}

/// Shows a system notification; failures are only logged.
fn desktop_notification(title: &str, body: &str) {
    let result = if cfg!(target_os = "macos") {
        // AppleScript strings: escape backslashes and quotes.
        let quote = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            quote(body),
            quote(title)
        );
        spawn_detached("osascript", ["-e", &script])
    } else if cfg!(windows) {
        // No dependency-free toast API; the in-app notification covers Windows.
        return;
    } else {
        spawn_detached("notify-send", ["--app-name", title, title, body])
    };
    if let Err(e) = result {
        tracing::debug!(error = %e, "desktop notification unavailable");
    }
}

/// Asks for an audio file to use as the completion sound.
fn pick_sound_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(tr("settings.sound_custom"), sound::CUSTOM_EXTENSIONS)
        .pick_file()
}

/// Powers the computer off (the user opted in and saw a countdown).
fn power_off() {
    let result = if cfg!(windows) {
        spawn_detached("shutdown", ["/s", "/t", "0"])
    } else if cfg!(target_os = "macos") {
        spawn_detached(
            "osascript",
            ["-e", "tell app \"System Events\" to shut down"],
        )
    } else {
        spawn_detached("systemctl", ["poweroff"])
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, "could not shut down");
    }
}

impl eframe::App for FmxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(debug_assertions)]
        crate::devshot::tick(&ctx);
        // Each call means the previous frame was drawn.
        match self.self_test_frames {
            Some(0) => {
                tracing::info!("self-test passed: the window opened and drew frames");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                self.self_test_frames = None;
            }
            Some(left) => {
                self.self_test_frames = Some(left - 1);
                ctx.request_repaint();
            }
            None => {}
        }
        let requests: Vec<instance::Request> = self
            .requests
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for request in requests {
            self.handle_request(&ctx, request);
        }
        self.handle_input(&ctx);
        if exec::take_minimize_request() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        self.sync_window_theme(&ctx);
        self.queue
            .pump(self.settings.parallel_jobs, &self.toolbox, &self.notifier);
        self.check_after_done(&ctx);
        self.poll_update(&ctx);
        self.poll_link(&ctx);
        self.poll_details();
        let p = theme::palette(ui);

        let bar = |fill| {
            egui::Frame::new()
                .fill(fill)
                .inner_margin(Margin::symmetric(14, 10))
        };
        // Like GNOME Settings, the sidebar runs the window's full height with
        // the app's name and Settings on top, and the content has no header bar:
        // an open tool is a page with its own back button. The queue makes way
        // for the tool's action bar and comes back once its jobs are added.
        dir::start_panel("tools")
            .resizable(true)
            .default_size(SIDEBAR_WIDTH)
            .frame(bar(p.sidebar))
            .show(ui, |ui| dir::vertical(ui, |ui| self.sidebar(ui)));
        if !self.in_editor {
            // An empty queue is a single line.
            if self.queue.jobs().is_empty() {
                egui::Panel::bottom("queue_empty")
                    .resizable(false)
                    .frame(bar(p.surface))
                    .show(ui, |ui| dir::vertical(ui, |ui| self.queue_panel(ui)));
            } else {
                egui::Panel::bottom("queue")
                    .resizable(true)
                    .default_size(QUEUE_HEIGHT)
                    .frame(bar(p.surface))
                    .show(ui, |ui| dir::vertical(ui, |ui| self.queue_panel(ui)));
            }
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, CornerRadius::ZERO, p.panel);
            let p = theme::palette(ui);
            let mut in_editor = self.in_editor;
            egui::Panel::bottom("editor_actions")
                .frame(
                    egui::Frame::new()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0, p.border))
                        .inner_margin(Margin::symmetric(18, 10)),
                )
                .show_collapsible(ui, &mut in_editor, |ui| {
                    dir::vertical(ui, |ui| {
                        centered(ui, EDITOR_MAX_WIDTH, |ui| self.action_bar(ui));
                    });
                });
            // Opening a tool slides its page in over the tools, like a GNOME
            // subpage; another category fades in, rising or sinking the way
            // the sidebar's highlight moves.
            let depth = u8::from(self.in_editor);
            let order = Category::ALL
                .iter()
                .position(|c| *c == self.category)
                .unwrap_or_default();
            let key = (self.tool, self.category, self.search.is_empty());
            widgets::navigation(ui, "main", depth, order, key, |ui, depth| {
                egui::Frame::new()
                    // The same top margin as the sidebar, so a page's header row
                    // lines up with the sidebar's.
                    .inner_margin(Margin {
                        left: 18,
                        right: 18,
                        top: 10,
                        bottom: 14,
                    })
                    .show(ui, |ui| {
                        dir::vertical(ui, |ui| {
                            if depth == 1 {
                                centered(ui, EDITOR_MAX_WIDTH, |ui| self.task_editor(ui));
                            } else {
                                theme::page_scroll()
                                    .auto_shrink([false; 2])
                                    .show(ui, |ui| self.tool_grid(ui));
                            }
                        });
                    });
            });
        });
        self.settings_window(&ctx);
        self.media_info_window(&ctx);
        self.preview_window(&ctx);
        self.toasts.show(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_minutes_and_hours() {
        assert_eq!(clock(Duration::from_secs(5)), "0:05");
        assert_eq!(clock(Duration::from_secs(125)), "2:05");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn sizes_use_readable_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(12_400_000), "12.4 MB");
        assert_eq!(human_size(3_000_000_000), "3.0 GB");
    }

    #[test]
    fn markdown_marks_are_removed() {
        assert_eq!(
            plain_markdown("**Faster** `ffmpeg` and [the docs](https://x.y) [x]"),
            "Faster ffmpeg and the docs [x]"
        );
    }

    #[test]
    fn card_detail_only_adds_what_the_name_lacks() {
        assert_eq!(card_detail(Tool::ToVideo(VideoFormat::Mp4)), "");
        assert_eq!(card_detail(Tool::ToVideo(VideoFormat::Mp4Hevc)), "");
        assert_eq!(card_detail(Tool::ToImage(ImageFormat::Jxl)), ".jxl");
        assert_eq!(card_detail(Tool::JoinImages), "");
    }

    #[test]
    fn formats_line_lists_a_few_extensions() {
        assert!(formats_line(InputKind::Pdf).contains("PDF"));
        assert!(formats_line(InputKind::Media).ends_with("…"));
        assert!(formats_line(InputKind::Folder).is_empty());
    }

    #[test]
    fn eta_needs_some_progress() {
        assert_eq!(time_left(Duration::from_secs(10), 0.01), None);
        assert_eq!(
            time_left(Duration::from_secs(10), 0.5),
            Some(Duration::from_secs(10))
        );
        assert_eq!(time_left(Duration::from_secs(10), 1.0), None);
    }

    #[test]
    fn rotate_tool_has_inline_preview() {
        assert_eq!(
            inline_preview_mode(Tool::Rotate),
            Some(PreviewMode::ROTATION)
        );
    }
}
