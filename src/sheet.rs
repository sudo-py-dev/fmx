//! PDF → Excel: lays out each page's text with `pdftotext -layout`, then
//! splits it into table cells on runs of two or more spaces.

use std::path::Path;

use rust_xlsxwriter::{Workbook, XlsxError};

/// Excel's limits on rows and columns per sheet.
const MAX_ROWS: usize = 1_048_576;
const MAX_COLS: usize = 16_384;

/// A cell value: numbers are stored as numbers so formulas work on them.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Text(String),
    Number(f64),
}

/// Splits laid-out text into pages (form feeds), rows (lines) and cells (2+ spaces).
pub fn parse_pages(text: &str) -> Vec<Vec<Vec<Cell>>> {
    let pages: Vec<Vec<Vec<Cell>>> = text
        .split('\u{c}')
        .map(|page| {
            page.lines()
                .map(str::trim_end)
                .filter(|line| !line.trim().is_empty())
                .map(split_row)
                .collect()
        })
        .collect();
    // pdftotext ends with a form feed, leaving an empty trailing page.
    let keep = pages
        .iter()
        .rposition(|p| !p.is_empty())
        .map_or(1, |i| i + 1);
    pages.into_iter().take(keep).collect()
}

fn split_row(line: &str) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut spaces = 0usize;
    for c in line.trim_start().chars() {
        if c == ' ' {
            spaces += 1;
            continue;
        }
        if spaces >= 2 && !current.is_empty() {
            cells.push(to_cell(&current));
            current.clear();
        } else if spaces == 1 && !current.is_empty() {
            current.push(' ');
        }
        spaces = 0;
        current.push(c);
    }
    if !current.is_empty() {
        cells.push(to_cell(&current));
    }
    cells
}

fn to_cell(text: &str) -> Cell {
    let plain: String = text.chars().filter(|c| *c != ',').collect();
    match plain.parse::<f64>() {
        Ok(n) if n.is_finite() && plain.chars().any(|c| c.is_ascii_digit()) => Cell::Number(n),
        _ => Cell::Text(text.to_owned()),
    }
}

/// Writes one worksheet per page.
pub fn write_xlsx(
    pages: &[Vec<Vec<Cell>>],
    output: &Path,
    sheet_name: impl Fn(usize) -> String,
) -> Result<(), XlsxError> {
    let mut workbook = Workbook::new();
    for (index, rows) in pages.iter().enumerate() {
        let sheet = workbook.add_worksheet();
        sheet.set_name(sheet_name(index + 1))?;
        for (r, row) in rows.iter().take(MAX_ROWS).enumerate() {
            for (c, cell) in row.iter().take(MAX_COLS).enumerate() {
                let (r, c) = (r as u32, c as u16);
                match cell {
                    Cell::Number(n) => sheet.write_number(r, c, *n)?,
                    Cell::Text(t) => sheet.write_string(r, c, t)?,
                };
            }
        }
    }
    workbook.save(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_columns_on_wide_gaps_and_keeps_numbers() {
        let pages = parse_pages(
            "Item name    Qty   Price\nBlue pen      3     1,250.50\n\u{c}Page two\n\u{c}",
        );
        assert_eq!(pages.len(), 2);
        assert_eq!(
            pages[0][0],
            vec![
                Cell::Text("Item name".into()),
                Cell::Text("Qty".into()),
                Cell::Text("Price".into())
            ]
        );
        assert_eq!(
            pages[0][1],
            vec![
                Cell::Text("Blue pen".into()),
                Cell::Number(3.0),
                Cell::Number(1250.5)
            ]
        );
        assert_eq!(pages[1][0], vec![Cell::Text("Page two".into())]);
    }

    #[test]
    fn writes_a_readable_workbook() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("t.xlsx");
        write_xlsx(&parse_pages("a  1\nb  2\n"), &out, |n| format!("Page {n}")).expect("xlsx");
        let bytes = std::fs::read(&out).expect("read");
        assert!(bytes.starts_with(b"PK"), "xlsx is a zip file");
    }
}
