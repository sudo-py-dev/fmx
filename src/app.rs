//! The egui front end: tool sidebar, task editor, and job queue.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{
    self, Align, Button, Color32, ComboBox, DragValue, Key, KeyboardShortcut, Layout, Modifiers,
    ProgressBar, RichText, ScrollArea, Slider, TextEdit,
};

use crate::catalog::{AudioFormat, Category, ImageFormat, InputArity, Tool};
use crate::ffmpeg;
use crate::i18n::{tr, trf};
use crate::imaging::is_supported_image;
use crate::jobs::plan_jobs;
use crate::options::{self, Quality, Resolution, Rotation};
use crate::queue::{JobState, Notifier, Queue};
use crate::settings::{MAX_PARALLEL_JOBS, STORAGE_KEY, Settings};

const ADD_FILES: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const ENQUEUE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
const SIDEBAR_WIDTH: f32 = 210.0;
const QUEUE_HEIGHT: f32 = 280.0;
const PROGRESS_WIDTH: f32 = 180.0;

/// Extensions offered in the media file picker.
const MEDIA_EXTS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "flv", "wmv", "mpg", "mpeg", "ts", "mts", "m2ts",
    "ogv", "3gp", "gif", "vob", "rmvb", "rm", "asf", "mp3", "aac", "m4a", "flac", "wav", "ogg",
    "opus", "wma", "aiff", "aif", "ac3", "amr", "ape", "mka", "wv",
];
const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif", "ico", "tga", "qoi", "pnm", "ppm",
    "pgm", "pbm",
];

enum Notice {
    Info(String),
    Error(String),
}

pub struct MuxrApp {
    settings: Settings,
    queue: Queue,
    ffmpeg: Option<PathBuf>,
    tool: Tool,
    files: Vec<PathBuf>,
    notice: Option<Notice>,
    show_settings: bool,
    notifier: Notifier,
}

impl MuxrApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        let ctx = cc.egui_ctx.clone();
        let ffmpeg = ffmpeg::locate(settings.ffmpeg_path.as_deref());
        if ffmpeg.is_none() {
            tracing::warn!("ffmpeg not found; media tools are disabled");
        }
        Self {
            tool: settings.last_tool,
            queue: Queue::new(settings.auto_start),
            ffmpeg,
            files: Vec::new(),
            notice: None,
            show_settings: false,
            notifier: Arc::new(move || ctx.request_repaint()),
            settings,
        }
    }

    fn select_tool(&mut self, tool: Tool) {
        if tool.takes_images() != self.tool.takes_images() {
            self.files.clear();
        }
        self.tool = tool;
        self.settings.last_tool = tool;
        self.notice = None;
    }

    fn add_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let wants_images = self.tool.takes_images();
        let mut known: HashSet<PathBuf> = self.files.iter().cloned().collect();
        let mut skipped = 0usize;
        for path in paths {
            if !path.is_file() || (wants_images && !is_supported_image(&path)) {
                skipped += 1;
            } else if known.insert(path.clone()) {
                self.files.push(path);
            }
        }
        self.notice =
            (skipped > 0).then(|| Notice::Error(trf("notice.skipped", &[("count", &skipped)])));
    }

    fn pick_files(&mut self) {
        let (name, exts) = if self.tool.takes_images() {
            (tr("filter.images"), IMAGE_EXTS)
        } else {
            (tr("filter.media"), MEDIA_EXTS)
        };
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(name, exts)
            .add_filter(tr("filter.all"), &["*"])
            .pick_files()
        {
            self.add_files(paths);
        }
    }

    fn enqueue(&mut self) {
        let mut claimed: HashSet<PathBuf> = self.queue.claimed_outputs().cloned().collect();
        match plan_jobs(
            self.tool,
            &self.files,
            self.settings.output_dir.as_deref(),
            &self.settings.options,
            &mut claimed,
        ) {
            Ok(specs) => {
                let count = specs.len();
                specs.into_iter().for_each(|s| self.queue.push(s));
                self.files.clear();
                if self.settings.auto_start {
                    self.queue.is_running = true;
                }
                self.notice = Some(Notice::Info(trf("notice.queued", &[("count", &count)])));
            }
            Err(e) => self.notice = Some(Notice::Error(e.to_string())),
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
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ADD_FILES)) {
            self.pick_files();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ENQUEUE)) && self.can_enqueue() {
            self.enqueue();
        }
    }

    fn can_enqueue(&self) -> bool {
        !self.files.is_empty() && (self.ffmpeg.is_some() || !self.tool.needs_ffmpeg())
    }

    // ── Toolbar ─────────────────────────────────────────────────────────

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new(tr("app.name")).strong());
            ui.separator();
            let counts = self.queue.counts();
            let (label, hint) = if self.queue.is_running {
                (tr("action.pause"), tr("hint.pause"))
            } else {
                (tr("action.start"), tr("hint.start"))
            };
            if ui.button(label).on_hover_text(hint).clicked() {
                self.queue.is_running = !self.queue.is_running;
            }
            if ui
                .add_enabled(
                    counts.running + counts.queued > 0,
                    Button::new(tr("action.stop_all")),
                )
                .on_hover_text(tr("hint.stop_all"))
                .clicked()
            {
                self.queue.cancel_all();
            }
            if ui
                .add_enabled(counts.failed > 0, Button::new(tr("action.retry")))
                .clicked()
            {
                self.queue.retry_failed();
            }
            if ui
                .add_enabled(
                    counts.done + counts.failed > 0,
                    Button::new(tr("action.clear_finished")),
                )
                .clicked()
            {
                self.queue.clear_finished();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(tr("action.settings")).clicked() {
                    self.show_settings = !self.show_settings;
                }
                self.output_dir_picker(ui);
            });
        });
    }

    fn output_dir_picker(&mut self, ui: &mut egui::Ui) {
        if self.settings.output_dir.is_some()
            && ui
                .button(tr("action.reset"))
                .on_hover_text(tr("hint.output_reset"))
                .clicked()
        {
            self.settings.output_dir = None;
        }
        if ui.button(tr("action.change")).clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            self.settings.output_dir = Some(dir);
        }
        let current = self.settings.output_dir.as_ref().map_or_else(
            || tr("output.beside_source").to_owned(),
            |d| d.display().to_string(),
        );
        ui.label(RichText::new(current).monospace())
            .on_hover_text(tr("output.label"));
        ui.label(tr("output.label"));
    }

    // ── Sidebar ─────────────────────────────────────────────────────────

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for category in Category::ALL {
                    egui::CollapsingHeader::new(RichText::new(category.label()).strong())
                        .default_open(matches!(category, Category::Video | Category::Utilities))
                        .show(ui, |ui| {
                            for tool in category.tools() {
                                let selected = self.tool == tool;
                                if ui
                                    .selectable_label(selected, tool.label())
                                    .on_hover_text(tool.description())
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
        ui.heading(self.tool_title());
        ui.label(RichText::new(self.tool.description()).weak());
        if self.tool.needs_ffmpeg() && self.ffmpeg.is_none() {
            ui.colored_label(ui.visuals().warn_fg_color, tr("warning.no_ffmpeg"));
        }
        ui.add_space(6.0);
        self.file_list(ui);
        ui.add_space(6.0);
        egui::Grid::new("options")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                self.options_ui(ui);
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let primary = Button::new(RichText::new(tr("action.enqueue")).strong())
                .fill(ui.visuals().selection.bg_fill)
                .min_size(egui::vec2(140.0, 30.0));
            if ui
                .add_enabled(self.can_enqueue(), primary)
                .on_hover_text(ui.ctx().format_shortcut(&ENQUEUE))
                .clicked()
            {
                self.enqueue();
            }
            match &self.notice {
                Some(Notice::Info(text)) => {
                    ui.label(text);
                }
                Some(Notice::Error(text)) => {
                    ui.colored_label(ui.visuals().error_fg_color, text);
                }
                None => {}
            }
        });
    }

    fn tool_title(&self) -> String {
        match self.tool {
            Tool::ToVideo(_) | Tool::ToAudio(_) | Tool::ToImage(_) => {
                trf("title.convert_to", &[("format", &self.tool.label())])
            }
            tool => tool.label().to_owned(),
        }
    }

    fn file_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button(tr("action.add_files"))
                .on_hover_text(ui.ctx().format_shortcut(&ADD_FILES))
                .clicked()
            {
                self.pick_files();
            }
            if ui
                .add_enabled(!self.files.is_empty(), Button::new(tr("action.clear")))
                .clicked()
            {
                self.files.clear();
            }
            let hint = match self.tool.arity() {
                InputArity::Exactly(2) => tr("hint.mux_order"),
                InputArity::AllAtLeast(_) => tr("hint.order_matters"),
                _ => tr("hint.drop_files"),
            };
            ui.label(RichText::new(hint).weak());
        });

        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        let stroke = if hovering {
            ui.visuals().selection.stroke
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke
        };
        egui::Frame::group(ui.style())
            .stroke(stroke)
            .show(ui, |ui| {
                ui.set_min_height(120.0);
                ui.set_width(ui.available_width());
                if self.files.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new(tr("files.empty")).weak())
                    });
                    return;
                }
                let ordered = !matches!(self.tool.arity(), InputArity::EachFile);
                let mut action = None;
                ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for (i, path) in self.files.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("✕")
                                .on_hover_text(tr("action.remove"))
                                .clicked()
                            {
                                action = Some(FileAction::Remove(i));
                            }
                            if ordered {
                                if ui
                                    .add_enabled(i > 0, Button::new("▲").small())
                                    .on_hover_text(tr("action.move_up"))
                                    .clicked()
                                {
                                    action = Some(FileAction::Swap(i, i - 1));
                                }
                                if ui
                                    .add_enabled(i + 1 < self.files.len(), Button::new("▼").small())
                                    .on_hover_text(tr("action.move_down"))
                                    .clicked()
                                {
                                    action = Some(FileAction::Swap(i, i + 1));
                                }
                            }
                            ui.label(file_name(path))
                                .on_hover_text(path.display().to_string());
                        });
                    }
                });
                match action {
                    Some(FileAction::Remove(i)) => {
                        self.files.remove(i);
                    }
                    Some(FileAction::Swap(a, b)) => self.files.swap(a, b),
                    None => {}
                }
            });
        ui.label(RichText::new(trf("files.count", &[("count", &self.files.len())])).weak());
    }

    fn options_ui(&mut self, ui: &mut egui::Ui) {
        let opts = &mut self.settings.options;
        let quality = |ui: &mut egui::Ui, q: &mut Quality| {
            ui.label(tr("option.quality"));
            combo(ui, "quality", q, &Quality::ALL, |v| v.label().to_owned());
            ui.end_row();
        };
        let resolution = |ui: &mut egui::Ui, r: &mut Resolution| {
            ui.label(tr("option.resolution"));
            combo(ui, "resolution", r, &Resolution::ALL, |v| v.label());
            ui.end_row();
        };
        let image_size = |ui: &mut egui::Ui, side: &mut Option<u32>, gray: &mut bool| {
            ui.label(tr("option.max_side"));
            ui.horizontal(|ui| {
                let mut limit = side.is_some();
                ui.checkbox(&mut limit, tr("option.limit"));
                *side = limit.then(|| side.unwrap_or(1920));
                if let Some(value) = side {
                    ui.add(
                        DragValue::new(value)
                            .range(options::MIN_IMAGE_SIDE..=options::MAX_IMAGE_SIDE)
                            .suffix(" px"),
                    );
                }
            });
            ui.end_row();
            ui.label(tr("option.color"));
            ui.checkbox(gray, tr("option.grayscale"));
            ui.end_row();
        };

        match self.tool {
            Tool::ToVideo(_) | Tool::JoinVideo => {
                quality(ui, &mut opts.quality);
                resolution(ui, &mut opts.resolution);
            }
            Tool::ToAudio(_) | Tool::Rotate | Tool::Speed => quality(ui, &mut opts.quality),
            Tool::ToImage(_) | Tool::ImagesToPdf => {
                quality(ui, &mut opts.quality);
                image_size(ui, &mut opts.image_max_side, &mut opts.grayscale);
            }
            Tool::JoinAudio => {
                quality(ui, &mut opts.quality);
                ui.label(tr("option.output_format"));
                combo(
                    ui,
                    "join_audio",
                    &mut opts.join_audio_format,
                    &AudioFormat::ALL,
                    |f| f.label().to_owned(),
                );
                ui.end_row();
            }
            Tool::Trim => {
                for (label, text) in [
                    (tr("option.start"), &mut opts.trim_start),
                    (tr("option.end"), &mut opts.trim_end),
                ] {
                    ui.label(label);
                    ui.add(
                        TextEdit::singleline(text)
                            .hint_text(tr("hint.timestamp"))
                            .desired_width(120.0),
                    );
                    ui.end_row();
                }
                ui.label(tr("option.mode"));
                ui.checkbox(&mut opts.stream_copy, tr("option.stream_copy"))
                    .on_hover_text(tr("hint.stream_copy"));
                ui.end_row();
                if !opts.stream_copy {
                    quality(ui, &mut opts.quality);
                    resolution(ui, &mut opts.resolution);
                }
            }
            Tool::Compress => {
                ui.label(tr("option.target_size"));
                ui.add(
                    DragValue::new(&mut opts.target_size_mb)
                        .range(options::MIN_TARGET_MB..=options::MAX_TARGET_MB)
                        .suffix(" MB"),
                );
                ui.end_row();
                resolution(ui, &mut opts.resolution);
            }
            Tool::ExtractFrames => {
                ui.label(tr("option.interval"));
                ui.add(
                    DragValue::new(&mut opts.frame_interval_s)
                        .range(options::MIN_FRAME_INTERVAL_S..=options::MAX_FRAME_INTERVAL_S)
                        .speed(0.1)
                        .suffix(" s"),
                );
                ui.end_row();
                ui.label(tr("option.output_format"));
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
                ui.end_row();
            }
            Tool::Mux | Tool::RemoveAudio | Tool::Normalize => {
                ui.label(RichText::new(tr("option.none")).weak());
                ui.end_row();
            }
        }
        if self.tool == Tool::Rotate {
            ui.label(tr("option.rotation"));
            combo(ui, "rotation", &mut opts.rotation, &Rotation::ALL, |r| {
                r.label().to_owned()
            });
            ui.end_row();
        }
        if self.tool == Tool::Speed {
            ui.label(tr("option.speed"));
            ui.add(
                Slider::new(&mut opts.speed, options::MIN_SPEED..=options::MAX_SPEED)
                    .suffix("×")
                    .step_by(0.05),
            );
            ui.end_row();
        }
    }

    // ── Queue ───────────────────────────────────────────────────────────

    fn queue_panel(&mut self, ui: &mut egui::Ui) {
        let counts = self.queue.counts();
        ui.horizontal(|ui| {
            ui.strong(tr("queue.title"));
            let total = self.queue.jobs().len();
            ui.label(
                RichText::new(trf(
                    "queue.summary",
                    &[("done", &counts.done), ("total", &total)],
                ))
                .weak(),
            );
            if !self.queue.is_running && counts.queued > 0 {
                ui.colored_label(ui.visuals().warn_fg_color, tr("queue.paused"));
            }
        });
        ui.separator();
        if self.queue.jobs().is_empty() {
            ui.centered_and_justified(|ui| ui.label(RichText::new(tr("queue.empty")).weak()));
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
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for job in self.queue.jobs() {
                            ui.label(job.spec.tool.label());
                            let first = job
                                .spec
                                .inputs
                                .first()
                                .map(|p| file_name(p))
                                .unwrap_or_default();
                            let source = match job.spec.inputs.len() {
                                0 | 1 => first,
                                n => trf(
                                    "queue.more_files",
                                    &[("name", &first), ("count", &(n - 1))],
                                ),
                            };
                            ui.label(source)
                                .on_hover_text(job.spec.output.display().to_string());
                            status_cell(ui, &job.state);
                            ui.horizontal(|ui| match job.state {
                                JobState::Queued | JobState::Running { .. } => {
                                    if ui.small_button(tr("action.cancel")).clicked() {
                                        cancel = Some(job.id);
                                    }
                                }
                                JobState::Done { .. } => {
                                    if ui.small_button(tr("action.open_folder")).clicked() {
                                        reveal(&job.spec.output);
                                    }
                                    if ui.small_button(tr("action.remove")).clicked() {
                                        remove = Some(job.id);
                                    }
                                }
                                _ => {
                                    if ui.small_button(tr("action.remove")).clicked() {
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

    // ── Settings window ─────────────────────────────────────────────────

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new(tr("settings.title"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("settings")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr("settings.parallel"));
                        ui.add(
                            DragValue::new(&mut self.settings.parallel_jobs)
                                .range(1..=MAX_PARALLEL_JOBS),
                        );
                        ui.end_row();
                        ui.label(tr("settings.auto_start"));
                        ui.checkbox(&mut self.settings.auto_start, "");
                        ui.end_row();
                        ui.label(tr("settings.ffmpeg"));
                        ui.vertical(|ui| self.ffmpeg_setting(ui));
                        ui.end_row();
                    });
            });
        self.show_settings = open;
    }

    fn ffmpeg_setting(&mut self, ui: &mut egui::Ui) {
        match &self.ffmpeg {
            Some(path) => ui.label(RichText::new(path.display().to_string()).monospace()),
            None => ui.colored_label(ui.visuals().error_fg_color, tr("settings.ffmpeg_missing")),
        };
        ui.horizontal(|ui| {
            if ui.button(tr("action.browse")).clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                self.settings.ffmpeg_path = Some(path);
                self.ffmpeg = ffmpeg::locate(self.settings.ffmpeg_path.as_deref());
            }
            if self.settings.ffmpeg_path.is_some() && ui.button(tr("action.auto_detect")).clicked()
            {
                self.settings.ffmpeg_path = None;
                self.ffmpeg = ffmpeg::locate(None);
            }
        });
    }
}

enum FileAction {
    Remove(usize),
    Swap(usize, usize),
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn combo<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    choices: &[T],
    label: impl Fn(T) -> String,
) {
    ComboBox::from_id_salt(id)
        .selected_text(label(*value))
        .show_ui(ui, |ui| {
            for &choice in choices {
                ui.selectable_value(value, choice, label(choice));
            }
        });
}

fn status_cell(ui: &mut egui::Ui, state: &JobState) {
    match state {
        JobState::Queued => {
            ui.label(RichText::new(tr("status.queued")).weak());
        }
        JobState::Running { progress: Some(p) } => {
            ui.add(
                ProgressBar::new(*p)
                    .show_percentage()
                    .desired_width(PROGRESS_WIDTH),
            );
        }
        JobState::Running { progress: None } => {
            ui.add(
                ProgressBar::new(0.0)
                    .animate(true)
                    .text(tr("status.running"))
                    .desired_width(PROGRESS_WIDTH),
            );
        }
        JobState::Done { elapsed } => {
            let secs = format!("{:.1}", elapsed.as_secs_f32());
            ui.colored_label(
                Color32::from_rgb(60, 170, 90),
                trf("status.done", &[("secs", &secs)]),
            );
        }
        JobState::Failed(message) => {
            let first_line = message.lines().next().unwrap_or_default();
            ui.colored_label(
                ui.visuals().error_fg_color,
                trf("status.failed", &[("reason", &first_line)]),
            )
            .on_hover_text(message);
        }
        JobState::Cancelled => {
            ui.label(RichText::new(tr("status.cancelled")).weak());
        }
    }
}

/// Opens the folder containing `path` in the system file manager.
fn reveal(path: &Path) {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(dir).spawn() {
        tracing::warn!(error = %e, "could not open file manager");
    }
}

impl eframe::App for MuxrApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_input(&ctx);
        self.queue.pump(
            self.settings.parallel_jobs,
            self.ffmpeg.as_ref(),
            &self.notifier,
        );

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::left("tools")
            .resizable(true)
            .default_size(SIDEBAR_WIDTH)
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::bottom("queue")
            .resizable(true)
            .default_size(QUEUE_HEIGHT)
            .show(ui, |ui| self.queue_panel(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| self.task_editor(ui));
        });
        self.settings_window(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
    }
}
