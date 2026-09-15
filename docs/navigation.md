# Navigation — pan, zoom, and keyboard shortcuts

Status: shipped 2026-09-08 (owner-decided pan/zoom scheme, fixing the laptop complaint
that a two-finger trackpad swipe zoomed instead of panning). Implementation:
`src/lib/viewport.ts` (`classifyWheelEvent`, `parseZoomPercent` — pure, unit-tested) and
`src/components/Viewport.svelte` (`onWheel`, `onGestureStart/Change/End`, the space-bar
hand-pan handlers, the toolbar zoom-percent input).

## Pan

- **Two-finger trackpad swipe** (macOS and Windows precision touchpads), unmodified — pans,
  including diagonally. This is a plain `wheel` event with no `ctrlKey`/`metaKey`/`shiftKey`;
  both axes are applied directly to scroll position.
- **Mouse wheel**, unmodified — pans vertically. A mouse with a tilt-wheel's `deltaX` pans
  horizontally the same way, with no modifier needed.
- **Space held + drag** — temporary hand-pan, regardless of which tool is active (a draw
  tool, the select tool, etc.). Releasing Space restores whatever tool was active before.
  Does not fire while typing in the Text/Callout inline editor or a toolbar input.

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

## Known gap

Real-device pinch behaviour (macOS trackpad, Windows precision touchpad) has not been
exercised on physical hardware as part of this change — verified only via unit tests
against synthetic wheel/gesture event shapes. Confirm on a real trackpad before treating
the pinch UX as fully validated.
