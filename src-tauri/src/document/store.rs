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
    /// Markups deleted this session that carried reader-owned fields (`source_ref`, `source_nm`, reply and
    /// state `nm`), keyed by markup id, so an undo (which re-sends the deleted markup as an
    /// add) gets back exactly the names the PDF reader produced for it - and only those:
    /// `add` never trusts names from its input. Holds only markups this store held, i.e.
    /// reader output for this document. Cleared when the document is closed or reloaded.
    pub tombstones: std::collections::HashMap<Uuid, Markup>,
    /// The source annotations (object id + `/NM`) of markups deleted since the document was
    /// loaded. A save passes them to `annots::write_markups_removing`: a markup a foreign
    /// viewer (Revu, Acrobat) authored has no `/RLType`, so absent from the model it would
    /// otherwise be kept as foreign and the delete would never reach the file. Only refs of
    /// markups this store held, never input. An undo (`add` of the same id) takes them back
    /// out. Cleared with `tombstones`.
    pub removed_sources: std::collections::HashSet<crate::markup::SourceRef>,
    /// Modified time and size of the file as redline last opened or wrote it. A save compares
    /// it to the file now: annotation matching uses object ids from the loaded file, so a
    /// file another program rewrote since would be saved over with stale ids (duplicating or
    /// stripping annotations). `None` when the file could not be stat'ed.
    pub disk_stamp: Option<DiskStamp>,
    /// A private copy of the file's bytes as redline last opened or wrote them (None when the
    /// file was too large to copy or the copy failed). "Save As (rebuild)" writes the user's
    /// mark-ups from these bytes after the real file changed under redline.
    pub original: Option<OriginalCopy>,
    /// Edit fingerprint of every markup as the file has it (loaded or last written), by id.
    /// The difference between this and `markups` is the user's unsaved work, which "Reload
    /// and keep my mark-ups" re-applies on top of the changed file.
    pub baseline: HashMap<Uuid, u64>,
    /// [`super::recover::content_key`] of every unnamed foreign markup as loaded, by id.
    pub keys: HashMap<Uuid, u64>,
}

/// Modified time, size and content hash of a file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStamp {
    pub modified: SystemTime,
    pub len: u64,
    /// Content hash (not cryptographic: it tells "same bytes" from "different bytes" within
    /// one run). `None` when the file could not be read through.
    pub hash: Option<u64>,
}

/// Files above this size are hashed but not copied for [`DocEntry::original`].
const ORIGINAL_COPY_CAP: u64 = 256 * 1024 * 1024;

/// Shown when a write finds the file changed under redline. A plain Save As is NOT offered:
/// it reads the same changed file, whose object numbers no longer match what redline loaded.
/// The recovery paths (`save_document_as_rebuild`, `reload_keeping_markups`) do not.
pub const ERR_DISK_CHANGED: &str = "The PDF was changed on disk by another program after redline \
opened it, so nothing was written (writing now could duplicate or remove annotations). Your \
mark-ups are still in redline. Press Save to recover them: Save As (rebuild) writes them to a \
new file, or you can reload the changed file and keep your mark-ups.";

/// Modified time and size only (cheap): enough to tell most changes apart.
fn stat_of(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Content hash of `path`, optionally copying the bytes to `copy_to` in the same pass.
fn hash_file(path: &std::path::Path, mut copy_to: Option<&mut std::fs::File>) -> Option<u64> {
    use std::hash::Hasher;
    use std::io::{Read, Write};
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.write(&buf[..n]);
        if let Some(out) = copy_to.as_deref_mut() {
            out.write_all(&buf[..n]).ok()?;
        }
    }
    Some(h.finish())
}

/// The process-private directory for original copies: a fresh random name, created
/// exclusively with mode 0700 (the `tempfile` crate), never a fixed or guessable path. Files
/// in it are `TempPath`s, deleted when dropped; the directory itself is swept on a later run
/// if a crash left it behind.
pub(crate) fn originals_dir() -> Option<&'static std::path::Path> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        sweep_old_originals();
        let mut b = tempfile::Builder::new();
        b.prefix("redline-originals-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            b.permissions(std::fs::Permissions::from_mode(0o700));
        }
        b.tempdir().ok().map(tempfile::TempDir::keep)
    })
    .as_deref()
}

/// Remove directories left by earlier runs that crashed before dropping their copies.
fn sweep_old_originals() {
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return };
    // A week: a long-running instance of ours keeps its directory younger by creating files.
    let day = std::time::Duration::from_secs(7 * 24 * 3600);
    for e in rd.flatten() {
        let ours = e.file_name().to_string_lossy().starts_with("redline-originals-");
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        // `symlink_metadata` semantics: never follow a link planted under our prefix.
        let real_dir = e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink());
        if ours && old && real_dir {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// A private, randomly named, exclusively created (0600 on unix) copy of a document's bytes;
/// the file is deleted when this is dropped (document closed, copy replaced, store dropped).
pub type OriginalCopy = tempfile::TempPath;

/// A consistent view of a file taken in one read: its stamp and the private copy of its
/// bytes. It is committed to a document as one unit (with the markups parsed from the same
/// file), never piecemeal.
pub struct Snapshot {
    path: PathBuf,
    stamp: Option<DiskStamp>,
    original: Option<OriginalCopy>,
}

/// Snapshot `path` now.
pub fn take_snapshot(path: &std::path::Path) -> Snapshot {
    let (stamp, original) = snapshot_of(path);
    Snapshot { path: path.to_path_buf(), stamp, original }
}

/// The file's current stamp, and - when it is small enough - a private copy of its bytes,
/// made in the same read as the hash. Never fails: what could not be read is `None`. Every
/// call makes a NEW file, so a copy is never shared or reused across documents.
fn snapshot_of(path: &std::path::Path) -> (Option<DiskStamp>, Option<OriginalCopy>) {
    let Some((modified, len)) = stat_of(path) else {
        return (None, None);
    };
    let mut copy: Option<(OriginalCopy, std::fs::File)> = None;
    if len <= ORIGINAL_COPY_CAP {
        // NamedTempFile: random name, O_EXCL create, mode 0600.
        copy = originals_dir()
            .and_then(|d| tempfile::Builder::new().suffix(".pdf").tempfile_in(d).ok())
            .map(|t| {
                let (file, tp) = t.into_parts();
                (tp, file)
            });
    }
    let hash = hash_file(path, copy.as_mut().map(|(_, f)| f));
    let stamp = Some(DiskStamp { modified, len, hash });
    // A copy whose read failed halfway is partial: drop (and so delete) it.
    let copy = copy.filter(|_| hash.is_some()).map(|(tp, _)| tp);
    (stamp, copy)
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
        let (disk_stamp, original) = snapshot_of(&path);
        self.docs.lock().unwrap().insert(
            doc_id.to_string(),
            DocEntry {
                disk_stamp,
                original,
                baseline: HashMap::new(),
                keys: HashMap::new(),
                path,
                password,
                markups: Vec::new(),
                loaded: false,
                saving: false,
                dirty: false,
                tombstones: Default::default(),
                removed_sources: Default::default(),
            },
        );
    }

    pub fn remove(&self, doc_id: &str) {
        // The entry's `original` (a `TempPath`) deletes the copy as it drops here.
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
        let (stamp, original) = snapshot_of(&path);
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        e.disk_stamp = stamp;
        e.original = original;
        e.path = path;
        Ok(())
    }

    /// After a successful write of the file: forget every reference to object positions of the
    /// file as it was before the write. Inline (direct-dictionary) annotations are matched by
    /// their position in the page's `/Annots`, which the write reordered, and a deleted
    /// markup's removal is done; keeping either would let the next save match a different
    /// annotation that now sits at that position. Written objects are managed by their own
    /// `/RLType` or `/RLPreserved` marker from here on.
    pub fn rebaseline_after_write(&self, doc_id: &str) {
        if let Some(e) = self.docs.lock().unwrap().get_mut(doc_id) {
            let drop_inline = |m: &mut Markup| {
                if m.source_ref.as_ref().is_some_and(|r| r.oid.is_none()) {
                    m.source_ref = None;
                }
            };
            e.markups.iter_mut().for_each(drop_inline);
            e.tombstones.values_mut().for_each(drop_inline);
            e.removed_sources.clear();
            // What was written is now the file: nothing is unsaved relative to it.
            e.baseline = fingerprints(&e.markups);
            // Written markups carry a /NM now: matched by id from here on.
            e.keys.clear();
        }
    }

    /// Err([`ERR_DISK_CHANGED`]) if the file registered for `doc_id` is no longer the one
    /// redline opened or last wrote. A different size is a change. A different modified time
    /// alone (a touch, a sync client) is a change only if the content hash differs too; an
    /// unchanged hash re-baselines the time. Fails open (Ok) when either stamp is unknown:
    /// the check exists to stop a stale-id save, not to block one on a stat error.
    pub fn check_disk_unchanged(&self, doc_id: &str) -> Result<(), String> {
        let (before, path) = {
            let g = self.docs.lock().unwrap();
            let e = g
                .get(doc_id)
                .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
            (e.disk_stamp, e.path.clone())
        };
        let (Some(before), Some((modified, len))) = (before, stat_of(&path)) else {
            return Ok(());
        };
        if (before.modified, before.len) == (modified, len) {
            return Ok(());
        }
        // Hash outside the lock: it reads the whole file.
        let same_content = len == before.len
            && before.hash.is_some()
            && hash_file(&path, None) == before.hash;
        if !same_content {
            return Err(ERR_DISK_CHANGED.to_string());
        }
        if let Some(e) = self.docs.lock().unwrap().get_mut(doc_id) {
            if e.path == path {
                e.disk_stamp = Some(DiskStamp { modified, ..before });
            }
        }
        Ok(())
    }

    /// Start a reload of the changed file: capture the user's unsaved work and a snapshot of
    /// the file as it is NOW, into locals. Nothing in the store changes until
    /// [`Self::commit_reload`], so any failure in between (parse, render reopen) leaves the
    /// old stamp, private copy and markups intact and the stale-file refusal in force.
    pub fn begin_reload(&self, doc_id: &str) -> Result<ReloadTxn, String> {
        let path = {
            let g = self.docs.lock().unwrap();
            let e = g
                .get(doc_id)
                .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
            if !e.loaded {
                return Err("This document's mark-ups were never loaded, so there is nothing \
                            to keep. Close and reopen it."
                    .to_string());
            }
            e.path.clone()
        };
        // Drop only the cached parse (it is keyed by modified time and could be stale).
        self.cache.lock().unwrap().invalidate(&path);
        let snapshot = take_snapshot(&path);
        let g = self.docs.lock().unwrap();
        let e = g
            .get(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        let mut unsaved = super::recover::Unsaved::default();
        for m in &e.markups {
            match e.baseline.get(&m.id()) {
                None => unsaved.added.push(m.clone()),
                Some(fp) if m.edit_fingerprint() != Some(*fp) => unsaved.modified.push(m.clone()),
                Some(_) => {}
            }
        }
        let present: std::collections::HashSet<Uuid> = e.markups.iter().map(|m| m.id()).collect();
        unsaved.deleted = e.baseline.keys().filter(|id| !present.contains(id)).copied().collect();
        Ok(ReloadTxn {
            path,
            unsaved,
            baseline: e.baseline.clone(),
            keys: e.keys.clone(),
            snapshot,
        })
    }

    /// Finish a reload once the changed file is parsed (`loaded`) and the render engine shows
    /// it (`page_count`): merge the unsaved edits, then install markups, stamp and private
    /// copy TOGETHER. Refuses (store untouched) if the document was re-pointed meanwhile.
    pub fn commit_reload(
        &self,
        doc_id: &str,
        txn: ReloadTxn,
        loaded: Vec<Markup>,
        page_count: u32,
    ) -> Result<super::recover::ReloadReport, String> {
        let (merged, removed, report) = super::recover::merge_unsaved(
            loaded.clone(),
            &txn.baseline,
            &txn.keys,
            txn.unsaved,
            page_count,
        );
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        if e.path != txn.snapshot.path {
            return Err("The document was moved while it was being reloaded; try again.".into());
        }
        e.markups = merged;
        e.baseline = fingerprints(&loaded);
        e.keys = content_keys(&loaded);
        e.tombstones.clear();
        e.removed_sources = removed;
        e.loaded = true;
        e.dirty = report.applied > 0;
        e.disk_stamp = txn.snapshot.stamp;
        e.original = txn.snapshot.original;
        drop(g);
        self.cache.lock().unwrap().set(txn.snapshot.path, loaded);
        Ok(report)
    }

    /// After a Save As (rebuild): the new file holds every markup as redline wrote it, so the
    /// object ids, original dictionaries and names of the OLD file mean nothing there and
    /// could match an unrelated annotation. Forget them all.
    pub fn forget_source(&self, doc_id: &str) {
        if let Some(e) = self.docs.lock().unwrap().get_mut(doc_id) {
            let forget = |m: &mut Markup| {
                m.source_ref = None;
                m.source_dict = None;
                m.source_fp = None;
            };
            e.markups.iter_mut().for_each(forget);
            e.tombstones.values_mut().for_each(forget);
            e.removed_sources.clear();
        }
    }

    /// The private copy of the bytes redline last opened or wrote, for Save As (rebuild).
    pub fn original_copy(&self, doc_id: &str) -> Option<PathBuf> {
        self.docs.lock().unwrap().get(doc_id).and_then(|e| e.original.as_ref().map(|c| c.to_path_buf()))
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
    pub fn add(&self, doc_id: &str, mut m: Markup) -> Result<(), String> {
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        if e.markups.iter().any(|x| x.id() == m.id()) {
            return Err(format!("duplicate markup id {}", m.id()));
        }
        // Reader-owned names never come from the input (see `Markup::clear_reader_owned`).
        // The one exception is an undo of a delete: the same id returns to the store and gets
        // back the names this store itself held for it, from the tombstone - not the input's.
        match e.tombstones.remove(&m.id()) {
            Some(deleted) => m.adopt_reader_owned_from(&deleted),
            None => m.clear_reader_owned(),
        }
        // Back in the model: no longer a deletion. (The tombstone's names are the ones that
        // were recorded for it; `m` has just adopted exactly those.)
        if let Some(r) = &m.source_ref {
            e.removed_sources.remove(r);
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
    pub fn update(&self, doc_id: &str, mut m: Markup) -> Result<(), String> {
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
        // Keep the stored markup's reader-owned names; ignore any the caller sent.
        m.adopt_reader_owned_from(slot);
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
        if existing.has_reader_owned() {
            e.tombstones.insert(id, existing.clone());
        }
        if let Some(r) = &existing.source_ref {
            e.removed_sources.insert(r.clone());
        }
        e.markups.retain(|x| x.id() != id);
        e.dirty = true;
        Ok(())
    }

    /// Bring every stored measurement under the document's current `scales`, with the rule the
    /// frontend and the exports use (`takeoff::recompute_all`). Returns how many changed. The
    /// quantities are derived data, so a locked markup is recomputed too.
    pub fn recompute_measurements(
        &self,
        doc_id: &str,
        scales: &[crate::takeoff::ScaleRecord],
    ) -> usize {
        let mut g = self.docs.lock().unwrap();
        let Some(e) = g.get_mut(doc_id) else { return 0 };
        let changed = crate::takeoff::recompute_all(&mut e.markups, scales);
        if changed > 0 {
            e.dirty = true;
        }
        changed
    }

    /// Merge markups loaded from the PDF beneath any unsaved in-memory ones
    /// (the store wins on id collision) and mark the doc as loaded.
    /// Returns the merged set.
    ///
    /// On a collision a foreign markup the user never edited is replaced by the loaded one;
    /// any other stored markup keeps its model (the user's unsaved edits) but takes
    /// the reader-owned state of the loaded one: the file may have been replaced since the
    /// stored copy was read (a History restore), and a stale original or object id there
    /// would be written back or matched against the wrong annotation.
    pub fn seed_loaded(&self, doc_id: &str, loaded: Vec<Markup>) -> Result<Vec<Markup>, String> {
        self.seed_loaded_with(doc_id, loaded, None)
    }

    /// The snapshot to pass to [`Self::seed_loaded_with`]: taken BEFORE the file is parsed, and
    /// only for a document whose markups are not loaded yet. The stamp, private copy and
    /// markup ids then all describe the same file even if it changed after the document was
    /// registered. `None` when already loaded (the existing baseline stays authoritative).
    pub fn begin_first_load(&self, doc_id: &str) -> Option<Snapshot> {
        let path = {
            let g = self.docs.lock().unwrap();
            let e = g.get(doc_id)?;
            (!e.loaded).then(|| e.path.clone())?
        };
        Some(take_snapshot(&path))
    }

    /// [`Self::seed_loaded`], also adopting `snapshot` (from [`Self::begin_first_load`]) as the
    /// document's stamp and private copy if it is still the first load of the same path.
    pub fn seed_loaded_with(
        &self,
        doc_id: &str,
        loaded: Vec<Markup>,
        snapshot: Option<Snapshot>,
    ) -> Result<Vec<Markup>, String> {
        let mut g = self.docs.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        if let Some(snap) = snapshot {
            if !e.loaded && e.path == snap.path {
                e.disk_stamp = snap.stamp;
                e.original = snap.original;
            }
        }
        for (id, k) in content_keys(&loaded) {
            e.keys.insert(id, k);
        }
        for (id, fp) in fingerprints(&loaded) {
            e.baseline.insert(id, fp);
        }
        let by_id: HashMap<Uuid, &Markup> = loaded.iter().map(|m| (m.id(), m)).collect();
        for stored in e.markups.iter_mut() {
            if let Some(fresh) = by_id.get(&stored.id()) {
                if stored.is_unedited() {
                    // Never touched since it was read: the file's copy is the truth.
                    *stored = (*fresh).clone();
                } else {
                    stored.adopt_reader_owned_from(fresh);
                }
            }
        }
        let unsaved: std::collections::HashSet<uuid::Uuid> =
            e.markups.iter().map(|m| m.id()).collect();
        let mut merged: Vec<Markup> = loaded
            .into_iter()
            .filter(|m| !unsaved.contains(&m.id()))
            .collect();
        merged.append(&mut e.markups);
        e.markups = merged;
        e.tombstones.clear();
        e.removed_sources.clear();
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

    /// Source annotations of markups deleted since load, for `annots::write_markups_removing`
    /// (see [`DocEntry::removed_sources`]). Empty for an unknown doc.
    pub fn removed_sources(&self, doc_id: &str) -> std::collections::HashSet<crate::markup::SourceRef> {
        self.docs
            .lock()
            .unwrap()
            .get(doc_id)
            .map(|e| e.removed_sources.clone())
            .unwrap_or_default()
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
        // Every caller has just written `path` itself, so it is again the file redline knows.
        let ids: Vec<String> = self
            .docs
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, e)| e.path == *path)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let (stamp, original) = snapshot_of(path);
            if let Some(e) = self.docs.lock().unwrap().get_mut(&id) {
                if e.path == *path {
                    e.disk_stamp = stamp;
                    e.original = original;
                }
            }
        }
    }
}

/// An in-flight reload (see [`MarkupStore::begin_reload`]).
pub struct ReloadTxn {
    path: PathBuf,
    unsaved: super::recover::Unsaved,
    baseline: HashMap<Uuid, u64>,
    keys: HashMap<Uuid, u64>,
    snapshot: Snapshot,
}

impl ReloadTxn {
    /// The unsaved work, the baseline fingerprints and the content keys captured at the start.
    pub fn into_parts(
        self,
    ) -> (super::recover::Unsaved, HashMap<Uuid, u64>, HashMap<Uuid, u64>) {
        (self.unsaved, self.baseline, self.keys)
    }
}

fn content_keys(markups: &[Markup]) -> HashMap<Uuid, u64> {
    markups
        .iter()
        .filter_map(|m| super::recover::content_key(m).map(|k| (m.id(), k)))
        .collect()
}

/// Edit fingerprint per markup id (a markup that cannot be fingerprinted is left out, so it
/// always reads as changed).
fn fingerprints(markups: &[Markup]) -> HashMap<Uuid, u64> {
    markups
        .iter()
        .filter_map(|m| m.edit_fingerprint().map(|f| (m.id(), f)))
        .collect()
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

    // --- save refuses a file changed under redline ------------------------------------

    #[test]
    fn writes_are_refused_once_the_file_changed_on_disk_and_allowed_again_after_our_own_write() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.pdf");
        std::fs::write(&p, b"%PDF-1.5 original").unwrap();
        let s = MarkupStore::default();
        s.register("d1", p.clone(), None);
        assert_eq!(s.check_disk_unchanged("d1"), Ok(()), "unchanged since open");

        // Another program rewrites it (different size, so the check holds on coarse mtimes).
        std::fs::write(&p, b"%PDF-1.5 rewritten by another program").unwrap();
        let err = s.check_disk_unchanged("d1").unwrap_err();
        assert_eq!(err, ERR_DISK_CHANGED);
        assert!(err.contains("Save As (rebuild)") && err.contains("reload"), "{err}");

        // redline's own write (every caller of invalidate_cache) makes it the known file again.
        s.invalidate_cache(&p);
        assert_eq!(s.check_disk_unchanged("d1"), Ok(()));
    }

    #[test]
    fn same_size_rewrite_is_caught_by_mtime_and_save_as_adopts_the_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.pdf"), dir.path().join("b.pdf"));
        std::fs::write(&a, b"AAAA").unwrap();
        let s = MarkupStore::default();
        s.register("d1", a.clone(), None);
        std::fs::write(&a, b"CCCC").unwrap(); // same size, different content
        let f = std::fs::OpenOptions::new().write(true).open(&a).unwrap();
        f.set_modified(SystemTime::now() + std::time::Duration::from_secs(60)).unwrap();
        assert_eq!(s.check_disk_unchanged("d1"), Err(ERR_DISK_CHANGED.to_string()));

        std::fs::write(&b, b"BBBB").unwrap();
        s.set_path("d1", b).unwrap();
        assert_eq!(s.check_disk_unchanged("d1"), Ok(()), "Save As re-baselines on the new file");
    }

    #[test]
    fn disk_check_fails_open_for_an_unreadable_path_and_errors_for_an_unknown_doc() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/nonexistent/none.pdf"), None);
        assert_eq!(s.check_disk_unchanged("d1"), Ok(()));
        assert!(s.check_disk_unchanged("nope").is_err());
    }
}
