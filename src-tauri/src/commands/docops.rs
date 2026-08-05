//! DocOps Tauri commands — M5 baseline (spec §4, §8).
//!
//! Exposes `flatten_document`, `optimize_document`, and `redact_document`:
//! lopdf-backed PDF surgery operations that reload the render engine and write
//! the result atomically.
//!
//! All three commands reuse `apply_page_edit` from `commands::document` (the
//! same load-op-save-reload pipeline used by all page ops) to keep the save
//! contract consistent: markups are preserved, the file is written atomically,
//! the render engine is reopened, and the markup cache is invalidated.

use serde::Serialize;
use tauri::State;

use crate::commands::document::apply_page_edit;
use crate::docops::{flatten_annotations, optimize_in_place, redact_annotations, redact_regions, RedactRegion};
use crate::AppState;

/// File-size before/after an `optimize_document` call, so the frontend can report what
/// actually happened (pruned/compressed vs. genuinely nothing to save) instead of giving
/// identical silent-success feedback either way - the same "success that changed nothing
/// must say so" gap `flatten_document`'s returned count closes for Flatten.
#[derive(Debug, Clone, Serialize)]
pub struct OptimizeReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
}

/// Flatten all annotation appearance streams in the open document into page content.
///
/// After completion:
/// - Annotations that had a Normal appearance stream (`/AP /N`) are baked into
///   the page content and removed from the PDF `/Annots` array.
/// - Annotations without an appearance stream (e.g. popup notes) are untouched.
/// - The render engine is reloaded so the viewport reflects the updated file.
///
/// Returns the number of annotations actually flattened (`0` is a real, meaningful
/// result - nothing on the document had a bakeable `/AP /N` - distinct from an error).
/// The frontend uses this to tell a genuine no-op apart from a real flatten, rather than
/// showing identical "success" feedback either way (see `obs` on the live report "flatten
/// doesn't seem to do anything" - an unqualified `Ok(())` can't distinguish the two).
///
/// # Errors
///
/// Returns an error string if the document is not open, the lopdf parse fails,
/// or the atomic save/rename fails.
#[tauri::command]
pub async fn flatten_document(state: State<'_, AppState>, doc_id: String) -> Result<usize, String> {
    // `apply_page_edit`'s `op` closure is `FnOnce(&mut Document) -> anyhow::Result<()>` -
    // shared by every page op (rotate/delete/reorder/insert/redact/optimize), so it isn't
    // generic over a return value. Captured via `Arc<Mutex<_>>` instead of widening that
    // shared signature - keeps the blast radius to this one command.
    let count = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let count_write = std::sync::Arc::clone(&count);
    apply_page_edit(&state, &doc_id, move |doc| {
        let n = flatten_annotations(doc)?;
        *count_write.lock().expect("count mutex poisoned") = n;
        Ok(())
    })
    .await?;
    let n = *count.lock().expect("count mutex poisoned");
    Ok(n)
}

/// Apply redactions to the open document.
///
/// Two modes, used together in the toolbar "Apply Redactions" flow:
///
/// 1. **`regions`** (explicit coordinates): each `RedactRegion` overlays a 1×1 solid-black
///    DeviceGray Image XObject scaled to fill the region rectangle.  Pass an empty slice
///    when this mode is not needed.
/// 2. **`apply_annots`**: when `true`, scan every page for `/Subtype /Redact` annotations,
///    apply the same Image XObject overlay for each one, and remove the consumed annotations
///    from `/Annots`.
///
/// Both modes run in a single `apply_page_edit` pass so the file is written once.
///
/// After completion the render engine is reloaded so the viewport reflects the updated file.
///
/// # Errors
///
/// Returns an error string if the document is not open, the lopdf parse fails,
/// or the atomic save/rename fails.
#[tauri::command]
pub async fn redact_document(
    state: State<'_, AppState>,
    doc_id: String,
    regions: Vec<RedactRegion>,
    apply_annots: bool,
) -> Result<(), String> {
    apply_page_edit(&state, &doc_id, move |doc| {
        if !regions.is_empty() {
            redact_regions(doc, &regions)?;
        }
        if apply_annots {
            redact_annotations(doc)?;
        }
        Ok(())
    })
    .await
}

/// Optimize the open document (prune unused objects and/or compress streams).
///
/// `level` controls optimization depth (see `docops::optimize_in_place`):
/// - 0: no-op.
/// - 1: prune unreferenced objects only (lossless, fast).
/// - 2: prune + Deflate-compress all compressable streams (default for UI button).
///
/// After completion the render engine is reloaded so the viewport reflects the
/// updated file. Returns the file size before and after (see `OptimizeReport`) - optimize
/// has no other user-visible effect (no annotations change, no viewport difference), so
/// without this the toolbar action would look identical whether it saved a meaningful
/// amount of space or nothing at all.
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
) -> Result<OptimizeReport, String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let bytes_before = std::fs::metadata(&path)
        .map(|m| m.len())
        .map_err(|e| format!("stat {}: {e}", path.display()))?;

    apply_page_edit(&state, &doc_id, move |doc| optimize_in_place(doc, level)).await?;

    let bytes_after = std::fs::metadata(&path)
        .map(|m| m.len())
        .map_err(|e| format!("stat {}: {e}", path.display()))?;
    Ok(OptimizeReport { bytes_before, bytes_after })
}
