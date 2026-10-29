//! Custom widgets with animated hover/press states: buttons, sidebar items,
//! status chips, smooth progress bars, empty states, and toast notifications.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, emath, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Pos2, Rect, Response, Sense,
    Stroke, StrokeKind, TextStyle, Vec2, WidgetInfo,
};

use crate::art::Art;
use crate::dir;
use crate::theme::{
    self, BUTTON_HEIGHT, MOTION, Palette, RADIUS, SMALL_BUTTON_HEIGHT, VALUE_MOTION,
};

const LARGE_BUTTON_HEIGHT: f32 = 40.0;
const SEGMENT_HEIGHT: f32 = 28.0;
const SEGMENT_PAD: f32 = 12.0;
/// The track's margin around a toggle group's items.
const SEGMENT_INSET: f32 = 3.0;
const STEP_BADGE: f32 = 28.0;

const FOCUS_RING: f32 = 2.0;
const ICON_GAP: f32 = 7.0;
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
    /// The one main action of a screen.
    Large,
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
            None,
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
            None,
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
) -> Response {
    button_tinted(ui, icon, None, text, kind, size)
}

/// A button's laid-out icon and text, and its size.
struct ButtonContent {
    icon: Option<std::sync::Arc<egui::Galley>>,
    text: Option<std::sync::Arc<egui::Galley>>,
    gap: f32,
    content_w: f32,
    size: Vec2,
}

fn button_content(
    ui: &egui::Ui,
    icon: Option<&str>,
    text: &str,
    size: Size,
    color: Color32,
) -> ButtonContent {
    let base = TextStyle::Button.resolve(ui.style());
    let font = match size {
        Size::Small => FontId::new(base.size - 1.5, base.family.clone()),
        Size::Large => FontId::new(base.size + 1.0, base.family.clone()),
        Size::Regular => base,
    };
    let icon_galley = icon.map(|i| {
        ui.painter().layout_no_wrap(
            i.to_owned(),
            FontId::new(font.size + 1.5, font.family.clone()),
            color,
        )
    });
    let text_galley =
        (!text.is_empty()).then(|| ui.painter().layout_no_wrap(text.to_owned(), font, color));
    let gap = if icon_galley.is_some() && text_galley.is_some() {
        ICON_GAP
    } else {
        0.0
    };
    let content_w = icon_galley.as_ref().map_or(0.0, |g| g.size().x)
        + gap
        + text_galley.as_ref().map_or(0.0, |g| g.size().x);
    let (height, pad) = match size {
        Size::Small => (SMALL_BUTTON_HEIGHT, 9.0),
        Size::Regular => (BUTTON_HEIGHT, 14.0),
        Size::Large => (LARGE_BUTTON_HEIGHT, 22.0),
    };
    ButtonContent {
        icon: icon_galley,
        text: text_galley,
        gap,
        content_w,
        size: Vec2::new((content_w + pad * 2.0).max(height), height),
    }
}

/// How wide [`button`] draws this icon and text, to lay out around it beforehand.
pub fn button_width(ui: &egui::Ui, icon: Option<&str>, text: &str, size: Size) -> f32 {
    button_content(ui, icon, text, size, Color32::PLACEHOLDER)
        .size
        .x
}

/// Like [`button`], with the icon drawn in its own color.
pub fn button_tinted(
    ui: &mut egui::Ui,
    icon: Option<&str>,
    tint: Option<Color32>,
    text: &str,
    kind: Kind,
    size: Size,
) -> Response {
    // Disabled through `ui.add_enabled_ui`, like egui's own widgets.
    let enabled = ui.is_enabled();
    let p = theme::palette(ui);
    let (fills, text_color, border) = colors(kind, &p);
    let ButtonContent {
        icon: icon_galley,
        text: text_galley,
        gap,
        content_w,
        size: button_size,
    } = button_content(ui, icon, text, size, text_color);
    let (width, height) = (button_size.x, button_size.y);
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
        let opacity = if enabled { 1.0 } else { DISABLED_OPACITY };
        let painter = ui.painter().with_clip_rect(ui.clip_rect());
        let stroke = border.map_or(Stroke::NONE, |c| {
            Stroke::new(1.0, c.gamma_multiply(opacity))
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
                Stroke::new(FOCUS_RING, p.accent.gamma_multiply(0.5)),
                StrokeKind::Outside,
            );
        }
        // The icon leads: left of the text, or right of it in right-to-left languages.
        let content = Rect::from_center_size(rect.center(), Vec2::new(content_w, 0.0));
        let mut x = content.min.x;
        if let Some(g) = icon_galley {
            let color = tint.filter(|_| kind != Kind::Primary).unwrap_or(text_color);
            let w = g.size().x;
            painter.galley_with_override_text_color(
                Pos2::new(
                    dir::mirror_x(content, x, w),
                    rect.center().y - g.size().y / 2.0,
                ),
                g,
                color.gamma_multiply(opacity),
            );
            x += w + gap;
        }
        if let Some(g) = text_galley {
            let w = g.size().x;
            painter.galley_with_override_text_color(
                Pos2::new(
                    dir::mirror_x(content, x, w),
                    rect.center().y - g.size().y / 2.0,
                ),
                g,
                text_color.gamma_multiply(opacity),
            );
        }
    }
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Fill and glyph colors for a tile in the current theme.
fn tile_colors(art: Art, solid: bool, dark: bool) -> (Color32, Color32) {
    if solid {
        (art.hue, Color32::WHITE)
    } else if dark {
        (
            art.hue.gamma_multiply(0.22),
            mix(art.hue, Color32::WHITE, 0.3),
        )
    } else {
        (
            art.hue.gamma_multiply(0.14),
            mix(art.hue, Color32::BLACK, 0.12),
        )
    }
}

/// Paints a rounded tile with a filled icon into `rect`.
pub fn paint_tile(painter: &egui::Painter, rect: Rect, art: Art, solid: bool, dark: bool) {
    let (fill, fg) = tile_colors(art, solid, dark);
    let radius = (rect.height() * 0.28).round().clamp(4.0, 14.0) as u8;
    painter.rect_filled(rect, CornerRadius::same(radius), fill);
    if solid {
        // A soft top highlight gives the solid tile some depth.
        let top = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.center().y));
        painter.rect_filled(
            top,
            CornerRadius {
                nw: radius,
                ne: radius,
                sw: 0,
                se: 0,
            },
            Color32::from_white_alpha(22),
        );
    }
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        art.glyph,
        crate::icons::fill(rect.height() * 0.58),
        fg,
    );
}

/// A standalone icon tile.
pub fn icon_tile(ui: &mut egui::Ui, art: Art, size: f32, solid: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_tile(ui.painter(), rect, art, solid, ui.visuals().dark_mode);
    }
    response
}

/// A full-width sidebar row like GNOME Settings': a symbolic icon and its
/// label, on a soft neutral highlight when hovered or selected.
pub fn nav_item(
    ui: &mut egui::Ui,
    art: Option<Art>,
    text: &str,
    selected: bool,
    dimmed: bool,
    badge: Option<usize>,
) -> Response {
    let p = theme::palette(ui);
    let font = TextStyle::Body.resolve(ui.style());
    let color = if dimmed { p.text_weak } else { p.text };
    let height = if art.is_some() { 40.0 } else { 32.0 };
    let width = ui.available_width();
    let reserved =
        12.0 + if art.is_some() { 30.0 } else { 0.0 } + if badge.is_some() { 44.0 } else { 12.0 };
    let galley = elided(ui, text, font, color, width - reserved);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    response.widget_info(|| WidgetInfo::selected(egui::Role::Tab, true, selected, text));
    if ui.is_rect_visible(rect) {
        let id = response.id;
        let hover = animate(ui, id.with("hover"), response.hovered(), MOTION);
        let press = animate(
            ui,
            id.with("press"),
            response.is_pointer_button_down_on(),
            MOTION * 0.5,
        );
        let active = animate(ui, id.with("selected"), selected, MOTION);
        // Adwaita's sidebar: 6 % ink on hover, 10 % when selected, more while pressed.
        let ink = if ui.visuals().dark_mode {
            Color32::WHITE
        } else {
            Color32::from_rgb(0, 0, 6)
        };
        let alpha = (0.06 * hover).max(0.1 * active) + 0.08 * press;
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(RADIUS), ink.gamma_multiply(alpha));
        if response.has_focus() {
            painter.rect_stroke(
                rect,
                CornerRadius::same(RADIUS),
                Stroke::new(FOCUS_RING, p.accent.gamma_multiply(0.5)),
                StrokeKind::Inside,
            );
        }
        // Laid out left to right, then mirrored for right-to-left languages.
        let mut x = rect.left() + 12.0;
        if let Some(art) = art {
            let glyph = painter.layout_no_wrap(
                art.glyph.to_owned(),
                crate::icons::fill(17.0),
                color,
            );
            painter.galley_with_override_text_color(
                Pos2::new(
                    dir::mirror_x(rect, x + (18.0 - glyph.size().x) / 2.0, glyph.size().x),
                    rect.center().y - glyph.size().y / 2.0,
                ),
                glyph,
                color,
            );
            x += 18.0 + 12.0;
        }
        painter.galley_with_override_text_color(
            Pos2::new(
                dir::mirror_x(rect, x, galley.size().x),
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            color,
        );
        if let Some(count) = badge {
            let label = ui.painter().layout_no_wrap(
                count.to_string(),
                FontId::proportional(11.5),
                p.on_accent,
            );
            let pill = Rect::from_center_size(
                rect.right_center() - Vec2::new(10.0 + label.size().x / 2.0 + 6.0, 0.0),
                Vec2::new((label.size().x + 12.0).max(20.0), 20.0),
            );
            let pill = pill.translate(Vec2::new(
                dir::mirror_x(rect, pill.min.x, pill.width()) - pill.min.x,
                0.0,
            ));
            painter.rect_filled(pill, CornerRadius::same(255), p.accent);
            painter.galley_with_override_text_color(
                pill.center() - label.size() / 2.0,
                label,
                p.on_accent,
            );
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Lays out one line of text, ending in "…" if it doesn't fit.
fn elided(
    ui: &egui::Ui,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(font, color),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: max_width.max(20.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

/// The soft shadow under a libadwaita card; dark cards are lit instead.
pub fn card_shadow(dark: bool) -> egui::Shadow {
    egui::Shadow {
        offset: [0, 1],
        blur: if dark { 0 } else { 4 },
        spread: 0,
        color: Color32::from_black_alpha(if dark { 0 } else { 20 }),
    }
}

/// The hairline around a card.
pub fn card_stroke(p: &Palette, dark: bool) -> Stroke {
    let ink = if dark {
        Color32::from_white_alpha(10)
    } else {
        Color32::from_black_alpha(14)
    };
    let _ = p;
    Stroke::new(1.0, ink)
}

/// Paints an empty card into `rect`.
pub fn paint_card(painter: &egui::Painter, rect: Rect, p: &Palette, dark: bool) {
    let radius = CornerRadius::same(theme::CARD_RADIUS);
    painter.add(card_shadow(dark).as_shape(rect, radius));
    painter.rect(rect, radius, p.surface, card_stroke(p, dark), StrokeKind::Inside);
}

/// A tool card's smallest size; cards grow to fill the row evenly.
pub const CARD_SIZE: Vec2 = Vec2::new(172.0, 104.0);
/// The widest a tool card grows.
pub const CARD_MAX_WIDTH: f32 = 260.0;

/// A tool card for the tool grid, flat like a libadwaita card: hovering and
/// pressing only darken it a little.
pub fn tool_card(
    ui: &mut egui::Ui,
    width: f32,
    art: Art,
    title: &str,
    subtitle: &str,
    unavailable: bool,
) -> Response {
    let p = theme::palette(ui);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, CARD_SIZE.y), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, title));
    if ui.is_rect_visible(rect) {
        let hover = animate(ui, response.id.with("hover"), response.hovered(), MOTION);
        let press = animate(
            ui,
            response.id.with("press"),
            response.is_pointer_button_down_on(),
            MOTION * 0.5,
        );
        let card = rect.shrink(3.0);
        let painter = ui.painter();
        paint_card(painter, card, &p, ui.visuals().dark_mode);
        let ink = if ui.visuals().dark_mode {
            Color32::WHITE
        } else {
            Color32::from_rgb(0, 0, 6)
        };
        painter.rect_filled(
            card,
            CornerRadius::same(theme::CARD_RADIUS),
            ink.gamma_multiply(0.03 * hover + 0.05 * press),
        );
        if response.has_focus() {
            painter.rect_stroke(
                card,
                CornerRadius::same(theme::CARD_RADIUS),
                Stroke::new(FOCUS_RING, p.accent.gamma_multiply(0.5)),
                StrokeKind::Inside,
            );
        }
        let opacity = if unavailable { DISABLED_OPACITY } else { 1.0 };
        let tile_x = dir::mirror_x(card, card.min.x + 14.0, 38.0);
        let tile = Rect::from_min_size(Pos2::new(tile_x, card.min.y + 12.0), Vec2::splat(38.0));
        let dimmed = Art {
            hue: art.hue.gamma_multiply(opacity),
            ..art
        };
        paint_tile(painter, tile, dimmed, true, ui.visuals().dark_mode);
        let text_x = card.min.x + 14.0;
        let title_g = elided(
            ui,
            title,
            FontId::proportional(15.0),
            p.text,
            card.width() - 28.0,
        );
        let title_x = dir::mirror_x(card, text_x, title_g.size().x);
        painter.galley(Pos2::new(title_x, tile.max.y + 9.0), title_g, p.text);
        if !subtitle.is_empty() {
            let sub = elided(
                ui,
                subtitle,
                FontId::proportional(12.0),
                p.text_weak,
                card.width() - 28.0,
            );
            let sub_x = dir::mirror_x(card, text_x, sub.size().x);
            painter.galley(Pos2::new(sub_x, tile.max.y + 29.0), sub, p.text_weak);
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A rounded, tinted pill with an icon and short text.
pub fn chip(ui: &mut egui::Ui, icon: &str, text: &str, color: Color32) -> Response {
    let font = TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(
        format!("{}{icon} {text}", crate::i18n::direction_mark()),
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

/// Height of a progress bar.
const PROGRESS_HEIGHT: f32 = 20.0;

/// A progress bar whose value glides toward its target instead of jumping.
/// It fills from the start edge (the right in right-to-left languages).
pub fn progress(ui: &mut egui::Ui, id: Id, target: f32, text: String, width: f32) -> Response {
    let time = if ui.style().animation_time == 0.0 {
        0.0
    } else {
        VALUE_MOTION
    };
    let shown = ui
        .ctx()
        .animate_value_with_time(id, target.clamp(0.0, 1.0), time);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, PROGRESS_HEIGHT), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::ProgressIndicator, true, &text));
    if ui.is_rect_visible(rect) && shown > 0.0 {
        // At least a circle, so a little progress still shows.
        let filled = (rect.width() * shown).max(rect.height());
        let x = dir::mirror_x(rect, rect.min.x, filled);
        let fill = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(filled, rect.height()));
        paint_bar(ui, rect, Some(fill), text);
    } else if ui.is_rect_visible(rect) {
        paint_bar(ui, rect, None, text);
    }
    response
}

/// A bar for work whose progress is unknown: a short segment slides along it.
/// With reduced motion it stays still in the middle.
pub fn busy(ui: &mut egui::Ui, text: &str, width: f32) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, PROGRESS_HEIGHT), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::ProgressIndicator, true, text));
    if ui.is_rect_visible(rect) {
        let segment = rect.width() * 0.3;
        let travel = rect.width() - segment;
        let place = if ui.style().animation_time == 0.0 {
            0.5
        } else {
            ui.ctx().request_repaint();
            // Back and forth once every two seconds, easing at the ends.
            let t = (ui.input(|i| i.time) * std::f64::consts::PI).sin() as f32;
            0.5 + 0.5 * t
        };
        let fill = Rect::from_min_size(
            Pos2::new(rect.min.x + travel * place, rect.min.y),
            Vec2::new(segment, rect.height()),
        );
        paint_bar(ui, rect, Some(fill), text.to_owned());
    }
    response
}

/// A rounded track with `fill` on it and `text` in the middle, in two colors
/// (one over the track, one over the fill), so it reads wherever the fill is.
fn paint_bar(ui: &egui::Ui, rect: Rect, fill: Option<Rect>, text: String) {
    let p = theme::palette(ui);
    let painter = ui.painter().with_clip_rect(rect);
    let round = CornerRadius::same(255);
    // The controls' color: it stands out on plain and striped rows alike.
    painter.rect_filled(rect, round, p.control);
    let galley = painter.layout_no_wrap(
        text,
        TextStyle::Small.resolve(ui.style()),
        Color32::PLACEHOLDER,
    );
    let pos = rect.center() - galley.size() / 2.0;
    // Text in the track's color, then the fill over it, then the fill's part
    // of the text again in the color that reads on the fill.
    painter.galley_with_override_text_color(pos, galley.clone(), p.text);
    if let Some(fill) = fill {
        painter.rect_filled(fill, round, p.accent);
        painter
            .with_clip_rect(fill.intersect(rect))
            .galley_with_override_text_color(pos, galley, p.on_accent);
    }
}

/// A row of joined choices where exactly one is selected: radio buttons that
/// read as one control, drawn like libadwaita's toggle group with a raised
/// pill that slides to the chosen item. Returns `true` when the selection changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    choices: &[(T, &str)],
) -> bool {
    let p = theme::palette(ui);
    let dark = ui.visuals().dark_mode;
    let font = TextStyle::Button.resolve(ui.style());
    let mut changed = false;
    let group_id = ui.next_auto_id();
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.horizontal(|ui| {
            // The track goes under the items, so reserve its place first.
            let track = ui.painter().add(egui::Shape::Noop);
            let pill = ui.painter().add(egui::Shape::Noop);
            ui.add_space(SEGMENT_INSET);
            let mut outer = Rect::NOTHING;
            let mut chosen = None;
            for (choice, text) in choices {
                let selected = *value == *choice;
                let galley = ui
                    .painter()
                    .layout_no_wrap((*text).to_owned(), font.clone(), p.text);
                let size = Vec2::new(
                    galley.size().x + SEGMENT_PAD * 2.0,
                    SEGMENT_HEIGHT + SEGMENT_INSET * 2.0,
                );
                let (cell, response) = ui.allocate_exact_size(size, Sense::click());
                outer = outer.union(cell);
                let rect = cell.shrink2(Vec2::new(0.0, SEGMENT_INSET));
                response.widget_info(|| {
                    WidgetInfo::selected(egui::Role::RadioButton, true, selected, *text)
                });
                if response.clicked() && !selected {
                    *value = *choice;
                    changed = true;
                }
                if *value == *choice {
                    chosen = Some(rect);
                }
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let hover = animate(ui, response.id.with("hover"), response.hovered(), MOTION);
                let painter = ui.painter();
                if hover > 0.0 && *value != *choice {
                    painter.rect_filled(
                        rect.shrink(1.0),
                        CornerRadius::same(RADIUS - 1),
                        p.control_hover.gamma_multiply(hover),
                    );
                }
                if response.has_focus() {
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(RADIUS - 1),
                        Stroke::new(FOCUS_RING, p.accent.gamma_multiply(0.5)),
                        StrokeKind::Inside,
                    );
                }
                painter.galley_with_override_text_color(
                    rect.center() - galley.size() / 2.0,
                    galley,
                    p.text,
                );
                response.on_hover_cursor(CursorIcon::PointingHand);
            }
            ui.add_space(SEGMENT_INSET);
            let outer = outer.expand2(Vec2::new(SEGMENT_INSET, 0.0));
            ui.painter().set(
                track,
                egui::epaint::RectShape::filled(outer, CornerRadius::same(RADIUS + 2), p.control),
            );
            if let Some(target) = chosen {
                let time = if ui.style().animation_time == 0.0 {
                    0.0
                } else {
                    MOTION
                };
                let ctx = ui.ctx();
                let left = ctx.animate_value_with_time(group_id.with("l"), target.left(), time);
                let right = ctx.animate_value_with_time(group_id.with("r"), target.right(), time);
                let rect = Rect::from_x_y_ranges(left..=right, target.y_range());
                let raised = if dark { p.control_pressed } else { p.surface };
                let radius = CornerRadius::same(RADIUS);
                ui.painter().set(
                    pill,
                    egui::Shape::Vec(vec![
                        egui::Shadow {
                            offset: [0, 1],
                            blur: 3,
                            spread: 0,
                            color: Color32::from_black_alpha(if dark { 60 } else { 30 }),
                        }
                        .as_shape(rect, radius)
                        .into(),
                        egui::epaint::RectShape::filled(rect, radius, raised).into(),
                    ]),
                );
            }
        });
    });
    changed
}

/// How far a page slides in when going deeper or back, in points.
const PAGE_SLIDE: f32 = 48.0;

/// Shows `content` as a page that eases in whenever `key` changes, like
/// libadwaita's navigation: a deeper `depth` slides in from the end edge, a
/// shallower one from the start edge, and a page at the same depth fades in.
pub fn page_transition<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    key: impl std::hash::Hash + std::fmt::Debug,
    depth: u8,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let id = Id::unique(("page_transition", id_salt));
    let key = Id::unique(key);
    let now = ui.input(|i| i.time);
    // The shown page, its depth, when it arrived and which way it slid in.
    let previous: Option<(Id, u8, f64, f32)> = ui.data(|d| d.get_temp(id));
    let (start, slide) = match previous {
        Some((k, _, start, slide)) if k == key => (start, slide),
        Some((_, old_depth, ..)) => {
            let slide = f32::from(depth.cmp(&old_depth) as i8);
            ui.data_mut(|d| d.insert_temp(id, (key, depth, now, slide)));
            (now, slide)
        }
        // The first page shows as it is.
        None => {
            ui.data_mut(|d| d.insert_temp(id, (key, depth, f64::MIN, 0.0)));
            (f64::MIN, 0.0)
        }
    };
    let elapsed = ((now - start) as f32 / theme::PAGE_MOTION).clamp(0.0, 1.0);
    if elapsed >= 1.0 || ui.style().animation_time == 0.0 {
        return content(ui);
    }
    ui.ctx().request_repaint();
    let eased = emath::easing::cubic_out(elapsed);
    let toward_end = if dir::is_rtl() { -1.0 } else { 1.0 };
    let dx = slide * toward_end * PAGE_SLIDE * (1.0 - eased);
    let rect = ui.available_rect_before_wrap();
    let clip = ui.clip_rect();
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(rect.translate(Vec2::new(dx, 0.0))),
        |ui| {
            ui.set_clip_rect(clip);
            ui.multiply_opacity(0.2 + 0.8 * eased);
            content(ui)
        },
    )
    .inner
}

/// A numbered step marker: the number in a circle, or a check once the step is complete.
pub fn step_badge(ui: &mut egui::Ui, number: usize, done: bool) -> Response {
    let p = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(STEP_BADGE), Sense::hover());
    if ui.is_rect_visible(rect) {
        let t = animate(ui, response.id.with("done"), done, MOTION * 2.0);
        let fill = mix(p.accent, p.success, t);
        let painter = ui.painter();
        painter.circle_filled(rect.center(), STEP_BADGE / 2.0, fill);
        let number = number.to_string();
        let (text, font) = if t > 0.5 {
            (crate::icons::CHECK, FontId::proportional(16.0))
        } else {
            (number.as_str(), FontId::proportional(14.5))
        };
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            font,
            p.on_accent,
        );
    }
    response
}

/// A large click-or-drop target for adding files. It glows while the pointer is
/// over it or files are being dragged over the window.
pub fn drop_zone(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    subtitle: &str,
    detail: &str,
    files_hovering: bool,
    height: f32,
) -> Response {
    let p = theme::palette(ui);
    let size = Vec2::new(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, title));
    if ui.is_rect_visible(rect) {
        let hover = animate(
            ui,
            response.id.with("hover"),
            response.hovered() || files_hovering,
            MOTION,
        );
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(12),
            mix(p.sunken, p.accent, 0.03 + hover * 0.07),
            Stroke::new(1.0 + hover, mix(p.border, p.accent, 0.35 + hover * 0.65)),
            StrokeKind::Inside,
        );
        let c = rect.center();
        let lift = -3.0 * hover;
        painter.circle_filled(
            c + Vec2::new(0.0, -34.0 + lift),
            28.0,
            p.accent.gamma_multiply(0.14 + hover * 0.1),
        );
        painter.text(
            c + Vec2::new(0.0, -34.0 + lift),
            Align2::CENTER_CENTER,
            icon,
            FontId::proportional(28.0),
            p.accent,
        );
        painter.text(
            c + Vec2::new(0.0, 12.0),
            Align2::CENTER_CENTER,
            title,
            FontId::proportional(17.0),
            p.text,
        );
        painter.text(
            c + Vec2::new(0.0, 34.0),
            Align2::CENTER_CENTER,
            subtitle,
            FontId::proportional(13.5),
            p.text_weak,
        );
        if !detail.is_empty() {
            let galley = elided(
                ui,
                detail,
                FontId::proportional(12.0),
                p.text_weak,
                rect.width() - 40.0,
            );
            painter.galley(
                c + Vec2::new(-galley.size().x / 2.0, 52.0),
                galley,
                p.text_weak,
            );
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
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
            .anchor(
                if dir::is_rtl() {
                    Align2::LEFT_BOTTOM
                } else {
                    Align2::RIGHT_BOTTOM
                },
                Vec2::new(if dir::is_rtl() { 16.0 } else { -16.0 }, -16.0),
            )
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

/// Paints a sleek, modern chevron that rotates smoothly from pointing right (0°) to pointing
/// down (90°). In right-to-left languages it points left when closed.
pub fn paint_chevron(
    painter: &egui::Painter,
    center: Pos2,
    size: f32,
    openness: f32,
    stroke: Stroke,
) {
    let flip = if dir::is_rtl() { -1.0 } else { 1.0 };
    let angle = openness * (std::f32::consts::PI * 0.5);
    let cos = angle.cos();
    let sin = angle.sin();
    let rotate =
        |x: f32, y: f32| -> Vec2 { Vec2::new(flip * (x * cos - y * sin), x * sin + y * cos) };
    let half_w = size * 0.35;
    let half_h = size * 0.6;
    let p1 = center + rotate(-half_w, -half_h);
    let apex = center + rotate(half_w, 0.0);
    let p2 = center + rotate(-half_w, half_h);
    painter.line_segment([p1, apex], stroke);
    painter.line_segment([apex, p2], stroke);
}

/// A modern collapsible section card with a sleek rotating chevron, clean hover states,
/// and border framing to replace archaic disclosure triangles and vertical tree lines.
pub fn collapsible_section<R>(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    icon: Option<&str>,
    title: &str,
    badge: Option<&str>,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let p = theme::palette(ui);
    let id = ui.make_persistent_id(id_salt);
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, {
            #[cfg(debug_assertions)]
            let open = crate::devshot::expand_sections();
            #[cfg(not(debug_assertions))]
            let open = false;
            open
        });
    let is_open = state.is_open();
    let openness = state.openness(ui.ctx());

    let header_height = 38.0;
    let (header_rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), header_height),
        Sense::click(),
    );
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, title));

    if response.clicked() {
        state.set_open(!is_open);
    }

    if ui.is_rect_visible(header_rect) {
        let hover = animate(ui, id.with("hover"), response.hovered(), MOTION);
        let painter = ui.painter();

        // Header background card
        let r = RADIUS;
        let corners = if openness > 0.05 {
            CornerRadius {
                nw: r,
                ne: r,
                sw: 0,
                se: 0,
            }
        } else {
            CornerRadius::same(r)
        };
        let fill = mix(
            p.control.gamma_multiply(0.4),
            p.control_hover,
            hover.max(openness * 0.3),
        );
        let border = mix(
            p.border,
            p.accent.gamma_multiply(0.6),
            hover.max(openness * 0.7),
        );
        painter.rect(
            header_rect,
            corners,
            fill,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );

        // Chevron, icon and title from the start edge; the badge sits on the end edge.
        let mirror = |x: f32, w: f32| dir::mirror_x(header_rect, x, w);
        let chevron_center = Pos2::new(
            mirror(header_rect.min.x + 18.0, 0.0),
            header_rect.center().y,
        );
        let chevron_color = mix(p.text_weak, p.accent, hover.max(openness));
        paint_chevron(
            painter,
            chevron_center,
            9.0,
            openness,
            Stroke::new(2.0, chevron_color),
        );

        // Icon (if present)
        let mut text_x = header_rect.min.x + 34.0;
        if let Some(ic) = icon {
            let icon_font = FontId::proportional(15.0);
            let icon_galley = painter.layout_no_wrap(ic.to_owned(), icon_font, p.accent);
            painter.galley(
                Pos2::new(
                    mirror(text_x, icon_galley.size().x),
                    header_rect.center().y - icon_galley.size().y / 2.0,
                ),
                icon_galley,
                p.accent,
            );
            text_x += 22.0;
        }

        // Title text
        let title_font = FontId::proportional(14.5);
        let title_galley = painter.layout_no_wrap(title.to_owned(), title_font, p.text);
        painter.galley(
            Pos2::new(
                mirror(text_x, title_galley.size().x),
                header_rect.center().y - title_galley.size().y / 2.0,
            ),
            title_galley,
            p.text,
        );

        // Optional badge on the right
        if let Some(b) = badge {
            let badge_font = TextStyle::Small.resolve(ui.style());
            let badge_galley = painter.layout_no_wrap(b.to_owned(), badge_font, p.accent);
            let badge_pad = Vec2::new(12.0, 4.0);
            let badge_size = badge_galley.size() + badge_pad;
            let badge_rect = Rect::from_min_size(
                Pos2::new(
                    mirror(header_rect.max.x - badge_size.x - 10.0, badge_size.x),
                    header_rect.center().y - badge_size.y / 2.0,
                ),
                badge_size,
            );
            painter.rect_filled(
                badge_rect,
                CornerRadius::same(255),
                p.accent.gamma_multiply(0.15),
            );
            painter.galley(
                badge_rect.center() - badge_galley.size() / 2.0,
                badge_galley,
                p.accent,
            );
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand);

    let result = state.show_body_unindented(ui, |ui| {
        let p = theme::palette(ui);
        let frame = egui::Frame::new()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius {
                nw: 0,
                ne: 0,
                sw: RADIUS,
                se: RADIUS,
            })
            .inner_margin(egui::Margin::symmetric(14, 12));
        frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                content(ui)
            })
            .inner
    });
    state.store(ui.ctx());
    ui.add_space(6.0);

    result.map(|res| res.inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsible_section_runs_in_context() {
        let ctx = egui::Context::default();
        let mut called = false;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            collapsible_section(
                ui,
                "test_section",
                Some(crate::icons::SLIDERS),
                "Output settings",
                Some("Badge"),
                |_ui| {
                    called = true;
                },
            );
        })
        .textures_delta
        .clear();
        // By default closed, so closure was not called yet.
        assert!(!called);
    }

    #[test]
    fn paint_chevron_runs_without_panic() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter();
            paint_chevron(
                painter,
                Pos2::new(50.0, 50.0),
                10.0,
                0.5,
                Stroke::new(1.8, Color32::WHITE),
            );
        })
        .textures_delta
        .clear();
    }
}
