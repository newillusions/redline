# Legends: behaviour spec

Status (as of 2026-10-12): written before the build; implemented in PR feat/legends. A Legend is the sheet-level fixture key / schedule Emittiv issues with lighting drawings (gap matrix row "Legends from tool sets", `scout-redline-bluebeam-gap.md` item 7).

Labels: **V** verified (source named), **I** inferred (reasoned, or standard table behaviour; not seen in Revu), **U** unknown (not found; the redline choice is stated and is open to correction).

Sources (raw pages fetched with curl on 2026-10-12, not summaries):
- LO = https://support.bluebeam.com/revu/features/legends-overview.html (Revu 20/21)
- LM = https://support.bluebeam.com/user-manual/menus/tools/markups-legend.html ("Markups Legend")
- `scout-revu-behaviour.md` for the Markups List / count behaviour the legend reads from.
- No tutorial video was transcribed: LM lists one ("How to create markup Legends", 2:35) but its transcript was not read, so anything only a video would show is U below.

No Bluebeam software was decompiled or instrumented. Revu's own legend file format was not captured (U), so redline does not read or regenerate Revu legends: they stay foreign annotations (see Interop).

## 1. What Revu does

| # | Behaviour | Label | Source |
|---|-----------|-------|--------|
| L1 | A legend is a table markup placed on a page: a symbol for a markup plus a description, optionally more columns (measurements, quantities, anything in the Markups List). | V | LM intro; LO "What is a legend?" |
| L2 | It updates automatically as markups are added to or removed from the source pages. Quantities of matching markups are recalculated. | V | LO; LM intro |
| L3 | Two kinds. Tool-set legend: includes every markup from a Tool Chest tool set; it can be placed before any markup exists. Ad-hoc legend: built from user-selected markups. | V | LM "Generate a Legend" |
| L4 | Ad-hoc: a new markup is included when it matches the type and appearance properties of one already in the legend. Which appearance properties are compared is not stated. | V (rule) / U (property list) | LO "Adding markups individually"; LM "Add markups to a Legend" |
| L5 | A new markup adds to the Quantity of an existing row, or makes a new row when it carries information unique within the legend's columns. Adding columns can split otherwise similar rows. | V | LM "Add markups to a Legend", intro |
| L6 | Existing markups are added to a legend from a right-click Legend menu (lists legends by Title). | V | LM |
| L7 | Title, Show Title (title bar hidden when off), Show Column Headers, font size and style for title and for columns. | V | LM "Appearance properties" |
| L8 | Columns come from the Markups List, including custom columns; chosen and ordered with Edit Columns; edits in the Markups List show in the legend. | V | LO "Display pertinent information with columns"; LM |
| L9 | Description: "determines what data is shown in the Description column". Which source field it reads by default is not stated. | V (exists) / U (default) | LM |
| L10 | Source Page(s): Current, All Pages, or Custom (a range such as `1-3, 5, 9`). | V | LM |
| L11 | Table appearance: line colour, fill colour, line and fill opacity, line width, table style; Symbol Size 1% to 5000% of default. | V | LM |
| L12 | Resize, copy, customise like any markup; save to the Tool Chest; Copy (dynamic, recalculates on the target page), Copy Legend to Snapshot (static), Apply to All Pages. | V | LO; LM |
| L13 | A legend placed on one page can source markups from other pages via Source Page(s). | V | LM "Copy a Legend to other pages" |
| L14 | Row order, sorting, hiding single rows, behaviour when a counted markup is deleted below zero rows: not stated. | U | |
| L15 | Revu's on-disk representation of a legend (annotation subtype, keys). | U | not captured, not inspected |

## 2. What redline builds

| # | redline behaviour | Basis |
|---|-------------------|-------|
| R1 | A `Legend` markup (`MarkupType::Legend`): a table drawn on a page. Geometry is a `Rect`; the left edge, right edge and top edge are the user's, the height is derived from the number of rows. | L1, L12 (resize: width + position; height follows the rows, **I**) |
| R2 | Auto-update: rows are recomputed from the live markups. In the app on every change (add, edit, delete, undo, redo, recalibration, reload); in the saved file at save time. Nothing is cached as truth. | L2 |
| R3 | **Row = one entry.** An entry is a match key: markup type, plus either a count-set id (counts of a set, any colour) or colour + fill. A markup is claimed by the first entry that matches, so a markup is never counted twice. | L4, L5 (**I**: type + colour + fill + set is redline's reading of "type and appearance"); extra-column row splitting is not built (**U**) |
| R4 | Creating: **from selection** (ad-hoc: one entry per distinct key among the selected markups) and **from count sets** (the tool-set analogue: one entry per count set, shown with 0 until its first marker; later sets are picked up automatically). Redline's Tool Chest tool sets are not used as legend sources yet. | L3, L4 |
| R5 | Adding to an existing legend: Properties panel offers "Add to legend" for the selected markups. | L6 (UI placement is our choice) |
| R6 | Scope: this page, all pages, or a page list (`1-3, 5, 9`; at most 50 tokens). "This page" is the page the legend sits on. The legend itself is never counted. | L10, L13 |
| R7 | Columns, in order, any subset: **Symbol, Subject, Description, Quantity, Unit, Pages**. The Markups List's other columns are not offered (U for demand; follow-up). | L8 |
| R8 | Subject: the row's override, else the subject of the first matching markup (so a rename in the Markups List shows), else the markup type or count-set name. Description: the row's override, else the first non-empty comment of a matching markup. Per-row override text, and Hide. | L8, L9 (default is **U**, our choice), L14 (hide is our choice) |
| R9 | Quantity, **the same numbers as the Quantities panel, the Markups List and the exports**: count = sum of `count_value` over the claimed items (equals the sum of the count-group totals of #138); length, perimeter, area, volume = sum of `computed_quantity`; every other markup type = number of markups. Unit = the first claimed markup's measurement unit (`ea` for counts without one, and for non-measurements). Counts print as integers; measurements with 2 decimals (the Markups List formatting). | L1, L2, `docs/count-polylength.md` |
| R10 | Recalibration needs no special case: the app recomputes each measurement's `computed_quantity` (#138/#139) and the legend sums that field. At save, quantities come from the same stored field the exports read. | L2 |
| R11 | Pages column: the pages the claimed markups sit on, as ranges (`1-3, 5`), clipped to 80 characters. | R7 (our column) |
| R12 | Options: title, show title, show column headers, font size (4 to 72 pt, one size for the whole table), hide empty rows, new-count-sets-are-added. Appearance: the legend markup's own `appearance` (line colour, fill colour, line weight, opacity). Table Style and Symbol Size are not built (**U**). | L7, L11 |
| R13 | Not built (follow-ups): Add to Tool Chest, dynamic Copy / Apply to All Pages / snapshot copy, Markups-List custom columns, per-row ordering, row splitting by extra columns, Revu legend reading. | L8, L12, L14 |

## 3. Data model

`Markup.legend: Option<LegendProps>` on a `Legend` markup (geometry `Rect`):

```
LegendProps {
  title: String            // default "Legend", <= 200 chars
  show_title: bool         // default true
  show_headers: bool       // default true
  scope: CurrentPage | AllPages | Pages([[from, to], ...])   // 1-based inclusive, <= 50 ranges
  columns: [Symbol | Subject | Description | Quantity | Unit | Pages]   // ordered, distinct, 1..6
  entries: [LegendEntry]   // <= 256
  auto_count_sets: bool    // unclaimed count sets become rows after the explicit entries
  hide_empty: bool
  font_size_pt: f64        // 4..72, default 9
}
LegendEntry { key: LegendKey, label: Option<String>, description: Option<String>, hidden: bool,
              symbol: Option<CountSymbol>, set_name: Option<String> }   // text <= 500 chars; set_name = the count set's name, shown while nothing matches
LegendKey   { markup_type, count_set: Option<Uuid>, color: String, fill: Option<String> }
```

- Computed rows are never part of the model that is edited. At save the Rust side computes them and uses them to draw the appearance (`LegendProps.rows`, `serde(skip)`).
- Limits: 256 entries, 50 page ranges, 500 chars per text field, 200 for the title, 6 columns; the definition JSON read from a file is refused above 512 KiB; every value is clamped or dropped on read AND on write (`LegendProps::sanitized`). Row computation is at most markups x 256 key comparisons (the first matching entry claims a mark-up); a 6,000-markup, 256-entry legend is tested inside a time budget in both implementations. The app recomputes a legend's rows when the mark-ups change, not on pan or zoom.
- Layout (identical in Rust and TypeScript, pinned by a shared fixture): row height = 1.8 x font; title bar = 2.2 x font when shown; header row = one row when shown; Symbol column = one row height wide; the other columns share the rest of the width by weights Subject 3, Description 4, Quantity 1.5, Unit 1, Pages 2; text is clipped to `floor((column width - font) / (0.52 x font))` characters with `...`. Minimum legend width 80 pt, maximum 20,000 pt. Height = title + header + rows x row height (at least one row high).

## 4. Storage and interop

- Written as a standard `/Subtype /Stamp` (the interop subtype every viewer draws from its appearance stream, as for count markers) with `/RLType /Legend`, `/RLRect` (authored rectangle) and a generated `/AP /N` form: the whole table (title bar, header, one row per visible entry with the symbol drawn, text in Helvetica written as WinAnsiEncoding bytes (octal escapes; a character outside WinAnsi becomes `?`), grid lines, fill), so Revu and Acrobat show the table. `/Contents` is not written (a stamp's contents would show as a comment note); the subject is the legend title.
- The definition is the private key `/RLLegend` (JSON string). Read side: size-capped, parsed, sanitised; a failure leaves a `Legend` mark-up with no definition: it is drawn (and saved) as an empty outlined box so it stays visible and can be deleted, while foreign viewers keep showing the appearance saved earlier until the next save.
- `/Rect` equals the appearance `/BBox` like every other redline markup (`appearance::ap_bbox`); for a legend that is the derived rectangle (top edge kept, bottom edge = top - table height).
- The appearance is a snapshot at the last save. A foreign viewer does not update it; redline refreshes it at the next save and recomputes live on open. Revu does not know `/RLLegend`, so editing the stamp there does not change the definition (U whether Revu rewrites the appearance of a stamp it moves; counts/callouts have shown it can drop non-standard keys, which is why redline keeps `/RLRect`).
- **Foreign legends (Revu)**: not recognised (no `/RLLegend`). They are ordinary foreign annotations, preserved by the unedited-foreign mechanism (#140) byte-for-byte on save; redline never regenerates them. A foreign annotation carrying a forged `/RLLegend` without `/RLType /Legend` is not interpreted either.
- Reader-owned fields (`source_ref`, `source_nm`, ...) never come from IPC/MCP; `/NM` identity, tombstone delete/undo, the stale-source guard and 3-cycle idempotence with a Revu 16-character `/NM` follow the existing store paths unchanged (a legend is an ordinary markup there).

## 5. Agreement of numbers

The legend, the Quantities panel, the Markups List and the CSV/XLSX exports must print the same totals. There is one rule in code (R9) in two implementations (the app computes live in TypeScript; the saved appearance is computed in Rust), pinned together by:

- one JSON fixture (`src-tauri/tests/fixtures/legend-agreement.json`) holding markups, a legend definition and the expected rows, executed by both a Rust test and a vitest test;
- Rust tests that compare the legend totals with `takeoff::count::group_totals` / `collapse_groups` and with the CSV export of the same markups, including after a recalibration;
- TypeScript tests that compare the legend with the Quantities-panel subtotals (`countSubtotalsByPage`) and `buildRows` of the Markups List.

One rule for a count with no recorded `count_value` (a foreign or hand-edited item): it counts once, like a placed count mark, on every surface: legend, Quantities panel, Markups List, group totals, CSV/XLSX and the MCP schedule. The Quantities panel prints measurements with 2 decimals, like the exports.

Quantities under the current scales: the app recomputes `computed_quantity` on a calibration and on open (#138/#139); the MCP `add_scale` now recomputes the stored measurements in the backend as the GUI does; and a save computes the legend table from quantities recomputed on a copy under the document's current scales (`write_markups_scaled`), the same recompute the exports run first. Page-operation rewrites (`apply_page_edit`) keep using the stored values.

Units: a legend never adds measured quantities in different units. Length, perimeter, area and volume rows are split by unit: one row per (entry, unit), so 5.0 m and 7.5 ft print as two rows. No unit conversion is attempted.

## 6. Verification still owed

- A human check of the saved legend in Revu and Acrobat (appearance only; as with G9 the strict-reader leg is owed, not assumed). PDFium rendering of the appearance is checked in a test.
- Whether Revu keeps `/RLLegend`/`/RLRect` when a user moves the stamp (U).
