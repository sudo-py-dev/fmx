//! Custom widgets with animated hover/press states: buttons, sidebar items,
//! status chips, smooth progress bars, empty states, and toast notifications.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Rect, Response, Sense, Stroke,
    StrokeKind, TextStyle, Vec2, WidgetInfo,
};

use crate::theme::{
    self, BUTTON_HEIGHT, MOTION, Palette, RADIUS, SMALL_BUTTON_HEIGHT, VALUE_MOTION,
};

const FOCUS_RING: f32 = 2.0;
const DISABLED_OPACITY: f32 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The main action of a view: filled with the accent color.
    Primary,
    /// Ordinary actions.
    Secondary,
    /// Low-emphasis actions in dense places such as table rows.
    Ghost,
    /// Destructive or stopping actions.
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Regular,
    Small,
}

/// Fill and text colors for (rest, hover, pressed).
fn colors(kind: Kind, p: &Palette) -> ([Color32; 3], Color32, Option<Color32>) {
    match kind {
        Kind::Primary => (
            [p.accent, p.accent_hover, p.accent_pressed],
            p.on_accent,
            None,
        ),
        Kind::Secondary => (
            [p.control, p.control_hover, p.control_pressed],
            p.text,
            Some(p.border),
        ),
        Kind::Ghost => (
            [Color32::TRANSPARENT, p.control_hover, p.control_pressed],
            p.text,
            None,
        ),
        Kind::Danger => (
            [
                p.control,
                p.danger.gamma_multiply(0.18),
                p.danger.gamma_multiply(0.3),
            ],
            p.danger,
            Some(p.border),
        ),
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

/// Animation progress of a boolean state, respecting the global motion setting.
fn animate(ui: &egui::Ui, id: Id, on: bool, seconds: f32) -> f32 {
    let time = if ui.style().animation_time == 0.0 {
        0.0
    } else {
        seconds
    };
    ui.ctx().animate_bool_with_time(id, on, time)
}

/// A standard button with an optional leading icon.
pub fn button(
    ui: &mut egui::Ui,
    icon: Option<&str>,
    text: &str,
    kind: Kind,
    size: Size,
    enabled: bool,
) -> Response {
    let p = theme::palette(ui);
    let font = TextStyle::Button.resolve(ui.style());
    let font = if size == Size::Small {
        FontId::new(font.size - 1.5, font.family)
    } else {
        font
    };
    let (fills, text_color, border) = colors(kind, &p);

    let label = match icon {
        Some(icon) if !text.is_empty() => format!("{icon}  {text}"),
        Some(icon) => icon.to_owned(),
        None => text.to_owned(),
    };
    let galley = ui.painter().layout_no_wrap(label, font, text_color);
    let height = if size == Size::Small {
        SMALL_BUTTON_HEIGHT
    } else {
        BUTTON_HEIGHT
    };
    let pad = if size == Size::Small { 9.0 } else { 14.0 };
    let width = (galley.size().x + pad * 2.0).max(height);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, enabled, text));

    if ui.is_rect_visible(rect) {
        let id = response.id;
        let hover = animate(ui, id.with("hover"), enabled && response.hovered(), MOTION);
        let press = animate(
            ui,
            id.with("press"),
            enabled && response.is_pointer_button_down_on(),
            MOTION * 0.5,
        );
        let fill = mix(mix(fills[0], fills[1], hover), fills[2], press);
        // A subtle "push" when pressed.
        let rect = rect.shrink(press * 0.8);
        let opacity = if enabled { 1.0 } else { DISABLED_OPACITY };
        let painter = ui.painter().with_clip_rect(ui.clip_rect());
        let stroke = border.map_or(Stroke::NONE, |c| {
            Stroke::new(1.0, mix(c, p.accent, hover * 0.5).gamma_multiply(opacity))
        });
        painter.rect(
            rect,
            CornerRadius::same(RADIUS),
            fill.gamma_multiply(opacity),
            stroke,
            StrokeKind::Inside,
        );
        if response.has_focus() {
            painter.rect_stroke(
                rect.expand(FOCUS_RING),
                CornerRadius::same(RADIUS + 2),
                Stroke::new(FOCUS_RING, p.accent),
                StrokeKind::Outside,
            );
        }
        let text_pos = rect.center() - galley.size() / 2.0;
        painter.galley_with_override_text_color(
            text_pos,
            galley,
            text_color.gamma_multiply(opacity),
        );
    }
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}

/// A full-width sidebar entry with an animated highlight and accent bar.
pub fn nav_item(ui: &mut egui::Ui, text: &str, selected: bool, dimmed: bool) -> Response {
    let p = theme::palette(ui);
    let font = TextStyle::Body.resolve(ui.style());
    let color = if dimmed { p.text_weak } else { p.text };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let width = ui.available_width().max(galley.size().x + 24.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
    response.widget_info(|| WidgetInfo::selected(egui::Role::Tab, true, selected, text));
    if ui.is_rect_visible(rect) {
        let id = response.id;
        let hover = animate(ui, id.with("hover"), response.hovered(), MOTION);
        let active = animate(ui, id.with("selected"), selected, MOTION * 1.5);
        let fill = mix(
            mix(Color32::TRANSPARENT, p.control_hover, hover),
            p.accent.gamma_multiply(0.22),
            active,
        );
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(6), fill);
        if active > 0.0 {
            let bar_height = rect.height() * 0.55 * active;
            let bar = Rect::from_center_size(
                rect.left_center() + Vec2::new(2.0, 0.0),
                Vec2::new(3.0, bar_height),
            );
            painter.rect_filled(bar, CornerRadius::same(2), p.accent);
        }
        let text_color = mix(
            color,
            if ui.visuals().dark_mode {
                Color32::WHITE
            } else {
                p.accent_pressed
            },
            active * 0.6,
        );
        let pos = rect.left_center() + Vec2::new(12.0 + 2.0 * hover, -galley.size().y / 2.0);
        painter.galley_with_override_text_color(pos, galley, text_color);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A rounded, tinted pill with an icon and short text.
pub fn chip(ui: &mut egui::Ui, icon: &str, text: &str, color: Color32) -> Response {
    let font = TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(
        format!("{icon} {text}"),
        FontId::new(font.size + 0.5, font.family),
        color,
    );
    let size = galley.size() + Vec2::new(16.0, 8.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(255), color.gamma_multiply(0.16));
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    response
}

/// A progress bar whose value glides toward its target instead of jumping.
pub fn progress(ui: &mut egui::Ui, id: Id, target: f32, text: String, width: f32) -> Response {
    let p = theme::palette(ui);
    let time = if ui.style().animation_time == 0.0 {
        0.0
    } else {
        VALUE_MOTION
    };
    let shown = ui
        .ctx()
        .animate_value_with_time(id, target.clamp(0.0, 1.0), time);
    ui.add(
        egui::ProgressBar::new(shown)
            .desired_width(width)
            .desired_height(20.0)
            .corner_radius(CornerRadius::same(255))
            .fill(p.accent)
            .text(text),
    )
}

/// A centered icon with a message, for empty lists.
pub fn empty_state(ui: &mut egui::Ui, icon: &str, text: &str, height: f32) {
    let p = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let painter = ui.painter();
    painter.text(
        rect.center() - Vec2::new(0.0, 12.0),
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(30.0),
        p.text_weak.gamma_multiply(0.7),
    );
    painter.text(
        rect.center() + Vec2::new(0.0, 18.0),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(14.0),
        p.text_weak,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
}

struct Toast {
    text: String,
    kind: ToastKind,
    born: Instant,
}

/// Short-lived notifications in the bottom-right corner that fade in and out.
#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

const TOAST_LIFETIME: Duration = Duration::from_secs(4);
const TOAST_ERROR_LIFETIME: Duration = Duration::from_secs(8);
const TOAST_FADE: f32 = 0.25;
const MAX_TOASTS: usize = 4;

impl Toasts {
    pub fn push(&mut self, kind: ToastKind, text: impl Into<String>) {
        if self.items.len() == MAX_TOASTS {
            self.items.remove(0);
        }
        self.items.push(Toast {
            text: text.into(),
            kind,
            born: Instant::now(),
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let lifetime = |kind| {
            if kind == ToastKind::Error {
                TOAST_ERROR_LIFETIME
            } else {
                TOAST_LIFETIME
            }
        };
        self.items.retain(|t| t.born.elapsed() < lifetime(t.kind));
        if self.items.is_empty() {
            return;
        }
        let reduce_motion = ctx.global_style().animation_time == 0.0;
        let mut dismissed = None;
        egui::Area::new(Id::unique("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                let p = theme::palette(ui);
                for (i, toast) in self.items.iter().enumerate() {
                    let age = toast.born.elapsed().as_secs_f32();
                    let left = lifetime(toast.kind).as_secs_f32() - age;
                    let alpha = if reduce_motion {
                        1.0
                    } else {
                        (age / TOAST_FADE).min(left / TOAST_FADE).clamp(0.0, 1.0)
                    };
                    let (icon, accent) = match toast.kind {
                        ToastKind::Success => (crate::icons::CHECK_CIRCLE, p.success),
                        ToastKind::Error => (crate::icons::WARNING, p.danger),
                    };
                    ui.scope(|ui| {
                        ui.multiply_opacity(alpha);
                        // Slide up slightly while fading in.
                        ui.add_space((1.0 - alpha) * 8.0);
                        let frame = egui::Frame::popup(ui.style())
                            .fill(p.surface)
                            .stroke(Stroke::new(1.0, accent.gamma_multiply(0.6)));
                        let response = frame
                            .show(ui, |ui| {
                                ui.set_max_width(380.0);
                                ui.horizontal(|ui| {
                                    ui.colored_label(accent, egui::RichText::new(icon).size(18.0));
                                    ui.label(&toast.text);
                                });
                            })
                            .response
                            .interact(Sense::click());
                        if response.clicked() {
                            dismissed = Some(i);
                        }
                    });
                }
            });
        if let Some(i) = dismissed {
            self.items.remove(i);
        }
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
