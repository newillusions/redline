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
    /// Password used to open this document via PDFium, if it was encrypted.
    /// Session-only (never persisted to disk) - used to decrypt the same file's
    /// existing annotations via lopdf in `load_markups` without re-prompting.
    pub password: Option<String>,
    pub markups: Vec<Markup>,
    pub loaded: bool,
    pub saving: bool,
    /// True once `add`/`update`/`delete` has been called since the doc was
    /// registered or since [`MarkupStore::clear_dirty`] last ran (MCP server design,
    /// Phase 2a `list_open_documents`/`close_document` - the same "unsaved changes
    /// since open/last save" concept the frontend's own `MarkupStore.dirty`
    /// ($state, `markup-store.svelte.ts`) tracks, mirrored here so the RPC bridge
    /// (which has no visibility into Svelte state) can answer without a new
    /// frontend->backend push for every edit. `seed_loaded` deliberately does NOT
    /// touch this flag - merging a document's pre-existing on-disk annotations at
    /// open time is not an edit. Cleared by `clear_dirty`, called from both
    /// `commands::document::save_inner` (save_document/save_document_as) and
    /// `commands::document::apply_page_edit` (rotate/delete/reorder/insert pages,
    /// and - via `commands::docops` - flatten/optimize/redact), since all of those
    /// flush the current markup state to disk exactly like a save does.
    pub dirty: bool,
}

/// One open document as reported by [`MarkupStore::list_open`] - the data
/// `rpc::tools::list_open_documents` needs that doesn't require a doc_id to look up
/// (MCP server design, Phase 2a).
#[derive(Debug, Clone)]
pub struct OpenDocEntry {
    pub doc_id: String,
    pub path: PathBuf,
    pub dirty: bool,
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
    pub fn register(&self, doc_id: &str, path: PathBuf, password: Option<String>) {
        self.docs.lock().unwrap().insert(
            doc_id.to_string(),
            DocEntry {
                path,
                password,
                markups: Vec::new(),
                loaded: false,
                saving: false,
                dirty: false,
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

    /// The doc_id of the currently-open document registered at exactly `path`, if any
    /// (`open_document` MCP tool's already-open dedup, Phase 2a - the design's "returns
    /// the existing doc_id if that path is already open" requirement; without this, an
    /// MCP-driven open of an already-open path would register a second, independent
    /// PDFium handle under a fresh doc_id rather than reusing the one the GUI or an
    /// earlier MCP call already has open).
    pub fn find_by_path(&self, path: &std::path::Path) -> Option<String> {
        self.docs
            .lock()
            .unwrap()
            .iter()
            .find(|(_, e)| e.path == path)
            .map(|(doc_id, _)| doc_id.clone())
    }

    /// Every currently-open document's doc_id/path/dirty state (`list_open_documents`
    /// MCP tool, Phase 2a). Order is unspecified (backed by a `HashMap`).
    pub fn list_open(&self) -> Vec<OpenDocEntry> {
        self.docs
            .lock()
            .unwrap()
            .iter()
            .map(|(doc_id, e)| OpenDocEntry {
                doc_id: doc_id.clone(),
                path: e.path.clone(),
                dirty: e.dirty,
            })
            .collect()
    }

    /// True if markups have changed since this doc was opened or since the last
    /// [`Self::clear_dirty`] - see the field doc comment on [`DocEntry::dirty`].
    /// `false` for an unknown doc_id, matching [`Self::is_loaded`]'s convention.
    pub fn is_dirty(&self, doc_id: &str) -> bool {
        self.docs
            .lock()
            .unwrap()
            .get(doc_id)
            .map(|e| e.dirty)
            .unwrap_or(false)
    }

    /// Mark a doc clean (call after a successful save or any other flush of the
    /// current markup state to disk - see [`DocEntry::dirty`]'s doc comment for the
    /// full list of call sites). No-op for an unknown doc_id (the entry may have
    /// already been removed), matching [`Self::end_save`]'s convention.
    pub fn clear_dirty(&self, doc_id: &str) {
        if let Some(e) = self.docs.lock().unwrap().get_mut(doc_id) {
            e.dirty = false;
        }
    }

    /// Password this doc was opened with, if it required one. `None` for both
    /// "unknown doc_id" and "doc is open but wasn't encrypted" - callers that need
    /// to tell those apart should check `path()` first.
    pub fn password(&self, doc_id: &str) -> Option<String> {
        self.docs
            .lock()
            .unwrap()
            .get(doc_id)
            .and_then(|e| e.password.clone())
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
        e.dirty = true;
        Ok(())
    }

    /// Replace a markup by id. Errors on unknown doc or absent id, or if the existing
    /// markup is locked (MCP server design §4 - the shared lock guard; this is the one
    /// choke point both the GUI's `update_markup` Tauri command and the MCP bridge call
    /// through, so refusing here fixes the gap for both surfaces from one change).
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
        crate::markup::check_not_locked(slot).map_err(|e| e.to_string())?;
        *slot = m;
        e.dirty = true;
        Ok(())
    }

    /// Remove a markup by id. Errors on unknown doc or absent id, or if the markup is
    /// locked (MCP server design §4 - see the doc comment on [`Self::update`], the same
    /// guard applies here).
    pub fn delete(&self, doc_id: &str, id: Uuid) -> Result<(), String> {
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        let existing = e
            .markups
            .iter()
            .find(|x| x.id() == id)
            .ok_or_else(|| format!("unknown markup id {id}"))?;
        crate::markup::check_not_locked(existing).map_err(|e| e.to_string())?;
        e.markups.retain(|x| x.id() != id);
        e.dirty = true;
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
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = markup();
        let id = m.id();
        s.add("d1", m).unwrap();
        let got = s.list("d1").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), id);
        assert_eq!(s.path("d1"), Some(PathBuf::from("/tmp/a.pdf")));
    }

    #[test]
    fn password_none_when_registered_without_one() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        assert_eq!(s.password("d1"), None);
    }

    #[test]
    fn password_roundtrips_when_registered_with_one() {
        let s = MarkupStore::default();
        s.register(
            "d1",
            PathBuf::from("/tmp/a.pdf"),
            Some("redline-pw".to_string()),
        );
        assert_eq!(s.password("d1"), Some("redline-pw".to_string()));
    }

    #[test]
    fn password_none_for_unknown_doc() {
        let s = MarkupStore::default();
        assert_eq!(s.password("nope"), None);
    }

    #[test]
    fn duplicate_id_rejected() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = markup();
        s.add("d1", m.clone()).unwrap();
        assert!(s.add("d1", m).is_err());
    }

    #[test]
    fn unknown_doc_errors_and_remove_forgets() {
        let s = MarkupStore::default();
        assert!(s.list("nope").is_err());
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        s.remove("d1");
        assert!(s.list("d1").is_err());
    }

    #[test]
    fn seed_loaded_merges_and_marks_loaded() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);

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
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);

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

    // --- MCP server design §4: lock guard wired into update/delete (the single choke
    // point both the GUI's update_markup/delete_markup commands and the MCP bridge call
    // through). Four cases per the design's own rollout gate (§6): a locked foreign
    // (Bluebeam-authored) annotation, a locked redline-authored one, an unlocked control,
    // and LockedContents-only. ---

    fn locked_markup(flags: i32, origin: crate::markup::Origin) -> Markup {
        let mut m = markup();
        m.annot_flags = flags;
        m.audit.origin = origin;
        m
    }

    #[test]
    fn update_refuses_locked_foreign_bluebeam_authored_markup() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = locked_markup(0x80, crate::markup::Origin::FieldApp);
        let id = m.id();
        s.add("d1", m.clone()).unwrap();

        let mut edited = m;
        edited.contents = Some("attempted edit".into());
        let err = s.update("d1", edited).unwrap_err();
        assert!(err.contains("markup_locked"), "got: {err}");
        assert!(err.contains(&id.to_string()));

        // Refused, not silently applied - the store must still hold the original.
        let stored = s.list("d1").unwrap();
        assert_eq!(stored[0].contents, None);
    }

    #[test]
    fn update_refuses_locked_redline_authored_markup_too() {
        // Origin-agnostic: a markup redline itself created and locked is refused
        // exactly like a foreign one - proves the guard doesn't special-case origin.
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = locked_markup(0x80, crate::markup::Origin::Desktop);
        s.add("d1", m.clone()).unwrap();

        let mut edited = m;
        edited.contents = Some("attempted edit".into());
        assert!(s.update("d1", edited).is_err());
    }

    #[test]
    fn update_allows_unlocked_control_case() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = markup(); // default annot_flags = 4 (Print), not locked
        s.add("d1", m.clone()).unwrap();

        let mut edited = m;
        edited.contents = Some("a real edit".into());
        s.update("d1", edited).unwrap();
        assert_eq!(
            s.list("d1").unwrap()[0].contents,
            Some("a real edit".into())
        );
    }

    #[test]
    fn update_refuses_locked_contents_only_markup() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = locked_markup(0x200, crate::markup::Origin::Desktop);
        s.add("d1", m.clone()).unwrap();

        let mut edited = m;
        edited.contents = Some("attempted edit".into());
        let err = s.update("d1", edited).unwrap_err();
        assert!(err.contains("LockedContents"), "got: {err}");
    }

    #[test]
    fn delete_refuses_locked_foreign_bluebeam_authored_markup() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = locked_markup(0x80, crate::markup::Origin::FieldApp);
        let id = m.id();
        s.add("d1", m).unwrap();

        let err = s.delete("d1", id).unwrap_err();
        assert!(err.contains("markup_locked"), "got: {err}");
        assert_eq!(s.list("d1").unwrap().len(), 1, "must not be deleted");
    }

    #[test]
    fn delete_refuses_locked_redline_authored_markup_too() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = locked_markup(0x80, crate::markup::Origin::Desktop);
        let id = m.id();
        s.add("d1", m).unwrap();

        assert!(s.delete("d1", id).is_err());
        assert_eq!(s.list("d1").unwrap().len(), 1);
    }

    #[test]
    fn delete_allows_unlocked_control_case() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = markup();
        let id = m.id();
        s.add("d1", m).unwrap();

        s.delete("d1", id).unwrap();
        assert!(s.list("d1").unwrap().is_empty());
    }

    #[test]
    fn delete_refuses_locked_contents_only_markup() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = locked_markup(0x200, crate::markup::Origin::Desktop);
        let id = m.id();
        s.add("d1", m).unwrap();

        let err = s.delete("d1", id).unwrap_err();
        assert!(err.contains("LockedContents"), "got: {err}");
    }

    #[test]
    fn update_and_delete_unknown_markup_id_still_report_unknown_not_locked() {
        // A not-found error must not be confused with a lock refusal - distinct error
        // shapes matter to a calling agent.
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let err = s.delete("d1", uuid::Uuid::new_v4()).unwrap_err();
        assert!(err.contains("unknown markup id"), "got: {err}");
        assert!(!err.contains("markup_locked"));
    }

    #[test]
    fn update_replaces_markup_by_id() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
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
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        // markup() not added -> its id is absent
        assert!(s.update("d1", markup()).is_err());
        // unknown doc also errors
        assert!(s.update("nope", markup()).is_err());
    }

    #[test]
    fn delete_removes_by_id() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = markup();
        let id = m.id();
        s.add("d1", m).unwrap();
        s.delete("d1", id).unwrap();
        assert_eq!(s.list("d1").unwrap().len(), 0);
    }

    #[test]
    fn delete_unknown_id_or_doc_errors() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
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

    // -----------------------------------------------------------------------
    // dirty tracking + list_open + find_by_path (MCP server design, Phase 2a)
    // -----------------------------------------------------------------------

    #[test]
    fn fresh_doc_is_not_dirty() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        assert!(!s.is_dirty("d1"));
    }

    #[test]
    fn unknown_doc_is_not_dirty() {
        let s = MarkupStore::default();
        assert!(!s.is_dirty("nope"));
    }

    #[test]
    fn add_marks_dirty() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        s.add("d1", markup()).unwrap();
        assert!(s.is_dirty("d1"));
    }

    #[test]
    fn update_marks_dirty() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = markup();
        s.add("d1", m.clone()).unwrap();
        s.clear_dirty("d1");
        assert!(!s.is_dirty("d1"), "precondition: clean after clear_dirty");
        s.update("d1", m).unwrap();
        assert!(s.is_dirty("d1"), "update must mark the doc dirty again");
    }

    #[test]
    fn delete_marks_dirty() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let m = markup();
        let id = m.id();
        s.add("d1", m).unwrap();
        s.clear_dirty("d1");
        s.delete("d1", id).unwrap();
        assert!(s.is_dirty("d1"), "delete must mark the doc dirty again");
    }

    #[test]
    fn seed_loaded_does_not_mark_dirty() {
        // Merging a document's own pre-existing on-disk annotations at open time is
        // not an edit - see DocEntry::dirty's doc comment.
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        s.seed_loaded("d1", vec![markup()]).unwrap();
        assert!(!s.is_dirty("d1"));
    }

    #[test]
    fn clear_dirty_resets_flag_and_is_a_noop_for_unknown_doc() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        s.add("d1", markup()).unwrap();
        assert!(s.is_dirty("d1"));
        s.clear_dirty("d1");
        assert!(!s.is_dirty("d1"));
        // Unknown doc: must not panic.
        s.clear_dirty("nope");
    }

    #[test]
    fn find_by_path_matches_a_registered_doc() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        s.register("d2", PathBuf::from("/tmp/b.pdf"), None);
        assert_eq!(
            s.find_by_path(&PathBuf::from("/tmp/b.pdf")),
            Some("d2".to_string())
        );
    }

    #[test]
    fn find_by_path_none_for_unopened_path() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        assert_eq!(s.find_by_path(&PathBuf::from("/tmp/nope.pdf")), None);
    }

    #[test]
    fn list_open_reports_every_registered_doc_with_its_dirty_state() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        s.register("d2", PathBuf::from("/tmp/b.pdf"), None);
        s.add("d2", markup()).unwrap();

        let mut open = s.list_open();
        open.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));

        assert_eq!(open.len(), 2);
        assert_eq!(open[0].doc_id, "d1");
        assert_eq!(open[0].path, PathBuf::from("/tmp/a.pdf"));
        assert!(!open[0].dirty);
        assert_eq!(open[1].doc_id, "d2");
        assert!(open[1].dirty);
    }

    #[test]
    fn list_open_empty_when_nothing_registered() {
        let s = MarkupStore::default();
        assert!(s.list_open().is_empty());
    }

    #[test]
    fn remove_drops_the_doc_from_list_open() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        assert_eq!(s.list_open().len(), 1);
        s.remove("d1");
        assert!(s.list_open().is_empty());
    }
}
