# Bluebeam Alternative — v1 Technical Specification

**Status:** Draft for Claude Code build handoff
**Scope:** Internal-use desktop tool; near-zero-cost stack; architecture kept open for possible future commercialization
**Last updated:** 2026-06-06 (all §12 decisions resolved; consultant/reviewer focus locked; markup model + audit/identity (§6), measurement/scale schema (§7), review workflow promoted to core M2 (§13), and the sidecar format (§18) all specified; large-file perf flagged as an early M1 spike)

---

## 1. Purpose

A cross-platform (Windows + macOS) desktop application for AEC PDF **markup, takeoff, and document management**, built to replace internal Bluebeam Revu seats. Optimized for working with very large construction plan sets (vector-heavy, often 100s of MB). Licensing cost for v1 must be effectively zero.

**Primary audience: consultants and reviewers** (design review, QA/checking, mark-up handoffs) rather than contractors/estimators. So **markup, comments/notes, and review workflow are the core**; measurement/takeoff is first-class but supporting (see §2, §7).

## 2. Goals / Non-goals

**Primary focus:** the core job is **markup, comments/notes, and review-QA for consultants and reviewers** (not contractor estimating). Measurement/takeoff is first-class but supporting.

**Goals (v1)**
- Fast viewing of large plan sets
- **Full markup/annotation authoring + review workflow (core)**
- **Review-grade** measurement/takeoff with quantity rollups + summary export (supporting — §7)
- Multi-file handling with Sets navigation
- Flatten, file-size reduction, redaction (redaction modular, see §8)
- Local save with version hooks (to enable compare)
- Import existing Bluebeam Tool Sets (`.btx`) and stamps
- Full-text search: in-document, across a Set, and across folders/libraries

**Non-goals (for now)** — but architecture must not preclude them:
- Real-time multi-user collaboration (Studio Sessions equivalent)
- Cloud sync / hosting
- Commercial distribution (keep module boundaries clean + proprietary deps swappable so this stays possible)

*Future direction:* Field tools (punch lists, issue/RFI tracking, status pinned to plan locations) are planned **post-v1 as a separate mobile/tablet app**, mirroring Bluebeam's Revu-desktop / Cloud-field split. Tauri 2 ships iOS/Android from the same Rust core, so the `document` / `markup` / `storage` logic can be shared rather than rewritten. The genuinely new infrastructure that implies is an **offline-first store + async sync layer** (field edits flowing back to the central store) — *not* real-time collaboration. v1 must keep the data model portable and sync-ready (see §6) so this lands without a retrofit.

## 3. Tech stack (pinned)

- **Shell:** Tauri 2.x (Rust core + OS webview)
- **Frontend:** Svelte 5 + Vite (SPA mode), TypeScript
- **UI docking:** `dockview-core` (MIT) — reconfigurable panels (see §17); `svelte-splitpanes` as lighter alternative
- **Render engine:** PDFium via `pdfium-render` 0.8.x (BSD-licensed)
- **Low-level PDF ops:** `lopdf`
- **OCR:** Tesseract (Apache-2.0) via `leptess` (rust binding)
- **Full-text search:** Tantivy (MIT) — folder/library index
- **Document-surgery backend:** trait-based, swappable — free baseline for v1; MuPDF (commercial) or Apryse Advanced pluggable later
- **Targets:** Windows x64, macOS (universal)

## 4. Architecture

**Rust core (Tauri backend)**
- `render` — PDFium tiled rasterization, page/tile cache, zoom levels (**display only**)
- `geometry` — vector path extraction from PDF page objects (PDFium *transformed* path segments), spatial index of snap targets (endpoints / vertices / intersections / midpoints / arc & circle centers) — all in PDF user space
- `document` — PDF parse/model, open/save, page manipulation (insert/delete/rotate/reorder/extract/crop/merge) (PDFium + `lopdf`)
- `text` — PDFium text extraction + search (within doc and across a Set)
- `ocr` — Tesseract (`leptess`) invisible-text-layer generation for scanned pages
- `search` — folder/library full-text index (Tantivy) over extracted + OCR'd text; incremental re-index via file watcher (`notify`); query → file / page / snippet
- `markup` — annotation model, serialize markups → standard PDF annotations
- `takeoff` — scale calibration, measurement types, quantity calc; consumes `geometry` for snapping, all math in PDF user space at f64
- `docops` *(swappable)* — flatten / optimize / redact behind one trait
- `compare` — page-pair alignment + diff rendering (Phase 1.1)
- `storage` — local-first file & version management

**Frontend (Svelte webview)**
- Viewport + markup overlay (canvas/SVG over rendered tiles)
- Markup tools + Tool Chest / Profiles (reusable custom tools)
- Takeoff UI (calibration, measurement tools, summary table)
- Sets navigator (thumbnails, page labels, bookmarks)
- Compare UI (Phase 1.1)
- **Dockable 3-column panel system** + collapsible bottom Markups/Comments list (see §17); toolbars

**Bridge:** Tauri commands (tile requests, markup persistence, docops calls) + events for async render/progress.

## 5. Rendering pipeline

Render in Rust, not in the webview. Flow: webview requests visible tiles at current zoom → Rust rasterizes via PDFium → tiles streamed to a canvas → markups drawn as a vector overlay on top. Maintain a tile cache keyed by (page, zoom, region). Keep a clean coordinate mapping between PDF user space and screen space (needed for accurate markup + measurement). **Validate against 300 MB+ plan sets early — this is the make-or-break performance test.**

**Display vs. geometry — two independent layers (precision-critical).** The raster tiles are *display only*, and they are sharp, not blurry: each tile is rendered at the resolution matching the current zoom and device pixel ratio, then re-rendered on zoom change — never upscaled. Snapping and measurement **never read the raster**. In parallel, the `geometry` module extracts the page's real vector paths in PDF user space and indexes snap targets. On cursor move, the position is transformed screen → PDF user space, queried against the index within a pixel tolerance, and snapped to the *exact* vector coordinate; all measurement math runs in user space at full f64 precision, independent of zoom or tile resolution. Use `pdfium-render`'s *transformed* path-segment iteration so coordinates compose parent matrices correctly (paths nested in Form XObjects otherwise return near-(0,0)).

## 6. Markup model, tools & stamps

Supports the full annotation set **and** measurement metadata from day one (takeoff is full-scope in v1). Markups serialize to standard PDF annotations on save/flatten for interoperability (they open correctly in Bluebeam/Acrobat); app-only data that doesn't belong in the annotation dict lives in the sidecar (§15).

**v1 markup types (locked — decision (a), §12):** text, callout, cloud, rectangle/ellipse/polygon, line/polyline/arrow, highlight, pen/ink, stamp, plus measurement markups (§7). Grouping is in; snapshot / region-copy is deferred (§14).

**Markup record — common envelope (every markup):**
- `id` — stable UUID, assigned at creation, **immutable**; the sync/merge anchor → PDF `/NM` annotation name on save
- `type`, `page`, `geometry` (PDF user space, f64), `appearance` (color / weight / opacity / fill / line-style / font)
- `subject` (drives summary grouping → `/Subj`), `layer` (optional OCG / logical), `contents` (note text → `/Contents`)

**Audit & attribution (every markup):**
- `created_by` — **user identity = stable `user_id` (UUID) + display name**, not a bare name string
- `created_at` (ISO-8601 UTC), `modified_by`, `modified_at`, `revision` (monotonic, bumped per edit), `origin` (desktop vs future field app — sync provenance)
- **User identity source (decision, §12):** *app-configured stable identity* — on first run the app generates a `user_id` (UUID) + an editable display name defaulting to the OS username, and embeds both. No auth system in v1; the `user_id` shape stays compatible with real accounts / SSO when the post-v1 sync / collaboration layer lands.
- **Audit trail (decision, §12):** the annotation embeds creator + last-modified (display name + `user_id`); the sidecar (§15) keeps a **full append-only history** — create / edit / status-change / **delete-as-tombstone**, each with `user_id` + timestamp — so deletions leave a record and the audit has no holes. The sidecar also holds the `user_id ↔ display-name` registry so renames never orphan attribution.

**Measurement extension** (measurement markups): `scale_ref`, `raw_measure`, `unit`, `computed_quantity`, `depth` (volume), `custom_columns` (estimating, §7).

**Portability / sync-readiness (design in now):** the stable `id`, clean serialization, and **reserved workflow fields** (status, assignee, thread/replies — unused in v1; status value embeds + history in sidecar per decision (f)) mean a field-tool "issue" is just a markup with workflow state. The future mobile app + async sync layer reuse this model rather than forcing a rework.

**Persistence mapping (per decision (f), §15):** *embed in the annotation* → `id`→`/NM`, display name→`/T`, `created_at`→`/CreationDate`, `modified_at`→`/M`, `contents`→`/Contents`, `subject`→`/Subj`, plus `user_id`, status value, and custom columns as app-namespaced keys. *Sidecar* → append-only audit history, status-change history, replies/threads, version metadata, and the user-id registry.

### Tools & Tool Sets

A **tool** is a serialized markup template — a markup type plus its saved properties (color, line weight, opacity, font, fill, line style, subject; for measurement tools, the scale/depth/unit settings), optionally with fixed geometry. Because the markup model above is already clean, portable, and serializable, tool sets fall out of it with little new machinery.

- **Two placement modes** (matching Bluebeam): *Properties mode* applies the tool's properties to newly drawn geometry; *Drawing mode* drops an exact copy (used for symbols/stamps).
- **Tool Set** = a named, ordered collection of tools, serialized to a shareable file (own versioned JSON format; sync-friendly per §2), plus an auto-populated *Recent Tools* set.
- Measurement tools are tools too (a calibrated area tool with a fixed subject/depth is just a measurement-markup template) — no special-casing.
- Frontend Tool Chest panel lists sets/tools; the active tool drives markup creation.

### Stamps

A specialized tool type, in two forms:

- **Static stamp** — image- or vector-backed markup placed as a PDF stamp annotation (Subtype `/Stamp`) with an appearance stream. Prefer **vector/PDF or SVG** sources so stamps stay crisp at any zoom (per §5); raster (PNG) supported but pixelates.
- **Dynamic stamp** — appearance generated *at placement* from a template with auto-populated fields (date, time, datetime, username, sequential auto-number, document name, prompted text/dropdown). Implement by **composing the appearance ourselves** (text + graphics → appearance stream, values substituted at placement) — **not** via embedded PDF JavaScript/form-field scripting, which is brittle. Once placed, computed values bake into a static appearance, so it flattens and round-trips cleanly (per §8). Sequence counters need persistent state — **per-document by default, selectable to a global/project-wide counter per stamp; counter state persists in the sidecar (§15)** (decision (c), §12).

### Importing Bluebeam Tool Sets & stamps (v1 requirement)

Confirmed in scope for v1. Feasibility is good: `.btx` is an **XML / UTF-8 text** format, not an opaque binary. Each `<ToolChestItem>` carries a `<Name>` (id), a `<Type>` (e.g. `Bluebeam.PDF.Annotations.AnnotationFreeText`), a `<Mode>` (`properties` / `drawing` — maps to our two placement modes), `<BSIColumnData>` (custom columns), and a `<Raw>` payload that is **the markup written as a PDF annotation dictionary** (`/Subtype/FreeText /Rect[...] /CL[...] /Subj(...)` etc.).

That last point is the win: `<Raw>` maps almost directly onto our markup model, because we already parse and emit standard PDF annotations. The importer largely **reuses the annotation parser we're building anyway** — parse XML → read Type/Mode/columns → hand `<Raw>` to the annotation reader → construct a tool template.

Wrinkles (all manageable):
- **zlib blobs** — some `<Raw>` / `<Script>` payloads contain hex beginning `789c` (zlib-deflated); inflate before parsing.
- **Packaging** — tool sets / stamps are often distributed zipped; handle `.zip`-wrapped files.
- **Stamps** — imported as PDFs. *Static* stamps import directly (we place PDF-backed stamps anyway). *Dynamic* stamps embed form fields + JavaScript; we do **not** execute their JS — import the static appearance and map recognizable auto-fields (date / user / sequence) onto our dynamic-stamp model, falling back to a static stamp where a field can't be mapped.
- Prior community decoding tools exist, so this is a **parser project, not from-scratch reverse engineering**.

Validate the importer early against a library of real-world `.btx` files and stamps (measurement tools, custom columns, embedded images).

## 7. Takeoff & measurement (review-grade, supporting)

**Scope note:** primary audience is consultants/reviewers, not contractors — measurement is here to *check and annotate* (verify a dimension, confirm an area, count fittings), not to drive contractor-grade estimating. The full type set is supported, but this is a **supporting** capability; markup/review (§6) is the core (priority per §2, decision (i) §12).

**Scale / calibration (sidecar-persisted, embedded on save — decision (j), §12):**
- A `scale` record **per page**, with a `document_default` fallback and an **apply-to-range / apply-to-all** helper (calibrate one sheet, propagate across the set) — construction sets carry different scales per sheet.
- Methods: `two_point` (known-length two-point calibration) · `page_declared` (read the PDF `/Measure` viewport if present) · `preset` (1:100, 1/4"=1'-0", …).
- Fields: `id`, `applies_to`, `method`, `ratio` (real-world units per PDF point, f64), two-point def (`p1`/`p2` in PDF user space, `known_length`, `known_unit`), `unit` (+ `imperial_arch` flag for feet-inches), `label`, `precision`, audit (`created_by`/`created_at` — §6 identity model).
- **Persistence:** the **sidecar is the source of truth**; on save we **also write the standard PDF `/Measure` viewport** per page so scale + measurements reproduce in Bluebeam/Acrobat.

- **Measurement types:** length, perimeter, area, volume (area × depth), count, angle, radius. Area supports **cutouts** (subtract openings, §14).
- **Per-measurement storage (extends the §6 markup envelope):** `measure_type`, `scale_id` (the calibration used), `raw_measure` (**scale-independent** value in PDF user space — length = points, area = points², angle = °, count = n), `computed_quantity` + `unit` (raw × ratio, converted), `depth` (volume), `count_value`, `custom_columns` + `subject` (estimating extras / grouping — secondary).
- **Recompute invariant:** `raw_measure` is stored scale-independent and references a `scale_id`, so recalibrating a scale **deterministically recomputes** every dependent measurement — no stale or lost quantities when a scale is corrected.
- **Rollups:** live quantity totals grouped by subject/layer; unit handling + conversions (default **metric** mm/m; full imperial incl. architectural feet-inches). Useful for review, secondary to markup.
- **Snapping & precision:** snap to extracted vector geometry via the `geometry` module; all measurement computed in PDF user space at f64, independent of zoom/raster resolution (see §5). No vector snapping on scanned plans (§11/§12).

**Markup List export (critical, first-class v1 feature).** Distinct from measurement totals: export the *full* markup list — every annotation with author, date, comment/note text, status, color, layer, and measurement columns — with user-selectable columns, to **XLSX / CSV (M3), PDF summary fast-follow** (decision (b), §12). This is a core QA and handoff artifact (the equivalent of Bluebeam's Markup List export) and a hard requirement, not a takeoff convenience.

## 8. Document operations (swappable module)

`DocOps` trait: `flatten()`, `optimize(level)`, `redact(regions)`.

- **v1 baseline (free):**
  - flatten — merge annotation appearance streams into page content
  - optimize — strip unused objects + recompress streams (`lopdf`); *note: deep image downsampling is out of scope for the free baseline*
  - redact — **rasterize-the-region fallback** (safe, removes underlying data, lossy/non-searchable in that region)
- **Pluggable backend (later):** MuPDF (commercial license) or Apryse Advanced for true vector redaction + high-quality image-downsampling optimization. Slots in behind the same trait; no caller changes.

> Rationale: true redaction and deep file-size reduction (image downsampling) are correctness-/liability-sensitive and painful to hand-roll. The trait keeps v1 at zero cost while making the upgrade a drop-in.

## 9. Sets & document handling (v1)

Open multiple PDFs; combine and navigate as an ordered **Set** (page labels, thumbnails, bookmarks). Local file save. Storage is local-first, but design **version hooks now**: retain the last N revisions (configurable, default ~10) in a per-file revision store, markups versioned with the document snapshot (decision (e), §12), so Compare lands without a storage retrofit.

## 10. Compare & overlay (Phase 1.1 — fast follow)

- **Alignment:** manual two-point registration first; auto content-based registration later
- **Diff rendering:** color-channel overlay (old vs new in contrasting colors) + optional change-highlight
- Reuses render tiles; isolated in the `compare` module

## 11. Key risks & mitigations

- **Large-file performance (DEEP-DIVE REQUIRED — explore early, before building on top)** → Large drawing sets are in scope **from the outset**, so the memory + rendering strategy must be proven early in the dev process, not deferred. Approach: tiled render + bounded tile cache + streaming (never fully load 100s-MB files into memory). Treat as a **dedicated M1 spike**: on the largest *real* plan sets, measure peak memory, tile-render latency at each zoom level, and cache-eviction behavior — and prove the budget holds — *before* layering markup/takeoff on top. Make-or-break gate. (Tracked as a KB task assigned to redline.)
- **Redaction correctness** → never ship a drawn "black box"; rasterize fallback is the v1 safe floor; real redaction only via mature engine
- **Annotation round-trip fidelity** → serialize to standard PDF annotations; verify in Bluebeam/Acrobat
- **Compare alignment accuracy** → start manual; validate on real plan revisions
- **Snapping needs vector geometry** → vector-snap precision works on vector PDFs (CAD/Revit exports); scanned/raster plans have no geometry to snap to (Bluebeam shares this limit) → fall back to calibrated manual placement. **Raster corner/edge-detection snapping is explicitly post-v1** (decided 2026-06-06, §12/§14); the v1 scanned-plan story is OCR-for-search + manual placement.
- **Dynamic-stamp import fidelity** → Bluebeam dynamic stamps embed JavaScript we won't execute; map recognizable auto-fields and import the static appearance as fallback; validate against a real stamp library

## 12. Decisions (resolved 2026-06-06)

All v1 open decisions are now locked. (Date noted so later changes stay visible.)

- **Field tools — deferred:** post-v1 standalone mobile/tablet app sharing the Rust core (see §2 *Future direction* and §13). Not in the v1 desktop build. v1 obligation: keep the markup/data model portable and sync-ready (§6).
- **Bluebeam migration — v1 requirement:** importing `.btx` Tool Sets and stamps is in v1; format is XML/UTF-8 and the approach reuses the annotation parser (see §6).
- **(a) v1 markup type list — locked to the §6 set:** text, callout, cloud, rectangle/ellipse/polygon, line/polyline/arrow, highlight, pen/ink, stamp, plus measurement markups (§7). Grouping is in. **Snapshot / region-copy stays deferred** (§14).
- **(b) Summary / Markup-List export priority — XLSX + CSV in M3, PDF summary as fast-follow.** XLSX and CSV share the same tabular model (CSV is near-free); the PDF summary needs layout work and follows.
- **(c) Dynamic-stamp auto-fields + counter scope:** v1 auto-fields = date, time, datetime, username, document name, sequential auto-number, prompted text/dropdown. **Sequence-counter scope = per-document by default, selectable to a global/project-wide counter per stamp.** Counter state persists in the sidecar (§15).
- **(d) Profiles scope:** v1 persists the single current panel layout (the layout JSON *is* the Profiles mechanism, §17). **Named, switchable multi-workspace Profiles are post-v1.**
- **(e) Versioning model depth — lightweight retained-N:** retain the last N revisions (configurable, default ~10) in a per-file revision store; markups are versioned with the document snapshot. Enough to feed Compare (Phase 1.1, §10); not a full VCS. (§9, §15)
- **(f) Workflow-status persistence — hybrid:** the status *value* (Accepted/Rejected/Completed) embeds in the PDF annotation (travels with the file; Bluebeam-compatible custom field); status-change *history* and replies/threads live in the sidecar (§15).
- **(g) User identity — app-configured stable identity:** on first run the app generates a stable `user_id` (UUID) + an editable display name (default = OS username), and embeds both on every markup. No auth system in v1; shape stays compatible with real accounts/SSO when post-v1 sync/collaboration lands. (§6, §15)
- **(h) Audit trail — full append-only history incl. deletions:** every markup embeds creator + last-modified; the sidecar keeps an append-only log of create/edit/status-change/**delete (tombstone)** with `user_id` + timestamp, so the audit has no holes. (§6, §15)
- **(i) Primary audience & priority:** consultants/reviewers, not contractors — **markup / comments / review-QA is the core**; takeoff/measurement is first-class but supporting. (§1, §2, §7)
- **(j) Scale persistence:** the **sidecar is the source of truth** for per-page scales (with a document-default fallback + apply-to-range/all); on save we **also embed the standard PDF `/Measure` viewport** per page so measurements reproduce in other tools. (§7)
- **Scanned-plan snapping:** v1 = OCR-for-search + calibrated manual placement only. **Raster corner/edge-detection snapping is post-v1** (§11, §14) — same limit as Bluebeam, accepted.

## 13. Suggested build order (milestones)

- **M1** — Tauri + Svelte shell incl. 3-column dockable layout (§17); PDFium tiled render; open/pan/zoom a large PDF smoothly
- **M2 (the core — reviewers' primary surface)** — Markup overlay + core annotation types + comments/notes + save to PDF annotations; **review workflow: status (Accepted/Rejected/Completed) + the Markups/Comments list panel with sort/filter (§17)** — first-class, not an add-on, because reviewers are the primary audience (§12 i); Tool Chest / Tool Sets + stamps (static + dynamic) build directly on the markup model; **`.btx` / stamp importer** (reuses the annotation parser)
- **M3** — Takeoff & measurement (review-grade, §7): scale calibration, measurement types, rollups; **Markup List export (XLSX/CSV — the reviewer's primary deliverable) + measurement summary export**
- **M4** — Sets navigation + local versioning hooks; page manipulation, layers/hyperlinks, in-doc text search + OCR; folder/library full-text index (Tantivy) — the low-cost doc-management wins (§14)
- **M5** — `docops` baseline (flatten / optimize / rasterize-redact) behind the trait
- **M6 (Phase 1.1)** — Compare & overlay
- **Later** — pluggable mature `docops` engine; **field tools as a standalone mobile/tablet app** (shared Rust core + offline-first store + async sync); (optional) collaboration / cloud / commercial packaging

## 14. Additional capabilities (review pass)

Gap review against Bluebeam Revu, split by cost. These augment the milestones above (most slot into M2/M4/M5).

### Low-cost — recommended in v1
- **OCR** — searchable scanned/image PDFs. Tesseract (Apache-2.0) via `leptess`: rasterize → OCR → write an *invisible* text layer. On-demand/batch (per-page cost is non-trivial); imperfect on drawings; adds native tesseract + leptonica deps to bundle.
- **Text search (in-doc, Set, and folder/library)** — within a document and across a Set via `pdfium-render`; and **across whole folders/libraries via a persistent full-text index (Tantivy, MIT)** — a flagship requested feature. Background/incremental indexing of extracted + OCR'd text (file watcher for changes); results give file + page + snippet, click to open at the page. First-time indexing of a large library is a background job; OCR makes it slower (best as an optional background pass).
- **Page manipulation** — insert/delete/rotate/reorder/extract/crop/split/merge. Free via PDFium + `lopdf`.
- **PDF layers (OCG) show/hide** — high-value on construction drawings; free via PDFium (`/OC` already appears in `.btx` raw, so it ties into import).
- **Hyperlinks** — sheet-to-sheet / region links; big for Set navigation.
- **Markup status / review workflow — PROMOTED TO CORE (M2):** Accepted/Rejected/Completed + history; sort/filter the markup list by status. Given the consultant/reviewer focus (§12 i) this is **no longer an optional add — it is first-class M2 scope**. Status value embeds in the annotation, history in the sidecar (§12 f, §18). Kept in this list only for the gap-review record.
- **Area cutouts** — subtract openings from area measurements; free geometry, real takeoff-accuracy gain.
- **XFDF / FDF import-export** — standard annotation exchange; cheap interop to round-trip markups with Acrobat/Bluebeam (complements `.btx`).
- **Bates numbering / headers & footers / page labels** — document control; text stamping, free.

### Deliberately deferred — not low-cost, keep out of v1
- **Visual Search + auto-count** — find a symbol, count all instances. Highest-value takeoff feature after the basics, but it's template/image matching (`imageproc`/OpenCV) — real effort. **Top post-v1 candidate.**
- **Slip-sheeting** — replace a sheet revision while preserving markups/links; fast-follow with compare/versioning.
- **Forms (confirmed v2)** — create/fill PDF form fields (PDFium supports).
- **Digital signatures (confirmed v2)** — PKI/certificates; complex, so v2.
- **Spaces** — location-based takeoff grouping; moderate, post-v1.
- **Batch processing (confirmed, later)** — apply ops (flatten/headers/Bates/redact) across many files; mostly a queue + loop around `docops`, so straightforward once single-file ops exist.
- **Snapshot / region copy** — minor convenience; later.
- **Raster edge/corner-detection snapping** — give scanned/raster plans some snap targets via line/edge detection (Hough/OpenCV-style). Post-v1; v1 scanned-plan story is OCR-for-search + calibrated manual placement (§11). Pairs naturally with Visual Search (shared image-processing stack).

## 15. Cross-cutting requirements

- **Persistence strategy (embed-vs-sidecar split — decided, §12 (f)):** markups + custom-column data + **workflow status value** (Accepted/Rejected/Completed) embed as standard PDF annotations (files round-trip, open in Acrobat/Bluebeam; Bluebeam stores custom-column data in the annotation too). The **sidecar** beside the PDF (a `<file>.redline/` companion folder — full format in §18) holds app-only state that doesn't fit — or shouldn't bloat — the annotation dict: version history (retained-N, §9), status-change history + replies/threads, scale records (§7), and dynamic-stamp sequence-counter state (§6). Rule of thumb: the *current* portable value embeds; *history* and app-internal bookkeeping go to the sidecar.
- **User identity & audit (§6, §12 g/h)** — every install has a stable `user_id` (UUID) + editable display name (default OS username), embedded on each markup alongside the display name. The sidecar keeps a full append-only audit log (create/edit/status-change/delete-tombstone, each with `user_id` + UTC timestamp) plus the `user_id ↔ display-name` registry. This is the substrate for the Markup-List export's author/date columns (§7) and for cross-source attribution once the field app + sync layer arrive. Note: PDF `/T` alone (a free-text name, Bluebeam's default = OS username) is **insufficient for audit** — hence the embedded stable `user_id`.
- **Undo/redo** — command-pattern history for all markup/page ops; design in from the start. (Distinct from the audit trail: undo/redo is in-session editing; the audit log is the durable who-did-what record.)
- **Autosave & crash recovery** — periodic recovery snapshots (large files, long sessions).
- **Large-file memory budget** — bound the tile cache and page-object memory; stream rather than fully load 100s-MB files.
- **Auto-update** — Tauri's built-in updater for internal distribution.
- **Logging / error capture** — local logs for diagnosing field issues.

## 16. Dependency licensing (zero-cost + future-commercial)

The proposed stack is permissive and commercialization-friendly: Tauri (MIT/Apache-2.0), PDFium / `pdfium-render` (BSD), `lopdf` (MIT), Tesseract + `leptess` (Apache-2.0 / MIT). None impose copyleft on the app. The **one** copyleft gotcha is MuPDF (`mupdf-rs` is AGPL) — already quarantined behind the swappable `docops` trait (§8), so it only matters if/when you opt in, and a commercial Artifex license removes the constraint. Net: v1 ships at zero license cost, and nothing in the core blocks a future commercial release.

## 17. UI layout & panel system

Mirrors the Bluebeam Revu workspace: a **three-column layout** — left panel column · center document viewport · right panel column — plus a **collapsible bottom Markups/Comments list**.

- **Dockable, reconfigurable panels:** each side column hosts a stack of panels (modules) that are re-orderable, movable between columns, addable/removable, and individually collapsible; columns collapse/hide; draggable splitters resize the columns and the bottom list.
- **Panel inventory (v1):** Thumbnails, Bookmarks, Tool Chest, Properties (selected markup), Layers (OCG), Search (in-doc + folder/library results), Sets/Document navigator, Takeoff/Measurements summary, and the bottom Markups/Comments list.
- **Markups/Comments list (bottom):** tabular — author, date, comment, status, color, layer, measurement columns — sortable and filterable (incl. by status); the source of the Markup List export (§7/§14); click a row to jump to the markup.
- **Layout state = Profiles:** the layout (placement, sizes, collapsed state) serializes to JSON — which *is* the Profiles feature. v1 persists the current layout between sessions; named, switchable Profiles (multiple saved workspaces) build directly on it and can follow.
- **Frontend approach (Svelte):** recommend **`dockview-core`** (MIT, zero-dependency docking — tabs, groups, splitviews, drag-and-drop) so the docking engine isn't built from scratch; it's vanilla TS with a known Svelte integration pattern (mount Svelte components into panels). Lighter alternative: **`svelte-splitpanes`** / PaneForge for resizable-collapsible columns with custom panel-move logic.
- **Phasing within v1:** collapsible/resizable columns + a sensible default layout first; full drag-rearrange between columns next — usable early without front-loading all the docking polish.

## 18. Sidecar format (`<file>.redline/`)

Holds everything not embedded in the PDF annotations (per §12 f / §15). Design goals: append-only **audit integrity**, **crash-safe** writes, **mergeable** for the future sync layer, human-inspectable, and keyed by the stable markup `id` (= PDF `/NM`) so it **reconciles if the PDF is edited externally** rather than corrupting.

**Layout — a companion folder beside the PDF** (one item per document keeps the working directory tidy):

```
plans-L3.pdf
plans-L3.redline/
├── meta.json        # schema version, doc fingerprint, scales, users, counters, version index
├── audit.ndjson     # append-only audit log (one JSON object per line)
├── markups.json     # app-only per-markup data not in the annotation (replies/threads, overflow fields)
└── history/         # retained-N revision snapshots
    └── 0007__2026-06-06T14-30-00Z__u_9f3.pdf
```

Why this shape (engineering choices — flag if you disagree): the audit log is append-only and grows, so it is **NDJSON** appended with `O_APPEND` (never rewrite the whole file); the rest is small mutable state written **atomically** (temp-file + `rename`, never in-place — the workspace sensitive-write pattern). A **folder** (vs loose `<file>.redline.json` + sibling files) groups companions so "copy the PDF + its redline data" stays a two-item operation. Revision snapshots are **full PDF copies** in `history/` for v1 (simple, robust); delta storage is a later optimization.

**`meta.json`**
```json
{
  "schema_version": 1,
  "app_version": "0.1.0",
  "document": { "filename": "plans-L3.pdf", "fingerprint": "sha256:…", "page_count": 142,
                "created_at": "…", "updated_at": "…" },
  "users": { "u_9f3…": { "display_name": "Martin Robert", "first_seen": "…", "last_seen": "…" } },
  "scales": [
    { "id": "sc_1a2…", "applies_to": { "kind": "page", "page": 12 }, "method": "two_point",
      "ratio": 0.0254, "unit": "mm", "imperial_arch": false,
      "two_point": { "p1": [0,0], "p2": [0,0], "known_length": 5000, "known_unit": "mm" },
      "label": "1:100", "precision": 0, "created_by": "u_9f3…", "created_at": "…" }
  ],
  "sequence_counters": { "per_document": { "RFI": 7 }, "global_refs": ["project:acme-tower"] },
  "versions": { "retain": 10,
    "entries": [ { "revision": 7, "ts": "…", "user_id": "u_9f3…", "label": "pre-issue",
                   "snapshot": "history/0007__….pdf", "parent": 6 } ] }
}
```

**`audit.ndjson`** — one entry per line, strictly append:
```
{"seq":1,"ts":"2026-06-06T14:30:00Z","user_id":"u_9f3…","action":"create","target":{"type":"markup","id":"nm_b1c…"},"page":12,"markup_type":"measurement.area"}
{"seq":2,"ts":"…","user_id":"…","action":"status_change","target":{"type":"markup","id":"nm_b1c…"},"from":"none","to":"accepted"}
{"seq":3,"ts":"…","user_id":"…","action":"edit","target":{"type":"markup","id":"nm_b1c…"},"changed":["geometry","appearance.color"]}
{"seq":4,"ts":"…","user_id":"…","action":"delete","target":{"type":"markup","id":"nm_b1c…"}}
```
`action` ∈ `create | edit | status_change | delete | scale_set | scale_recalibrate`. Deletions are **tombstones** — the markup leaves the PDF, the record stays (§12 h). `changed` lists field keys (not full diffs) to stay compact; the `history/` snapshots are the full before/after fallback.

**`markups.json`** — app-only per-markup data that doesn't belong in the annotation dict, keyed by markup `id` (= `/NM`):
```json
{
  "nm_b1c…": {
    "replies": [ { "id": "r_1", "user_id": "…", "ts": "…", "text": "…", "parent": null } ],
    "status_history": [ { "ts": "…", "user_id": "…", "from": "none", "to": "accepted" } ],
    "extra": {}
  }
}
```

**Reconciliation & integrity:**
- Records key on the stable `id`. On open, a `markups.json`/audit entry whose `id` is absent from the PDF is flagged as **externally deleted** (and a tombstone appended) — never silently dropped, so editing the PDF in another tool can't corrupt the audit.
- `document.fingerprint` detects a swapped/externally-edited PDF and warns rather than blindly reconciling.
- `meta.json` / `markups.json` atomic-write; `audit.ndjson` append-only.

**Sync-readiness (post-v1):** the append-only log + id-keyed state records merge cleanly across sources (desktop + future field app); the `user_id` registry survives renames; nothing assumes a single writer — the async sync layer (§2) layers on by adding only a conflict policy, no format change.

**Sets:** a Set (§9) spanning multiple PDFs is a separate small definition (`<setname>.redlineset.json` — ordered member list + page-label config); each member PDF keeps its own `.redline/` folder. (v1 detail — confirm when we spec Sets.)

## 19. Tool Set format & `.btx` import

Two formats: our **native Tool Set** (what we serialize) and the **`.btx` import mapping** (how we read Bluebeam's). Both lean on the same markup model (§6), so a tool is just a serialized markup template. **`.btx` export is NOT a v1 requirement; import is** (§12 Bluebeam migration).

### 19.1 Native Tool Set (`.redlinetools.json`)

A **tool** = a markup type + saved properties + placement mode, optionally with fixed geometry (symbols/stamps) and, for measurement tools, scale/unit/depth settings. A **Tool Set** = a named, ordered collection of tools. Versioned, shareable, sync-friendly (§2).

```json
{
  "schema_version": 1,
  "kind": "redline.toolset",
  "id": "ts_…",
  "name": "Lighting Review",
  "created_by": "u_9f3…", "created_at": "…",
  "tools": [
    {
      "id": "tl_…",
      "label": "Defect cloud",
      "markup_type": "cloud",
      "placement_mode": "properties",            // properties | drawing
      "properties": { "color": "#E11", "line_weight": 2.0, "opacity": 1.0,
                      "fill": null, "line_style": "solid", "subject": "Defect", "font": null },
      "geometry": null,                          // drawing-mode tools carry fixed geometry (symbols)
      "measurement": null,                       // measurement tools: { measure_type, unit, depth, imperial_arch }
      "stamp": null,                             // stamp tools: see below
      "custom_columns": [ { "key": "Discipline", "type": "text" } ],
      "source": { "imported_from": "btx", "original_type": "Bluebeam.PDF.Annotations.AnnotationPolygon" }
    }
  ]
}
```

- **Stamps are tools** (`markup_type: "stamp"` static, or `"stamp.dynamic"`). A dynamic-stamp tool carries `stamp: { template, fields: [{ kind: date|time|datetime|user|sequence|doc_name|prompt_text|prompt_dropdown, format, options }] }` — appearance composed by us at placement (§6), never via embedded JS.
- **Measurement tools** are not special-cased — a calibrated area tool is just a tool whose `markup_type` is a measurement type with a fixed `subject`/`depth`.
- **Recent Tools** is an app-managed, auto-populated set in the same format (not necessarily shared).
- `source` preserves import provenance so an imported tool's origin is traceable (and re-export stays possible later).

### 19.2 `.btx` import mapping

`.btx` is **XML / UTF-8** (not opaque binary). Each `<ToolChestItem>` → one native tool:

| `.btx` element | → native field | Notes |
|---|---|---|
| `<Name>` | `label` (+ `id` seed) | display name |
| `<Type>` | `markup_type` | via the type map below |
| `<Mode>` | `placement_mode` | `properties` / `drawing` map 1:1 |
| `<BSIColumnData>` | `custom_columns` | estimating/custom columns |
| `<Raw>` | `properties` + `geometry` + `subject` | **PDF annotation dict** → hand to the annotation reader we already build |

**Type map** (Bluebeam `<Type>` → our `markup_type`):

| Bluebeam | Ours |
|---|---|
| `AnnotationFreeText` | `text`, or `callout` if a callout line (`/CL`) is present |
| `AnnotationLine` | `line`, or `arrow` if line-endings (`/LE`) |
| `AnnotationPolyLine` | `polyline` |
| `AnnotationPolygon` | `polygon` |
| `AnnotationSquare` / `AnnotationCircle` | `rectangle` / `ellipse` |
| `AnnotationInk` | `pen/ink` |
| `AnnotationHighlight` | `highlight` |
| `AnnotationStamp` | `stamp` |
| measurement intents (`/IT` = `PolygonDimension`, `PolyLineDimension`, `LineDimension`, …) | `measurement.area` / `.perimeter`/`.length` / etc., with the scale metadata mapped to a `measurement` block |

**Wrinkles (from §6, handled in the parser):**
- **zlib blobs** — `<Raw>`/`<Script>` payloads beginning `789c` are zlib-deflated → inflate before parsing.
- **Packaging** — tool sets/stamps often arrive zipped → unzip first.
- **Stamps** — imported as PDFs. *Static* → import directly. *Dynamic* → we do **not** execute embedded JS; map recognizable auto-fields (date/user/sequence) onto our dynamic-stamp model, fall back to a static appearance where a field can't be mapped.
- **Unmappable types/attributes** → never silently dropped: import as the closest generic annotation (preserving `<Raw>` in `source` for fidelity) **or** skip with an entry in an **import report**.

**Output:** a native Tool Set (§19.1) + an **import report** (mapped / fell-back / skipped, per item) so a reviewer sees exactly what came across. Validate the importer early against a library of real-world `.btx` files + stamps (measurement tools, custom columns, embedded images) — this is a parser project, not from-scratch reverse engineering (§6, §11).
