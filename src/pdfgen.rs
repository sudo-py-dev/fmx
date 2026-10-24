//! Native document rendering to PDF: a small layout engine (wrapped text,
//! headings, lists, tables, page breaks) and a PDF writer that embeds a
//! Unicode TrueType font, so text is searchable and copyable.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

use flate2::Compression;
use flate2::write::ZlibEncoder;

/// The embedded font: Ubuntu Light, which egui already ships (Ubuntu Font Licence).
const FONT: &[u8] = epaint_default_fonts::UBUNTU_LIGHT;
const FONT_NAME: &str = "Ubuntu-Light";

const PAGE_W: f32 = 595.0;
const PAGE_H: f32 = 842.0;
const MARGIN: f32 = 56.0;
const BODY_SIZE: f32 = 10.5;
const TABLE_SIZE: f32 = 9.0;
const LINE_GAP: f32 = 1.38;
const PARA_GAP: f32 = 6.0;
const LIST_INDENT: f32 = 18.0;
const CELL_PAD: f32 = 4.0;
const ITALIC_SKEW: f32 = 0.2;
/// Line width for the "fill and stroke" render mode that fakes bold.
const BOLD_STROKE: f32 = 0.035;

/// A run of text with one style.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: false,
            italic: false,
        }
    }
}

/// Document content in reading order.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, String),
    Paragraph(Vec<Span>),
    ListItem(Vec<Span>),
    Table(Vec<Vec<String>>),
    PageBreak,
}

#[derive(Debug)]
pub enum PdfError {
    Font,
    Io(std::io::Error),
}

impl From<std::io::Error> for PdfError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

struct Font<'a> {
    face: ttf_parser::Face<'a>,
    units: f32,
}

impl<'a> Font<'a> {
    fn load(data: &'a [u8]) -> Result<Self, PdfError> {
        let face = ttf_parser::Face::parse(data, 0).map_err(|_| PdfError::Font)?;
        let units = f32::from(face.units_per_em());
        Ok(Self { face, units })
    }

    /// Glyph id for a character, falling back to `?` when the font lacks it.
    fn glyph(&self, c: char) -> u16 {
        self.face
            .glyph_index(c)
            .or_else(|| self.face.glyph_index('?'))
            .map_or(0, |g| g.0)
    }

    /// Advance width in 1/1000 em.
    fn advance(&self, gid: u16) -> f32 {
        let adv = self
            .face
            .glyph_hor_advance(ttf_parser::GlyphId(gid))
            .unwrap_or(0);
        f32::from(adv) * 1000.0 / self.units
    }

    fn width(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|c| self.advance(self.glyph(c)))
            .sum::<f32>()
            * size
            / 1000.0
    }
}

/// A word (or forced break) ready for line breaking.
#[derive(Clone, Debug)]
enum Token {
    Word {
        text: String,
        bold: bool,
        italic: bool,
    },
    Break,
}

fn tokenize(spans: &[Span]) -> Vec<Token> {
    let mut tokens = Vec::new();
    for span in spans {
        let text = span.text.replace('\t', "    ");
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                tokens.push(Token::Break);
            }
            for word in line.split(' ').filter(|w| !w.is_empty()) {
                tokens.push(Token::Word {
                    text: word.to_owned(),
                    bold: span.bold,
                    italic: span.italic,
                });
            }
        }
    }
    tokens
}

type Line = Vec<(String, bool, bool)>;

/// Greedy line breaking; words longer than a line are split by characters.
fn wrap(font: &Font<'_>, tokens: &[Token], size: f32, max_width: f32) -> Vec<Line> {
    let space = font.width(" ", size);
    let mut lines: Vec<Line> = vec![Vec::new()];
    let mut used = 0.0f32;
    for token in tokens {
        let Token::Word { text, bold, italic } = token else {
            lines.push(Vec::new());
            used = 0.0;
            continue;
        };
        let mut word = text.clone();
        loop {
            let w = font.width(&word, size);
            let line = lines.last_mut().expect("at least one line");
            let needed = if line.is_empty() { w } else { used + space + w };
            if needed <= max_width {
                used = needed;
                line.push((word, *bold, *italic));
                break;
            }
            if !line.is_empty() {
                lines.push(Vec::new());
                used = 0.0;
                continue;
            }
            // A single word wider than the line: cut it where it overflows.
            let mut cut = 0;
            let mut acc = 0.0;
            for (idx, c) in word.char_indices() {
                acc += font.width(&c.to_string(), size);
                if acc > max_width && idx > 0 {
                    break;
                }
                cut = idx + c.len_utf8();
            }
            let rest = word.split_off(cut);
            line.push((word, *bold, *italic));
            lines.push(Vec::new());
            used = 0.0;
            if rest.is_empty() {
                break;
            }
            word = rest;
        }
    }
    lines
}

/// Builds page content streams and records which glyphs were used.
struct Layout<'f> {
    font: &'f Font<'f>,
    pages: Vec<String>,
    page: String,
    y: f32,
    used: BTreeMap<u16, char>,
}

impl<'f> Layout<'f> {
    fn new(font: &'f Font<'f>) -> Self {
        Self {
            font,
            pages: Vec::new(),
            page: String::new(),
            y: PAGE_H - MARGIN,
            used: BTreeMap::new(),
        }
    }

    fn content_width() -> f32 {
        PAGE_W - 2.0 * MARGIN
    }

    fn new_page(&mut self) {
        self.pages.push(std::mem::take(&mut self.page));
        self.y = PAGE_H - MARGIN;
    }

    fn ensure(&mut self, height: f32) {
        if self.y - height < MARGIN && !self.page.is_empty() {
            self.new_page();
        }
    }

    fn hex(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() * 4);
        for c in text.chars() {
            let gid = self.font.glyph(c);
            self.used.entry(gid).or_insert(c);
            let _ = write!(out, "{gid:04X}");
        }
        out
    }

    fn text(&mut self, x: f32, y: f32, size: f32, text: &str, bold: bool, italic: bool) {
        let hex = self.hex(text);
        let skew = if italic { ITALIC_SKEW } else { 0.0 };
        let (mode, stroke) = if bold {
            (2, size * BOLD_STROKE)
        } else {
            (0, 0.0)
        };
        let _ = writeln!(
            self.page,
            "BT /F1 {size:.2} Tf {mode} Tr {stroke:.3} w 1 0 {skew:.2} 1 {x:.2} {y:.2} Tm <{hex}> Tj ET"
        );
    }

    fn line_height(size: f32) -> f32 {
        size * LINE_GAP
    }

    fn paragraph(&mut self, spans: &[Span], size: f32, indent: f32, bullet: bool) {
        let width = Self::content_width() - indent;
        let lines = wrap(self.font, &tokenize(spans), size, width);
        let lh = Self::line_height(size);
        let space = self.font.width(" ", size);
        for (i, line) in lines.iter().enumerate() {
            self.ensure(lh);
            self.y -= lh;
            if bullet && i == 0 {
                self.text(
                    MARGIN + indent - LIST_INDENT * 0.7,
                    self.y,
                    size,
                    "•",
                    false,
                    false,
                );
            }
            let mut x = MARGIN + indent;
            for (word, bold, italic) in line {
                self.text(x, self.y, size, word, *bold, *italic);
                x += self.font.width(word, size) + space;
            }
        }
        self.y -= PARA_GAP;
    }

    fn heading(&mut self, level: u8, text: &str) {
        let size = match level {
            1 => 20.0,
            2 => 16.0,
            3 => 13.5,
            _ => 12.0,
        };
        // Keep a heading together with at least two lines that follow it.
        self.ensure(Self::line_height(size) + 2.0 * Self::line_height(BODY_SIZE) + 8.0);
        self.y -= 6.0;
        self.paragraph(
            &[Span {
                text: text.to_owned(),
                bold: true,
                italic: false,
            }],
            size,
            0.0,
            false,
        );
    }

    fn table(&mut self, rows: &[Vec<String>]) {
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let total = Self::content_width();
        // Share the width by each column's longest text, with a floor.
        let longest: Vec<f32> = (0..cols)
            .map(|c| {
                rows.iter()
                    .filter_map(|r| r.get(c))
                    .map(|t| self.font.width(t, TABLE_SIZE))
                    .fold(24.0f32, f32::max)
                    .min(total)
            })
            .collect();
        let sum: f32 = longest.iter().sum();
        let floor = (total / cols as f32 * 0.35).min(40.0);
        let mut widths: Vec<f32> = longest
            .iter()
            .map(|w| (w / sum * total).max(floor))
            .collect();
        let scale = total / widths.iter().sum::<f32>();
        widths.iter_mut().for_each(|w| *w *= scale);

        let lh = Self::line_height(TABLE_SIZE);
        let max_lines = ((PAGE_H - 2.0 * MARGIN - 2.0 * CELL_PAD) / lh)
            .floor()
            .max(1.0) as usize;
        for row in rows {
            let cells: Vec<Vec<Line>> = (0..cols)
                .map(|c| {
                    let text = row.get(c).map_or("", String::as_str);
                    let mut lines = wrap(
                        self.font,
                        &tokenize(&[Span::plain(text)]),
                        TABLE_SIZE,
                        widths[c] - 2.0 * CELL_PAD,
                    );
                    lines.truncate(max_lines);
                    lines
                })
                .collect();
            let height =
                cells.iter().map(Vec::len).max().unwrap_or(1).max(1) as f32 * lh + 2.0 * CELL_PAD;
            self.ensure(height);
            let top = self.y;
            let mut x = MARGIN;
            for (c, lines) in cells.iter().enumerate() {
                let _ = writeln!(
                    self.page,
                    "0.6 G 0.5 w {x:.2} {:.2} {:.2} {height:.2} re S",
                    top - height,
                    widths[c]
                );
                let mut y = top - CELL_PAD;
                for line in lines {
                    y -= lh;
                    let text = line
                        .iter()
                        .map(|(w, _, _)| w.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    self.text(x + CELL_PAD, y + lh * 0.25, TABLE_SIZE, &text, false, false);
                }
                x += widths[c];
            }
            self.y = top - height;
        }
        self.y -= PARA_GAP * 2.0;
    }

    fn finish(mut self) -> (Vec<String>, BTreeMap<u16, char>) {
        if !self.page.is_empty() || self.pages.is_empty() {
            self.pages.push(std::mem::take(&mut self.page));
        }
        (self.pages, self.used)
    }
}

fn zlib(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data)?;
    enc.finish()
}

/// Counts bytes so xref offsets are exact.
struct Out<W: Write> {
    inner: W,
    pos: usize,
    offsets: Vec<usize>,
}

impl<W: Write> Out<W> {
    fn raw(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(bytes)?;
        self.pos += bytes.len();
        Ok(())
    }

    /// Writes object `n` (1-based, in order).
    fn object(&mut self, body: &str) -> std::io::Result<()> {
        self.offsets.push(self.pos);
        let n = self.offsets.len();
        self.raw(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes())
    }

    fn stream(&mut self, dict: &str, data: &[u8]) -> std::io::Result<()> {
        self.offsets.push(self.pos);
        let n = self.offsets.len();
        self.raw(format!("{n} 0 obj\n<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes())?;
        self.raw(data)?;
        self.raw(b"\nendstream\nendobj\n")
    }
}

fn to_unicode_cmap(used: &BTreeMap<u16, char>) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<_> = used.iter().collect();
    for chunk in entries.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for (gid, c) in chunk {
            let mut utf16 = [0u16; 2];
            let units: String = c
                .encode_utf16(&mut utf16)
                .iter()
                .map(|u| format!("{u:04X}"))
                .collect();
            let _ = writeln!(cmap, "<{gid:04X}> <{units}>");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}

/// Lays out `blocks` on A4 pages and writes the PDF to `output` atomically.
pub fn render(blocks: &[Block], output: &Path) -> Result<(), PdfError> {
    let font = Font::load(FONT)?;
    let mut layout = Layout::new(&font);
    for block in blocks {
        match block {
            Block::Heading(level, text) => layout.heading(*level, text),
            Block::Paragraph(spans) => layout.paragraph(spans, BODY_SIZE, 0.0, false),
            Block::ListItem(spans) => layout.paragraph(spans, BODY_SIZE, LIST_INDENT, true),
            Block::Table(rows) => layout.table(rows),
            Block::PageBreak => layout.new_page(),
        }
    }
    let (pages, used) = layout.finish();

    let dir = output.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut out = Out {
            inner: std::io::BufWriter::new(tmp.as_file_mut()),
            pos: 0,
            offsets: Vec::new(),
        };
        out.raw(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n")?;
        // Fixed objects: 1 catalog, 2 page tree, 3 Type0 font, 4 CID font,
        // 5 descriptor, 6 font file, 7 ToUnicode; then a page and its content per page.
        let first_page = 8;
        let kids: Vec<String> = (0..pages.len())
            .map(|i| format!("{} 0 R", first_page + 2 * i))
            .collect();
        out.object("<< /Type /Catalog /Pages 2 0 R >>")?;
        out.object(&format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        ))?;
        out.object(&format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /{FONT_NAME} /Encoding /Identity-H \
             /DescendantFonts [4 0 R] /ToUnicode 7 0 R >>"
        ))?;
        let widths: String = used
            .keys()
            .map(|g| format!("{g} [{:.0}]", font.advance(*g)))
            .collect::<Vec<_>>()
            .join(" ");
        out.object(&format!(
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{FONT_NAME} \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
             /FontDescriptor 5 0 R /DW 500 /W [{widths}] /CIDToGIDMap /Identity >>"
        ))?;
        let s = 1000.0 / font.units;
        let bbox = font.face.global_bounding_box();
        out.object(&format!(
            "<< /Type /FontDescriptor /FontName /{FONT_NAME} /Flags 32 \
             /FontBBox [{:.0} {:.0} {:.0} {:.0}] /ItalicAngle 0 /Ascent {:.0} /Descent {:.0} \
             /CapHeight {:.0} /StemV 80 /FontFile2 6 0 R >>",
            f32::from(bbox.x_min) * s,
            f32::from(bbox.y_min) * s,
            f32::from(bbox.x_max) * s,
            f32::from(bbox.y_max) * s,
            f32::from(font.face.ascender()) * s,
            f32::from(font.face.descender()) * s,
            f32::from(font.face.capital_height().unwrap_or(700)) * s,
        ))?;
        out.stream(
            &format!("/Filter /FlateDecode /Length1 {}", FONT.len()),
            &zlib(FONT)?,
        )?;
        out.stream(
            "/Filter /FlateDecode",
            &zlib(to_unicode_cmap(&used).as_bytes())?,
        )?;
        for (i, content) in pages.iter().enumerate() {
            let content_obj = first_page + 2 * i + 1;
            out.object(&format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {content_obj} 0 R >>"
            ))?;
            out.stream(
                "/Filter /FlateDecode",
                &zlib(format!("0 g 0 G\n{content}").as_bytes())?,
            )?;
        }
        let xref_at = out.pos;
        let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", out.offsets.len() + 1);
        for offset in &out.offsets {
            let _ = writeln!(xref, "{offset:010} 00000 n ");
        }
        let _ = write!(
            xref,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            out.offsets.len() + 1
        );
        out.raw(xref.as_bytes())?;
        out.inner.flush()?;
    }
    tmp.persist(output).map_err(|e| PdfError::Io(e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Block> {
        vec![
            Block::Heading(1, "Quarterly report".into()),
            Block::Paragraph(vec![
                Span::plain("Sales rose in "),
                Span {
                    text: "every".into(),
                    bold: true,
                    italic: false,
                },
                Span::plain(" region — including Zürich and Москва."),
            ]),
            Block::ListItem(vec![Span::plain("First point")]),
            Block::Table(vec![
                vec!["Region".into(), "Total".into()],
                vec!["North".into(), "1,200".into()],
            ]),
            Block::PageBreak,
            Block::Paragraph(vec![Span::plain("x".repeat(400))]),
        ]
    }

    #[test]
    fn wraps_and_splits_long_words() {
        let font = Font::load(FONT).expect("font");
        let lines = wrap(
            &font,
            &tokenize(&[Span::plain("a ".repeat(200))]),
            10.0,
            200.0,
        );
        assert!(lines.len() > 1);
        let long = wrap(
            &font,
            &tokenize(&[Span::plain("w".repeat(300))]),
            10.0,
            100.0,
        );
        assert!(long.len() > 3, "an overlong word is broken up");
        assert!(
            long.iter()
                .all(|l| l.iter().map(|(w, _, _)| font.width(w, 10.0)).sum::<f32>() <= 100.5)
        );
    }

    #[test]
    fn writes_valid_pdf_with_correct_xref() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("doc.pdf");
        render(&sample(), &out).expect("render");
        let bytes = std::fs::read(&out).expect("read");
        assert!(bytes.starts_with(b"%PDF-1.7"));
        let text = String::from_utf8_lossy(&bytes);
        let xref_at: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|t| t.lines().next())
            .and_then(|n| n.parse().ok())
            .expect("startxref");
        let table = String::from_utf8_lossy(&bytes[xref_at..]);
        for (n, line) in table
            .lines()
            .skip(3)
            .take_while(|l| l.len() >= 18 && l.ends_with('n'))
            .enumerate()
        {
            let offset: usize = line[..10].parse().expect("offset");
            assert!(
                bytes[offset..].starts_with(format!("{} 0 obj", n + 1).as_bytes()),
                "object {}",
                n + 1
            );
        }
        assert!(text.contains("/Count 2"), "page break makes two pages");
    }

    /// With Poppler available, the embedded text must extract back as written.
    #[test]
    fn text_round_trips_through_pdftotext() {
        let Some(pdftotext) = crate::externals::Toolbox::detect(None)
            .get(crate::externals::External::Pdftotext)
            .map(Path::to_path_buf)
        else {
            return;
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let pdf = dir.path().join("doc.pdf");
        render(&sample(), &pdf).expect("render");
        let txt = dir.path().join("doc.txt");
        // UTF-8 like the PDF → TXT tool; Windows builds default to Latin-1.
        let status = std::process::Command::new(pdftotext)
            .args(["-enc", "UTF-8"])
            .arg(&pdf)
            .arg(&txt)
            .status()
            .expect("pdftotext");
        assert!(status.success());
        let extracted = std::fs::read_to_string(&txt).expect("text");
        for needle in [
            "Quarterly report",
            "Sales rose in every region",
            "Zürich",
            "Москва",
            "North",
            "1,200",
        ] {
            assert!(
                extracted.contains(needle),
                "missing {needle:?} in {extracted:?}"
            );
        }
    }
}
