//! The egui front end: tool sidebar, task editor, and job queue.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, ComboBox, CornerRadius, DragValue, Id, Key, KeyboardShortcut, Layout, Margin,
    Modifiers, RichText, ScrollArea, Slider, Stroke, TextEdit,
};

use crate::catalog::{
    AudioFormat, Category, ImageFormat, InputArity, InputKind, Tool, VideoFormat,
};
use crate::externals::{External, Toolbox};
use crate::ffmpeg;
use crate::i18n::{self, tr, trf};
use crate::icons;
use crate::jobs::{JobSpec, Selection, allocate_output, plan_jobs};
use crate::options::{
    self, Channels, Corner, DownloadKind, Edit, HwAccel, MixLayout, Quality, Rect, Resolution,
    Rotation,
};
use crate::queue::{Job, JobState, Notifier, Queue};
use crate::settings::{MAX_PARALLEL_JOBS, STORAGE_KEY, Settings};
use crate::theme::{self, MOTION, ThemeChoice};
use crate::widgets::{self, Kind, Size, ToastKind, Toasts};

const ADD_FILES: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const ENQUEUE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
const SIDEBAR_WIDTH: f32 = 230.0;
const QUEUE_HEIGHT: f32 = 230.0;
const PROGRESS_WIDTH: f32 = 220.0;
const DROP_ZONE_HEIGHT: f32 = 90.0;
const SHUTDOWN_DELAY: Duration = Duration::from_secs(60);
/// Task-list files larger than this are refused.
const MAX_TASK_LIST_BYTES: u64 = 10 << 20;
/// Folder scans stop after this many files.
const MAX_SCAN_FILES: usize = 10_000;
/// Progress below this is too early to estimate time left.
const MIN_PROGRESS_FOR_ETA: f32 = 0.03;

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

pub struct MuxrApp {
    settings: Settings,
    queue: Queue,
    toolbox: Toolbox,
    tool: Tool,
    files: Vec<PathBuf>,
    url: String,
    toasts: Toasts,
    show_settings: bool,
    media_info: Option<(String, String)>,
    after_done: AfterDone,
    was_busy: bool,
    shutdown_at: Option<Instant>,
    notifier: Notifier,
}

impl MuxrApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        icons::install(&cc.egui_ctx);
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        theme::apply(&cc.egui_ctx, settings.theme, settings.reduce_motion);
        // Clip/crop/subtitle edits belong to one batch, not to the next session.
        settings.options.edit = Edit::default();
        // MUXR_LANG (applied in main) wins unless a language was chosen in Settings.
        if settings.language.is_some() || std::env::var_os("MUXR_LANG").is_none() {
            i18n::set_language(settings.language.as_deref());
        }
        let ctx = cc.egui_ctx.clone();
        let toolbox = Toolbox::detect(settings.ffmpeg_path.as_deref());
        if toolbox.get(External::Ffmpeg).is_none() {
            tracing::warn!("ffmpeg not found; media tools are disabled");
        }
        Self {
            tool: settings.last_tool,
            queue: Queue::new(settings.auto_start),
            toolbox,
            files: Vec::new(),
            url: String::new(),
            toasts: Toasts::default(),
            show_settings: false,
            media_info: None,
            after_done: AfterDone::Nothing,
            was_busy: false,
            shutdown_at: None,
            notifier: Arc::new(move || ctx.request_repaint()),
            settings,
        }
    }

    fn select_tool(&mut self, tool: Tool) {
        if tool.input_kind() != self.tool.input_kind() {
            self.files.clear();
        }
        self.tool = tool;
        self.settings.last_tool = tool;
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
        for path in paths {
            if !self.accepts(&path) {
                skipped += 1;
            } else if known.insert(path.clone()) {
                self.files.push(path);
            }
        }
        if skipped > 0 {
            self.toasts.push(
                ToastKind::Error,
                trf("notice.skipped", &[("count", &skipped)]),
            );
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
                specs.into_iter().for_each(|s| self.queue.push(s));
                self.files.clear();
                self.url.clear();
                self.settings.options.edit = Edit::default();
                if self.settings.auto_start {
                    self.queue.is_running = true;
                }
                self.toasts.push(
                    ToastKind::Success,
                    trf("notice.queued", &[("count", &count)]),
                );
            }
            Err(e) => self.toasts.push(ToastKind::Error, e.to_string()),
        }
    }

    fn can_enqueue(&self) -> bool {
        let has_input = match self.tool.input_kind() {
            InputKind::Url => !self.url.trim().is_empty(),
            InputKind::Device => !self.settings.cd_device.trim().is_empty(),
            InputKind::Capture => true,
            _ => !self.files.is_empty(),
        };
        has_input && self.unavailable_reason(self.tool).is_none()
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
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ADD_FILES)) {
            self.pick_files();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ENQUEUE)) && self.can_enqueue() {
            self.enqueue();
        }
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        theme::apply(ctx, self.settings.theme, self.settings.reduce_motion);
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
            .set_file_name("muxr-tasks.json")
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
        self.was_busy = busy;
        if just_finished && counts.done > 0 {
            self.toasts.push(ToastKind::Success, tr("notice.all_done"));
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
                ui.label(trf("shutdown.countdown", &[("secs", &left.as_secs())]));
                ui.add_space(8.0);
                cancel = widgets::button(
                    ui,
                    Some(icons::X),
                    tr("action.cancel"),
                    Kind::Primary,
                    Size::Regular,
                    true,
                )
                .clicked();
            });
        if cancel {
            self.shutdown_at = None;
            self.after_done = AfterDone::Nothing;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    // ── Toolbar ─────────────────────────────────────────────────────────

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let counts = self.queue.counts();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("app.name")).heading().strong());
            ui.add_space(8.0);
            let (icon, label, hint, kind) = if self.queue.is_running {
                (
                    icons::PAUSE,
                    tr("action.pause"),
                    tr("hint.pause"),
                    Kind::Secondary,
                )
            } else {
                (
                    icons::PLAY,
                    tr("action.start"),
                    tr("hint.start"),
                    Kind::Primary,
                )
            };
            if widgets::button(ui, Some(icon), label, kind, Size::Regular, true)
                .on_hover_text(hint)
                .clicked()
            {
                self.queue.is_running = !self.queue.is_running;
            }
            let busy = counts.running + counts.queued > 0;
            if widgets::button(
                ui,
                Some(icons::STOP),
                tr("action.stop_all"),
                Kind::Danger,
                Size::Regular,
                busy,
            )
            .on_hover_text(tr("hint.stop_all"))
            .clicked()
            {
                self.queue.cancel_all();
            }
            if widgets::button(
                ui,
                Some(icons::ARROW_CLOCKWISE),
                tr("action.retry"),
                Kind::Secondary,
                Size::Regular,
                counts.failed > 0,
            )
            .clicked()
            {
                self.queue.retry_failed();
            }
            let finished = counts.done + counts.failed > 0;
            if widgets::button(
                ui,
                Some(icons::BROOM),
                tr("action.clear_finished"),
                Kind::Secondary,
                Size::Regular,
                finished,
            )
            .clicked()
            {
                self.queue.clear_finished();
            }
            ui.separator();
            if widgets::button(
                ui,
                Some(icons::FLOPPY_DISK),
                tr("action.save_list"),
                Kind::Ghost,
                Size::Regular,
                true,
            )
            .clicked()
            {
                self.save_task_list();
            }
            if widgets::button(
                ui,
                Some(icons::FOLDER_OPEN),
                tr("action.load_list"),
                Kind::Ghost,
                Size::Regular,
                true,
            )
            .clicked()
            {
                self.load_task_list();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::button(
                    ui,
                    Some(icons::GEAR),
                    tr("action.settings"),
                    Kind::Ghost,
                    Size::Regular,
                    true,
                )
                .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
            });
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            self.output_dir_picker(ui);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                combo(
                    ui,
                    "after_done",
                    &mut self.after_done,
                    &AfterDone::ALL,
                    |a| a.label().to_owned(),
                );
                ui.label(RichText::new(tr("after.label")).weak());
            });
        });
    }

    fn output_dir_picker(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(tr("output.label")).weak());
        let current = self.settings.output_dir.as_ref().map_or_else(
            || tr("output.beside_source").to_owned(),
            |d| d.display().to_string(),
        );
        ui.label(RichText::new(current).monospace());
        if widgets::button(
            ui,
            Some(icons::FOLDER),
            tr("action.change"),
            Kind::Ghost,
            Size::Small,
            true,
        )
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
                true,
            )
            .on_hover_text(tr("hint.output_reset"))
            .clicked()
        {
            self.settings.output_dir = None;
        }
    }

    // ── Sidebar ─────────────────────────────────────────────────────────

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for category in Category::ALL {
                    let tools = category.tools();
                    let header = RichText::new(category.label()).strong();
                    egui::CollapsingHeader::new(header)
                        .id_salt(category.label())
                        .default_open(category == Category::Video || tools.contains(&self.tool))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for tool in tools {
                                let reason = self.unavailable_reason(tool);
                                let hover = match &reason {
                                    Some(r) => {
                                        format!("{}\n\n{} {r}", tool.description(), icons::WARNING)
                                    }
                                    None => tool.description().to_owned(),
                                };
                                if widgets::nav_item(
                                    ui,
                                    tool.label(),
                                    self.tool == tool,
                                    reason.is_some(),
                                )
                                .on_hover_text(hover)
                                .clicked()
                                {
                                    self.select_tool(tool);
                                }
                            }
                        });
                }
            });
    }

    // ── Task editor ─────────────────────────────────────────────────────

    fn task_editor(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.label(RichText::new(self.tool_title()).heading().strong());
        ui.label(RichText::new(self.tool.description()).color(p.text_weak));
        if let Some(reason) = self.unavailable_reason(self.tool) {
            ui.add_space(4.0);
            widgets::chip(ui, icons::WARNING, &reason, p.warning);
        }
        ui.add_space(10.0);
        card(ui, |ui| match self.tool.input_kind() {
            InputKind::Url => self.url_input(ui),
            InputKind::Device => {
                ui.horizontal(|ui| {
                    ui.label(tr("input.device"));
                    ui.add(TextEdit::singleline(&mut self.settings.cd_device).desired_width(220.0));
                });
            }
            InputKind::Capture => {
                ui.label(tr("input.capture"));
                if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    ui.add_space(4.0);
                    ui.colored_label(
                        p.warning,
                        icons::label(icons::WARNING, tr("warning.wayland_capture")),
                    );
                }
            }
            _ => self.file_list(ui),
        });
        ui.add_space(10.0);
        card(ui, |ui| {
            egui::Grid::new("options")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .show(ui, |ui| {
                    self.options_ui(ui);
                });
            if let Tool::ToVideo(format) = self.tool {
                ui.add_space(4.0);
                self.advanced_video(ui, format);
                self.edit_panel(ui, true);
            } else if let Tool::ToAudio(format) = self.tool {
                ui.add_space(4.0);
                self.advanced_audio(ui, !format.is_lossless());
                self.edit_panel(ui, false);
            }
        });
        ui.add_space(8.0);
    }

    /// The editor's sticky footer: the main action stays visible while options scroll.
    fn action_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (icon, label) = self.primary_action();
            if widgets::button(
                ui,
                Some(icon),
                label,
                Kind::Primary,
                Size::Regular,
                self.can_enqueue(),
            )
            .on_hover_text(ui.ctx().format_shortcut(&ENQUEUE))
            .clicked()
            {
                self.enqueue();
            }
            let count = self.files.len();
            if count > 0 {
                ui.label(RichText::new(trf("files.count", &[("count", &count)])).weak());
            }
        });
    }

    fn primary_action(&self) -> (&'static str, &'static str) {
        match self.tool {
            Tool::Record => (icons::RECORD, tr("action.start_recording")),
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

    fn url_input(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(tr("input.url"));
            ui.add(
                TextEdit::singleline(&mut self.url)
                    .hint_text("https://…")
                    .desired_width(f32::INFINITY),
            );
        });
        ui.label(RichText::new(tr("hint.download_rights")).weak());
    }

    fn file_list(&mut self, ui: &mut egui::Ui) {
        let kind = self.tool.input_kind();
        ui.horizontal_wrapped(|ui| {
            let (icon, label) = if kind.picks_folders() {
                (icons::FOLDER_PLUS, tr("action.add_folders"))
            } else {
                (icons::FILE_PLUS, tr("action.add_files"))
            };
            if widgets::button(ui, Some(icon), label, Kind::Secondary, Size::Regular, true)
                .on_hover_text(ui.ctx().format_shortcut(&ADD_FILES))
                .clicked()
            {
                self.pick_files();
            }
            if kind == InputKind::Disc
                && widgets::button(
                    ui,
                    Some(icons::DISC),
                    tr("action.add_iso"),
                    Kind::Secondary,
                    Size::Regular,
                    true,
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
                    tr("action.add_folder_scan"),
                    Kind::Secondary,
                    Size::Regular,
                    true,
                )
                .clicked()
            {
                self.add_folder();
            }
            if widgets::button(
                ui,
                Some(icons::BROOM),
                tr("action.clear"),
                Kind::Ghost,
                Size::Regular,
                !self.files.is_empty(),
            )
            .clicked()
            {
                self.files.clear();
            }
        });
        ui.add_space(6.0);

        let p = theme::palette(ui);
        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        let glow = ui
            .ctx()
            .animate_bool_with_time(Id::unique("drop_glow"), hovering, MOTION);
        let stroke = Stroke::new(1.0 + glow, p.border.lerp_to_gamma(p.accent, glow));
        let ordered = !matches!(self.tool.arity(), InputArity::EachFile);
        let probeable = kind == InputKind::Media && self.toolbox.get(External::Ffmpeg).is_some();
        let mut action = None;
        egui::Frame::new()
            .fill(p.sunken.lerp_to_gamma(p.accent, glow * 0.08))
            .stroke(stroke)
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if self.files.is_empty() {
                    widgets::empty_state(ui, icons::FILE_PLUS, tr("files.empty"), DROP_ZONE_HEIGHT);
                    return;
                }
                ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (i, path) in self.files.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let small = |ui: &mut egui::Ui, icon, enabled| {
                                widgets::button(
                                    ui,
                                    Some(icon),
                                    "",
                                    Kind::Ghost,
                                    Size::Small,
                                    enabled,
                                )
                            };
                            if small(ui, icons::X, true)
                                .on_hover_text(tr("action.remove"))
                                .clicked()
                            {
                                action = Some(FileAction::Remove(i));
                            }
                            if ordered {
                                if small(ui, icons::CARET_UP, i > 0)
                                    .on_hover_text(tr("action.move_up"))
                                    .clicked()
                                {
                                    action = Some(FileAction::Swap(i, i - 1));
                                }
                                if small(ui, icons::CARET_DOWN, i + 1 < self.files.len())
                                    .on_hover_text(tr("action.move_down"))
                                    .clicked()
                                {
                                    action = Some(FileAction::Swap(i, i + 1));
                                }
                            }
                            if probeable
                                && small(ui, icons::INFO, true)
                                    .on_hover_text(tr("action.media_info"))
                                    .clicked()
                            {
                                action = Some(FileAction::Info(i));
                            }
                            ui.label(file_name(path))
                                .on_hover_text(path.display().to_string());
                        });
                    }
                });
            });
        match action {
            Some(FileAction::Remove(i)) => {
                self.files.remove(i);
            }
            Some(FileAction::Swap(a, b)) => self.files.swap(a, b),
            Some(FileAction::Info(i)) => self.show_media_info(i),
            None => {}
        }
        ui.add_space(4.0);
        let hint = match self.tool.arity() {
            InputArity::Exactly(2) if self.tool == Tool::Mux => tr("hint.mux_order"),
            InputArity::Exactly(2) => tr("hint.mix_order"),
            InputArity::AllAtLeast(_) => tr("hint.order_matters"),
            _ => tr("hint.drop_files"),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(trf("files.count", &[("count", &self.files.len())])).weak());
            ui.label(RichText::new("·").weak());
            ui.label(RichText::new(hint).weak());
        });
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

    fn options_ui(&mut self, ui: &mut egui::Ui) {
        let tool = self.tool;
        let opts = &mut self.settings.options;
        match tool {
            Tool::ToVideo(_) | Tool::JoinVideo | Tool::VideoMix => {
                quality_row(ui, &mut opts.quality);
                resolution_row(ui, &mut opts.resolution);
            }
            Tool::ToAudio(_) | Tool::Rotate | Tool::Speed | Tool::Delogo | Tool::PdfCompress => {
                quality_row(ui, &mut opts.quality);
            }
            Tool::ToImage(_) | Tool::ImagesToPdf => {
                quality_row(ui, &mut opts.quality);
                image_rows(ui, opts);
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
            Tool::Trim => {
                trim_rows(ui, &mut opts.edit);
                row(ui, tr("option.mode"), |ui| {
                    ui.checkbox(&mut opts.stream_copy, tr("option.stream_copy"))
                        .on_hover_text(tr("hint.stream_copy"));
                });
                if !opts.stream_copy {
                    quality_row(ui, &mut opts.quality);
                    resolution_row(ui, &mut opts.resolution);
                }
            }
            Tool::Split => {
                row(ui, tr("option.split_mode"), |ui| {
                    ui.radio_value(&mut opts.split_by_parts, false, tr("option.split_by_time"));
                    ui.radio_value(&mut opts.split_by_parts, true, tr("option.split_by_parts"));
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
                row(ui, tr("option.target_size"), |ui| {
                    ui.add(
                        DragValue::new(&mut opts.target_size_mb)
                            .range(options::MIN_TARGET_MB..=options::MAX_TARGET_MB)
                            .suffix(" MB"),
                    );
                });
                resolution_row(ui, &mut opts.resolution);
            }
            Tool::ExtractFrames => {
                row(ui, tr("option.interval"), |ui| {
                    ui.add(
                        DragValue::new(&mut opts.frame_interval_s)
                            .range(options::MIN_FRAME_INTERVAL_S..=options::MAX_FRAME_INTERVAL_S)
                            .speed(0.1)
                            .suffix(" s"),
                    );
                });
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
            | Tool::ToEbook(_) => {
                row(ui, "", |ui| {
                    ui.label(RichText::new(tr("option.none")).weak());
                });
            }
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
                row(ui, tr("option.download_kind"), |ui| {
                    combo(
                        ui,
                        "download_kind",
                        &mut opts.download_kind,
                        &DownloadKind::ALL,
                        |k| k.label().to_owned(),
                    );
                });
            }
            Tool::Record => {
                row(ui, tr("option.fps"), |ui| {
                    ui.add(DragValue::new(&mut opts.record_fps).range(1..=options::MAX_RECORD_FPS));
                });
                row(ui, tr("option.audio"), |ui| {
                    ui.checkbox(&mut opts.record_audio, tr("option.record_audio"));
                });
            }
        }
        if tool == Tool::Rotate {
            row(ui, tr("option.rotation"), |ui| {
                combo(ui, "rotation", &mut opts.rotation, &Rotation::ALL, |r| {
                    r.label().to_owned()
                });
            });
        }
        if tool == Tool::Speed {
            row(ui, tr("option.speed"), |ui| {
                ui.add(
                    Slider::new(&mut opts.speed, options::MIN_SPEED..=options::MAX_SPEED)
                        .suffix("×")
                        .step_by(0.05),
                );
            });
        }
        if tool == Tool::VideoMix {
            row(ui, tr("option.layout"), |ui| {
                combo(
                    ui,
                    "mix_layout",
                    &mut opts.mix_layout,
                    &MixLayout::ALL,
                    |l| l.label().to_owned(),
                );
            });
        }
        if tool == Tool::Delogo {
            row(ui, tr("option.logo_area"), |ui| {
                rect_editor(ui, &mut opts.delogo)
            });
        }
    }

    fn advanced_video(&mut self, ui: &mut egui::Ui, format: VideoFormat) {
        let toolbox = &self.toolbox;
        let opts = &mut self.settings.options;
        egui::CollapsingHeader::new(icons::label(icons::SLIDERS, tr("section.output_settings")))
            .id_salt("adv_video")
            .show(ui, |ui| {
                egui::Grid::new("adv_video_grid")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
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
                                        hw.suffix().is_none_or(|s| {
                                            toolbox.has_encoder(&format!("h264_{s}"))
                                        })
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
                        row(ui, tr("option.fps"), |ui| {
                            let mut choices: Vec<Option<u32>> = vec![None];
                            choices.extend(options::FRAME_RATES.map(Some));
                            combo(ui, "fps", &mut opts.video.fps, &choices, |f| {
                                f.map_or_else(
                                    || tr("option.original").to_owned(),
                                    |n| n.to_string(),
                                )
                            });
                        });
                    });
                if !format.is_animation() {
                    audio_grid(ui, &mut opts.audio, true);
                }
            });
    }

    fn advanced_audio(&mut self, ui: &mut egui::Ui, lossy: bool) {
        let opts = &mut self.settings.options;
        egui::CollapsingHeader::new(icons::label(icons::SLIDERS, tr("section.output_settings")))
            .id_salt("adv_audio")
            .show(ui, |ui| audio_grid(ui, &mut opts.audio, lossy));
    }

    fn edit_panel(&mut self, ui: &mut egui::Ui, video: bool) {
        let edit = &mut self.settings.options.edit;
        egui::CollapsingHeader::new(icons::label(icons::SCISSORS, tr("section.edit")))
            .id_salt("edit_panel")
            .show(ui, |ui| {
                ui.label(RichText::new(tr("hint.edit_applies")).weak());
                egui::Grid::new("edit_grid")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        trim_rows(ui, edit);
                        if !video {
                            return;
                        }
                        row(ui, tr("option.crop"), |ui| {
                            let mut on = edit.crop.is_some();
                            ui.checkbox(&mut on, "");
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
                                let mut corner =
                                    edit.watermark_corner.unwrap_or(Corner::BottomRight);
                                combo(ui, "wm_corner", &mut corner, &Corner::ALL, |c| {
                                    c.label().to_owned()
                                });
                                edit.watermark_corner = Some(corner);
                            }
                        });
                    });
            });
    }

    // ── Queue ───────────────────────────────────────────────────────────

    fn queue_panel(&mut self, ui: &mut egui::Ui) {
        let counts = self.queue.counts();
        let p = theme::palette(ui);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("queue.title")).strong().size(16.0));
            let total = self.queue.jobs().len();
            ui.label(
                RichText::new(trf(
                    "queue.summary",
                    &[("done", &counts.done), ("total", &total)],
                ))
                .weak(),
            );
            if !self.queue.is_running && counts.queued > 0 {
                widgets::chip(ui, icons::PAUSE, tr("queue.paused"), p.warning);
            }
        });
        ui.add_space(4.0);
        if self.queue.jobs().is_empty() {
            widgets::empty_state(
                ui,
                icons::LIST_PLUS,
                tr("queue.empty"),
                ui.available_height().max(80.0),
            );
            return;
        }
        let mut cancel = None;
        let mut remove = None;
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                egui::Grid::new("queue")
                    .striped(true)
                    .num_columns(4)
                    .spacing([18.0, 8.0])
                    .min_row_height(30.0)
                    .show(ui, |ui| {
                        for job in self.queue.jobs() {
                            ui.label(RichText::new(job.spec.tool.label()).strong());
                            ui.label(job_source(&job.spec))
                                .on_hover_text(job.spec.output.display().to_string());
                            status_cell(ui, job);
                            ui.horizontal(|ui| match job.state {
                                JobState::Queued | JobState::Running { .. } => {
                                    let label = if job.spec.tool == Tool::Record
                                        && job.state != JobState::Queued
                                    {
                                        tr("action.stop_recording")
                                    } else {
                                        tr("action.cancel")
                                    };
                                    if widgets::button(
                                        ui,
                                        Some(icons::X),
                                        label,
                                        Kind::Ghost,
                                        Size::Small,
                                        true,
                                    )
                                    .clicked()
                                    {
                                        cancel = Some(job.id);
                                    }
                                }
                                JobState::Done { .. } => {
                                    if job.spec.output.is_file()
                                        && widgets::button(
                                            ui,
                                            Some(icons::ARROW_SQUARE_OUT),
                                            tr("action.open"),
                                            Kind::Ghost,
                                            Size::Small,
                                            true,
                                        )
                                        .clicked()
                                    {
                                        open_path(&job.spec.output);
                                    }
                                    if widgets::button(
                                        ui,
                                        Some(icons::FOLDER),
                                        tr("action.open_folder"),
                                        Kind::Ghost,
                                        Size::Small,
                                        true,
                                    )
                                    .clicked()
                                    {
                                        reveal(&job.spec.output);
                                    }
                                    if widgets::button(
                                        ui,
                                        Some(icons::X),
                                        "",
                                        Kind::Ghost,
                                        Size::Small,
                                        true,
                                    )
                                    .on_hover_text(tr("action.remove"))
                                    .clicked()
                                    {
                                        remove = Some(job.id);
                                    }
                                }
                                _ => {
                                    if widgets::button(
                                        ui,
                                        Some(icons::X),
                                        tr("action.remove"),
                                        Kind::Ghost,
                                        Size::Small,
                                        true,
                                    )
                                    .clicked()
                                    {
                                        remove = Some(job.id);
                                    }
                                }
                            });
                            ui.end_row();
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

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut theme_changed = false;
        egui::Window::new(icons::label(icons::GEAR, tr("settings.title")))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("settings")
                    .num_columns(2)
                    .spacing([16.0, 12.0])
                    .show(ui, |ui| {
                        row(ui, tr("settings.language"), |ui| self.language_picker(ui));
                        row(ui, tr("settings.theme"), |ui| {
                            let before = self.settings.theme;
                            combo(
                                ui,
                                "theme",
                                &mut self.settings.theme,
                                &ThemeChoice::ALL,
                                |t| t.label().to_owned(),
                            );
                            theme_changed |= before != self.settings.theme;
                        });
                        row(ui, tr("settings.motion"), |ui| {
                            theme_changed |= ui
                                .checkbox(
                                    &mut self.settings.reduce_motion,
                                    tr("settings.reduce_motion"),
                                )
                                .changed();
                        });
                        row(ui, tr("settings.parallel"), |ui| {
                            ui.add(
                                DragValue::new(&mut self.settings.parallel_jobs)
                                    .range(1..=MAX_PARALLEL_JOBS),
                            );
                        });
                        row(ui, tr("settings.auto_start"), |ui| {
                            ui.checkbox(&mut self.settings.auto_start, "");
                        });
                        row(ui, tr("settings.ffmpeg"), |ui| {
                            ui.vertical(|ui| self.ffmpeg_setting(ui));
                        });
                        row(ui, tr("settings.programs"), |ui| {
                            ui.vertical(|ui| self.programs_list(ui));
                        });
                    });
            });
        self.show_settings = open;
        if theme_changed {
            self.apply_theme(ctx);
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
                ui.selectable_value(&mut current, None, label(&None));
                for (code, _) in &names {
                    let choice = Some((*code).to_owned());
                    let text = label(&choice);
                    ui.selectable_value(&mut current, choice, text);
                }
            });
        if current != self.settings.language {
            i18n::set_language(current.as_deref());
            self.settings.language = current;
        }
    }

    fn programs_list(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let mut seen = HashSet::new();
        for ext in External::ALL.into_iter().filter(|e| *e != External::Ffmpeg) {
            if !seen.insert(ext.name()) {
                continue;
            }
            if self.toolbox.get(ext).is_some() {
                widgets::chip(ui, icons::CHECK_CIRCLE, ext.name(), p.success);
            } else {
                widgets::chip(ui, icons::X_CIRCLE, ext.name(), p.text_weak);
            }
        }
        if widgets::button(
            ui,
            Some(icons::ARROW_CLOCKWISE),
            tr("action.rescan"),
            Kind::Secondary,
            Size::Small,
            true,
        )
        .clicked()
        {
            self.toolbox = Toolbox::detect(self.settings.ffmpeg_path.as_deref());
        }
    }

    fn ffmpeg_setting(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        match self.toolbox.get(External::Ffmpeg) {
            Some(path) => {
                ui.label(RichText::new(path.display().to_string()).monospace());
            }
            None => {
                widgets::chip(ui, icons::X_CIRCLE, tr("settings.ffmpeg_missing"), p.danger);
            }
        }
        ui.horizontal(|ui| {
            if widgets::button(
                ui,
                Some(icons::FOLDER_OPEN),
                tr("action.browse"),
                Kind::Secondary,
                Size::Small,
                true,
            )
            .clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                self.settings.ffmpeg_path = Some(path);
                self.toolbox = Toolbox::detect(self.settings.ffmpeg_path.as_deref());
            }
            if self.settings.ffmpeg_path.is_some()
                && widgets::button(
                    ui,
                    None,
                    tr("action.auto_detect"),
                    Kind::Ghost,
                    Size::Small,
                    true,
                )
                .clicked()
            {
                self.settings.ffmpeg_path = None;
                self.toolbox = Toolbox::detect(None);
            }
        });
    }

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

enum FileAction {
    Remove(usize),
    Swap(usize, usize),
    Info(usize),
}

/// A rounded surface that groups related controls.
fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    let p = theme::palette(ui);
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(theme::RADIUS + 2))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
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
fn row(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(label).color(theme::palette(ui).text_weak));
    ui.horizontal(content);
    ui.end_row();
}

fn quality_row(ui: &mut egui::Ui, q: &mut Quality) {
    row(ui, tr("option.quality"), |ui| {
        combo(ui, "quality", q, &Quality::ALL, |v| v.label().to_owned())
    });
}

fn resolution_row(ui: &mut egui::Ui, r: &mut Resolution) {
    row(ui, tr("option.resolution"), |ui| {
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

fn trim_rows(ui: &mut egui::Ui, edit: &mut Edit) {
    for (label, text) in [
        (tr("option.start"), &mut edit.trim_start),
        (tr("option.end"), &mut edit.trim_end),
    ] {
        row(ui, label, |ui| {
            ui.add(
                TextEdit::singleline(text)
                    .hint_text(tr("hint.timestamp"))
                    .desired_width(140.0),
            );
        });
    }
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
    row(ui, tr("option.color"), |ui| {
        ui.checkbox(&mut opts.grayscale, tr("option.grayscale"));
    });
}

fn audio_grid(ui: &mut egui::Ui, audio: &mut options::AudioSettings, lossy: bool) {
    egui::Grid::new("adv_audio_grid")
        .num_columns(2)
        .spacing([16.0, 10.0])
        .show(ui, |ui| {
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
    ui.checkbox(&mut on, "");
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

/// A file picker slot showing the chosen file with a clear button.
fn file_slot(ui: &mut egui::Ui, slot: &mut Option<PathBuf>, exts: &[&str]) {
    if widgets::button(
        ui,
        Some(icons::FOLDER_OPEN),
        tr("action.choose"),
        Kind::Secondary,
        Size::Small,
        true,
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
        if widgets::button(ui, Some(icons::X), "", Kind::Ghost, Size::Small, true)
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
    ComboBox::from_id_salt(id)
        .selected_text(label(value.clone()))
        .show_ui(ui, |ui| {
            for choice in choices {
                ui.selectable_value(value, choice.clone(), label(choice.clone()));
            }
        });
}

fn status_cell(ui: &mut egui::Ui, job: &Job) {
    let p = theme::palette(ui);
    let bar_id = Id::unique(("job_progress", job.id));
    match &job.state {
        JobState::Queued => {
            widgets::chip(ui, icons::LIST_PLUS, tr("status.queued"), p.text_weak);
        }
        JobState::Running { .. } if job.spec.tool == Tool::Record => {
            let text = format!(
                "{} {}",
                tr("status.recording"),
                clock(job.elapsed().unwrap_or_default())
            );
            widgets::chip(ui, icons::RECORD, &text, p.danger);
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        JobState::Running {
            progress: Some(fraction),
        } => {
            let percent = format!("{:.0}%", fraction * 100.0);
            let text = match job.elapsed().and_then(|e| time_left(e, *fraction)) {
                Some(left) => trf(
                    "status.eta",
                    &[("percent", &percent), ("left", &clock(left))],
                ),
                None => percent,
            };
            widgets::progress(ui, bar_id, *fraction, text, PROGRESS_WIDTH);
        }
        JobState::Running { progress: None } => {
            ui.add(
                egui::ProgressBar::new(0.0)
                    .animate(true)
                    .text(tr("status.running"))
                    .desired_width(PROGRESS_WIDTH)
                    .desired_height(20.0)
                    .corner_radius(CornerRadius::same(255))
                    .fill(p.accent),
            );
        }
        JobState::Done { elapsed } => {
            let text = trf(
                "status.done",
                &[("secs", &format!("{:.1}", elapsed.as_secs_f32()))],
            );
            widgets::chip(ui, icons::CHECK_CIRCLE, &text, p.success);
        }
        JobState::Failed(message) => {
            let first_line = message.lines().next().unwrap_or_default();
            widgets::chip(ui, icons::X_CIRCLE, first_line, p.danger).on_hover_text(message);
        }
        JobState::Cancelled => {
            widgets::chip(ui, icons::X, tr("status.cancelled"), p.text_weak);
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
    if let Err(e) = std::process::Command::new(opener()).arg(path).spawn() {
        tracing::warn!(error = %e, "could not open file");
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

/// Powers the computer off (the user opted in and saw a countdown).
fn power_off() {
    let result = if cfg!(windows) {
        std::process::Command::new("shutdown")
            .args(["/s", "/t", "0"])
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("osascript")
            .args(["-e", "tell app \"System Events\" to shut down"])
            .spawn()
    } else {
        std::process::Command::new("systemctl")
            .arg("poweroff")
            .spawn()
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, "could not shut down");
    }
}

impl eframe::App for MuxrApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_input(&ctx);
        self.queue
            .pump(self.settings.parallel_jobs, &self.toolbox, &self.notifier);
        self.check_after_done(&ctx);
        let p = theme::palette(ui);

        let bar = |fill| {
            egui::Frame::new()
                .fill(fill)
                .inner_margin(Margin::symmetric(14, 10))
        };
        egui::Panel::top("toolbar")
            .frame(bar(p.surface).stroke(Stroke::new(1.0, p.border)))
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::left("tools")
            .resizable(true)
            .default_size(SIDEBAR_WIDTH)
            .frame(bar(p.panel))
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::bottom("queue")
            .resizable(true)
            .default_size(QUEUE_HEIGHT)
            .frame(bar(p.surface))
            .show(ui, |ui| self.queue_panel(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, CornerRadius::ZERO, p.panel);
            let p = theme::palette(ui);
            egui::Panel::bottom("editor_actions")
                .frame(
                    egui::Frame::new()
                        .fill(p.panel)
                        .inner_margin(Margin::symmetric(18, 10)),
                )
                .show(ui, |ui| self.action_bar(ui));
            egui::Frame::new()
                .inner_margin(Margin::symmetric(18, 14))
                .show(ui, |ui| {
                    ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| self.task_editor(ui));
                });
        });
        self.settings_window(&ctx);
        self.media_info_window(&ctx);
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
    fn eta_needs_some_progress() {
        assert_eq!(time_left(Duration::from_secs(10), 0.01), None);
        assert_eq!(
            time_left(Duration::from_secs(10), 0.5),
            Some(Duration::from_secs(10))
        );
        assert_eq!(time_left(Duration::from_secs(10), 1.0), None);
    }
}
