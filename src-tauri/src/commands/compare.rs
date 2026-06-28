//! Tauri command — page-pair two-tier diff (M6 compare module).
//!
//! The `compare_pages` command is the IPC entry point for the Svelte compare UI.
//! It runs a text-layer diff (tier 1) followed by a pixel diff (tier 2) and returns
//! a `PageDiffResult` that the frontend can render as a color-channel overlay.

use crate::compare::{run_two_tier_diff, PageDiffResult};

/// Compare two PDF pages using the two-tier diff algorithm.
///
/// Runs on a `spawn_blocking` thread because `PdfDiffEngine` is `!Send + !Sync`.
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
    path_a: String,
    path_b: String,
    page_a: u32,
    page_b: u32,
    dpi: Option<f32>,
    pixel_tolerance: Option<u8>,
) -> Result<PageDiffResult, String> {
    let dpi = dpi.unwrap_or(150.0);
    let tolerance = pixel_tolerance.unwrap_or(5);

    tokio::task::spawn_blocking(move || {
        run_two_tier_diff(
            std::path::Path::new(&path_a),
            std::path::Path::new(&path_b),
            page_a,
            page_b,
            dpi,
            tolerance,
        )
        .map_err(|e| format!("{:#}", e))
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}
