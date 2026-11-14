# Symbol search (find every copy of a luminaire symbol)

Added 2026-11-13. Ported from redcolumn (Apache-2.0), see `NOTICE`.

## Use

1. Draw a Rectangle markup tightly around ONE clean symbol and leave it selected.
2. `Quantities > Symbol search`: choose the scope (this page, or all pages of the document),
   Match (Loose / Normal / Exact), and whether to also find turned copies and other sizes.
3. `Find symbols`. Every hit is listed, grouped by page, all accepted. Untick a hit to reject
   it, `Show` jumps to it. The box you drew is flagged "your box" (it counts as one).
4. `Place N counts` creates one `MeasurementCount` marker per accepted hit in the active count
   set, as one undo step. Each page's markers form one count (`count_group`), carrying the
   optional label.

The helper rectangle stays in the document; delete it if you do not want it.

## How it matches

- **Vector (default when the box holds 4 or more line segments).** `RenderEngine::page_segments`
  returns the page's path segments (same extraction as the snap index, including Form XObject
  transforms). The template's longest line anchors the search: each page line of the same length
  is tried as its image, and the copy is kept when at least the Match share of the template's
  other lines are present within 0.75 pt. Rotations try any turn; sizes try 0.5x to 2x.
  Source: redcolumn `visualSearch.ts`.
- **Raster (scanned sheets, or any box with fewer than 4 line segments).** The page is rendered
  at 150 dpi (capped at 10 000 px on the long side, pages over 150 Mpx are skipped and reported),
  lightly blurred, and the box is matched by normalised cross-correlation, coarse then fine.
  Source: redcolumn `rasterSearch.ts`.
- All matching runs on tokio blocking threads. Only segment extraction and the page render touch
  PDFium, on the render thread, one page at a time, so tile rendering is not stalled by the
  match itself. At most `max(1, cpus/2)` (8 at most) pages' segments wait for matching at once, and a page whose match task fails is skipped (reason shown) without discarding the other pages' hits.

## Bounds

Per search: 2 000 matches per page, 20 000 in total (later pages are then skipped as "result limit
reached"), 20 s per page, 5 000 000 line segments per page, 5 000 pages, raster pages up to
100 Mpx. Hitting a match, time or total limit sets `truncated` and the panel says results may be
incomplete. Closing the window or starting another search cancels the running match tasks. The
box must lie on the page.

## Limits (measured, not guessed)

- The box defines the template. Stray line work inside it (a wall, a grid line) becomes part of
  the template, and copies without that line work are then not matched. Box one clean symbol.
- Vector matching anchors on the template's single longest line, so that line must be present in a copy. Of the other lines it allows `floor((1 - share) * lines)` missing (Normal: 1 of 12). A decoy
  that differs from the symbol by one line is a match at Normal; use Exact to reject it.
- Curves are matched by their end points (PDFium exposes no Bezier control points here), so a
  circle is a handful of chords. Template and page are reduced the same way, so this is
  consistent, but two different curved symbols with the same end points would not be told apart.
- Raster correlation does not separate near-identical symbols (an octagon with one cross arm
  against the full symbol) and is sensitive to a different background inside the box. It is the
  fallback for scans, not the primary path.
- Coordinates are DISPLAY space (rotation baked in, MediaBox lower-left as origin), the space of markup geometry, the rendered page and the viewer. `page_segments` converts PDFium's true-space path coordinates with `true_to_display`; the raster path is display space by construction. Tested on `/Rotate` 0/90/180/270 sheets with and without a non-zero MediaBox origin (`vector_and_raster_agree_on_rotated_and_offset_sheets`).

## Tests

`src-tauri/src/symbol_search/tests.rs`: fixture drawings with a known number of symbols
(clean sheet, decoys, endpoint jitter, turned and resized copies, a missing line, a 250 000
segment page), scored for precision and recall; PDFium tests write the same drawings to real
PDFs (vector, multi-sheet set, scanned image-only sheet). `timing_large_sheet` (ignored) times
10 A0 sheets of 600 000 segments each. Frontend: `src/lib/symbol-search.test.ts`,
`src/components/SymbolSearchPanel.test.ts`.
