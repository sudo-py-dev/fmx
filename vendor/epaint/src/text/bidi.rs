//! FMX patch: right-to-left text (Hebrew) for epaint, which has no bidi support.
//!
//! epaint shapes and line-wraps text in logical order, which is what line
//! breaking needs. This module adds the two Unicode Bidirectional Algorithm
//! (UAX #9) steps that epaint is missing:
//!
//! - [`mirrored_text`]: before shaping, paired characters such as `(` that end
//!   up in a right-to-left run are swapped for their mirror image (rule L4).
//! - [`reorder_rows`]: after wrapping and alignment, each row's glyphs are
//!   moved into visual order (rules L1 and L2). Whole clusters move together,
//!   so combining marks stay on their base letter, and the row keeps the span
//!   that alignment gave it.
//!
//! The glyph vector stays in logical order, so cursor indices still match
//! characters. Text without right-to-left characters is never touched.

use alloc::{string::String, vec::Vec};
use core::ops::Range;

use unicode_bidi::{BidiClass, BidiInfo, bidi_class};

use super::{Glyph, PlacedRow};

/// Whether `text` contains a strong right-to-left character.
pub(super) fn has_rtl(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(bidi_class(c), BidiClass::R | BidiClass::AL))
}

/// `text` with every mirrorable character inside a right-to-left run replaced
/// by its mirror, or `None` when nothing changes. Only same-length swaps are
/// made, so section byte ranges stay valid.
pub(super) fn mirrored_text(text: &str) -> Option<String> {
    if !has_rtl(text) {
        return None;
    }
    let info = BidiInfo::new(text, None);
    let mut changed = false;
    let mirrored = text
        .char_indices()
        .map(|(at, c)| {
            let mirror = info.levels[at]
                .is_rtl()
                .then(|| unicode_bidi_mirroring::get_mirrored(c))
                .flatten()
                .filter(|m| m.len_utf8() == c.len_utf8());
            changed |= mirror.is_some();
            mirror.unwrap_or(c)
        })
        .collect();
    changed.then_some(mirrored)
}

/// Moves the glyphs of every row that holds right-to-left text into visual order.
pub(super) fn reorder_rows(rows: &mut [PlacedRow]) {
    let mut start = 0;
    while start < rows.len() {
        // A paragraph is the run of rows up to and including one ending in a newline.
        let end = rows[start..]
            .iter()
            .position(|row| row.ends_with_newline)
            .map_or(rows.len(), |i| start + i + 1);
        reorder_paragraph(&mut rows[start..end]);
        start = end;
    }
}

fn reorder_paragraph(rows: &mut [PlacedRow]) {
    let mut text = String::new();
    // Byte offset of each glyph's character, per row.
    let mut offsets: Vec<Vec<usize>> = Vec::with_capacity(rows.len());
    for placed in rows.iter() {
        offsets.push(
            placed
                .row
                .glyphs
                .iter()
                .map(|glyph| {
                    let at = text.len();
                    text.push(glyph.chr);
                    at
                })
                .collect(),
        );
    }
    if !has_rtl(&text) {
        return;
    }
    let info = BidiInfo::new(&text, None);
    for (placed, offsets) in rows.iter_mut().zip(&offsets) {
        let (Some(&first), Some(&last)) = (offsets.first(), offsets.last()) else {
            continue;
        };
        let line = first..last + placed.row.glyphs[offsets.len() - 1].chr.len_utf8();
        let Some(para) = info.paragraphs.iter().find(|p| p.range.contains(&line.start)) else {
            continue;
        };
        let (levels, runs) = info.visual_runs(para, line);
        let glyph_at = |byte: usize| offsets.partition_point(|&o| o < byte);
        let order: Vec<Range<usize>> = runs
            .iter()
            .flat_map(|run| {
                let mut clusters = clusters(&placed.row.glyphs, glyph_at(run.start)..glyph_at(run.end));
                if levels[run.start].is_rtl() {
                    clusters.reverse();
                }
                clusters
            })
            .collect();
        place(&mut alloc::sync::Arc::make_mut(&mut placed.row).glyphs, &order);
    }
}

/// Splits `range` into clusters: a glyph with width plus any zero-width
/// glyphs (combining marks) that follow it.
fn clusters(glyphs: &[Glyph], range: Range<usize>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for i in range {
        match out.last_mut() {
            Some(last) if glyphs[i].advance_width == 0.0 => last.end = i + 1,
            _ => out.push(i..i + 1),
        }
    }
    out
}

/// Lays clusters out left to right in `order`, keeping the row's content
/// (everything but leading and trailing whitespace) where alignment put it.
fn place(glyphs: &mut [Glyph], order: &[Range<usize>]) {
    let is_space = |g: &Glyph| g.chr.is_whitespace();
    let leading = glyphs.iter().take_while(|g| is_space(g)).count();
    if leading == glyphs.len() {
        return;
    }
    let trailing = glyphs.iter().rev().take_while(|g| is_space(g)).count();
    let content = leading..glyphs.len() - trailing;

    // Each cluster's width in logical order, so kerning and letter spacing survive.
    let width = |cluster: &Range<usize>| match glyphs.get(cluster.end) {
        Some(next) => next.pos.x - glyphs[cluster.start].pos.x,
        None => glyphs[cluster.end - 1].pos.x + glyphs[cluster.end - 1].advance_width
            - glyphs[cluster.start].pos.x,
    };
    let before_content: f32 = order
        .iter()
        .take_while(|c| !content.contains(&c.start))
        .map(width)
        .sum();
    let widths: Vec<f32> = order.iter().map(width).collect();

    let mut cursor = glyphs[content.start].pos.x - before_content;
    for (cluster, w) in order.iter().zip(widths) {
        let dx = cursor - glyphs[cluster.start].pos.x;
        for glyph in &mut glyphs[cluster.clone()] {
            glyph.pos.x += dx;
        }
        cursor += w;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_right_to_left_text() {
        assert!(has_rtl("שלום"));
        assert!(has_rtl("Save to תיקייה"));
        assert!(has_rtl("\u{200F}MP4"));
        assert!(!has_rtl("Convert to MP4 (fast)"));
    }

    #[test]
    fn mirrors_brackets_only_inside_right_to_left_runs() {
        assert_eq!(mirrored_text("(שלום)").as_deref(), Some(")שלום("));
        // Latin inside Hebrew keeps its brackets.
        assert_eq!(mirrored_text("שלום f(x)"), None);
        assert_eq!(mirrored_text("plain (text)"), None);
    }

    #[test]
    fn clusters_keep_marks_on_their_base() {
        let glyph = |advance_width| Glyph {
            chr: 'a',
            pos: emath::Pos2::ZERO,
            advance_width,
            line_height: 0.0,
            font_ascent: 0.0,
            font_height: 0.0,
            font_face_ascent: 0.0,
            font_face_height: 0.0,
            uv_rect: Default::default(),
            is_color: false,
            section_index: 0,
            first_vertex: 0,
        };
        let glyphs = [glyph(5.0), glyph(0.0), glyph(4.0)];
        assert_eq!(clusters(&glyphs, 0..3), vec![0..2, 2..3]);
    }
}
