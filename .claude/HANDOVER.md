# Redline — Handover Notes

## Current Status

**M2 / S2 (markup authoring + undo/redo) in flight on branch `feat/s2a-markup-overlay`.**
S2a (overlay display) + S2 groups **G1–G6 are code-complete and reviewed**, plus **viewport
zoom-snap controls** and the **first real GUI run (2026-06-15/16) which fixed 6 latent M1
render-loop bugs** (see that section below). NOT yet pushed/merged. **170 frontend + 57 Rust
tests green, clippy 0, `cargo fmt` clean, `npm run check` 0 errors** (2 a11y warnings: pre-existing
viewport `<div>`, plus `autofocus` on the text editor — both expected/non-blocking).

Authoring works end-to-end: tool palette → draw on the overlay → commit through the Svelte
store → async per-op mirror to the Rust backend → undoable → persists on Save. **The GUI now
genuinely renders** (upright, correct scale, smooth zoom, seamless tiles — it never did before).

## Last Session
**Date**: 2026-06-16 (cont.)
**Summary**: Shipped **viewport zoom-snap controls** (`87f355c`: Fit-Width/Fit-Height/100%,
buttons + ⌘/Ctrl 1/2/0) then drove **G6 select/move/resize/delete** to completion across 5
committed increments via the subagent-driven flow: **G6.1** pure selection geometry
(`markup-select.ts`, `c88785d`), **G6.2** batch-undo transaction frames (`296cd2b`), **G6.3**
select/shift/marquee + chrome (`4cbe947`), **G6.4+G6.5** move/rect-resize/delete + keyboard
(`861e44b`). One hiccup: a `/reload-plugins` mid-run made the G6.4+G6.5 agent's writes land late
and interleave with a manual takeover — resolved by verifying the final tree (no dup defs, 170
green) before committing. Prior: 6 M1 GUI render-loop fixes + GUI harness + mtime cache
(2026-06-15/16), G5 Text/Callout, S2a-T3 + G1–G4 (2026-06-14).

## Plan / Spec (read these first)
| Doc | Path |
|-----|------|
| S2 design spec | `docs/superpowers/specs/2026-06-14-s2-markup-authoring-design.md` |
| S2 impl plan (G1–G4 detailed, G5–G9 mapped) | `docs/superpowers/plans/2026-06-14-s2-markup-authoring.md` |
| Architecture decision | `decision:vic6slsasg6njkf7haka` (Svelte-SoT + per-op async mirror) |

## What's built (S2 groups, all on this branch)
- **G1 backend rails**: `MarkupStore::update`/`delete`; `update_markup`/`delete_markup`/`get_user_identity` Tauri commands; persisted first-run identity (`src-tauri/src/identity.rs`); `ipc.ts` wrappers.
- **G2 undo/sync core**: `src/lib/markup-commands.ts` (command-pattern History) + `src/lib/markup-store.svelte.ts` (reactive in-session SoT, ordered async mirror queue, `flush()` rejects on undrained queue so save refuses stale state). App owns the store, flushes before save.
- **Test harness**: `@testing-library/svelte` + vitest jsdom (e-fees' pattern). Setup `src/tests/setup.ts`; component tests carry `// @vitest-environment jsdom`. Interaction tests mount the real component, script gestures, assert store + SVG + a **glued-on-zoom (§5 no-drift) assertion**. This replaces per-operation manual GUI testing.
- **G3 drag-draw**: `markup-tools.ts` (`dragDrawGeometry`/`buildMarkup`/`isDrawTool`); `ToolPalette.svelte`; Viewport overlay pointer capture (Hand pans, draw tools capture); `pointercancel` reset; `DrawTool` type guard.
- **G4 multi-click + ink**: `markup-tools.ts` (`MULTI_CLICK_TOOLS`/`isMultiClickTool`/`isInkTool`/`polylineGeometry`/`inkGeometry`); scalloped Cloud render (`cloudPath` + `cloud` SvgShape kind); Viewport multi-click state machine (click=vertex, Enter/dblclick=finish with dedup, Esc=cancel) + freehand ink (sampled, ≥2 pts); `$effect` clears gesture state on tool-switch.
- **G5 text + callout**: `markup-tools.ts` (`TEXT_TOOLS`/`isTextTool`/`textBoxGeometry`(Rect)/`calloutGeometry`(Polyline leader)/`DEFAULT_TEXT_FONT`; `buildMarkup` gained optional `contents`); `markup-render.ts` `text`+`callout` SvgShape kinds (font-scaled by zoom, `dominant-baseline="hanging"`); **`annotation.rs` serde**: Text/Callout→`/Subtype FreeText`, Callout leader→`/CL`, font→`/DA`+`/RLFontFamily`+`/RLFontSize` (read back lossless), foreign FreeText `/CL`?Callout:Text; Viewport inline screen-positioned `<textarea>` editor (Text=1 click, Callout=2 clicks; commit on blur+Cmd/Ctrl+Enter, Esc cancels, empty/whitespace=no-op; editor split into `editor`+`editorText` to avoid bind-teardown crash). ToolPalette: Text/Callout buttons.
- **Zoom-snap** (`87f355c`): `viewport.ts` pure `fitWidthZoom`/`fitHeightZoom`/`ACTUAL_SIZE_ZOOM` (§5: page pts vs css px); Viewport `applySnapZoom()` (reuses the placeholder/debounced-render path) + bottom-right buttons + ⌘/Ctrl 1/2/0.
- **G6 select / move / resize / delete** (plan `docs/superpowers/plans/2026-06-16-g6-select-move-resize-delete.md`): new pure `markup-select.ts` (`boundsOf`/`hitTest`/`marqueeHits`/`translateGeometry`/`scaleGeometryToBounds`/`handleAnchors`/`resizeBounds`, §5 y-up); `markup-commands.ts` frame-based History (`pushBatch`, `undo/redo`→`MirrorOp[]`) so a multi-select edit = ONE undo while mirroring N ops; store `applyBatch`/`deleteSelected`/`selectedMarkups`; `markup-render.ts` `selectionChrome`; `markup-tools.ts` `bumpAudit`. Viewport Select tool: click/shift/marquee select, drag-move (all selected), rect resize (single Rect, 8 handles), Delete/Backspace/Esc; `dragPreview`-aware `pageShapes`+`selectionBounds` keep shapes+chrome glued during the gesture. **Deferred to a G6.1 follow-up:** resize of non-rect geometries (Polyline/Ink/Callout are move-only) and multi-select resize. **Known minor:** undoing a multi-delete re-appends in reverse order (z-order among the restored set flips; content correct).

## M1 GUI render-loop fixes (first real GUI run — 2026-06-15/16)
The GUI render path had never run end-to-end (vitest mocks `invoke`; the §20 bench is headless),
so it shipped with multiple defects — all now fixed on this branch:
- **`71949c4` IPC camelCase** — `ipc.ts` sent snake_case invoke arg keys; Tauri v2 wants
  camelCase (auto-mapped to snake_case Rust params). Every multi-word command failed ("missing
  required key") → blank viewport (`get_page_size` failed → no tiles) + "Load markups failed"
  banner. Guard test `src/lib/ipc.test.ts`. (KB obs:me9oo7nq06hvpbne926f.)
- **`5dd9017` orientation** — tile matrix double-flipped y (pdfium-render's matrix path already
  flips); page was upside-down. Now `d=+scale, f=-tile_origin_y`.
- **`e47c7c2` DPR scale + race** — `render_tile` bitmap was `css×zoom×dpr` while `drawTile`
  blitted into a dpr-scaled ctx with no dest size → page ~zoom×dpr too large. Now bitmap=`css×dpr`.
  Plus: `tileKey` includes pageIndex; canvas cleared each frame; `renderEpoch` discards stale
  async tiles (page-switch race / zoom ghosting); zoom anchored + scroll clamped.
- **`b342d71` smooth zoom** — wheel was per-event multiplicative (flick → 800% cap), re-rendering
  every tile per event (slowdown). Now proportional `wheelZoomFactor()` (exp, in `lib/viewport.ts`,
  shared with tests) + CSS-scaled placeholder during the gesture + debounced sharp re-render
  (~120 ms). Keyboard zoom added (Cmd/Ctrl +/-/0).
- **`d976cef` tile seams** — tiles drawn at fractional CSS positions in a dpr-scaled ctx left
  sub-pixel join lines. Now drawn in integer device px at native size (ctx NOT dpr-scaled);
  integer tile stride → exact abutment.

**Lessons:** (1) the §20 GUI-smoke (G9) MUST include a visual render/zoom/pan/page pass —
headless timing missed all six. (2) Observability: I can SEE the live app via
`screencapture -R <bounds>` (window bounds via `osascript` System Events) and render tiles to PNG
(then Read the image), but CANNOT drive it — `cliclick` keystrokes/clicks don't reach the
WKWebView. Driving needs a Playwright + mock-IPC harness (proposed, not built). (3) Do NOT
`pgrep -fl` the dev process — its command line carries the full env (creds) and dumps them.

## Next Steps (remaining S2 groups — author detail JIT, then subagent-driven execute)
1. **G7 — Properties panel** *(next)* (right column): bind selection `Appearance`/`contents`/`subject`/`layer`; with no selection edit `draftAppearance`; reuse the **`bumpAudit` helper already shipped in G6** (`markup-tools.ts`). **Carries the font picker** — and with it the `/DA` family fix (G5 pins `/DA` to `/Helv`; the exact family already round-trips via `/RLFontFamily`).
2. **G8 — Grouping** (cut-line): add `group_id: Option<Uuid>` to Rust `Markup` + serde + `/RLGroup` key + TS; group/ungroup commands; group-aware select/move. *Touches annotation serde — `/code-review` before ship.*
3. **G9 — Ship**: full-app GUI smoke — now MUST include a visual render/zoom/pan/page-nav pass + **a select/move/resize/delete + zoom-snap pass** (the 6 M1 bugs prove headless isn't enough) + save round-trip in Acrobat/Bluebeam; update handover/roadmap, `/code-review`, `/sendit`.
4. **G6.1 follow-up** (optional, any time): resize handles for non-rect geometry (Polyline/Ink/Callout vertex/segment resize) + multi-select resize; fix multi-delete-undo z-order.

## UI Backlog
- ~~**Viewport zoom-snap controls**~~ **DONE 2026-06-16** *(req. user; `obs:1hjcevau4cpcisu9koy4`)*: Fit-Width / Fit-Height / 100% as bottom-right toolbar buttons + key-commands **⌘/Ctrl 1 / 2 / 0**. Pure fit math in `lib/viewport.ts` (`fitWidthZoom`/`fitHeightZoom`/`ACTUAL_SIZE_ZOOM`, §5 — page pts vs css px, never the raster); `applySnapZoom()` in `Viewport.svelte` reuses the placeholder + debounced sharp-render path. 11 new tests (5 unit + 6 interaction asserting the live zoom-indicator %). Cmd+0 now routes through `actualSize()`.

**Perf — partially addressed (`59f47fe` mtime cache):** reopen of an unmodified file is now instant
(<1 ms); **first** open of a large file still pays ~96 s (lopdf `Document::load` parses the whole
150 MB / 691 pp file, off the UI thread via `spawn_blocking`). The proper fix (read annotations via
the already-open PDFium) is **blocked**: pdfium-render 0.8.37 keeps the annotation key accessors
`pub(crate)`, so our custom `/RL*` keys (RLType/RLPage/RLFontSize/etc.) are unreachable without
`unsafe`. Killing the first-open cost needs a lighter custom `/Annots`-only parser or a different
PDF lib — deferred.

## Execution method (working well — keep using)
Subagent-driven development at **group granularity**: author the group's detailed TDD tasks in the plan → implementer subagent (sonnet) → spec-compliance review → code-quality review → fix loop → verify. Every tool/gesture group ships interaction tests (no per-op manual GUI).

## Key Gotchas (carry forward)
- Svelte store is in-session SoT; Rust store is a mirror + save buffer. `flush()` (awaited before save) THROWS on an undrained mirror queue — App's `catch` aborts the save.
- Overlay `pointer-events` toggles via `isCreateTool()`; Hand tool pans (the `viewport-root` mouse handlers), creation tools capture on the SVG overlay.
- Tests run via `npm run test` (vitest, mixed node + jsdom envs); Rust tests still `--test-threads=1`.
- Type guards (`isDrawTool`/`isMultiClickTool`/`isInkTool`) narrow `ToolKind`→`MarkupType` so no unsafe casts; `as MultiClickTool` only appears post-guard.
- §5 precision invariant: overlay maps PDF user space → screen every render (never reads raster); the glued-on-zoom interaction test guards against drift.
- PDFium 2 GiB limit, global C state (serial tests), `RenderEngine` drop order, page-handle LRU — unchanged from M1.
- §20 definitive floor-machine run (16 GB, Windows + macOS) still OWED (Track B, blocked on hardware).

## Key References
| Item | Value |
|------|-------|
| Remote | `git@ssh.forge.mms.name:emittiv/redline.git` |
| Branch | `feat/s2a-markup-overlay` (S2a + G1–G5 + 6 GUI fixes + mtime cache + GUI harness, unpushed) |
| Ship pipeline | `.claude/skills/sendit/SKILL.md` |
| Precedent test setup | e-fees (`/Volumes/base/dev/claude/e-fees`): @testing-library/svelte + Playwright + tauri-driver |
| **GUI harness** (drive frontend headlessly) | `npm run gui:harness` — Playwright + mock-IPC (`tools/gui-harness.mjs`); loads the real Vite app with synthetic labelled tiles; needs the dev server on :1421. Lets the agent screenshot zoom/pan/page/tool gestures (the WKWebView can't be cliclick-driven). |

---
*Updated: 2026-06-16*
