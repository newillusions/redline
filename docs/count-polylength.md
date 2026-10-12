# Count and Polylength: behaviour spec

Status (as of 2026-10-12): implemented in PR feat/count-polylength-parity. Revu-parity stage for the takeoff tools Emittiv uses for luminaire take-offs and cable/track runs.

Label legend (same as the scouts): **V** verified (source named), **I** inferred, **U** unknown (not found; our choice, flagged).

Sources:
- `scout-revu-behaviour.md` 2.6 Count, 2.7 Length/Area/Polylength + calibration (video id@seconds, captions are auto-generated).
- `scout-redline-bluebeam-gap.md` rows "Count tool with sets, Resume Count, split count", "Area cutouts", "recompute on recalibrate".
- Bluebeam boost-takeoff-workflows-with-count-tool, multiple-measurement-values, set-the-page-scale-on-drawing (support.bluebeam.com, listed by the gap scout; not re-fetched this stage).

No Bluebeam software was decompiled or instrumented.

## 1. Count

| # | Behaviour | Label | Source |
|---|-----------|-------|--------|
| C1 | Click places a count item; no scale needed. | V | uyZkTkKYmMo@0-25 |
| C2 | Items placed in one run (no Esc, no tool change, same page, same set) are ONE count; starting a new run makes a new count. | V | uyZkTkKYmMo@21-41 |
| C3 | Resume: right-click a finished count, "Resume count", to keep adding to it. | V | uyZkTkKYmMo@21-41 |
| C4 | Split Counts: selected items become a new count; Split All: one count per item. | V | uyZkTkKYmMo@143-228 |
| C5 | Merge Counts: select 2 or more counts, they become one. | V | uyZkTkKYmMo@143-228 |
| C6 | Totals appear in the Markups List. | V | uyZkTkKYmMo@7-25 |
| C7 | Count, and Merge only of counts of the same type (symbol/set). Merging different types is refused. | I | Revu merges "counts of the same type" per the task brief; Revu's refusal message is not captured |
| C8 | A count lives on one page; Resume/Merge never cross pages. | I | A Revu count markup is a single-page annotation; redline items carry a page |
| C9 | Per-count subject (label) and colour. | I | Revu counts are ordinary markups with Subject and Color; marker size, numbering: U |

### Where the commands live (redline)

redline has no right-click context menu infrastructure, so the Revu right-click commands are exposed as buttons (C-UI = our choice, **U** for Revu parity of placement):

- **Quantities panel (bottom)**: each count group is a row (label, colour swatch, total, page). Row buttons: Resume, Select. Panel also shows the per-set subtotals as before.
- **Properties panel**, when one or more count items are selected: a "Count" section with Label, Colour, and the buttons Resume, Split selected (new count), Split all (one count per item), Merge (selection spans 2+ counts).
- **Markups List**: one row per count group (not per item), Measurement column = group total. Selecting the row selects every item.

### Data model

- Each item stays one `MeasurementCount` markup (point), exactly as before. A count is the set of items sharing `Markup.count_group` (new optional UUID). No count_group = a count of one (legacy documents unchanged).
- Totals = sum of `count_value` over items in the group. Set subtotals (existing) are unchanged.
- Label = `subject` copied to every item in the group by the Label control. Colour = `appearance.color` of every item (marker colour); the set colour is only the default for new items.
- Storage: `/RLCountGroup` (private string key, UUID) on each item annotation. Items remain `/Subtype /Stamp` with their appearance stream, so foreign viewers show every item. Foreign Bluebeam `/NumCounts`, `/CountStyle`, `/CountScale` keys remain preserved verbatim by the foreign-key allowlist; they are never interpreted or written by redline.
- Limits: a count group holds at most 10,000 items (new items beyond it start a new group; on load, items past the cap in a group are treated as ungrouped); `/RLCountGroup` must parse as a UUID or it is ignored.

## 2. Polylength

| # | Behaviour | Label | Source |
|---|-----------|-------|--------|
| P1 | Polylength is its own tool: click each vertex, double-click or Enter to finish. Length stays the 2-point drag. | I | Revu has separate Length/Polylength tools (BbBylKOedOw@220-240 for Length; Polylength per support article, not video) |
| P2 | Rise/Drop: a vertical run (e.g. 2 ft) entered for a segment is added to the total length. | V | 2l8ogVsMN5k@29-62 |
| P3 | Rise and drop both ADD to length (a drop is not subtracted). | I | A cable drop and a rise are both extra cable; sign carries no length meaning |
| P4 | Split a polylength at a vertex into two separate polylengths. | V | 2l8ogVsMN5k@83-112 ("Split", warns it drops the first value; Split All segments everything) |
| P5 | Revu's "drops the first value" warning on Split: meaning unclear in the captions. | U | redline carries rise/drop values to the half their segment lands in; nothing is dropped |
| P6 | Totals are computed under the page's scale (calibration); changing the calibration recomputes them. | V (calibration flow 9boKGA1YduI@83-139) / I (recompute on recalibrate) | |
| P7 | Rise/drop entry UI. | U | Captions only say a value can be entered "within" the polylength. redline: type a rise/drop number for the segment while drawing is NOT built; set per-segment values in the Properties panel after placing |
| P8 | Unit of a rise/drop is the measurement's unit at entry. When the scale's unit later changes, known length units are converted (mm, cm, m, km, in, ft, yd); an unknown unit pair is left unconverted. | I | Our choice, see follow-ups |

### Where the commands live (redline)

- Palette tool "Polylength".
- **Properties panel**, when a length measurement is selected: a "Polylength" section: one row per segment (segment length under the scale, plus a Rise/drop numeric field), the total (path length + rise/drop), and "Split at vertex" with a vertex picker (interior vertices 1..n-2).
- Quantities panel and Markups List show the total (path + rise/drop) for the markup.

### Data model

- Type stays `MeasurementLength` with a `Polyline` geometry of 2 or more vertices; no new `MarkupType`. A 2-vertex markup is a plain Length.
- `Measurement.rise_drop: [{segment, amount, unit}]` (new, serde default empty). `segment` indexes the segment from vertex `segment` to `segment + 1`. `raw_measure` stays the scale-independent path length in PDF points; `computed_quantity = raw_measure * ratio + sum(|amount| converted to the scale unit)`.
- Split at vertex v (1 <= v <= n-2): first half keeps vertices 0..=v and rise/drops with segment < v; second half is a new markup with vertices v..n-1 and rise/drops with segment >= v re-indexed by v. Both inherit appearance, subject, layer, measurement scale_ref; each gets a fresh id and `/NM`.
- Storage (standard PDF where one exists): a polylength (3+ vertices) is written as `/Subtype /PolyLine` with `/IT /PolyLineDimension` and `/Vertices`, so any viewer draws the whole path (previously a `/Line` with only the first segment visible to foreign viewers). Two-vertex lengths stay `/Line`. The measurement payload stays in redline's private `/RLMeasure` JSON (rise/drop included); the standard `/Measure` dictionary on markup annotations is NOT written yet (follow-up). Foreign `/RiseDrop`, `/BBMeasure` keys remain preserved verbatim by the allowlist, uninterpreted (their format is U).
- Limits: 10,000 vertices per polylength; at most one rise/drop entry per segment; `amount` finite and |amount| <= 1e9; entries failing these are dropped on read. Non-finite values are rejected on write too.

## 3. Calibration recompute (previously dormant)

- `takeoff::recompute_measurement` is now used: whenever the exports run, quantities are recomputed from `raw_measure` under the document's current scales (Rust), so an export is never stale.
- In the app, a new calibration recomputes every length, perimeter, radius, area and volume markup on pages where the new scale is the effective one (page scale first, else document default), as ONE undo step. Angle and count are scale-independent and untouched.
- `geometry::area_with_cutouts` stays unwired: it needs a cutout drawing gesture and a storage shape for holes (Polygon Cutout, release 21.9). Listed as a follow-up.

## 4. Exports

- Quantities CSV/XLSX: one row per count group (not per item), Quantity = group total, Subject = group label. Polylength Quantity includes rise/drop. All text cells keep using the shared formula-neutralisation helper.
