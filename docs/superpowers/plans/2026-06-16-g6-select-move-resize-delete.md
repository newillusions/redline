# G6 — Select / Move / Resize / Delete (detailed TDD breakdown)

**Group:** S2 / G6. **Branch:** `feat/s2a-markup-overlay`. **Date:** 2026-06-16.
**Parent plan:** `docs/superpowers/plans/2026-06-14-s2-markup-authoring.md` (G6 map at §"G6").
**Scope decided 2026-06-16 (user):**
- **Full selection interaction** — hit-test, selection chrome, 8 resize handles, drag-move, marquee + shift multi-select, Delete.
- **Resize is rect-based first** — only Rect-geometry markups (Rectangle, Ellipse, Highlight, Text box) resize via handles. Polyline/Polygon/Cloud/Ink/Callout/Line/Arrow are **move-only** this pass (handles hidden when the selection contains a non-rect or is multi-select). A `G6.1` follow-up can add vertex/segment resize.

## Architecture fit (what already exists — do NOT rebuild)
- `MarkupStore` (`markup-store.svelte.ts`): `markups`, `selectedIds = $state<Set<string>>`, `activeTool`, `update(before, after)`, `delete(id)`, `undo/redo`, ordered mirror queue, `flush()`. `removeById` already prunes `selectedIds`.
- `markup-commands.ts`: `CreateCmd`/`UpdateCmd`/`DeleteCmd`, `History.push/undo/redo` (single-command). `MirrorOp = add|update|delete`.
- IPC rails (G1): `update_markup`/`delete_markup` + `ipc.ts` wrappers; store injects `{add,update,remove}`.
- `markup-render.ts`: `markupToSvg`, `SvgShape`. `Viewport.svelte`: overlay, `isCreateTool()` gating, `screenToPdfUserSpace`/`pdfUserSpaceToScreen`, `clientToPdf`.
- Geometry (`ipc.ts`): `MarkupGeometry = {Point} | {Rect:{min,max}} | {Polyline:PdfPoint[]} | {Ink:PdfPoint[][]}`. **PDF user space: y-up.**

## §5 invariant (binding)
All hit-test / bounds / move / resize math runs in **PDF user space at f64**, never reads the raster. Handles are fixed *screen-size* squares (hit-tested in screen px), but their anchors derive from PDF-space bounds mapped through `pdfUserSpaceToScreen`. The glued-on-zoom interaction test guards drift (mirror the G3/G5 pattern).

---

## G6.1 — Pure selection geometry module (`src/lib/markup-select.ts`) + unit tests
No DOM, no Svelte — fully unit-tested (mirror `markup-tools.ts`). This is the foundation; build + green it first.

**Types & signatures:**
```ts
export interface Bounds { minX: number; minY: number; maxX: number; maxY: number } // PDF user space (y-up)
export type HandleId = "nw"|"n"|"ne"|"e"|"se"|"s"|"sw"|"w";
export const HANDLE_IDS: readonly HandleId[];

/** AABB of any geometry in PDF user space. Point → zero-size bounds at the point. */
export function boundsOf(m: Markup): Bounds;

/** Topmost markup id hit at PDF point `p`, else null. Iterate markups in REVERSE (last drawn = topmost).
 *  Rect/closed → point-in-bounds (filled) OR within `tolPts` of an edge (unfilled). Polyline/Ink →
 *  min segment distance ≤ tolPts. Point → distance ≤ tolPts. `tolPts` is PDF points (caller passes
 *  screenTolPx / zoom so the grab band is constant on screen). */
export function hitTest(markups: Markup[], p: PdfPoint, tolPts: number): string | null;

/** Ids whose bounds INTERSECT the marquee rect (PDF space). Order = markups order. */
export function marqueeHits(markups: Markup[], rect: Bounds): string[];

/** Translate every point of a geometry by (dx,dy) PDF points. Works for all 4 geometry kinds. */
export function translateGeometry(g: MarkupGeometry, dx: number, dy: number): MarkupGeometry;

/** Remap every point proportionally from `from` bounds to `to` bounds (Rect resize). Degenerate
 *  source dimension (0 width/height) maps that axis to `to.min` (no divide-by-zero). */
export function scaleGeometryToBounds(g: MarkupGeometry, from: Bounds, to: Bounds): MarkupGeometry;

/** Whether a markup resizes via rect handles this pass (geometry is Rect). */
export function isRectResizable(m: Markup): boolean;   // "Rect" in m.geometry

/** Handle anchor points in PDF space for a bounds (8 handles). */
export function handleAnchors(b: Bounds): Record<HandleId, PdfPoint>;

/** New bounds after dragging `handle` to PDF point `p`, clamped so width/height ≥ minPts.
 *  Opposite edge/corner stays fixed; edge handles move one axis only. No flip (clamp at min). */
export function resizeBounds(b: Bounds, handle: HandleId, p: PdfPoint, minPts: number): Bounds;

/** Apply a resize to a Rect markup's geometry: scaleGeometryToBounds(g, boundsOf(m), newBounds). */
```
Helper: a private `segDistance(p, a, b)` (point→segment distance) for polyline/ink hit-test.

**Tests (`markup-select.test.ts`):**
- `boundsOf`: Rect (min/max direct), Polyline (aabb of verts), Ink (aabb across all strokes), Point (zero-size). Exact values.
- `hitTest`: filled rect interior hit; rect edge hit within tol; miss outside tol; polyline near-segment hit + far miss; **topmost wins** (two overlapping rects → later id); empty list → null.
- `marqueeHits`: contained + intersecting included; disjoint excluded; ordering preserved.
- `translateGeometry`: each kind shifted exactly (dx,dy); immutable (input unchanged).
- `scaleGeometryToBounds`: rect doubled maps corners exactly; polyline verts remap proportionally; degenerate axis safe.
- `resizeBounds`: each corner handle keeps the opposite corner fixed (exact); each edge handle moves one axis; min-size clamp; drag past opposite edge clamps (no negative size).
- `handleAnchors`: 8 anchors at expected coords for a known bounds.
- `isRectResizable`: true for Rect geometry, false for Polyline/Ink/Point.

**Exit:** `npx vitest run src/lib/markup-select.test.ts` green; `npm run check` 0. Commit `feat(g6): pure selection geometry (hit-test, bounds, move, resize)`.

---

## G6.2 — Batch undo (transaction frame) in History + store + tests
Needed so a multi-select move/resize/delete is **one** undo, while still mirroring N granular ops.

**`markup-commands.ts`:** make `History` frame-based, additively.
- Internally store `undoStack: Command[][]` (each frame = 1+ commands). `push(cmd)` keeps returning a single `MirrorOp` (frame of one). Add:
  ```ts
  pushBatch(cmds: Command[]): MirrorOp[];   // apply in order, one undo frame, clears redo
  ```
- Change `undo()`/`redo()` to return `MirrorOp[] | null` (invert/apply each command in the frame; on undo invert in REVERSE order). Single-command frames return a length-1 array.
- Update `markup-commands.test.ts` accordingly (assert arrays; existing single-op cases → `[op]`).

**`markup-store.svelte.ts`:**
- `undo()/redo()`: enqueue each op in the returned array (in order).
- Add `applyBatch(pairs: {before: Markup; after: Markup}[])`: build `UpdateCmd[]`, `history.pushBatch`, enqueue all ops. Used by coalesced move/resize across the selection.
- Add `deleteSelected()`: collect selected markups → `DeleteCmd[]` → `pushBatch` → enqueue; clears `selectedIds`. (Single-select reuses the same path with one id.)
- `selectedMarkups` getter (derived): `markups.filter(m => selectedIds.has(m.id))`.

**Tests (`markup-store.test.ts` additions):** batch move of 2 markups → one `undo()` reverts both; mirror queue saw 2 update ops; `deleteSelected()` of 2 → one undo restores both, 2 remove ops drained; redo re-applies the whole frame.

**Exit:** vitest green (incl. updated command/store tests); `npm run check` 0. Commit `feat(g6): batch-undo transaction frames for multi-select edits`.

---

## G6.3 — Select tool: hit-test, click/shift/marquee select + chrome render
`Viewport.svelte` + `markup-render.ts` (chrome descriptor).

- **Tool gating:** `isCreateTool()` must stay FALSE for `select` (it already is — `select` isn't in draw/multiclick/ink/text sets). Add a parallel `isSelectTool(t) = t === "select"`. The overlay must capture pointer events when select is active too — extend the overlay `class:capture` / `pointer-events` gate to `isCreateTool() || isSelectTool()`, and branch overlay handlers by tool family so draw logic never runs under select.
- **Chrome:** compute selection bounds (union of `boundsOf` over `selectedMarkups`), map to screen (`pdfUserSpaceToScreen`), render a dashed bbox `<rect>` + 8 handle `<rect>`s (fixed screen size, e.g. 8px) — **handles only when** single selection AND `isRectResizable` (rect-based-first). A new `SvgSelection` descriptor in `markup-render.ts` (pure, tested) keeps the math out of the template: `selectionChrome(bounds, viewState, {showHandles}) → {box:{x,y,w,h}, handles:{id,x,y}[]}`.
- **Select gestures (pointerdown on overlay, select tool):**
  - hit `hitTest(store.markups, p, GRAB_PX / zoom)`:
    - hit + no shift → `selectedIds = new Set([id])`.
    - hit + shift → toggle id in a cloned Set.
    - miss + no shift → start **marquee** (drag rect); on up, `selectedIds = new Set(marqueeHits(...))`. miss + shift → marquee adds to existing.
  - Marquee preview = a dashed `<rect>` in screen space (live).
- **Tests (interaction):** click selects topmost (chrome appears); shift-click adds a 2nd (2 handles-less chrome / bbox spans both); click empty clears; marquee over 2 selects both; glued-on-zoom: selection bbox scales with wheel-zoom.

**Exit:** vitest green; `npm run check` 0. Commit `feat(g6): select tool — hit-test, shift + marquee multi-select, chrome`.

---

## G6.4 — Move + resize gestures (coalesced → one batch UpdateCmd)
- **Move:** pointerdown *inside* the selection bbox (hit on a selected markup, not on a handle) → enter move; pointermove updates a **live preview** (translate each selected markup's geometry by the running PDF delta — preview only, not committed); pointerup → `store.applyBatch(selected.map(before → after=translateGeometry(...)))` (one undo frame). Zero net delta → no-op (no command).
- **Resize (single rect selection only):** pointerdown on a handle → enter resize with that `HandleId` + the original bounds; pointermove → `newBounds = resizeBounds(orig, handle, p, MIN_PTS)`, preview = `scaleGeometryToBounds`; pointerup → one `UpdateCmd` via `applyBatch([{before, after}])`. Min-size clamp from `resizeBounds`.
- **Audit:** bump on commit — `modified_by`/`modified_at`/`revision+1` (small `bumpAudit(m, identity, now)` helper; G7 reuses it — put it in `markup-tools.ts` and unit-test). `before` = current store markup, `after` = clone with new geometry + bumped audit.
- **Preview rendering:** reuse the `previewMarkup`/preview-shape path or a `previewMarkups[]` for multi-move; ensure it re-derives from `viewState` (glued).
- **Cursor affordance:** handle hover → resize cursors; inside-selection → move cursor (CSS, best-effort).
- **Tests (interaction):** drag-move single rect → geometry shifted by exact PDF delta, `ipc.update` once, one undo reverts; drag-move multi (2) → both shifted, **one** undo reverts both; drag SE handle → max corner moves, min corner fixed (exact), one undo; resize min-size clamp; glued-on-zoom during move (preview scales).

**Exit:** vitest green; `npm run check` 0. Commit `feat(g6): coalesced move + rect resize → single undo per gesture`.

---

## G6.5 — Delete + keyboard
- In the existing `window` `onKeyDown` (guard: a doc is open, `editor` is null, not typing in a field): `Delete`/`Backspace` → `store.deleteSelected()`; `Escape` → clear `selectedIds` (in addition to the existing draw/multiclick cancels). Only act when `selectedIds.size > 0` for delete.
- **Tests:** select 2 + Delete → both gone, one undo restores both; Escape clears selection (no delete); Delete with empty selection = no-op; Backspace path covered.

**Exit:** vitest green; `npm run check` 0. Commit `feat(g6): Delete/Backspace removes selection, Esc clears`.

---

## G6.6 — Exit gates + housekeeping
- Full standing gates: `npm run test` (all green), `npm run check` 0 errors, `cargo test -- --test-threads=1` (unchanged — G6 is frontend-only; confirm still green), `cargo clippy --all-targets` 0, `cargo fmt`.
- Update `.claude/HANDOVER.md` (G6 done; note multi-select resize deferred to G6.1) + the KB.
- **No `/code-review` blocker** — G6 touches no annotation serde or render-raster path. (G8 does.) A `/code-review` of the move/resize math is still good hygiene before `/sendit` at G9.
- Commit any docs; leave G7 as next.

## Risks / watch-items
- **History return-type change (G6.2)** ripples into `markup-store` undo/redo + existing command tests — additive but touches G2 core; keep the single-command path byte-for-byte equivalent (length-1 frame).
- **Overlay event gating:** select must capture pointer events without re-triggering draw logic — the tool-family branch in the overlay handlers is the sharp edge (mirror the existing `isCreateTool` discipline).
- **Handle vs. move disambiguation:** test a pointerdown that lands on a handle does NOT start a move.
- **`$effect` on tool-switch** already resets draw gesture state; extend it to clear marquee/move/resize transient state too (not `selectedIds` — selection persists across tool switches by design? Decision: **clear selection when leaving the select tool** to avoid orphan chrome under a draw tool).

## Self-review
- Spec coverage: select/transform archetype (§6/§15) ✓; one-undo-per-gesture ✓ (G6.2 frames); §5 glued ✓ (chrome + preview re-derive from viewState, tested). Rect-resize-first is an explicit scope cut, logged, with a G6.1 follow-up.
- Type consistency: `Bounds`/`HandleId` local to `markup-select.ts`; `applyBatch`/`deleteSelected`/`selectedMarkups` added to `MarkupStore`; `pushBatch` + `MirrorOp[]` undo/redo in `History`; `bumpAudit` in `markup-tools.ts` (reused by G7).
