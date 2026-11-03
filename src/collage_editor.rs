//! Background-rendered photo preview; only the latest edit is displayed.
use crate::{i18n::tr, options::Options};
use eframe::egui;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::mpsc,
};

type Request = (u64, Vec<PathBuf>, Options, Option<PathBuf>);
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
            self.initialized = true;
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        files: &mut [PathBuf],
        opts: &Options,
        ffmpeg: Option<PathBuf>,
    ) {
        if self.worker.is_none() {
            let (tx, requests) = mpsc::channel::<Request>();
            let (results, rx) = mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let mut photos = HashMap::new();
                let mut adjustments = None;
                while let Ok(mut request) = requests.recv() {
                    while let Ok(latest) = requests.try_recv() {
                        request = latest;
                    }
                    let (version, files, opts, ffmpeg) = request;
                    let result = (|| {
                        let current = (opts.image_max_side, opts.image_rotation, opts.grayscale);
                        if adjustments != Some(current) {
                            photos.clear();
                            adjustments = Some(current);
                        }
                        // Reordering or adding a photo preserves already decoded sources.
                        let active: HashSet<_> = files.iter().collect();
                        photos.retain(|path, _| active.contains(path));
                        for path in &files {
                            if !photos.contains_key(path) {
                                photos.insert(
                                    path.clone(),
                                    crate::imaging::collage_photo(path, &opts, ffmpeg.as_deref())?,
                                );
                            }
                        }
                        let pictures: Vec<_> = files.iter().map(|path| &photos[path]).collect();
                        crate::imaging::collage_preview(&pictures, &opts, ffmpeg.as_deref())
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
        if changed {
            if self
                .last
                .as_ref()
                .is_some_and(|(paths, _)| paths.as_slice() != files)
            {
                self.selected = None;
                self.texture = None;
            }
            self.version += 1;
            self.last = Some((files.to_vec(), opts.clone()));
            self.error = None;
            self.worker
                .as_ref()
                .unwrap()
                .0
                .send((self.version, files.to_vec(), opts.clone(), ffmpeg))
                .ok();
        }
        while let Ok((version, result)) = self.worker.as_ref().unwrap().1.try_recv() {
            if version != self.version {
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
                Err(error) => self.error = Some(error),
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(tr("collage.preview"));
            if self.ready != self.version {
                ui.spinner();
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if files.is_empty() {
            ui.label(tr("collage.empty"));
        } else {
            ui.label(tr("collage.swap"));
        }
        let side = ui
            .available_width()
            .min(ui.available_height().max(200.0))
            .clamp(120.0, 520.0);
        // Reserve the whole row so RTL and LTR layouts share the same canvas center.
        let (area, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), side), egui::Sense::hover());
        let canvas = egui::Rect::from_center_size(area.center(), egui::vec2(side, side));
        ui.painter().rect_filled(canvas, 0, egui::Color32::WHITE);
        if let Some(texture) = &self.texture {
            ui.painter().image(
                texture.id(),
                canvas,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        let count = if files.is_empty() { 4 } else { files.len() };
        for (i, [x, y, w, h]) in opts.join_layout.slots(count).into_iter().enumerate() {
            let cell = egui::Rect::from_min_size(
                canvas.min + egui::vec2(x, y) * side,
                egui::vec2(w, h) * side,
            )
            .shrink(opts.collage_gap as f32 / opts.collage_size.max(1) as f32 * side);
            if files.is_empty() {
                ui.painter()
                    .rect_filled(cell, 0, ui.visuals().faint_bg_color);
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
                    egui::Sense::click(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        true,
                        format!("{} {}", tr("collage.photo"), i + 1),
                    )
                });
                if response.hovered() || self.selected == Some(i) {
                    ui.painter().rect_stroke(
                        cell,
                        0,
                        egui::Stroke::new(3.0, ui.visuals().selection.bg_fill),
                        egui::StrokeKind::Inside,
                    );
                }
                if response.clicked() {
                    if let Some(first) = self.selected.take() {
                        if first < files.len() {
                            files.swap(first, i);
                        }
                    } else {
                        self.selected = Some(i);
                    }
                }
            }
        }
    }
}
