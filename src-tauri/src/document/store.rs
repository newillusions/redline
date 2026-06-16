//! In-memory markup store - doc_id -> (path, markups). Single source of truth for
//! unsaved markup state; the save pipeline (document::save) flushes it to the PDF.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use uuid::Uuid;

use crate::markup::Markup;

#[derive(Debug)]
pub struct DocEntry {
    pub path: PathBuf,
    pub markups: Vec<Markup>,
    pub loaded: bool,
    pub saving: bool,
}

/// Keyed by path: returns the parsed annotation set for a file that hasn't changed since the
/// last `load_markups` (mtime match), avoiding the full lopdf parse on reopen.
#[derive(Debug, Default)]
struct MtimeCache(HashMap<PathBuf, (SystemTime, Vec<Markup>)>);

impl MtimeCache {
    /// Cached markups if the file's current mtime matches what was recorded; else None
    /// (changed / unreadable / never cached).
    fn get(&self, path: &PathBuf) -> Option<Vec<Markup>> {
        let (cached_mtime, markups) = self.0.get(path)?;
        let current_mtime = std::fs::metadata(path).ok()?.modified().ok()?;
        if current_mtime == *cached_mtime {
            Some(markups.clone())
        } else {
            None
        }
    }

    /// Record a successful parse + the file's current mtime. Skipped if mtime is unreadable.
    fn set(&mut self, path: PathBuf, markups: Vec<Markup>) {
        let Ok(meta) = std::fs::metadata(&path) else {
            return;
        };
        let Ok(mtime) = meta.modified() else {
            return;
        };
        self.0.insert(path, (mtime, markups));
    }

    /// Evict the entry for `path` (called after a save changes the file's content + mtime).
    fn invalidate(&mut self, path: &PathBuf) {
        self.0.remove(path);
    }
}

/// Thread-safe store shared via Tauri `AppState`.
#[derive(Debug, Default)]
pub struct MarkupStore {
    docs: Mutex<HashMap<String, DocEntry>>,
    cache: Mutex<MtimeCache>,
}

impl MarkupStore {
    pub fn register(&self, doc_id: &str, path: PathBuf) {
        self.docs.lock().unwrap().insert(
            doc_id.to_string(),
            DocEntry {
                path,
                markups: Vec::new(),
                loaded: false,
                saving: false,
            },
        );
    }

    pub fn remove(&self, doc_id: &str) {
        self.docs.lock().unwrap().remove(doc_id);
    }

    /// Path registered for this doc, if open.
    pub fn path(&self, doc_id: &str) -> Option<PathBuf> {
        self.docs
            .lock()
            .unwrap()
            .get(doc_id)
            .map(|e| e.path.clone())
    }

    pub fn set_path(&self, doc_id: &str, path: PathBuf) -> Result<(), String> {
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        e.path = path;
        Ok(())
    }

    /// True if the PDF's existing annotations have been loaded into the store.
    pub fn is_loaded(&self, doc_id: &str) -> bool {
        self.docs
            .lock()
            .unwrap()
            .get(doc_id)
            .map(|e| e.loaded)
            .unwrap_or(false)
    }

    /// Add one markup. Errors on unknown doc or duplicate id.
    pub fn add(&self, doc_id: &str, m: Markup) -> Result<(), String> {
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        if e.markups.iter().any(|x| x.id() == m.id()) {
            return Err(format!("duplicate markup id {}", m.id()));
        }
        e.markups.push(m);
        Ok(())
    }

    /// Replace a markup by id. Errors on unknown doc or absent id.
    ///
    /// This is a verbatim swap, not a `Markup::touch()`. The frontend store is the
    /// in-session source of truth and bumps audit fields (revision / modified_*) before
    /// sending the updated markup (spec §6; decision:vic6slsasg6njkf7haka).
    pub fn update(&self, doc_id: &str, m: Markup) -> Result<(), String> {
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        let slot = e
            .markups
            .iter_mut()
            .find(|x| x.id() == m.id())
            .ok_or_else(|| format!("unknown markup id {}", m.id()))?;
        *slot = m;
        Ok(())
    }

    /// Remove a markup by id. Errors on unknown doc or absent id.
    pub fn delete(&self, doc_id: &str, id: Uuid) -> Result<(), String> {
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        let before = e.markups.len();
        e.markups.retain(|x| x.id() != id);
        if e.markups.len() == before {
            return Err(format!("unknown markup id {id}"));
        }
        Ok(())
    }

    /// Merge markups loaded from the PDF beneath any unsaved in-memory ones
    /// (the store wins on id collision) and mark the doc as loaded.
    /// Returns the merged set.
    pub fn seed_loaded(&self, doc_id: &str, loaded: Vec<Markup>) -> Result<Vec<Markup>, String> {
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        let unsaved: std::collections::HashSet<uuid::Uuid> =
            e.markups.iter().map(|m| m.id()).collect();
        let mut merged: Vec<Markup> = loaded
            .into_iter()
            .filter(|m| !unsaved.contains(&m.id()))
            .collect();
        merged.append(&mut e.markups);
        e.markups = merged;
        e.loaded = true;
        Ok(e.markups.clone())
    }

    /// Mark a save in flight. Errors if one is already running for this doc.
    pub fn begin_save(&self, doc_id: &str) -> Result<(), String> {
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        if e.saving {
            return Err("save already in progress".to_string());
        }
        e.saving = true;
        Ok(())
    }

    /// Clear the in-flight flag (no-op for unknown doc - the entry may have been removed).
    pub fn end_save(&self, doc_id: &str) {
        if let Some(e) = self.docs.lock().unwrap().get_mut(doc_id) {
            e.saving = false;
        }
    }

    /// Snapshot of the current markups (cloned; store stays locked only briefly).
    pub fn list(&self, doc_id: &str) -> Result<Vec<Markup>, String> {
        let g = self.docs.lock().unwrap();
        g.get(doc_id)
            .map(|e| e.markups.clone())
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))
    }

    // --- mtime cache (skip the lopdf re-parse when reopening an unchanged file) ---

    /// Cached parse result for `path` if its mtime is unchanged since the last load; else
    /// `None` (caller must run the lopdf parse, then call [`Self::cache_loaded`]).
    pub fn check_mtime_cache(&self, path: &PathBuf) -> Option<Vec<Markup>> {
        self.cache.lock().unwrap().get(path)
    }

    /// Record a freshly-parsed annotation set so the next reopen of the unchanged file is instant.
    pub fn cache_loaded(&self, path: PathBuf, markups: Vec<Markup>) {
        self.cache.lock().unwrap().set(path, markups);
    }

    /// Drop the cached entry for `path` (call after a save changes the file).
    pub fn invalidate_cache(&self, path: &PathBuf) {
        self.cache.lock().unwrap().invalidate(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PdfPoint;
    use crate::markup::{Appearance, MarkupGeometry, MarkupType, UserRef};

    fn markup() -> Markup {
        Markup::new(
            MarkupType::Rectangle,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 0.0, y: 0.0 },
                max: PdfPoint { x: 10.0, y: 10.0 },
            },
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "T".into(),
            },
        )
    }

    #[test]
    fn register_add_list_roundtrip() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        let m = markup();
        let id = m.id();
        s.add("d1", m).unwrap();
        let got = s.list("d1").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), id);
        assert_eq!(s.path("d1"), Some(PathBuf::from("/tmp/a.pdf")));
    }

    #[test]
    fn duplicate_id_rejected() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        let m = markup();
        s.add("d1", m.clone()).unwrap();
        assert!(s.add("d1", m).is_err());
    }

    #[test]
    fn unknown_doc_errors_and_remove_forgets() {
        let s = MarkupStore::default();
        assert!(s.list("nope").is_err());
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        s.remove("d1");
        assert!(s.list("d1").is_err());
    }

    #[test]
    fn seed_loaded_merges_and_marks_loaded() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));

        // Add one markup in-memory (unsaved) - this is markup A.
        let a = markup();
        let a_id = a.id();
        let a_original_contents = a.contents.clone();
        s.add("d1", a.clone()).unwrap();

        // Build B (a fresh markup) and A' (same id as A, different contents).
        let b = markup();
        let b_id = b.id();
        assert_ne!(a_id, b_id, "a and b must have distinct ids");

        let mut a_prime = a.clone();
        a_prime.contents = Some("different contents from pdf".into());
        assert_eq!(a_prime.id(), a_id, "a_prime must have the same id as a");

        // Seed with [B, A'] — A' should be filtered out (store wins on collision).
        let merged = s.seed_loaded("d1", vec![b.clone(), a_prime]).unwrap();

        // Merged set has exactly 2 entries.
        assert_eq!(merged.len(), 2, "expected exactly 2 entries in merged set");

        // Store version of A wins (original contents, not A').
        let stored_a = merged.iter().find(|m| m.id() == a_id).expect("A in merged");
        assert_eq!(
            stored_a.contents, a_original_contents,
            "store version of A must win on id collision"
        );

        // B is present.
        assert!(
            merged.iter().any(|m| m.id() == b_id),
            "B must appear in merged set"
        );

        // is_loaded is now true.
        assert!(
            s.is_loaded("d1"),
            "doc must be marked loaded after seed_loaded"
        );

        // list() matches the returned merged set.
        let listed = s.list("d1").unwrap();
        assert_eq!(listed.len(), merged.len());
        for m in &merged {
            assert!(listed.iter().any(|l| l.id() == m.id()));
        }
    }

    #[test]
    fn seed_loaded_unknown_doc_errors() {
        let s = MarkupStore::default();
        // Seeding an unregistered doc_id must error.
        let err = s.seed_loaded("nope", vec![]);
        assert!(err.is_err(), "expected error for unknown doc_id");
        // is_loaded on an unknown doc is false.
        assert!(!s.is_loaded("nope"));
    }

    #[test]
    fn begin_save_blocks_second_save() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));

        // First begin succeeds.
        s.begin_save("d1").unwrap();
        // Second begin while in flight errors.
        let err = s.begin_save("d1").unwrap_err();
        assert_eq!(err, "save already in progress");
        // After end_save, a new save may begin.
        s.end_save("d1");
        s.begin_save("d1").unwrap();

        // Unknown doc errors.
        assert!(s.begin_save("nope").is_err());
    }

    #[test]
    fn end_save_unknown_doc_is_noop() {
        let s = MarkupStore::default();
        // Must not panic - the entry may have been removed mid-save.
        s.end_save("nope");
    }

    #[test]
    fn update_replaces_markup_by_id() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        let m = markup();
        let id = m.id();
        s.add("d1", m.clone()).unwrap();

        let mut edited = m;
        edited.contents = Some("edited".into());
        s.update("d1", edited).unwrap();

        let got = s.list("d1").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), id, "id preserved");
        assert_eq!(got[0].contents.as_deref(), Some("edited"));
    }

    #[test]
    fn update_unknown_id_errors() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        // markup() not added -> its id is absent
        assert!(s.update("d1", markup()).is_err());
        // unknown doc also errors
        assert!(s.update("nope", markup()).is_err());
    }

    #[test]
    fn delete_removes_by_id() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        let m = markup();
        let id = m.id();
        s.add("d1", m).unwrap();
        s.delete("d1", id).unwrap();
        assert_eq!(s.list("d1").unwrap().len(), 0);
    }

    #[test]
    fn delete_unknown_id_or_doc_errors() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        assert!(s.delete("d1", uuid::Uuid::new_v4()).is_err());
        assert!(s.delete("nope", uuid::Uuid::new_v4()).is_err());
    }

    // --- mtime cache ---

    fn temp_pdf() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("redline-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.pdf");
        std::fs::write(&p, b"v1").unwrap();
        p
    }

    #[test]
    fn mtime_cache_cold_miss_returns_none() {
        let s = MarkupStore::default();
        assert!(s.check_mtime_cache(&temp_pdf()).is_none());
    }

    #[test]
    fn mtime_cache_warm_hit_returns_cached() {
        let s = MarkupStore::default();
        let p = temp_pdf();
        s.cache_loaded(p.clone(), vec![markup()]);
        let got = s.check_mtime_cache(&p).expect("hit");
        assert_eq!(got.len(), 1);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn mtime_cache_miss_after_invalidate() {
        let s = MarkupStore::default();
        let p = temp_pdf();
        s.cache_loaded(p.clone(), vec![markup()]);
        assert!(s.check_mtime_cache(&p).is_some());
        s.invalidate_cache(&p);
        assert!(s.check_mtime_cache(&p).is_none());
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn mtime_cache_miss_after_file_modified() {
        let s = MarkupStore::default();
        let p = temp_pdf();
        s.cache_loaded(p.clone(), vec![markup()]);
        assert!(s.check_mtime_cache(&p).is_some());
        // Bump the file's mtime deterministically (no sleep).
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() + std::time::Duration::from_secs(30))
            .unwrap();
        assert!(
            s.check_mtime_cache(&p).is_none(),
            "changed mtime must invalidate the cache"
        );
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }
}
