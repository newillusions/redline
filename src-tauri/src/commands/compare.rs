//! Tauri command — page-pair two-tier diff (M6 compare module).
//!
//! The `compare_pages` command is the IPC entry point for the Svelte compare UI
//! (and, via `rpc::dispatch`, the MCP `compare_pages` tool). It runs a text-layer
//! diff (tier 1) followed by a pixel diff (tier 2) and returns a `PageDiffResult`
//! that the frontend can render as a color-channel overlay.

use tauri::State;

use crate::compare::PageDiffResult;
use crate::AppState;

/// Compare two PDF pages using the two-tier diff algorithm.
///
/// Dispatches to the render thread via `RenderHandle::compare_pages` so the diff
/// reuses the render engine's already-loaded PDFium binding instead of creating a
/// second, independent one — see `compare::run_two_tier_diff`'s doc comment for why
/// that used to hang the process indefinitely (PR #99 review finding, 2026-09-03,
/// observation:x6xsf3hlpepo9ohc7wij). No `spawn_blocking` needed here: the render
/// thread is where the (synchronous, `!Send`) PDFium work actually runs; this
/// command just awaits the channel round trip like every other render-thread command.
///
/// # Arguments
/// * `path_a`         — absolute path to the "old" PDF
/// * `path_b`         — absolute path to the "new" PDF
/// * `page_a`         — 0-based page index in `path_a`
/// * `page_b`         — 0-based page index in `path_b`
/// * `dpi`            — render DPI for the pixel diff (default 150.0)
/// * `pixel_tolerance`— per-channel delta that counts as "same" (default 5, handles AA)
///
/// # Returns
/// `PageDiffResult` — tier-1 text match + tier-2 pixel stats + PNG diff overlay as base64.
#[tauri::command]
pub async fn compare_pages(
    state: State<'_, AppState>,
    path_a: String,
    path_b: String,
    page_a: u32,
    page_b: u32,
    dpi: Option<f32>,
    pixel_tolerance: Option<u8>,
) -> Result<PageDiffResult, String> {
    let dpi = dpi.unwrap_or(150.0);
    let tolerance = pixel_tolerance.unwrap_or(5);

    state
        .render
        .compare_pages(
            std::path::PathBuf::from(path_a),
            std::path::PathBuf::from(path_b),
            page_a,
            page_b,
            dpi,
            tolerance,
        )
        .await
        .map_err(|e| format!("{:#}", e))
}
