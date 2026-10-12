//! Markups List export: the filtered, column-picked view of the mark-up list as CSV or XLSX.
//!
//! The frontend sends the ids in the order the list currently shows them (after filter and
//! sort) plus the visible column keys; this module resolves both against the backend store
//! and writes the file. The fixed-column whole-document export stays in `takeoff.rs`.
//!
//! Cell text is mark-up data taken from PDFs and is untrusted: text cells that a spreadsheet
//! would read as a formula (`=`, `+`, `-`, `@`, tab, CR) are written with a leading `'`.

use std::path::{Path, PathBuf};

use tauri::State;

use super::takeoff::ExportFormat;
use crate::markup::Markup;
use crate::AppState;

/// Upper bound on columns in one export (a custom column per measurement key is the only
/// open-ended source).
pub const MAX_EXPORT_COLUMNS: usize = 64;

/// Longest accepted custom column name (characters). Names come from `Measurement.custom_columns`
/// persisted in the PDF, so they are untrusted: capped here and control characters refused.
pub const MAX_CUSTOM_NAME_CHARS: usize = 64;

/// One exportable column. Keys mirror `src/lib/markups-list.ts` `COLUMN_DEFS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListColumn {
    Number,
    Type,
    Subject,
    Page,
    Comment,
    Author,
    Date,
    Modified,
    Status,
    Replies,
    Colour,
    Layer,
    Measurement,
    StatusBy,
    StatusAt,
    Custom(String),
}

impl ListColumn {
    pub fn parse(key: &str) -> Result<Self, String> {
        Ok(match key {
            "number" => Self::Number,
            "type" => Self::Type,
            "subject" => Self::Subject,
            "page" => Self::Page,
            "comment" => Self::Comment,
            "author" => Self::Author,
            "date" => Self::Date,
            "modified" => Self::Modified,
            "status" => Self::Status,
            "replies" => Self::Replies,
            "colour" => Self::Colour,
            "layer" => Self::Layer,
            "measurement" => Self::Measurement,
            "status_by" => Self::StatusBy,
            "status_at" => Self::StatusAt,
            other => match other.strip_prefix("custom:") {
                Some(name)
                    if !name.is_empty()
                        && name.chars().count() <= MAX_CUSTOM_NAME_CHARS
                        && !name.chars().any(char::is_control) =>
                {
                    Self::Custom(name.to_string())
                }
                _ => return Err(format!("unknown markups-list column: {other}")),
            },
        })
    }

    pub fn header(&self) -> String {
        match self {
            Self::Number => "#".into(),
            Self::Type => "Type".into(),
            Self::Subject => "Subject".into(),
            Self::Page => "Page".into(),
            Self::Comment => "Comments".into(),
            Self::Author => "Author".into(),
            Self::Date => "Date".into(),
            Self::Modified => "Modified".into(),
            Self::Status => "Status".into(),
            Self::Replies => "Replies".into(),
            Self::Colour => "Colour".into(),
            Self::Layer => "Layer".into(),
            Self::Measurement => "Measurement".into(),
            Self::StatusBy => "Status set by".into(),
            Self::StatusAt => "Status set at".into(),
            Self::Custom(name) => name.clone(),
        }
    }

    fn is_numeric(&self) -> bool {
        matches!(self, Self::Number | Self::Page | Self::Replies)
    }

    /// True when the cell text originates in the PDF (and so can carry a formula). Columns the
    /// app formats itself (type, dates, status, replies, measurement) never start with
    /// attacker-chosen text; a negative measurement must stay numeric-looking.
    fn is_untrusted_text(&self) -> bool {
        matches!(
            self,
            Self::Subject
                | Self::Comment
                | Self::Author
                | Self::Layer
                | Self::Colour
                | Self::StatusBy
                | Self::Custom(_)
        )
    }
}

fn stamp(t: &chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%d %H:%M").to_string()
}

/// Cell text for `m`; `number` is its 1-based position in the exported view.
fn cell(m: &Markup, number: usize, col: &ListColumn) -> String {
    match col {
        ListColumn::Number => number.to_string(),
        ListColumn::Type => format!("{:?}", m.markup_type),
        ListColumn::Subject => m
            .subject
            .clone()
            .unwrap_or_else(|| format!("{:?}", m.markup_type)),
        ListColumn::Page => (m.page + 1).to_string(),
        ListColumn::Comment => m.contents.clone().unwrap_or_default(),
        ListColumn::Author => m.audit.created_by.display_name.clone(),
        ListColumn::Date => stamp(&m.audit.created_at),
        ListColumn::Modified => stamp(&m.audit.modified_at),
        ListColumn::Status => format!("{:?}", m.workflow.status),
        ListColumn::Replies => m.workflow.reply_count().to_string(),
        ListColumn::Colour => m.appearance.color.clone(),
        ListColumn::Layer => m.layer.clone().unwrap_or_default(),
        ListColumn::Measurement => m
            .measurement
            .as_ref()
            .map(|meas| {
                if matches!(m.markup_type, crate::markup::MarkupType::MeasurementCount) {
                    format!("{:.0}", f64::from(meas.count_value.unwrap_or(1)))
                } else {
                    format!("{:.2} {}", meas.computed_quantity, meas.unit)
                        .trim_end()
                        .to_string()
                }
            })
            .unwrap_or_default(),
        ListColumn::StatusBy => m
            .workflow
            .latest_review_change()
            .map(|c| c.author.display_name.clone())
            .unwrap_or_default(),
        ListColumn::StatusAt => m
            .workflow
            .latest_review_change()
            .map(|c| stamp(&c.at))
            .unwrap_or_default(),
        ListColumn::Custom(name) => m
            .measurement
            .as_ref()
            .and_then(|meas| meas.custom_columns.get(name))
            .cloned()
            .unwrap_or_default(),
    }
}

/// Neutralise spreadsheet formula injection in an untrusted text cell: when the value, left
/// trimmed, starts with `= + - @`, TAB, CR or a full-width `= + - @` (U+FF1D U+FF0B U+FF0D
/// U+FF20), prefix a single quote so Excel and LibreOffice read it as text. Shared by every
/// CSV export in the app. Only for text that came from a PDF; numbers the app formats itself
/// (a negative quantity, a page number) must stay unquoted so they remain numeric.
pub(crate) fn neutralise_formula(s: &str) -> String {
    let lead = s.trim_start();
    let risky = matches!(
        lead.chars().next(),
        Some(
            '=' | '+' | '-' | '@' | '\t' | '\r' | '\u{FF1D}' | '\u{FF0B}' | '\u{FF0D}' | '\u{FF20}'
        )
    ) || matches!(s.chars().next(), Some('\t' | '\r'));
    if risky {
        format!("'{s}")
    } else {
        s.to_string()
    }
}

/// Resolve `ids` (in view order) against `all`, skipping ids that no longer exist.
fn pick<'a>(all: &'a [Markup], ids: &[String]) -> Vec<&'a Markup> {
    let by_id: std::collections::HashMap<String, &Markup> =
        all.iter().map(|m| (m.id().to_string(), m)).collect();
    ids.iter().filter_map(|id| by_id.get(id).copied()).collect()
}

fn parse_columns(keys: &[String]) -> Result<Vec<ListColumn>, String> {
    if keys.is_empty() {
        return Err("no columns selected".into());
    }
    if keys.len() > MAX_EXPORT_COLUMNS {
        return Err(format!("too many columns (max {MAX_EXPORT_COLUMNS})"));
    }
    keys.iter().map(|k| ListColumn::parse(k)).collect()
}

/// Write the view to `path`. Returns the number of rows written.
pub fn export_view_to(
    all: &[Markup],
    ids: &[String],
    column_keys: &[String],
    path: &Path,
    format: ExportFormat,
) -> Result<usize, String> {
    export_view_with_scales(all, &[], ids, column_keys, path, format)
}

/// [`export_view_to`] that first recomputes scaled quantities from `raw_measure` under the
/// document's current scales, so this export shows the same totals as the Quantities one.
pub fn export_view_with_scales(
    all: &[Markup],
    scales: &[crate::takeoff::ScaleRecord],
    ids: &[String],
    column_keys: &[String],
    path: &Path,
    format: ExportFormat,
) -> Result<usize, String> {
    let mut recomputed = all.to_vec();
    crate::takeoff::recompute_all(&mut recomputed, scales);
    let all = &recomputed[..];
    let cols = parse_columns(column_keys)?;
    // A count item stands for its whole count: show the group total, not 1.
    let picked = pick(all, ids);
    let with_totals = crate::takeoff::count::with_group_totals(all, &picked);
    let rows: Vec<&Markup> = with_totals.iter().collect();
    match format {
        ExportFormat::Csv => write_csv(&rows, &cols, path).map_err(|e| e.to_string())?,
        ExportFormat::Xlsx => write_xlsx(&rows, &cols, path).map_err(|e| e.to_string())?,
    }
    Ok(rows.len())
}

fn write_csv(
    rows: &[&Markup],
    cols: &[ListColumn],
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut wtr = csv::Writer::from_path(path)?;
    // Headers can carry untrusted text (custom column names), so they are neutralised too.
    wtr.write_record(cols.iter().map(|c| neutralise_formula(&c.header())))?;
    for (i, m) in rows.iter().enumerate() {
        wtr.write_record(cols.iter().map(|c| {
            let text = cell(m, i + 1, c);
            if c.is_untrusted_text() {
                neutralise_formula(&text)
            } else {
                text
            }
        }))?;
    }
    wtr.flush()?;
    Ok(())
}

fn write_xlsx(
    rows: &[&Markup],
    cols: &[ListColumn],
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use rust_xlsxwriter::{Format, Workbook};

    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let header_fmt = Format::new().set_bold();
    for (ci, c) in cols.iter().enumerate() {
        ws.write_string_with_format(0, ci as u16, c.header(), &header_fmt)?;
    }
    for (ri, m) in rows.iter().enumerate() {
        let r = (ri + 1) as u32;
        for (ci, c) in cols.iter().enumerate() {
            let text = cell(m, ri + 1, c);
            if c.is_numeric() {
                ws.write_number(r, ci as u16, text.parse::<f64>().unwrap_or(0.0))?;
            } else {
                // `write_string` stores a plain string cell, never a formula, so XLSX needs no
                // leading `'` (it would show). Only CSV is neutralised.
                ws.write_string(r, ci as u16, text)?;
            }
        }
    }
    wb.save(path)?;
    Ok(())
}

/// Export the Markups List view (filtered + sorted `ids`, chosen `columns`) to XLSX or CSV.
/// `path` is the absolute output path chosen by the user. Returns the rows written.
#[tauri::command]
pub async fn export_markups_view(
    state: State<'_, AppState>,
    doc_id: String,
    path: String,
    format: ExportFormat,
    ids: Vec<String>,
    columns: Vec<String>,
) -> Result<usize, String> {
    let all = state.markups.list(&doc_id)?;
    crate::commands::takeoff::ensure_scales_loaded(&state, &doc_id);
    let scales = state.scales.lock().unwrap().list(&doc_id).to_vec();
    export_view_with_scales(&all, &scales, &ids, &columns, &PathBuf::from(&path), format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PdfPoint;
    use crate::markup::{Appearance, MarkupGeometry, MarkupStatus, MarkupType, UserRef};
    use tempfile::tempdir;
    use uuid::Uuid;

    fn mk(page: u32, contents: &str, author: &str) -> Markup {
        let mut m = Markup::new(
            MarkupType::Rectangle,
            page,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 0.0, y: 0.0 },
                max: PdfPoint { x: 10.0, y: 10.0 },
            },
            Appearance::default(),
            UserRef {
                user_id: Uuid::new_v4(),
                display_name: author.into(),
            },
        );
        m.contents = Some(contents.into());
        m
    }

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn csv_follows_view_order_and_column_pick() {
        let a = mk(0, "first", "Ann");
        let mut b = mk(2, "second", "Bob");
        b.workflow.status = MarkupStatus::Rejected;
        let ids = vec![b.id().to_string(), a.id().to_string()];
        let dir = tempdir().unwrap();
        let out = dir.path().join("v.csv");
        let n = export_view_to(
            &[a, b],
            &ids,
            &keys(&["page", "comment", "status"]),
            &out,
            ExportFormat::Csv,
        )
        .unwrap();
        assert_eq!(n, 2);
        let text = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Page,Comments,Status");
        assert_eq!(lines[1], "3,second,Rejected");
        assert_eq!(lines[2], "1,first,None");
        assert_eq!(lines.len(), 3, "only the requested ids are exported");
    }

    #[test]
    fn unknown_missing_and_empty_inputs() {
        let a = mk(0, "x", "A");
        let dir = tempdir().unwrap();
        let out = dir.path().join("v.csv");
        let err = export_view_to(
            std::slice::from_ref(&a),
            &[],
            &keys(&["bogus"]),
            &out,
            ExportFormat::Csv,
        )
        .unwrap_err();
        assert!(err.contains("bogus"), "{err}");
        assert!(
            export_view_to(std::slice::from_ref(&a), &[], &[], &out, ExportFormat::Csv).is_err()
        );
        let too_many: Vec<String> = (0..=MAX_EXPORT_COLUMNS)
            .map(|_| "page".to_string())
            .collect();
        assert!(export_view_to(
            std::slice::from_ref(&a),
            &[],
            &too_many,
            &out,
            ExportFormat::Csv
        )
        .is_err());
        // An id that no longer exists is skipped, not an error.
        let n = export_view_to(
            &[a],
            &["not-an-id".into()],
            &keys(&["page"]),
            &out,
            ExportFormat::Csv,
        )
        .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn hostile_text_cannot_become_a_formula() {
        let a = mk(0, "=HYPERLINK(\"http://x\",\"y\")", "+cmd|' /C calc'!A0");
        let ids = vec![a.id().to_string()];
        let dir = tempdir().unwrap();
        let out = dir.path().join("v.csv");
        export_view_to(
            &[a],
            &ids,
            &keys(&["comment", "author"]),
            &out,
            ExportFormat::Csv,
        )
        .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        let row = text.lines().nth(1).unwrap();
        assert!(row.starts_with("\"'=HYPERLINK"), "{row}");
        assert!(row.contains(",'+cmd") || row.contains(",\"'+cmd"), "{row}");
    }

    #[test]
    fn neutralise_formula_cases() {
        for bad in [
            "=HYPERLINK(\"http://x\",\"y\")",
            "+cmd|' /C calc'!A0",
            "-1+1",
            "@SUM(A1:A2)",
            "\t=1+1",
            "\r=1+1",
            "  =1+1",
            "\u{FF1D}1+1",
            "\u{FF0B}1",
            "\u{FF0D}1",
            "\u{FF20}SUM(1)",
        ] {
            assert_eq!(neutralise_formula(bad), format!("'{bad}"), "{bad:?}");
        }
        for ok in [
            "",
            "plain",
            "a=b",
            "1-2",
            "'=already",
            "#e02424",
            "Door -5 mm",
        ] {
            assert_eq!(neutralise_formula(ok), ok);
        }
    }

    #[test]
    fn csv_neutralises_every_text_origin_column_and_quotes_per_rfc4180() {
        use crate::markup::{Measurement, Reply};
        use std::collections::BTreeMap;
        let mut m = mk(0, "=1+1\n\"q\",x", "-1+1");
        m.subject = Some("@SUM(A1)".into());
        m.layer = Some("\t=cmd".into());
        let mut cc = BTreeMap::new();
        cc.insert("k".to_string(), "+9".to_string());
        m.measurement = Some(Measurement {
            scale_ref: None,
            raw_measure: 1.0,
            unit: "m".into(),
            computed_quantity: -2.5,
            depth: None,
            count_value: None,
            custom_columns: cc,
            rise_drop: Vec::new(),
        });
        m.workflow.thread.push(Reply {
            id: Uuid::new_v4(),
            author: UserRef {
                user_id: Uuid::new_v4(),
                display_name: "=r".into(),
            },
            at: chrono::Utc::now(),
            contents: "=reply".into(),
            in_reply_to: None,
            nm: None,
            extras: None,
        });
        let ids = vec![m.id().to_string()];
        let dir = tempdir().unwrap();
        let out = dir.path().join("v.csv");
        export_view_to(
            &[m],
            &ids,
            &keys(&[
                "comment",
                "author",
                "subject",
                "layer",
                "custom:k",
                "measurement",
                "page",
                "replies",
            ]),
            &out,
            ExportFormat::Csv,
        )
        .unwrap();
        let mut rdr = csv::Reader::from_path(&out).unwrap();
        let rec = rdr.records().next().unwrap().unwrap();
        // Round-tripped by a real CSV reader: embedded newline/quotes/comma survive quoting.
        assert_eq!(&rec[0], "'=1+1\n\"q\",x");
        assert_eq!(&rec[1], "'-1+1");
        assert_eq!(&rec[2], "'@SUM(A1)");
        assert_eq!(&rec[3], "'\t=cmd");
        assert_eq!(&rec[4], "'+9");
        // A negative measurement and the page/reply counts stay plain numbers.
        assert_eq!(&rec[5], "-2.50 m");
        assert_eq!(&rec[6], "1");
        assert_eq!(&rec[7], "1");
    }

    fn custom_measurement(name: &str, value: &str) -> crate::markup::Measurement {
        let mut cc = std::collections::BTreeMap::new();
        cc.insert(name.to_string(), value.to_string());
        crate::markup::Measurement {
            scale_ref: None,
            raw_measure: 1.0,
            unit: "m".into(),
            computed_quantity: 1.0,
            depth: None,
            count_value: None,
            custom_columns: cc,
            rise_drop: Vec::new(),
        }
    }

    #[test]
    fn hostile_custom_column_name_is_neutralised_in_the_csv_header() {
        let mut m = mk(0, "c", "A");
        m.measurement = Some(custom_measurement("=HYPERLINK(1)", "v"));
        let ids = vec![m.id().to_string()];
        let dir = tempdir().unwrap();
        let out = dir.path().join("v.csv");
        export_view_to(
            &[m],
            &ids,
            &keys(&["page", "custom:=HYPERLINK(1)"]),
            &out,
            ExportFormat::Csv,
        )
        .unwrap();
        let mut rdr = csv::Reader::from_path(&out).unwrap();
        let hdr = rdr.headers().unwrap().clone();
        assert_eq!(&hdr[1], "'=HYPERLINK(1)");
        assert_eq!(&rdr.records().next().unwrap().unwrap()[1], "v");
    }

    #[test]
    fn custom_column_names_are_length_capped_and_control_chars_refused() {
        let ok = format!("custom:{}", "a".repeat(MAX_CUSTOM_NAME_CHARS));
        assert!(ListColumn::parse(&ok).is_ok());
        let long = format!("custom:{}", "a".repeat(MAX_CUSTOM_NAME_CHARS + 1));
        assert!(ListColumn::parse(&long).is_err());
        assert!(ListColumn::parse("custom:a\nb").is_err());
        assert!(ListColumn::parse("custom:a\u{0}b").is_err());
        assert!(ListColumn::parse("custom:").is_err());
    }

    /// Read a named entry of an XLSX (zip) file as text.
    fn xlsx_entry(path: &std::path::Path, name: &str) -> String {
        use std::io::Read;
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut out = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn xlsx_hostile_text_and_header_are_plain_strings_without_a_visible_quote() {
        let mut m = mk(0, "=1+1", "@SUM(A1)");
        m.measurement = Some(custom_measurement("=HYPERLINK(1)", "-1+1"));
        let ids = vec![m.id().to_string()];
        let dir = tempdir().unwrap();
        let out = dir.path().join("v.xlsx");
        export_view_to(
            &[m],
            &ids,
            &keys(&["comment", "author", "custom:=HYPERLINK(1)"]),
            &out,
            ExportFormat::Xlsx,
        )
        .unwrap();
        let strings = xlsx_entry(&out, "xl/sharedStrings.xml");
        for v in ["=1+1", "@SUM(A1)", "=HYPERLINK(1)", "-1+1"] {
            assert!(
                strings.contains(&format!(">{v}<")),
                "{v} missing as plain string: {strings}"
            );
            assert!(
                !strings.contains(&format!(">'{v}<")),
                "{v} must not carry a visible quote"
            );
        }
        let sheet = xlsx_entry(&out, "xl/worksheets/sheet1.xml");
        assert!(
            !sheet.contains("<f>") && !sheet.contains("<f "),
            "no formula cells: {sheet}"
        );
        assert!(sheet.contains("t=\"s\""), "cells are shared-string typed");
    }

    #[test]
    fn replies_status_by_and_custom_columns() {
        use crate::markup::{Measurement, Reply, StatusChange};
        use std::collections::BTreeMap;
        let mut m = mk(0, "c", "Ann");
        let who = UserRef {
            user_id: Uuid::new_v4(),
            display_name: "Reviewer".into(),
        };
        m.workflow.thread.push(Reply {
            id: Uuid::new_v4(),
            author: who.clone(),
            at: chrono::Utc::now(),
            contents: "r".into(),
            in_reply_to: None,
            nm: None,
            extras: None,
        });
        m.workflow.status = MarkupStatus::Accepted;
        m.workflow.status_history.push(StatusChange {
            id: Uuid::new_v4(),
            author: who,
            at: chrono::Utc::now(),
            model: "Review".into(),
            state: "Accepted".into(),
            on_reply: None,
            nm: None,
            extras: None,
        });
        let mut cc = BTreeMap::new();
        cc.insert("cost_code".to_string(), "E-100".to_string());
        m.measurement = Some(Measurement {
            scale_ref: None,
            raw_measure: 1.0,
            unit: "m".into(),
            computed_quantity: 2.5,
            depth: None,
            count_value: None,
            custom_columns: cc,
            rise_drop: Vec::new(),
        });
        let ids = vec![m.id().to_string()];
        let dir = tempdir().unwrap();
        let out = dir.path().join("v.csv");
        export_view_to(
            &[m],
            &ids,
            &keys(&["replies", "status_by", "measurement", "custom:cost_code"]),
            &out,
            ExportFormat::Csv,
        )
        .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(text.lines().nth(1).unwrap(), "1,Reviewer,2.50 m,E-100");
    }

    #[test]
    fn xlsx_view_is_written() {
        let a = mk(0, "x", "A");
        let ids = vec![a.id().to_string()];
        let dir = tempdir().unwrap();
        let out = dir.path().join("v.xlsx");
        let n = export_view_to(
            &[a],
            &ids,
            &keys(&["page", "comment"]),
            &out,
            ExportFormat::Xlsx,
        )
        .unwrap();
        assert_eq!(n, 1);
        assert!(out.metadata().unwrap().len() > 500);
    }

    #[test]
    fn view_export_recomputes_like_the_quantities_export() {
        use crate::markup::Measurement;
        use crate::takeoff::{ScaleMethod, ScaleRecord, ScaleTarget};
        let mut m = crate::markup::Markup::new(
            crate::markup::MarkupType::MeasurementLength,
            0,
            crate::markup::MarkupGeometry::Polyline(vec![
                crate::geometry::PdfPoint { x: 0.0, y: 0.0 },
                crate::geometry::PdfPoint { x: 1000.0, y: 0.0 },
            ]),
            crate::markup::Appearance::default(),
            crate::markup::UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "T".into(),
            },
        );
        m.measurement = Some(Measurement {
            scale_ref: Some("old".into()),
            raw_measure: 1000.0,
            unit: "m".into(),
            computed_quantity: 1.0, // stale: measured under the replaced scale
            depth: None,
            count_value: None,
            custom_columns: Default::default(),
            rise_drop: Vec::new(),
        });
        let scale = ScaleRecord::new(
            ScaleTarget::DocumentDefault,
            ScaleMethod::TwoPoint,
            0.005,
            "m".into(),
            "new".into(),
            2,
        );
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("v.csv");
        let id = m.id().to_string();
        export_view_with_scales(
            &[m],
            &[scale],
            &[id],
            &["measurement".to_string()],
            &out,
            ExportFormat::Csv,
        )
        .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("5.00 m"), "{text}");
    }
}
