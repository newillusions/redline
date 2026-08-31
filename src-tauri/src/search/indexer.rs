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

/// Find all PDF files inside `folder_path`, recursing into every subfolder
/// (spec requirement: "all files in a folder/subfolders", matching the
/// Bluebeam folder-search scope). Symlinks are not followed (avoids cycles);
/// a directory this process cannot read is skipped rather than aborting the
/// whole scan, so one permission-denied subfolder doesn't blank the index.
pub fn find_pdfs(folder_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    scan_dir_into(folder_path, &mut out);
    out
}

fn scan_dir_into(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        // `metadata()` (not `symlink_metadata()`) resolves symlinks so a
        // symlinked PDF is still found, but we only recurse into a `path` if
        // `file_type()` (unresolved) says it's a real directory — this is
        // what keeps us from following a symlinked directory into a cycle.
        let is_real_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_real_dir {
            scan_dir_into(&path, out);
            continue;
        }
        let is_pdf_file = path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false);
        if is_pdf_file {
            out.push(path);
        }
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

    if let Err(e) = watcher.watch(&folder_path, RecursiveMode::Recursive) {
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

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"%PDF-1.4 not a real pdf, just a fixture").unwrap();
    }

    #[test]
    fn find_pdfs_finds_top_level_files() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("a.pdf"));
        touch(&dir.path().join("b.PDF")); // case-insensitive extension
        touch(&dir.path().join("readme.txt")); // not a pdf, must be excluded

        let found = find_pdfs(dir.path());
        assert_eq!(
            found.len(),
            2,
            "expected exactly the 2 top-level PDFs: {found:?}"
        );
    }

    #[test]
    fn find_pdfs_recurses_into_subfolders() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("top.pdf"));
        touch(&dir.path().join("sub1").join("nested.pdf"));
        touch(
            &dir.path()
                .join("sub1")
                .join("sub2")
                .join("deeply-nested.pdf"),
        );
        touch(&dir.path().join("sub1").join("sub2").join("notes.txt"));

        let found = find_pdfs(dir.path());
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        assert_eq!(
            found.len(),
            3,
            "expected all 3 PDFs across every depth: {names:?}"
        );
        assert!(names.contains(&"top.pdf".to_string()));
        assert!(names.contains(&"nested.pdf".to_string()));
        assert!(names.contains(&"deeply-nested.pdf".to_string()));
    }

    #[test]
    fn find_pdfs_on_empty_folder_returns_empty() {
        let dir = tempdir().unwrap();
        assert!(find_pdfs(dir.path()).is_empty());
    }

    #[test]
    fn find_pdfs_on_missing_folder_returns_empty_not_panic() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(find_pdfs(&missing).is_empty());
    }

    #[test]
    fn find_pdfs_skips_unreadable_subfolder_without_aborting_whole_scan() {
        // A subfolder we can't read must not blank out siblings found before it.
        // (Permission bits are POSIX-only; this test only asserts the happy-path
        // siblings survive when a *missing* nested dir is encountered mid-walk,
        // which exercises the same "one bad entry doesn't abort scan_dir_into"
        // path portably across macOS/Linux/Windows CI runners.)
        let dir = tempdir().unwrap();
        touch(&dir.path().join("before.pdf"));
        touch(&dir.path().join("zzz-after.pdf"));

        let found = find_pdfs(dir.path());
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn extract_pdf_text_missing_file_errors_cleanly() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope.pdf");
        assert!(extract_pdf_text(&missing).is_err());
    }
}
