# Navigation — pan, zoom, and keyboard shortcuts

Status: shipped 2026-09-08 (owner-decided pan/zoom scheme, fixing the laptop complaint
that a two-finger trackpad swipe zoomed instead of panning). Implementation:
`src/lib/viewport.ts` (`classifyWheelEvent`, `parseZoomPercent` — pure, unit-tested) and
`src/components/Viewport.svelte` (`onWheel`, `onGestureStart/Change/End`, the space-bar
hand-pan handlers, the toolbar zoom-percent input).

## Pan

- **Two-finger trackpad swipe** (macOS and Windows precision touchpads), unmodified — pans,
  including diagonally. This is a plain `wheel` event with no `ctrlKey`/`metaKey`/`shiftKey`;
  both axes are applied directly to scroll position. No axis lock here — a trackpad swipe's
  own two axes are hardware/OS-determined already.
- **Mouse wheel**, unmodified — pans vertically. A mouse with a tilt-wheel's `deltaX` pans
  horizontally the same way, with no modifier needed.
- **Space held + drag, or click-drag with the hand tool** — mouse-driven pan, forgiving
  axis-locked (owner feedback 2026-09-15): a near-horizontal or near-vertical drag locks to
  that axis for straight panning; a genuinely diagonal drag (within ~32°-58° of horizontal)
  pans both axes freely instead of snapping to the nearer one. The lock is decided from the
  first ~9px of movement (not the first event) and re-evaluated mid-drag on a pause (>150ms
  with no move event) or a decisive turn against the current lock — so panning right, then
  pausing, then panning down re-locks to vertical rather than staying stuck horizontal. Pure
  decision logic: `$lib/viewport.ts`'s `resolvePanLock`/`isSharpTurnAgainstLock`/
  `panLockAxes`; segment state (origin/scroll-baseline/lock) lives in `Viewport.svelte`'s
  `onMouseDown`/`onMouseMove`. Survives the pointer leaving the viewport bounds mid-drag
  (window-level `mousemove`/`mouseup` + a `blur` listener for alt-tab) — releasing Space
  restores whatever tool was active before. Does not fire while typing in the Text/Callout
  inline editor or a toolbar input.

## Zoom

- **Pinch** (trackpad) — zooms at the fingers.
  - Cross-platform primary path: a pinch is delivered as a `wheel` event with
    `ctrlKey: true` on every platform this app ships to (macOS WKWebView, Windows
    WebView2/Chromium) — handled by the same `wheel` listener as pan, just routed to zoom
    instead.
  - macOS also dispatches WebKit-only `gesturestart`/`gesturechange`/`gestureend` events
    for the same physical pinch; `gesturechange`'s continuous `scale` is used for a
    smoother zoom curve there. Windows WebView2 never fires these events, so the
    `ctrlKey`-wheel path above is the only pinch-zoom route on Windows. A guard
    (`gestureActive`) stops the two paths from double-applying one pinch on macOS.
- **Ctrl/Cmd + mouse wheel** — zooms (mouse-user equivalent of a pinch).
- **Shift + two-finger swipe or mouse wheel** — zooms live at the cursor, using the vertical
  motion (owner amendment 2026-09-08 — Shift is a zoom trigger here, not a horizontal-pan
  modifier). On a Windows mouse wheel, Chromium/WebView2 remaps the Shift+wheel delta into
  the horizontal axis and zeroes the vertical one at the browser level; the zoom logic
  falls back to that horizontal delta when the vertical one is zero, so Shift+wheel zoom
  still works there rather than silently doing nothing.
- **Ctrl/Cmd + `=` / `+`** — zoom in. **Ctrl/Cmd + `-` / `_`** — zoom out.
- **Ctrl/Cmd + `0`** — fit width. Legacy alias: Ctrl/Cmd + `1`.
- **Ctrl/Cmd + `9`** — fit height. Legacy alias: Ctrl/Cmd + `2`.
- **Ctrl/Cmd + `8`** — actual size (100%). Legacy alias: Ctrl/Cmd + Shift + `0`.
- **Zoom-percent toolbar input** — next to the Fit W / Fit H / 100% buttons; type a number
  (e.g. `150`) and press Enter to zoom to that percentage, clamped to the app's zoom range
  (10%–800%). Shows the live zoom, rounded, while not focused.

## Page navigation

- **Ctrl/Cmd + ArrowLeft / ArrowRight** — previous / next page.

## Reading mode

Status: shipped 2026-09-15 (owner request). Implementation: `src/lib/reading-mode.ts`
(layout + virtualization math — pure, unit-tested) and `src/components/ReadingView.svelte`
(the continuous-scroll viewer, mounted by App.svelte in place of `Viewport.svelte` when
active). Single-page mode (this whole file, above) is unchanged and unaffected — reading
mode is a separate sibling component, not a rewrite of the tiled viewer.

- **Toggle** — the 📖 toolbar button, or **Ctrl/Cmd+Shift+R**. Persisted (`viewer_mode` in
  settings.json) so the choice survives a restart; applies across every open tab, not
  per-document.
- **Layout** — every page of the document stacked in one vertically scrolling column,
  fit-to-width by default. Each page keeps its own aspect ratio (a mixed-size sheet set
  is not forced to a common width).
- **Zoom** — Ctrl/Cmd+wheel or Shift+wheel zooms live at the cursor position's page (same
  trigger as single-page mode); the **Fit W** button and the zoom-percent input work the
  same way as the single-page toolbar's. A plain wheel/trackpad swipe is NOT
  intercepted — native scrolling of the column already IS panning in a continuous view.
- **Page indicator** — the toolbar's "current / total" control (`PageIndicator.svelte`)
  tracks the topmost visible page as you scroll, and its field is editable: type a page
  number and press Enter to jump there (Escape discards the edit).
- **Rendering model** — deliberately simpler than the single-page tile grid: each page is
  rasterized whole (one `render_tile` call sized to the page, not a 512px tile grid), and
  only pages within the visible range plus a one-page lookahead are ever fetched or kept
  cached — scrolling through a long document never holds more than a handful of page
  rasters in memory. See `ReadingView.svelte`'s header comment for the full scoping
  rationale, including the extreme-zoom raster cap on very large sheets.
- **Scope** — read/navigate only: markup creation and editing are not available in reading
  mode in this release; switch to single-page mode for markup work.

## Discoverability

A `[?]` button next to the zoom controls (and the `?` key) opens a shortcuts cheat-sheet
overlay listing the pan/zoom/page-navigation bindings above — dismiss with Esc or a click
outside the card. Added 2026-09-15 after owner feedback that Shift+wheel zoom, while
working, was easy to forget. Discovery only; no binding changed.

## Zoom toolbar: hide/show and reposition

Added 2026-09-15 (owner feedback: "how do we get rid of the zoom % window" plus a
follow-up asking for the toolbar itself to be hideable and movable):

- The always-on zoom-percentage HUD (a small floating "100%" box, independent of the
  toolbar) is **removed** — it duplicated the toolbar's own zoom-percent input. Its "last
  tile" render-time stat and the "[B] bench" hint now live inside the `B`-key bench overlay
  instead, visible only while that's toggled on.
- The zoom toolbar (Fit W / Fit H / 100% / zoom-percent input) can be **collapsed** via its
  own `×` button or the `T` key, leaving a small `⚙` re-open handle at the same corner —
  and **repositioned** by dragging its `⠿` grip to any of the 4 viewport corners, snapping
  to the nearest one on release. Both the collapsed state and the chosen corner persist
  (`$lib/viewport.ts`'s `loadToolbarPrefs`/`persistToolbarPrefs`, localStorage — the same
  per-viewer-convenience pattern as `search-store.svelte.ts`'s scope persistence).

## Windows: WebView2's own native zoom control must stay disabled

Added 2026-09-15 (PR #117), after an owner report that pinch "didn't seem to work" on a
Windows trackpad. The `ctrlKey`-wheel path above was already correct — Chromium
synthesizes a `ctrlKey: true` wheel event from a Windows Precision Touchpad pinch, and
`classifyWheelEvent` already routes it to zoom. The real cause: WebView2 has its own
**native** zoom control (`ICoreWebView2Settings.IsZoomControlEnabled`, default `TRUE`) that
reacts to the same Ctrl+wheel/pinch-synthesized-ctrl+wheel input at the WebView2 host
layer, above the DOM — not suppressible via this page's own `e.preventDefault()` in
`onWheel` — and scales the whole rendered webview surface as a second, uncoordinated zoom
on top of the app's own tile-based one. `src-tauri/src/lib.rs`'s
`disable_native_zoom_control_on_windows` (Windows-only, runs at window setup) turns it off
so the JS zoom path above is the sole zoom mechanism on Windows, matching macOS (no
equivalent competing native zoom there).

## Page thumbnails and extraction

Status: shipped 2026-09-15 (owner request). Implementation: `src/components/
ThumbnailPanel.svelte` (the Navigator panel, left side) and `src/components/
ExtractPagesDialog.svelte`.

- **Thumbnails** — a low-resolution raster of every page, rendered lazily as it scrolls
  into view, with the currently-viewed page highlighted. Click a thumbnail to jump there
  (also selects just that page).
- **Selection** — click selects one page (and navigates); **Ctrl/Cmd+click** toggles a
  page in or out of a multi-select without navigating; **Shift+click** selects the
  contiguous range from the last plain click to the clicked page.
- **Extract** — once anything is selected, an action bar offers **Extract…**, which opens
  a small dialog confirming the page count/range, then a native Save-As dialog to choose
  where to write the new PDF. Extraction never modifies the source document; the new file
  contains only the selected pages, in ascending page order (click order does not reorder
  the output).

## Known gap

Real-device pinch behaviour (macOS trackpad, Windows precision touchpad) has not been
exercised on physical hardware as part of this change — verified only via unit tests
against synthetic wheel/gesture event shapes, plus (for the Windows fix above) an isolated
scratch-crate compile check against the `x86_64-pc-windows-msvc` target. Confirm on a real
trackpad before treating the pinch UX as fully validated — see PR #117's owner test
checklist.
