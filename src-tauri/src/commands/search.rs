//! Tauri IPC commands — folder full-text search (M4 S4).
//!
//! Three commands:
//!   open_folder_index   — create/open the Tantivy index for a folder and start
//!                         the background indexer + file watcher.
//!   search_folder       — run a query against the active index.
//!   folder_index_status — poll the indexing state / hit counts.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};

use crate::{
    AppState,
    search::{FolderIndex, FolderSearchHit, IndexState, IndexStatus, indexer},
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
pub async fn folder_index_status(
    state: State<'_, AppState>,
) -> Result<IndexStatus, String> {
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
