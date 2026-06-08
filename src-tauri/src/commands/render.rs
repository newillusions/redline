//! Tauri commands for the render module (spec §4, §5).
//!
//! All tile requests go through here. The webview requests visible tiles at the
//! current zoom; Rust rasterizes via PDFium on the dedicated render thread and
//! returns PNG-encoded bytes.

use crate::render::{PageSize, RenderedTile, TileRequest};
use crate::AppState;
use tauri::State;

/// Rasterize a single tile and return it as a PNG-encoded base64 string.
///
/// Called by the frontend viewport for every tile in the visible area.
/// The webview draws the returned bytes onto a canvas element.
#[tauri::command]
pub async fn render_tile(
    state: State<'_, AppState>,
    req: TileRequest,
) -> Result<RenderedTile, String> {
    state
        .render
        .render_tile(req)
        .await
        .map_err(|e| format!("{:#}", e))
}

/// Return the number of pages in an open document.
#[tauri::command]
pub async fn get_page_count(state: State<'_, AppState>, doc_id: String) -> Result<u32, String> {
    state
        .render
        .page_count(doc_id.clone())
        .await
        .map_err(|e| format!("{:#}", e))?
        .ok_or_else(|| format!("Unknown doc_id: {}", doc_id))
}

/// Return the size of a specific page in PDF user-space points.
#[tauri::command]
pub async fn get_page_size(
    state: State<'_, AppState>,
    doc_id: String,
    page_index: u32,
) -> Result<PageSize, String> {
    state
        .render
        .page_size(doc_id, page_index)
        .await
        .map_err(|e| format!("{:#}", e))
}
