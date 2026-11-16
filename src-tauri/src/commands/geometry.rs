//! Tauri commands for the vector snap-target index (spec §5, v1).
//!
//! Own file (not `commands/document.rs`) for the same reason as
//! `commands/text_select.rs`: a pure passthrough to `RenderHandle`, which does
//! the actual PDFium work on the dedicated render thread, and isolated so it can
//! be edited without touching a shared hot file.

use tauri::State;

use crate::geometry::PageSnapTargets;
use crate::AppState;

/// Return the full snap-target index for a page (spec §5, v1 — `Endpoint` and
/// `Midpoint` targets only; see `geometry` module doc comment for what's
/// deliberately not yet populated). Returns the whole page's targets rather than
/// a per-cursor nearest-neighbour query so the frontend can do the actual
/// nearest-target lookup client-side against a local cache while dragging,
/// without an IPC round-trip on every pointer-move.
///
/// The response also carries `cap` (`SegmentCap`) when the page was too dense to index in
/// full, so the viewer can say snapping is reduced.
///
/// Built once per (document, page) on first call and cached on the render
/// thread until the document closes (see `RenderEngine::page_snap_index`).
#[tauri::command]
pub async fn get_page_snap_targets(
    state: State<'_, AppState>,
    doc_id: String,
    page_index: u32,
) -> Result<PageSnapTargets, String> {
    state
        .render
        .page_snap_index(doc_id, page_index)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Return the page's straight segments as `[x1, y1, x2, y2]` (PDF user space) so the
/// frontend can compute cursor-dependent snaps (perpendicular, nearest-on-segment)
/// locally. Curve pieces are excluded. Same cached geometry as
/// [`get_page_snap_targets`].
#[tauri::command]
pub async fn get_page_snap_segments(
    state: State<'_, AppState>,
    doc_id: String,
    page_index: u32,
) -> Result<Vec<[f64; 4]>, String> {
    state
        .render
        .page_snap_segments(doc_id, page_index)
        .await
        .map_err(|e| format!("{e:#}"))
}
