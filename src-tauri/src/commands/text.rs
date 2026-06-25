//! Tauri commands for in-document text search (spec §4, M4 S3).

use tauri::State;

use crate::text::{SearchHit, SearchOptions};
use crate::AppState;

/// Search for all occurrences of `query` across all pages of an open document.
///
/// Iterates every page and calls `render_handle.search_page` (which runs the
/// PDFium text-search API on the render thread). Results across all pages are
/// returned as a flat list ordered by page then occurrence.
///
/// Returns an empty Vec when the query is empty or has no matches.
/// Returns Err if `doc_id` is unknown.
///
/// # Timeout discipline
/// Each per-page call dispatches to the render thread channel (bounded, 64 slots).
/// The render thread processes messages serially; a search on a 300-page document
/// serialises 300 messages. On typical construction drawings this runs in < 1 s.
/// The command is async; Tauri runs it on the tokio thread pool so the webview
/// remains responsive.
#[tauri::command]
pub async fn search_document(
    state: State<'_, AppState>,
    doc_id: String,
    query: String,
    case_sensitive: bool,
    whole_word: bool,
) -> Result<Vec<SearchHit>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let options = SearchOptions {
        case_sensitive,
        whole_word,
    };

    // Get the page count via the render handle (it knows what's open).
    let page_count = state
        .render
        .page_count(doc_id.clone())
        .await
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("unknown doc_id: {doc_id}"))?;

    let mut all_hits: Vec<SearchHit> = Vec::new();

    for page_index in 0..page_count {
        let hits = state
            .render
            .search_page(doc_id.clone(), page_index, query.clone(), options.clone())
            .await
            .map_err(|e| format!("page {page_index}: {e:#}"))?;
        all_hits.extend(hits);
    }

    Ok(all_hits)
}
