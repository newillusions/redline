//! Tauri commands for scale calibration + markup list export (spec §7, M3).

use std::path::PathBuf;

use tauri::State;

use crate::sidecar::{load_meta, save_meta};
use crate::takeoff::{ScaleMethod, ScaleRecord, ScaleTarget};
use crate::AppState;

// ---------------------------------------------------------------------------
// Scale commands
// ---------------------------------------------------------------------------

/// Add (or replace by applies_to target) a scale record for the document. Persists to sidecar.
#[tauri::command]
pub async fn add_scale(
    state: State<'_, AppState>,
    doc_id: String,
    applies_to_page: Option<u32>,
    ratio: f64,
    unit: String,
    label: String,
    precision: u8,
) -> Result<ScaleRecord, String> {
    let applies_to = match applies_to_page {
        Some(p) => ScaleTarget::Page { page: p },
        None => ScaleTarget::DocumentDefault,
    };
    let rec = ScaleRecord::new(
        applies_to.clone(),
        ScaleMethod::TwoPoint,
        ratio,
        unit,
        label,
        precision,
    );

    // Persist to sidecar
    let pdf_path = state.markups.path(&doc_id).ok_or("unknown doc")?;
    let mut meta = load_meta(&pdf_path).map_err(|e| e.to_string())?;
    // Replace if same applies_to target already exists
    meta.scales.retain(|r| r.applies_to != applies_to);
    meta.scales.push(rec.clone());
    save_meta(&pdf_path, &meta).map_err(|e| e.to_string())?;

    // Update in-memory store
    state.scales.lock().unwrap().add(&doc_id, rec.clone());

    Ok(rec)
}

/// List all scale records for the document.
#[tauri::command]
pub async fn list_scales(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<Vec<ScaleRecord>, String> {
    Ok(state.scales.lock().unwrap().list(&doc_id).to_vec())
}

/// Delete a scale by id. Returns true if found.
#[tauri::command]
pub async fn delete_scale(
    state: State<'_, AppState>,
    doc_id: String,
    scale_id: String,
) -> Result<bool, String> {
    let removed = state.scales.lock().unwrap().delete(&doc_id, &scale_id);
    if removed {
        if let Some(pdf_path) = state.markups.path(&doc_id) {
            let mut meta = load_meta(&pdf_path).map_err(|e| e.to_string())?;
            meta.scales.retain(|r| r.id != scale_id);
            save_meta(&pdf_path, &meta).map_err(|e| e.to_string())?;
        }
    }
    Ok(removed)
}

/// List the scales applicable to a page for the preset picker: a page-specific
/// scale (if any) plus the document default. Lets the user pick a saved scale
/// without re-drawing a calibration line (M4 S1, M3-deferred). Page-specific first.
#[tauri::command]
pub async fn list_applicable_scales(
    state: State<'_, AppState>,
    doc_id: String,
    page_idx: u32,
) -> Result<Vec<ScaleRecord>, String> {
    let store = state.scales.lock().unwrap();
    let all = store.list(&doc_id);
    Ok(crate::takeoff::applicable_scales(all, page_idx)
        .into_iter()
        .cloned()
        .collect())
}

/// Embed a standard PDF /Measure viewport dictionary (spec §12.7) for a page,
/// using a saved scale, so Acrobat/Bluebeam can read the calibration. Writes the
/// PDF on disk (atomic temp+rename) and reloads the render engine (M4 S1).
#[tauri::command]
pub async fn write_page_measure(
    state: State<'_, AppState>,
    doc_id: String,
    page_idx: u32,
    scale_id: String,
) -> Result<(), String> {
    let scale = {
        let store = state.scales.lock().unwrap();
        crate::takeoff::find_scale(store.list(&doc_id), &scale_id)
            .ok_or_else(|| format!("scale {scale_id} not found for doc {doc_id}"))?
            .clone()
    };
    crate::commands::document::apply_page_edit(&state, &doc_id, move |doc| {
        crate::takeoff::write_measure_dict(doc, page_idx, &scale)
    })
    .await
}

// ---------------------------------------------------------------------------
// Export command
// ---------------------------------------------------------------------------

/// Export format selector.
#[derive(Debug, serde::Deserialize)]
pub enum ExportFormat {
    Xlsx,
    Csv,
}

/// Export the full markup list (all markups for the document) to XLSX or CSV.
/// `path` is the absolute output file path chosen by the user (via dialog in JS).
#[tauri::command]
pub async fn export_markup_list(
    state: State<'_, AppState>,
    doc_id: String,
    path: String,
    format: ExportFormat,
) -> Result<(), String> {
    let markups = state.markups.list(&doc_id)?;
    let out = PathBuf::from(&path);
    match format {
        ExportFormat::Xlsx => export_xlsx(&markups, &out).map_err(|e| e.to_string()),
        ExportFormat::Csv => export_csv(&markups, &out).map_err(|e| e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// XLSX writer
// ---------------------------------------------------------------------------

fn export_xlsx(
    markups: &[crate::markup::Markup],
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use rust_xlsxwriter::{Format, Workbook};

    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();

    let header_fmt = Format::new().set_bold();
    let headers = [
        "#", "Type", "Page", "Subject", "Author", "Date", "Contents", "Layer", "Quantity", "Unit",
        "Status",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_string_with_format(0, col as u16, *h, &header_fmt)?;
    }

    for (row_idx, m) in markups.iter().enumerate() {
        let row = (row_idx + 1) as u32;
        let (qty, unit) = m
            .measurement
            .as_ref()
            .map(|meas| (format!("{:.2}", meas.computed_quantity), meas.unit.clone()))
            .unwrap_or_default();

        ws.write_number(row, 0, (row_idx + 1) as f64)?;
        ws.write_string(row, 1, format!("{:?}", m.markup_type))?;
        ws.write_number(row, 2, (m.page + 1) as f64)?; // 1-based for users
        ws.write_string(row, 3, m.subject.as_deref().unwrap_or(""))?;
        ws.write_string(row, 4, &m.audit.created_by.display_name)?;
        ws.write_string(
            row,
            5,
            m.audit.created_at.format("%Y-%m-%d %H:%M").to_string(),
        )?;
        ws.write_string(row, 6, m.contents.as_deref().unwrap_or(""))?;
        ws.write_string(row, 7, m.layer.as_deref().unwrap_or(""))?;
        ws.write_string(row, 8, &qty)?;
        ws.write_string(row, 9, &unit)?;
        ws.write_string(row, 10, format!("{:?}", m.workflow.status))?;
    }

    wb.save(path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// CSV writer
// ---------------------------------------------------------------------------

fn export_csv(
    markups: &[crate::markup::Markup],
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut wtr = csv::Writer::from_path(path)?;
    wtr.write_record([
        "#", "Type", "Page", "Subject", "Author", "Date", "Contents", "Layer", "Quantity", "Unit",
        "Status",
    ])?;
    for (i, m) in markups.iter().enumerate() {
        let (qty, unit) = m
            .measurement
            .as_ref()
            .map(|meas| (format!("{:.2}", meas.computed_quantity), meas.unit.clone()))
            .unwrap_or_default();
        wtr.write_record([
            &(i + 1).to_string(),
            &format!("{:?}", m.markup_type),
            &(m.page + 1).to_string(),
            m.subject.as_deref().unwrap_or(""),
            &m.audit.created_by.display_name,
            &m.audit.created_at.format("%Y-%m-%d %H:%M").to_string(),
            m.contents.as_deref().unwrap_or(""),
            m.layer.as_deref().unwrap_or(""),
            &qty,
            &unit,
            &format!("{:?}", m.workflow.status),
        ])?;
    }
    wtr.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_markup(typ: crate::markup::MarkupType) -> crate::markup::Markup {
        use crate::geometry::PdfPoint;
        use crate::markup::{Appearance, MarkupGeometry, UserRef};
        use uuid::Uuid;
        crate::markup::Markup::new(
            typ,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 0.0, y: 0.0 },
                max: PdfPoint { x: 10.0, y: 10.0 },
            },
            Appearance::default(),
            UserRef {
                user_id: Uuid::new_v4(),
                display_name: "Tester".into(),
            },
        )
    }

    #[test]
    fn csv_export_produces_file() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("list.csv");
        let markups = vec![sample_markup(crate::markup::MarkupType::Rectangle)];
        export_csv(&markups, &out).unwrap();
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(
            content.contains("Rectangle"),
            "expected markup type in CSV: {content}"
        );
        assert!(content.contains("Tester"), "expected author in CSV");
    }

    #[test]
    fn xlsx_export_produces_file() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("list.xlsx");
        let markups = vec![sample_markup(crate::markup::MarkupType::MeasurementLength)];
        export_xlsx(&markups, &out).unwrap();
        assert!(out.exists(), "xlsx file should be created");
        assert!(
            out.metadata().unwrap().len() > 1000,
            "xlsx should be non-trivial size"
        );
    }

    #[test]
    fn csv_measurement_row_includes_quantity() {
        use crate::markup::Measurement;
        use std::collections::BTreeMap;
        let dir = tempdir().unwrap();
        let out = dir.path().join("meas.csv");
        let mut m = sample_markup(crate::markup::MarkupType::MeasurementLength);
        m.measurement = Some(Measurement {
            scale_ref: Some("sc1".into()),
            raw_measure: 5000.0,
            unit: "m".into(),
            computed_quantity: 5.0,
            depth: None,
            count_value: None,
            custom_columns: BTreeMap::new(),
        });
        export_csv(&[m], &out).unwrap();
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(
            content.contains("5.00"),
            "expected quantity 5.00 in CSV: {content}"
        );
        assert!(content.contains(",m,"), "expected unit m in CSV");
    }
}
