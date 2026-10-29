//! The Clip window: play or scrub through a video, set start/end points,
//! mark moments, and drag a crop rectangle directly on the picture.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, Pos2, Rect as ScreenRect, RichText, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Vec2,
};

use crate::dir;
use crate::i18n::{tr, trf};
use crate::icons;
use crate::options::{
    Edit, Orientation, Rect, add_frame_time, compact_timestamp, display_time, parse_timestamp,
};
use crate::plan::input_url;
use crate::probe::MediaInfo;
use crate::process::{KillOnDrop, command};
use crate::theme;
use crate::widgets::{self, Kind, Size};

/// Preview frames are scaled down to at most this width.
const PREVIEW_WIDTH: u32 = 1280;
/// Cap on the PNG bytes read for one frame.
const MAX_FRAME_BYTES: u64 = 64 << 20;
const DISPLAY_MAX_WIDTH: f32 = 1100.0;
/// The Clip window's picture width; the window opens just wide enough.
const WINDOW_DISPLAY_WIDTH: f32 = 760.0;
/// Tall (portrait) videos are shown at most this high in the window.
const DISPLAY_MAX_HEIGHT: f32 = 430.0;
const MIN_RECT_SIDE: u32 = 8;
const TIMELINE_HEIGHT: f32 = 38.0;
/// How close (in points) the pointer must be to grab a start/end handle.
const HANDLE_GRAB: f32 = 10.0;
/// The shortest clip the handles allow, in seconds.
const MIN_CLIP_S: f64 = 0.1;
/// Width of a typed in or out point.
const TIME_FIELD_WIDTH: f32 = 84.0;
/// Space around the clip's length, between its in and out points.
const CLIP_GAP: f32 = 10.0;
/// How far the skip buttons move, in seconds; the arrow keys move one.
const SKIP_S: f64 = 5.0;
/// Frames per second shown while playing; enough to watch, light to decode.
const PLAY_FPS: u32 = 25;

/// What the preview lets the user set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// Start and end points of the part to keep.
    pub clip: bool,
    /// A rectangle drawn on the picture (crop or logo area).
    pub area: bool,
    /// Show live rotation transformation.
    pub rotate: bool,
    /// Moments marked on the timeline (frames to extract).
    pub marks: bool,
}

impl Mode {
    pub const CLIP: Self = Self {
        clip: true,
        area: false,
        rotate: false,
        marks: false,
    };
    pub const AREA: Self = Self {
        clip: false,
        area: true,
        rotate: false,
        marks: false,
    };
    pub const ROTATION: Self = Self {
        clip: false,
        area: false,
        rotate: true,
        marks: false,
    };
    pub const MARKS: Self = Self {
        clip: false,
        area: false,
        rotate: false,
        marks: true,
    };
    const BOTH: Self = Self {
        clip: true,
        area: true,
        rotate: false,
        marks: false,
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
    player: Option<Player>,
}

/// Plays the video from a moment: ffmpeg decodes it at its own pace (`-re`)
/// into raw frames, read on a thread. Dropping it stops ffmpeg.
struct Player {
    _child: KillOnDrop,
    frames: Receiver<(f64, ColorImage)>,
    /// The playhead as the player last set it; anything else means a seek.
    last_time: f64,
}

impl Player {
    fn start(
        ffmpeg: &Path,
        path: &Path,
        from: f64,
        source: (u32, u32),
        ctx: &egui::Context,
    ) -> std::io::Result<Self> {
        let (w, h) = play_size(source);
        let mut child = command(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-re", "-ss"])
            .arg(format!("{from:.3}"))
            .arg("-i")
            .arg(input_url(path, None))
            .args(["-an", "-sn", "-vf"])
            .arg(format!("fps={PLAY_FPS},scale={w}:{h}"))
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map(KillOnDrop)?;
        let mut out = child
            .0
            .stdout
            .take()
            .ok_or(std::io::ErrorKind::BrokenPipe)?;
        // A short queue: frames wait for the window, never pile up.
        let (tx, rx) = mpsc::sync_channel(2);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let mut rgba = vec![0u8; w as usize * h as usize * 4];
            for n in 0u32.. {
                if out.read_exact(&mut rgba).is_err() {
                    break;
                }
                let image = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                let time = from + f64::from(n) / f64::from(PLAY_FPS);
                if tx.send((time, image)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
            // The window learns the end from the closed channel.
            drop(tx);
            ctx.request_repaint();
        });
        Ok(Self {
            _child: child,
            frames: rx,
            last_time: from,
        })
    }
}

/// The size frames are played at: the preview width at most, even sides.
fn play_size(source: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = (source.0.max(2), source.1.max(2));
    let w = sw.min(PREVIEW_WIDTH) & !1;
    let h = ((u64::from(w) * u64::from(sh) / u64::from(sw)) as u32 & !1).max(2);
    (w, h)
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

/// Paints the frame turned and mirrored as `orientation` says, filling `rect`.
fn paint_oriented(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    rect: ScreenRect,
    orientation: Orientation,
) {
    let mut mesh = egui::Mesh::with_texture(texture_id);
    // Each corner of the frame goes where the orientation sends it.
    for uv in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        let (x, y) = orientation.map_point(uv);
        mesh.vertices.push(egui::epaint::Vertex {
            pos: rect.lerp_inside(Vec2::new(x, y)),
            uv: Pos2::new(uv.0, uv.1),
            color: Color32::WHITE,
        });
    }
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
            player: None,
        }
    }

    fn is_playing(&self) -> bool {
        self.player.is_some()
    }

    /// Plays from the playhead (from the start once at the end), or pauses.
    fn toggle_play(&mut self, ctx: &egui::Context) {
        if self.player.take().is_some() {
            return;
        }
        let Some(source) = self.source_size() else {
            return;
        };
        if self.time >= self.duration() - 0.05 {
            self.time = 0.0;
        }
        match Player::start(&self.ffmpeg, &self.path, self.time, source, ctx) {
            Ok(player) => self.player = Some(player),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Shows the newest played frame; restarts playing after a seek.
    fn poll_player(&mut self, ctx: &egui::Context) {
        let duration = self.duration();
        let Some(player) = &mut self.player else {
            return;
        };
        if (self.time - player.last_time).abs() > 1e-6 {
            // The user moved the playhead: carry on playing from there.
            self.player = None;
            self.toggle_play(ctx);
            return;
        }
        let mut newest = None;
        let ended = loop {
            match player.frames.try_recv() {
                Ok(frame) => newest = Some(frame),
                Err(TryRecvError::Empty) => break false,
                Err(TryRecvError::Disconnected) => break true,
            }
        };
        if let Some((time, image)) = newest {
            self.texture = Some(ctx.load_texture("clip_preview", image, TextureOptions::LINEAR));
            self.time = time.min(duration);
            self.shown_time = Some(self.time);
            player.last_time = self.time;
        } else if ended {
            self.player = None;
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
        self.poll_player(ctx);
        let wanted = self.time;
        let stale = self.shown_time.is_none_or(|t| (t - wanted).abs() > 0.01);
        if self.info.is_some() && self.frame_rx.is_none() && stale && !self.is_playing() {
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
            .default_width(WINDOW_DISPLAY_WIDTH + 32.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(WINDOW_DISPLAY_WIDTH);
                dir::vertical(ui, |ui| {
                    self.contents(
                        ui,
                        edit,
                        rect,
                        &mut Vec::new(),
                        Mode::BOTH,
                        DISPLAY_MAX_HEIGHT,
                        edit.rotation.map(Orientation::from).as_mut(),
                    );
                });
            });
        open
    }

    /// Draws the preview inside the current layout, for tools whose main job is
    /// picking a part of the video. The picture is at most `max_height` tall;
    /// `marks` are the moments [`Mode::marks`] lets the user choose, and
    /// `orientation` how the picture is turned ([`Mode::rotate`] changes it).
    #[allow(clippy::too_many_arguments)]
    pub fn inline(
        &mut self,
        ui: &mut egui::Ui,
        edit: &mut Edit,
        rect: &mut Option<Rect>,
        marks: &mut Vec<f64>,
        mode: Mode,
        max_height: f32,
        orientation: Option<&mut Orientation>,
    ) {
        self.poll(ui.ctx());
        // Centred in the panel, like a video in GNOME's players.
        ui.vertical_centered(|ui| {
            self.contents(ui, edit, rect, marks, mode, max_height, orientation);
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        edit: &mut Edit,
        rect: &mut Option<Rect>,
        marks: &mut Vec<f64>,
        mode: Mode,
        max_height: f32,
        mut orientation: Option<&mut Orientation>,
    ) {
        let p = theme::palette(ui);
        let turned = orientation.as_deref().copied().unwrap_or_default();
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
        let swaps = turned.swaps_dimensions();
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
            if !turned.is_unchanged() {
                paint_oriented(&painter, texture.id(), shown, turned);
            } else {
                painter.image(
                    texture.id(),
                    shown,
                    ScreenRect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        // The clock sits on the picture, as in a video player; a narrow
        // picture shows only where the playhead is.
        let clock_font = egui::FontId::monospace(12.0);
        let mut clock = painter.layout_no_wrap(
            format!(
                "{} / {}",
                display_time(self.time),
                display_time(self.duration())
            ),
            clock_font.clone(),
            Color32::WHITE,
        );
        if clock.size().x + 32.0 > shown.width() {
            clock = painter.layout_no_wrap(display_time(self.time), clock_font, Color32::WHITE);
        }
        let at = shown.right_bottom() - Vec2::new(10.0, 8.0) - clock.size();
        let badge = ScreenRect::from_min_size(at, clock.size()).expand2(Vec2::new(6.0, 3.0));
        painter.rect_filled(badge, CornerRadius::same(4), Color32::from_black_alpha(160));
        painter.galley(at, clock, Color32::WHITE);
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
        self.timeline(ui, width, edit, mode.clip, mode.marks.then_some(&*marks));
        ui.add_space(4.0);
        self.shortcuts(ui, duration);
        // Like GNOME's players: skip back, play and skip forward, centred
        // under the picture, then a row with the few actions this tool needs.
        // Left to right in every language, like the timeline above.
        let gap = ui.spacing().item_spacing.x;
        let icon_w = |ui: &egui::Ui, icon| widgets::button_width(ui, Some(icon), "", Size::Regular);
        let transport_w = icon_w(ui, icons::SKIP_BACK)
            + icon_w(ui, icons::PLAY)
            + icon_w(ui, icons::SKIP_FORWARD);
        centered_row(ui, width, transport_w + 2.0 * gap, |ui| {
            let skip = |ui: &mut egui::Ui, icon, hint| {
                widgets::button(ui, Some(icon), "", Kind::Ghost, Size::Regular)
                    .on_hover_text(hint)
                    .clicked()
            };
            if skip(ui, icons::SKIP_BACK, tr("preview.back")) {
                self.time = (self.time - SKIP_S).max(0.0);
            }
            let (icon, hint) = if self.is_playing() {
                (icons::PAUSE, tr("preview.pause"))
            } else {
                (icons::PLAY, tr("preview.play"))
            };
            if widgets::button(ui, Some(icon), "", Kind::Primary, Size::Regular)
                .on_hover_text(hint)
                .clicked()
            {
                self.toggle_play(ui.ctx());
            }
            if skip(ui, icons::SKIP_FORWARD, tr("preview.forward")) {
                self.time = (self.time + SKIP_S).min(duration);
            }
        });

        if mode.clip {
            ui.add_space(4.0);
            self.clip_controls(ui, width, edit, duration);
        }

        // Clearing is offered only once there is something to clear.
        let rotating = mode.rotate && orientation.is_some();
        let has_edit = (mode.area && rect.is_some()) || (rotating && !turned.is_unchanged());
        // Turning and flipping, as in GNOME's image viewer: icon buttons.
        let turns = [
            (
                icons::ARROW_COUNTER_CLOCKWISE,
                tr("rotation.rotate_ccw"),
                Orientation::rotate_ccw as fn(Orientation) -> Orientation,
            ),
            (
                icons::ARROW_CLOCKWISE,
                tr("rotation.rotate_cw"),
                Orientation::rotate_cw,
            ),
            (
                icons::FLIP_HORIZONTAL,
                tr("rotation.flip_h"),
                Orientation::flip_horizontal,
            ),
            (
                icons::FLIP_VERTICAL,
                tr("rotation.flip_v"),
                Orientation::flip_vertical,
            ),
        ];
        let mut actions_w = 0.0;
        let mut add_w = |w: f32| actions_w += w + if actions_w > 0.0 { gap } else { 0.0 };
        if rotating {
            for (icon, _, _) in &turns {
                add_w(icon_w(ui, icon));
            }
        }
        if mode.marks {
            add_w(widgets::button_width(
                ui,
                Some(icons::PLUS),
                tr("preview.add_moment"),
                Size::Regular,
            ));
        }
        if has_edit {
            add_w(icon_w(ui, icons::X));
        }
        if actions_w > 0.0 {
            ui.add_space(4.0);
            centered_row(ui, width, actions_w, |ui| {
                if let Some(o) = orientation.as_deref_mut().filter(|_| rotating) {
                    for (icon, hint, turn) in turns {
                        if widgets::button(ui, Some(icon), "", Kind::Secondary, Size::Regular)
                            .on_hover_text(hint)
                            .clicked()
                        {
                            *o = turn(*o);
                        }
                    }
                }
                if mode.marks
                    && widgets::button(
                        ui,
                        Some(icons::PLUS),
                        tr("preview.add_moment"),
                        Kind::Secondary,
                        Size::Regular,
                    )
                    .on_hover_text(tr("preview.add_moment_hint"))
                    .clicked()
                {
                    add_frame_time(marks, self.time);
                }
                if has_edit
                    && widgets::button(ui, Some(icons::X), "", Kind::Ghost, Size::Regular)
                        .on_hover_text(tr("preview.clear"))
                        .clicked()
                {
                    if mode.area {
                        *rect = None;
                    }
                    if let Some(o) = orientation.filter(|_| rotating) {
                        *o = Orientation::default();
                    }
                }
            });
        }
        ui.add_space(4.0);
        let mut lines = Vec::new();
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
            let (out_w, out_h) = turned.transformed_size(source.0, source.1);
            let shape = if out_h > out_w {
                tr("rotation.portrait")
            } else {
                tr("rotation.landscape")
            };
            lines.push(format!(
                "{}  •  {out_w} × {out_h} ({shape})",
                turned.describe()
            ));
        }
        for line in lines {
            ui.label(RichText::new(line).color(p.text_weak));
        }
    }

    /// The clip's in and out points, as in a video editor: set either one
    /// to the playhead, or type it; the clip's length sits between them.
    fn clip_controls(&mut self, ui: &mut egui::Ui, width: f32, edit: &mut Edit, duration: f64) {
        let p = theme::palette(ui);
        let (start, end) = clip_range(edit, duration);
        let empty = end <= start;
        let length = RichText::new(display_time((end - start).max(0.0))).color(p.text_weak);
        let length_w = ui
            .painter()
            .layout_no_wrap(
                length.text().to_owned(),
                egui::TextStyle::Body.resolve(ui.style()),
                p.text_weak,
            )
            .size()
            .x;
        let gap = ui.spacing().item_spacing.x;
        let icon_w = widgets::button_width(ui, Some(icons::ARROW_LINE_LEFT), "", Size::Regular);
        let clear = edit.has_trim();
        let content = 2.0 * (icon_w + TIME_FIELD_WIDTH)
            + length_w
            + 2.0 * CLIP_GAP
            + 4.0 * gap
            + if clear { icon_w + gap } else { 0.0 };
        centered_row(ui, width, content, |ui| {
            if widgets::button(
                ui,
                Some(icons::ARROW_LINE_LEFT),
                "",
                Kind::Ghost,
                Size::Regular,
            )
            .on_hover_text(tr("preview.set_start_hint"))
            .clicked()
            {
                edit.trim_start = compact_timestamp(self.time);
            }
            time_field(ui, &mut edit.trim_start, &display_time(0.0), empty);
            ui.add_space(CLIP_GAP);
            ui.label(length).on_hover_text(tr("preview.length"));
            ui.add_space(CLIP_GAP);
            time_field(ui, &mut edit.trim_end, &display_time(duration), empty);
            if widgets::button(
                ui,
                Some(icons::ARROW_LINE_RIGHT),
                "",
                Kind::Ghost,
                Size::Regular,
            )
            .on_hover_text(tr("preview.set_end_hint"))
            .clicked()
            {
                edit.trim_end = compact_timestamp(self.time);
            }
            if clear
                && widgets::button(ui, Some(icons::X), "", Kind::Ghost, Size::Regular)
                    .on_hover_text(tr("preview.clear"))
                    .clicked()
            {
                edit.trim_start.clear();
                edit.trim_end.clear();
            }
        });
    }

    /// Space plays or pauses and the arrow keys step a second, while no
    /// field is being typed in.
    fn shortcuts(&mut self, ui: &egui::Ui, duration: f64) {
        if ui.memory(|m| m.focused().is_some()) {
            return;
        }
        let (space, left, right) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Space),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight),
            )
        });
        if space {
            self.toggle_play(ui.ctx());
        }
        if left {
            self.time = (self.time - 1.0).max(0.0);
        }
        if right {
            self.time = (self.time + 1.0).min(duration);
        }
    }

    /// A scrubbable track. With `clip`, the kept part is highlighted and its two
    /// ends can be dragged; anywhere else moves the playhead. `marks` are
    /// drawn as ticks.
    fn timeline(
        &mut self,
        ui: &mut egui::Ui,
        width: f32,
        edit: &mut Edit,
        clip: bool,
        marks: Option<&Vec<f64>>,
    ) {
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
                    edit.trim_start = compact_timestamp(t);
                    self.time = t;
                }
                Grab::End => {
                    let t = t.max(start + MIN_CLIP_S).min(duration);
                    edit.trim_end = compact_timestamp(t);
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
        for t in marks.into_iter().flatten() {
            let tick = ScreenRect::from_center_size(
                Pos2::new(x_of(*t), track.center().y),
                Vec2::new(4.0, track.height() - 4.0),
            );
            painter.rect_filled(tick, CornerRadius::same(2), p.accent);
        }
        let x = x_of(self.time);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(2.0, p.text),
        );
        painter.circle_filled(Pos2::new(x, rect.top() + 2.0), 4.0, p.text);
    }
}

/// A typed in or out point; empty means the start or end of the video,
/// shown as `hint`. It is outlined in red when it cannot be read, or when the
/// clip it makes is empty.
fn time_field(ui: &mut egui::Ui, text: &mut String, hint: &str, empty_clip: bool) {
    let p = theme::palette(ui);
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(TIME_FIELD_WIDTH)
            .align(egui::Align2::CENTER_CENTER),
    );
    let problem = if parse_timestamp(text).is_err() {
        Some(trf("error.bad_timestamp", &[("value", &text.trim())]))
    } else if empty_clip && !text.trim().is_empty() {
        Some(tr("error.empty_trim").to_owned())
    } else {
        None
    };
    match problem {
        Some(problem) => {
            ui.painter().rect_stroke(
                response.rect,
                CornerRadius::same(theme::RADIUS),
                Stroke::new(1.5, p.danger),
                StrokeKind::Inside,
            );
            response.on_hover_text(problem);
        }
        None => {
            response.on_hover_text(tr("hint.timestamp"));
        }
    }
}

/// A left-to-right row of `content_w` points, centred in `width`.
fn centered_row(ui: &mut egui::Ui, width: f32, content_w: f32, add: impl FnOnce(&mut egui::Ui)) {
    let row = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(Vec2::new(width, theme::BUTTON_HEIGHT), row, |ui| {
        ui.add_space(((width - content_w) / 2.0).max(0.0));
        add(ui);
    });
}

/// The kept part as `(start, end)` seconds inside the video; empty fields
/// mean the whole file, and an end past the video stops at its end.
fn clip_range(edit: &Edit, duration: f64) -> (f64, f64) {
    let start = parse_timestamp(&edit.trim_start)
        .ok()
        .flatten()
        .unwrap_or(0.0)
        .min(duration);
    let end = parse_timestamp(&edit.trim_end)
        .ok()
        .flatten()
        .unwrap_or(duration)
        .min(duration);
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_range_stays_inside_the_video() {
        let edit = Edit {
            trim_start: "1.2".into(),
            trim_end: "3:9".into(),
            ..Edit::default()
        };
        assert_eq!(clip_range(&edit, 4.0), (1.2, 4.0));
        assert_eq!(clip_range(&Edit::default(), 4.0), (0.0, 4.0));
    }

    #[test]
    fn played_frames_keep_the_shape_with_even_sides() {
        assert_eq!(play_size((1920, 1080)), (1280, 720));
        assert_eq!(play_size((641, 361)), (640, 360));
        assert_eq!(play_size((1080, 1920)), (1080, 1920));
    }

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
