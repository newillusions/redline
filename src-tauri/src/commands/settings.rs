//! Tauri IPC commands for application settings (local user preferences).
//!
//! Persists to `<app-data-dir>/settings.json` using Tauri's resolved
//! app data directory (per-user, OS-standard location).
//!
//! ## Commands exposed to the webview
//! - `load_settings` - return the current settings (defaults if never saved)
//! - `save_settings` - persist updated settings

use tauri::Manager;

use crate::storage::settings::{
    load_settings as storage_load, save_settings as storage_save, AppSettings,
};

/// Return the stored settings from `<app-data-dir>/settings.json`.
///
/// Returns `AppSettings::default()` if the file has never been written yet.
/// Returns a rejected promise on genuine IO or parse errors.
#[tauri::command]
pub async fn load_settings(app_handle: tauri::AppHandle) -> Result<AppSettings, String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;

    tokio::task::spawn_blocking(move || storage_load(&data_dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e}"))
}

/// Persist the provided settings to `<app-data-dir>/settings.json`.
#[tauri::command]
pub async fn save_settings(
    app_handle: tauri::AppHandle,
    settings: AppSettings,
) -> Result<(), String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;

    tokio::task::spawn_blocking(move || storage_save(&data_dir, &settings))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e}"))
}
