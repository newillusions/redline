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

## Document structure: bookmarks, page labels, hyperlinks (behaviour spec)

Status (as of 2026-10-12): spec written BEFORE the build; implemented on `feat/navigation-structure`.
Gap matrix row 8 (`scout-redline-bluebeam-gap.md`) said bookmarks, labels and hyperlinks were "doc-only, no code".
Verified before starting: `git grep -E "Outlines|PageLabels"` over `src/`, `src-tauri/src/` on `origin/main` (929fea63)
found no reader or writer; the only `Link` handling was the preserve-untouched rule for foreign Link annotations
(`document/annots.rs`, `markup/preserve_tests.rs`). `App.svelte` carried the comment "Bookmarks/Layers remain unbuilt".

Labels: **V** verified (source named), **I** inferred (reasoned, or standard behaviour; not seen in Revu), **U** unknown
(not found; the redline choice is stated and is open to correction).

Sources (raw pages fetched with curl on 2026-10-12, converted to text and read in full; no summaries):
- BP = https://support.bluebeam.com/user-manual/menus/window/bookmarks-panel.html
- WB = https://support.bluebeam.com/user-manual/menus/window/create-edit-bookmarks.html
- BS = https://support.bluebeam.com/user-manual/menus/window/create-edit-bookmark-structure.html (Complete/Max only; not built)
- PL = https://support.bluebeam.com/user-manual/menus/window/edit-page-labels-page-numbers.html
- HT = https://support.bluebeam.com/user-manual/menus/tools/hyperlink-tool.html
- HI = https://support.bluebeam.com/user-manual/tutorials/hyperlinks.html ("Introduction to links in Revu")
- HP = https://support.bluebeam.com/revu/features/advanced/hyperlinks-and-paths.html
- ISO = ISO 32000-1:2008 (PDF32000_2008.pdf, https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf),
  sections 12.3.2.2 (explicit destinations, Table 151), 12.3.2.3 (named destinations), 12.3.3 (outline, Tables 152-154),
  12.4.2 (page labels, Table 159), 12.5.6.5 (Link annotations, Table 173), 12.6.4.2 (GoTo, Table 199), 12.6.4.7 (URI, Table 206).
- No tutorial video was transcribed ("How to create bookmarks" 2:35, "How to create hyperlinks and edit action" 3:00): anything
  only a video would show is U. No Bluebeam software was decompiled or instrumented. Revu's own on-disk bookmark/link keys were
  not captured, so redline reads and writes the standard ISO structures only (Bluebeam-private keys are carried verbatim).

### 1. What Revu does

| # | Behaviour | Label | Source |
|---|-----------|-------|--------|
| B1 | The Bookmarks panel shows the active PDF's bookmarks as a parent-child tree with a toggle arrow per parent. The order is user-defined (not alphabetical). | V | BP |
| B2 | Selecting a bookmark runs its action. | V | BP |
| B3 | A bookmark located on the page being viewed gets a triangular marker. | V | BP |
| B4 | Add Bookmark creates a bookmark for the current page and view: below the selected bookmark, or at the end of the list when none is selected. Add Before, Add After and Add Child (not available with no selection) are in the arrow menu and the right-click Add menu. | V | WB |
| B5 | A new bookmark is named after the page's label and is immediately editable. | V | WB |
| B6 | Rename: double-click, F2, or right-click Rename. | V | WB |
| B7 | Delete: right-click Delete, or select and press Delete. What happens to children of a deleted bookmark is not stated. | V / U (children) | WB |
| B8 | Reorder by dragging; a line shows the drop position; dragging inward nests under the item above. Ctrl-drag copies. Shift or Ctrl selects several. | V | WB |
| B9 | Six actions: Jump to Page (page number, scale Fit Page / Actual Size / Fit Width / Inherit Zoom), Jump to Place, Jump to Space, Snapshot View (a rectangle), Hyperlink (URL), Open (file). Jump to Page / Place / Space can target another open PDF. | V | WB, HT, HI |
| B10 | Right-click "Set to Current View" re-points a bookmark at the current page view. | V | WB |
| B11 | Properties: Title, Color (font colour), Style (Bold, Italic); applies to a multi-selection. | V | WB |
| B12 | Audit Bookmarks: a bookmark is broken when its action points at a page no longer in the PDF; a broken-bookmark icon shows; delete it or re-point it. | V | WB |
| B13 | "Save Collapse State" stores the current expand/collapse state as the file default (needs a file save). | V | BP |
| B14 | Create Bookmarks from the PDF's page labels (page range All / Current / Selected / Custom such as `1-3, 5, 9`) or from a page region (AutoMark text). | V | WB |
| B15 | Export Bookmarks (CSV or PDF, several files); Bookmark Structures (templates, dynamic numbering, Complete/Max only). | V | WB, BP, BS |
| B16 | Row order, selection after delete, drag-copy behaviour details, and what Revu stores for Place / Space / Snapshot actions. | U | |
| L1 | Page labels are separate from bookmarks and file names and appear in the Thumbnails panel. | V | PL |
| L2 | Rename a label directly: Thumbnails menu "Rename Page Label", F2, or double-click the label. Tab moves to the next label while editing, Shift+Tab to the previous. | V | PL |
| L3 | Number Pages dialog (Document > Number Pages, Thumbnails menu, thumbnail right-click): Style, Prefix (add a trailing space to separate it from the number), Start, Page Range (All / Current / Selected / Custom `1-3, 5, 9`). | V | PL |
| L4 | Text-only label: Style None + a prefix. Remove labels: Style None + blank prefix. | V | PL |
| L5 | Create labels from bookmarks (a page with a bookmark takes the bookmark's title) or from a page region. | V | PL |
| L6 | The PDF side: label ranges keyed by the first page index; page index 0 shall have an entry; style D, R, r, A, a or none; prefix; start >= 1 (default 1); letters run A..Z, AA..ZZ, ... | V | ISO 12.4.2 |
| L7 | How numbering runs across a non-contiguous custom range (`1-3, 5, 9`), the exact Style list in Revu's dialog, and what happens to labels when pages are reordered, deleted or inserted. | U | |
| H1 | Tools > Hyperlink shows the current PDF's hyperlinks highlighted in blue; Esc turns the view off. | V | HT |
| H2 | Create: drag a rectangle, or select text, then the Action dialog appears. | V | HT, HI |
| H3 | Same six actions as B9. Selecting a hyperlink runs its action; opening another PDF is foreground, Ctrl-select is background. | V | HT |
| H4 | Edit Action (right-click). Properties: Color, Line Width, Style (outline or fill), Visible. Delete: select and press Delete, or right-click Delete. | V | HT |
| H5 | The Links panel lists hyperlinks, has a text filter, selecting an entry jumps to the link, several can be selected. | V | HI |
| H6 | Hyperlinks are image-free dynamic content: not active in Presentation view. | V | https://support.bluebeam.com/revu/troubleshooting/hyperlinks-dont-work-in-presentation-view.html |
| H7 | Batch Link, Create Hyperlinks from URLs, links to Places, File Access links. | V (exist) | HT, HI |
| H8 | Which hand tool or mode follows a link on click; whether a link is visible without the Hyperlink tool (a "Visible" property exists, so the default is hidden). | I / U | HT |
| P1 | PDF structures: outline items are a /First /Last /Prev /Next /Parent linked list with /Count (negative = closed), /Title, /Dest or /A (never both), /C colour, /F flags (bit 1 italic, bit 2 bold). Destinations are `[page /XYZ left top zoom]`, `/Fit`, `/FitH top`, `/FitV left`, `/FitR l b r t`, `/FitB`, `/FitBH`, `/FitBV` (page = page object reference), or a name / byte string resolved through the catalog /Dests or /Names /Dests. A Link annotation has /Rect and either /Dest or /A; /A is a GoTo (/D) or URI (/URI) action here. | V | ISO 12.3.2.2, 12.3.2.3, 12.3.3, 12.5.6.5, 12.6.4.2, 12.6.4.7 |

### 2. What redline builds

Scope rule: structure that redline understands is read and edited; everything else in the file is carried through untouched.

| # | redline behaviour | Basis |
|---|-------------------|-------|
| RB1 | A **Bookmarks** panel (left sidebar, above Navigator) shows the outline as a tree. Initial expand state from the sign of /Count. Items whose target is the current page get a marker. Items with no usable target are shown dimmed; items whose page is gone are flagged "broken" (this is Audit, always on, with a count in the panel header). | B1, B3, B12 |
| RB2 | Click selects and runs the action. Page target: go to the page; if the destination carries a position, centre on it, and a zoom if it has one. URL target: ask first (RL4). Any other action type: nothing runs; the status line says the action is not supported. | B2, B9 |
| RB3 | **Add** creates a bookmark for the current page: `/XYZ` with the current left, top and zoom, below the selected bookmark (as its next sibling), or at the end of the top level when none is selected. The title is the page's label, falling back to `Page N`, and the row opens in rename mode. **More** holds Add before selection, Add child of selection (disabled with no selection) and **Add bookmark for the selected mark-up** (the mark-up's subject, else comment, else type as the title; a `/FitR` box around it, never smaller than 120 pt a side). | B4, B5 (view capture is **I**: the page says "current view"; the mark-up form is redline's reading of "create from selection") |
| RB4 | Rename: double-click, F2, menu. Delete: Delete key, menu; the item and its children are removed together, undoable. | B6, B7 (children: **U**, our choice) |
| RB5 | Reorder by drag and drop (drop above / onto = child / below), plus Move Up, Move Down, Indent (become a child of the previous sibling) and Outdent (become a sibling after the parent) from the More menu or Alt+Up / Down / Right / Left. A move into the item's own subtree is refused. Copy-drag, multi-select and Shift/Ctrl selection are not built. | B8 |
| RB6 | **Action** dialog: Jump to Page (page number or label, view Inherit / Fit Page / Fit Width / Actual Size) or Open URL. "Set to Current View" re-points at the current page and view. Place, Space, Snapshot View, Open File and other-PDF targets are not built (the PDF has no equivalent that Revu and redline share, **U**). | B9, B10 |
| RB7 | **Properties** (menu): Title, Colour, Bold, Italic for the selected bookmark. Written as /Title, /C, /F. | B11 |
| RB8 | "Save collapse state" writes the panel's current expand/collapse state into the file (/Count signs). The panel's own toggling does not touch the file. | B13 |
| RB9 | "Create from page labels": one top-level bookmark per page of a page range (`1-3, 5, 9`, `All`, `Current`) titled with the page's label, appended after existing bookmarks. AutoMark / page-region creation, Export Bookmarks and Bookmark Structures are not built. | B14, B15 |
| RB10 | Unsupported or foreign actions already in the file are preserved on edit and never run. | RL4, ISO |
| RL1 | Page labels are read from /PageLabels (number tree, depth- and size-bounded) and shown in: the thumbnail caption, the page indicator (`E-101` beside the number; the jump box accepts a page number or an exact label), the Markups List Page column (the label when there is one, else the number; sorting and the page filter still use the number), a **Page label** column in the Markups List and its export, a trailing **Page label** column in the Quantities export (added only when the document has labels; exports read the labels the Navigator last read, so a session that never loaded them, such as MCP alone, exports without), and the legend Pages column (labels listed, an unlabelled page by its number; both the on-screen table and the saved appearance). A page with no label shows its number. | L1 |
| RL2 | **Number Pages…** (thumbnail action bar, Document menu): Style (none, 1 2 3, I II III, i ii iii, A B C, a b c), Prefix, Start, Page Range (All, Current, Selected, Custom). Pages in the range are numbered in page order from Start; a non-contiguous range continues the count across the gap (`1-3, 5` starting at 1 gives 1, 2, 3 and 4 on page 5). Style none + blank prefix removes the labels of those pages. | L3, L4 (continuation across gaps **U**) |
| RL3 | Rename a label: double-click the thumbnail caption or F2 on the selected thumbnail; Tab / Shift+Tab are not built. The result is a text-only label for that page. | L2 (Tab: not built) |
| RL4 | Written as /PageLabels with a single /Nums array, an entry for page 0 whenever any page has a label. Consecutive pages with the same style and prefix and consecutive numbers merge into one range. When all labels are removed the entry is deleted. | L6 |
| RL5 | Rotate leaves labels alone. Delete, Reorder and Insert Blank carry each page's label with the page (the label belongs to the sheet, so `E-101` stays on that sheet). | L7 **U**: redline's choice, flagged for owner confirmation against Revu |
| RL6 | "Create labels from bookmarks" is built: pages that have a bookmark pointing at them take the bookmark title as a text-only label (page range as RL2). Page-region creation is not built. | L5 |
| RH1 | A **Link** tool (tool palette) puts the page into link view: every Link annotation on the page is outlined in blue, cursor is a crosshair; drag a rectangle to create; Esc leaves link view. | H1, H2 |
| RH2 | After the drag, a dialog offers Jump to Page (page number or label, view) or Open URL. Written as a Link annotation with /Rect, /Border [0 0 0] (hidden), and `/Dest` for a page target or `/A << /S /URI /URI (...) >>` for a URL. Text-selection linking is not built. | H2, H3, H8 |
| RH3 | In the Hand tool, clicking a link runs it: a page link navigates; a URL link asks first. The outline shows on hover. | H3, H8 (**I**: click-to-follow in the pan tool) |
| RH4 | A **Links** panel lists the document's links (page, target) with a filter box; click jumps to the link; Delete removes the selected link; Edit Action re-points it (page or URL). Link colour / width / fill / visible properties are not built. | H4, H5 |
| RH5 | Only GoTo (a destination, direct or as /A GoTo) and URI actions are understood. Every other action (Launch, JavaScript, GoToR, SubmitForm, Named, ...) is listed as "unsupported", never run, and never written. | ISO, security |
| RS1 | Edits write the PDF immediately, like page operations (atomic temp file + rename, render engine reloaded), through the same stale-file guard as every other write path: a file changed by another program refuses the write with the usual message. Unsaved mark-up edits are saved by the same write (exactly as for Rotate, Delete Page and Measure). Encrypted PDFs are read-only here. | existing page-op contract |
| RS2 | Undo / Redo buttons in the Bookmarks and Links panels (and Ctrl/Cmd+Z, Shift+Ctrl/Cmd+Z while the Bookmarks tree has focus) step through bookmark, label and link edits made in this session (one shared history, 30 steps). A rewrite of the file by another command (page rotate / delete / reorder / insert, flatten, optimise, redact, save-as, reload) clears the history. | task rule: delete/undo |
| RS3 | Untrusted input: outline walks keep a visited set and cap at 20 000 nodes and depth 32; page-label trees cap at depth 16, 10 000 ranges; destination and name-tree resolution cap hops and size; a link page cap of 5 000 Link annotations per page; strings are decoded with a length cap and rendered as text only. A cyclic, duplicated-node or oversized outline is shown read-only (never partially rewritten). | ISO, hostile-input rule |
| RS4 | URL safety: only `http`, `https` and `mailto`. Validation parses with the `url` crate and checks the parsed, normalised value: web addresses need `scheme://host` as written (no `https:host`, `https:///x`, backslash or scheme-relative forms), a host and no user name / password; non-ASCII hosts are refused (punycode is what is shown); the host and port must already be written the way the parser would write them (no decimal / hex / octal / short IPv4, no `%` or trailing / empty-label dots, no unicode dot look-alikes, no zero-padded or empty ports, canonical IPv6 only), so the parser and the opener cannot read them differently; a mail address is ONE plain address with at most `subject` and `body` (no `cc`, `bcc`, `attach`, `from`, other headers, extra recipients or fragments) and is rebuilt from those parts with RFC 6068 percent-encoding (a space is `%20`, a `+` is a plus); host names may carry `_` in any label but the last; whitespace and control characters (raw or percent-encoded), `"<>\^`{}\|` and over 2 048 characters are refused. Opening is two steps: the backend validates, normalises and remembers the address and returns it with a one-use token (`prepare_external_open`); the dialog shows exactly that text; only the user's yes (`confirm_external_open`, token only, 120 s) opens the stored address, through the OS opener with no shell, which is only ever given a string that is already the parser's own serialisation (re-checked at the opener; on Windows that opener is `%SystemRoot%\System32\rundll32.exe url.dll,FileProtocolHandler` by absolute path, the ShellExecute path, not run here). A PDF action never opens anything by itself, and neither command nor any navigation edit is reachable from the MCP bridge (guard test). | task rule |
| RS7 | Writing never destroys what it does not own: a new /PageLabels tree and a new /Outlines root are written and the catalog repointed; an old outline object is removed only when nothing else in the file references it (a hostile /PageLabels or /Outlines that points at a page, an annotation or a shared object leaves them intact); old label trees are left orphaned. The "referenced elsewhere" scan covers the trailer, every object at every depth (iteratively), stream dictionaries and the contents of object streams, and fails closed: if its node budget runs out nothing is deleted. References are matched by object number alone (any generation); a candidate whose number is shared with another object is kept; a candidate pointed at by a kept candidate (a kept outline item's /Next, /Parent, /First) is kept, so no deletion leaves a dangling reference; a non-candidate object that shares a candidate's number (another generation) counts as an outside source, so what it points at is kept; every deletion is followed by an in-code check that no reference to a missing (number, generation) was added, and is rolled back (nothing deleted) if one was or the check cannot finish; only objects with no /Type, /Outlines or /Annot (never a page, page-tree node, catalog, font or other typed object) are ever deleted. Deleting a link removes the object only if that scan says nothing else points at it (a link shared with another page or an outline action, or a page dictionary posing as a link, stays); /F is dereferenced and accepts an integer or an integral real, anything else is locked. A page-label tree cut short by the depth, range or step cap (or a loop) is read-only: label edits are refused and page operations leave /PageLabels alone. A link with the Locked or LockedContents flag cannot be deleted or re-pointed, and a link whose flags (or dictionary) cannot be read is treated as locked; the confirmation state fails closed too (a poisoned lock, wrong, empty, expired or reused token all refuse). | ISO, hostile-input rule |
| RS5 | Idempotence: opening and saving without editing navigation never rewrites outline, labels or links (they are not touched by the mark-up save path). Re-reading after any edit yields the same structure that was written. | task rule |
| RS6 | Not built (follow-ups): Bookmark Structures; AutoMark / page-region bookmarks and labels; Export Bookmarks; drag-copy; multi-select properties; Place / Space / Snapshot / Open-file / other-PDF actions; Batch Link and link-from-URL-text; link appearance properties; text-selection links; Extract Pages does not carry labels or bookmarks into the new file; Tab / Shift+Tab label editing; MCP tools; links are followed in single-page mode only (in reading mode bookmarks and the Links list still jump to pages); bookmark creation from selected *text*; a right-click menu on bookmarks (the More menu and keyboard cover it); deleting a page leaves bookmarks that pointed at it flagged broken rather than removing them. | |

### 3. Verification (as of 2026-10-12)

What was run, and what is still owed.

- **Rust**: `cargo test --all-targets` and `cargo clippy --all-targets -- -D warnings -A dead_code`. New: `src-tauri/src/document/navigation/` (outline, labels, links, URL allowlist, edit / undo layer) with hostile-input tests (outline cycles through /Next, /First and a shared entry, a depth and node cap, garbage structure; label-tree cycles, 200-deep nesting, an oversized range list, hostile start values; a cyclic name tree; a link cap), the JSON contract with `src/lib/navigation-ipc.ts`, three-cycle idempotence of the mark-up save path and of a no-op page-edit write over outline, labels and links, an unedited foreign Launch link preserved, and a failed edit leaving the file byte-identical.
- **Frontend**: `npm test`, `npm run check`. New unit and component tests for the helpers, the store, the Bookmarks and Links panels, the Number Pages and action dialogs, the link layer, the page indicator, thumbnails, the Markups List, and the Viewport (link following and drawing, destination jumps, `*.links.test.ts`).
- **Independent readers on a real file**: `REDLINE_NAV_SAMPLE_OUT=/tmp/navigation-sample.pdf cargo test --lib navigation_sample -- --ignored` writes a six-page set (labels i, ii, E-101 to E-104; nested bookmarks incl. a web address; page, URL and foreign Launch links; a mark-up). `qpdf --check` reports no errors; MuPDF 1.28.2 (PyMuPDF) and pypdf 6.6.0 read the outline tree, the labels and every link (incl. the Launch link as a launch, not run). The raw objects show `/Count` per ISO 12.3.3 (open +2, closed -2, root 6).
- **Real browser**: `tools/gui-harness-navigation.mjs` drives the real Svelte app in headless Chromium over a mocked backend: bookmarks tree, label in the page box, click-to-navigate, add and rename, undo, label in thumbnails, Number pages, the Link tool (draw, dialog, create, follow), the Links list, the confirm-before-opening dialog for a web address, and an unsupported action running nothing.
- **Owed, not done**: opening the sample in Revu and Acrobat to see the bookmarks, labels and links behave there (owner check; the file above is the one to open), a Windows run, and timing on a very large set (each navigation edit rewrites the file, like a page operation).


