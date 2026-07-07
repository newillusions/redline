//! Tauri commands for the Tool Chest (spec "Tools & Tool Sets", "Stamps",
//! "Importing Bluebeam Tool Sets & stamps") - list/create/rename/delete Tool Sets,
//! add/delete/reorder Tools, Recent Tools, `.btx` import, and dynamic-stamp field
//! composition.

use chrono::Utc;
use tauri::State;
use uuid::Uuid;

use crate::markup::Markup;
use crate::toolchest::btx::{self, ImportReport};
use crate::toolchest::stamp::{compose_dynamic_text, DynamicField};
use crate::toolchest::{CounterScope, PlacementMode, Tool, ToolSet};
use crate::AppState;

fn parse_uuid(s: &str, what: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("bad {what} id: {e}"))
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
    let report = tokio::task::spawn_blocking(move || btx::import_btx_bytes(&bytes))
        .await
        .map_err(|e| e.to_string())?;

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
/// wall-clock access belongs) and handed to the pure composer.
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
    compose_dynamic_text(&base_text, &fields, Utc::now(), &username, &document_name, sequence, &prompted)
}
