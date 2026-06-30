//! Tauri IPC commands for the Most-Recently-Used document list.
//!
//! Persists to `<app-data-dir>/recent-docs.json` using Tauri's resolved
//! app data directory (per-user, OS-standard location).
//!
//! ## Commands exposed to the webview
//! - `load_recent_docs`   — return the current MRU list (newest first)
//! - `save_recent_docs`   — persist an updated MRU list
//! - `check_file_exists`  — tell the panel whether a path still exists on disk

use tauri::Manager;

use crate::storage::recent_docs::{
    load_recent_docs as storage_load, save_recent_docs as storage_save, MruEntry,
};

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Return the stored MRU list from `<app-data-dir>/recent-docs.json`.
///
/// Returns an empty array if the file has never been written yet.
/// Returns a rejected promise on genuine IO or parse errors.
#[tauri::command]
pub async fn load_recent_docs(app_handle: tauri::AppHandle) -> Result<Vec<MruEntry>, String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;

    tokio::task::spawn_blocking(move || storage_load(&data_dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e}"))
}

/// Persist the provided MRU list to `<app-data-dir>/recent-docs.json`.
///
/// The list should already be sorted newest-first and capped (the frontend
/// `upsertMru` helper manages this before calling here).
#[tauri::command]
pub async fn save_recent_docs(
    app_handle: tauri::AppHandle,
    entries: Vec<MruEntry>,
) -> Result<(), String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;

    tokio::task::spawn_blocking(move || storage_save(&data_dir, &entries))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e}"))
}

/// Return `true` if the given absolute path exists on disk.
///
/// Used by the panel to grey out files that have been moved or deleted.
/// Non-blocking — `std::path::Path::exists()` is cheap for local paths.
#[tauri::command]
pub fn check_file_exists(path: String) -> bool {
    std::path::Path::new(&path).exists()
}
