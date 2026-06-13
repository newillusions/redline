//! Tauri commands for document open/close (spec §4).

use std::path::PathBuf;
use tauri::State;

use crate::document::save::{load_markups_from, save_with_markups};
use crate::document::{new_doc_id, DocumentInfo};
use crate::markup::Markup;
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
    let page_count = state
        .render
        .open_document(path.clone(), doc_id.clone())
        .await
        .map_err(|e| format!("{:#}", e))?;

    state.markups.register(&doc_id, path.clone());

    Ok(DocumentInfo {
        doc_id,
        path: path.to_string_lossy().into_owned(),
        page_count,
    })
}

/// Close an open document and release its resources.
/// Render close happens first; store entry is removed only after it succeeds.
#[tauri::command]
pub async fn close_document(state: State<'_, AppState>, doc_id: String) -> Result<(), String> {
    state
        .render
        .close_document(doc_id.clone())
        .await
        .map_err(|e| format!("{:#}", e))?;
    state.markups.remove(&doc_id);
    Ok(())
}

/// Add a markup to the open document's in-memory set (not yet saved to the file).
#[tauri::command]
pub async fn add_markup(
    state: State<'_, AppState>,
    doc_id: String,
    markup: Markup,
) -> Result<(), String> {
    state.markups.add(&doc_id, markup)
}

/// List the open document's in-memory markups.
#[tauri::command]
pub async fn list_markups(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<Vec<Markup>, String> {
    state.markups.list(&doc_id)
}

/// Read existing annotations from the PDF into the store (call after open; lopdf runs
/// in a blocking task). Merges beneath unsaved in-memory markups; store wins on id.
#[tauri::command]
pub async fn load_markups(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<Vec<Markup>, String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let loaded = tokio::task::spawn_blocking(move || load_markups_from(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    state.markups.seed_loaded(&doc_id, loaded)
}

/// Save the in-memory markups into the open file (atomic in-place).
#[tauri::command]
pub async fn save_document(state: State<'_, AppState>, doc_id: String) -> Result<(), String> {
    save_impl(state, doc_id, None).await
}

/// Save-As: write to `new_path` and switch the open document to it (same doc_id).
#[tauri::command]
pub async fn save_document_as(
    state: State<'_, AppState>,
    doc_id: String,
    new_path: String,
) -> Result<(), String> {
    save_impl(state, doc_id, Some(PathBuf::from(new_path))).await
}

/// Shared save flow entry: acquires the per-doc save-in-flight guard, then runs
/// the actual save. The guard is released on EVERY exit path - `save_inner`
/// returns its Result here and `end_save` runs unconditionally before returning.
/// Two concurrent saves on the same doc_id would otherwise write the same
/// staged path and interleave the close/rename/reopen sequence (corruption).
async fn save_impl(
    state: State<'_, AppState>,
    doc_id: String,
    new_path: Option<PathBuf>,
) -> Result<(), String> {
    state.markups.begin_save(&doc_id)?;
    let result = save_inner(&state, &doc_id, new_path).await;
    state.markups.end_save(&doc_id);
    result
}

/// Actual save flow. Order matters (see save_with_markups doc contract):
/// stage the rewritten file to a sibling path FIRST (source open in the render
/// engine is fine - reads only), THEN close the render doc (Windows cannot
/// rename over an open file), swap, reopen under the SAME doc_id.
async fn save_inner(
    state: &State<'_, AppState>,
    doc_id: &str,
    new_path: Option<PathBuf>,
) -> Result<(), String> {
    let src = state
        .markups
        .path(doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let dest = new_path.clone().unwrap_or_else(|| src.clone());

    // Load-before-save guard: never strip annotations that were never imported.
    // is_managed treats every /RLType annot as ours, so saving an un-loaded doc
    // would replace pre-existing redline annotations with only the new ones.
    if !state.markups.is_loaded(doc_id) {
        let p = src.clone();
        let loaded = tokio::task::spawn_blocking(move || load_markups_from(&p))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
        state.markups.seed_loaded(doc_id, loaded)?;
    }
    let markups = state.markups.list(doc_id)?;

    // 1. Stage the complete rewritten file next to the destination.
    let staged = dest.with_extension("pdf.redline-staged");
    {
        let src = src.clone();
        let staged = staged.clone();
        tokio::task::spawn_blocking(move || save_with_markups(&src, &staged, &markups))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
    }

    // 2. Release the file from the render engine, swap, reopen under the same doc_id.
    state
        .render
        .close_document(doc_id.to_string())
        .await
        .map_err(|e| format!("{e:#}"))?;
    if let Err(e) = std::fs::rename(&staged, &dest) {
        let _ = std::fs::remove_file(&staged);
        // Try to restore the render doc on the ORIGINAL path before failing.
        let _ = state
            .render
            .open_document(src.clone(), doc_id.to_string())
            .await;
        return Err(format!("swap failed: {e}"));
    }
    state
        .render
        .open_document(dest.clone(), doc_id.to_string())
        .await
        .map_err(|e| format!("reopen after save: {e:#}"))?;
    if new_path.is_some() {
        state.markups.set_path(doc_id, dest)?;
    }
    Ok(())
}
