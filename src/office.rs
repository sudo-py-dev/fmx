//! Native reading of office documents into [`Block`]s, and writing DOCX.
//! No office suite is needed: DOCX, ODT, PPTX, ODP, spreadsheets (via
//! calamine), CSV, TXT, Markdown, HTML and RTF are parsed in pure Rust.

use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::i18n::trf;
use crate::pdfgen::{Block, Span};

/// Largest XML part read from a document package (guards against zip bombs).
const MAX_PART_BYTES: u64 = 64 << 20;
/// Largest plain-text input read whole.
const MAX_TEXT_BYTES: u64 = 32 << 20;
/// Spreadsheet rows rendered per sheet.
const MAX_SHEET_ROWS: usize = 5_000;

#[derive(Debug)]
pub enum OfficeError {
    Io(std::io::Error),
    Corrupt(String),
    Unsupported(String),
}

impl fmt::Display for OfficeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => f.write_str(&trf("error.io", &[("error", e)])),
            Self::Corrupt(detail) => f.write_str(&trf("error.bad_document", &[("detail", detail)])),
            Self::Unsupported(ext) => f.write_str(&trf("error.legacy_office", &[("ext", ext)])),
        }
    }
}

impl std::error::Error for OfficeError {}

impl From<std::io::Error> for OfficeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

fn corrupt(e: impl fmt::Display) -> OfficeError {
    OfficeError::Corrupt(e.to_string())
}

/// Formats Fmx reads without any external program.
pub const NATIVE_EXTENSIONS: &[&str] = &[
    "docx", "odt", "pptx", "odp", "xlsx", "xlsm", "xls", "ods", "csv", "tsv", "txt", "md", "html",
    "htm", "rtf",
];

/// Reads a document into blocks, choosing the parser by extension.
pub fn read_document(path: &Path) -> Result<Vec<Block>, OfficeError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "docx" => docx_blocks(&zip_part(path, "word/document.xml")?),
        "odt" => odf_blocks(&zip_part(path, "content.xml")?),
        "odp" => odf_blocks(&zip_part(path, "content.xml")?),
        "pptx" => pptx_blocks(path),
        "xlsx" | "xlsm" | "xls" | "xlsb" | "ods" => spreadsheet_blocks(path),
        "csv" => Ok(vec![Block::Table(parse_delimited(&read_text(path)?, ','))]),
        "tsv" => Ok(vec![Block::Table(parse_delimited(&read_text(path)?, '\t'))]),
        "md" => Ok(markdown_blocks(&read_text(path)?)),
        "html" | "htm" => Ok(html_blocks(&read_text(path)?)),
        "rtf" => Ok(text_blocks(&rtf_text(&read_text(path)?))),
        "txt" => Ok(text_blocks(&read_text(path)?)),
        other => Err(OfficeError::Unsupported(other.to_owned())),
    }
}

fn read_text(path: &Path) -> Result<String, OfficeError> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_TEXT_BYTES)
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn zip_part(path: &Path, name: &str) -> Result<String, OfficeError> {
    let mut archive = zip::ZipArchive::new(File::open(path)?).map_err(corrupt)?;
    let part = archive.by_name(name).map_err(corrupt)?;
    let mut bytes = Vec::new();
    part.take(MAX_PART_BYTES).read_to_end(&mut bytes)?;
    String::from_utf8(bytes).map_err(corrupt)
}

fn attr(e: &BytesStart<'_>, local: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == local)
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// `<w:b/>` turns bold on unless its value says off.
fn toggle_on(e: &BytesStart<'_>) -> bool {
    !matches!(
        attr(e, "val").as_deref(),
        Some("0" | "false" | "off" | "none")
    )
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

fn heading_level(style: &str) -> Option<u8> {
    let lower = style.to_ascii_lowercase();
    if lower == "title" {
        return Some(1);
    }
    let digits = lower
        .strip_prefix("heading")?
        .trim_start_matches(['_', ' ']);
    digits.parse::<u8>().ok().map(|n| n.clamp(1, 4))
}

fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// Collects paragraphs, runs and tables from WordprocessingML.
#[derive(Default)]
struct DocxState {
    blocks: Vec<Block>,
    spans: Vec<Span>,
    style: Option<String>,
    numbered: bool,
    bold: bool,
    italic: bool,
    in_text: bool,
    page_break: bool,
    tables: Vec<Vec<Vec<String>>>,
    row: Vec<String>,
    cell: String,
}

impl DocxState {
    fn push_text(&mut self, text: &str) {
        match self.spans.last_mut() {
            Some(last) if last.bold == self.bold && last.italic == self.italic => {
                last.text.push_str(text)
            }
            _ => self.spans.push(Span {
                text: text.to_owned(),
                bold: self.bold,
                italic: self.italic,
            }),
        }
    }

    fn end_paragraph(&mut self) {
        let spans = std::mem::take(&mut self.spans);
        if !self.tables.is_empty() {
            if !self.cell.is_empty() {
                self.cell.push('\n');
            }
            self.cell.push_str(&spans_text(&spans));
            return;
        }
        let level = self.style.as_deref().and_then(heading_level);
        let block = match level {
            Some(n) => Block::Heading(n, spans_text(&spans)),
            None if self.numbered => Block::ListItem(spans),
            None => Block::Paragraph(spans),
        };
        self.blocks.push(block);
        if std::mem::take(&mut self.page_break) {
            self.blocks.push(Block::PageBreak);
        }
    }
}

fn docx_blocks(xml: &str) -> Result<Vec<Block>, OfficeError> {
    let mut reader = Reader::from_str(xml);
    let mut st = DocxState::default();
    loop {
        match reader.read_event().map_err(corrupt)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                "p" => {
                    st.style = None;
                    st.numbered = false;
                }
                "pStyle" => st.style = attr(&e, "val"),
                "numPr" => st.numbered = true,
                "r" => {
                    st.bold = false;
                    st.italic = false;
                }
                "b" => st.bold = toggle_on(&e),
                "i" => st.italic = toggle_on(&e),
                "t" => st.in_text = true,
                "tab" => st.push_text("\t"),
                "br" | "cr" => {
                    if attr(&e, "type").as_deref() == Some("page") {
                        st.page_break = true;
                    } else {
                        st.push_text("\n");
                    }
                }
                "tbl" => st.tables.push(Vec::new()),
                "tr" => st.row.clear(),
                "tc" => st.cell.clear(),
                _ => {}
            },
            Event::Text(t) if st.in_text => st.push_text(&t.xml_content(XmlVersion::Implicit1_0)),
            Event::GeneralRef(r) if st.in_text => {
                if let Some(c) = entity(&r.xml_content(XmlVersion::Implicit1_0)) {
                    st.push_text(&c.to_string());
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => st.in_text = false,
                "p" => st.end_paragraph(),
                "tc" => {
                    let cell = std::mem::take(&mut st.cell);
                    st.row.push(cell);
                }
                "tr" => {
                    let row = std::mem::take(&mut st.row);
                    if let Some(table) = st.tables.last_mut() {
                        table.push(row);
                    }
                }
                "tbl" => {
                    if let Some(rows) = st.tables.pop() {
                        if st.tables.is_empty() {
                            st.blocks.push(Block::Table(rows));
                        } else {
                            // A nested table becomes text inside its parent cell.
                            let text = rows
                                .iter()
                                .map(|r| r.join(" | "))
                                .collect::<Vec<_>>()
                                .join("\n");
                            st.cell.push_str(&text);
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(st.blocks)
}

/// OpenDocument text and presentations: headings, paragraphs, lists, tables.
fn odf_blocks(xml: &str) -> Result<Vec<Block>, OfficeError> {
    let mut reader = Reader::from_str(xml);
    let mut blocks = Vec::new();
    let mut text = String::new();
    let mut depth_text = 0usize;
    let mut heading: Option<u8> = None;
    let mut list_depth = 0usize;
    let mut tables: Vec<Vec<Vec<String>>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    loop {
        match reader.read_event().map_err(corrupt)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "h" => {
                    depth_text += 1;
                    heading = attr(&e, "outline-level")
                        .and_then(|l| l.parse().ok())
                        .or(Some(1));
                }
                "p" => depth_text += 1,
                "list" => list_depth += 1,
                "table" => tables.push(Vec::new()),
                "table-row" => row.clear(),
                "table-cell" => cell.clear(),
                "page" if !blocks.is_empty() => blocks.push(Block::PageBreak),
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "s" => {
                    let n: usize = attr(&e, "c").and_then(|c| c.parse().ok()).unwrap_or(1);
                    text.push_str(&" ".repeat(n.min(64)));
                }
                "tab" => text.push('\t'),
                "line-break" => text.push('\n'),
                _ => {}
            },
            Event::Text(t) if depth_text > 0 => {
                text.push_str(&t.xml_content(XmlVersion::Implicit1_0))
            }
            Event::GeneralRef(r) if depth_text > 0 => {
                text.extend(entity(&r.xml_content(XmlVersion::Implicit1_0)))
            }
            Event::End(e) => match e.local_name().as_ref() {
                "h" | "p" => {
                    depth_text = depth_text.saturating_sub(1);
                    if depth_text > 0 {
                        continue;
                    }
                    let content = std::mem::take(&mut text);
                    if !tables.is_empty() {
                        if !cell.is_empty() {
                            cell.push('\n');
                        }
                        cell.push_str(&content);
                    } else if let Some(level) = heading.take() {
                        blocks.push(Block::Heading(level.clamp(1, 4), content));
                    } else if list_depth > 0 {
                        blocks.push(Block::ListItem(vec![Span::plain(content)]));
                    } else {
                        blocks.push(Block::Paragraph(vec![Span::plain(content)]));
                    }
                }
                "list" => list_depth = list_depth.saturating_sub(1),
                "table-cell" => row.push(std::mem::take(&mut cell)),
                "table-row" => {
                    let r = std::mem::take(&mut row);
                    if let Some(t) = tables.last_mut() {
                        t.push(r);
                    }
                }
                "table" => {
                    if let Some(rows) = tables.pop() {
                        blocks.push(Block::Table(rows));
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(blocks)
}

/// One page per slide; the first text box becomes the slide's heading.
fn pptx_blocks(path: &Path) -> Result<Vec<Block>, OfficeError> {
    let mut archive = zip::ZipArchive::new(File::open(path)?).map_err(corrupt)?;
    let mut slides: Vec<(u32, String)> = archive
        .file_names()
        .filter_map(|n| {
            let n = n.ok()?;
            let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?;
            Some((num.parse().ok()?, n.to_string()))
        })
        .collect();
    slides.sort();
    let mut blocks = Vec::new();
    for (i, (_, name)) in slides.iter().enumerate() {
        let mut xml = String::new();
        archive
            .by_name(name)
            .map_err(corrupt)?
            .take(MAX_PART_BYTES)
            .read_to_string(&mut xml)?;
        if i > 0 {
            blocks.push(Block::PageBreak);
        }
        let paragraphs = drawingml_paragraphs(&xml)?;
        for (j, p) in paragraphs
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .enumerate()
        {
            blocks.push(if j == 0 {
                Block::Heading(2, p)
            } else {
                Block::ListItem(vec![Span::plain(p)])
            });
        }
    }
    Ok(blocks)
}

fn drawingml_paragraphs(xml: &str) -> Result<Vec<String>, OfficeError> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event().map_err(corrupt)? {
            Event::Start(e) if e.local_name().as_ref() == "t" => in_text = true,
            Event::End(e) if e.local_name().as_ref() == "t" => in_text = false,
            Event::End(e) if e.local_name().as_ref() == "p" => {
                out.push(std::mem::take(&mut current))
            }
            Event::Empty(e) if e.local_name().as_ref() == "br" => current.push('\n'),
            Event::Text(t) if in_text => current.push_str(&t.xml_content(XmlVersion::Implicit1_0)),
            Event::GeneralRef(r) if in_text => {
                current.extend(entity(&r.xml_content(XmlVersion::Implicit1_0)))
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn spreadsheet_blocks(path: &Path) -> Result<Vec<Block>, OfficeError> {
    use calamine::{Data, Reader as _, open_workbook_auto};
    let mut workbook = open_workbook_auto(path).map_err(corrupt)?;
    let mut blocks = Vec::new();
    for name in workbook.sheet_names() {
        let range = workbook.worksheet_range(&name).map_err(corrupt)?;
        let rows: Vec<Vec<String>> = range
            .rows()
            .take(MAX_SHEET_ROWS)
            .map(|r| {
                r.iter()
                    .map(|c| match c {
                        Data::Empty => String::new(),
                        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.0}"),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .filter(|r: &Vec<String>| r.iter().any(|c| !c.is_empty()))
            .collect();
        if !blocks.is_empty() {
            blocks.push(Block::PageBreak);
        }
        blocks.push(Block::Heading(2, name));
        if !rows.is_empty() {
            blocks.push(Block::Table(rows));
        }
    }
    Ok(blocks)
}

/// Paragraphs separated by blank lines; single newlines are kept.
fn text_blocks(text: &str) -> Vec<Block> {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .map(|p| p.trim_end_matches('\n'))
        .filter(|p| !p.trim().is_empty())
        .map(|p| Block::Paragraph(vec![Span::plain(p)]))
        .collect()
}

fn markdown_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            blocks.push(Block::Paragraph(vec![Span::plain(para.join(" "))]));
            para.clear();
        }
    };
    for line in text.lines() {
        let trimmed = line.trim();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Heading(
                hashes.min(4) as u8,
                trimmed[hashes..].trim().to_owned(),
            ));
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            flush(&mut para, &mut blocks);
            blocks.push(Block::ListItem(vec![Span::plain(item)]));
        } else if trimmed.is_empty() {
            flush(&mut para, &mut blocks);
        } else {
            para.push(trimmed.to_owned());
        }
    }
    flush(&mut para, &mut blocks);
    blocks
}

/// A tolerant HTML reader: headings, paragraphs, list items and tables.
fn html_blocks(html: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut text = String::new();
    let mut heading: Option<u8> = None;
    let mut in_item = false;
    let mut skip = 0usize;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut in_table = false;
    let mut rest = html;
    let flush_text =
        |text: &mut String, heading: &mut Option<u8>, in_item: bool, blocks: &mut Vec<Block>| {
            let content = text.split_whitespace().collect::<Vec<_>>().join(" ");
            text.clear();
            if content.is_empty() {
                return;
            }
            blocks.push(match heading.take() {
                Some(n) => Block::Heading(n, content),
                None if in_item => Block::ListItem(vec![Span::plain(content)]),
                None => Block::Paragraph(vec![Span::plain(content)]),
            });
        };
    while let Some(lt) = rest.find('<') {
        if skip == 0 {
            text.push_str(&decode_entities(&rest[..lt]));
        }
        let Some(gt) = rest[lt..].find('>') else {
            break;
        };
        let tag = rest[lt + 1..lt + gt].trim().to_ascii_lowercase();
        rest = &rest[lt + gt + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        match (name.as_str(), closing) {
            ("script" | "style" | "head", false) => skip += 1,
            ("script" | "style" | "head", true) => skip = skip.saturating_sub(1),
            _ if skip > 0 => {}
            ("table", false) => {
                flush_text(&mut text, &mut heading, in_item, &mut blocks);
                in_table = true;
            }
            ("table", true) => {
                in_table = false;
                blocks.push(Block::Table(std::mem::take(&mut rows)));
            }
            ("tr", true) if in_table => rows.push(std::mem::take(&mut row)),
            ("td" | "th", true) if in_table => {
                row.push(text.split_whitespace().collect::<Vec<_>>().join(" "));
                text.clear();
            }
            (h, false) if h.len() == 2 && h.starts_with('h') && h[1..].parse::<u8>().is_ok() => {
                flush_text(&mut text, &mut heading, in_item, &mut blocks);
                heading = h[1..].parse::<u8>().ok().map(|n| n.clamp(1, 4));
            }
            ("li", false) => {
                flush_text(&mut text, &mut heading, in_item, &mut blocks);
                in_item = true;
            }
            ("br", _) => text.push('\n'),
            ("p" | "div" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol", _)
                if !in_table =>
            {
                flush_text(&mut text, &mut heading, in_item, &mut blocks);
                if name == "li" && closing {
                    in_item = false;
                }
            }
            _ => {}
        }
    }
    if skip == 0 {
        text.push_str(&decode_entities(rest));
    }
    flush_text(&mut text, &mut heading, in_item, &mut blocks);
    blocks
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        match after
            .find(';')
            .filter(|&i| i <= 10)
            .and_then(|i| entity(&after[..i]).map(|c| (c, i)))
        {
            Some((c, i)) => {
                out.push(c);
                rest = &after[i + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Plain text from RTF: drops control words and groups such as font tables.
fn rtf_text(rtf: &str) -> String {
    const SKIP_GROUPS: &[&str] = &["fonttbl", "colortbl", "stylesheet", "info", "pict", "*"];
    let mut out = String::new();
    let mut chars = rtf.chars().peekable();
    // Brace depth at which an ignored group started, if any.
    let mut skip_from: Option<usize> = None;
    let mut depth = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' => {
                if skip_from == Some(depth) {
                    skip_from = None;
                }
                depth = depth.saturating_sub(1);
            }
            '\\' => {
                let mut word = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_alphabetic() {
                        word.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if word.is_empty() {
                    match chars.next() {
                        Some('*') if skip_from.is_none() => skip_from = Some(depth),
                        Some('\'') => {
                            let hex: String = chars.by_ref().take(2).collect();
                            if skip_from.is_none()
                                && let Ok(b) = u8::from_str_radix(&hex, 16)
                            {
                                out.push(char::from(b));
                            }
                        }
                        Some(other) if skip_from.is_none() && !other.is_alphanumeric() => {
                            out.push(other)
                        }
                        _ => {}
                    }
                    continue;
                }
                // Optional numeric parameter and one delimiting space.
                let mut param = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_digit() || (n == '-' && param.is_empty()) {
                        param.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                if SKIP_GROUPS.contains(&word.as_str()) && skip_from.is_none() {
                    skip_from = Some(depth);
                }
                if skip_from.is_none() {
                    match word.as_str() {
                        "par" | "line" => out.push('\n'),
                        "tab" => out.push('\t'),
                        "u" => {
                            if let Some(ch) = param
                                .parse::<i32>()
                                .ok()
                                .and_then(|n| char::from_u32(n.rem_euclid(65_536) as u32))
                            {
                                out.push(ch);
                            }
                            // Skip the plain-text fallback that follows a Unicode escape.
                            if chars.peek().is_some_and(|c| !matches!(c, '\\' | '{' | '}')) {
                                chars.next();
                            }
                        }
                        _ => {}
                    }
                }
            }
            '\r' | '\n' => {}
            _ if skip_from.is_none() => out.push(c),
            _ => {}
        }
    }
    out
}

/// Splits CSV/TSV text, honouring double-quoted fields.
pub fn parse_delimited(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', true) => quoted = false,
            ('"', false) if field.is_empty() => quoted = true,
            (d, false) if d == delimiter => row.push(std::mem::take(&mut field)),
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            ('\r', false) => {}
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Control characters other than tab/newline are invalid in XML 1.0.
            c if c.is_control() && c != '\t' && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// Writes a Word document: one paragraph per text paragraph, page breaks kept.
pub fn write_docx(pages: &[Vec<String>], output: &Path) -> Result<(), OfficeError> {
    let mut body = String::new();
    for (i, paragraphs) in pages.iter().enumerate() {
        if i > 0 {
            body.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
        }
        for p in paragraphs {
            body.push_str("<w:p>");
            for (j, line) in p.split('\n').enumerate() {
                if j > 0 {
                    body.push_str("<w:r><w:br/></w:r>");
                }
                body.push_str(&format!(
                    r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
                    xml_escape(line)
                ));
            }
            body.push_str("</w:p>");
        }
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1134" w:right="1134" w:bottom="1134" w:left="1134" w:header="709" w:footer="709" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let parts: [(&str, &str); 3] = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        ),
        ("word/document.xml", &document),
    ];
    let dir = output.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut zip = zip::ZipWriter::new(tmp.as_file_mut());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in parts {
            zip.start_file(name, options).map_err(corrupt)?;
            zip.write_all(content.as_bytes())?;
        }
        zip.finish().map_err(corrupt)?;
    }
    tmp.persist(output).map_err(|e| OfficeError::Io(e.error))?;
    Ok(())
}

/// Splits `pdftotext -layout` output into pages of paragraphs.
pub fn text_pages(text: &str) -> Vec<Vec<String>> {
    let mut pages: Vec<Vec<String>> = text
        .split('\u{c}')
        .map(|page| {
            page.split("\n\n")
                .map(|p| {
                    p.lines()
                        .map(str::trim_end)
                        .collect::<Vec<_>>()
                        .join("\n")
                        .trim_matches('\n')
                        .to_owned()
                })
                .filter(|p| !p.trim().is_empty())
                .collect()
        })
        .collect();
    while pages.last().is_some_and(Vec::is_empty) && pages.len() > 1 {
        pages.pop();
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCX_XML: &str = r#"<w:document xmlns:w="w"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title &amp; more</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Plain </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r><w:r><w:rPr><w:b w:val="0"/></w:rPr><w:t> not</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r><w:r><w:br w:type="page"/></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
</w:body></w:document>"#;

    #[test]
    fn docx_headings_styles_lists_tables_and_breaks() {
        let blocks = docx_blocks(DOCX_XML).expect("parse");
        assert_eq!(blocks[0], Block::Heading(1, "Title & more".into()));
        assert_eq!(
            blocks[1],
            Block::Paragraph(vec![
                Span::plain("Plain "),
                Span {
                    text: "bold".into(),
                    bold: true,
                    italic: false
                },
                Span::plain(" not")
            ])
        );
        assert_eq!(blocks[2], Block::ListItem(vec![Span::plain("item")]));
        assert_eq!(blocks[3], Block::PageBreak);
        assert_eq!(
            blocks[4],
            Block::Table(vec![vec!["A1".into(), "B1".into()]])
        );
    }

    #[test]
    fn odf_reads_headings_spaces_and_tables() {
        let xml = r#"<office:document-content xmlns:office="o" xmlns:text="t" xmlns:table="b"><office:body><office:text>
<text:h text:outline-level="2">Intro</text:h><text:p>a<text:s text:c="2"/>b</text:p>
<text:list><text:list-item><text:p>one</text:p></text:list-item></text:list>
<table:table><table:table-row><table:table-cell><text:p>x</text:p></table:table-cell></table:table-row></table:table>
</office:text></office:body></office:document-content>"#;
        let blocks = odf_blocks(xml).expect("parse");
        assert_eq!(blocks[0], Block::Heading(2, "Intro".into()));
        assert_eq!(blocks[1], Block::Paragraph(vec![Span::plain("a  b")]));
        assert_eq!(blocks[2], Block::ListItem(vec![Span::plain("one")]));
        assert_eq!(blocks[3], Block::Table(vec![vec!["x".into()]]));
    }

    #[test]
    fn csv_handles_quotes_and_commas() {
        let rows = parse_delimited("a,\"b, c\",\"say \"\"hi\"\"\"\n1,2,3\n", ',');
        assert_eq!(
            rows,
            vec![vec!["a", "b, c", "say \"hi\""], vec!["1", "2", "3"]]
        );
    }

    #[test]
    fn html_and_markdown_structure() {
        let html = "<html><head><title>x</title></head><body><h1>Top</h1><p>One &amp; two</p><ul><li>it</li></ul><table><tr><td>c1</td><td>c2</td></tr></table><script>bad()</script></body></html>";
        let blocks = html_blocks(html);
        assert_eq!(blocks[0], Block::Heading(1, "Top".into()));
        assert_eq!(blocks[1], Block::Paragraph(vec![Span::plain("One & two")]));
        assert_eq!(blocks[2], Block::ListItem(vec![Span::plain("it")]));
        assert_eq!(
            blocks[3],
            Block::Table(vec![vec!["c1".into(), "c2".into()]])
        );
        assert!(!format!("{blocks:?}").contains("bad()"));
        let md = markdown_blocks("# Head\n\nsome\ntext\n\n- point");
        assert_eq!(
            md,
            vec![
                Block::Heading(1, "Head".into()),
                Block::Paragraph(vec![Span::plain("some text")]),
                Block::ListItem(vec![Span::plain("point")])
            ]
        );
    }

    #[test]
    fn rtf_drops_control_groups() {
        let rtf = r"{\rtf1\ansi{\fonttbl{\f0 Arial;}}\f0 Hello \b world\b0\par caf\'e9 \u8364?}";
        let text = rtf_text(rtf);
        assert!(text.starts_with("Hello world\ncafé €"), "{text:?}");
        assert!(!text.contains("Arial"));
    }

    #[test]
    fn docx_round_trip_through_writer_and_reader() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("t.docx");
        write_docx(
            &[
                vec!["Hello <world> & co".into(), "line1\nline2".into()],
                vec!["page two".into()],
            ],
            &out,
        )
        .expect("write");
        let blocks = read_document(&out).expect("read back");
        let text = format!("{blocks:?}");
        assert!(
            text.contains("Hello <world> & co") && text.contains("page two"),
            "{text}"
        );
        assert!(blocks.contains(&Block::PageBreak));
    }

    #[test]
    fn layout_text_splits_into_pages_and_paragraphs() {
        let pages = text_pages("Title\n\nBody line 1\nBody line 2\n\u{c}Next page\n\u{c}");
        assert_eq!(
            pages,
            vec![
                vec!["Title".to_owned(), "Body line 1\nBody line 2".to_owned()],
                vec!["Next page".to_owned()]
            ]
        );
    }
}
