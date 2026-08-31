# Bluebeam Revu Search — Behavior Reference

Distilled from official documentation and official video tutorials (owner
directive, 2026-08-31: "check bluebeam documentation and maybe online videos
as well to get a more in depth idea of how it works"). This is the acceptance
bar for redline's search feature — real Revu behavior, not a paraphrase of it.
Cross-referenced against redline's shipped implementation (PR #86 +
follow-up) below each section.

Sources (raw pages/transcripts fetched, not summaries-of-summaries):
- [Search panel — Bluebeam user manual](https://support.bluebeam.com/se/user-manual/menus/window/search-panel.html) (current, "se" locale mirror — the primary reference)
- [Visual search overview — Bluebeam Technical Support](https://support.bluebeam.com/revu/features/visual-search-overview.html)
- ["Bluebeam Revu: How to Search PDFs"](https://www.youtube.com/watch?v=5DSkf1kGwXs) — official Bluebeam channel, 111s, transcript pulled via captions
- ["Bluebeam Revu: How To Use Visual Search"](https://www.youtube.com/watch?v=oP6QGMDGlk0) — official Bluebeam channel, transcript pulled via captions

## 1. How to open it

`Window > Panels > Search`, or the shortcut `Alt+1` / `Ctrl+F`. redline: `Cmd/Ctrl+F` (macOS convention swap), matching keystroke intent — `src/lib/keyboard-shortcuts.ts::resolveSearchShortcut`.

## 2. Two search methods

- **Text search** — "faster and more reliable, but only works on actual text elements in the PDF." redline: PDFium-backed, `search_document`/Tantivy `search_folder`/`search_paths`. Shipped.
- **Visual search (VisualSearch™)** — searches for a graphical symbol/shape by drawing a rectangle around a sample instance (`Get Rectangle`), tuned by a sensitivity slider, with extra filters: multiple rotations, filter by color, search markups, "find detail", limit by selection. Requires Core/Complete/Max tier. **Not built, not attempted.** This is a real, substantial differentiator Bluebeam offers that redline does not replicate — it needs a vector/image symbol-matching engine, which is out of scope for the current `geometry` module (snap-target spatial index, not pattern matching). Named as a large, separate future project if ever prioritized — not a search-parity gap in the sense of "we forgot it," a capability redline's architecture doesn't have a path to cheaply.

## 3. Scope options (the dropdown)

Per the user-manual page, six scopes exist: **Current Document**, **Current Page**, **All Open Documents**, **Current Set** (only when a Set is open), **Recents**, **Folder** (with an "Include Sub-Folders" checkbox), and **Current Studio Project** (filename-only, requires being logged into a cloud Studio Project).

redline ships **five**: Document, Page, Open Docs, Recents, Folder+Subfolders (`src/lib/search-store.svelte.ts::SearchScope`). Two are deliberately not built:
- **Current Set** — Bluebeam's Set is a named/saved multi-file bundle. redline has no persisted grouping construct; "Open Docs" already covers "search everything I currently have open," which is the practical intent for a tool without cloud-project bundles.
- **Current Studio Project** — cloud collaboration, an explicit v1 non-goal (spec §2, "real-time collaboration/cloud sync").

## 4. Search options

Documented: Include Sub-Folders, Search Pages (default target), Search Filenames, Search File Properties, Search Form Fields, **Search Markups**, Case Sensitive, Whole Words Only.

**Correction to this PR's original framing**: earlier work described markup-comment search as "the differentiator Bluebeam gates behind higher tiers." That was wrong — the documentation confirms **Bluebeam already has a "Search Markups" option as a standard part of text search**, no tier gate mentioned anywhere in the manual or either video. redline's markup-text search (`src/lib/markup-search.ts`) is real parity, not an improvement invention — corrected in the PR body's shipped-items table. Filenames, file properties, and form fields are NOT searched by redline (no equivalent exists) — named as small follow-ups, lower priority than the bulk-action gap below.

Case Sensitive / Whole Words Only: shipped, `SearchStore.caseSensitive` / `.wholeWord`.

## 5. Result display

"When searching multiple documents... the search results are grouped by document." Clicking a result jumps to that location. From the Visual Search video: **the minus-sign icon collapses all groups, the plus-sign icon expands all** (a single global toggle, not per-group only), and **Check All / Uncheck All** buttons select/deselect every result at once.

redline: grouped-by-file list with per-group collapse (`toggleGroupCollapsed`) — shipped in the original PR. **Global** Collapse All / Expand All and Check All / Uncheck All were the gap this addendum closes (`SearchStore.collapseAll/expandAll/checkAll/uncheckAll`, `SearchPanel.svelte`'s `.check-options` toolbar).

## 6. Bulk actions on checked results — "Check Options" (the lightning-bolt menu)

This is the single largest gap the original audit missed, and the reason the owner asked for a documentation/video pass before finalizing. Quoted directly from the official transcript ("How to Search PDFs"):

> "You can also quickly select multiple results or deselect all at once. For additional actions, click the lightning bolt icon to hyperlink, mark for redaction, highlight, or count each selected item. If you need to replace any text result, simply select it and choose replace checked to input your new term and font."

And from "How To Use Visual Search," the fuller action list:

> "...click the lightning bolt icon and choose from several options, including hyperlink checked, mark for redaction, and apply count measurements to checked... Other actions include applying a highlight, underline, strike through, or squiggly line under the results."

Full documented action set: **Hyperlink Checked, Mark for Redaction, Apply Count Measurement to Checked, Highlight Checked, Underline Checked, Strikethrough Checked, Squiggly Checked, Replace Checked** (Search & Replace, text-only, PDF content layer text — cannot replace text inside an unflattened markup).

### Shipped this pass
- **Highlight Checked** — `App.svelte::applyHighlightToChecked`, reuses the existing `Highlight` `MarkupType` and the same Quads-geometry pattern the I-beam text-selection tool already uses (`Viewport.svelte::commitTextSelectionHighlight`). Scoped to text-kind hits belonging to an already-open tab; a checked hit for a not-yet-open folder/recents file is skipped and counted in the status banner rather than silently dropped or auto-opening every referenced file.

### Named follow-ups, not built this pass (each is real scope, not a stub)
- **Underline / Squiggly / Strikethrough Checked** — redline's `MarkupType` enum has no `Underline`/`Squiggly`/`StrikeOut` variants at all. Each needs: a new Rust `MarkupType` case, PDF annotation-subtype read/write in `document/annots.rs` (`/Underline`, `/Squiggly`, `/StrikeOut` per ISO 32000-1 12.5.6), an SVG render case in `markup-render.ts` (mirrors the Highlight quad-rendering, different stroke), and TS union updates. Roughly three new markup types with full round-trip support — a distinct, TDD-able PR.
- **Hyperlink Checked** — redline has no `/Link` annotation concept modeled anywhere (Markup is annotation-object-shaped, not link-shaped). This is new domain modeling, not an extension of an existing type.
- **Mark for Redaction from search results** — redline's redaction (`docops::redact`, M5) is a manual rectangle-region tool, not integrated with search hits. Wiring a search hit's rect into the existing redact pipeline is plausible but needs its own design pass (redaction is described in CLAUDE.md as "rasterize-the-region safe floor" — a genuinely destructive operation that should not be a one-click bulk action without deliberate UX).
- **Apply Count Measurement to Checked** — `MeasurementCount` markup type exists and is used for takeoff counts, but "place a count marker at a text search hit's location" is a placement/geometry design decision (where exactly does the marker anchor relative to the matched text?) that hasn't been made.
- **Replace Checked (Search & Replace)** — content-stream text mutation. Real doc-surgery risk (irreversible if mishandled, touches the same lopdf save path judgment.md already flags for numeric-precision bugs) — deserves its own TDD pass and should probably get explicit owner sign-off before shipping, given "search and replace across a whole folder of construction drawings" is the kind of action a mistake in is expensive to discover late.

## 7. Recents scope, mechanically

The manual doesn't specify whether Recents indexes files without opening them or requires a prior open. redline implements it as an on-the-fly, non-persistent lopdf text extraction per path (`search_paths` Tauri command, reusing `search::indexer::extract_pdf_text` — the same extraction folder search uses), searching whatever paths are in the existing MRU list (`recentDocs`, already tracked by `src/lib/recent-docs.ts`). A moved/deleted MRU entry is skipped, not an error.

## 8. Open question for the hands-on Revu session (mr-desktop)

Documentation does not say definitively whether clicking a Folder- or Recents-scope result for a file that is **not currently open** in Revu (a) opens it automatically, (b) requires a separate action, or (c) shows a preview without opening. redline's behavior (built in the original PR, unchanged here) is (a) — `App.svelte::handleSearchJump` calls the existing dedup-aware `openFilePath` and then jumps. This should be confirmed against the real product once mr-desktop is free; if Revu's actual behavior differs, this is a one-line change (swap open-then-jump for an intermediate confirm step) and is flagged, not silently assumed correct.
