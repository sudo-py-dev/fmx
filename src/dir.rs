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

/// Like `ui.vertical`, aligned to the start edge.
pub fn vertical<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    ui.scope_builder(UiBuilder::new().layout(column()), add_contents)
}

/// The contents of a drop-down menu: full-width items with text on the
/// start edge. egui's own menus always align their text to the left.
pub fn menu<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.with_layout(Layout::top_down_justified(start()), add_contents)
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
        let mut response = ui.checkbox(checked, "");
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

/// One cell of a [`Grid`] row.
pub type Cell<'a> = Box<dyn FnOnce(&mut Ui) + 'a>;

/// A grid that mirrors in right-to-left languages, where `egui::Grid` can't:
/// it always lays out left to right. Left-to-right languages get a plain
/// `egui::Grid`; right-to-left ones get rows flowing from the right, sized like
/// `egui::Grid`: each column as wide as its widest cell in the previous frame,
/// except the last, which gets the rest of the row. Add rows with [`grid_row`].
pub struct Grid {
    id: &'static str,
    columns: usize,
    spacing: egui::Vec2,
    min_col_width: f32,
    max_col_width: f32,
    min_row_height: f32,
    striped: bool,
}

/// The right-to-left grid being drawn, with the column widths and row
/// heights measured in the previous frame.
#[derive(Clone, Default)]
struct RtlGrid {
    spacing: egui::Vec2,
    min_col_width: f32,
    max_col_width: f32,
    min_row_height: f32,
    striped: bool,
    previous: Sizes,
    current: Sizes,
    rows: usize,
}

#[derive(Clone, Default, PartialEq)]
struct Sizes {
    columns: Vec<f32>,
    rows: Vec<f32>,
}

fn current_grid_id() -> egui::Id {
    egui::Id::unique("fmx_rtl_grid_current")
}

impl Grid {
    pub fn new(id: &'static str, columns: usize) -> Self {
        Self {
            id,
            columns,
            spacing: egui::vec2(8.0, 4.0),
            min_col_width: 0.0,
            max_col_width: f32::INFINITY,
            min_row_height: 0.0,
            striped: false,
        }
    }

    pub fn spacing(mut self, spacing: [f32; 2]) -> Self {
        self.spacing = spacing.into();
        self
    }

    pub fn min_col_width(mut self, width: f32) -> Self {
        self.min_col_width = width;
        self
    }

    pub fn max_col_width(mut self, width: f32) -> Self {
        self.max_col_width = width;
        self
    }

    pub fn min_row_height(mut self, height: f32) -> Self {
        self.min_row_height = height;
        self
    }

    pub fn striped(mut self, striped: bool) -> Self {
        self.striped = striped;
        self
    }

    pub fn show<R>(self, ui: &mut Ui, add_rows: impl FnOnce(&mut Ui) -> R) -> R {
        if !is_rtl() {
            return egui::Grid::new(self.id)
                .num_columns(self.columns)
                .spacing(self.spacing)
                .min_col_width(self.min_col_width)
                .max_col_width(self.max_col_width)
                .min_row_height(self.min_row_height)
                .striped(self.striped)
                .show(ui, add_rows)
                .inner;
        }
        let widths_id = ui.make_persistent_id(("fmx_rtl_grid", self.id));
        let previous: Sizes = ui.data(|d| d.get_temp(widths_id)).unwrap_or_default();
        ui.data_mut(|d| {
            d.insert_temp(
                current_grid_id(),
                RtlGrid {
                    spacing: self.spacing,
                    min_col_width: self.min_col_width,
                    max_col_width: self.max_col_width,
                    min_row_height: self.min_row_height,
                    striped: self.striped,
                    previous: previous.clone(),
                    current: Sizes {
                        columns: vec![0.0; self.columns],
                        rows: Vec::new(),
                    },
                    rows: 0,
                },
            );
        });
        let result = vertical(ui, |ui| {
            ui.spacing_mut().item_spacing.y = self.spacing.y;
            add_rows(ui)
        })
        .inner;
        let grid: RtlGrid = ui
            .data_mut(|d| d.remove_temp(current_grid_id()))
            .unwrap_or_default();
        if grid.current != previous {
            ui.data_mut(|d| d.insert_temp(widths_id, grid.current));
            ui.ctx().request_repaint();
        }
        result
    }
}

/// Adds one row of `cells` to the [`Grid`] being shown, in reading order.
pub fn grid_row(ui: &mut Ui, cells: Vec<Cell<'_>>) {
    let grid: Option<RtlGrid> = ui.data(|d| d.get_temp(current_grid_id()));
    let Some(mut grid) = grid.filter(|_| is_rtl()) else {
        for cell in cells {
            cell(ui);
        }
        ui.end_row();
        return;
    };
    let background = ui.painter().add(egui::Shape::Noop);
    // Last frame's height lets every cell center on the row, as in `egui::Grid`.
    let height = grid
        .previous
        .rows
        .get(grid.rows)
        .copied()
        .unwrap_or(0.0)
        .max(grid.min_row_height);
    let row = ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        Layout::right_to_left(Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = grid.spacing.x;
            let mut tallest: f32 = 0.0;
            let last = cells.len().saturating_sub(1);
            for (i, cell) in cells.into_iter().enumerate() {
                let known = grid.previous.columns.get(i).copied().filter(|w| *w > 0.0);
                // Column width to claim, so the next cell lines up.
                let claim = known
                    .unwrap_or(0.0)
                    .clamp(grid.min_col_width, grid.max_col_width)
                    .min(ui.available_width());
                // Room for the content: the rest of the row for the last column,
                // or before the width is known; otherwise the column's width.
                let room = if i == last || known.is_none() {
                    ui.available_width().min(grid.max_col_width)
                } else {
                    claim
                };
                let used = ui
                    .allocate_ui_with_layout(
                        egui::vec2(room, height),
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            ui.set_min_width(claim);
                            ui.scope(cell).response.rect
                        },
                    )
                    .inner;
                if let Some(max) = grid.current.columns.get_mut(i) {
                    *max = max.max(used.width());
                }
                tallest = tallest.max(used.height());
            }
            tallest
        },
    );
    if grid.striped && grid.rows % 2 == 1 {
        let rect = row
            .response
            .rect
            .expand2(egui::vec2(0.0, grid.spacing.y / 2.0));
        ui.painter().set(
            background,
            egui::Shape::rect_filled(rect, 0.0, ui.visuals().faint_bg_color),
        );
    }
    grid.current.rows.push(row.inner.max(grid.min_row_height));
    grid.rows += 1;
    ui.data_mut(|d| d.insert_temp(current_grid_id(), grid));
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
