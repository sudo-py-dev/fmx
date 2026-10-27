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
use crate::options::{Edit, Rect, Rotation, display_time, format_timestamp, parse_timestamp};
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
/// Tall (portrait) videos are shown at most this high in the window.
const DISPLAY_MAX_HEIGHT: f32 = 430.0;
const MIN_RECT_SIDE: u32 = 8;
const TIMELINE_HEIGHT: f32 = 38.0;
/// How close (in points) the pointer must be to grab a start/end handle.
const HANDLE_GRAB: f32 = 10.0;
/// The shortest clip the handles allow, in seconds.
const MIN_CLIP_S: f64 = 0.1;
/// Seek steps offered under the timeline, in seconds.
const NUDGES: [f64; 4] = [-5.0, -1.0, 1.0, 5.0];

/// What the preview lets the user set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// Start and end points of the part to keep.
    pub clip: bool,
    /// A rectangle drawn on the picture (crop or logo area).
    pub area: bool,
    /// Show live rotation transformation.
    pub rotate: bool,
}

impl Mode {
    pub const CLIP: Self = Self {
        clip: true,
        area: false,
        rotate: false,
    };
    pub const AREA: Self = Self {
        clip: false,
        area: true,
        rotate: false,
    };
    pub const ROTATION: Self = Self {
        clip: false,
        area: false,
        rotate: true,
    };
    const BOTH: Self = Self {
        clip: true,
        area: true,
        rotate: true,
    };
}

/// What a drag on the timeline moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    Playhead,
    Start,
    End,
}

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
    grab: Option<Grab>,
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

fn paint_rotated_image(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    rect: ScreenRect,
    rotation: Rotation,
) {
    let mut mesh = egui::Mesh::with_texture(texture_id);
    let (tl_uv, tr_uv, br_uv, bl_uv) = match rotation {
        Rotation::Clockwise => (
            Pos2::new(0.0, 1.0),
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(1.0, 1.0),
        ),
        Rotation::CounterClockwise => (
            Pos2::new(1.0, 0.0),
            Pos2::new(1.0, 1.0),
            Pos2::new(0.0, 1.0),
            Pos2::new(0.0, 0.0),
        ),
        Rotation::UpsideDown => (
            Pos2::new(1.0, 1.0),
            Pos2::new(0.0, 1.0),
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
        ),
        Rotation::FlipHorizontal => (
            Pos2::new(1.0, 0.0),
            Pos2::new(0.0, 0.0),
            Pos2::new(0.0, 1.0),
            Pos2::new(1.0, 1.0),
        ),
        Rotation::FlipVertical => (
            Pos2::new(0.0, 1.0),
            Pos2::new(1.0, 1.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(0.0, 0.0),
        ),
    };
    let c = Color32::WHITE;
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.left_top(),
        uv: tl_uv,
        color: c,
    });
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.right_top(),
        uv: tr_uv,
        color: c,
    });
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.right_bottom(),
        uv: br_uv,
        color: c,
    });
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.left_bottom(),
        uv: bl_uv,
        color: c,
    });
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
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
            grab: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn duration(&self) -> f64 {
        self.info.as_ref().and_then(|i| i.duration_s).unwrap_or(0.0)
    }

    pub fn source_size(&self) -> Option<(u32, u32)> {
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
            .show(ctx, |ui| {
                self.contents(
                    ui,
                    edit,
                    rect,
                    Mode::BOTH,
                    DISPLAY_MAX_HEIGHT,
                    edit.rotation,
                );
            });
        open
    }

    /// Draws the preview inside the current layout, for tools whose main job is
    /// picking a part of the video. The picture is at most `max_height` tall.
    pub fn inline(
        &mut self,
        ui: &mut egui::Ui,
        edit: &mut Edit,
        rect: &mut Option<Rect>,
        mode: Mode,
        max_height: f32,
        rotation: Option<Rotation>,
    ) {
        self.poll(ui.ctx());
        self.contents(ui, edit, rect, mode, max_height, rotation);
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        edit: &mut Edit,
        rect: &mut Option<Rect>,
        mode: Mode,
        max_height: f32,
        rotation: Option<Rotation>,
    ) {
        let p = theme::palette(ui);
        if let Some(error) = &self.error {
            widgets::chip(ui, icons::WARNING, error, p.danger);
        }
        let Some(source) = self.source_size() else {
            if self.error.is_none() {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(RichText::new(tr("preview.loading")).color(p.text_weak));
                });
            }
            return;
        };
        let swaps = rotation.is_some_and(|r| r.swaps_dimensions());
        let (src_w, src_h) = if swaps {
            (source.1, source.0)
        } else {
            (source.0, source.1)
        };
        let aspect = src_w.max(1) as f32 / src_h.max(1) as f32;
        let width = ui
            .available_width()
            .min(DISPLAY_MAX_WIDTH)
            .min(max_height * aspect);
        let height = width / aspect;
        let sense = if mode.area {
            Sense::drag()
        } else {
            Sense::hover()
        };
        let (shown, response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
        let painter = ui.painter_at(shown);
        painter.rect_filled(shown, CornerRadius::same(6), p.sunken);
        if let Some(texture) = &self.texture {
            if let Some(rot) = rotation {
                paint_rotated_image(&painter, texture.id(), shown, rot);
            } else {
                painter.image(
                    texture.id(),
                    shown,
                    ScreenRect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
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
        if mode.area {
            let response = response.on_hover_cursor(egui::CursorIcon::Crosshair);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Image, true, tr("preview.draw_hint"))
            });
        }

        ui.add_space(8.0);
        let duration = self.duration();
        self.timeline(ui, width, edit, mode.clip);
        ui.add_space(4.0);
        // Seeking, the clock and the set/clear buttons share one row to save height.
        ui.horizontal_wrapped(|ui| {
            for step in NUDGES {
                let label = if step < 0.0 {
                    format!("\u{2212}{} s", -step)
                } else {
                    format!("+{step} s")
                };
                if widgets::button(ui, None, &label, Kind::Ghost, Size::Small, true)
                    .on_hover_text(tr("preview.nudge"))
                    .clicked()
                {
                    self.time = (self.time + step).clamp(0.0, duration);
                }
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    display_time(self.time),
                    display_time(duration)
                ))
                .monospace(),
            );
            ui.add_space(10.0);
            if mode.clip {
                if widgets::button(
                    ui,
                    Some(icons::CARET_RIGHT),
                    tr("preview.set_start"),
                    Kind::Secondary,
                    Size::Regular,
                    true,
                )
                .on_hover_text(tr("preview.set_start_hint"))
                .clicked()
                {
                    edit.trim_start = format_timestamp(self.time);
                }
                if widgets::button(
                    ui,
                    Some(icons::CARET_LEFT),
                    tr("preview.set_end"),
                    Kind::Secondary,
                    Size::Regular,
                    true,
                )
                .on_hover_text(tr("preview.set_end_hint"))
                .clicked()
                {
                    edit.trim_end = format_timestamp(self.time);
                }
            }
            if mode.clip || mode.area {
                let has_edit = (mode.clip && edit.has_trim()) || (mode.area && rect.is_some());
                if widgets::button(
                    ui,
                    Some(icons::X),
                    tr("preview.clear"),
                    Kind::Ghost,
                    Size::Regular,
                    has_edit,
                )
                .clicked()
                {
                    if mode.clip {
                        edit.trim_start.clear();
                        edit.trim_end.clear();
                    }
                    if mode.area {
                        *rect = None;
                    }
                }
            }
        });
        ui.add_space(4.0);
        let (start, end) = clip_range(edit, duration);
        let mut lines = Vec::new();
        if mode.clip {
            lines.push(trf(
                "preview.summary",
                &[
                    ("start", &display_time(start)),
                    ("end", &display_time(end)),
                    ("length", &display_time((end - start).max(0.0))),
                ],
            ));
        }
        if mode.area {
            lines.push(match *rect {
                Some(r) => trf(
                    "preview.summary_area",
                    &[("w", &r.w), ("h", &r.h), ("x", &r.x), ("y", &r.y)],
                ),
                None => tr("preview.draw_hint").to_owned(),
            });
        }
        if mode.rotate {
            let (out_w, out_h) =
                rotation.map_or(source, |r| r.transformed_size(source.0, source.1));
            let is_portrait = out_h > out_w;
            let orientation = if is_portrait {
                tr("rotation.portrait")
            } else {
                tr("rotation.landscape")
            };
            let rot_label = rotation.map_or("—", |r| r.label());
            lines.push(format!("{rot_label}  •  {out_w} × {out_h} ({orientation})"));
        }
        for line in lines {
            ui.label(RichText::new(line).color(p.text_weak));
        }
    }

    /// A scrubbable track. With `clip`, the kept part is highlighted and its two
    /// ends can be dragged; anywhere else moves the playhead.
    fn timeline(&mut self, ui: &mut egui::Ui, width: f32, edit: &mut Edit, clip: bool) {
        let p = theme::palette(ui);
        let duration = self.duration().max(0.001);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, TIMELINE_HEIGHT), Sense::click_and_drag());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Slider, true, tr("preview.timeline"))
        });
        let x_of = |t: f64| rect.left() + (t / duration).clamp(0.0, 1.0) as f32 * rect.width();
        let t_of =
            |x: f32| f64::from(((x - rect.left()) / rect.width()).clamp(0.0, 1.0)) * duration;
        let (start, end) = clip_range(edit, duration);
        let near = |x: f32, t: f64| clip && (x - x_of(t)).abs() <= HANDLE_GRAB;
        let pointer = response.interact_pointer_pos().or(response.hover_pos());

        if response.drag_started()
            && let Some(pos) = pointer
        {
            // The handle nearest the pointer wins when both are within reach.
            let to_start = (pos.x - x_of(start)).abs();
            let to_end = (pos.x - x_of(end)).abs();
            self.grab = Some(match (near(pos.x, start), near(pos.x, end)) {
                (true, true) if to_start < to_end => Grab::Start,
                (true, true) | (false, true) => Grab::End,
                (true, false) => Grab::Start,
                (false, false) => Grab::Playhead,
            });
        }
        if let Some(pos) = pointer
            && (response.dragged() || response.clicked())
        {
            let t = t_of(pos.x);
            match self.grab.unwrap_or(Grab::Playhead) {
                Grab::Playhead => self.time = t,
                Grab::Start => {
                    let t = t.min(end - MIN_CLIP_S).max(0.0);
                    edit.trim_start = format_timestamp(t);
                    self.time = t;
                }
                Grab::End => {
                    let t = t.max(start + MIN_CLIP_S).min(duration);
                    edit.trim_end = format_timestamp(t);
                    self.time = t;
                }
            }
        }
        if response.drag_stopped() {
            self.grab = None;
        }
        let over_handle = response
            .hover_pos()
            .is_some_and(|pos| near(pos.x, start) || near(pos.x, end));
        let cursor = if over_handle || matches!(self.grab, Some(Grab::Start | Grab::End)) {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::PointingHand
        };
        let response = response.on_hover_cursor(cursor);
        if response.hovered() && !over_handle && self.grab.is_none() {
            response.on_hover_text(tr("preview.timeline_hint"));
        }

        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter();
        let track = rect.shrink2(Vec2::new(0.0, 6.0));
        painter.rect_filled(track, CornerRadius::same(6), p.sunken);
        painter.rect_stroke(
            track,
            CornerRadius::same(6),
            Stroke::new(1.0, p.border),
            StrokeKind::Inside,
        );
        if clip {
            let kept = ScreenRect::from_x_y_ranges(x_of(start)..=x_of(end), track.y_range());
            painter.rect_filled(kept, CornerRadius::same(4), p.accent.gamma_multiply(0.35));
            for t in [start, end] {
                let handle = ScreenRect::from_center_size(
                    Pos2::new(x_of(t), rect.center().y),
                    Vec2::new(8.0, rect.height()),
                );
                painter.rect_filled(handle, CornerRadius::same(3), p.accent);
                // Grip lines make the handles read as draggable.
                for dy in [-4.0, 0.0, 4.0] {
                    let y = handle.center().y + dy;
                    painter.line_segment(
                        [
                            Pos2::new(handle.left() + 2.0, y),
                            Pos2::new(handle.right() - 2.0, y),
                        ],
                        Stroke::new(1.0, p.on_accent),
                    );
                }
            }
        }
        let x = x_of(self.time);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(2.0, p.text),
        );
        painter.circle_filled(Pos2::new(x, rect.top() + 2.0), 4.0, p.text);
    }
}

/// The kept part as `(start, end)` seconds; empty fields mean the whole file.
fn clip_range(edit: &Edit, duration: f64) -> (f64, f64) {
    let start = parse_timestamp(&edit.trim_start)
        .ok()
        .flatten()
        .unwrap_or(0.0);
    let end = parse_timestamp(&edit.trim_end)
        .ok()
        .flatten()
        .unwrap_or(duration);
    (start, end)
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
