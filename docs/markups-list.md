# Markups List - behaviour spec

Written before the build, from the timestamped Bluebeam Revu facts in the 2026-10-12 scout
report (`scout-revu-behaviour.md` section 2.1 / 2.2). Redline's list is built to this spec.

Labels: **V** verified (a Revu tutorial caption, `video-id@seconds`), **I** inferred (reasoned,
or standard table behaviour; not seen in Revu), **U** unknown (Revu behaviour not found; the
redline choice is stated and is open to correction). Redline status: **built** unless noted.

Nothing here comes from decompiling or instrumenting Bluebeam software.

## Panel

| Behaviour | Revu | Redline |
|---|---|---|
| A panel lists every mark-up of the document as a table, one row each | V G-i9S5o-BDw@23 | Bottom panel, "Markups" tab (default), toggled by the existing bottom-panel button |
| Clicking a row selects the mark-up and jumps the view to it | V G-i9S5o-BDw@39-45, c16Kk8Cffcc@205-211 | Single click selects and jumps (page + centre on the mark-up's bounds). Double-click selects only that row and jumps again |
| Selecting a mark-up on the page highlights its row | V G-i9S5o-BDw@39-45 | Row highlight follows `store.selectedIds`; a single canvas selection also scrolls the list to the row |
| Revu may miss a pre-selected mark-up if the list is opened after selecting | V NJUK7XIZnaU@46-61 | Not reproduced: the list reads the live selection on open |
| Multi-select rows | U | **I** Ctrl/Cmd-click toggles, Shift-click selects a range in the current view order, plain click selects one. Modifier clicks do not jump |
| Up/Down arrows move the selection | U | **I** Arrow keys move one row and jump (focus must be in the list) |

## Columns

| Behaviour | Revu | Redline |
|---|---|---|
| Columns are chosen from the list menu, each toggled by a tick | V G-i9S5o-BDw@59-77 | "Columns" menu with a checkbox per column |
| Built-in columns: Page, Comments, Author, Date, Status, Layer, Subject, Measurement, Label, Space, Sequence, ... | V G-i9S5o-BDw@45-59, c16Kk8Cffcc@41-104 | Subject, Type, Page, Comments, Author, Date, Modified, Status, Replies, Colour, Layer, Measurement, Status set by, Status set at, # . Space, Label, Sequence, Wall Area, Volume have no equivalent data in redline yet |
| Only columns with data are populated | V G-i9S5o-BDw@51-59 | Empty cells are blank |
| Status column is off by default | V IfYMI1gVsvo@27-36 | **Deviation:** on by default. This panel exists for review, and a review list without status is not useful. Revu's reason for the default is U |
| Columns reorder by dragging the header | V c16Kk8Cffcc@62-70 | Up/down buttons in the Columns menu (no drag). Order and visibility persist in localStorage; the page works if storage is unavailable |
| Column resizing | I c16Kk8Cffcc@171-176 ("size them, move them") | Not built: fixed widths. Follow-up |
| Custom columns exist | V c16Kk8Cffcc@178-182 | Measurement custom columns (`custom_columns`) appear in the Columns menu as `cost_code` etc. when the document has them. Bluebeam-private `/BSIColumnData` is opaque in redline today and is not shown. Follow-up |
| Replies count shown (speech-bubble symbol on the parent) | V c16Kk8Cffcc@233-247 | A "Replies" count column plus a disclosure triangle on rows that have replies |

## Sorting

| Behaviour | Revu | Redline |
|---|---|---|
| Click a column header to sort | V G-i9S5o-BDw@88-100 | Click a header |
| The list also "groups by that column alphabetically" | V G-i9S5o-BDw@88-100 | Not grouped: sorted only. Collapsible subject groups (V c16Kk8Cffcc@407-414) are a follow-up |
| Direction toggle / secondary sort | U | **I** Click cycles ascending, descending, back to document order. `aria-sort` set. Ties keep document order |
| Number and page columns sort numerically (page 10 after page 3) | I | Page, #, Replies and Measurement sort as numbers, others with a case-insensitive natural collator. Empty cells sort last in both directions |

## Filtering and search

| Behaviour | Revu | Redline |
|---|---|---|
| "Filter List": pick a column, then criteria | V G-i9S5o-BDw@77-88, c16Kk8Cffcc@417-434 | Filter by Status, Author, Type (multi-tick each) and by page (`1-3, 7`). Facets combine with AND; ticks within one facet are OR |
| Filter by status, e.g. rejected, on hold | V IfYMI1gVsvo@91-100 | Status facet |
| Free-text search box | V G-i9S5o-BDw@101-111 | Search matches subject, comments, author, type, layer and reply text, case-insensitive. Input capped at 200 characters |
| A page spec with no valid token | U | Matches nothing (not everything) |
| The count shows how many rows match | I | "N mark-ups" or "N of M" |

## Status

| Behaviour | Revu | Redline |
|---|---|---|
| Set via right-click > Status, or double-click the Status cell | V G-i9S5o-BDw@118-128, c16Kk8Cffcc@376-380 | Double-click the Status cell for an inline picker; a "Set status" menu acts on the selection. The list has no right-click menu (its contents are U) |
| Shift+1 .. Shift+9 shortcuts, "in the order statuses are displayed" | V IfYMI1gVsvo@42-50 | With focus in the list: Shift+1 Accepted, 2 Rejected, 3 Cancelled, 4 Completed, 5 None; 6-9 unused. The order is Revu's default model order (V c16Kk8Cffcc@184-192); that Shift+N follows that exact order is **I** |
| Status is stamped with who and when | V c16Kk8Cffcc@199-205, IfYMI1gVsvo@72-78 | "Status set by" and "Status set at" columns, from the latest markup-level Review entry of the status history |
| A status change on a reply is not the mark-up's status | I (PDF model) | A history entry with `on_reply` never drives the shown status or "Status set by/at" |
| Changing status does not create a reply row | V IfYMI1gVsvo@72-78 | It appends a history entry only (existing `patchStatus`) |
| Custom status models and per-state colours | V G-i9S5o-BDw@135-148, c16Kk8Cffcc@283-365 | Not built (redline has the fixed Review set). Follow-up |
| Locked mark-ups | U | Skipped by the store; the panel reports "N locked mark-ups skipped" |
| Undo | U | A multi-selection status change is one undo frame |

## Replies

| Behaviour | Revu | Redline |
|---|---|---|
| A reply is an indented row under its parent, text in Comments with date and time | V NJUK7XIZnaU@63-76, @196-204 | Disclosure triangle shows reply rows (indented by nesting depth) with the reply text, author and date. Reply rows are display-only: clicking one selects its parent |
| Creating and deleting replies | V NJUK7XIZnaU@12-37 | Done in the Properties panel (existing); not repeated in the list |

## Export

| Behaviour | Revu | Redline |
|---|---|---|
| Export the list from its menu | V c16Kk8Cffcc@115-150 | "Export CSV" / "Export XLSX" write the **current view**: only the rows that pass the filter, in the sorted order, with the visible columns in their order |
| Summary report with chosen fields (CSV/XML/PDF) | V G-i9S5o-BDw@148-175 | CSV and XLSX only. XML and PDF are not built |
| Formula injection | n/a | **CSV only.** Text that came from the PDF (subject, comments, author, layer, colour, status-set-by, custom columns) and every header cell (custom column names are PDF data) is prefixed with `'` when its left-trimmed value starts with `= + - @`, TAB, CR or the full-width forms; the same helper protects the fixed whole-document CSV export. XLSX cells are plain strings (never formulas) and carry no visible quote. App-formatted cells (page, replies, dates, measurement incl. a negative quantity) are untouched. Custom column names over 64 characters or with control characters are refused |
| Date column | U | Created date, UTC, `YYYY-MM-DD HH:MM` (same text as the existing fixed export). A separate Modified column is available |

The older fixed-column whole-document export (`export_markup_list`, also used by MCP) is unchanged apart from the formula-injection guard above.

## Performance and hostile data

- Rows are virtualised: fixed 24 px rows, only the visible window (plus 8 rows of overscan) is in the DOM.
  Test: 5,000 mark-ups render fewer than 60 row elements, scrolling moves the window, and
  build + filter + sort of 5,000 rows is asserted inside a time budget (`markups-list.test.ts`,
  `MarkupsListPanel.test.ts`).
- All cell text is mark-up data from PDFs and is rendered as text (no `{@html}`). Each cell is clipped
  to 2,000 characters, the search to 200, page-filter tokens to 50, custom columns to 32.
  Cyclic or dangling reply parents cannot hang the reply expansion (`threadRows` visits each reply once).

## Not built in this change (follow-ups)

Right-click menu, column resize and drag-reorder, subject grouping, custom status models and
colours, `/BSIColumnData` custom columns, XML/PDF summary, "Space/Label/Sequence/Wall Area/Volume"
columns, hiding or locking from the list, pre-selection edge case wording, and re-confirming the
Shift+N order and the Markups List right-click menu against a real Revu session (the scout could
not read two captions and the right-click menu is not in any transcript).
