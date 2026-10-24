//! The Clip window: scrub through a video, set start/end points, and drag a
//! crop rectangle directly on the picture.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, Pos2, Rect as ScreenRect, RichText, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Vec2,
};

use crate::i18n::{tr, trf};
use crate::icons;
use crate::options::{Edit, Rect, format_timestamp, parse_timestamp};
use crate::plan::input_url;
use crate::probe::MediaInfo;
use crate::process::{KillOnDrop, command};
use crate::theme;
use crate::widgets::{self, Kind, Size};

/// Preview frames are scaled down to at most this width.
const PREVIEW_WIDTH: u32 = 960;
/// Cap on the PNG bytes read for one frame.
const MAX_FRAME_BYTES: u64 = 64 << 20;
const DISPLAY_MAX_WIDTH: f32 = 760.0;
const MIN_RECT_SIDE: u32 = 8;

type FrameResult = Result<ColorImage, String>;

pub struct Preview {
    path: PathBuf,
    ffmpeg: PathBuf,
    info: Option<MediaInfo>,
    probe_rx: Option<Receiver<Option<MediaInfo>>>,
    time: f64,
    shown_time: Option<f64>,
    texture: Option<TextureHandle>,
    frame_rx: Option<(f64, Receiver<FrameResult>)>,
    error: Option<String>,
    drag_from: Option<Pos2>,
}

/// Decodes one frame at `time` as an egui image.
fn grab_frame(ffmpeg: &Path, path: &Path, time: f64) -> FrameResult {
    let mut child = command(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &format!("{time:.3}"),
            "-i",
        ])
        .arg(input_url(path, None))
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale='min({PREVIEW_WIDTH},iw)':-2"),
            "-f",
            "image2pipe",
            "-c:v",
            "png",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map(KillOnDrop)
        .map_err(|e| e.to_string())?;
    let mut png = Vec::new();
    if let Some(out) = child.0.stdout.take() {
        out.take(MAX_FRAME_BYTES)
            .read_to_end(&mut png)
            .map_err(|e| e.to_string())?;
    }
    let _ = child.0.wait();
    let rgba = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|_| tr("preview.no_frame").to_owned())?
        .to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// Maps a point inside the displayed picture to source-pixel coordinates.
fn to_source(pos: Pos2, shown: ScreenRect, source: (u32, u32)) -> (u32, u32) {
    let fx = ((pos.x - shown.min.x) / shown.width()).clamp(0.0, 1.0);
    let fy = ((pos.y - shown.min.y) / shown.height()).clamp(0.0, 1.0);
    (
        (fx * source.0 as f32).round() as u32,
        (fy * source.1 as f32).round() as u32,
    )
}

/// The rectangle spanned by two corner points, clamped to the frame.
fn rect_between(a: (u32, u32), b: (u32, u32)) -> Rect {
    let (x, y) = (a.0.min(b.0), a.1.min(b.1));
    Rect {
        x,
        y,
        w: a.0.abs_diff(b.0),
        h: a.1.abs_diff(b.1),
    }
}

fn to_screen(rect: Rect, shown: ScreenRect, source: (u32, u32)) -> ScreenRect {
    let sx = shown.width() / source.0.max(1) as f32;
    let sy = shown.height() / source.1.max(1) as f32;
    ScreenRect::from_min_size(
        shown.min + Vec2::new(rect.x as f32 * sx, rect.y as f32 * sy),
        Vec2::new(rect.w as f32 * sx, rect.h as f32 * sy),
    )
}

impl Preview {
    pub fn open(path: PathBuf, ffmpeg: PathBuf, ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let (probe_ffmpeg, probe_path, ctx) = (ffmpeg.clone(), path.clone(), ctx.clone());
        thread::spawn(move || {
            let info = crate::ffmpeg::probe(&probe_ffmpeg, &probe_path, None).ok();
            let _ = tx.send(info);
            ctx.request_repaint();
        });
        Self {
            path,
            ffmpeg,
            info: None,
            probe_rx: Some(rx),
            time: 0.0,
            shown_time: None,
            texture: None,
            frame_rx: None,
            error: None,
            drag_from: None,
        }
    }

    fn duration(&self) -> f64 {
        self.info.as_ref().and_then(|i| i.duration_s).unwrap_or(0.0)
    }

    fn source_size(&self) -> Option<(u32, u32)> {
        let info = self.info.as_ref()?;
        Some((info.width?, info.height?))
    }

    /// Collects finished background work and starts a frame grab when needed.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.probe_rx
            && let Ok(info) = rx.try_recv()
        {
            self.probe_rx = None;
            match info {
                Some(info) if info.has_video => self.info = Some(info),
                _ => self.error = Some(tr("error.no_video").to_owned()),
            }
        }
        if let Some((time, rx)) = &self.frame_rx
            && let Ok(result) = rx.try_recv()
        {
            let time = *time;
            self.frame_rx = None;
            match result {
                Ok(image) => {
                    self.texture =
                        Some(ctx.load_texture("clip_preview", image, TextureOptions::LINEAR));
                    self.shown_time = Some(time);
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
        let wanted = self.time;
        let stale = self.shown_time.is_none_or(|t| (t - wanted).abs() > 0.01);
        if self.info.is_some() && self.frame_rx.is_none() && stale {
            let (tx, rx) = mpsc::channel();
            let (ffmpeg, path, ctx) = (self.ffmpeg.clone(), self.path.clone(), ctx.clone());
            thread::spawn(move || {
                let _ = tx.send(grab_frame(&ffmpeg, &path, wanted));
                ctx.request_repaint();
            });
            self.frame_rx = Some((wanted, rx));
        }
    }

    /// Shows the window; returns `false` once the user closes it.
    /// `edit` receives the clip points; `rect` is the crop (or logo) area.
    pub fn show(&mut self, ctx: &egui::Context, edit: &mut Edit, rect: &mut Option<Rect>) -> bool {
        self.poll(ctx);
        let mut open = true;
        let title = icons::label(
            icons::EYE,
            &trf(
                "preview.title",
                &[(
                    "name",
                    &self
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default(),
                )],
            ),
        );
        egui::Window::new(title)
            .id(egui::Id::unique("clip_window"))
            .open(&mut open)
            .default_width(DISPLAY_MAX_WIDTH + 32.0)
            .resizable(false)
            .show(ctx, |ui| self.contents(ui, edit, rect));
        open
    }

    fn contents(&mut self, ui: &mut egui::Ui, edit: &mut Edit, rect: &mut Option<Rect>) {
        let p = theme::palette(ui);
        if let Some(error) = &self.error {
            widgets::chip(ui, icons::WARNING, error, p.danger);
        }
        let Some(source) = self.source_size() else {
            ui.add(egui::Spinner::new());
            return;
        };
        let width = ui.available_width().min(DISPLAY_MAX_WIDTH);
        let height = width * source.1 as f32 / source.0.max(1) as f32;
        let (shown, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::drag());
        let painter = ui.painter_at(shown);
        painter.rect_filled(shown, CornerRadius::same(6), p.sunken);
        if let Some(texture) = &self.texture {
            painter.image(
                texture.id(),
                shown,
                ScreenRect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if self.frame_rx.is_some() {
            painter.text(
                shown.right_top() + Vec2::new(-12.0, 12.0),
                egui::Align2::RIGHT_TOP,
                tr("preview.loading"),
                egui::FontId::proportional(13.0),
                p.text_weak,
            );
        }

        // Drag on the picture to draw the rectangle.
        if response.drag_started()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.drag_from = Some(pos);
        }
        if let (Some(from), Some(pos)) = (self.drag_from, response.interact_pointer_pos())
            && response.dragged()
        {
            let r = rect_between(
                to_source(from, shown, source),
                to_source(pos, shown, source),
            );
            *rect = (r.w >= MIN_RECT_SIDE && r.h >= MIN_RECT_SIDE).then_some(r);
        }
        if response.drag_stopped() {
            self.drag_from = None;
        }
        if let Some(r) = *rect {
            let area = to_screen(r, shown, source);
            let shade = Color32::from_black_alpha(140);
            for outside in [
                ScreenRect::from_min_max(shown.min, Pos2::new(shown.max.x, area.min.y)),
                ScreenRect::from_min_max(Pos2::new(shown.min.x, area.max.y), shown.max),
                ScreenRect::from_min_max(
                    Pos2::new(shown.min.x, area.min.y),
                    Pos2::new(area.min.x, area.max.y),
                ),
                ScreenRect::from_min_max(
                    Pos2::new(area.max.x, area.min.y),
                    Pos2::new(shown.max.x, area.max.y),
                ),
            ] {
                painter.rect_filled(outside, CornerRadius::ZERO, shade);
            }
            painter.rect_stroke(
                area,
                CornerRadius::ZERO,
                Stroke::new(2.0, p.accent),
                StrokeKind::Middle,
            );
        }
        let response = response.on_hover_cursor(egui::CursorIcon::Crosshair);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Image, true, tr("preview.draw_hint"))
        });

        ui.add_space(8.0);
        let duration = self.duration();
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = (width - 140.0).max(120.0);
            ui.add(egui::Slider::new(&mut self.time, 0.0..=duration.max(0.001)).show_value(false));
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    format_timestamp(self.time),
                    format_timestamp(duration)
                ))
                .monospace(),
            );
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::button(
                ui,
                Some(icons::CARET_UP),
                tr("preview.set_start"),
                Kind::Secondary,
                Size::Regular,
                true,
            )
            .clicked()
            {
                edit.trim_start = format_timestamp(self.time);
            }
            if widgets::button(
                ui,
                Some(icons::CARET_DOWN),
                tr("preview.set_end"),
                Kind::Secondary,
                Size::Regular,
                true,
            )
            .clicked()
            {
                edit.trim_end = format_timestamp(self.time);
            }
            if widgets::button(
                ui,
                Some(icons::X),
                tr("preview.clear"),
                Kind::Ghost,
                Size::Regular,
                edit.has_trim() || rect.is_some(),
            )
            .clicked()
            {
                edit.trim_start.clear();
                edit.trim_end.clear();
                *rect = None;
            }
        });
        ui.add_space(4.0);
        let start = parse_timestamp(&edit.trim_start)
            .ok()
            .flatten()
            .unwrap_or(0.0);
        let end = parse_timestamp(&edit.trim_end)
            .ok()
            .flatten()
            .unwrap_or(duration);
        let summary = match *rect {
            Some(r) => trf(
                "preview.summary_crop",
                &[
                    ("start", &format_timestamp(start)),
                    ("end", &format_timestamp(end)),
                    ("w", &r.w),
                    ("h", &r.h),
                    ("x", &r.x),
                    ("y", &r.y),
                ],
            ),
            None => trf(
                "preview.summary",
                &[
                    ("start", &format_timestamp(start)),
                    ("end", &format_timestamp(end)),
                ],
            ),
        };
        ui.label(RichText::new(summary).color(p.text_weak));
        ui.label(RichText::new(tr("preview.draw_hint")).weak());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_and_source_coordinates_round_trip() {
        let shown = ScreenRect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(480.0, 270.0));
        let source = (1920, 1080);
        assert_eq!(to_source(Pos2::new(10.0, 20.0), shown, source), (0, 0));
        assert_eq!(
            to_source(Pos2::new(490.0, 290.0), shown, source),
            (1920, 1080)
        );
        // Points outside the picture are clamped to its edge.
        assert_eq!(to_source(Pos2::new(-50.0, 500.0), shown, source), (0, 1080));
        let r = rect_between((400, 300), (100, 100));
        assert_eq!(
            r,
            Rect {
                x: 100,
                y: 100,
                w: 300,
                h: 200
            }
        );
        let back = to_screen(r, shown, source);
        assert!((back.min.x - 35.0).abs() < 0.01 && (back.width() - 75.0).abs() < 0.01);
    }
}
