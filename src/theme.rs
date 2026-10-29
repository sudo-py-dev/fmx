//! Visual theme: palette, spacing, typography, and motion for both light and
//! dark mode. Widgets read colors from [`Palette`] so the look stays consistent.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme,
    ThemePreference, Vec2, style::ScrollAnimation,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

use crate::i18n::tr;

/// Seconds for hover/press and panel transitions (GNOME's 200 ms).
pub const MOTION: f32 = 0.2;
/// Seconds for value changes such as progress bars.
pub const VALUE_MOTION: f32 = 0.35;
/// Seconds for a page fading in, such as another sidebar category.
pub const PAGE_MOTION: f32 = 0.3;
/// Seconds for a tool's page sliding in or out: short, so it feels snappy.
pub const SLIDE_MOTION: f32 = 0.2;
/// Buttons, entries and sidebar rows.
pub const RADIUS: u8 = 6;
/// Cards and boxed lists.
pub const CARD_RADIUS: u8 = 12;
pub const BUTTON_HEIGHT: f32 = 32.0;
pub const SMALL_BUTTON_HEIGHT: f32 = 26.0;

/// A page's vertical scroll area. Its scroll bar floats over a margin of its
/// own on the right, where egui puts it even in right-to-left layouts, so the
/// bar never covers text and content keeps its width when the bar shows.
pub fn page_scroll() -> egui::ScrollArea {
    egui::ScrollArea::vertical().content_margin(Margin {
        right: 10,
        ..Margin::ZERO
    })
}

/// The desktop's light/dark preference as last read from the XDG settings
/// portal: 0 unknown, 1 light, 2 dark. winit reads it only once on Linux.
static DESKTOP_THEME: AtomicU8 = AtomicU8::new(0);

fn desktop_theme() -> Option<Theme> {
    match DESKTOP_THEME.load(Ordering::Relaxed) {
        1 => Some(Theme::Light),
        2 => Some(Theme::Dark),
        _ => None,
    }
}

/// Follows the desktop's light/dark preference while the app runs, so the
/// "System" choice changes along with it without a restart.
#[cfg(target_os = "linux")]
pub fn watch_desktop_theme(ctx: &egui::Context) {
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new()
        .name("theme-watch".into())
        .spawn(move || {
            if let Err(err) = watch_portal(&ctx) {
                tracing::debug!("desktop theme is not watched: {err}");
            }
        });
}

#[cfg(target_os = "linux")]
fn watch_portal(ctx: &egui::Context) -> zbus::Result<()> {
    use zbus::zvariant::OwnedValue;
    const NAMESPACE: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";

    // color-scheme: 0 no preference (light), 1 dark, 2 light.
    let store = |value: &OwnedValue| {
        let scheme = match u32::try_from(value) {
            Ok(v) => v,
            Err(_) => return,
        };
        let theme = if scheme == 1 { 2 } else { 1 };
        if DESKTOP_THEME.swap(theme, Ordering::Relaxed) != theme {
            ctx.request_repaint();
        }
    };
    let conn = zbus::blocking::Connection::session()?;
    let proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )?;
    let changes = proxy.receive_signal("SettingChanged")?;
    if let Ok(value) = proxy.call::<_, _, OwnedValue>("ReadOne", &(NAMESPACE, KEY)) {
        store(&value);
    }
    for message in changes {
        let (namespace, key, value): (String, String, OwnedValue) =
            message.body().deserialize()?;
        if namespace == NAMESPACE && key == KEY {
            store(&value);
        }
    }
    Ok(())
}

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

    pub fn preference(self) -> ThemePreference {
        match self {
            Self::System => match desktop_theme() {
                Some(Theme::Dark) => ThemePreference::Dark,
                Some(Theme::Light) => ThemePreference::Light,
                None => ThemePreference::System,
            },
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
    pub sidebar: Color32,
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

// GNOME's libadwaita colors, flattened onto their backgrounds.
const DARK: Palette = Palette {
    accent: Color32::from_rgb(53, 132, 228),
    accent_hover: Color32::from_rgb(73, 146, 233),
    accent_pressed: Color32::from_rgb(42, 112, 199),
    on_accent: Color32::WHITE,
    surface: Color32::from_rgb(54, 54, 58),
    panel: Color32::from_rgb(34, 34, 38),
    sidebar: Color32::from_rgb(46, 46, 50),
    sunken: Color32::from_rgb(29, 29, 32),
    control: Color32::from_rgb(62, 62, 66),
    control_hover: Color32::from_rgb(74, 74, 78),
    control_pressed: Color32::from_rgb(94, 94, 98),
    border: Color32::from_rgb(68, 68, 72),
    text: Color32::WHITE,
    text_weak: Color32::from_rgb(172, 172, 176),
    success: Color32::from_rgb(87, 227, 137),
    warning: Color32::from_rgb(248, 228, 92),
    danger: Color32::from_rgb(255, 123, 99),
};

const LIGHT: Palette = Palette {
    accent: Color32::from_rgb(53, 132, 228),
    accent_hover: Color32::from_rgb(73, 146, 233),
    accent_pressed: Color32::from_rgb(42, 112, 199),
    on_accent: Color32::WHITE,
    surface: Color32::WHITE,
    panel: Color32::from_rgb(250, 250, 251),
    sidebar: Color32::from_rgb(235, 235, 237),
    sunken: Color32::from_rgb(240, 240, 242),
    control: Color32::from_rgb(230, 230, 232),
    control_hover: Color32::from_rgb(220, 220, 222),
    control_pressed: Color32::from_rgb(200, 200, 203),
    border: Color32::from_rgb(222, 222, 224),
    text: Color32::from_rgb(46, 46, 50),
    text_weak: Color32::from_rgb(96, 96, 101),
    success: Color32::from_rgb(27, 133, 83),
    warning: Color32::from_rgb(156, 110, 3),
    danger: Color32::from_rgb(192, 28, 40),
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
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.window_stroke = Stroke::new(1.0, p.border);
    let shadow_alpha = if dark { 120 } else { 36 };
    v.window_shadow = Shadow {
        offset: [0, 6],
        blur: 32,
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
    v.selection.stroke = Stroke::new(2.0, p.accent);
    v.slider_trailing_fill = true;
    v.striped = true;
    v.window_highlight_topmost = false;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;
    // Flat controls with no outline, as in Adwaita; focus draws the accent.
    for (state, fill, stroke) in [
        (&mut w.inactive, p.control, Stroke::NONE),
        (&mut w.hovered, p.control_hover, Stroke::NONE),
        (&mut w.active, p.control_pressed, Stroke::new(1.0, p.accent)),
        (&mut w.open, p.control_hover, Stroke::NONE),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = stroke;
        state.fg_stroke = Stroke::new(1.5, p.text);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    style
}

/// Applies the theme preference and both light and dark styles.
pub fn apply(ctx: &egui::Context, choice: ThemeChoice, reduce_motion: bool) {
    ctx.set_theme(choice.preference());
    // A desktop with no dark-mode preference (GNOME's "Default") is light,
    // and so is the frame the system draws around the window.
    ctx.options_mut(|o| o.fallback_theme = Theme::Light);
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
