//! Most-Recently-Used document list — per-app persistence (spec §15).
//!
//! Stores a capped, newest-first list of recently opened PDFs so the user can
//! quickly reopen previous work across sessions.
//!
//! ## Storage
//! One JSON file: `<app-data-dir>/recent-docs.json`.
//! Atomic write (temp + rename) to survive a crash mid-write.
//!
//! ## Invariants (enforced by tests)
//! - Inserting a path already in the list moves it to the top (no duplicates).
//! - The list is capped at `MAX_RECENT` entries; oldest entries are evicted.
//! - Newest entry is always at index 0 after upsert.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub const MAX_RECENT: usize = 20;

/// A single entry in the MRU list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MruEntry {
    /// Absolute path to the PDF file.
    pub path: String,
    /// Filename component (e.g. `"floor-plan.pdf"`).
    pub file_name: String,
    /// RFC3339 timestamp of the last open.
    pub last_opened: String,
    /// Page count at time of open (cheaply available from `DocumentInfo`).
    pub page_count: Option<u32>,
}

// ---------------------------------------------------------------------------
// Pure list logic (testable without filesystem)
// ---------------------------------------------------------------------------

/// Insert or move `entry` to the top of `list`, capping at `max_items`.
///
/// - If `entry.path` already exists in the list it is removed first (dedup).
/// - The updated entry is prepended (index 0).
/// - Excess tail entries are dropped to enforce the cap.
pub fn upsert_mru(list: &mut Vec<MruEntry>, entry: MruEntry, max_items: usize) {
    // Remove any existing entry with the same path (case-sensitive match).
    list.retain(|e| e.path != entry.path);
    // Prepend — newest first.
    list.insert(0, entry);
    // Cap.
    if list.len() > max_items {
        list.truncate(max_items);
    }
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

/// Absolute path to the MRU JSON file inside `data_dir`.
pub fn mru_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("recent-docs.json")
}

/// Load the MRU list from `data_dir/recent-docs.json`.
///
/// Returns an empty list if the file does not exist yet.
/// Returns an IO error only for genuine read or parse failures.
pub fn load_recent_docs(data_dir: &Path) -> io::Result<Vec<MruEntry>> {
    let path = mru_file_path(data_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(&path)?;
    serde_json::from_slice::<Vec<MruEntry>>(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Save the MRU list to `data_dir/recent-docs.json` atomically (temp + rename).
///
/// Creates `data_dir` if it does not yet exist.
pub fn save_recent_docs(data_dir: &Path, entries: &[MruEntry]) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let dest = mru_file_path(data_dir);
    let tmp = data_dir.join(format!(
        ".recent-docs-{}-{}.tmp",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let json = serde_json::to_vec_pretty(entries)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let write_result = fs::write(&tmp, &json);
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
        return write_result;
    }
    if let Err(e) = fs::rename(&tmp, &dest) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Build an `MruEntry` from its components.
pub fn make_mru_entry(path: &str, page_count: Option<u32>) -> MruEntry {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_owned();
    MruEntry {
        path: path.to_owned(),
        file_name,
        last_opened: Utc::now().to_rfc3339(),
        page_count,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(path: &str) -> MruEntry {
        make_mru_entry(path, None)
    }

    fn entry_with_pages(path: &str, pages: u32) -> MruEntry {
        make_mru_entry(path, Some(pages))
    }

    // ---- Pure list logic -------------------------------------------------

    #[test]
    fn insert_moves_to_top_on_new_entry() {
        let mut list: Vec<MruEntry> = vec![entry("/a.pdf"), entry("/b.pdf")];
        upsert_mru(&mut list, entry("/c.pdf"), MAX_RECENT);
        assert_eq!(list[0].path, "/c.pdf");
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn insert_existing_path_moves_to_top_no_duplicate() {
        let mut list: Vec<MruEntry> = vec![entry("/a.pdf"), entry("/b.pdf"), entry("/c.pdf")];
        upsert_mru(&mut list, entry("/b.pdf"), MAX_RECENT);
        // b is now at the top, only 3 entries total.
        assert_eq!(list[0].path, "/b.pdf");
        assert_eq!(list.len(), 3, "no duplicate should be created");
        // a and c still present.
        assert!(list.iter().any(|e| e.path == "/a.pdf"));
        assert!(list.iter().any(|e| e.path == "/c.pdf"));
    }

    #[test]
    fn insert_existing_first_entry_leaves_length_unchanged() {
        let mut list: Vec<MruEntry> = vec![entry("/a.pdf"), entry("/b.pdf")];
        upsert_mru(&mut list, entry("/a.pdf"), MAX_RECENT);
        assert_eq!(list[0].path, "/a.pdf");
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn cap_evicts_oldest() {
        let max = 3;
        let mut list: Vec<MruEntry> = Vec::new();
        for i in 0..5u32 {
            upsert_mru(&mut list, entry(&format!("/{i}.pdf")), max);
        }
        // List should have 3 entries, newest first.
        assert_eq!(list.len(), max);
        assert_eq!(list[0].path, "/4.pdf");
        assert_eq!(list[1].path, "/3.pdf");
        assert_eq!(list[2].path, "/2.pdf");
        // Oldest (/0.pdf, /1.pdf) must be evicted.
        assert!(!list.iter().any(|e| e.path == "/0.pdf"));
        assert!(!list.iter().any(|e| e.path == "/1.pdf"));
    }

    #[test]
    fn page_count_stored_on_entry() {
        let mut list: Vec<MruEntry> = Vec::new();
        upsert_mru(&mut list, entry_with_pages("/plan.pdf", 42), MAX_RECENT);
        assert_eq!(list[0].page_count, Some(42));
    }

    #[test]
    fn file_name_extracted_from_path() {
        let e = entry("/some/dir/floor-plan.pdf");
        assert_eq!(e.file_name, "floor-plan.pdf");
    }

    #[test]
    fn update_refreshes_last_opened_and_page_count() {
        let mut list: Vec<MruEntry> = vec![entry_with_pages("/a.pdf", 10)];
        let older_ts = list[0].last_opened.clone();
        // Re-open with updated page count (e.g. after edit).
        // Sleep briefly so timestamps differ (chrono precision).
        std::thread::sleep(std::time::Duration::from_millis(2));
        upsert_mru(&mut list, entry_with_pages("/a.pdf", 12), MAX_RECENT);
        assert_eq!(list[0].page_count, Some(12));
        // Timestamp should have been refreshed.
        assert_ne!(list[0].last_opened, older_ts);
    }

    // ---- Filesystem round-trip -------------------------------------------

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let entries = vec![
            entry_with_pages("/project/plans.pdf", 8),
            entry("/archive/spec.pdf"),
        ];
        save_recent_docs(dir.path(), &entries).unwrap();

        let loaded = load_recent_docs(dir.path()).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].path, "/project/plans.pdf");
        assert_eq!(loaded[0].page_count, Some(8));
        assert_eq!(loaded[1].path, "/archive/spec.pdf");
    }

    #[test]
    fn load_returns_empty_when_file_absent() {
        let dir = tempdir().unwrap();
        let loaded = load_recent_docs(dir.path()).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn save_creates_data_dir_if_absent() {
        let root = tempdir().unwrap();
        let data_dir = root.path().join("app-data").join("redline");
        // data_dir does not exist yet.
        assert!(!data_dir.exists());
        save_recent_docs(&data_dir, &[entry("/a.pdf")]).unwrap();
        assert!(data_dir.exists());
        assert!(mru_file_path(&data_dir).exists());
    }
}
