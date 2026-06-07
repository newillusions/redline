//! Tauri commands for document open/close (spec §4).

use std::path::PathBuf;
use tauri::State;

use crate::document::{new_doc_id, DocumentInfo};
use crate::AppState;

/// Open a PDF file. Returns a `DocumentInfo` with a fresh `doc_id`.
#[tauri::command]
pub async fn open_document(
    state: State<'_, AppState>,
    path: String,
) -> Result<DocumentInfo, String> {
    let path = PathBuf::from(&path);

    if !path.exists() {
        return Err(format!("File not found: {}", path.display()));
    }
    if path.extension().and_then(|e| e.to_str()) != Some("pdf") {
        return Err(format!("Not a PDF file: {}", path.display()));
    }

    let doc_id = new_doc_id();
    let page_count = state.render
        .open_document(path.clone(), doc_id.clone())
        .await
        .map_err(|e| format!("{:#}", e))?;

    Ok(DocumentInfo {
        doc_id,
        path: path.to_string_lossy().into_owned(),
        page_count,
    })
}

/// Close an open document and release its resources.
#[tauri::command]
pub async fn close_document(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<(), String> {
    state.render.close_document(doc_id).await.map_err(|e| format!("{:#}", e))
}
