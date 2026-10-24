//! Visual theme: palette, spacing, typography, and motion for both light and
//! dark mode. Widgets read colors from [`Palette`] so the look stays consistent.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme,
    ThemePreference, Vec2, style::ScrollAnimation,
};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;

/// Seconds for hover/press and panel transitions.
pub const MOTION: f32 = 0.16;
/// Seconds for value changes such as progress bars.
pub const VALUE_MOTION: f32 = 0.35;
pub const RADIUS: u8 = 8;
pub const BUTTON_HEIGHT: f32 = 32.0;
pub const SMALL_BUTTON_HEIGHT: f32 = 26.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => tr("theme.system"),
            Self::Light => tr("theme.light"),
            Self::Dark => tr("theme.dark"),
        }
    }

    fn preference(self) -> ThemePreference {
        match self {
            Self::System => ThemePreference::System,
            Self::Light => ThemePreference::Light,
            Self::Dark => ThemePreference::Dark,
        }
    }
}

/// Semantic colors for one theme.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    pub on_accent: Color32,
    pub surface: Color32,
    pub panel: Color32,
    pub sunken: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub control_pressed: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
}

const DARK: Palette = Palette {
    accent: Color32::from_rgb(76, 125, 255),
    accent_hover: Color32::from_rgb(101, 145, 255),
    accent_pressed: Color32::from_rgb(58, 104, 226),
    on_accent: Color32::WHITE,
    surface: Color32::from_rgb(30, 32, 38),
    panel: Color32::from_rgb(23, 24, 28),
    sunken: Color32::from_rgb(17, 18, 21),
    control: Color32::from_rgb(42, 45, 53),
    control_hover: Color32::from_rgb(51, 55, 66),
    control_pressed: Color32::from_rgb(60, 65, 79),
    border: Color32::from_rgb(52, 55, 64),
    text: Color32::from_rgb(230, 232, 238),
    text_weak: Color32::from_rgb(150, 155, 168),
    success: Color32::from_rgb(52, 190, 112),
    warning: Color32::from_rgb(232, 170, 30),
    danger: Color32::from_rgb(229, 72, 77),
};

const LIGHT: Palette = Palette {
    accent: Color32::from_rgb(47, 99, 230),
    accent_hover: Color32::from_rgb(67, 117, 240),
    accent_pressed: Color32::from_rgb(36, 82, 200),
    on_accent: Color32::WHITE,
    surface: Color32::WHITE,
    panel: Color32::from_rgb(246, 247, 249),
    sunken: Color32::from_rgb(238, 240, 244),
    control: Color32::from_rgb(234, 236, 241),
    control_hover: Color32::from_rgb(222, 226, 233),
    control_pressed: Color32::from_rgb(210, 215, 224),
    border: Color32::from_rgb(214, 218, 226),
    text: Color32::from_rgb(28, 31, 38),
    text_weak: Color32::from_rgb(98, 104, 118),
    success: Color32::from_rgb(28, 140, 78),
    warning: Color32::from_rgb(176, 112, 0),
    danger: Color32::from_rgb(206, 44, 49),
};

pub fn palette(ui: &egui::Ui) -> Palette {
    if ui.visuals().dark_mode { DARK } else { LIGHT }
}

fn style_for(base: &egui::Style, p: &Palette, dark: bool, reduce_motion: bool) -> egui::Style {
    let mut style = base.clone();
    let radius = CornerRadius::same(RADIUS);
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(21.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.5, FontFamily::Monospace),
        ),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 8.0);
    s.button_padding = Vec2::new(12.0, 6.0);
    s.interact_size = Vec2::new(40.0, 28.0);
    s.window_margin = Margin::same(16);
    s.menu_margin = Margin::same(8);
    s.indent = 18.0;
    s.combo_width = 140.0;

    style.animation_time = if reduce_motion { 0.0 } else { MOTION };
    style.scroll_animation = if reduce_motion {
        ScrollAnimation::none()
    } else {
        ScrollAnimation::default()
    };

    let v = &mut style.visuals;
    v.dark_mode = dark;
    v.override_text_color = None;
    v.panel_fill = p.panel;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.sunken;
    v.text_edit_bg_color = Some(p.sunken);
    v.faint_bg_color = p.sunken;
    v.code_bg_color = p.sunken;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.hyperlink_color = p.accent;
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_stroke = Stroke::new(1.0, p.border);
    let shadow_alpha = if dark { 110 } else { 40 };
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.45 } else { 0.25 });
    v.selection.stroke = Stroke::new(1.5, p.accent);
    v.slider_trailing_fill = true;
    v.striped = true;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;
    for (state, fill, stroke) in [
        (&mut w.inactive, p.control, p.border),
        (
            &mut w.hovered,
            p.control_hover,
            p.accent.gamma_multiply(0.6),
        ),
        (&mut w.active, p.control_pressed, p.accent),
        (&mut w.open, p.control_hover, p.border),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.5, p.text);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    style
}

/// Applies the theme preference and both light and dark styles.
pub fn apply(ctx: &egui::Context, choice: ThemeChoice, reduce_motion: bool) {
    ctx.set_theme(choice.preference());
    for (theme, palette, dark) in [(Theme::Dark, &DARK, true), (Theme::Light, &LIGHT, false)] {
        let base = egui::Style {
            visuals: theme.default_visuals(),
            ..Default::default()
        };
        ctx.set_style_of(theme, style_for(&base, palette, dark, reduce_motion));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance contrast ratio.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let ch = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.039_28 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (l1, l2) = (lum(a), lum(b));
        (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
    }

    #[test]
    fn text_meets_contrast_guidelines() {
        for p in [DARK, LIGHT] {
            for bg in [p.panel, p.surface, p.control] {
                assert!(contrast(p.text, bg) >= 7.0, "body text");
                assert!(contrast(p.text_weak, bg) >= 4.5, "secondary text");
            }
            assert!(contrast(p.on_accent, p.accent) >= 3.0, "button label");
        }
    }

    #[test]
    fn reduce_motion_disables_animation() {
        let base = egui::Style::default();
        assert_eq!(style_for(&base, &DARK, true, true).animation_time, 0.0);
        assert!(style_for(&base, &DARK, true, false).animation_time > 0.0);
    }
}
