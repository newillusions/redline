//! DocOps Tauri commands — M5 baseline (spec §4, §8).
//!
//! Exposes `flatten_document` and `optimize_document`: lopdf-backed PDF surgery
//! operations that reload the render engine and write the result atomically.
//!
//! Both commands reuse `apply_page_edit` from `commands::document` (the same
//! load-op-save-reload pipeline used by all page ops) to keep the save
//! contract consistent: markups are preserved, the file is written atomically,
//! the render engine is reopened, and the markup cache is invalidated.

use tauri::State;

use crate::commands::document::apply_page_edit;
use crate::docops::{flatten_annotations, optimize_in_place};
use crate::AppState;

/// Flatten all annotation appearance streams in the open document into page content.
///
/// After completion:
/// - Annotations that had a Normal appearance stream (`/AP /N`) are baked into
///   the page content and removed from the PDF `/Annots` array.
/// - Annotations without an appearance stream (e.g. popup notes) are untouched.
/// - The render engine is reloaded so the viewport reflects the updated file.
///
/// # Errors
///
/// Returns an error string if the document is not open, the lopdf parse fails,
/// or the atomic save/rename fails.
#[tauri::command]
pub async fn flatten_document(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<(), String> {
    apply_page_edit(&state, &doc_id, move |doc| flatten_annotations(doc)).await
}

/// Optimize the open document (prune unused objects and/or compress streams).
///
/// `level` controls optimization depth (see `docops::optimize_in_place`):
/// - 0: no-op.
/// - 1: prune unreferenced objects only (lossless, fast).
/// - 2: prune + Deflate-compress all compressable streams (default for UI button).
///
/// After completion the render engine is reloaded so the viewport reflects the
/// updated file.
///
/// # Errors
///
/// Returns an error string if the document is not open, the lopdf parse fails,
/// or the atomic save/rename fails.
#[tauri::command]
pub async fn optimize_document(
    state: State<'_, AppState>,
    doc_id: String,
    level: u8,
) -> Result<(), String> {
    apply_page_edit(&state, &doc_id, move |doc| optimize_in_place(doc, level)).await
}
