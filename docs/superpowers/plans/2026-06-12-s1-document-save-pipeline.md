# S1: Document Save Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Markups persist into the PDF file: open reads existing annotations, save writes the in-memory markup set back as standard PDF annotations, atomically, without disturbing foreign annotations (links etc.).

**Architecture:** An in-memory `MarkupStore` (doc_id → path + markups) lives in `AppState`. A new `document::annots` layer reads/writes markups in a `lopdf::Document` using the existing `Markup::to_annotation_dict` / `from_annotation_dict` (markup/annotation.rs). `document::save` does load → mutate /Annots → save-to-temp → fsync → atomic rename. Save-in-place closes the doc in the render engine first (Windows can't rename over an open file), renames, reopens under the same doc_id. All lopdf work runs in `tokio::task::spawn_blocking`.

**Tech Stack:** lopdf 0.36 (verified APIs: `Document::load`, `doc.save`, `add_object`, `get_pages` → `BTreeMap<u32 /*1-based*/, ObjectId>`, `get_dictionary_mut`, `get_object_mut`, `as_array_mut`, `Object::Reference`, `Document::with_version`, `new_object_id`, `dictionary!` macro, pub `trailer`/`objects`). Tauri 2 commands. tempfile (dev-dep already present) for tests.

**Design decisions (locked):**
- **Full-rewrite save, not incremental update.** lopdf load+save is the v1 path; measured acceptable on corpus (C5 normalise ≈ 8 s is the worst case and is an explicit user action here). Incremental update is a later optimization.
- **Managed-annotation policy:** an annotation is *managed* (replaced on save) iff it has an `/RLType` key OR its `/NM` is in the store's id set. Everything else (Link, Popup, Widget, foreign markups the user didn't import) is preserved untouched.
- **Import filter:** on load, only subtypes `Text FreeText Square Circle Line Polygon PolyLine Highlight Ink Stamp` become `Markup`s. Others stay in the file but out of the store.
- **Page indexing:** `get_pages()` is 1-based; `Markup.page` is 0-based. The page a markup is read from always wins over a stale `/RLPage`.
- **Save-As switches the open document** to the new path (same doc_id, render reopened).

**Files:**
- Modify: `src-tauri/src/lib.rs` (AppState + command registration)
- Create: `src-tauri/src/document/store.rs` (MarkupStore)
- Create: `src-tauri/src/document/annots.rs` (lopdf ↔ Markup)
- Create: `src-tauri/src/document/save.rs` (atomic save + load)
- Modify: `src-tauri/src/document/mod.rs` (module wiring)
- Modify: `src-tauri/src/commands/document.rs` (new commands + store registration)
- Modify: `src/lib/ipc.ts` (wrappers + Markup type)
- Modify: `src/App.svelte` (Save/Save-As buttons + Cmd/Ctrl+S)

**Standing constraints (every task):** PDFium tests stay serial (`--test-threads=1`); never touch `bench/corpus/` originals (copy to temp); conventional commits; `cargo clippy --all-targets` must stay at 0 warnings. Read every file fresh before editing (parallel-edit staleness risk).

---

### Task 1: MarkupStore + add/list commands

**Files:**
- Create: `src-tauri/src/document/store.rs`
- Modify: `src-tauri/src/document/mod.rs` (add `pub mod store;` + re-export)
- Modify: `src-tauri/src/lib.rs` (AppState field, command registration)
- Modify: `src-tauri/src/commands/document.rs` (register doc in store on open, drop on close, add/list commands)

- [ ] **Step 1: Write failing tests** in `src-tauri/src/document/store.rs`:

```rust
//! In-memory markup store — doc_id → (path, markups). Single source of truth for
//! unsaved markup state; the save pipeline (document::save) flushes it to the PDF.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::markup::Markup;

#[derive(Debug)]
pub struct DocEntry {
    pub path: PathBuf,
    pub markups: Vec<Markup>,
}

/// Thread-safe store shared via Tauri `AppState`.
#[derive(Debug, Default)]
pub struct MarkupStore(Mutex<HashMap<String, DocEntry>>);

impl MarkupStore {
    pub fn register(&self, doc_id: &str, path: PathBuf) {
        self.0.lock().unwrap().insert(
            doc_id.to_string(),
            DocEntry { path, markups: Vec::new() },
        );
    }

    pub fn remove(&self, doc_id: &str) {
        self.0.lock().unwrap().remove(doc_id);
    }

    /// Path registered for this doc, if open.
    pub fn path(&self, doc_id: &str) -> Option<PathBuf> {
        self.0.lock().unwrap().get(doc_id).map(|e| e.path.clone())
    }

    pub fn set_path(&self, doc_id: &str, path: PathBuf) -> Result<(), String> {
        let mut g = self.0.lock().unwrap();
        let e = g.get_mut(doc_id).ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        e.path = path;
        Ok(())
    }

    /// Add one markup. Errors on unknown doc or duplicate id.
    pub fn add(&self, doc_id: &str, m: Markup) -> Result<(), String> {
        let mut g = self.0.lock().unwrap();
        let e = g.get_mut(doc_id).ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        if e.markups.iter().any(|x| x.id() == m.id()) {
            return Err(format!("duplicate markup id {}", m.id()));
        }
        e.markups.push(m);
        Ok(())
    }

    /// Replace the full markup set (used after loading from the PDF).
    pub fn set_all(&self, doc_id: &str, markups: Vec<Markup>) -> Result<(), String> {
        let mut g = self.0.lock().unwrap();
        let e = g.get_mut(doc_id).ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        e.markups = markups;
        Ok(())
    }

    /// Snapshot of the current markups (cloned; store stays locked only briefly).
    pub fn list(&self, doc_id: &str) -> Result<Vec<Markup>, String> {
        let g = self.0.lock().unwrap();
        g.get(doc_id)
            .map(|e| e.markups.clone())
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))
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
            UserRef { user_id: uuid::Uuid::new_v4(), display_name: "T".into() },
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
}
```

- [ ] **Step 2:** `cargo test store` → FAIL (module not wired). Wire `pub mod store;` into `src-tauri/src/document/mod.rs` (keep existing content), run again → PASS (implementation is written with the tests in this slice — store is pure data, test-first at file granularity).
- [ ] **Step 3:** Add the store to `AppState` in `src-tauri/src/lib.rs`:

```rust
use document::store::MarkupStore;

pub struct AppState {
    pub render: RenderHandle,
    pub markups: MarkupStore,
}
```
and in `.setup(...)`: `app.manage(AppState { render, markups: MarkupStore::default() });`

- [ ] **Step 4:** In `src-tauri/src/commands/document.rs`: register/remove on open/close, and add two commands:

```rust
use crate::markup::Markup;

// inside open_document, after render open succeeds:
state.markups.register(&doc_id, path.clone());

// inside close_document, before returning:
state.markups.remove(&doc_id);

/// Add a markup to the open document's in-memory set (not yet saved to the file).
#[tauri::command]
pub async fn add_markup(state: State<'_, AppState>, doc_id: String, markup: Markup) -> Result<(), String> {
    state.markups.add(&doc_id, markup)
}

/// List the open document's in-memory markups.
#[tauri::command]
pub async fn list_markups(state: State<'_, AppState>, doc_id: String) -> Result<Vec<Markup>, String> {
    state.markups.list(&doc_id)
}
```
Register both in `lib.rs` `generate_handler![...]`.
NOTE: `Markup` already derives Serialize+Deserialize; the private `id` field round-trips through serde (existing test `serde_round_trip_preserves_everything`).

- [ ] **Step 5:** `cargo test && cargo clippy --all-targets` → green/0 warnings.
- [ ] **Step 6:** Commit: `feat(document): in-memory markup store + add/list commands`

---

### Task 2: Read markups from a lopdf Document

**Files:**
- Create: `src-tauri/src/document/annots.rs`
- Modify: `src-tauri/src/document/mod.rs` (`pub mod annots;`)

- [ ] **Step 1: Write the failing test** (bottom of new `annots.rs`). The fixture builds a one-page PDF in memory with one redline-authored annot, one foreign Square, and one Link:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PdfPoint;
    use crate::markup::{Appearance, Markup, MarkupGeometry, MarkupType, UserRef};
    use lopdf::{dictionary, Document, Object, Stream};

    /// Minimal valid one-page PDF built programmatically (no file I/O).
    pub(super) fn one_page_doc() -> (Document, lopdf::ObjectId) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let content_id = doc.add_object(Stream::new(dictionary! {}, b"BT ET".to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        (doc, page_id)
    }

    pub(super) fn redline_markup(page: u32) -> Markup {
        let mut m = Markup::new(
            MarkupType::Cloud,
            page,
            MarkupGeometry::Polyline(vec![
                PdfPoint { x: 10.0, y: 10.0 },
                PdfPoint { x: 50.0, y: 10.0 },
                PdfPoint { x: 50.0, y: 40.0 },
            ]),
            Appearance::default(),
            UserRef { user_id: uuid::Uuid::new_v4(), display_name: "Alice".into() },
        );
        m.contents = Some("check clearance".into());
        m
    }

    fn link_dict() -> lopdf::Dictionary {
        dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => vec![0.into(), 0.into(), 100.into(), 20.into()],
        }
    }

    #[test]
    fn reads_markup_annots_skips_links_and_fixes_page_index() {
        let (mut doc, page_id) = one_page_doc();
        let m = redline_markup(7); // wrong page index on purpose — read must override to 0
        let a1 = doc.add_object(Object::Dictionary(m.to_annotation_dict()));
        let a2 = doc.add_object(Object::Dictionary(link_dict()));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Array(vec![a1.into(), a2.into()]));

        let got = read_markups(&doc).unwrap();
        assert_eq!(got.len(), 1, "Link must not import");
        assert_eq!(got[0].id(), m.id());
        assert_eq!(got[0].markup_type, MarkupType::Cloud);
        assert_eq!(got[0].page, 0, "page index comes from the page tree, not /RLPage");
        assert_eq!(got[0].contents.as_deref(), Some("check clearance"));
    }

    #[test]
    fn reads_direct_and_referenced_annots_arrays() {
        // /Annots may be a direct array (above) or a Reference to an array object.
        let (mut doc, page_id) = one_page_doc();
        let a1 = doc.add_object(Object::Dictionary(redline_markup(0).to_annotation_dict()));
        let arr_id = doc.add_object(Object::Array(vec![a1.into()]));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Reference(arr_id));
        assert_eq!(read_markups(&doc).unwrap().len(), 1);
    }

    #[test]
    fn no_annots_key_reads_empty() {
        let (doc, _) = one_page_doc();
        assert!(read_markups(&doc).unwrap().is_empty());
    }
}
```

- [ ] **Step 2:** `cargo test annots` → FAIL (`read_markups` undefined).
- [ ] **Step 3: Implement** (top of `annots.rs`):

```rust
//! lopdf-level read/write of redline markups in a PDF's page /Annots arrays.
//!
//! Managed-annotation policy: an annotation is *managed* (owned/replaced by redline on
//! save) iff it carries an /RLType key OR its /NM matches a markup id in the store.
//! Foreign annotations (links, popups, widgets, third-party markups) are preserved
//! untouched. Import filter: only markup-like subtypes become `Markup`s on read.

use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::markup::Markup;

/// PDF annotation subtypes imported as markups (spec §6 type set).
const MARKUP_SUBTYPES: &[&str] = &[
    "Text", "FreeText", "Square", "Circle", "Line", "Polygon", "PolyLine",
    "Highlight", "Ink", "Stamp",
];

fn subtype(d: &Dictionary) -> Option<String> {
    d.get(b"Subtype").ok()?.as_name().ok().map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Resolve the page's /Annots into a list of (annot ObjectId | inline dict).
/// Returns owned dictionaries plus the id when the annot is an indirect object.
fn page_annots(doc: &Document, page_id: ObjectId) -> Result<Vec<(Option<ObjectId>, Dictionary)>> {
    let page = doc.get_dictionary(page_id).context("page dict")?;
    let Ok(annots_obj) = page.get(b"Annots") else { return Ok(Vec::new()) };
    // /Annots may be a direct array or a Reference to an array.
    let arr: Vec<Object> = match annots_obj {
        Object::Array(a) => a.clone(),
        Object::Reference(r) => doc
            .get_object(*r)
            .and_then(|o| o.as_array().map(|a| a.clone()))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for entry in arr {
        match entry {
            Object::Reference(rid) => {
                if let Ok(d) = doc.get_dictionary(rid) {
                    out.push((Some(rid), d.clone()));
                }
            }
            Object::Dictionary(d) => out.push((None, d)),
            _ => {}
        }
    }
    Ok(out)
}

/// Read all markup-like annotations. Page index (0-based) comes from the page tree.
pub fn read_markups(doc: &Document) -> Result<Vec<Markup>> {
    let mut out = Vec::new();
    for (page_no_1based, page_id) in doc.get_pages() {
        for (_, dict) in page_annots(doc, page_id)? {
            let Some(st) = subtype(&dict) else { continue };
            if !MARKUP_SUBTYPES.contains(&st.as_str()) {
                continue;
            }
            let mut m = Markup::from_annotation_dict(&dict);
            m.page = page_no_1based - 1;
            out.push(m);
        }
    }
    Ok(out)
}
```

- [ ] **Step 4:** `cargo test annots` → 3 PASS. `cargo clippy --all-targets` → 0 warnings.
- [ ] **Step 5:** Commit: `feat(document): read markups from PDF /Annots via lopdf`

---

### Task 3: Write markups into a lopdf Document

**Files:**
- Modify: `src-tauri/src/document/annots.rs`

- [ ] **Step 1: Write failing tests** (append to `annots.rs` tests):

```rust
    #[test]
    fn write_then_read_roundtrips_and_preserves_foreign() {
        let (mut doc, page_id) = one_page_doc();
        // Pre-existing foreign Link on the page.
        let link = doc.add_object(Object::Dictionary(link_dict()));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Array(vec![link.into()]));

        let m = redline_markup(0);
        write_markups(&mut doc, &[m.clone()]).unwrap();

        // Our markup reads back; the Link is still in the page's /Annots.
        let got = read_markups(&doc).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), m.id());
        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), 2, "link + markup");
        assert!(annots.iter().any(|(_, d)| subtype(d).as_deref() == Some("Link")));
    }

    #[test]
    fn second_write_replaces_not_duplicates() {
        let (mut doc, page_id) = one_page_doc();
        let mut m = redline_markup(0);
        write_markups(&mut doc, &[m.clone()]).unwrap();
        m.contents = Some("edited".into());
        write_markups(&mut doc, &[m.clone()]).unwrap();

        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), 1, "managed annot replaced, not duplicated");
        let got = read_markups(&doc).unwrap();
        assert_eq!(got[0].contents.as_deref(), Some("edited"));
    }

    #[test]
    fn deleting_from_store_removes_from_pdf() {
        let (mut doc, _) = one_page_doc();
        write_markups(&mut doc, &[redline_markup(0)]).unwrap();
        write_markups(&mut doc, &[]).unwrap(); // markup deleted in the app
        assert!(read_markups(&doc).unwrap().is_empty());
    }

    #[test]
    fn out_of_range_page_errors() {
        let (mut doc, _) = one_page_doc();
        let m = redline_markup(5); // page 5 doesn't exist
        assert!(write_markups(&mut doc, &[m]).is_err());
    }
```

- [ ] **Step 2:** `cargo test annots` → FAIL (`write_markups` undefined).
- [ ] **Step 3: Implement** (in `annots.rs`):

```rust
fn nm_of(d: &Dictionary) -> Option<String> {
    d.get(b"NM").ok()?.as_str().ok().map(|b| String::from_utf8_lossy(b).into_owned())
}

/// True if redline owns this annotation (replace-on-save).
fn is_managed(d: &Dictionary, ids: &std::collections::HashSet<String>) -> bool {
    d.has(b"RLType") || nm_of(d).map(|nm| ids.contains(&nm)).unwrap_or(false)
}

/// Write the full markup set into the document: strip managed annotations from every
/// page, keep foreign ones, then append the current set as indirect objects.
pub fn write_markups(doc: &mut Document, markups: &[Markup]) -> Result<()> {
    let ids: std::collections::HashSet<String> =
        markups.iter().map(|m| m.id().to_string()).collect();
    let pages = doc.get_pages(); // 1-based page no → page ObjectId

    // 1. Surviving foreign entries per page (as raw array entries).
    let mut kept: std::collections::BTreeMap<ObjectId, Vec<Object>> = pages
        .values()
        .map(|pid| (*pid, Vec::new()))
        .collect();
    for (_, page_id) in &pages {
        let mut keep = Vec::new();
        for (oid, dict) in page_annots(doc, *page_id)? {
            if !is_managed(&dict, &ids) {
                keep.push(match oid {
                    Some(rid) => Object::Reference(rid),
                    None => Object::Dictionary(dict),
                });
            }
        }
        kept.insert(*page_id, keep);
    }

    // 2. Append the current markups to their pages as fresh indirect objects.
    for m in markups {
        let page_no = m.page + 1; // store is 0-based
        let page_id = *pages
            .get(&page_no)
            .with_context(|| format!("markup {} targets page {} of {}", m.id(), m.page, pages.len()))?;
        let aid = doc.add_object(Object::Dictionary(m.to_annotation_dict()));
        kept.get_mut(&page_id).expect("page in map").push(Object::Reference(aid));
    }

    // 3. Set each page's /Annots directly (drop any old Reference indirection).
    for (page_id, entries) in kept {
        let page = doc.get_dictionary_mut(page_id).context("page dict")?;
        if entries.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", Object::Array(entries));
        }
    }
    Ok(())
}
```
NOTE for implementer: `doc.get_pages()` borrows immutably and `add_object` mutably — the implementation above collects first, mutates after; if the borrow checker objects, clone `pages` up front (it is a small `BTreeMap<u32, ObjectId>`).

- [ ] **Step 4:** `cargo test annots` → all PASS (7 total in module). `cargo clippy --all-targets` → 0.
- [ ] **Step 5:** Commit: `feat(document): write markup set into PDF /Annots (foreign-preserving)`

---

### Task 4: Atomic save + path-level load

**Files:**
- Create: `src-tauri/src/document/save.rs`
- Modify: `src-tauri/src/document/mod.rs` (`pub mod save;`)

- [ ] **Step 1: Write failing tests** in `save.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::annots::tests_support::{one_page_doc, redline_markup};

    #[test]
    fn save_with_markups_writes_dest_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        let dest = dir.path().join("out.pdf");
        let m = redline_markup(0);
        save_with_markups(&src, &dest, &[m.clone()]).unwrap();

        let got = load_markups_from(&dest).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), m.id());
        // Source untouched.
        assert!(load_markups_from(&src).unwrap().is_empty());
    }

    #[test]
    fn save_in_place_via_temp_swap() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("doc.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&p).unwrap();

        save_with_markups(&p, &p, &[redline_markup(0)]).unwrap();
        assert_eq!(load_markups_from(&p).unwrap().len(), 1);
        // No stray temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".redline-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file not cleaned: {leftovers:?}");
    }

    #[test]
    fn missing_source_errors_and_dest_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.pdf");
        let err = save_with_markups(&dir.path().join("absent.pdf"), &dest, &[]);
        assert!(err.is_err());
        assert!(!dest.exists());
    }
}
```
And in `annots.rs`, expose the two test fixtures for reuse (above its `mod tests`):

```rust
/// Test-only fixture support shared with document::save tests.
#[cfg(test)]
pub mod tests_support {
    pub use super::tests::{one_page_doc, redline_markup};
}
```
(make `one_page_doc` / `redline_markup` `pub(crate)` inside the tests module, or move them into `tests_support` directly — implementer's choice, keep clippy clean.)

- [ ] **Step 2:** `cargo test save` → FAIL.
- [ ] **Step 3: Implement** `save.rs`:

```rust
//! Atomic markup save: lopdf load → write_markups → save to a sibling temp file →
//! fsync → rename over the destination (the workspace sensitive-write pattern).
//! Full-rewrite strategy (v1); incremental PDF update is a later optimization.

use std::path::Path;

use anyhow::{Context, Result};
use lopdf::Document;

use super::annots::{read_markups, write_markups};
use crate::markup::Markup;

/// Read the markup set from a PDF on disk.
pub fn load_markups_from(path: &Path) -> Result<Vec<Markup>> {
    let doc = Document::load(path).with_context(|| format!("load {}", path.display()))?;
    read_markups(&doc)
}

/// Load `src`, replace its managed annotations with `markups`, atomically produce
/// `dest`. `src == dest` is the save-in-place case (temp + rename over).
pub fn save_with_markups(src: &Path, dest: &Path, markups: &[Markup]) -> Result<()> {
    let mut doc = Document::load(src).with_context(|| format!("load {}", src.display()))?;
    write_markups(&mut doc, markups)?;

    let dir = dest.parent().context("dest has no parent dir")?;
    let tmp = dir.join(format!(
        ".redline-tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> Result<()> {
        let f = doc.save(&tmp).with_context(|| format!("write {}", tmp.display()))?;
        f.sync_all().context("fsync temp")?;
        std::fs::rename(&tmp, dest).context("atomic rename")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
```

- [ ] **Step 4:** `cargo test` (whole suite) → PASS; clippy 0.
- [ ] **Step 5:** Commit: `feat(document): atomic save_with_markups + load_markups_from`

---

### Task 5: Tauri commands (load/save/save-as) with render close-reopen

**Files:**
- Modify: `src-tauri/src/commands/document.rs`
- Modify: `src-tauri/src/lib.rs` (register 3 commands)

No unit-testable seam here beyond what Tasks 1-4 cover (commands are orchestration over tested parts + the render thread); verification is compile + clippy + the Task 8 corpus test + Task 9 manual run.

- [ ] **Step 1: Implement** in `commands/document.rs`:

```rust
use crate::document::save::{load_markups_from, save_with_markups};

/// Read existing annotations from the PDF into the store (call after open; runs lopdf
/// in a blocking task — cheap on typical sets, seconds on very large ones).
#[tauri::command]
pub async fn load_markups(state: State<'_, AppState>, doc_id: String) -> Result<Vec<Markup>, String> {
    let path = state.markups.path(&doc_id).ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let loaded = tokio::task::spawn_blocking(move || load_markups_from(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    state.markups.set_all(&doc_id, loaded.clone())?;
    Ok(loaded)
}

/// Save the in-memory markups into the open file (atomic in-place).
/// Order matters: build the new file to a temp via lopdf FIRST (source still open is
/// fine — read-only), then close the doc in the render engine (Windows cannot rename
/// over an open file), rename, reopen under the SAME doc_id.
#[tauri::command]
pub async fn save_document(state: State<'_, AppState>, doc_id: String) -> Result<(), String> {
    save_impl(state, doc_id, None).await
}

/// Save-As: write to `new_path` and switch the open document to it (same doc_id).
#[tauri::command]
pub async fn save_document_as(
    state: State<'_, AppState>,
    doc_id: String,
    new_path: String,
) -> Result<(), String> {
    save_impl(state, doc_id, Some(PathBuf::from(new_path))).await
}

async fn save_impl(
    state: State<'_, AppState>,
    doc_id: String,
    new_path: Option<PathBuf>,
) -> Result<(), String> {
    let src = state.markups.path(&doc_id).ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let dest = new_path.clone().unwrap_or_else(|| src.clone());
    let markups = state.markups.list(&doc_id)?;

    // 1. Build the destination bytes to a temp + rename — EXCEPT in-place, where we
    //    must close the render doc before the rename. So: stage to a real temp path
    //    first, then swap.
    let staged = dest.with_extension("pdf.redline-staged");
    {
        let src = src.clone();
        let staged = staged.clone();
        tokio::task::spawn_blocking(move || save_with_markups(&src, &staged, &markups))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
    }

    // 2. Release the file from the render engine, swap, reopen under the same doc_id.
    state.render.close_document(doc_id.clone()).await.map_err(|e| format!("{e:#}"))?;
    let swap = std::fs::rename(&staged, &dest);
    if let Err(e) = swap {
        let _ = std::fs::remove_file(&staged);
        // Try to restore the render doc on the ORIGINAL path before failing.
        let _ = state.render.open_document(src.clone(), doc_id.clone()).await;
        return Err(format!("swap failed: {e}"));
    }
    state
        .render
        .open_document(dest.clone(), doc_id.clone())
        .await
        .map_err(|e| format!("reopen after save: {e:#}"))?;
    if new_path.is_some() {
        state.markups.set_path(&doc_id, dest)?;
    }
    Ok(())
}
```
NOTE: `save_with_markups` already does its own temp+rename to produce `staged`; the second rename swaps it in after the render close. Net effect: the original file is replaced only by a complete, fsynced file.

- [ ] **Step 2:** Register `load_markups`, `save_document`, `save_document_as` in `lib.rs` `generate_handler![]`.
- [ ] **Step 3:** `cargo build && cargo test && cargo clippy --all-targets` → green/0.
- [ ] **Step 4:** Commit: `feat(commands): load/save/save-as markups with render close-reopen swap`

---

### Task 6: ipc.ts wrappers + Markup type

**Files:**
- Modify: `src/lib/ipc.ts`

- [ ] **Step 1: Append** the serde-mirroring types + wrappers:

```typescript
// ---------------------------------------------------------------------------
// Markup types (mirrors src-tauri/src/markup/mod.rs serde JSON shapes)
// ---------------------------------------------------------------------------

export interface PdfPoint { x: number; y: number; }
export interface UserRef { user_id: string; display_name: string; }

export type MarkupGeometry =
  | { Point: PdfPoint }
  | { Rect: { min: PdfPoint; max: PdfPoint } }
  | { Polyline: PdfPoint[] }
  | { Ink: PdfPoint[][] };

export interface Appearance {
  color: string;
  line_weight: number;
  opacity: number;
  fill: string | null;
  line_style: "Solid" | "Dashed" | "Dotted";
  font: { family: string; size_pt: number } | null;
}

export interface Markup {
  id: string;
  markup_type: string; // e.g. "Rectangle", "Cloud", "MeasurementArea"
  page: number;
  geometry: MarkupGeometry;
  appearance: Appearance;
  subject: string | null;
  layer: string | null;
  contents: string | null;
  audit: {
    created_by: UserRef;
    created_at: string; // RFC3339
    modified_by: UserRef;
    modified_at: string;
    revision: number;
    origin: "Desktop" | "FieldApp";
  };
  workflow: {
    status: "None" | "Accepted" | "Rejected" | "Completed";
    assignee: UserRef | null;
    thread: unknown[];
  };
  measurement: unknown | null;
}

// ---------------------------------------------------------------------------
// Markup + save commands
// ---------------------------------------------------------------------------

export async function addMarkup(doc_id: string, markup: Markup): Promise<void> {
  return invoke<void>("add_markup", { doc_id, markup });
}

export async function listMarkups(doc_id: string): Promise<Markup[]> {
  return invoke<Markup[]>("list_markups", { doc_id });
}

/** Pull existing PDF annotations into the store (call once after open). */
export async function loadMarkups(doc_id: string): Promise<Markup[]> {
  return invoke<Markup[]>("load_markups", { doc_id });
}

export async function saveDocument(doc_id: string): Promise<void> {
  return invoke<void>("save_document", { doc_id });
}

export async function saveDocumentAs(doc_id: string, new_path: string): Promise<void> {
  return invoke<void>("save_document_as", { doc_id, new_path });
}
```

- [ ] **Step 2:** `npm run check` → 0 errors.
- [ ] **Step 3:** Commit: `feat(ipc): markup + save command wrappers and types`

---

### Task 7: Save UI (toolbar buttons + Cmd/Ctrl+S)

**Files:**
- Modify: `src/App.svelte` (READ IT FIRST — integrate with the existing toolbar, `currentDoc` $state, and error-banner pattern)

- [ ] **Step 1: Add** (adapting names to what App.svelte actually uses):
  - Imports: `loadMarkups, saveDocument, saveDocumentAs` from `./lib/ipc`; `save` dialog from `@tauri-apps/plugin-dialog` (the plugin is already installed — `open` is used for Open PDF).
  - After a successful open (where `currentDoc` is set): fire-and-forget `loadMarkups(doc.doc_id).catch(...)` into the existing error path (non-blocking — first render must not wait on lopdf).
  - State: `let isSaving = $state(false);`
  - Handlers:

```typescript
async function handleSave() {
  if (!currentDoc || isSaving) return;
  isSaving = true;
  try {
    await saveDocument(currentDoc.doc_id);
  } catch (e) {
    openError = `Save failed: ${e}`;   // reuse the existing error-banner state
  } finally {
    isSaving = false;
  }
}

async function handleSaveAs() {
  if (!currentDoc || isSaving) return;
  const dest = await save({ filters: [{ name: "PDF", extensions: ["pdf"] }] });
  if (!dest) return;
  isSaving = true;
  try {
    await saveDocumentAs(currentDoc.doc_id, dest);
    currentDoc = { ...currentDoc, path: dest };
  } catch (e) {
    openError = `Save As failed: ${e}`;
  } finally {
    isSaving = false;
  }
}
```
  - Toolbar (next to Open PDF): `Save` and `Save As…` buttons, `disabled={!currentDoc || isSaving}`, matching the existing button classes/CSS-token styling (px units, design tokens — no new hardcoded values).
  - Keyboard: in the existing window-level keydown path (or add one with onMount/onDestroy if none at App level): `(e.metaKey || e.ctrlKey) && e.key === "s"` → `e.preventDefault(); handleSave();`

- [ ] **Step 2:** `npm run check` → 0 errors. `cargo build` still green.
- [ ] **Step 3:** Commit: `feat(ui): Save / Save As + Cmd-Ctrl-S`

---

### Task 8: Corpus-gated end-to-end fidelity test

**Files:**
- Modify: `src-tauri/src/document/save.rs` (append test)

Follow the existing corpus-test pattern in `render/mod.rs` (tests skip with a message when `bench/corpus/` is absent; PDFium tests run with `--test-threads=1`).

- [ ] **Step 1: Write the test** (append to `save.rs` tests; READ `render/mod.rs` tests first and mirror its corpus-path discovery + skip pattern exactly — same helper if one is exported, else same inline shape):

```rust
    /// E2E on the real C1 corpus tier: copy → save with one markup → reload markups →
    /// PDFium still opens and renders a tile from the saved file (fidelity smoke).
    /// Gated like the render corpus tests: skips when the corpus is absent.
    #[test]
    fn corpus_c1_save_roundtrip_and_renders() {
        let Some(c1) = crate::render::tests_corpus_path("c1") else {
            eprintln!("corpus not present — skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("c1-work.pdf");
        std::fs::copy(&c1, &work).unwrap();

        let m = crate::document::annots::tests_support::redline_markup(0);
        save_with_markups(&work, &work, &[m.clone()]).unwrap();

        let got = load_markups_from(&work).unwrap();
        assert!(got.iter().any(|x| x.id() == m.id()), "markup persisted");

        // PDFium fidelity: the saved file must still open + render.
        // Use the same RenderEngine test entry the render corpus tests use.
        crate::render::tests_assert_renders(&work);
    }
```
NOTE for implementer: `tests_corpus_path` / `tests_assert_renders` are the *intent* — `render/mod.rs` almost certainly already has equivalents inside its `#[cfg(test)]` module (corpus dir resolution + open/render-one-tile). Extract/share them behind `#[cfg(test)] pub(crate)` helpers rather than duplicating; match the actual names you find. The assertions above are the contract.

- [ ] **Step 2:** Run WITHOUT corpus path knowledge assumptions:
`cargo test --release corpus_c1_save -- --test-threads=1` → PASS (or visible skip on a corpus-less machine).
- [ ] **Step 3:** Full gate: `cargo test && cargo clippy --all-targets && cargo fmt --check && npm run check` → all green.
- [ ] **Step 4:** Commit: `test(document): corpus C1 save round-trip + render fidelity`

---

### Task 9: Manual verification + ship

- [ ] **Step 1:** `cargo tauri dev` with a small real PDF: open → Save (Cmd+S) → close app → reopen → `loadMarkups` returns the annotation (verify via Save button still enabled + no error; full UI verification lands with S2). Confirm in an external viewer (Preview/Acrobat) that the saved file opens and shows the annotation.
- [ ] **Step 2:** Update `.claude/HANDOVER.md` Current Status (S1 done) and tick the roadmap slice.
- [ ] **Step 3:** Ship: `/sendit` (branch: current `feat/m2-annotation-serde` extended, or new `feat/m2-save-pipeline` if the serde branch merged separately). Run `/code-review` first — this slice touches the save path (risky-diff category).

---

## Self-review (done at authoring)

- **Spec coverage:** §6 persistence map (embed on save) ✓ via Tasks 2-5; §15 atomic sensitive-write ✓ Task 4; §11 round-trip fidelity ✓ Tasks 8-9 (external viewer); foreign-annotation safety (implied by interop goal) ✓ Task 3. Sidecar pieces intentionally NOT here (S4).
- **Placeholder scan:** none; all code real; Task 8's two helper names are explicitly marked as match-what-exists with the contract stated.
- **Type consistency:** `MarkupStore` methods used in Task 5 (`path/list/set_all/set_path/add/register/remove`) all defined in Task 1; `save_with_markups(&Path, &Path, &[Markup])` consistent across Tasks 4/5/8; TS `Markup` mirrors the serde shape of `markup/mod.rs`.
