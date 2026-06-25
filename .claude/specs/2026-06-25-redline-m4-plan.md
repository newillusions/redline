# M4 Implementation Plan — Sets Navigation + Local Versioning + Page Ops + Full-Text Search + OCR

**Date:** 2026-06-25
**Milestone:** M4 (spec §13)
**Status:** Planning — M3 shipped 2026-06-25 (PR #7, eb64651)
**Author:** redline instance

---

## What M4 Is (spec §13 definition)

> Sets navigation + local versioning hooks; page manipulation, layers/hyperlinks, in-doc text search + OCR; folder/library full-text index (Tantivy) — the low-cost doc-management wins (§14)

That covers six distinct capability areas. Broken down from the spec:

| Area | Spec refs | Effort |
|---|---|---|
| Page ops (insert/delete/rotate/reorder/extract) | §4, §14 | Medium |
| In-doc text search (PDFium text extraction) | §4, §14 | Small |
| OCR invisible text layer (Tesseract / leptess) | §3, §14, §16 | Medium |
| Folder/library full-text index (Tantivy) | §3, §4, §14 | Large |
| Sets navigation (.redlineset.json, multi-PDF) | §2, §9, §12k, §18 | Large |
| Local versioning hooks (.redline/history/) | §15, §18 | Medium |

M3 deferred items that also land in M4 (from HANDOVER.md):
- Preset-calibration picker UI (choose saved scale without drawing)
- Perimeter, volume, angle, radius gesture tools
- PDF `/Measure` viewport dictionary write
- Area cutout subtraction

These deferred items are functionally contained within takeoff/measurement, so they slot into **S1** (first slice) alongside page ops setup without blocking the search/OCR/Sets work.

---

## Architecture Decisions

### Decision 1: OCR Engine — leptess (Tesseract via leptonica)

**Chosen:** `leptess = "0.14"` — already specified in Cargo.toml (commented out behind `features = ["ocr"]`), already in CLAUDE.md and spec §3.

**Rationale:**
- Spec §3 and §14 explicitly name Tesseract/leptess. The dependency is already stubbed in Cargo.toml (`# leptess = { version = "0.14", optional = true }`).
- Tesseract is Apache-2.0, matching the zero-cost/commercializable licensing requirement (§16).
- The cross-platform story is sound: Tesseract/leptonica ship as system libraries on macOS (Homebrew) and Windows (vcpkg or bundled). The `leptess` binding handles the C FFI layer.
- Alternative (pure Rust OCR) does not exist at comparable accuracy. Alternatives like `ocrs` (Rust-native, `candle`-based neural net) are emerging but pre-production — Tesseract's quality on construction drawings is well-understood.
- **Key constraint:** leptess requires a tessdata language pack at runtime. The bundle must include `eng.traineddata` (Apache-2.0, ~12 MB). Windows bundles this via `tauri.bundle.resources`; macOS via the same path. Size is acceptable.
- **Desktop/offline fit:** Tesseract runs entirely locally — no API key, no network. Required by the "zero cloud dependency" constraint.

**Implementation shape:** `src-tauri/src/ocr.rs` — `OcrEngine` struct wrapping `LepTess`, exposes `ocr_page(page_image: &[u8]) -> Result<String>`. Page images come from the PDFium render pipeline (rasterize page at 300 DPI for OCR pass). Output text is written as an invisible text layer via lopdf or stored in the Tantivy index (not embedded in PDF for v1 — adding a real invisible text layer to the PDF is deferred to a follow-up; v1 just indexes the OCR output for search).

**Bundles native deps:** Yes. CI (ubuntu-latest) needs `tesseract-ocr libtesseract-dev libleptonica-dev` system packages for the `ocr` feature build. The portable (non-OCR) CI job continues without these. Two CI jobs: `test-rust` (no OCR feature, current) and optionally `test-rust-ocr` (with `--features ocr`, needs apt packages).

### Decision 2: Full-Text Search Index — Tantivy

**Chosen:** `tantivy = "0.22"` — already specified in Cargo.toml (commented out behind `features = ["search"]`), already in CLAUDE.md and spec §3.

**Rationale:**
- Spec §3 explicitly names Tantivy (MIT). Dependency already stubbed in Cargo.toml.
- Pure Rust, no native library deps — compiles cleanly on all targets without system packages.
- Mature and production-proven (powers Quickwit/Meilisearch internals). Context7 confirms full API coverage for IndexWriter + IndexReader + file-watcher incremental re-index pattern.
- **Alternative considered:** SQLite FTS5 (via `rusqlite`). Pros: single-file index, already a common Tauri dep, simpler setup. Cons: FTS5 is document-level (no built-in snippet extraction), no async indexing, weaker relevance scoring, slower on large corpora. For a use case with large construction plan sets (hundreds of PDFs, thousands of pages), Tantivy's segment-based incremental indexing and snippet API are materially better.
- **Alternative considered:** in-memory index on open. Rejected: a 300 MB plan set has thousands of pages; an in-memory search that re-reads text on every query session is too slow and defeats the "across folders/libraries" requirement.
- **Desktop/offline fit:** Tantivy writes the index to a local directory (`.redline-index/` beside the project folder or in `$APPDATA/redline/indexes/`). Fully offline, no server.

**Index schema:**
```
file_path: STRING (stored, fast)
page_number: U64 (stored, fast)
text: TEXT (tokenized, stored for snippets)
source: STRING (stored) — "pdfium" | "ocr"
indexed_at: U64 (stored, fast)
```

**Re-index strategy:** `notify` crate (already a Tauri ecosystem dep) watches the indexed folder for `Create`/`Modify`/`Remove` events. On change, a background tokio task re-indexes the affected file. First-time index of a large folder is a background job with progress events to the frontend.

### Decision 3: Local Versioning Model — Full-PDF Snapshot in `.redline/history/`

**Chosen:** Full PDF copy per save, retained-N (default 10), stored in `.redline/history/`.

**Rationale:**
- Already fully specified in §18 of the spec, including the filename convention (`0007__2026-06-06T14-30-00Z__u_9f3.pdf`) and the `meta.json` versions array.
- `meta.json` already exists (M3 shipped it for scale persistence) — versioning adds to the same file under the `versions` key.
- **Full PDF copy vs delta:** spec §18 explicitly says "full PDF copies in `history/` for v1 (simple, robust); delta storage is a later optimization." This is not a new decision.
- Full copies are safe and predictable on 100s-MB plan sets: retain-10 = worst case 10x the file size in history. On a 50 MB average plan, that's 500 MB of history per file. Acceptable for internal use; the retain-N limit controls growth. A background prune job trims to the limit on save.
- **Desktop/offline fit:** pure local disk writes, no external service, atomic (temp-file + rename per §15).

**Version trigger:** every `save` command that modifies the PDF creates a snapshot before the write, keeping the pre-edit state. User-named snapshots ("pre-issue", "client version") are a minor add-on to `meta.json` `label` field.

---

## M4 Slices (sequenced)

M4 decomposes into 7 slices. Each is independently shippable (passing tests, merged to main). They follow a dependency order but each builds on a stable main.

### S1 — Page Ops + M3 Deferred Takeoff Items

**What:** `document::page_ops` module — insert, delete, rotate, reorder pages via `lopdf`. Plus the M3 deferred measurement tools (preset-calibration picker, perimeter/volume/angle/radius gestures, PDF `/Measure` write, area cutout).

**Why first:** Page ops touch the document model and save pipeline, which must be stable before versioning (S2) snapshots the PDF. The deferred takeoff items are bounded and don't block anything else; clearing them keeps the M3 backlog from aging.

**Files touched:**
- `src-tauri/src/document.rs` — add page manipulation functions
- `src-tauri/src/lib.rs` — Tauri commands: `insert_page`, `delete_page`, `rotate_page`, `reorder_pages`
- `src/lib/ipc.ts` — IPC wrappers for page commands
- `src/components/ThumbnailPanel.svelte` — page reorder drag UI (new component)
- `src-tauri/src/takeoff.rs` — preset calibration picker, additional geometry gestures
- `src/components/CalibrationDialog.svelte` — preset picker mode

**Test gate:** `cargo test` covers page-ops pure logic (rotate/reorder/delete invariants); vitest covers IPC type wrappers and ThumbnailPanel interactions.

**Dependencies on this slice:** S2 (versioning hooks into save, which page-ops touches).

---

### S2 — Local Versioning Hooks

**What:** Version snapshot on save. Reads/writes `meta.json` `versions` array; copies pre-save PDF to `.redline/history/`; prunes to retain-N; exposes version list + restore to UI.

**Why here:** Depends on stable page-ops and save pipeline from S1. Must land before Sets (S5) so Sets navigation can show per-document version state.

**Files touched:**
- `src-tauri/src/storage.rs` — `save_with_version()`, `list_versions()`, `restore_version()`
- `src-tauri/src/lib.rs` — Tauri commands for version ops
- `src/components/VersionPanel.svelte` — version history list, restore button (new component)
- `src-tauri/src/sidecar.rs` (or `meta.rs`) — extend `meta.json` schema for `versions`

**Test gate:** Rust unit tests: snapshot-on-save, prune-to-N, restore atomicity. Frontend tests: VersionPanel render + interaction.

---

### S3 — In-Doc Text Search (PDFium)

**What:** Per-document text search using PDFium's text extraction. Search panel UI, result highlighting in Viewport, page-jump on click.

**Why here:** Self-contained — PDFium text extraction is already wired (pdfium-render is a dep). No new system deps. Fast win, unblocks the search UX pattern that S4 (Tantivy) extends.

**Files touched:**
- `src-tauri/src/text.rs` (new) — `search_text(page: u32, query: &str) -> Vec<SearchHit>`; `SearchHit { page, rects: Vec<[f64;4]>, snippet: String }`
- `src-tauri/src/lib.rs` — `search_document` command
- `src/components/SearchPanel.svelte` (new) — query input, result list, page jump
- `src/components/Viewport.svelte` — search hit highlight rects overlay

**Test gate:** Rust: text extraction + search hit detection on a fixture PDF. Frontend: SearchPanel render, result click navigation.

---

### S4 — Full-Text Search Index (Tantivy + file watcher)

**What:** Folder/library full-text index via Tantivy. Background indexer, incremental re-index via `notify` file watcher. Search results across all files in a folder — file + page + snippet. UI: SearchPanel extended to show cross-file results.

**Why here:** Builds on S3's SearchPanel and search hit UX. Tantivy is already in Cargo.toml (commented out). This is the largest single-slice effort.

**Files touched:**
- `src-tauri/src/search.rs` (new) — `FolderIndex` struct; schema, `index_folder()`, `search_folder()`, file watcher setup
- `src-tauri/Cargo.toml` — uncomment `tantivy` dep, enable `search` feature
- `src-tauri/src/lib.rs` — `open_folder_index`, `search_folder`, `index_status` commands
- `src/components/SearchPanel.svelte` — cross-file results mode, indexing progress indicator
- `src/lib/ipc.ts` — FolderIndex IPC types

**Tantivy index location:** `$APPDATA/Redline/indexes/<folder_fingerprint>/` — keeps index out of the watched folder itself, avoids re-trigger loops.

**Test gate:** Rust: index-and-query on a temp dir fixture; re-index on file change; delete removes doc from index. Frontend: SearchPanel cross-file results display.

**Task link:** task:mb4a5139a5kh6ud27gnp ("Add folder-wide full-text search across all files in a project folder") — this slice fulfils that task.

---

### S5 — OCR Invisible Text Layer (leptess)

**What:** Per-page OCR pass for scanned/image PDFs. Output indexed into Tantivy (S4 must be merged first). On-demand per-page and batch-all modes. UI: "Index this file (OCR)" button in SearchPanel.

**Why here:** Depends on S4's Tantivy index (OCR output is written to the index, not into the PDF for v1). leptess requires native system libs — CI needs a separate `test-rust-ocr` job with apt packages.

**Files touched:**
- `src-tauri/src/ocr.rs` (new) — `OcrEngine`, `ocr_page(image: &[u8]) -> Result<String>`
- `src-tauri/Cargo.toml` — uncomment `leptess` dep, enable `ocr` feature
- `src-tauri/src/search.rs` — extend `index_folder` to accept OCR text alongside PDFium text
- `src-tauri/src/lib.rs` — `ocr_page`, `ocr_document` commands
- `src/components/SearchPanel.svelte` — "OCR and index" button, progress events
- `.forgejo/workflows/ci.yml` — add `test-rust-ocr` job with tesseract apt packages

**Tessdata bundling:**
- `src-tauri/tauri.conf.json` — add `eng.traineddata` to `bundle.resources`
- `src-tauri/build.rs` — download tessdata if absent (same pattern as PDFium)

**Test gate:** Rust (ocr feature): OCR a fixture image, verify non-empty text output. Confidence threshold gate: skip pages below 30% mean confidence (likely blank/graphical). Frontend: OCR progress indicator display.

---

### S6 — Sets Navigation (.redlineset.json)

**What:** Multi-PDF Set definition and navigator. Create/open a Set; thumbnail navigator across all member PDFs; sheet labels from PDF `/PageLabels`; click to open member PDF at page.

**Why here:** Depends on S2 (versioning) so each member's version state is visible. Large frontend effort (Sets navigator panel with cross-document thumbnails).

**Spec refs:** §9, §12k, §15, §18.

**Files touched:**
- `src-tauri/src/sets.rs` (new) — `RedlineSet` struct, `create_set`, `open_set`, `add_member`, `list_members`, `get_page_labels` commands
- `src-tauri/src/lib.rs` — Set commands
- `src/components/SetsPanel.svelte` (new) — Set navigator, member list, thumbnail strip
- `src/lib/sets-store.svelte.ts` (new) — `SetsStore` (active Set, member state, page jump)
- `src/App.svelte` — SetsStore lifecycle, SetsPanel wiring

**`.redlineset.json` format:** already fully specified in §18 of the spec.

**Test gate:** Rust: create/open/add-member/list-members; page label extraction. Frontend: SetsPanel render, member click navigation.

---

### S7 — PDF Layers (OCG show/hide) + Hyperlinks

**What:** OCG layer show/hide via PDFium's optional content group API. Sheet-to-sheet hyperlinks (click an annotation link to jump to another page/file). Both are high-value on construction drawings.

**Why last:** Independent of S1-S6 but easiest to layer on once the viewport and Sets navigation are stable. Low risk, bounded scope.

**Files touched:**
- `src-tauri/src/document.rs` — `list_ocg_layers()`, `set_layer_visibility()`
- `src-tauri/src/lib.rs` — OCG commands
- `src/components/LayersPanel.svelte` (new) — OCG layer list with visibility toggles
- `src/components/Viewport.svelte` — hyperlink annotation detection + click handler (open page or file)

**Test gate:** Rust: OCG list on a fixture with layers; toggle visibility. Frontend: LayersPanel render + toggle interaction; hyperlink click to page jump.

---

## Sequencing Summary

```
S1 (Page Ops + M3 Deferred)
  └─ S2 (Versioning)
       └─ S5 (OCR)  ← depends on S4 index
S3 (In-doc search)
  └─ S4 (Tantivy folder index) ← task:mb4a5139a5kh6ud27gnp
       └─ S5 (OCR)
S2 (Versioning) ← also needed before S6
  └─ S6 (Sets Navigation)
[S7 independent]
```

Parallel path: S1→S2 can run in parallel with S3→S4. S5 waits for both S2 and S4. S6 waits for S2. S7 is independent.

**Recommended order:** S1, S3, S2, S4, S5, S6, S7
- S1 first (clears M3 deferred, stabilises page-ops for versioning)
- S3 next (quick win, establishes search UX pattern)
- S2 after S1 (versioning on stable save pipeline)
- S4 after S3 (extends search to folder-wide)
- S5 after S2+S4 (OCR needs both versioning pipeline and Tantivy index)
- S6 after S2 (Sets needs versioning per-member state)
- S7 at any point after S3 (viewport work is self-contained)

---

## First Shippable Slice: S1

S1 is the first slice because it clears the M3 deferred backlog and establishes the page manipulation foundation that S2 (versioning) depends on. It touches the most bounded set of files and has no new external dependencies.

**What to build:**
1. `document::page_ops` Rust module: `rotate_page(doc, page, degrees)`, `delete_page(doc, page)`, `reorder_pages(doc, new_order: Vec<u32>)`, `insert_page(doc, at, source_doc, source_page)`
2. Tauri commands: `rotate_page`, `delete_page`, `reorder_pages`, `insert_page`
3. `ThumbnailPanel.svelte` — basic thumbnail strip with drag-to-reorder
4. M3 deferred: preset calibration picker (add "Use saved scale" mode to `CalibrationDialog`), additional gesture tools (perimeter = polyline length, volume = area * depth input, angle = two-line bearing), PDF `/Measure` write (embed standard measurement dict on save), area cutout (subtract inner polygon from outer polygon in area calc)

**TDD approach:** Write Rust tests for page-ops invariants first (rotate 4x = identity; delete then count decreases; reorder is a permutation). Write vitest tests for ThumbnailPanel drag-reorder before implementing.

**Estimated scope:** ~40-60 Rust test additions, ~20-30 FE test additions.

---

## Cross-Project and Hardware Dependencies

1. **§20 floor-machine run (owed from M2):** The indicative §20 PASS on Apple Silicon stands in; the formal run (16 GB Windows + macOS, `bench/RUNBOOK-S20.md`) is still owed. M4 page ops add new document manipulation surface that should be included in a §20 re-run. Recommend scheduling the floor run before S6 (Sets navigation adds multi-document load paths).

2. **Acrobat/Bluebeam visual check (owed from M2):** `/tmp/redline-g9-sample.pdf` still needs a human visual check for annotation round-trip fidelity. This is independent of M4 but should be cleared before M6 (compare) work.

3. **tessdata bundling:** OCR (S5) requires `eng.traineddata` (~12 MB) in the Tauri bundle. Tauri's `bundle.resources` mechanism handles this, but the `src-tauri/build.rs` download step needs to be validated on both macOS and Windows before S5 ships.

4. **Windows CI runner:** S5 OCR requires `leptess` + Tesseract on Windows. The workspace's NUC runs a Windows runner (`ai-server` label seen in cad-export), but a Windows Tesseract install via Chocolatey/vcpkg in CI is needed. This is S5-specific; S1-S4 and S6-S7 are unaffected.

5. **`notify` file watcher:** Used by S4's incremental re-indexer. Already a Tauri ecosystem dep (bundled via `tauri-plugin-fs`), so no new top-level dep. Verify the `notify` version matches what Tauri 2.5 bundles before adding it directly.

---

## Library Decision Summary

| Decision | Choice | One-line rationale |
|---|---|---|
| OCR engine | `leptess 0.14` (Tesseract) | Spec-mandated, already stubbed in Cargo.toml, Apache-2.0, fully offline, best OCR quality on construction drawings |
| Full-text search | `tantivy 0.22` | Spec-mandated, already stubbed in Cargo.toml, pure Rust (no native deps), incremental indexing + snippet API, better than SQLite FTS5 for large corpora |
| Local versioning | Full PDF copy in `.redline/history/` | Spec-mandated (§18), already modelled in meta.json schema, simple + robust for v1, retain-N controls growth |
