//! Native eBook conversion, so the common conversions need no Calibre:
//! EPUB and FB2 books, and the documents FMX reads natively (DOCX, ODT, HTML,
//! Markdown, RTF, TXT), to EPUB, FB2, TXT or PDF.
//!
//! A book is read into its title, author and [`Block`]s, with
//! [`Block::PageBreak`] between chapters. Kindle output (MOBI, AZW3), and
//! reading MOBI, AZW3, LIT, PDB or PDF, still go through Calibre.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::catalog::EbookFormat;
use crate::office::{self, OfficeError, attr, corrupt, entity};
use crate::pdfgen::{self, Block, Span};

/// Largest part read from an EPUB (guards against zip bombs).
const MAX_PART_BYTES: u64 = 64 << 20;
/// Largest FB2 file read.
const MAX_FB2_BYTES: u64 = 128 << 20;
/// Chapters read from one EPUB, at most.
const MAX_CHAPTERS: usize = 5_000;

/// A book's text and what it is called.
#[derive(Debug, PartialEq)]
pub struct Book {
    /// From the book's own metadata; documents have none.
    pub title: Option<String>,
    pub author: Option<String>,
    pub blocks: Vec<Block>,
}

/// Whether FMX reads `path` without Calibre.
pub fn can_read(path: &Path) -> bool {
    let ext = extension(path);
    ext == "epub" || ext == "fb2" || office::NATIVE_EXTENSIONS.contains(&ext.as_str())
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Reads a book or document.
pub fn read(path: &Path) -> Result<Book, OfficeError> {
    match extension(path).as_str() {
        "epub" => read_epub(path),
        "fb2" => read_fb2(path),
        _ => Ok(Book {
            title: None,
            author: None,
            blocks: office::read_document(path)?,
        }),
    }
}

/// Writes `book` as `format`, which must be [`EbookFormat::is_native`].
/// `fallback_title` names a book without one (its file name).
pub fn write(
    book: &Book,
    format: EbookFormat,
    fallback_title: &str,
    output: &Path,
) -> io::Result<()> {
    let title = book.title.as_deref().unwrap_or(fallback_title);
    match format {
        EbookFormat::Epub => write_epub(book, title, output),
        EbookFormat::Fb2 => replace(output, fb2(book, title).as_bytes()),
        EbookFormat::Txt => replace(output, text(book).as_bytes()),
        EbookFormat::Pdf => {
            let mut blocks = Vec::with_capacity(book.blocks.len() + 1);
            // A book's title opens the PDF, unless its first page already says it.
            if let Some(title) = &book.title
                && !matches!(book.blocks.first(), Some(Block::Heading(_, h)) if h == title)
            {
                blocks.push(Block::Heading(1, title.clone()));
            }
            blocks.extend(book.blocks.iter().cloned());
            pdfgen::render(&blocks, output).map_err(|e| match e {
                pdfgen::PdfError::Io(e) => e,
                pdfgen::PdfError::Font => io::Error::other("the PDF font could not be read"),
            })
        }
        EbookFormat::Mobi | EbookFormat::Azw3 => Err(io::Error::other("needs Calibre")),
    }
}

/// Writes `bytes` to `output` in one step, so a half-written book is never left.
fn replace(output: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = output.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.persist(output).map_err(|e| e.error)?;
    Ok(())
}

// ── Reading EPUB ─────────────────────────────────────────────────────────

fn zip_text(zip: &mut zip::ZipArchive<File>, name: &str) -> Result<String, OfficeError> {
    let part = zip.by_name(name).map_err(corrupt)?;
    let mut bytes = Vec::new();
    part.take(MAX_PART_BYTES).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// What an EPUB's package file (OPF) lists.
#[derive(Default)]
struct Package {
    title: Option<String>,
    author: Option<String>,
    /// Manifest items: id and file.
    items: Vec<(String, String)>,
    /// Reading order, as manifest ids.
    spine: Vec<String>,
}

fn read_epub(path: &Path) -> Result<Book, OfficeError> {
    let mut zip = zip::ZipArchive::new(File::open(path)?).map_err(corrupt)?;
    let container = zip_text(&mut zip, "META-INF/container.xml")?;
    let opf_path = rootfile(&container)?;
    let package = parse_opf(&zip_text(&mut zip, &opf_path)?)?;
    let base = opf_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut blocks = Vec::new();
    for id in package.spine.iter().take(MAX_CHAPTERS) {
        let Some((_, href)) = package.items.iter().find(|(item, _)| item == id) else {
            continue;
        };
        let Ok(html) = zip_text(&mut zip, &resolve(base, href)) else {
            continue;
        };
        let chapter = office::html_blocks(&html);
        if chapter.is_empty() {
            continue;
        }
        if !blocks.is_empty() {
            blocks.push(Block::PageBreak);
        }
        blocks.extend(chapter);
    }
    Ok(Book {
        title: package.title,
        author: package.author,
        blocks,
    })
}

/// The package file named in `META-INF/container.xml`.
fn rootfile(container: &str) -> Result<String, OfficeError> {
    let mut reader = Reader::from_str(container);
    loop {
        match reader.read_event().map_err(corrupt)? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "rootfile" => {
                if let Some(path) = attr(&e, "full-path") {
                    return Ok(path);
                }
            }
            Event::Eof => return Err(corrupt("no package file in the EPUB")),
            _ => {}
        }
    }
}

fn parse_opf(xml: &str) -> Result<Package, OfficeError> {
    let mut reader = Reader::from_str(xml);
    let mut package = Package::default();
    // Which metadata element's text is being read.
    let mut reading: Option<&'static str> = None;
    let mut text = String::new();
    loop {
        match reader.read_event().map_err(corrupt)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                "item" => {
                    if let (Some(id), Some(href)) = (attr(&e, "id"), attr(&e, "href")) {
                        package.items.push((id, href));
                    }
                }
                "itemref" => package.spine.extend(attr(&e, "idref")),
                "title" if package.title.is_none() => reading = Some("title"),
                "creator" if package.author.is_none() => reading = Some("creator"),
                _ => {}
            },
            Event::Text(t) if reading.is_some() => {
                text.push_str(&t.xml_content(XmlVersion::Implicit1_0));
            }
            Event::GeneralRef(r) if reading.is_some() => {
                text.extend(entity(&r.xml_content(XmlVersion::Implicit1_0)));
            }
            Event::End(e) if reading.is_some() => {
                let name = e.local_name();
                let value = Some(clean(&std::mem::take(&mut text))).filter(|v| !v.is_empty());
                match (reading, name.as_ref()) {
                    (Some("title"), "title") => package.title = value,
                    (Some("creator"), "creator") => package.author = value,
                    _ => continue,
                }
                reading = None;
            }
            Event::Eof => return Ok(package),
            _ => {}
        }
    }
}

/// `href`, relative to the package file's folder, as a path inside the zip.
fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or_default();
    let mut parts: Vec<String> = base
        .split('/')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    for part in percent_decode(href).split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            name => parts.push(name.to_owned()),
        }
    }
    parts.join("/")
}

/// Decodes `%20`-style escapes in an EPUB link.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whitespace collapsed to single spaces.
fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── Reading FB2 ──────────────────────────────────────────────────────────

/// FB2 text, decoded from the encoding its XML declaration names (often
/// windows-1251).
fn fb2_text(bytes: &[u8]) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]);
    let label = head.split("encoding=").nth(1).and_then(|rest| {
        rest.trim_start_matches(['"', '\''])
            .split(['"', '\''])
            .next()
    });
    let encoding = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

fn read_fb2(path: &Path) -> Result<Book, OfficeError> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FB2_BYTES)
        .read_to_end(&mut bytes)?;
    parse_fb2(&fb2_text(&bytes))
}

#[derive(Default)]
struct Fb2State {
    blocks: Vec<Block>,
    title: Option<String>,
    author: Vec<String>,
    /// The first author was read; later ones are left out.
    author_done: bool,
    in_title_info: bool,
    /// Text of the element being read in the book's description.
    meta: Option<String>,
    /// Nesting of `<section>`s; chapters are the outermost.
    depth: usize,
    in_heading: bool,
    heading: Vec<String>,
    /// The paragraph (or verse line) being read.
    spans: Option<Vec<Span>>,
    bold: usize,
    italic: usize,
    /// Inside `<binary>` (images), which hold no text.
    skip: bool,
    /// The table being read, its current row, and the cell's text.
    table: Option<Vec<Vec<String>>>,
    row: Vec<String>,
    cell: Option<String>,
}

impl Fb2State {
    fn text(&mut self, text: &str) {
        if self.skip {
            return;
        }
        if let Some(meta) = &mut self.meta {
            meta.push_str(text);
        } else if let Some(cell) = &mut self.cell {
            cell.push_str(text);
        } else if let Some(spans) = &mut self.spans {
            let (bold, italic) = (self.bold > 0, self.italic > 0);
            match spans.last_mut() {
                Some(last) if last.bold == bold && last.italic == italic => {
                    last.text.push_str(text)
                }
                _ => spans.push(Span {
                    text: text.to_owned(),
                    bold,
                    italic,
                }),
            }
        }
    }

    fn end_paragraph(&mut self) {
        let Some(spans) = self.spans.take() else {
            return;
        };
        let line = clean(&spans.iter().map(|s| s.text.as_str()).collect::<String>());
        if line.is_empty() {
            return;
        }
        if self.in_heading {
            self.heading.push(line);
        } else {
            let spans = spans
                .into_iter()
                .map(|s| Span {
                    text: s.text.split_whitespace().collect::<Vec<_>>().join(" "),
                    ..s
                })
                .filter(|s| !s.text.is_empty())
                .collect();
            self.blocks.push(Block::Paragraph(spans));
        }
    }
}

fn parse_fb2(xml: &str) -> Result<Book, OfficeError> {
    let mut reader = Reader::from_str(xml);
    let mut st = Fb2State::default();
    loop {
        match reader.read_event().map_err(corrupt)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "binary" => st.skip = true,
                "title-info" => st.in_title_info = true,
                "book-title" if st.in_title_info => st.meta = Some(String::new()),
                "first-name" | "middle-name" | "last-name"
                    if st.in_title_info && !st.author_done =>
                {
                    st.meta = Some(String::new());
                }
                "section" => {
                    st.end_paragraph();
                    if st.depth == 0 && !st.blocks.is_empty() {
                        st.blocks.push(Block::PageBreak);
                    }
                    st.depth += 1;
                }
                "title" if !st.in_title_info => st.in_heading = true,
                "p" | "v" | "subtitle" | "text-author" => {
                    st.end_paragraph();
                    st.spans = Some(Vec::new());
                }
                "strong" => st.bold += 1,
                "emphasis" => st.italic += 1,
                "table" => {
                    st.end_paragraph();
                    st.table = Some(Vec::new());
                }
                "td" | "th" if st.table.is_some() => st.cell = Some(String::new()),
                _ => {}
            },
            Event::Empty(e) if e.local_name().as_ref() == "empty-line" => st.end_paragraph(),
            Event::Text(t) => st.text(&t.xml_content(XmlVersion::Implicit1_0)),
            Event::GeneralRef(r) => {
                if let Some(c) = entity(&r.xml_content(XmlVersion::Implicit1_0)) {
                    st.text(&c.to_string());
                }
            }
            Event::CData(t) => st.text(&t.into_inner()),
            Event::End(e) => match e.local_name().as_ref() {
                "binary" => st.skip = false,
                "title-info" => st.in_title_info = false,
                "book-title" if st.in_title_info => {
                    st.title = st.meta.take().map(|t| clean(&t)).filter(|t| !t.is_empty());
                }
                // Only the first author is kept, in name order.
                "first-name" | "middle-name" | "last-name" if st.in_title_info => {
                    if let Some(name) = st.meta.take().map(|n| clean(&n))
                        && !name.is_empty()
                    {
                        st.author.push(name);
                    }
                }
                "author" if st.in_title_info => st.author_done = !st.author.is_empty(),
                "section" => {
                    st.end_paragraph();
                    st.depth = st.depth.saturating_sub(1);
                }
                "title" if st.in_heading => {
                    st.end_paragraph();
                    st.in_heading = false;
                    let heading = std::mem::take(&mut st.heading).join(" — ");
                    if !heading.is_empty() {
                        let level = st.depth.clamp(1, 4) as u8;
                        st.blocks.push(Block::Heading(level, heading));
                    }
                }
                "subtitle" => {
                    if let Some(spans) = st.spans.take() {
                        let line =
                            clean(&spans.iter().map(|s| s.text.as_str()).collect::<String>());
                        if !line.is_empty() {
                            let level = (st.depth + 1).clamp(2, 4) as u8;
                            st.blocks.push(Block::Heading(level, line));
                        }
                    }
                }
                "p" | "v" | "text-author" => st.end_paragraph(),
                "td" | "th" => {
                    if let Some(cell) = st.cell.take() {
                        st.row.push(clean(&cell));
                    }
                }
                "tr" => {
                    let row = std::mem::take(&mut st.row);
                    if let Some(table) = &mut st.table {
                        table.push(row);
                    }
                }
                "table" => {
                    if let Some(rows) = st.table.take().filter(|rows| !rows.is_empty()) {
                        st.blocks.push(Block::Table(rows));
                    }
                }
                "strong" => st.bold = st.bold.saturating_sub(1),
                "emphasis" => st.italic = st.italic.saturating_sub(1),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    st.end_paragraph();
    let author = Some(st.author.join(" ")).filter(|a| !a.is_empty());
    Ok(Book {
        title: st.title,
        author,
        blocks: st.blocks,
    })
}

// ── Writing ──────────────────────────────────────────────────────────────

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

fn span_text(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Spans with bold and italic as `bold` / `italic` tags (HTML or FB2 names).
fn styled(spans: &[Span], bold: &str, italic: &str) -> String {
    let mut out = String::new();
    for (i, span) in spans.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut text = escape(&span.text);
        if span.italic {
            text = format!("<{italic}>{text}</{italic}>");
        }
        if span.bold {
            text = format!("<{bold}>{text}</{bold}>");
        }
        out.push_str(&text);
    }
    out
}

/// The book split into chapters: at page breaks, or else at top-level headings.
fn chapters(blocks: &[Block]) -> Vec<&[Block]> {
    let at_breaks: Vec<&[Block]> = blocks
        .split(|b| *b == Block::PageBreak)
        .filter(|c| !c.is_empty())
        .collect();
    if at_breaks.len() > 1 {
        return at_breaks;
    }
    let mut chapters = Vec::new();
    let mut start = 0;
    for (i, block) in blocks.iter().enumerate() {
        if i > start && matches!(block, Block::Heading(1, _)) {
            chapters.push(&blocks[start..i]);
            start = i;
        }
    }
    chapters.push(&blocks[start..]);
    chapters.retain(|c| !c.is_empty());
    chapters
}

fn chapter_title(chapter: &[Block], number: usize) -> String {
    chapter
        .iter()
        .find_map(|b| match b {
            Block::Heading(_, text) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| number.to_string())
}

/// Plain text: headings and paragraphs separated by blank lines.
fn text(book: &Book) -> String {
    let mut out = String::new();
    if let Some(title) = &book.title {
        out.push_str(title);
        out.push('\n');
        if let Some(author) = &book.author {
            out.push_str(author);
            out.push('\n');
        }
        out.push('\n');
    }
    for block in &book.blocks {
        match block {
            Block::Heading(_, text) => out.push_str(&format!("\n{text}\n\n")),
            Block::Paragraph(spans) => out.push_str(&format!("{}\n\n", span_text(spans))),
            Block::ListItem(spans) => out.push_str(&format!("• {}\n", span_text(spans))),
            Block::Table(rows) => {
                for row in rows {
                    out.push_str(&row.join("\t"));
                    out.push('\n');
                }
                out.push('\n');
            }
            Block::PageBreak => out.push_str("\n\n"),
        }
    }
    let trimmed = out.trim_end();
    format!("{trimmed}\n")
}

/// Today's date and time in UTC as `YYYY-MM-DD` and `hh:mm:ss`.
fn now_utc() -> (String, String) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let time = secs % 86_400;
    (
        format!("{year:04}-{month:02}-{day:02}"),
        format!(
            "{:02}:{:02}:{:02}",
            time / 3_600,
            time % 3_600 / 60,
            time % 60
        ),
    )
}

/// An identifier for a new book.
fn book_id(title: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let hash = title.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    format!("urn:fmx:{nanos:x}-{hash:016x}")
}

fn fb2(book: &Book, title: &str) -> String {
    let (date, _) = now_utc();
    let author = book.author.as_deref().unwrap_or("Unknown");
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\">\n\
         <description><title-info><genre>other</genre><author><nickname>{}</nickname></author>\
         <book-title>{}</book-title><lang>und</lang></title-info>\
         <document-info><author><nickname>FMX</nickname></author>\
         <program-used>FMX {}</program-used><date>{date}</date><id>{}</id><version>1.0</version>\
         </document-info></description>\n<body>\n",
        escape(author),
        escape(title),
        crate::update::VERSION,
        book_id(title),
    );
    for chapter in chapters(&book.blocks) {
        out.push_str("<section>\n");
        let mut blocks = chapter.iter().peekable();
        // A chapter's opening heading is its title; later ones are subtitles.
        if let Some(Block::Heading(_, text)) = blocks.peek() {
            out.push_str(&format!("<title><p>{}</p></title>\n", escape(text)));
            blocks.next();
        }
        for block in blocks {
            match block {
                Block::Heading(_, text) => {
                    out.push_str(&format!("<subtitle>{}</subtitle>\n", escape(text)))
                }
                Block::Paragraph(spans) => {
                    out.push_str(&format!("<p>{}</p>\n", styled(spans, "strong", "emphasis")));
                }
                Block::ListItem(spans) => {
                    out.push_str(&format!(
                        "<p>• {}</p>\n",
                        styled(spans, "strong", "emphasis")
                    ));
                }
                Block::Table(rows) => {
                    out.push_str("<table>\n");
                    for row in rows {
                        let cells: String = row
                            .iter()
                            .map(|cell| format!("<td>{}</td>", escape(cell)))
                            .collect();
                        out.push_str(&format!("<tr>{cells}</tr>\n"));
                    }
                    out.push_str("</table>\n");
                }
                Block::PageBreak => {}
            }
        }
        out.push_str("</section>\n");
    }
    out.push_str("</body>\n</FictionBook>\n");
    out
}

fn xhtml_chapter(chapter: &[Block], title: &str) -> String {
    let mut body = String::new();
    let mut in_list = false;
    for block in chapter {
        let item = matches!(block, Block::ListItem(_));
        if in_list && !item {
            body.push_str("</ul>\n");
        } else if !in_list && item {
            body.push_str("<ul>\n");
        }
        in_list = item;
        match block {
            Block::Heading(level, text) => {
                let level = (*level).clamp(1, 6);
                body.push_str(&format!("<h{level}>{}</h{level}>\n", escape(text)));
            }
            Block::Paragraph(spans) => {
                body.push_str(&format!("<p>{}</p>\n", styled(spans, "b", "i")))
            }
            Block::ListItem(spans) => {
                body.push_str(&format!("<li>{}</li>\n", styled(spans, "b", "i")))
            }
            Block::Table(rows) => {
                body.push_str("<table>\n");
                for row in rows {
                    let cells: String = row
                        .iter()
                        .map(|cell| format!("<td>{}</td>", escape(cell)))
                        .collect();
                    body.push_str(&format!("<tr>{cells}</tr>\n"));
                }
                body.push_str("</table>\n");
            }
            Block::PageBreak => {}
        }
    }
    if in_list {
        body.push_str("</ul>\n");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
         <head><meta charset=\"utf-8\"/><title>{}</title>\
         <link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head>\n<body>\n{body}</body>\n</html>\n",
        escape(title)
    )
}

const EPUB_CSS: &str = "body { margin: 0 5%; line-height: 1.5; }\n\
    h1, h2, h3, h4 { line-height: 1.2; margin: 1.2em 0 0.6em; }\n\
    p { margin: 0 0 0.6em; }\n\
    table { border-collapse: collapse; }\n\
    td { border: 1px solid #999; padding: 0.2em 0.4em; }\n";

/// An EPUB 3 book, with an EPUB 2 table of contents too, for older readers.
fn write_epub(book: &Book, title: &str, output: &Path) -> io::Result<()> {
    let (date, time) = now_utc();
    let id = book_id(title);
    let mut chapters = chapters(&book.blocks);
    if chapters.is_empty() {
        chapters.push(&[]);
    }
    let names: Vec<String> = (1..=chapters.len())
        .map(|n| format!("chapter{n:04}.xhtml"))
        .collect();
    let titles: Vec<String> = chapters
        .iter()
        .enumerate()
        .map(|(i, c)| chapter_title(c, i + 1))
        .collect();
    let creator = book
        .author
        .as_ref()
        .map(|a| format!("<dc:creator>{}</dc:creator>", escape(a)))
        .unwrap_or_default();
    let manifest: String = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            format!("<item id=\"c{i}\" href=\"{n}\" media-type=\"application/xhtml+xml\"/>\n")
        })
        .collect();
    let spine: String = (0..names.len())
        .map(|i| format!("<itemref idref=\"c{i}\"/>\n"))
        .collect();
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"book-id\">\n\
         <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
         <dc:identifier id=\"book-id\">{id}</dc:identifier>\n<dc:title>{}</dc:title>\n{creator}\n\
         <dc:language>und</dc:language>\n<meta property=\"dcterms:modified\">{date}T{time}Z</meta>\n\
         </metadata>\n<manifest>\n\
         <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n\
         <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n\
         <item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n{manifest}</manifest>\n\
         <spine toc=\"ncx\">\n{spine}</spine>\n</package>\n",
        escape(title)
    );
    let toc_items: String = names
        .iter()
        .zip(&titles)
        .map(|(n, t)| format!("<li><a href=\"{n}\">{}</a></li>\n", escape(t)))
        .collect();
    let nav = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
         <head><meta charset=\"utf-8\"/><title>{0}</title></head>\n<body>\n\
         <nav epub:type=\"toc\" id=\"toc\"><h1>{0}</h1>\n<ol>\n{toc_items}</ol></nav>\n</body>\n</html>\n",
        escape(title)
    );
    let nav_points: String = names
        .iter()
        .zip(&titles)
        .enumerate()
        .map(|(i, (n, t))| {
            format!(
                "<navPoint id=\"p{i}\" playOrder=\"{}\"><navLabel><text>{}</text></navLabel><content src=\"{n}\"/></navPoint>\n",
                i + 1,
                escape(t)
            )
        })
        .collect();
    let ncx = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n\
         <head><meta name=\"dtb:uid\" content=\"{id}\"/></head>\n\
         <docTitle><text>{}</text></docTitle>\n<navMap>\n{nav_points}</navMap>\n</ncx>\n",
        escape(title)
    );
    let container = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
        <rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles>\n\
        </container>\n";

    let dir = output.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut zip = zip::ZipWriter::new(tmp.as_file_mut());
        let zip_err = |e: zip::result::ZipError| io::Error::other(e.to_string());
        // The `mimetype` file comes first and uncompressed, as readers require.
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).map_err(zip_err)?;
        zip.write_all(b"application/epub+zip")?;
        let mut parts: Vec<(String, String)> = vec![
            ("META-INF/container.xml".into(), container.into()),
            ("OEBPS/content.opf".into(), opf),
            ("OEBPS/nav.xhtml".into(), nav),
            ("OEBPS/toc.ncx".into(), ncx),
            ("OEBPS/style.css".into(), EPUB_CSS.into()),
        ];
        for ((name, chapter), heading) in names.iter().zip(&chapters).zip(&titles) {
            parts.push((format!("OEBPS/{name}"), xhtml_chapter(chapter, heading)));
        }
        for (name, content) in parts {
            zip.start_file(name, deflated).map_err(zip_err)?;
            zip.write_all(content.as_bytes())?;
        }
        zip.finish().map_err(zip_err)?;
    }
    tmp.persist(output).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Book {
        Book {
            title: Some("A Tale & More".into()),
            author: Some("Ada Writer".into()),
            blocks: vec![
                Block::Heading(1, "One".into()),
                Block::Paragraph(vec![
                    Span::plain("It was"),
                    Span {
                        text: "dark".into(),
                        bold: true,
                        italic: false,
                    },
                ]),
                Block::ListItem(vec![Span::plain("first <thing>")]),
                Block::PageBreak,
                Block::Heading(1, "Two".into()),
                Block::Paragraph(vec![Span::plain("שלום, world.")]),
            ],
        }
    }

    fn texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading(_, t) => Some(t.clone()),
                Block::Paragraph(s) | Block::ListItem(s) => Some(span_text(s)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn epub_round_trip_keeps_text_chapters_and_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("book.epub");
        write(&sample(), EbookFormat::Epub, "fallback", &path).expect("write");
        let book = read(&path).expect("read");
        assert_eq!(book.title.as_deref(), Some("A Tale & More"));
        assert_eq!(book.author.as_deref(), Some("Ada Writer"));
        assert_eq!(texts(&book.blocks), texts(&sample().blocks));
        assert_eq!(
            book.blocks
                .iter()
                .filter(|b| **b == Block::PageBreak)
                .count(),
            1
        );
        // The mimetype comes first and uncompressed.
        let bytes = std::fs::read(&path).expect("bytes");
        assert_eq!(&bytes[30..38], b"mimetype");
        assert_eq!(&bytes[38..58], b"application/epub+zip");
    }

    #[test]
    fn fb2_round_trip_keeps_text_styles_and_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("book.fb2");
        write(&sample(), EbookFormat::Fb2, "fallback", &path).expect("write");
        let book = read(&path).expect("read");
        assert_eq!(book.title.as_deref(), Some("A Tale & More"));
        let mut expected = texts(&sample().blocks);
        expected[2] = format!("• {}", expected[2]);
        assert_eq!(texts(&book.blocks), expected);
        assert!(book.blocks.iter().any(|b| matches!(
            b,
            Block::Paragraph(s) if s.iter().any(|s| s.bold && s.text == "dark")
        )));
    }

    #[test]
    fn fb2_in_windows_1251_is_decoded() {
        let xml = "<?xml version=\"1.0\" encoding=\"windows-1251\"?><FictionBook><body><section><p>Привет</p></section></body></FictionBook>";
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode(xml);
        let book = parse_fb2(&fb2_text(&bytes)).expect("parse");
        assert_eq!(texts(&book.blocks), vec!["Привет".to_owned()]);
    }

    #[test]
    fn text_output_reads_naturally() {
        let text = text(&sample());
        assert!(text.starts_with("A Tale & More\nAda Writer\n\n"));
        assert!(text.contains("\nOne\n\nIt was dark\n\n• first <thing>\n"));
    }

    /// Converts a real book to every native format and reads them back:
    /// `FMX_TEST_BOOK=path/to/book.epub cargo test real_book -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a book file in FMX_TEST_BOOK"]
    fn converts_a_real_book() {
        let input =
            std::path::PathBuf::from(std::env::var("FMX_TEST_BOOK").expect("FMX_TEST_BOOK"));
        let book = read(&input).expect("read the book");
        let words: usize = texts(&book.blocks)
            .iter()
            .map(|t| t.split_whitespace().count())
            .sum();
        println!(
            "{:?} by {:?}: {} blocks, {words} words",
            book.title,
            book.author,
            book.blocks.len()
        );
        assert!(words > 100, "too little text was read");
        let dir = tempfile::tempdir().expect("tempdir");
        for format in [
            EbookFormat::Epub,
            EbookFormat::Fb2,
            EbookFormat::Txt,
            EbookFormat::Pdf,
        ] {
            let out = dir.path().join(format!("out.{}", format.ext()));
            write(&book, format, "book", &out).expect("write");
            let size = std::fs::metadata(&out).expect("written").len();
            println!("{}: {size} bytes", format.ext());
            if format == EbookFormat::Txt
                && let Ok(dump) = std::env::var("FMX_TEST_BOOK_TXT")
            {
                std::fs::copy(&out, dump).expect("copy the text");
            }
            if matches!(format, EbookFormat::Epub | EbookFormat::Fb2) {
                let again = read(&out).expect("read back");
                assert_eq!(
                    texts(&again.blocks).len(),
                    texts(&book.blocks).len(),
                    "{}",
                    format.ext()
                );
            }
        }
    }

    #[test]
    fn epub_links_resolve_inside_the_zip() {
        assert_eq!(
            resolve("OEBPS", "Text/ch%201.xhtml#top"),
            "OEBPS/Text/ch 1.xhtml"
        );
        assert_eq!(
            resolve("OEBPS/Text", "../Styles/a.css"),
            "OEBPS/Styles/a.css"
        );
        assert_eq!(resolve("", "a.xhtml"), "a.xhtml");
    }

    #[test]
    fn dates_are_civil() {
        let (date, time) = now_utc();
        assert_eq!(date.len(), 10);
        assert_eq!(time.len(), 8);
        assert!(date.starts_with("20"));
    }
}
