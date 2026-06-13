# Redline v1 Completion Roadmap

> **This is the master roadmap, not an executable task plan.** Each slice below gets its
> own detailed implementation plan (`docs/superpowers/plans/YYYY-MM-DD-<slice>.md`,
> bite-sized TDD tasks per superpowers:writing-plans) when it starts. Slices are ordered;
> each produces working, testable software on its own.

**Goal:** Complete the Bluebeam-alternative v1 per `docs/bluebeam-alternative-v1-spec.md`
(all sections; build order §13) from the current state: M1 done, M2 ~30% done.

**Spec version referenced:** 2026-06-06 (all §12 decisions locked).

---

## Current state (verified 2026-06-12)

| Area | State |
|---|---|
| M1 render core | DONE. Tiled PDFium render, byte-budgeted tile cache, page-handle LRU, mmap + lopdf normalise paths, §20 overlay, headless bench. 1,235 LOC, 8 tests. |
| M2 markup model | DONE. Full envelope (18 types, audit, workflow, measurement ext), spec §6-complete. 384 LOC, 8 tests. |
| M2 PDF annotation serde | DONE (commit 38d9d69, unmerged). Dict round-trip, standard + /RL* keys, foreign-annot import. 690 LOC, 9 tests. |
| Geometry | Stub + rstar index scaffold (165 LOC). Path extraction TODO at `geometry/mod.rs:98`. |
| Document save | **MISSING. Annotations serialize to dicts but nothing writes them into a PDF file.** `document` module is a 22-LOC shell. |
| Frontend | Viewport + 3-column shell live (1,060 LOC). No markup overlay (placeholder `Viewport.svelte:380`), no tools, no list panel. 0 frontend tests. |
| takeoff / text / ocr / search / storage / compare | Stubs (6-12 LOC each). `docops` trait shell (33 LOC). |
| §20 definitive gate | OWED. Indicative pass (Apple Silicon) only. Floor machine + Windows runs blocked on hardware. |
| Release pipeline | /sendit lands branches; cross-platform release build blocked on CI runners (see `.claude/skills/sendit/references/release-todo.md`). |

---

## Slice sequence

Dependency-ordered. "Exit" = the merge gate for that slice (all tests green, clippy 0,
`npm run check` clean, plus the listed acceptance). Estimates are working-session counts
(1 session ≈ one focused day-part), calibrated against M1/M2 actuals.

### Track A - critical path (build order §13)

**S1. M2a: Document save pipeline** — ✅ DONE 2026-06-13 (code-complete, branch `feat/m2-annotation-serde`, 13 commits ae4ea2c..90f0a8c, final review SHIP; awaiting GUI smoke + /sendit). (~1-2 sessions)
Write markups into the PDF: lopdf-based save (read annots on open via existing
`from_annotation_dict`; write/update/delete annotation objects + /NM keys on save;
atomic temp+rename; never touch the original until commit). Save / Save-As commands + IPC.
- Files: `document/` (real implementation), `commands/document.rs`, `ipc.ts`
- Exit: open → add markup (programmatic) → save → reopen round-trips; saved file opens
  with annotations visible in an external viewer (manual Acrobat/Bluebeam check, spec §11).
- Risk: lopdf rewrite vs incremental-update strategy on 300 MB files; decide in-plan
  (likely full rewrite to temp + rename; measure on C2/C3 corpus).

**S2. M2b: Markup overlay + authoring UI + undo/redo** (~3-4 sessions, the big one)
SVG overlay above the canvas (screen↔PDF transforms already in `viewport.ts`); creation
tools for the locked §6 type set (text, callout, cloud, rect/ellipse/polygon,
line/polyline/arrow, highlight, pen/ink); select/move/resize/edit; Properties panel
(right column) binding `Appearance`; comments (`contents`); grouping; command-pattern
undo/redo (§15, designed in NOW, not retrofitted). Vitest suite for overlay math +
command stack.
- Exit: author/edit/delete every v1 type in the GUI, undo/redo across ops, save →
  external-viewer check; overlay stays in sync through pan/zoom (no drift at extreme zoom).

**S3. M2c: Review workflow + Markups list panel** (~1-2 sessions)
Status (None/Accepted/Rejected/Completed) on markups; bottom Markups list (author, date,
comment, status, color, layer columns; sort/filter incl. by status; click-to-jump).
First-class M2 scope per §12(i)/§13.
- Exit: status round-trips through PDF (embedded value per §12 f); list reflects live edits.

**S4. M2d: Sidecar v1 (storage module, pulled forward from M4)** (~1-2 sessions)
`<file>.redline/` per §18: `meta.json` (atomic temp+rename), `audit.ndjson` (append-only,
O_APPEND), `markups.json`; user identity (first-run `user_id` UUID + display name, §12 g);
full audit log incl. delete-tombstones (§12 h); status history + replies storage;
external-edit reconciliation (id-keyed, fingerprint warn).
Rationale for pulling forward: the audit trail has holes for every session that edits
markups before the sidecar exists; §6 says audit is day-one scope.
- Exit: every create/edit/status/delete appends an audit line; sidecar survives crash-kill
  during write (atomicity test); external deletion detected and tombstoned on reopen.

**S5. M2e: Tool Chest + Tool Sets + stamps** (~2-3 sessions)
Native `.redlinetools.json` (§19.1); Tool Chest panel; two placement modes; Recent Tools;
static stamps (PDF/SVG-backed, appearance stream); dynamic stamps composed by us at
placement (date/time/user/doc-name/sequence/prompt fields; counters in sidecar, §12 c).
- Exit: create tool from existing markup; place via both modes; dynamic stamp bakes values
  and flattens cleanly; tool set file round-trips.

**S6. M2f: .btx importer** (~2 sessions)
XML parse → type map (§19.2) → `<Raw>` through the existing annotation reader; zlib
(`789c`) inflation; zip packaging; stamp import (static direct; dynamic via auto-field
mapping, JS never executed); import report (mapped/fell-back/skipped). **Needs a library
of real .btx files + stamps from Martin's Bluebeam install (user asset, like the corpus).**
- Exit: real-world .btx libraries import with a complete report; imported tools place
  correctly; unmappable items never silently dropped.
- **M2 COMPLETE** after S6.

**S7. M3: Takeoff + measurement + Markup List export** (~3-4 sessions)
Geometry path extraction first (the `geometry/mod.rs:98` TODO - transformed path-segment
iteration, §5 Form-XObject invariant, validated on C4) + snapping UI; scale records
(per-page + document default + apply-to-range, sidecar source of truth, `/Measure`
embed on save, §12 j); measurement types (length/perimeter/area+cutouts/volume/count/
angle/radius); `raw_measure` scale-independence + deterministic recompute on recalibrate;
live rollups panel; **Markup List export XLSX + CSV** (first-class deliverable, §7).
- Exit: calibrate → measure → recalibrate recomputes; rollups group by subject/layer;
  XLSX/CSV export matches list panel incl. selected columns; snapping hits exact vector
  coords on C4 at any zoom.

**S8. M4: Sets, versioning, page ops, search, OCR** (~4-5 sessions, internally splittable)
`<setname>.redlineset.json` (§9/§18) + Sets navigator (thumbnails, labels via /PageLabels
+ manual edit, cross-doc bookmarks); retained-N version snapshots (§12 e) in `history/`;
page manipulation (insert/delete/rotate/reorder/extract/crop/merge); layers (OCG)
show/hide; hyperlinks; in-doc + cross-Set text search (PDFium); OCR invisible text layer
(leptess, `ocr` feature gate ON); Tantivy folder index + `notify` watcher (`search`
feature gate ON).
- Exit: per-capability acceptance defined in the slice plan; headline: a multi-PDF Set
  navigates as one document, search hits open at page, scanned C5 becomes searchable.

**S9. M5: docops baseline** (~1-2 sessions)
Behind the existing trait: flatten (annotation appearance → page content), optimize
(lopdf strip + recompress - the C5 normalise path generalized), rasterize-redact
(region → raster, underlying data removed; §8 safe floor).
- Exit: flattened file shows markups as content in external viewers; redacted region
  yields no extractable text/vectors underneath.

**S10. M6 (Phase 1.1): Compare & overlay** (~2 sessions)
Manual two-point registration; color-channel diff overlay reusing render tiles.
Formally post-v1.0 fast-follow; included for completion.

### Track B - parallel / externally blocked (not on the critical path)

- **B1. §20 definitive floor run** (Windows + macOS, 16 GB) - blocked on hardware
  (Martin). Runbook ready (`bench/RUNBOOK-S20.md`). The formal v1 Go/No-Go; any FAIL
  re-opens render work ahead of everything else.
- **B2. Windows build verification** - bundling is wired but untested; first Windows
  session should also run B1.
- **B3. Release pipeline** (`/sendit --release` prerequisites): Forgejo runners
  (macOS + Windows), per-OS PDFium fetch + bundle workflow, signing/notarization
  (see `release-todo.md`). Needed before v1 distribution, not before feature work.
- **B4. Cross-cutting, attach to nearest slice:** autosave/recovery snapshots (with S4
  sidecar); Tauri auto-updater + logging/error capture (with B3); XFDF import-export
  (cheap interop, after S6 - shares the annotation reader).

### User-provided assets needed (flag early)

1. Real `.btx` Tool Set + stamp library exported from Bluebeam (before S6).
2. Floor-machine hardware time, both OSes (B1/B2).
3. Ongoing: real plan sets already in `bench/corpus/` cover S1-S8 testing.

---

## Sequencing rationale

- **Save pipeline (S1) before UI (S2):** authoring without persistence is demo-ware; the
  serde slice just merged makes save the natural completion of the round-trip, and every
  later slice depends on it.
- **Sidecar (S4) pulled forward from M4:** audit integrity is a day-one spec requirement
  (§6/§12 h); deferring it to M4 leaves un-audited editing sessions in the field.
- **Geometry extraction lives in S7,** not S2: markup authoring doesn't need snapping;
  measurement does. Keeps S2 smaller. If S2 finds drawing UX wants snap-to-vector, the
  extraction sub-task promotes into S2 and S7 shrinks.
- **Undo/redo inside S2,** not later: command-pattern history must wrap the markup ops
  from the first edit (§15) or it becomes a retrofit.

## Standing gates (every slice)

TDD (failing test first); `cargo test` + `cargo clippy --all-targets` (0 warnings) +
`cargo fmt --check` + `npm run check` green; corpus tests manually before render-path
merges; ship via `/sendit`; `/code-review` before shipping risky slices (S1, S2, S7
render/geometry touchpoints); KB decision log for architectural choices.

## Estimate summary

| Track | Sessions (rough) |
|---|---|
| S1-S6 (finish M2) | 10-14 |
| S7 (M3) | 3-4 |
| S8 (M4) | 4-5 |
| S9-S10 (M5-M6) | 3-4 |
| **Total to v1-complete** | **~20-27 working sessions** |
