//! Tauri commands for scale calibration + markup list export (spec §7, M3).

use std::path::PathBuf;

use tauri::State;

use crate::commands::markup_list::neutralise_formula;
use crate::markup::Markup;
use crate::sidecar::{load_meta, save_meta};
use crate::takeoff::{ScaleMethod, ScaleRecord, ScaleTarget};
use crate::AppState;

// ---------------------------------------------------------------------------
// Scale commands
// ---------------------------------------------------------------------------

/// Make sure the document's scale records are in `state.scales`. They were only ever
/// filled by `add_scale`, so a reopened document had the sidecar's scales on disk but none
/// in memory (empty `list_scales`, exports recomputed under nothing). Loads the sidecar
/// once per open; `close_document` clears the entry so a reopen loads it again.
pub fn ensure_scales_loaded(state: &AppState, doc_id: &str) {
    let path = state.markups.path(doc_id);
    state
        .scales
        .lock()
        .unwrap()
        .ensure_loaded(doc_id, path.as_deref());
}

/// Recompute the document's stored measurements under its current scales (after they changed).
pub fn recompute_stored_measurements(
    markups: &crate::document::store::MarkupStore,
    scales: &crate::takeoff::ScaleStore,
    doc_id: &str,
) -> usize {
    markups.recompute_measurements(doc_id, scales.list(doc_id))
}

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
        applies_to,
        ScaleMethod::TwoPoint,
        ratio,
        unit,
        label,
        precision,
    );
    let rec = store_scale(&state, &doc_id, rec)?;
    // Bring the stored measurements under the new scale, as the GUI does on a calibration, so
    // an MCP `add_scale` cannot leave stale quantities for a later save or export.
    recompute_stored_measurements(&state.markups, &state.scales.lock().unwrap(), &doc_id);

    Ok(rec)
}

/// Persist `rec` (sidecar first, then the in-memory store), replacing the record in the same
/// slot ([`ScaleTarget::same_slot`]) in place. A recalibration of an existing slot keeps
/// that slot's scale id, so the measurements taken under it follow the new calibration
/// (`recompute_all`) instead of falling back to whatever scale lies under them - a
/// measurement inside a viewport that was taken under the page scale stays under it.
fn store_scale(
    state: &AppState,
    doc_id: &str,
    mut rec: ScaleRecord,
) -> Result<ScaleRecord, String> {
    ensure_scales_loaded(state, doc_id);
    if let Some(old) = state
        .scales
        .lock()
        .unwrap()
        .list(doc_id)
        .iter()
        .find(|r| r.applies_to.same_slot(&rec.applies_to))
    {
        rec.id = old.id.clone();
    }
    let pdf_path = state.markups.path(doc_id).ok_or("unknown doc")?;
    let mut meta = load_meta(&pdf_path).map_err(|e| e.to_string())?;
    crate::takeoff::scale::replace_or_push(&mut meta.scales, rec.clone());
    save_meta(&pdf_path, &meta).map_err(|e| e.to_string())?;
    state.scales.lock().unwrap().add(doc_id, rec.clone());
    Ok(rec)
}

/// Create a measurement viewport: a named rectangle on one page with its own scale
/// (`bbox` in page space, the space markup geometry uses). Measurements whose first point
/// falls inside it use its scale from now on; measurements already on the page keep the
/// scale they were taken under (re-apply is an explicit action in the UI). Persists to the
/// sidecar; written to the PDF as a `/VP` viewport on the next save.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn add_viewport(
    state: State<'_, AppState>,
    doc_id: String,
    page: u32,
    name: String,
    bbox: crate::takeoff::BBox,
    ratio: f64,
    unit: String,
    label: String,
    precision: u8,
    two_point: bool,
) -> Result<ScaleRecord, String> {
    let rec = viewport_record(
        page,
        uuid::Uuid::new_v4().to_string(),
        &name,
        bbox,
        ratio,
        unit,
        label,
        precision,
        two_point,
    )?;
    store_scale(&state, &doc_id, rec)
}

/// Edit a viewport: rename it, move or resize its rectangle (to another page too, with
/// `page`), and/or recalibrate it. Its
/// scale id is kept, so measurements taken under it follow a recalibration; a move or
/// resize changes no stored measurement (one now outside keeps the viewport's scale until
/// the user re-applies scales).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn update_viewport(
    state: State<'_, AppState>,
    doc_id: String,
    scale_id: String,
    page: Option<u32>,
    name: Option<String>,
    bbox: Option<crate::takeoff::BBox>,
    ratio: Option<f64>,
    unit: Option<String>,
    label: Option<String>,
    precision: Option<u8>,
    two_point: Option<bool>,
) -> Result<ScaleRecord, String> {
    ensure_scales_loaded(&state, &doc_id);
    let old = state
        .scales
        .lock()
        .unwrap()
        .list(&doc_id)
        .iter()
        .find(|r| r.id == scale_id)
        .cloned()
        .ok_or_else(|| format!("scale {scale_id} not found for doc {doc_id}"))?;
    let ScaleTarget::Viewport {
        page: old_page,
        viewport_id,
        name: old_name,
        bbox: old_bbox,
    } = &old.applies_to
    else {
        return Err(format!("scale {scale_id} is not a viewport"));
    };
    let mut rec = viewport_record(
        page.unwrap_or(*old_page),
        viewport_id.clone(),
        name.as_deref().unwrap_or(old_name),
        bbox.unwrap_or(*old_bbox),
        ratio.unwrap_or(old.ratio),
        unit.unwrap_or_else(|| old.unit.clone()),
        label.unwrap_or_else(|| old.label.clone()),
        precision.unwrap_or(old.precision),
        two_point.unwrap_or(old.method == ScaleMethod::TwoPoint),
    )?;
    rec.id = old.id.clone();
    let rec = store_scale(&state, &doc_id, rec)?;
    if ratio.is_some() {
        recompute_stored_measurements(&state.markups, &state.scales.lock().unwrap(), &doc_id);
    }
    Ok(rec)
}

/// Validate and build a viewport scale record.
#[allow(clippy::too_many_arguments)]
fn viewport_record(
    page: u32,
    viewport_id: String,
    name: &str,
    bbox: crate::takeoff::BBox,
    ratio: f64,
    unit: String,
    label: String,
    precision: u8,
    two_point: bool,
) -> Result<ScaleRecord, String> {
    let bbox = crate::takeoff::scale::normalise_bbox(bbox);
    if !bbox.iter().all(|v| v.is_finite()) || bbox[2] - bbox[0] < 1.0 || bbox[3] - bbox[1] < 1.0 {
        return Err("a viewport needs a rectangle at least 1 pt wide and high".into());
    }
    if !ratio.is_finite() || ratio <= 0.0 {
        return Err("a viewport scale needs a positive ratio".into());
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("a viewport needs a name".into());
    }
    Ok(ScaleRecord::new(
        ScaleTarget::Viewport {
            page,
            viewport_id,
            name: name.to_string(),
            bbox,
        },
        if two_point {
            ScaleMethod::TwoPoint
        } else {
            ScaleMethod::Preset
        },
        ratio,
        unit,
        label,
        precision,
    ))
}

/// Import the rectilinear `/VP` viewports read from the document's PDF (another
/// application's calibrations - Revu, Acrobat - or redline's own after a lost sidecar) as
/// scales; see [`crate::takeoff::plan_viewport_import`]. Persists what it adds to the
/// sidecar (only when something is new). Returns how many scales were added.
pub fn import_pdf_viewports(
    state: &AppState,
    doc_id: &str,
    found: &[crate::takeoff::PdfViewport],
) -> Result<usize, String> {
    if found.is_empty() {
        return Ok(0);
    }
    ensure_scales_loaded(state, doc_id);
    let (records, keys) = {
        let store = state.scales.lock().unwrap();
        crate::takeoff::plan_viewport_import(store.list(doc_id), store.seen(doc_id), found)
    };
    if records.is_empty() && keys.is_empty() {
        return Ok(0);
    }
    // In memory first: even if the sidecar cannot be written (a read-only folder), the PDF's
    // own viewport scales apply for this session - measuring must not silently fall back
    // to the page scale. The failure is reported to the user, never swallowed.
    {
        let mut store = state.scales.lock().unwrap();
        for r in &records {
            store.add(doc_id, r.clone());
        }
        store.add_seen(doc_id, &keys);
    }
    let persisted = (|| -> Result<(), String> {
        let pdf_path = state.markups.path(doc_id).ok_or("unknown doc")?;
        let mut meta = load_meta(&pdf_path).map_err(|e| e.to_string())?;
        for r in &records {
            crate::takeoff::scale::replace_or_push(&mut meta.scales, r.clone());
        }
        meta.pdf_viewports_seen.extend(keys.iter().cloned());
        save_meta(&pdf_path, &meta).map_err(|e| e.to_string())
    })();
    if let Err(e) = persisted {
        let msg = format!(
            "The PDF's {} measurement viewport scale(s) are in use, but could not be saved next to the file ({e}). They will be read from the PDF again next time.",
            records.len()
        );
        state
            .scales
            .lock()
            .unwrap()
            .push_notice(doc_id, msg.clone());
        return Err(msg);
    }
    Ok(records.len())
}

/// Take the document's queued scale notices (e.g. an import that could not be saved), for
/// the frontend to show. Clears them.
#[tauri::command]
pub async fn take_scale_notices(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<Vec<String>, String> {
    Ok(state.scales.lock().unwrap().take_notices(&doc_id))
}

/// Put a scale record exactly as given (insert, or replace the one with its id in place), or
/// with `record: None` remove the scale `scale_id` outright. Persists to the sidecar; never
/// recomputes measurements - the caller sends those changes in the same undo step. This is
/// what the frontend's undo/redo of viewport changes (create, edit, delete) replays.
#[tauri::command]
pub async fn put_scale(
    state: State<'_, AppState>,
    doc_id: String,
    scale_id: String,
    record: Option<ScaleRecord>,
) -> Result<(), String> {
    ensure_scales_loaded(&state, &doc_id);
    if let Some(r) = &record {
        if r.id != scale_id {
            return Err(format!("scale id mismatch: {} vs {scale_id}", r.id));
        }
        if !r.ratio.is_finite() || r.ratio <= 0.0 {
            return Err("a scale needs a positive ratio".into());
        }
    }
    let pdf_path = state.markups.path(&doc_id).ok_or("unknown doc")?;
    let mut meta = load_meta(&pdf_path).map_err(|e| e.to_string())?;
    match &record {
        Some(r) => crate::takeoff::scale::replace_or_push(&mut meta.scales, r.clone()),
        None => meta.scales.retain(|r| r.id != scale_id),
    }
    save_meta(&pdf_path, &meta).map_err(|e| e.to_string())?;
    let mut store = state.scales.lock().unwrap();
    match record {
        Some(r) => store.add(&doc_id, r),
        None => {
            store.delete(&doc_id, &scale_id);
        }
    }
    Ok(())
}

/// After a page operation rewrote the PDF (with the viewports synced to `/VP` just before
/// it), point every viewport scale at where its `/VP` entry now is: a reorder moves it to
/// another page index, a rotation changes its page-space rectangle, and a deleted page takes
/// its viewports with it. Page and document-default scales are not touched.
pub fn follow_viewports_after_page_op(
    state: &AppState,
    doc_id: &str,
    found: &[crate::takeoff::PdfViewport],
    skipped_pages: &[u32],
) -> Result<(), String> {
    ensure_scales_loaded(state, doc_id);
    let (before, seen_before) = {
        let store = state.scales.lock().unwrap();
        (store.list(doc_id).to_vec(), store.seen(doc_id).to_vec())
    };
    // A page whose /VP could not be written has nothing to read back: keep its viewports
    // (positions are those from before the operation) and say so.
    let unsynced = crate::takeoff::unsynced_viewport_ids(&before, skipped_pages);
    if !unsynced.is_empty() {
        state.scales.lock().unwrap().push_notice(
            doc_id,
            format!(
                "{} measurement viewport(s) could not be moved with their pages because the page geometry could not be read; check their position.",
                unsynced.len()
            ),
        );
    }
    let (after, seen) =
        crate::takeoff::measure::follow_viewports(&before, &seen_before, found, &unsynced);
    if after == before && seen == seen_before {
        return Ok(());
    }
    {
        let mut store = state.scales.lock().unwrap();
        store.seed(doc_id, after.clone());
        store.set_seen(doc_id, seen.clone());
    }
    if let Some(pdf_path) = state.markups.path(doc_id) {
        let mut meta = load_meta(&pdf_path).map_err(|e| e.to_string())?;
        meta.scales = after;
        meta.pdf_viewports_seen = seen;
        save_meta(&pdf_path, &meta).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// List all scale records for the document, including retired viewports (deleted, kept so the
/// measurements taken under them keep their values): those carry `retired: true`, and the
/// GUI needs them. MCP/RPC callers get the live ones only (`rpc::dispatch`).
#[tauri::command]
pub async fn list_scales(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<Vec<ScaleRecord>, String> {
    ensure_scales_loaded(&state, &doc_id);
    Ok(state.scales.lock().unwrap().list(&doc_id).to_vec())
}

/// Delete a scale by id. Returns true if found.
#[tauri::command]
pub async fn delete_scale(
    state: State<'_, AppState>,
    doc_id: String,
    scale_id: String,
) -> Result<bool, String> {
    ensure_scales_loaded(&state, &doc_id);
    // A viewport is retired, not removed: the measurements taken under it keep their values
    // (never silently recalculated) and the delete can be undone.
    let viewport = state
        .scales
        .lock()
        .unwrap()
        .list(&doc_id)
        .iter()
        .find(|r| r.id == scale_id && matches!(r.applies_to, ScaleTarget::Viewport { .. }))
        .cloned();
    if let Some(mut v) = viewport {
        if v.retired {
            return Ok(false);
        }
        v.retired = true;
        store_scale(&state, &doc_id, v)?;
        return Ok(true);
    }
    let removed = state.scales.lock().unwrap().delete(&doc_id, &scale_id);
    if removed {
        if let Some(pdf_path) = state.markups.path(&doc_id) {
            let mut meta = load_meta(&pdf_path).map_err(|e| e.to_string())?;
            meta.scales.retain(|r| r.id != scale_id);
            save_meta(&pdf_path, &meta).map_err(|e| e.to_string())?;
        }
        // The measurements taken under that scale now fall back to the scale that applies
        // (as the GUI recomputes): never leave stale quantities for a save, page op or export.
        recompute_stored_measurements(&state.markups, &state.scales.lock().unwrap(), &doc_id);
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
    ensure_scales_loaded(&state, &doc_id);
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
    ensure_scales_loaded(&state, &doc_id);
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
    ensure_scales_loaded(&state, &doc_id);
    let scales = state.scales.lock().unwrap().list(&doc_id).to_vec();
    let labels = crate::commands::navigation::labels_for(&state, &doc_id);
    export_markup_list_with_labels(&markups, &scales, &PathBuf::from(&path), format, &labels)
}

/// Export from a bare markup slice - no `State`/`AppState` dependency - so the MCP bridge's
/// `export_markup_schedule` tool (`rpc::tools`) shares this code path with the Tauri command
/// above. First recomputes every scaled quantity from its
/// `raw_measure` under the document's current scales (so an export is never stale after
/// a recalibration) and collapses each count group to one row carrying its total.
pub fn export_markup_list_with_scales(
    markups: &[Markup],
    scales: &[crate::takeoff::ScaleRecord],
    path: &std::path::Path,
    format: ExportFormat,
) -> Result<(), String> {
    export_markup_list_with_labels(markups, scales, path, format, &[])
}

/// [`export_markup_list_with_scales`] that also knows the document's page labels: when any
/// page has one, a trailing **Page label** column is added (the page's label, else its
/// number). Documents without labels export exactly as before.
pub fn export_markup_list_with_labels(
    markups: &[Markup],
    scales: &[crate::takeoff::ScaleRecord],
    path: &std::path::Path,
    format: ExportFormat,
    labels: &[String],
) -> Result<(), String> {
    let mut rows = markups.to_vec();
    crate::takeoff::recompute_all(&mut rows, scales);
    let rows = crate::takeoff::count::collapse_groups(&rows);
    match format {
        ExportFormat::Xlsx => export_xlsx_labelled(&rows, path, labels).map_err(|e| e.to_string()),
        ExportFormat::Csv => export_csv_labelled(&rows, path, labels).map_err(|e| e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// XLSX writer
// ---------------------------------------------------------------------------

#[cfg(test)]
fn export_xlsx(
    markups: &[crate::markup::Markup],
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    export_xlsx_labelled(markups, path, &[])
}

/// The page's label, else its 1-based number (the Page label export column).
fn page_label_cell(m: &crate::markup::Markup, labels: &[String]) -> String {
    labels
        .get(m.page as usize)
        .filter(|l| !l.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| (m.page + 1).to_string())
}

fn any_label(labels: &[String]) -> bool {
    labels.iter().any(|l| !l.trim().is_empty())
}

fn export_xlsx_labelled(
    markups: &[crate::markup::Markup],
    path: &std::path::Path,
    labels: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    use rust_xlsxwriter::{Format, Workbook};
    let with_labels = any_label(labels);

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
    if with_labels {
        ws.write_string_with_format(0, headers.len() as u16, "Page label", &header_fmt)?;
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
        if with_labels {
            ws.write_string(row, 11, page_label_cell(m, labels))?;
        }
    }

    wb.save(path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// CSV writer
// ---------------------------------------------------------------------------

#[cfg(test)]
fn export_csv(
    markups: &[crate::markup::Markup],
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    export_csv_labelled(markups, path, &[])
}

fn export_csv_labelled(
    markups: &[crate::markup::Markup],
    path: &std::path::Path,
    labels: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let with_labels = any_label(labels);
    let mut wtr = csv::Writer::from_path(path)?;
    let mut header: Vec<&str> = vec![
        "#", "Type", "Page", "Subject", "Author", "Date", "Contents", "Layer", "Quantity", "Unit",
        "Status",
    ];
    if with_labels {
        header.push("Page label");
    }
    wtr.write_record(header)?;
    for (i, m) in markups.iter().enumerate() {
        let (qty, unit) = m
            .measurement
            .as_ref()
            .map(|meas| (format!("{:.2}", meas.computed_quantity), meas.unit.clone()))
            .unwrap_or_default();
        let mut record: Vec<String> = vec![
            (i + 1).to_string(),
            format!("{:?}", m.markup_type),
            (m.page + 1).to_string(),
            // Text that came from the PDF is neutralised against spreadsheet formula
            // injection; app-formatted cells (page, date, quantity) stay as they are.
            neutralise_formula(m.subject.as_deref().unwrap_or("")),
            neutralise_formula(&m.audit.created_by.display_name),
            m.audit.created_at.format("%Y-%m-%d %H:%M").to_string(),
            neutralise_formula(m.contents.as_deref().unwrap_or("")),
            neutralise_formula(m.layer.as_deref().unwrap_or("")),
            qty,
            neutralise_formula(&unit),
            format!("{:?}", m.workflow.status),
        ];
        if with_labels {
            // A label is document text: neutralised like every other PDF-derived cell.
            record.push(neutralise_formula(&page_label_cell(m, labels)));
        }
        wtr.write_record(record)?;
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
    fn csv_export_neutralises_hostile_text_but_keeps_negative_quantity() {
        use crate::markup::Measurement;
        use std::collections::BTreeMap;
        let dir = tempdir().unwrap();
        let out = dir.path().join("hostile.csv");
        let mut m = sample_markup(crate::markup::MarkupType::MeasurementLength);
        m.contents = Some("=HYPERLINK(\"http://x\",\"y\")".into());
        m.subject = Some("+cmd".into());
        m.layer = Some("@SUM(A1)".into());
        m.measurement = Some(Measurement {
            scale_ref: None,
            raw_measure: 1.0,
            unit: "-1+1".into(),
            computed_quantity: -5.0,
            depth: None,
            count_value: None,
            custom_columns: BTreeMap::new(),
            rise_drop: Vec::new(),
        });
        export_csv(&[m], &out).unwrap();
        let mut rdr = csv::Reader::from_path(&out).unwrap();
        let rec = rdr.records().next().unwrap().unwrap();
        assert_eq!(&rec[3], "'+cmd");
        assert_eq!(&rec[6], "'=HYPERLINK(\"http://x\",\"y\")");
        assert_eq!(&rec[7], "'@SUM(A1)");
        assert_eq!(&rec[8], "-5.00");
        assert_eq!(&rec[9], "'-1+1");
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
            rise_drop: Vec::new(),
        });
        export_csv(&[m], &out).unwrap();
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(
            content.contains("5.00"),
            "expected quantity 5.00 in CSV: {content}"
        );
        assert!(content.contains(",m,"), "expected unit m in CSV");
    }

    #[test]
    fn csv_export_collapses_count_groups_and_recomputes_under_current_scale() {
        use crate::markup::{Measurement, RiseDrop};
        use crate::takeoff::{ScaleMethod, ScaleRecord, ScaleTarget};
        use std::collections::BTreeMap;
        let dir = tempdir().unwrap();
        let out = dir.path().join("q.csv");
        let g = uuid::Uuid::new_v4();
        let mut items = Vec::new();
        for _ in 0..3 {
            let mut c = sample_markup(crate::markup::MarkupType::MeasurementCount);
            c.count_group = Some(g);
            c.subject = Some("Type A".into());
            c.measurement = Some(Measurement {
                scale_ref: None,
                raw_measure: 1.0,
                unit: "ea".into(),
                computed_quantity: 1.0,
                depth: None,
                count_value: Some(1),
                custom_columns: BTreeMap::new(),
                rise_drop: Vec::new(),
            });
            items.push(c);
        }
        // Polylength measured under a since-replaced scale (id "old"): 1000 pt + 2 m rise.
        let mut l = sample_markup(crate::markup::MarkupType::MeasurementLength);
        l.measurement = Some(Measurement {
            scale_ref: Some("old".into()),
            raw_measure: 1000.0,
            unit: "m".into(),
            computed_quantity: 1.0,
            depth: None,
            count_value: None,
            custom_columns: BTreeMap::new(),
            rise_drop: vec![RiseDrop {
                segment: 0,
                amount: 2.0,
                unit: "m".into(),
            }],
        });
        items.push(l);
        let scale = ScaleRecord::new(
            ScaleTarget::DocumentDefault,
            ScaleMethod::TwoPoint,
            0.005,
            "m".into(),
            "recal".into(),
            2,
        );
        export_markup_list_with_scales(&items, &[scale], &out, ExportFormat::Csv).unwrap();
        let mut rdr = csv::Reader::from_path(&out).unwrap();
        let recs: Vec<_> = rdr.records().map(|r| r.unwrap()).collect();
        assert_eq!(
            recs.len(),
            2,
            "3 count items collapse to one row + the polylength"
        );
        assert_eq!(&recs[0][3], "Type A");
        assert_eq!(&recs[0][8], "3.00", "group total");
        assert_eq!(
            &recs[1][8], "7.00",
            "1000 x 0.005 + 2 m rise under the new scale"
        );
    }

    #[test]
    fn export_uses_the_new_scale_after_a_recalibration_of_the_same_target() {
        use crate::markup::{Measurement, RiseDrop};
        use crate::takeoff::{ScaleMethod, ScaleRecord, ScaleStore, ScaleTarget};
        use std::collections::BTreeMap;
        let mk = |ratio: f64| {
            ScaleRecord::new(
                ScaleTarget::DocumentDefault,
                ScaleMethod::TwoPoint,
                ratio,
                "m".into(),
                "s".into(),
                2,
            )
        };
        let mut store = ScaleStore::default();
        store.add("d", mk(0.001));
        store.add("d", mk(0.002)); // recalibrated: must replace, not shadow
        assert_eq!(store.list("d").len(), 1);
        let mut l = sample_markup(crate::markup::MarkupType::MeasurementLength);
        l.measurement = Some(Measurement {
            scale_ref: None,
            raw_measure: 1000.0,
            unit: "pt".into(),
            computed_quantity: 0.0,
            depth: None,
            count_value: None,
            custom_columns: BTreeMap::new(),
            rise_drop: vec![RiseDrop {
                segment: 0,
                amount: 1.0,
                unit: "m".into(),
            }],
        });
        let dir = tempdir().unwrap();
        let out = dir.path().join("q.csv");
        export_markup_list_with_scales(&[l], store.list("d"), &out, ExportFormat::Csv).unwrap();
        let mut rdr = csv::Reader::from_path(&out).unwrap();
        let rec = rdr.records().next().unwrap().unwrap();
        assert_eq!(
            &rec[8], "3.00",
            "1000 x 0.002 + 1 m rise under the NEW scale"
        );
    }

    #[test]
    fn a_document_with_page_labels_gets_a_trailing_page_label_column() {
        let mut a = sample_markup(crate::markup::MarkupType::Rectangle);
        a.page = 0;
        let mut b = sample_markup(crate::markup::MarkupType::Rectangle);
        b.page = 1;
        let mut c = sample_markup(crate::markup::MarkupType::Rectangle);
        c.page = 2;
        let labels = vec!["E-101".to_string(), String::new(), "=1+1".to_string()];
        let dir = tempdir().unwrap();

        let out = dir.path().join("labelled.csv");
        export_markup_list_with_labels(
            &[a.clone(), b.clone(), c.clone()],
            &[],
            &out,
            ExportFormat::Csv,
            &labels,
        )
        .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].ends_with(",Status,Page label"), "{}", lines[0]);
        assert!(lines[1].ends_with(",E-101"), "{}", lines[1]);
        assert!(
            lines[2].ends_with(",2"),
            "an unlabelled page shows its number: {}",
            lines[2]
        );
        assert!(
            lines[3].ends_with(",'=1+1"),
            "a label is document text and is neutralised: {}",
            lines[3]
        );
        // The numeric Page column is unchanged.
        assert!(lines[1].contains(",1,"));

        // Without labels the export is exactly the old shape.
        let plain = dir.path().join("plain.csv");
        export_markup_list_with_labels(&[a.clone()], &[], &plain, ExportFormat::Csv, &[]).unwrap();
        assert!(!std::fs::read_to_string(&plain)
            .unwrap()
            .contains("Page label"));
        let blank = dir.path().join("blank.csv");
        export_markup_list_with_labels(
            &[a.clone()],
            &[],
            &blank,
            ExportFormat::Csv,
            &["".to_string(), " ".to_string()],
        )
        .unwrap();
        assert!(!std::fs::read_to_string(&blank)
            .unwrap()
            .contains("Page label"));

        // XLSX carries the same column (header cell present in the file's shared strings).
        let x = dir.path().join("labelled.xlsx");
        export_markup_list_with_labels(&[a, b, c], &[], &x, ExportFormat::Xlsx, &labels).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&x).unwrap()).unwrap();
        let mut shared = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("xl/sharedStrings.xml").unwrap(),
            &mut shared,
        )
        .unwrap();
        assert!(shared.contains("Page label"), "header cell");
        assert!(shared.contains("E-101"), "a label cell");
    }
}
