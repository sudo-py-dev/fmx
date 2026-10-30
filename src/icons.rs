//! Bundled fonts: the Phosphor icon font (MIT, see `assets/fonts/LICENSE-Phosphor.txt`)
//! and Noto Sans Hebrew (OFL, see `assets/fonts/NotoSansHebrew-OFL.txt`).
//!
//! egui's default fonts lack most symbol glyphs and all Hebrew letters, so
//! icons and Hebrew come from these fonts, registered as fallbacks.

use std::sync::Arc;

use eframe::egui;

const FONT_NAME: &str = "phosphor";
const FONT: &[u8] = include_bytes!("../assets/fonts/Phosphor.ttf");
const FILL_NAME: &str = "phosphor-fill";
const FILL_FONT: &[u8] = include_bytes!("../assets/fonts/Phosphor-Fill.ttf");
const HEBREW_NAME: &str = "noto-sans-hebrew";
const HEBREW_FONT: &[u8] = include_bytes!("../assets/fonts/NotoSansHebrew-Regular.ttf");

/// The filled icon weight, used for colorful tiles.
pub fn fill(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FILL_NAME.into()))
}

pub const ARROW_CLOCKWISE: &str = "\u{E036}";
pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{E038}";
pub const ARROW_LEFT: &str = "\u{E058}";
pub const ARROW_LINE_LEFT: &str = "\u{E062}";
pub const ARROW_LINE_RIGHT: &str = "\u{E064}";
pub const ARROW_RIGHT: &str = "\u{E06C}";
pub const FLIP_HORIZONTAL: &str = "\u{ED6A}";
pub const FLIP_VERTICAL: &str = "\u{ED6C}";
pub const ARROW_SQUARE_OUT: &str = "\u{E5DE}";
pub const BROOM: &str = "\u{EC54}";
pub const CARET_DOWN: &str = "\u{E136}";
pub const CARET_LEFT: &str = "\u{E138}";
pub const CARET_RIGHT: &str = "\u{E13A}";
pub const CARET_UP: &str = "\u{E13C}";
pub const CHECK: &str = "\u{E182}";
pub const DOTS_THREE_VERTICAL: &str = "\u{E208}";
pub const CHECK_CIRCLE: &str = "\u{E184}";
pub const CLIPBOARD: &str = "\u{E196}";
pub const DISC: &str = "\u{E564}";
pub const EYE: &str = "\u{E220}";
pub const DOWNLOAD: &str = "\u{E20C}";
pub const FILE_PLUS: &str = "\u{E236}";
pub const FLOPPY_DISK: &str = "\u{E248}";
pub const FOLDER: &str = "\u{E25A}";
pub const FOLDER_OPEN: &str = "\u{E256}";
pub const FOLDER_PLUS: &str = "\u{E258}";
pub const GEAR: &str = "\u{E270}";
pub const INFO: &str = "\u{E2CE}";
pub const LIST_PLUS: &str = "\u{E2F8}";
pub const MAGNIFYING_GLASS: &str = "\u{E30C}";
pub const PAUSE: &str = "\u{E39E}";
pub const PLAY: &str = "\u{E3D0}";
pub const PLUS: &str = "\u{E3D4}";
pub const SKIP_BACK: &str = "\u{E5A4}";
pub const SKIP_FORWARD: &str = "\u{E5A6}";
pub const RECORD: &str = "\u{E3EE}";
pub const CAMERA: &str = "\u{E10E}";
pub const SCISSORS: &str = "\u{EAE0}";
pub const SLIDERS: &str = "\u{E432}";
pub const SPARKLE: &str = "\u{E6A2}";
pub const STAR: &str = "\u{E46A}";
pub const STOP: &str = "\u{E46C}";
pub const WARNING: &str = "\u{E4E0}";
pub const X: &str = "\u{E4F6}";
pub const X_CIRCLE: &str = "\u{E4F8}";

/// Registers the icon and Hebrew fonts as fallbacks for both text families.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        Arc::new(egui::FontData::from_static(FONT)),
    );
    fonts.font_data.insert(
        HEBREW_NAME.to_owned(),
        Arc::new(egui::FontData::from_static(HEBREW_FONT)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let keys = fonts.families.entry(family).or_default();
        // Second, so the text font still wins for ordinary characters.
        keys.insert(keys.len().min(1), FONT_NAME.to_owned());
        // Last: it only has to supply the letters no other font has.
        keys.push(HEBREW_NAME.to_owned());
    }
    fonts.font_data.insert(
        FILL_NAME.to_owned(),
        Arc::new(egui::FontData::from_static(FILL_FONT)),
    );
    fonts.families.insert(
        egui::FontFamily::Name(FILL_NAME.into()),
        vec![FILL_NAME.to_owned()],
    );
    ctx.set_fonts(fonts);
}

/// An icon followed by a label.
pub fn label(icon: &str, text: &str) -> String {
    format!("{}{icon}  {text}", crate::i18n::direction_mark())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon constant must be a glyph that exists in the bundled font.
    #[test]
    fn all_icons_exist_in_font() {
        let ctx = egui::Context::default();
        install(&ctx);
        let icons = [
            ARROW_CLOCKWISE,
            ARROW_COUNTER_CLOCKWISE,
            ARROW_RIGHT,
            FLIP_HORIZONTAL,
            FLIP_VERTICAL,
            ARROW_SQUARE_OUT,
            BROOM,
            CARET_DOWN,
            CARET_LEFT,
            CARET_RIGHT,
            CARET_UP,
            CHECK,
            CHECK_CIRCLE,
            CLIPBOARD,
            DISC,
            EYE,
            DOWNLOAD,
            FILE_PLUS,
            FLOPPY_DISK,
            FOLDER,
            FOLDER_OPEN,
            FOLDER_PLUS,
            GEAR,
            INFO,
            LIST_PLUS,
            MAGNIFYING_GLASS,
            PAUSE,
            PLAY,
            PLUS,
            RECORD,
            CAMERA,
            SCISSORS,
            SPARKLE,
            SLIDERS,
            STOP,
            WARNING,
            X,
            X_CIRCLE,
        ];
        // Fonts load on the first frame; the frame's texture uploads are not needed.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let font = egui::FontId::proportional(14.0);
        ctx.fonts_mut(|fonts| {
            for icon in icons {
                let c = icon.chars().next().expect("icon char");
                assert!(
                    fonts.has_glyph(&font, c),
                    "missing glyph U+{:04X}",
                    c as u32
                );
            }
        });
    }

    /// Every colorful tile glyph must exist in the filled font.
    #[test]
    fn all_art_glyphs_exist_in_fill_font() {
        let ctx = egui::Context::default();
        install(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let font = fill(20.0);
        ctx.fonts_mut(|fonts| {
            for glyph in crate::art::glyph::ALL {
                let c = glyph.chars().next().expect("glyph");
                assert!(
                    fonts.has_glyph(&font, c),
                    "missing filled glyph U+{:04X}",
                    c as u32
                );
            }
        });
    }

    /// Every character shown in the UI, in every language, must render.
    #[test]
    fn all_ui_text_renders() {
        let ctx = egui::Context::default();
        install(&ctx);
        // Fonts load on the first frame; the frame's texture uploads are not needed.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let mut texts: Vec<String> = ["en", "es", "fr", "de", "he"]
            .iter()
            .flat_map(|code| {
                let path = format!("{}/locales/{code}.json", env!("CARGO_MANIFEST_DIR"));
                let src = std::fs::read_to_string(path).expect("locale");
                let map = crate::i18n::parse(&src, *code == "he");
                map.into_values()
            })
            .collect();
        for category in crate::catalog::Category::ALL {
            texts.extend(category.tools().iter().map(|t| t.label().to_owned()));
        }
        texts.push("× px kb/s dB Hz".to_owned());
        let font = egui::FontId::proportional(14.0);
        let mut missing = std::collections::BTreeSet::new();
        ctx.fonts_mut(|fonts| {
            for text in &texts {
                missing.extend(text.chars().filter(|c| {
                    !c.is_whitespace() && *c != crate::i18n::RTL_MARK && !fonts.has_glyph(&font, *c)
                }));
            }
        });
        assert!(
            missing.is_empty(),
            "characters without a glyph: {missing:?}"
        );
    }
}
