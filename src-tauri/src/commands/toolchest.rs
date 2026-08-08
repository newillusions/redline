//! Tauri commands for the Tool Chest (spec "Tools & Tool Sets", "Stamps",
//! "Importing Bluebeam Tool Sets & stamps") - list/create/rename/delete Tool Sets,
//! add/delete/reorder Tools, Recent Tools, `.btx` import, and dynamic-stamp field
//! composition.

use chrono::Local;
use tauri::State;
use uuid::Uuid;

use crate::markup::Markup;
use crate::toolchest::btx::{self, ImportReport};
use crate::toolchest::stamp::{compose_dynamic_text, DynamicField, StampAsset, StampDef};
use crate::toolchest::{CounterScope, PlacementMode, Tool, ToolSet};
use crate::AppState;

fn parse_uuid(s: &str, what: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("bad {what} id: {e}"))
}

/// Rasterize a `StampAsset::BluebeamFormXObject` to a `PngBase64` preview for live
/// display (2026-08-08 Form XObject stamp rendering follow-up - see
/// `document::annots::build_isolated_form_xobject_pdf` / `render::RenderEngine::
/// rasterize_pdf_bytes`). Falls back to returning `None` (leaving the caller's asset
/// unchanged) rather than erroring, for two DISTINCT reasons that are both real:
/// (a) the Form's root is genuinely unresolvable (a stock/library Bluebeam stamp with
/// no embedded artwork in the `.btx` export - `markupToSvg`'s existing box+label
/// fallback is the correct outcome for these, not a bug); (b) a transient render-thread
/// failure (never silently swallowed - logged so it's visible, matching "degrade
/// explicitly, never silently").
///
/// Deliberate trade-off, named here rather than hidden: this REPLACES the stored asset,
/// not just a display-only copy - a stamp placed from a rasterized-preview Tool and
/// then saved embeds the PNG raster in the output PDF, not the original Bluebeam Form
/// XObject vector artwork. Chosen for scope/complexity reasons (a dual canonical-vs-
/// preview asset representation was assessed as out of reach for this pass) - flagged
/// as a follow-up if save-time vector fidelity for these stamps matters later.
async fn rasterize_form_xobject_asset(
    state: &AppState,
    root_id: &str,
    objects: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Option<StampAsset> {
    if !objects.contains_key(root_id) {
        return None; // (a) - no embedded artwork to render, not an error
    }
    let pdf_bytes = match crate::document::annots::build_isolated_form_xobject_pdf(root_id, objects)
    {
        Ok(b) => b,
        Err(e) => {
            log::warn!("rasterize_form_xobject_asset: failed to build isolated stamp pdf: {e:#}");
            return None; // (b) - logged, not silent
        }
    };
    match state.render.rasterize_pdf_bytes(pdf_bytes, 512).await {
        Ok(png) => Some(StampAsset::PngBase64(crate::render::base64_encode(&png))),
        Err(e) => {
            log::warn!("rasterize_form_xobject_asset: PDFium render failed: {e:#}");
            None // (b) - logged, not silent
        }
    }
}

/// Rasterize every `BluebeamFormXObject`-backed stamp `Tool` in `tools` to a `PngBase64`
/// preview, in place, so Tool Chest placement shows real artwork immediately instead of
/// an empty box (see `rasterize_form_xobject_asset` for what "rasterize" means and its
/// named trade-off). A tool whose asset can't be rasterized (library stamp, or a
/// transient failure) is left completely unchanged.
async fn rasterize_form_xobject_stamps_in_place(state: &AppState, tools: &mut [Tool]) {
    for tool in tools.iter_mut() {
        let (root_id, objects) = match &tool.stamp {
            Some(StampDef::Static {
                asset: StampAsset::BluebeamFormXObject { root_id, objects },
            }) => (root_id.clone(), objects.clone()),
            Some(StampDef::Dynamic {
                asset: Some(StampAsset::BluebeamFormXObject { root_id, objects }),
                ..
            }) => (root_id.clone(), objects.clone()),
            _ => continue,
        };
        let Some(rasterized) = rasterize_form_xobject_asset(state, &root_id, &objects).await else {
            continue;
        };
        match &mut tool.stamp {
            Some(StampDef::Static { asset }) => *asset = rasterized,
            Some(StampDef::Dynamic { asset, .. }) => *asset = Some(rasterized),
            None => {}
        }
    }
}

/// List every Tool Set (order is load order - see `ToolChestStore::load`; the frontend
/// is free to re-sort by name if desired).
#[tauri::command]
pub fn list_tool_sets(state: State<'_, AppState>) -> Vec<ToolSet> {
    state.toolchest.list_sets()
}

/// The auto-populated Recent Tools list, most-recently-used first.
#[tauri::command]
pub fn recent_tools(state: State<'_, AppState>) -> Vec<Tool> {
    state.toolchest.recent()
}

#[tauri::command]
pub fn create_tool_set(state: State<'_, AppState>, name: String) -> Result<ToolSet, String> {
    state.toolchest.create_set(name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_tool_set(state: State<'_, AppState>, set_id: String, name: String) -> Result<(), String> {
    let id = parse_uuid(&set_id, "tool set")?;
    state.toolchest.rename_set(id, name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_tool_set(state: State<'_, AppState>, set_id: String) -> Result<(), String> {
    let id = parse_uuid(&set_id, "tool set")?;
    state.toolchest.delete_set(id).map_err(|e| e.to_string())
}

/// Serialize the given markup's type + appearance [+ geometry, for Drawing mode] into a
/// new Tool and add it to `set_id` ("save current markup as tool", spec "Tools & Tool
/// Sets").
#[tauri::command]
pub fn add_tool_from_markup(
    state: State<'_, AppState>,
    set_id: String,
    markup: Markup,
    name: String,
    placement_mode: PlacementMode,
) -> Result<Tool, String> {
    let id = parse_uuid(&set_id, "tool set")?;
    let tool = Tool::from_markup(&markup, name, placement_mode);
    state.toolchest.add_tool(id, tool).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_tool(state: State<'_, AppState>, set_id: String, tool_id: String) -> Result<(), String> {
    let set = parse_uuid(&set_id, "tool set")?;
    let tool = parse_uuid(&tool_id, "tool")?;
    state.toolchest.delete_tool(set, tool).map_err(|e| e.to_string())
}

/// Reorder a set's tools to match `tool_ids` (front to back). Unnamed ids keep their
/// relative order, appended after - see `ToolChestStore::reorder_tools`.
#[tauri::command]
pub fn reorder_tools(state: State<'_, AppState>, set_id: String, tool_ids: Vec<String>) -> Result<(), String> {
    let set = parse_uuid(&set_id, "tool set")?;
    let ids = tool_ids
        .iter()
        .map(|s| parse_uuid(s, "tool"))
        .collect::<Result<Vec<_>, _>>()?;
    state.toolchest.reorder_tools(set, ids).map_err(|e| e.to_string())
}

/// Record a tool as recently used (move-to-front, de-duplicated, capped). Call this when
/// the user activates a tool from the Tool Chest panel.
#[tauri::command]
pub fn record_recent_tool(state: State<'_, AppState>, tool: Tool) -> Result<(), String> {
    state.toolchest.record_recent(tool).map_err(|e| e.to_string())
}

/// Import a `.btx` (or `.zip`-wrapped `.btx`) file from `path` as a new Tool Set named
/// after the file. Malformed items are skipped and reported in `ImportReport.skipped`,
/// never fatal to the whole import (spec "Importing Bluebeam Tool Sets & stamps").
#[tauri::command]
pub async fn import_btx(state: State<'_, AppState>, path: String) -> Result<ImportReport, String> {
    let bytes = tokio::fs::read(&path).await.map_err(|e| format!("read {path}: {e}"))?;
    let mut report = tokio::task::spawn_blocking(move || btx::import_btx_bytes(&bytes))
        .await
        .map_err(|e| e.to_string())?;

    // Rasterize BluebeamFormXObject stamp artwork to PngBase64 previews (2026-08-08
    // Form XObject stamp rendering follow-up) so a Tool Chest placement shows the real
    // stamp immediately instead of an empty box - see
    // `rasterize_form_xobject_stamps_in_place`'s doc comment for the named save-time
    // fidelity trade-off this makes.
    rasterize_form_xobject_stamps_in_place(&state, &mut report.tools).await;

    if !report.tools.is_empty() {
        let set_name = std::path::Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Imported Tools")
            .to_string();
        let mut set = ToolSet::new(set_name);
        set.tools = report.tools.clone();
        state.toolchest.import_set(set).map_err(|e| e.to_string())?;
    }

    Ok(report)
}

/// Advance and return the next sequence value for a dynamic stamp's auto-number field
/// (spec decision c, section 12). In-memory for v1 - see `toolchest::sequence` doc
/// comment for the named sidecar-persistence deferral.
#[tauri::command]
pub fn next_stamp_sequence(
    state: State<'_, AppState>,
    tool_id: String,
    scope: CounterScope,
    doc_id: String,
) -> Result<u32, String> {
    let id = parse_uuid(&tool_id, "tool")?;
    Ok(state.sequence_counters.next(scope, id, &doc_id))
}

/// Compose a dynamic stamp's placement-time text (spec "Stamps" - auto-fields substituted
/// at placement, never via embedded PDF JavaScript). `now` is read here (the one place
/// wall-clock/OS-timezone access belongs) as the OS LOCAL time - `Local::now()` resolves
/// the machine's timezone, `.fixed_offset()` freezes it to a self-contained
/// `DateTime<FixedOffset>` before handing off to the pure composer (which stays
/// deterministic/testable - see `toolchest::stamp::compose_dynamic_text`'s doc comment).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn compose_stamp_text(
    base_text: String,
    fields: Vec<DynamicField>,
    username: String,
    document_name: String,
    sequence: u32,
    prompted: Vec<String>,
) -> String {
    let now = Local::now().fixed_offset();
    compose_dynamic_text(&base_text, &fields, now, &username, &document_name, sequence, &prompted)
}
