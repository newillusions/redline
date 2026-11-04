//! The Markups List as a PDF comment summary: a landscape A4 table, header repeated on every
//! page, long cells wrapped. Text only, in the built-in Helvetica (WinAnsi), so the file needs
//! no embedded font; characters outside Latin-1 print as `?`. Cell text is mark-up data taken
//! from PDFs and is untrusted: it is escaped into PDF string syntax and never interpreted.

use std::path::Path;

use anyhow::{Context, Result};
use lopdf::{dictionary, Document, Object, Stream};

const PAGE_W: f32 = 841.89;
const PAGE_H: f32 = 595.28;
const MARGIN: f32 = 28.0;
const FONT_PT: f32 = 7.5;
const LEADING: f32 = 9.0;
const CELL_PAD: f32 = 3.0;
/// Rows taller than this many lines are cut (an essay-length comment must not eat a page).
const MAX_LINES_PER_ROW: usize = 12;

/// Approximate Helvetica advance width (em fraction) of `c`. Deliberately a little wide so a
/// wrapped line never runs into the next column.
fn glyph_em(c: char) -> f32 {
    match c {
        'i' | 'l' | 'j' | '.' | ',' | ';' | ':' | '\'' | '|' | '!' | ' ' | 'I' | 'f' | 't'
        | 'r' => 0.30,
        'm' | 'w' | 'M' | 'W' | '@' | '%' => 0.88,
        'A'..='Z' | '0'..='9' => 0.70,
        _ => 0.58,
    }
}

fn text_width(s: &str) -> f32 {
    s.chars().map(glyph_em).sum::<f32>() * FONT_PT
}

/// `s` mapped to what the WinAnsi font can show: Latin-1 letters and symbols kept as they
/// are (e, u, degree, superscripts, micro), controls to a space, anything beyond Latin-1 `?`.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| match c as u32 {
            0..=31 | 0x7f..=0x9f => ' ',
            0x20..=0x7e | 0xa0..=0xff => c,
            _ => '?',
        })
        .collect()
}

/// The bytes of [`clean`]ed text, one per character (Latin-1 equals WinAnsi above 0xA0).
fn to_latin1(s: &str) -> Vec<u8> {
    clean(s).chars().map(|c| c as u32 as u8).collect()
}

/// Greedy wrap of `text` into lines no wider than `width` pt; a word wider than a line is
/// split mid-word. At most [`MAX_LINES_PER_ROW`] lines; the last ends in `...` when cut.
fn wrap(text: &str, width: f32) -> Vec<String> {
    let text = clean(text);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        loop {
            let sep = if cur.is_empty() { "" } else { " " };
            if text_width(&format!("{cur}{sep}{word}")) <= width {
                cur = format!("{cur}{sep}{word}");
                break;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                continue;
            }
            // A single word wider than the cell: take as many characters as fit.
            let mut take = 0;
            let mut w = 0.0;
            for c in word.chars() {
                w += glyph_em(c) * FONT_PT;
                if w > width && take > 0 {
                    break;
                }
                take += 1;
            }
            let (head, tail) =
                word.split_at(word.char_indices().nth(take).map_or(word.len(), |(i, _)| i));
            lines.push(head.to_string());
            word = tail.to_string();
            if word.is_empty() {
                break;
            }
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > MAX_LINES_PER_ROW {
        lines.truncate(MAX_LINES_PER_ROW);
        if let Some(last) = lines.last_mut() {
            last.push_str("...");
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// A PDF literal string for `bytes`: parentheses and backslash escaped, bytes above 126 and
/// controls written as octal escapes, so nothing in a cell can end the string early.
fn pdf_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![b'('];
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            32..=126 => out.push(b),
            _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
        }
    }
    out.push(b')');
    out
}

fn put_text(content: &mut Vec<u8>, x: f32, y: f32, bold: bool, text: &str) {
    content.extend_from_slice(
        format!(
            "BT /{} {FONT_PT} Tf {x:.2} {y:.2} Td ",
            if bold { "F2" } else { "F1" }
        )
        .as_bytes(),
    );
    content.extend_from_slice(&pdf_string(&to_latin1(text)));
    content.extend_from_slice(b" Tj ET\n");
}

/// Write the table to `path`. `weights` gives each column's share of the page width.
pub fn write_summary_pdf(
    path: &Path,
    title: &str,
    headers: &[String],
    rows: &[Vec<String>],
    weights: &[f32],
) -> Result<()> {
    anyhow::ensure!(!headers.is_empty(), "no columns to write");
    anyhow::ensure!(
        weights.len() == headers.len(),
        "one width weight per column"
    );
    let usable = PAGE_W - 2.0 * MARGIN;
    let total: f32 = weights.iter().sum();
    let widths: Vec<f32> = weights.iter().map(|w| usable * w / total).collect();
    let xs: Vec<f32> = widths
        .iter()
        .scan(MARGIN, |x, w| {
            let at = *x;
            *x += w;
            Some(at)
        })
        .collect();

    // Lay the rows out onto pages first, so every page can say "Page n of N".
    let header_cells: Vec<Vec<String>> = headers
        .iter()
        .zip(&widths)
        .map(|(h, w)| wrap(h, w - 2.0 * CELL_PAD))
        .collect();
    let header_lines = header_cells.iter().map(Vec::len).max().unwrap_or(1);
    let header_h = header_lines as f32 * LEADING + 2.0 * CELL_PAD;
    let top = PAGE_H - MARGIN - 14.0; // below the title line
    let bottom = MARGIN + 12.0; // above the footer
    let mut pages: Vec<Vec<(f32, Vec<Vec<String>>)>> = vec![Vec::new()];
    let mut y = top - header_h;
    for row in rows {
        let cells: Vec<Vec<String>> = row
            .iter()
            .zip(&widths)
            .map(|(t, w)| wrap(t, w - 2.0 * CELL_PAD))
            .collect();
        let lines = cells.iter().map(Vec::len).max().unwrap_or(1);
        let h = lines as f32 * LEADING + 2.0 * CELL_PAD;
        if y - h < bottom && !pages.last().is_some_and(Vec::is_empty) {
            pages.push(Vec::new());
            y = top - header_h;
        }
        pages.last_mut().unwrap().push((h, cells));
        y -= h;
    }

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let f1 = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });
    let f2 = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica-Bold", "Encoding" => "WinAnsiEncoding",
    });
    let resources = dictionary! { "Font" => dictionary! { "F1" => f1, "F2" => f2 } };
    let n_pages = pages.len();
    let mut kids = Vec::new();
    for (pi, page_rows) in pages.iter().enumerate() {
        let mut c: Vec<u8> = Vec::new();
        put_text(&mut c, MARGIN, PAGE_H - MARGIN - 6.0, true, title);
        let mut y = top;
        for (cells, x) in header_cells.iter().zip(&xs) {
            for (li, line) in cells.iter().enumerate() {
                put_text(
                    &mut c,
                    x + CELL_PAD,
                    y - CELL_PAD - FONT_PT - li as f32 * LEADING,
                    true,
                    line,
                );
            }
        }
        y -= header_h;
        c.extend_from_slice(
            format!("0.5 w {MARGIN} {y:.2} m {} {y:.2} l S\n", PAGE_W - MARGIN).as_bytes(),
        );
        for (h, cells) in page_rows {
            for (cell, x) in cells.iter().zip(&xs) {
                for (li, line) in cell.iter().enumerate() {
                    put_text(
                        &mut c,
                        x + CELL_PAD,
                        y - CELL_PAD - FONT_PT - li as f32 * LEADING,
                        false,
                        line,
                    );
                }
            }
            y -= h;
            c.extend_from_slice(
                format!("0.25 w {MARGIN} {y:.2} m {} {y:.2} l S\n", PAGE_W - MARGIN).as_bytes(),
            );
        }
        put_text(
            &mut c,
            MARGIN,
            MARGIN,
            false,
            &format!("Page {} of {n_pages}  -  {} mark-ups", pi + 1, rows.len()),
        );
        let content_id = doc.add_object(Stream::new(dictionary! {}, c));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(PAGE_W), Object::Real(PAGE_H)],
            "Resources" => resources.clone(),
            "Contents" => content_id,
        });
        kids.push(Object::Reference(page_id));
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => kids, "Count" => n_pages as i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.compress();
    doc.save(path)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn headers() -> Vec<String> {
        ["#", "Page", "Subject", "Comment"]
            .map(String::from)
            .to_vec()
    }
    const W: [f32; 4] = [0.6, 0.6, 2.0, 4.0];

    fn page_text(doc: &Document, page: u32) -> String {
        doc.extract_text(&[page]).unwrap()
    }

    #[test]
    fn a_short_table_is_one_page_with_header_title_footer_and_rows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s.pdf");
        let rows = vec![
            vec![
                "1".into(),
                "3".into(),
                "Rectangle".into(),
                "Move the downlight 200 mm east".into(),
            ],
            vec!["2".into(), "4".into(), "Cloud".into(), "Revise".into()],
        ];
        write_summary_pdf(&path, "Markups summary: plan.pdf", &headers(), &rows, &W).unwrap();
        let doc = Document::load(&path).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
        let t = page_text(&doc, 1);
        for needle in [
            "Markups summary: plan.pdf",
            "Subject",
            "Comment",
            "Move the downlight",
            "Page 1 of 1",
            "2 mark-ups",
        ] {
            assert!(t.contains(needle), "missing {needle:?} in {t:?}");
        }
    }

    #[test]
    fn many_rows_flow_onto_further_pages_and_each_repeats_the_header() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("m.pdf");
        let rows: Vec<Vec<String>> = (1..=200)
            .map(|i| {
                vec![
                    i.to_string(),
                    "1".into(),
                    "Note".into(),
                    format!("comment number {i}"),
                ]
            })
            .collect();
        write_summary_pdf(&path, "t", &headers(), &rows, &W).unwrap();
        let doc = Document::load(&path).unwrap();
        let n = doc.get_pages().len();
        assert!(n >= 3, "200 rows should need several pages, got {n}");
        for p in 1..=n as u32 {
            assert!(
                page_text(&doc, p).contains("Comment"),
                "page {p} lacks the header"
            );
        }
        assert!(page_text(&doc, n as u32).contains(&format!("Page {n} of {n}")));
        assert!(page_text(&doc, n as u32).contains("comment number 200"));
    }

    #[test]
    fn hostile_cell_text_cannot_break_out_of_its_string() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("h.pdf");
        let evil = r"a) Tj ET 0 0 m 999 999 l S BT (b \) ( \\ and 日本語 é";
        let rows = vec![vec!["1".into(), "1".into(), "x".into(), evil.into()]];
        write_summary_pdf(&path, "t", &headers(), &rows, &W).unwrap();
        let doc = Document::load(&path).unwrap();
        assert_eq!(doc.get_pages().len(), 1, "still parses as one page");
        let t = page_text(&doc, 1);
        assert!(
            t.contains("Tj ET 0 0 m 999 999 l S BT"),
            "the text stays text: {t:?}"
        );
        assert!(t.contains("???"), "non-Latin-1 shows as ?: {t:?}");
    }

    #[test]
    fn latin1_letters_and_symbols_survive_in_the_text_and_the_wrap() {
        assert_eq!(clean("Müller é ü ° ² ³ µ"), "Müller é ü ° ² ³ µ");
        assert_eq!(
            wrap("Müller 90° m² µ", 500.0),
            vec!["Müller 90° m² µ".to_string()]
        );
        assert_eq!(to_latin1("é°²³µ"), vec![0xe9, 0xb0, 0xb2, 0xb3, 0xb5]);
        // Through the writer: the bytes land in the content stream (compressed, so decode it).
        let dir = tempdir().unwrap();
        let path = dir.path().join("l.pdf");
        let rows = vec![vec![
            "1".into(),
            "1".into(),
            "Müller".into(),
            "90° m²".into(),
        ]];
        write_summary_pdf(&path, "t", &headers(), &rows, &W).unwrap();
        let doc = Document::load(&path).unwrap();
        let (_, id) = doc.get_pages().into_iter().next().unwrap();
        let content = doc.get_page_content(id);
        let has = |needle: &[u8]| content.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"(M\\374ller)"), "u-umlaut written as octal 374");
        assert!(
            has(b"(90\\260 m\\262)"),
            "degree and superscript two written as octal"
        );
        assert!(!has(b"M?ller"));
    }

    #[test]
    fn an_empty_view_is_still_a_valid_one_page_pdf() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("e.pdf");
        write_summary_pdf(&path, "t", &headers(), &[], &W).unwrap();
        assert_eq!(Document::load(&path).unwrap().get_pages().len(), 1);
    }

    #[test]
    fn wrap_splits_long_words_and_caps_a_runaway_row() {
        let lines = wrap(&"x".repeat(400), 60.0);
        assert!(lines.len() > 1 && lines.iter().all(|l| text_width(l) <= 60.0 + FONT_PT));
        let essay = "word ".repeat(5000);
        let capped = wrap(&essay, 80.0);
        assert_eq!(capped.len(), MAX_LINES_PER_ROW);
        assert!(capped.last().unwrap().ends_with("..."));
    }

    #[test]
    fn mismatched_column_weights_are_an_error_not_a_panic() {
        let dir = tempdir().unwrap();
        assert!(
            write_summary_pdf(&dir.path().join("x.pdf"), "t", &headers(), &[], &[1.0]).is_err()
        );
    }
}
