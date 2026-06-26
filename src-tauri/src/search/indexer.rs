//! Background PDF indexer — lopdf text extraction + `notify` file watcher.
//!
//! `index_folder_blocking` is designed to run on a dedicated OS thread
//! (via `std::thread::spawn`).  It performs an initial full index of all PDFs
//! in the folder, then sets up a file-system watcher for incremental updates.
//!
//! The function exits when the caller drops all external `FolderIndex` clones
//! (detected via `FolderIndex::alive()`), so the background thread cleans up
//! within ~1 s of the parent command opening a different folder.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use notify::{Event, EventKind, RecursiveMode, Watcher};

use super::{FolderIndex, IndexState};

// ---------------------------------------------------------------------------
// Text extraction
// ---------------------------------------------------------------------------

/// Extract per-page text from a PDF using lopdf.
///
/// Returns a `Vec<(page_number, text)>` where page_number is 1-based (matching
/// the PDF page numbering returned by `lopdf::Document::get_pages()`).
/// Pages that produce errors are silently skipped so a damaged page does not
/// abort indexing of the whole file.
pub fn extract_pdf_text(path: &Path) -> anyhow::Result<Vec<(u64, String)>> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| anyhow::anyhow!("lopdf load {:?}: {}", path, e))?;

    let page_map = doc.get_pages(); // BTreeMap<u32, ObjectId>, 1-based
    let mut result = Vec::with_capacity(page_map.len());

    for page_num in page_map.keys() {
        let text = doc.extract_text(&[*page_num]).unwrap_or_default();
        result.push((*page_num as u64, text));
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Folder scan
// ---------------------------------------------------------------------------

/// Find all PDF files directly inside `folder_path` (non-recursive for v1).
pub fn find_pdfs(folder_path: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(folder_path) {
        Err(_) => Vec::new(),
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("pdf"))
                        .unwrap_or(false)
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Background indexer entry point
// ---------------------------------------------------------------------------

/// Index all PDFs in `folder_path`, then watch for incremental changes.
///
/// Intended to run on a dedicated OS thread (via `std::thread::spawn`).
/// Returns when:
/// - `index.alive()` returns `false` (the AppState replaced the index), or
/// - The watcher cannot be set up (non-fatal: initial index still complete).
pub fn index_folder_blocking(index: FolderIndex, folder_path: PathBuf) {
    // -----------------------------------------------------------------------
    // Phase 1 — initial full index
    // -----------------------------------------------------------------------
    let pdfs = find_pdfs(&folder_path);
    let total = pdfs.len();

    for (i, pdf_path) in pdfs.iter().enumerate() {
        if !index.alive() {
            return;
        }

        let file_name = pdf_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        index.set_state(IndexState::Indexing {
            current_file: file_name,
            progress: i as f32 / total.max(1) as f32,
        });

        match extract_pdf_text(pdf_path) {
            Ok(pages) => {
                let path_str = pdf_path.display().to_string();
                if let Err(e) = index.index_pages(&path_str, &pages, "lopdf") {
                    log::warn!("folder-index: failed to index {:?}: {e}", pdf_path);
                }
            }
            Err(e) => {
                log::warn!("folder-index: text extraction failed for {:?}: {e}", pdf_path);
            }
        }
    }

    if !index.alive() {
        return;
    }

    index.set_state(IndexState::Idle);

    // -----------------------------------------------------------------------
    // Phase 2 — file watcher for incremental updates
    // -----------------------------------------------------------------------
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<Event>| {
        let _ = tx.send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("folder-index: could not create file watcher: {e}");
            return;
        }
    };

    if let Err(e) = watcher.watch(&folder_path, RecursiveMode::NonRecursive) {
        log::warn!("folder-index: could not watch {:?}: {e}", folder_path);
        return;
    }

    log::info!("folder-index: watcher running on {:?}", folder_path);

    // Event loop — runs until the index is abandoned or the channel closes.
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(event)) => {
                if index.alive() {
                    handle_event(&index, event);
                }
            }
            Ok(Err(e)) => log::warn!("folder-index: watcher error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !index.alive() {
                    log::info!("folder-index: index abandoned, stopping watcher");
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // `watcher` drops here, which also drops the notify internal thread.
}

// ---------------------------------------------------------------------------
// Watcher event handler
// ---------------------------------------------------------------------------

fn is_pdf(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

fn handle_event(index: &FolderIndex, event: Event) {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in event.paths.iter().filter(|p| p.is_file() && is_pdf(p)) {
                match extract_pdf_text(path) {
                    Ok(pages) => {
                        let path_str = path.display().to_string();
                        if let Err(e) = index.index_pages(&path_str, &pages, "lopdf") {
                            log::warn!("folder-index: re-index {:?} failed: {e}", path);
                        }
                    }
                    Err(e) => {
                        log::warn!("folder-index: extract {:?} failed: {e}", path);
                    }
                }
            }
        }
        EventKind::Remove(_) => {
            for path in event.paths.iter().filter(|p| is_pdf(p)) {
                let path_str = path.display().to_string();
                if let Err(e) = index.delete_document(&path_str) {
                    log::warn!("folder-index: delete {:?} from index failed: {e}", path);
                }
            }
        }
        _ => {}
    }
}
