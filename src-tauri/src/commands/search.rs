//! Tauri IPC commands — folder full-text search (M4 S4) + ad-hoc path search
//! (search-parity: Bluebeam's "Recents" scope, spec-confirmed via
//! support.bluebeam.com/se/user-manual/menus/window/search-panel.html).
//!
//! Four commands:
//!   open_folder_index   — create/open the Tantivy index for a folder and start
//!                         the background indexer + file watcher.
//!   search_folder       — run a query against the active index.
//!   folder_index_status — poll the indexing state / hit counts.
//!   search_paths        — one-off, non-indexed search over an explicit small
//!                         file list (the Recents/MRU list) — no persistent
//!                         Tantivy index for a bounded, rarely-searched set.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};

use crate::{
    search::{indexer, FolderIndex, FolderSearchHit, IndexState, IndexStatus},
    AppState,
};

// ---------------------------------------------------------------------------
// Deterministic folder fingerprint for the index subdirectory name.
//
// Uses `DefaultHasher` — not cryptographically stable across Rust releases,
// but sufficient for a local cache key (a changed fingerprint just means a
// fresh index is created, not a data loss event).
// ---------------------------------------------------------------------------

fn folder_fingerprint(folder_path: &std::path::Path) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    folder_path.to_string_lossy().hash(&mut h);
    format!("{:016x}", h.finish())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Open (or reopen) the Tantivy folder index for `folder_path`.
///
/// The index is stored at `$APPDATA/Redline/indexes/<fingerprint>/`.
/// A background OS thread is spawned to perform the initial full-index pass
/// and then watch the folder for incremental changes.
///
/// Returns the initial `IndexStatus` (files = 0, state = Indexing) so the
/// frontend can immediately start polling.
#[tauri::command]
pub async fn open_folder_index(
    app: AppHandle,
    state: State<'_, AppState>,
    folder_path: String,
) -> Result<IndexStatus, String> {
    let folder_path_buf = PathBuf::from(&folder_path);

    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;

    let fingerprint = folder_fingerprint(&folder_path_buf);
    let index_dir = app_data_dir
        .join("Redline")
        .join("indexes")
        .join(&fingerprint);

    let folder_index = FolderIndex::open_or_create(&index_dir, &folder_path_buf)
        .map_err(|e| format!("open_or_create index: {e}"))?;

    // Replace the active index in AppState.
    *state.folder_index.lock().unwrap() = Some(folder_index.clone());

    // Spawn the background indexer on a dedicated OS thread so it can block
    // without consuming tokio's blocking thread pool indefinitely.
    let index_for_bg = folder_index.clone();
    std::thread::spawn(move || {
        indexer::index_folder_blocking(index_for_bg, folder_path_buf);
    });

    Ok(folder_index.status())
}

/// Search the active folder index for `query`.
///
/// Returns up to `limit` hits (default 50) sorted by relevance.  Returns an
/// error if no folder index has been opened.
#[tauri::command]
pub async fn search_folder(
    state: State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<FolderSearchHit>, String> {
    // Clone the Arc handle then drop the mutex guard before the blocking call.
    let index = {
        let guard = state.folder_index.lock().unwrap();
        guard
            .as_ref()
            .ok_or_else(|| "No folder index open — call open_folder_index first".to_string())?
            .clone()
    };

    tokio::task::spawn_blocking(move || {
        index
            .search(&query, limit.unwrap_or(50) as usize)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Return the current status of the active folder index.
///
/// Returns an empty idle status (folder_path = "") if no index is open.
#[tauri::command]
pub async fn folder_index_status(state: State<'_, AppState>) -> Result<IndexStatus, String> {
    let guard = state.folder_index.lock().unwrap();
    Ok(match guard.as_ref() {
        Some(idx) => idx.status(),
        None => IndexStatus {
            folder_path: String::new(),
            indexed_files: 0,
            indexed_pages: 0,
            state: IndexState::Idle,
        },
    })
}

/// Search an explicit, bounded list of PDF paths (the Recents/MRU list) for
/// `query`, without building or touching any persistent Tantivy index.
///
/// A missing or unreadable file is skipped, not fatal — a stale MRU entry
/// (moved/deleted file) must not blank out results from the rest of the list.
/// Reuses `indexer::extract_pdf_text` (the same lopdf extraction folder search
/// uses) so behavior is consistent between the two scopes.
#[tauri::command]
pub async fn search_paths(
    paths: Vec<String>,
    query: String,
    case_sensitive: bool,
    whole_word: bool,
) -> Result<Vec<FolderSearchHit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    tokio::task::spawn_blocking(move || {
        let mut hits = Vec::new();
        for path_str in &paths {
            let path = PathBuf::from(path_str);
            let Ok(pages) = indexer::extract_pdf_text(&path) else {
                continue; // stale MRU entry (moved/deleted) — skip, don't fail the batch
            };
            for (page_num, text) in pages {
                if let Some(snippet) = find_snippet(&text, &query, case_sensitive, whole_word) {
                    hits.push(FolderSearchHit {
                        file_path: path_str.clone(),
                        page_number: page_num,
                        snippet,
                        source: "lopdf".to_string(),
                    });
                }
            }
        }
        hits
    })
    .await
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Snippet matching (mirrors src/lib/markup-search.ts's word-boundary logic so
// the two ad-hoc/non-Tantivy search paths behave identically to the user).
// ---------------------------------------------------------------------------

const SNIPPET_RADIUS: usize = 60;

fn find_snippet(
    haystack: &str,
    needle: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Option<String> {
    let h_owned;
    let n_owned;
    let (h, n): (&str, &str) = if case_sensitive {
        (haystack, needle)
    } else {
        h_owned = haystack.to_lowercase();
        n_owned = needle.to_lowercase();
        (&h_owned, &n_owned)
    };
    if n.is_empty() {
        return None;
    }

    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0usize;
    loop {
        let idx = h[from..].find(n)? + from;
        if whole_word {
            let before_ok = h[..idx]
                .chars()
                .next_back()
                .map(|c| !is_word_char(c))
                .unwrap_or(true);
            let after_idx = idx + n.len();
            let after_ok = h[after_idx..]
                .chars()
                .next()
                .map(|c| !is_word_char(c))
                .unwrap_or(true);
            if !before_ok || !after_ok {
                from = idx + 1;
                if from >= h.len() {
                    return None;
                }
                continue;
            }
        }

        // Build the snippet from the ORIGINAL (not lowercased) haystack, using
        // char-boundary-safe byte windows around the match.
        let start = haystack
            .char_indices()
            .rev()
            .find(|(i, _)| *i <= idx.saturating_sub(SNIPPET_RADIUS))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let end_target = (idx + n.len() + SNIPPET_RADIUS).min(haystack.len());
        let end = haystack
            .char_indices()
            .find(|(i, _)| *i >= end_target)
            .map(|(i, _)| i)
            .unwrap_or(haystack.len());
        let prefix = if start > 0 { "…" } else { "" };
        let suffix = if end < haystack.len() { "…" } else { "" };
        let raw = &haystack[start..end];
        return Some(format!(
            "{prefix}{}{suffix}",
            raw.split_whitespace().collect::<Vec<_>>().join(" ")
        ));
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_snippet_matches_case_insensitive_by_default() {
        assert!(find_snippet("Concrete Slab", "concrete", false, false).is_some());
    }

    #[test]
    fn find_snippet_case_sensitive_rejects_wrong_case() {
        assert!(find_snippet("Concrete Slab", "concrete", true, false).is_none());
        assert!(find_snippet("Concrete Slab", "Concrete", true, false).is_some());
    }

    #[test]
    fn find_snippet_whole_word_rejects_substring_match() {
        assert!(find_snippet("reinforced concrete works", "crete", false, true).is_none());
        assert!(find_snippet("reinforced concrete works", "crete", false, false).is_some());
        assert!(find_snippet("reinforced concrete works", "concrete", false, true).is_some());
    }

    #[test]
    fn find_snippet_empty_query_returns_none() {
        assert!(find_snippet("some text", "", false, false).is_none());
    }

    #[test]
    fn find_snippet_no_match_returns_none() {
        assert!(find_snippet("some text", "xyzzy", false, false).is_none());
    }

    #[test]
    fn find_snippet_truncates_long_context_with_ellipsis() {
        let long = "a".repeat(200) + "needle" + &"b".repeat(200);
        let snippet = find_snippet(&long, "needle", false, false).unwrap();
        assert!(
            snippet.starts_with('…'),
            "expected leading ellipsis: {snippet}"
        );
        assert!(
            snippet.ends_with('…'),
            "expected trailing ellipsis: {snippet}"
        );
        assert!(snippet.contains("needle"));
    }

    #[tokio::test]
    async fn search_paths_skips_missing_files_without_failing_the_batch() {
        let hits = search_paths(
            vec!["/definitely/does/not/exist.pdf".to_string()],
            "anything".to_string(),
            false,
            false,
        )
        .await
        .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_paths_empty_query_returns_empty_without_touching_disk() {
        let hits = search_paths(
            vec!["/definitely/does/not/exist.pdf".to_string()],
            "".to_string(),
            false,
            false,
        )
        .await
        .unwrap();
        assert!(hits.is_empty());
    }
}
