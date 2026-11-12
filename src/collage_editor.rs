//! Background-rendered photo preview; only the latest edit is displayed.
use crate::{i18n::tr, options::Options};
use eframe::egui;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

type Request = (u64, Vec<PathBuf>, Options, Option<PathBuf>, u32);
type Snapshot = (Vec<PathBuf>, Options);
type Reply = (u64, Result<image::RgbaImage, String>);

#[derive(Default)]
pub struct CollageEditor {
    worker: Option<(mpsc::Sender<Request>, mpsc::Receiver<Reply>)>,
    last: Option<(Vec<PathBuf>, Options)>,
    version: u64,
    ready: u64,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    selected: Option<usize>,
    initialized: bool,
    dragging: Option<usize>,
    reframe: bool,
    inspector_tab: u8,
    preview_side: u32,
    edited_at: Option<Instant>,
    history_last: Option<Snapshot>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    history_gesture: bool,
}

impl CollageEditor {
    pub fn prepare(&mut self, opts: &mut Options) {
        if !self.initialized {
            if matches!(
                opts.join_layout,
                crate::options::JoinLayout::Vertical | crate::options::JoinLayout::Horizontal
            ) {
                opts.join_layout = crate::options::JoinLayout::Grid2;
            }
            self.reframe = true;
            self.initialized = true;
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        files: &mut Vec<PathBuf>,
        opts: &mut Options,
        ffmpeg: Option<PathBuf>,
    ) {
        if self.worker.is_none() {
            let (tx, requests) = mpsc::channel::<Request>();
            let (results, rx) = mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let mut photos = HashMap::new();
                while let Ok(mut request) = requests.recv() {
                    while let Ok(latest) = requests.try_recv() {
                        request = latest;
                    }
                    let (version, files, opts, ffmpeg, side) = request;
                    let result = (|| {
                        // Reordering or adding a photo preserves already decoded sources.
                        let active: HashSet<_> = files.iter().collect();
                        photos.retain(|path, _| active.contains(path));
                        for path in &files {
                            if !photos.contains_key(path) {
                                photos.insert(
                                    path.clone(),
                                    crate::imaging::collage_photo(path, ffmpeg.as_deref())?,
                                );
                            }
                        }
                        let pictures: Vec<_> = files.iter().map(|path| &photos[path]).collect();
                        crate::imaging::collage_preview_sized(
                            &pictures,
                            &files,
                            &opts,
                            ffmpeg.as_deref(),
                            side,
                        )
                    })()
                    .map_err(|e: crate::imaging::ImageError| e.to_string());
                    if results.send((version, result)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
            self.worker = Some((tx, rx));
        }
        let changed = self
            .last
            .as_ref()
            .is_none_or(|(paths, options)| paths.as_slice() != files || options != opts);
        let busy = ui.input(|input| {
            input.pointer.any_down() || input.smooth_scroll_delta != egui::Vec2::ZERO
        });
        self.record_history(files, opts, busy);
        if changed {
            self.edited_at = Some(Instant::now());
        }
        let settling = self
            .edited_at
            .is_some_and(|time| time.elapsed() < Duration::from_millis(140));
        let side = if busy || settling { 320 } else { 640 };
        if settling {
            ui.ctx().request_repaint_after(Duration::from_millis(150));
        }
        if changed || self.preview_side != side {
            self.preview_side = side;
            if self
                .last
                .as_ref()
                .is_some_and(|(paths, _)| paths.as_slice() != files)
            {
                self.selected = None;
                self.dragging = None;
            }
            self.version += 1;
            self.last = Some((files.to_vec(), opts.clone()));
            self.error = None;
            self.worker
                .as_ref()
                .unwrap()
                .0
                .send((self.version, files.to_vec(), opts.clone(), ffmpeg, side))
                .ok();
        }
        while let Ok((version, result)) = self.worker.as_ref().unwrap().1.try_recv() {
            // Show completed intermediate frames while the next edit renders.
            // Discarding every older frame made continuous dragging appear frozen.
            if version < self.ready {
                continue;
            }
            self.ready = version;
            match result {
                Ok(image) => {
                    let pixels = egui::ColorImage::from_rgba_unmultiplied(
                        [image.width() as usize, image.height() as usize],
                        image.as_raw(),
                    );
                    if let Some(texture) = &mut self.texture {
                        texture.set(pixels, egui::TextureOptions::LINEAR);
                    } else {
                        self.texture = Some(ui.ctx().load_texture(
                            "collage_live",
                            pixels,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                Err(error) if version == self.version => self.error = Some(error),
                Err(_) => {}
            }
        }
        let undo_shortcut = !ui.ctx().egui_wants_keyboard_input()
            && ui.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::Z,
                ))
            });
        let redo_shortcut = !ui.ctx().egui_wants_keyboard_input()
            && ui.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
                    egui::Key::Z,
                ))
            });
        let mut undo = undo_shortcut;
        let mut redo = redo_shortcut;
        ui.horizontal_wrapped(|ui| {
            use crate::{
                icons,
                widgets::{self, Kind, Size},
            };
            undo |= ui
                .add_enabled_ui(!self.undo.is_empty(), |ui| {
                    widgets::icon_button(
                        ui,
                        icons::ARROW_COUNTER_CLOCKWISE,
                        tr("collage.undo"),
                        Kind::Ghost,
                        Size::Small,
                    )
                    .clicked()
                })
                .inner;
            redo |= ui
                .add_enabled_ui(!self.redo.is_empty(), |ui| {
                    widgets::icon_button(
                        ui,
                        icons::ARROW_CLOCKWISE,
                        tr("collage.redo"),
                        Kind::Ghost,
                        Size::Small,
                    )
                    .clicked()
                })
                .inner;
            ui.selectable_value(&mut self.reframe, true, tr("collage.reframe"));
            ui.selectable_value(&mut self.reframe, false, tr("collage.reorder"));
            if self.ready != self.version {
                ui.spinner();
            }
            ui.weak(format!("{} × {} px", opts.collage_size, opts.collage_size));
        });
        if undo {
            self.restore_history(files, opts, false);
        }
        if redo {
            self.restore_history(files, opts, true);
        }
        if let Some(index) = self.selected.filter(|&i| i < files.len()) {
            ui.horizontal_wrapped(|ui| {
                use crate::{
                    icons,
                    widgets::{self, Kind, Size},
                };
                let mut edit = opts
                    .collage_photos
                    .get(&files[index])
                    .copied()
                    .unwrap_or_default();
                let before = edit;
                if widgets::icon_button(
                    ui,
                    icons::ARROW_COUNTER_CLOCKWISE,
                    tr("collage.rotate_left"),
                    Kind::Secondary,
                    Size::Small,
                )
                .clicked()
                {
                    edit.turns = (edit.turns + 3) % 4;
                }
                if widgets::icon_button(
                    ui,
                    icons::ARROW_CLOCKWISE,
                    tr("collage.rotate"),
                    Kind::Secondary,
                    Size::Small,
                )
                .clicked()
                {
                    edit.turns = (edit.turns + 1) % 4;
                }
                ui.add(
                    egui::DragValue::new(&mut edit.angle)
                        .range(-180.0..=180.0)
                        .speed(1.0)
                        .suffix("°"),
                );
                if ui.small_button(tr("collage.reset")).clicked() {
                    edit = Default::default();
                }
                if before != edit {
                    opts.collage_photos.insert(files[index].clone(), edit);
                }
                ui.weak(format!("{} {}", tr("collage.photo"), index + 1));
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.selected = None;
            self.dragging = None;
        }
        if !files.is_empty() {
            ui.label(egui::RichText::new(tr("collage.gestures")).small().weak());
        }
        ui.add_space(8.0);
        let height = ui.available_height().max(80.0);
        let width = ui.available_width().max(1.0);
        let side = (width - 48.0).min(height - 48.0).max(1.0);
        let (area, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let canvas = egui::Rect::from_center_size(area.center(), egui::vec2(side, side));
        ui.painter()
            .rect_filled(area, 12, ui.visuals().extreme_bg_color);
        ui.painter().rect_filled(
            canvas.expand(1.0),
            1,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        );
        ui.painter().rect_filled(canvas, 0, egui::Color32::WHITE);
        if let Some(texture) = &self.texture
            && !files.is_empty()
        {
            ui.painter().image(
                texture.id(),
                canvas,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        let reframe = self.reframe && !ui.input(|input| input.modifiers.alt);
        let mut drop_target = None;
        let count = if files.is_empty() { 4 } else { files.len() };
        for (i, [x, y, w, h]) in opts.collage_slots(count).into_iter().enumerate() {
            let cell = egui::Rect::from_min_size(
                canvas.min + egui::vec2(x, y) * side,
                egui::vec2(w, h) * side,
            )
            .shrink(
                opts.collage_gap
                    .min(opts.collage_size.clamp(256, 4096) / 16) as f32
                    / opts.collage_size.clamp(256, 4096) as f32
                    * side,
            );
            if files.is_empty() {
                match opts.collage_frame {
                    crate::options::CollageFrame::Circle => {
                        ui.painter().circle_filled(
                            cell.center(),
                            cell.width().min(cell.height()) / 2.0,
                            ui.visuals().faint_bg_color,
                        );
                    }
                    shape => {
                        let radius = if shape == crate::options::CollageFrame::Rounded {
                            (cell.width().min(cell.height()) * 0.15).min(255.0) as u8
                        } else {
                            0
                        };
                        ui.painter()
                            .rect_filled(cell, radius, ui.visuals().faint_bg_color);
                    }
                }
                ui.painter().text(
                    cell.center(),
                    egui::Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    egui::FontId::proportional(24.0),
                    ui.visuals().text_color(),
                );
            } else {
                let response = ui.interact(
                    cell,
                    ui.scope_id().with(("collage_slot", i)),
                    egui::Sense::click_and_drag(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        true,
                        format!("{} {}", tr("collage.photo"), i + 1),
                    )
                });
                response
                    .clone()
                    .on_hover_text(files[i].file_name().unwrap_or_default().to_string_lossy());
                if response.hovered() || response.has_focus() || self.selected == Some(i) {
                    Self::frame_outline(ui, cell, opts.collage_frame);
                }
                if response.hovered() && reframe {
                    let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                    if scroll != 0.0 {
                        self.selected = Some(i);
                        self.inspector_tab = 1;
                        let edit = opts.collage_photos.entry(files[i].clone()).or_default();
                        edit.zoom = (edit.zoom * (scroll * 0.003).exp()).clamp(1.0, 4.0);
                        ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
                    }
                }
                if response.has_focus() && reframe {
                    let movement = ui.input(|input| {
                        egui::vec2(
                            f32::from(input.key_pressed(egui::Key::ArrowLeft))
                                - f32::from(input.key_pressed(egui::Key::ArrowRight)),
                            f32::from(input.key_pressed(egui::Key::ArrowUp))
                                - f32::from(input.key_pressed(egui::Key::ArrowDown)),
                        )
                    });
                    if movement != egui::Vec2::ZERO {
                        let edit = opts.collage_photos.entry(files[i].clone()).or_default();
                        edit.x = (edit.x + movement.x * 0.05).clamp(0.0, 1.0);
                        edit.y = (edit.y + movement.y * 0.05).clamp(0.0, 1.0);
                    }
                }
                if response.double_clicked() {
                    opts.collage_photos.remove(&files[i]);
                }
                response.clone().on_hover_cursor(if response.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
                // Hold Alt to reorder without switching tools.
                // Reordering and crop movement are direct gestures on the photo.
                if response.drag_started() {
                    self.selected = Some(i);
                    self.inspector_tab = 1;
                    if !reframe {
                        self.dragging = Some(i);
                    }
                }
                if reframe && response.dragged() {
                    let delta = ui.input(|input| input.pointer.delta());
                    let edit = opts.collage_photos.entry(files[i].clone()).or_default();
                    edit.x = (edit.x - delta.x / cell.width().max(1.0)).clamp(0.0, 1.0);
                    edit.y = (edit.y - delta.y / cell.height().max(1.0)).clamp(0.0, 1.0);
                }
                if response.contains_pointer() && self.dragging.is_some() {
                    drop_target = Some(i);
                    Self::frame_outline(ui, cell, opts.collage_frame);
                }
                if response.clicked() {
                    self.selected = Some(i);
                    self.inspector_tab = 1;
                }
                // A visible badge identifies the selected photo without relying on color.
                if self.selected == Some(i) {
                    let badge = egui::Rect::from_min_size(
                        egui::pos2(
                            crate::dir::mirror_x(cell, cell.min.x + 6.0, 28.0),
                            cell.min.y + 6.0,
                        ),
                        egui::vec2(28.0, 24.0),
                    );
                    ui.painter()
                        .rect_filled(badge, 6, ui.visuals().selection.bg_fill);
                    ui.painter().text(
                        badge.center(),
                        egui::Align2::CENTER_CENTER,
                        (i + 1).to_string(),
                        egui::FontId::proportional(14.0),
                        ui.visuals().selection.stroke.color,
                    );
                }
            }
        }
        if ui.input(|input| input.pointer.any_released())
            && let (Some(from), Some(to)) = (self.dragging.take(), drop_target)
            && from < files.len()
            && to < files.len()
        {
            files.swap(from, to);
            self.selected = Some(to);
            self.last = None;
        }
    }
    fn record_history(&mut self, files: &[PathBuf], opts: &Options, busy: bool) {
        let current = (files.to_vec(), opts.clone());
        if let Some(previous) = &self.history_last {
            if previous != &current {
                if !self.history_gesture {
                    self.undo.push(previous.clone());
                    if self.undo.len() > 40 {
                        self.undo.remove(0);
                    }
                }
                self.redo.clear();
                self.history_gesture = busy;
            } else if !busy {
                self.history_gesture = false;
            }
        }
        self.history_last = Some(current);
    }

    fn restore_history(&mut self, files: &mut Vec<PathBuf>, opts: &mut Options, redo: bool) {
        let (source, destination) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        if let Some((paths, options)) = source.pop() {
            destination.push((files.clone(), opts.clone()));
            *files = paths;
            *opts = options;
            self.history_last = Some((files.clone(), opts.clone()));
            self.history_gesture = false;
            self.selected = None;
            self.dragging = None;
            self.last = None;
        }
    }

    fn frame_outline(ui: &egui::Ui, cell: egui::Rect, frame: crate::options::CollageFrame) {
        let stroke = egui::Stroke::new(2.0, ui.visuals().selection.bg_fill);
        if frame == crate::options::CollageFrame::Circle {
            ui.painter().circle_stroke(
                cell.center(),
                cell.width().min(cell.height()) / 2.0 - 1.0,
                stroke,
            );
        } else {
            let radius = if frame == crate::options::CollageFrame::Rounded {
                (cell.width().min(cell.height()) * 0.15).min(255.0) as u8
            } else {
                0
            };
            ui.painter()
                .rect_stroke(cell, radius, stroke, egui::StrokeKind::Inside);
        }
    }

    pub fn inspector(&mut self, ui: &mut egui::Ui, files: &mut Vec<PathBuf>, opts: &mut Options) {
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.spacing_mut().slider_width = (ui.available_width() - 72.0).max(40.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.inspector_tab, 0, tr("option.layout"));
            ui.selectable_value(&mut self.inspector_tab, 1, tr("collage.photo"));
            ui.selectable_value(&mut self.inspector_tab, 2, tr("collage.output"));
        });
        ui.separator();
        if self.inspector_tab == 0 {
            self.layout_controls(ui, opts);
        }
        if self.inspector_tab == 1 {
            self.photo_controls(ui, files, opts);
        }
        if self.inspector_tab == 2 {
            ui.strong(tr("collage.size"));
            ui.horizontal_wrapped(|ui| {
                for size in [1024, 2048, 4096] {
                    ui.selectable_value(&mut opts.collage_size, size, format!("{size} × {size}"));
                }
            });
            ui.label(
                egui::RichText::new(tr("collage.preview_hint"))
                    .small()
                    .weak(),
            );
        }
    }

    pub fn tab(&self) -> u8 {
        self.inspector_tab
    }

    pub fn output_tab(&self) -> bool {
        self.inspector_tab == 2
    }

    fn layout_controls(&mut self, ui: &mut egui::Ui, opts: &mut Options) {
        ui.strong(tr("option.layout"));
        ui.horizontal_wrapped(|ui| {
            for layout in crate::options::JoinLayout::ALL
                .into_iter()
                .filter(|layout| {
                    !matches!(
                        layout,
                        crate::options::JoinLayout::Vertical
                            | crate::options::JoinLayout::Horizontal
                    )
                })
            {
                // Allocated straight in the row: a `ui.vertical` per thumbnail
                // stacks them all on one spot in a right-to-left row.
                let response = ui.allocate_response(egui::vec2(72.0, 58.0), egui::Sense::click());
                let selected = opts.join_layout == layout;
                ui.painter().rect_filled(
                    response.rect,
                    8,
                    if selected {
                        ui.visuals().selection.bg_fill
                    } else {
                        ui.visuals().faint_bg_color
                    },
                );
                let bounds = response.rect.shrink(6.0);
                for [x, y, w, h] in layout.slots(4) {
                    let slot = egui::Rect::from_min_size(
                        bounds.min + egui::vec2(x * bounds.width(), y * bounds.height()),
                        egui::vec2(w * bounds.width(), h * bounds.height()),
                    )
                    .shrink(2.0);
                    ui.painter().rect_filled(
                        slot,
                        2,
                        if selected {
                            ui.visuals().selection.stroke.color
                        } else {
                            ui.visuals().weak_text_color()
                        },
                    );
                }
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, layout.label())
                });
                if response.clicked() {
                    opts.join_layout = layout;
                }
                response.on_hover_text(layout.label());
            }
        });
        ui.separator();
        ui.strong(tr("collage.frame"));
        ui.horizontal_wrapped(|ui| {
            for shape in crate::options::CollageFrame::ALL {
                ui.selectable_value(&mut opts.collage_frame, shape, shape.label());
            }
        });
        ui.label(tr("collage.spacing"));
        ui.add(egui::Slider::new(&mut opts.collage_gap, 0..=64).suffix(" px"));
        ui.label(tr("collage.fit"));
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut opts.collage_crop, true, tr("collage.fill"));
            ui.selectable_value(&mut opts.collage_crop, false, tr("collage.contain"));
        });
    }

    fn photo_controls(&mut self, ui: &mut egui::Ui, files: &mut Vec<PathBuf>, opts: &mut Options) {
        ui.separator();
        ui.strong(tr("collage.photo"));
        let selected = self.selected.filter(|&i| i < files.len());
        if let Some(index) = selected {
            ui.label(
                egui::RichText::new(
                    files[index]
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
                .small()
                .weak(),
            );
        } else {
            ui.label(
                egui::RichText::new(tr("collage.select_hint"))
                    .small()
                    .weak(),
            );
        }
        ui.add_enabled_ui(selected.is_some(), |ui| {
            let mut edit = selected
                .and_then(|i| opts.collage_photos.get(&files[i]))
                .copied()
                .unwrap_or_default();
            let before = edit;
            for (label, value, range, suffix) in [
                (tr("collage.zoom"), &mut edit.zoom, 1.0..=4.0, "×"),
                (tr("collage.position_x"), &mut edit.x, 0.0..=1.0, ""),
                (tr("collage.position_y"), &mut edit.y, 0.0..=1.0, ""),
            ] {
                ui.label(label);
                ui.add(egui::Slider::new(value, range).suffix(suffix));
            }
            ui.label(tr("collage.angle"));
            ui.horizontal_wrapped(|ui| {
                use crate::{
                    icons,
                    widgets::{self, Kind, Size},
                };
                if widgets::icon_button(
                    ui,
                    icons::ARROW_COUNTER_CLOCKWISE,
                    tr("collage.rotate_left"),
                    Kind::Secondary,
                    Size::Regular,
                )
                .clicked()
                {
                    edit.turns = (edit.turns + 3) % 4;
                }
                ui.add(
                    egui::DragValue::new(&mut edit.angle)
                        .range(-180.0..=180.0)
                        .speed(1.0)
                        .suffix("°"),
                );
                if widgets::icon_button(
                    ui,
                    icons::ARROW_CLOCKWISE,
                    tr("collage.rotate"),
                    Kind::Secondary,
                    Size::Regular,
                )
                .clicked()
                {
                    edit.turns = (edit.turns + 1) % 4;
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button(tr("collage.reset")).clicked() {
                    edit = Default::default();
                }
            });
            if let Some(index) = selected {
                if before != edit {
                    opts.collage_photos.insert(files[index].clone(), edit);
                }
                if ui.button(tr("collage.remove_photo")).clicked() {
                    opts.collage_photos.remove(&files.remove(index));
                    self.selected = None;
                    self.dragging = None;
                    self.last = None;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_groups_a_drag_and_new_edits_clear_redo() {
        let mut editor = CollageEditor::default();
        let mut opts = Options::default();
        let mut files = vec![PathBuf::from("photo.png")];
        editor.record_history(&files, &opts, false);
        for gap in [20, 24, 28] {
            opts.collage_gap = gap;
            editor.record_history(&files, &opts, true);
        }
        editor.record_history(&files, &opts, false);
        assert_eq!(editor.undo.len(), 1);
        editor.restore_history(&mut files, &mut opts, false);
        assert_eq!(opts.collage_gap, Options::default().collage_gap);
        editor.restore_history(&mut files, &mut opts, true);
        assert_eq!(opts.collage_gap, 28);
        editor.restore_history(&mut files, &mut opts, false);
        opts.collage_gap = 8;
        editor.record_history(&files, &opts, false);
        assert!(editor.redo.is_empty());
    }

    #[test]
    fn canvas_remains_visible_below_toolbar_at_editor_sizes() {
        for width in [280.0, 500.0, 900.0] {
            for align in [egui::Align::Min, egui::Align::Max] {
                for dark in [false, true] {
                    let ctx = egui::Context::default();
                    ctx.set_visuals(if dark {
                        egui::Visuals::dark()
                    } else {
                        egui::Visuals::light()
                    });
                    let mut editor = CollageEditor::default();
                    let mut options = Options::default();
                    editor.prepare(&mut options);
                    let mut bounds = egui::Rect::NOTHING;
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 560.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            ui.with_layout(egui::Layout::top_down(align), |ui| {
                                bounds = ui.available_rect_before_wrap();
                                editor.show(ui, &mut Vec::new(), &mut options, None);
                            });
                        },
                    );
                    output.textures_delta.clear();
                    let canvas = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect)
                                if rect.fill == egui::Color32::WHITE
                                    && rect.rect.width() > 100.0 =>
                            {
                                Some(rect.rect)
                            }
                            _ => None,
                        })
                        .expect("the empty collage must show a real canvas without scrolling");
                    assert!(
                        bounds.contains_rect(canvas),
                        "canvas {canvas:?} outside {bounds:?}"
                    );
                    assert!(
                        canvas.width() >= (width - 80.0).min(440.0),
                        "canvas is too small: {canvas:?}"
                    );
                }
            }
        }
    }
}
