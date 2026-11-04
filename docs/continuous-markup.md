# Mark-up in continuous scroll (reading mode)

Status: implemented 2026-11-05 (UX benchmark must-fix, `dev-reports/2026-11-04/redline-ux-benchmark.md`:
"Mark up in continuous scroll" - Revu, Acrobat, PDF-XChange and Drawboard all allow it).
Before this, reading mode was read/navigate only and the palette was disabled with the hint
"Switch to page view to mark up" (PR #162/#165). That hint and the disabling are removed.

## What the user gets

In reading mode (the 📖 toggle, Ctrl/Cmd+Shift+R) everything that works in page view works while
scrolling the column of pages:

- every palette tool, the Stamp picker, Tool Chest tools and the tool keys (R, M, V, ...);
- the Select tool: click, marquee, move, resize, vertex edit, group/ungroup, Delete, copy/paste;
- measurements, calibration and measurement viewports (PR #166), each on its own page;
- the right-click menus (PR #163): the markup menu, and the page menu, whose Rotate Page items
  rotate the page that was right-clicked (not the page at the top of the view);
- undo/redo (Cmd/Ctrl+Z): one step per action, the same history as page view.

The target of every gesture, key and page command is the **page under the pointer** (its page box
carries the class `reading-page-active`, with no visible styling), and a gesture never crosses
pages: a drag carried past the bottom of one sheet into the gap or onto the
next sheet ends at the first sheet's edge.

Keyboard zoom and fit keys (Cmd/Ctrl + = / - / 0 / 9 / 8), the page menu's Fit items and
Cmd/Ctrl+Arrow page keys act on the column. The Hand tool (and Space+drag) pans the column.

## Design

Each mounted page box in `ReadingView.svelte` hosts a `Viewport.svelte` in **page-slot mode**
(`slot` prop) as that page's mark-up layer, sized to the page box. The slot is the same component
that powers page view, so every tool, hit-test, measurement, dialog and menu is shared code rather
than a second implementation that would drift. In slot mode the Viewport:

- draws no tiles (the column already shows the page raster under it) and has a transparent
  background;
- never scrolls or zooms itself: its scroll offset is 0, its zoom follows the column's, and wheel,
  pinch and hand-drag are left to the column; zoom/fit/page keys and menu items are handed to the
  column through `onslotcommand`;
- shows no page navigator, zoom toolbar, shortcuts sheet or bench overlay;
- takes keyboard input only while it is the active page (`slot.active`), so one key press acts once;
- reports when it holds an unfinished gesture (`onslotbusy`): a drag, the gap between clicks of a
  polyline/polygon/cloud/measurement run, a first Callout click, an open text editor, dialog or
  menu, or the first calibration point while the Calibrate tool is on. Leaving the Calibrate tool
  between its two clicks cancels the calibration (like a viewport Redraw), so a first point can
  never leave its page busy and the other pages inert; once both points are in, the scale dialog
  owns it.

Space-bar temporary pan keeps the tool it replaced in the store (`MarkupStore.beginSpacePan` /
`endSpacePan`), not in a Viewport: only the active slot starts it, key-repeats are ignored, and
whichever slot sees the key-up restores the tool once - even if the page that started the pan
scrolled out and unmounted while Space was held. The slots share one user-identity lookup
(`sharedIdentity`) instead of one IPC call per page mount, and in slot mode the Select tool's
marquee band stops at the page edge, like the selection it makes.

Coordinates need no new transform in the slot: the slot's container is the page box, so the
page-view formula `screenToPdfUserSpace(client - boxRect, scroll = 0)` is the page's own transform,
and `getBoundingClientRect` already accounts for the column's scroll. A rotated page needs no special
case either: the render engine reports a rotated page's dimensions swapped, and mark-ups live in
that page's rotated user space, exactly as in page view. In slot mode points are clamped to the page
(`clampToPage`), which is what keeps a drag on its page.

`ReadingView` decides which page is active:

- `hoverPage` follows the pointer (`pageAtPoint`, a binary search over the column layout), but only
  while no button is down and no page is busy, so a drag or a multi-click run keeps its page;
- while a page is busy (`busyPage`), it stays active and stays mounted even if scrolled out of the
  render window (unmounting it would drop the half-made mark-up), and every other page's slot is
  `inert` (ignores the pointer);
- otherwise the active page is the current (topmost) page, so tool keys still work. A remembered
  hover page counts only while it is in the render window (`resolveActivePage`), and it is cleared
  when the pointer leaves the column and on every jump (search hit, Markups list, thumbnail, page
  field): after a jump far away, the page last hovered is unmounted, and without this no mounted
  page would take Delete, Escape, tool keys or Ctrl+C.

The pure helpers (`src/lib/reading-mode.ts`): `readingContentWidthCss`, `pageLeftsCss`,
`pageAtPoint`, `columnPointToPdf` (the column-to-page transform, used by the tests to pin the
slot's transform, including mixed page sizes and a rotated sheet), `clampToPage` and
`resolveActivePage`.

Page boxes are now positioned with an explicit `left` instead of `left: 50%; transform:
translateX(-50%)`. Two reasons: the right-click menu is `position: fixed`, which a transformed
ancestor would turn into page-relative; and a page zoomed wider than the view was clipped on its
left side (negative left), where it now widens the column and scrolls sideways.

## Performance

Virtualisation is unchanged: only the render window (visible pages plus one page each side) has
page boxes, and slots draw no tiles, so scrolling still fetches one raster per page.

A mark-up layer is a full Viewport, and mounting one costs a few milliseconds of script time. Two
measures keep scrolling at the old frame rate:

1. **Shared per-page index.** `MarkupStore.onPage(page)` (a `$derived` map from page to markups) and
   a derived `selectedMarkups` replace each slot's own walk of the whole document's markups. In a
   CPU profile of a scroll through 150 sheets x 30 mark-ups, those walks (Svelte proxy reads of
   4,500 markups per slot mount) were the largest script cost. Legend syncing walks only legends and
   runs from the active slot only.
2. **Layers mount on idle.** A page's layer mounts in a `requestIdleCallback` (one per idle slice,
   nearest page first, 250 ms timeout), not in the scroll frame that brings the page in. A fast
   fling shows the rasters at full frame rate and the mark-ups follow as it settles, like tiles
   sharpening after a zoom. The page under the pointer, and a busy page, mount at once. After a
   wheel scroll under a still pointer, the page under it is re-targeted once the scroll settles
   (100 ms), not every frame.

Measured with `tools/gui-harness-continuous-markup.mjs` (headless Chromium, mocked IPC, 150 sheets,
30 mark-ups per sheet, 1400 x 900, scroll 240 px per frame to the end and back; 3 runs each,
interleaved with the old build on the same machine). Indicative only: a dev Mac and a browser, not
the floor machine or the Tauri webview.

| Build | CPU | mean FPS | p95 frame | worst frame | frames > 50 ms |
|---|---|---|---|---|---|
| before (a0b7ba1, no mark-up layer) | 1x | 60 | 18.3-18.6 ms | 19.6-31.3 ms (one run 800 ms) | 0 (one run 10) |
| after | 1x | 60 | 18.1-18.4 ms | 20.0-26.6 ms | 0 |
| before | 4x slower | 60 | 19.1-19.3 ms | 21.7-25.8 ms | 0 |
| after | 4x slower | 59.8-59.9 | 20.9-21.2 ms | 50.6-66.4 ms | 1 |

After a jump to the last sheet its page box shows in 16-21 ms (before: 15-18 ms) and its 30
mark-ups in about 66 ms. Without the two measures above (every layer mounted in the scroll frame,
each walking all markups) the same run measured p95 27 ms at 1x and 30 ms at 4x, worst 64-70 ms.

## Known limits

- During a fast fling the mark-ups of pages entering the view appear a moment after the raster
  (see "Layers mount on idle").
- A text editor or the edge of a selection near a page edge is clipped by its page box.
- The Arrange bar and the I-beam text-selection bar belong to the active page's box.
- Previous view / Next view (Alt+Left / Alt+Right) is page-view only; in reading mode the page
  menu's view-history items are disabled.
- Pinch zoom in the column is not new here: the column zooms with Ctrl/Cmd/Shift+wheel as before.

## Checks

- Unit: `src/lib/reading-mode.test.ts` (page hit-testing in gaps, beside narrow pages and past the
  end; column-to-page transform for mixed sizes, a rotated sheet and a round trip through
  `pdfUserSpaceToScreen`; clamping; the active page after a jump), `src/lib/markup-store.test.ts`
  (`onPage`, derived `selectedMarkups`, Space-pan save/restore once),
  `src/components/Viewport.slot.test.ts` (keys from the active slot only; Space-pan across two slots,
  with a key-repeat on the other slot and with the arming slot unmounted; leaving Calibrate after
  the first point cancels it and frees the page; a calibration with both points is left to the
  dialog).
- Browser: `tools/gui-harness-continuous-markup.mjs` draws on page 1 and page 3 while scrolled,
  drags across a page gap, selects/deletes/undoes on page 3, rotates page 3 from its menu with page
  2 at the top of the view, measures on page 3, uses tool keys, checks keys after a jump to page 80,
  draws on page 4 after leaving Calibrate with one point on page 3, checks Space-pan across pages
  and after the arming page scrolled away, and prints the perf numbers above.
  `tools/gui-harness-tool-keys.mjs` checks the palette stays live in reading mode.
