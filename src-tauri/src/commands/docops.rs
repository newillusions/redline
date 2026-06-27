//! DocOps Tauri commands — M5 baseline (spec §4, §8).
//!
//! Exposes `flatten_document`: bake annotation appearance streams into page
//! content via lopdf, then reload the render engine so tiles refresh.
//!
//! The command reuses `apply_page_edit` from `commands::document` (the same
//! load-op-save-reload pipeline used by all page ops) to keep the save
//! contract consistent: markups are preserved, the file is written atomically,
//! the render engine is reopened, and the markup cache is invalidated.

use tauri::State;

use crate::commands::document::apply_page_edit;
use crate::docops::flatten_annotations;
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
