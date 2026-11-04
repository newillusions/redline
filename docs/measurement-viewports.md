# Measurement viewports (per-area scales)

UX benchmark must-fix 7 (2026-11-04): lighting sheets often put a 1:100 plan next to 1:20 or
1:50 details. A **viewport** is a named rectangle on one page with its own scale, the way
Bluebeam Revu does it ("different scales for different areas"). It is stored as a scale
record and written to the PDF as a standard ISO 32000-1 viewport dictionary, so Revu and
Acrobat read it too.

## Using it

- **Viewport tool** (palette, next to Calibrate): drag a rectangle, then name it and pick a
  drawing scale (1:1 ... 1:1000, or 1/16" ... 1" = 1'-0") or choose "Calibrate", then click
  two points of a known length inside it.
- While a measuring, calibrate or viewport tool is active, each viewport on the page is
  outlined with its name and scale.
- **Quantities panel > Viewports** lists every viewport (name, page, scale):
  - **Edit**: rename, change the drawing scale, or recalibrate with two points.
  - **Redraw**: drag a new rectangle for it (move or resize), on any page. Picking any
    other tool (or Escape) cancels a Redraw, so a later New viewport never moves it.
  - **Re-apply**: put the measurements that start inside it under its scale.
  - **Delete**: asks first, and says how many measurements were taken under it; they keep
    their values.
- Creating, editing, redrawing and deleting a viewport are each ONE undo step together with
  the measurement changes they cause (Ctrl+Z restores the viewport and the quantities).
- A failed viewport action is shown in the error banner. If the scales of a PDF's own `/VP`
  viewports cannot be saved next to the file (read-only folder), they still apply for the
  session and a banner says so.
- The Scale column of the quantities table names the viewport a measurement was taken under.

## Which scale a measurement uses

The rule follows ISO 32000-1:2008 §12.9:

1. A measurement uses the scale of the viewport that contains its **first point**
   (§12.9 NOTE 1: "Any measurement that potentially involves multiple viewports, such as one
   specifying the distance between two points, shall use the information specified in the
   viewport of the first point"). A line that starts inside a 1:20 detail and ends on the
   1:100 plan is a 1:20 measurement.
2. Where viewports overlap, the **last-drawn** one wins (§12.9: the `/VP` dictionaries are in
   drawing order and "shall be examined, starting with the last one and iterating in reverse,
   and the first one whose BBox entry contains the point shall be chosen").
3. Outside every viewport: the page's scale, else the document default.

The first point is the first vertex of a line, polyline or polygon, a count's point, and a
rectangle's lower-left corner. Code: `takeoff::scale::scale_at` (Rust) and `scaleAt` in
`src/lib/polylength.ts` (frontend); the edge of a rectangle counts as inside.

## Existing measurements keep their values

A measurement remembers the scale it was taken under (`scale_ref`). It is recomputed only
when that scale itself changes or the user acts:

| Action | Effect on measurements |
| --- | --- |
| Draw a viewport over existing measurements | None, except unscaled ones (no scale yet, quantity 0) inside it, which take its scale |
| Recalibrate a viewport, a page or the document (same slot keeps its scale id) | Measurements taken under that scale follow the new calibration |
| Move or resize a viewport (Redraw) | None. A measurement now outside keeps the viewport's scale |
| Re-apply (explicit) | Measurements starting inside go under the viewport's scale, one undo step |
| Delete a viewport (confirmed) | None: the viewport is retired, not removed, so its measurements keep the values they were taken with (at the next save and export too). Undo brings it back |

Rule, in `takeoff::math::recompute_all` and `recomputeAll`: a measurement whose scale still
exists and still applies where it lies (the default, its page's scale, or a viewport that
contains its first point) is recomputed under that scale; one pinned to a scale that does
not apply there - including a retired (deleted) viewport - is left as it is; one with no
scale, or a scale id that no longer exists at all (e.g. from an older recalibration), goes
under the scale at its first point. A recalibration keeps the scale's id (`commands::takeoff::store_scale`),
so a measurement taken under the page scale inside a viewport is not taken over by the
viewport when the page is recalibrated.

## Data model

`ScaleTarget::Viewport { page, viewport_id, name, bbox }` (`src-tauri/src/takeoff/scale.rs`),
next to `Page` and `DocumentDefault`. `bbox` is `[x0, y0, x1, y1]` in redline's page space
(the space markup geometry uses). The scale list order is the drawing order; an edit
replaces the record in place. `viewport_id` is the region's identity (survives rename, move
and recalibration); the scale `id` is what measurements reference. `retired: true` marks a
deleted viewport (hidden, never picked, not written to the PDF, kept so its measurements
keep their values and the delete can be undone).

Commands: `add_viewport`, `update_viewport` (rename / page + bbox / recalibrate, keeps the
id), `put_scale` (exact put or removal, no recompute: what undo/redo replays),
`delete_scale` (a viewport is retired), `list_scales`, `take_scale_notices`. The MCP `list_scales` lists live viewports too (retired ones, kept only for their measurements, are left out); MCP `add_scale` creates a viewport when given `applies_to_page` + `viewport_name` + `viewport_bbox`. A page whose geometry cannot be read when `/VP` is synced keeps its viewport scales through a page operation and queues a notice (shown after any scale or page operation).

## Storage

- **Sidecar** (`<file>.redline/meta.json`, `scales`): the source of truth, like page scales.
  Written immediately by every viewport command. `pdf_viewports_seen` records which `/VP`
  entries of the PDF were already imported.
- **PDF `/VP`**: every save (and every page operation, before it runs) rewrites the pages'
  `/VP` arrays from the viewport scales (`takeoff::measure::sync_viewports`). redline's own
  entries carry the private key `/RLViewportId` (readers ignore unknown keys) and are
  replaced. A region viewport imported from another application is kept byte for byte (its
  feet-and-inches formats, `/O`, `/Y` and private keys) while its scale is unedited -
  `pdf_viewports_seen` keeps the record as imported to tell; once the user edits it, it is
  replaced by redline's copy, and once deleted it is removed. Whole-page entries (`write_page_measure`, other apps' page calibrations),
  non-rectilinear viewports (`/Subtype /GEO`) and other apps' viewports never imported are
  left alone.
- **Page operations**: viewports move with their pages (reorder), follow a rotation, and
  are removed with a deleted page (`follow_viewports_after_page_op`).

### `/VP` keys written

Checked against the text of ISO 32000-1:2008 (Adobe PDF32000_2008.pdf, §12.9, Tables 260-263):

```
/VP [ << /Type /Viewport                      % Table 260
        /BBox [x0 y0 x1 y1]                   % required, default user space (true page space)
        /Name (Detail A)                      % text string (UTF-16BE when not ASCII)
        /RLViewportId (<uuid>)                % redline's own key
        /Measure << /Type /Measure /Subtype /RL               % Table 261
                    /R (1:20)                                % required: scale ratio text
                    /X [<< /Type /NumberFormat /U (m) /C <m per pt> /F /D /D 100 >>]  % required
                    /D [<< /Type /NumberFormat /U (m) /C 1 /F /D /D 100 >>]           % required
                    /A [<< /Type /NumberFormat /U (sq m) /C 1 /F /D /D 100 >>]        % required
                 >> >> ]
```

`/O` (origin) is omitted: Table 262 gives its default as "the first coordinate pair
(lower-left corner) of the rectangle specified by the viewport's BBox entry". `/D` in a
number format is the precision as a power of ten (100 = two decimals, Table 263). The bbox
is converted from redline's page space to the PDF's true default user space (page
`/Rotate` and a non-zero `/MediaBox` origin), with the same transforms markups use
(`geometry::display_to_true`). `write_page_measure` (whole-page `/VP`) puts its entry FIRST
in `/VP` and replaces any earlier whole-page entry: readers take the last entry that
contains a point, so a whole-page entry after a region would hide the region's scale. It
now writes the same spec-complete dictionary (it previously omitted the required `/R` and `/A`, wrote `/O` as a
number format instead of two numbers, and `/D` as a digit count).

### Reading another application's `/VP`

On open (the same parse that reads the annotations), every rectilinear viewport is read:
ratio = `/X[0] /C` x `/D[0] /C`, unit = `/D[0] /U`, precision from the last `/D` format,
label = `/R`, name = `/Name`. `takeoff::measure::plan_viewport_import` decides:

- a region viewport not seen before becomes a viewport scale (unnamed ones become
  "Viewport N");
- a whole-page viewport becomes the page's scale only when the page has no scale and there
  is no document default;
- an entry redline wrote whose viewport is missing from the scales (lost sidecar, or a
  deletion that was never saved) comes back with its id.

Imported scales are saved to the sidecar at once, so a sidecar is created next to a PDF that
carries `/VP` viewports the first time it is opened.

## Known limits

- Viewports are rectangles only (Revu also allows polygons).
- A viewport has one scale for both axes (the spec's optional `/Y` and `/CYX` are not
  written; a foreign viewport with a different Y scale is read with its X scale).
- Measurement annotations do not yet carry their own `/Measure` dictionary, so other
  applications show redline measurements as plain lines; the viewport scale itself is
  readable.
- The Quantities totals add lengths regardless of unit; keep viewports on one sheet in the
  same unit.
- Precision is read and written up to 6 decimal places; a `/Measure` entry redline cannot
  use (another subtype, a zero or missing conversion factor) is not imported and is kept in
  the file as it is.
- `/BBox` is mapped through the page's `/MediaBox` (and `/Rotate`), as for markups; `/CropBox`
  is not used.
