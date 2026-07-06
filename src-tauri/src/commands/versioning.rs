//! Tauri commands for version snapshot operations (M4 S2, spec §15/§18).

use tauri::State;

use crate::sidecar::VersionRecord;
use crate::storage::{list_versions, restore_version, save_version_snapshot};
use crate::AppState;

/// Default number of history snapshots to retain per file.
const DEFAULT_RETAIN_N: usize = 10;

/// Save a version snapshot of the open document before overwriting it.
///
/// Hooks into the save pipeline: call this BEFORE `save_document` to capture the
/// pre-save state in `.redline/history/`.
///
/// Returns the created `VersionRecord`.
#[tauri::command]
pub async fn snapshot_version(
    state: State<'_, AppState>,
    doc_id: String,
    label: Option<String>,
) -> Result<VersionRecord, String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;

    tokio::task::spawn_blocking(move || save_version_snapshot(&path, label, DEFAULT_RETAIN_N))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e}"))
}

/// List version records for the open document, newest first.
#[tauri::command]
pub async fn list_document_versions(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<Vec<VersionRecord>, String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;

    tokio::task::spawn_blocking(move || list_versions(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e}"))
}

/// Restore a version snapshot back over the live PDF, then reload the render engine.
///
/// The render engine must be reloaded after restore so tiles reflect the restored content.
#[tauri::command]
pub async fn restore_document_version(
    state: State<'_, AppState>,
    doc_id: String,
    version_id: String,
) -> Result<(), String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;

    // Restore the snapshot (blocking IO).
    let path2 = path.clone();
    tokio::task::spawn_blocking(move || restore_version(&path2, &version_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e}"))?;

    // Invalidate the markup cache so the next load_markups re-parses the restored PDF.
    state.markups.invalidate_cache(&path);

    // Close + reopen the render engine so tiles reflect restored content.
    let password = state.markups.password(&doc_id);
    state
        .render
        .close_document(doc_id.clone())
        .await
        .map_err(|e| format!("{e:#}"))?;
    state
        .render
        .open_document(path.clone(), doc_id, password)
        .await
        .and_then(|outcome| outcome.into_page_count())
        .map_err(|e| format!("reopen after restore: {e:#}"))?;

    Ok(())
}
