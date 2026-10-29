//! Layout direction: mirrors the UI for right-to-left languages.
//!
//! egui lays everything out left to right. These helpers make the UI follow
//! the active language instead:
//! - [`vertical`] and [`column`] align content to the start edge. egui then
//!   also flows every `ui.horizontal` inside them from that edge;
//! - [`leading`] and [`trailing`] replace `Layout::left_to_right` and
//!   `Layout::right_to_left` for rows that start at the start or end edge;
//! - widgets that paint themselves ask [`is_rtl`] where icons and fills go.
//!
//! Text order inside a string is handled by the patched epaint (see
//! `vendor/epaint/src/text/bidi.rs`); this module only places widgets.

use eframe::egui::{self, Align, InnerResponse, Layout, Response, Sense, Ui, UiBuilder};

/// Whether the UI is mirrored for a right-to-left language.
pub fn is_rtl() -> bool {
    crate::i18n::is_rtl()
}

/// The edge text starts from: left, or right in right-to-left languages.
pub fn start() -> Align {
    if is_rtl() { Align::Max } else { Align::Min }
}

/// A top-down layout aligned to the start edge.
pub fn column() -> Layout {
    Layout::top_down(start())
}

/// A row that flows from the start edge.
pub fn leading(valign: Align) -> Layout {
    if is_rtl() {
        Layout::right_to_left(valign)
    } else {
        Layout::left_to_right(valign)
    }
}

/// A row that flows from the end edge, for content pinned to the end.
pub fn trailing(valign: Align) -> Layout {
    if is_rtl() {
        Layout::left_to_right(valign)
    } else {
        Layout::right_to_left(valign)
    }
}

/// Puts `add` (about `width` wide) at the end edge of the current row when it
/// fits there, and otherwise just next in the row, so in a wrapping row it
/// moves to the next line instead of overlapping what came before.
pub fn at_end<R>(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    // `available_width` counts the whole row in a right-to-left wrapping
    // layout, so measure from the cursor to the row's far edge instead.
    let (cursor, row) = (ui.cursor(), ui.max_rect());
    let room = if ui.layout().main_dir() == egui::Direction::RightToLeft {
        cursor.max.x - row.min.x
    } else {
        row.max.x - cursor.min.x
    };
    if room >= width + ui.spacing().item_spacing.x {
        ui.with_layout(trailing(Align::Center), add).inner
    } else {
        add(ui)
    }
}

/// Like `ui.vertical`, aligned to the start edge.
pub fn vertical<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    ui.scope_builder(UiBuilder::new().layout(column()), add_contents)
}

/// The contents of a drop-down menu: full-width items with text on the
/// start edge. egui's own menus always align their text to the left.
pub fn menu<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    // While egui measures a new menu, right-aligned items would sit at the far
    // end of all the room it could take, making the menu that wide; measured
    // from the left, it fits its items.
    let align = if ui.is_sizing_pass() {
        Align::Min
    } else {
        start()
    };
    ui.with_layout(Layout::top_down_justified(align), add_contents)
        .inner
}

/// A side panel on the start edge.
pub fn start_panel(id: &'static str) -> egui::Panel {
    if is_rtl() {
        egui::Panel::right(id)
    } else {
        egui::Panel::left(id)
    }
}

/// A checkbox with its box on the start edge. egui always puts the box on
/// the left, so right-to-left languages get a box followed by a clickable label.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, text: &str) -> Response {
    if !is_rtl() {
        return ui.checkbox(checked, text);
    }
    ui.with_layout(leading(Align::Center), |ui| {
        // The box's own padding is the gap, as in egui's checkbox with text.
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut response = ui.add(egui::Checkbox::without_text(checked));
        let label = ui.add(egui::Label::new(text).sense(Sense::click()));
        if label.clicked() {
            *checked = !*checked;
            response.mark_changed();
        }
        response | label
    })
    .inner
}

/// Where something `width` wide starts inside `rect`, given where it would
/// start (`x`) in a left-to-right layout: mirrored in right-to-left languages.
pub fn mirror_x(rect: egui::Rect, x: f32, width: f32) -> f32 {
    if is_rtl() {
        rect.max.x - (x - rect.min.x) - width
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lays out `text` with FMX's fonts and returns its rows as `(char, x)` in
    /// logical order.
    fn layout(text: &str, wrap_width: f32) -> Vec<Vec<(char, f32)>> {
        let ctx = egui::Context::default();
        crate::icons::install(&ctx);
        // Fonts load on the first frame; the frame's texture uploads are not needed.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let galley = ctx.fonts_mut(|fonts| {
            fonts.layout(
                text.to_owned(),
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
                wrap_width,
            )
        });
        galley
            .rows
            .iter()
            .map(|row| row.row.glyphs.iter().map(|g| (g.chr, g.pos.x)).collect())
            .collect()
    }

    fn x_of(row: &[(char, f32)], c: char) -> f32 {
        row.iter().find(|(g, _)| *g == c).expect("char in row").1
    }

    /// The row's characters from left to right, as they appear on screen.
    fn visual(row: &[(char, f32)]) -> String {
        let mut sorted = row.to_vec();
        sorted.sort_by(|a, b| a.1.total_cmp(&b.1));
        sorted.into_iter().map(|(c, _)| c).collect()
    }

    #[test]
    fn hebrew_reads_right_to_left() {
        let rows = layout("שלום עולם", f32::INFINITY);
        assert_eq!(visual(&rows[0]), "םלוע םולש");
    }

    #[test]
    fn wrapped_hebrew_keeps_its_line_order() {
        let rows = layout("שלום עולם", 40.0);
        assert_eq!(rows.len(), 2);
        assert!(visual(&rows[0]).contains("םולש"), "{rows:?}");
        assert!(visual(&rows[1]).contains("םלוע"), "{rows:?}");
    }

    #[test]
    fn latin_inside_hebrew_stays_left_to_right() {
        let rows = layout("המרה ל-MP4", f32::INFINITY);
        assert_eq!(visual(&rows[0]), "MP4-ל הרמה");
    }

    #[test]
    fn brackets_mirror_in_hebrew() {
        let rows = layout("(שלום)", f32::INFINITY);
        assert_eq!(visual(&rows[0]), "(םולש)");
    }

    #[test]
    fn direction_mark_makes_latin_start_read_right_to_left() {
        // Without the mark the first letter, M, makes the line left to right.
        let rows = layout("MP4 קובץ", f32::INFINITY);
        assert!(x_of(&rows[0], 'M') < x_of(&rows[0], 'ק'), "{rows:?}");
        // With it, the line is right to left, so MP4 comes first: on the right.
        let rows = layout("\u{200F}MP4 קובץ", f32::INFINITY);
        assert!(x_of(&rows[0], 'M') > x_of(&rows[0], 'ק'), "{rows:?}");
        assert!(x_of(&rows[0], 'ק') > x_of(&rows[0], 'ץ'), "{rows:?}");
    }

    /// Glyphs off the pixel grid look heavier than their neighbours.
    #[test]
    fn mixed_text_stays_on_the_pixel_grid() {
        let rows = layout(
            "\u{200F}ספרים (EPUB, FB2) ל-PDF או TXT — מובנה",
            f32::INFINITY,
        );
        for (c, x) in &rows[0] {
            assert!((x - x.round()).abs() < 1e-3, "{c:?} at {x}");
        }
    }

    #[test]
    fn english_is_untouched() {
        let rows = layout("Convert to MP4 (fast)", f32::INFINITY);
        let xs: Vec<f32> = rows[0].iter().map(|(_, x)| *x).collect();
        assert!(xs.windows(2).all(|w| w[0] <= w[1]), "{xs:?}");
        assert_eq!(visual(&rows[0]), "Convert to MP4 (fast)");
    }

    #[test]
    fn rtl_strings_start_with_the_direction_mark() {
        let he = crate::i18n::parse(r#"{"a": "שלום", "b": "PDF → TXT"}"#, true);
        assert!(he["a"].starts_with(crate::i18n::RTL_MARK));
        assert!(he["b"].contains(crate::icons::ARROW_LEFT));
        let en = crate::i18n::parse(r#"{"b": "PDF → TXT"}"#, false);
        assert!(en["b"].contains(crate::icons::ARROW_RIGHT));
    }
}
