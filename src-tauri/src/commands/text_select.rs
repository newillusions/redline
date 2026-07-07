//! Tauri commands for the I-beam text-selection tool (redline text-selection +
//! text-anchored highlight feature).
//!
//! Deliberately its own file (not `commands/text.rs`, which owns in-document
//! search/M4 S3) so it can be edited without touching that file - both commands
//! here are pure passthroughs to `RenderHandle`, which does the actual PDFium
//! work on the dedicated render thread.

use tauri::State;

use crate::text::TextRangeSelection;
use crate::AppState;

/// Hit-test a PDF-user-space point (`x`, `y`, same coordinate system as markups -
/// spec §5 invariant) to the nearest character index on `page_index`, within
/// `tolerance` PDF points. Returns `None` when no character is within tolerance.
///
/// Called on pointer-down and pointer-move while the I-beam tool is dragging, to
/// resolve the drag anchor/focus into character indices.
#[tauri::command]
pub async fn char_index_at_point(
    state: State<'_, AppState>,
    doc_id: String,
    page_index: u32,
    x: f64,
    y: f64,
    tolerance: f64,
) -> Result<Option<usize>, String> {
    state
        .render
        .char_index_at_point(doc_id, page_index, x, y, tolerance)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Resolve a character range `[start, end)` on `page_index` into the `Quad`s for
/// a text-anchored Highlight annotation plus the plain-text content for the
/// clipboard. Returns an empty selection (not an error) for `end <= start`.
#[tauri::command]
pub async fn get_text_selection(
    state: State<'_, AppState>,
    doc_id: String,
    page_index: u32,
    start: usize,
    end: usize,
) -> Result<TextRangeSelection, String> {
    state
        .render
        .text_range_selection(doc_id, page_index, start, end)
        .await
        .map_err(|e| format!("{e:#}"))
}
