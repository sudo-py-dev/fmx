//! Bundled Phosphor icon font (MIT, see `assets/fonts/LICENSE-Phosphor.txt`).
//!
//! egui's default fonts lack most symbol glyphs, so icons come from this font,
//! registered as a fallback right after the text font.

use std::sync::Arc;

use eframe::egui;

const FONT_NAME: &str = "phosphor";
const FONT: &[u8] = include_bytes!("../assets/fonts/Phosphor.ttf");

pub const ARROW_CLOCKWISE: &str = "\u{E036}";
pub const ARROW_RIGHT: &str = "\u{E06C}";
pub const ARROW_SQUARE_OUT: &str = "\u{E5DE}";
pub const BROOM: &str = "\u{EC54}";
pub const CARET_DOWN: &str = "\u{E136}";
pub const CARET_UP: &str = "\u{E13C}";
pub const CHECK_CIRCLE: &str = "\u{E184}";
pub const DISC: &str = "\u{E564}";
pub const DOWNLOAD: &str = "\u{E20C}";
pub const FILE_PLUS: &str = "\u{E236}";
pub const FILE_TEXT: &str = "\u{E23A}";
pub const FILM_STRIP: &str = "\u{E792}";
pub const FLOPPY_DISK: &str = "\u{E248}";
pub const FOLDER: &str = "\u{E25A}";
pub const FOLDER_OPEN: &str = "\u{E256}";
pub const FOLDER_PLUS: &str = "\u{E258}";
pub const GEAR: &str = "\u{E270}";
pub const IMAGE: &str = "\u{E2CA}";
pub const INFO: &str = "\u{E2CE}";
pub const LIST_PLUS: &str = "\u{E2F8}";
pub const MUSIC_NOTES: &str = "\u{E340}";
pub const PAUSE: &str = "\u{E39E}";
pub const PLAY: &str = "\u{E3D0}";
pub const RECORD: &str = "\u{E3EE}";
pub const SCISSORS: &str = "\u{EAE0}";
pub const SLIDERS: &str = "\u{E432}";
pub const STOP: &str = "\u{E46C}";
pub const WARNING: &str = "\u{E4E0}";
pub const WRENCH: &str = "\u{E5D4}";
pub const X: &str = "\u{E4F6}";
pub const X_CIRCLE: &str = "\u{E4F8}";

/// Registers the icon font as a fallback for both text families.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        Arc::new(egui::FontData::from_static(FONT)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let keys = fonts.families.entry(family).or_default();
        // Second, so the text font still wins for ordinary characters.
        keys.insert(keys.len().min(1), FONT_NAME.to_owned());
    }
    ctx.set_fonts(fonts);
}

/// An icon followed by a label.
pub fn label(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
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
            ARROW_RIGHT,
            ARROW_SQUARE_OUT,
            BROOM,
            CARET_DOWN,
            CARET_UP,
            CHECK_CIRCLE,
            DISC,
            DOWNLOAD,
            FILE_PLUS,
            FILE_TEXT,
            FILM_STRIP,
            FLOPPY_DISK,
            FOLDER,
            FOLDER_OPEN,
            FOLDER_PLUS,
            GEAR,
            IMAGE,
            INFO,
            LIST_PLUS,
            MUSIC_NOTES,
            PAUSE,
            PLAY,
            RECORD,
            SCISSORS,
            SLIDERS,
            STOP,
            WARNING,
            WRENCH,
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

    /// Every character shown in the UI, in every language, must render.
    #[test]
    fn all_ui_text_renders() {
        let ctx = egui::Context::default();
        install(&ctx);
        // Fonts load on the first frame; the frame's texture uploads are not needed.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let mut texts: Vec<String> = ["en", "es", "fr", "de"]
            .iter()
            .flat_map(|code| {
                let path = format!("{}/locales/{code}.json", env!("CARGO_MANIFEST_DIR"));
                let src = std::fs::read_to_string(path).expect("locale");
                let map = crate::i18n::parse(&src);
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
                missing.extend(
                    text.chars()
                        .filter(|c| !c.is_whitespace() && !fonts.has_glyph(&font, *c)),
                );
            }
        });
        assert!(
            missing.is_empty(),
            "characters without a glyph: {missing:?}"
        );
    }
}
