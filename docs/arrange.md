# Copy / paste, Multiply, Align and Arrange: behaviour spec

Status (as of 2026-10-12): implemented in PR feat/arrange-copy-multiply (gap-matrix item 4: mark-up editing productivity the Bluebeam way).

Label legend: **V** verified (raw page fetched with curl 2026-10-12, URL given), **I** inferred (reasoned from a named basis), **U** unknown (no source found; the row says what redline does and why).

No Bluebeam software was decompiled or instrumented. Every V row below quotes a public support page.

Sources (raw HTML, `curl`, 2026-10-12):
- S1 https://support.bluebeam.com/user-manual/menus/edit/cut-copy-paste.html
- S2 https://support.bluebeam.com/user-manual/toolbars/edit-toolbar.html
- S3 https://support.bluebeam.com/revu/how-to/multiply-markups.html
- S4 https://support.bluebeam.com/user-manual/toolbars/alignment-toolbar.html
- S5 https://support.bluebeam.com/user-manual/menus/tools/align-markups.html
- S6 https://support.bluebeam.com/revu/how-to/resize-and-arrange-markups-with-alignment-tools.html
- S7 https://support.bluebeam.com/user-manual/toolbars/order-toolbar.html
- S8 https://support.bluebeam.com/user-manual/menus/tools/group.html
- S9 https://support.bluebeam.com/revu/how-to/convert-duplicate-markup.html
- S10 `~/.claude/state/returns/scout-revu-behaviour.md` (video uyZkTkKYmMo@41-49, "copy/paste counts for repeated rooms adds to the total")

## 1. Copy, Cut, Paste, Duplicate

| # | Behaviour | Label | Source / redline choice |
|---|-----------|-------|--------------------------|
| C1 | Cut, Copy, Paste of markups from the Edit menu, Edit toolbar, right-click menu and Ctrl+X / Ctrl+C / Ctrl+V. | V | S1, S2. redline: Ctrl/Cmd+X/C/V and buttons on the Arrange bar. redline has no right-click menu or Edit menu, so those two surfaces are not built. |
| C2 | Cut "cuts the selected markup and places it on the clipboard"; Copy "leaves it in place". | V | S2 |
| C3 | Ctrl+V and right-click > Paste put the markup at the cursor, its lower-left corner on the cursor. | V | S1 ("specifically, its lower-left corner is placed on the cursor"). redline: the lower-left of the whole clipboard's bounding box lands on the cursor. |
| C4 | Pasting inside Revu carries over the entire markup; pasting into another application gives raw text only. | V | S1. redline: the clipboard is in-app memory (one clipboard shared by every open document); nothing is written to the OS clipboard. OS-clipboard interchange between two redline windows: not built (follow-up). |
| C5 | A group "can be moved, edited, and copied and pasted at the same time". | V | S8. redline: a copied group pastes as a NEW group (new `group_id`), never joined to the original. |
| C6 | Copying and pasting count markups for repeated rooms adds to the total. | V | S10. redline: pasted count items form a new count (new `count_group` per source count) and the totals add. |
| C7 | Paste onto another page. | I | The cursor position is the target, so the page under the cursor is the target (S1). redline: the page being viewed; coordinates are PDF user space, no scaling between pages of different size. |
| C8 | Paste into another open document. | I | Same clipboard, target is the active tab. Count sets travel with the count items; measurements are recomputed under the TARGET page's scale. |
| C9 | Paste in Place. | U | No Revu page found in S1/S2 or the edit-menu pages. redline: Ctrl/Cmd+Shift+V pastes at the SAME coordinates on the viewed page. "Paste to pages..." pastes in place onto a typed list of pages (our choice, requested by the owner brief). |
| C10 | Cursor not over the page (keyboard paste with the pointer elsewhere, or the Paste button). | U | redline: the Paste button centres the clipboard in the view; Ctrl+V with no known cursor position pastes in place offset by 12 pt per repeat so a copy never hides exactly under its source. |
| C11 | Duplicate. | U | Revu's "Duplicate as" is a Bluebeam Max feature that makes a copy of a DIFFERENT markup type (S9); a same-type Duplicate command is not documented. redline: Ctrl/Cmd+D copies the selection 12 pt right and 12 pt down on the same page, leaving the clipboard untouched. Convert/Duplicate-as (Max) is not built. |
| C12 | Copies are independent, new redline-owned markups. | I | Owner brief. Each copy gets a new id (the `/NM` is derived from it on save), a fresh audit (created by the pasting user now, revision 0), and none of: `source_nm`, `source_rect`, foreign keys (`/BSI*` and friends), rich text, raw `/DA`, optional-content reference. The backend also strips reader-owned fields from every `add_markup` (`Markup::clear_reader_owned`), which stays the last line of defence. |
| C13 | Replies and review status are copied. | U | S1/S2 are silent. redline: NOT copied; the copy starts with status None, no assignee, no replies and no history. A reply thread belongs to the conversation about the original. |
| C14 | A copy of a locked markup is locked. | U | redline: the Locked and LockedContents `/F` bits are cleared on the copy so the pasted markup can be moved. |
| C15 | Cut of a locked markup. | U | redline: locked markups are neither cut nor copied by Cut (they stay put); the message names how many were skipped. Copy copies them. |
| C16 | Subject, comment text, layer, appearance, measurement custom columns. | I | Properties of the markup travel with it (S8 "behave like a single markup"). **Layer:** a copy within the same document keeps the layer name AND its optional-content (`/OC`) reference, so it hides with the source's layer; a paste into another document drops both (the reference points into the source file and the target may have no such layer). Mapping by layer name across documents is not done. A paste into a document that has a markup layer set joins that layer like any new mark-up (`onNewMarkup`). Tested against the real Layers code (`src/lib/arrange-layers.test.ts`). |
| C17 | Limits. | I | At most 10,000 markups per paste (all pages together) and per Multiply; a paste that would exceed it is refused whole. Hostile or damaged clipboard items (non-finite or out-of-range numbers including line weight, opacity and font size, unknown type or geometry, absurd vertex counts) are skipped and counted, never panic. |
| C19 | A copy of a mark-up Revu or Acrobat authored. | I | The copy is rebuilt from redline's model on save (it has no original dictionary to write back, unlike an unedited foreign mark-up), so exotic appearance details of the original may be simplified. Reader-owned and foreign data is never carried (C12). |
| C18 | Undo. | I | One undo step per operation (cut = copy + delete is one step, a paste to 20 pages is one step). |

## 2. Multiply

| # | Behaviour | Label | Source / redline choice |
|---|-----------|-------|--------------------------|
| M1 | Right-click > Multiply makes same-size parallel copies of markups and measurements. | V | S3. redline: the Multiply... button / dialog on the Arrange bar. |
| M2 | Only the four cardinal directions (up, down, left, right). | V | S3. redline: those four are the Direction choices; "Grid" (columns x rows) is an extension beyond Revu. |
| M3 | The number of copies is limited to what fits on the sheet. | V | S3. redline: copies whose bounding box would leave the page are not made; the message says how many fit. |
| M4 | Dialog asks direction, distance between copies with a unit, number of copies, and a Rotation checkbox. | V | S3. redline: direction, spacing with unit, number. Rotation checkbox: not applicable, redline markups have no rotation property. |
| M5 | "Distance between the copies" is measured centre to centre, or edge to edge. | V | Owner-confirmed from Revu 2026-10-12: the default is **centre to centre**; the Multiply dialog has a switch, "Centre to centre" or "Gap" (edge-to-edge clear space), remembered per viewer (`redline.multiply.measure`). Centre to centre: copies step by exactly the distance, whatever their size (the same as the old point-to-point pitch). Gap: the step is the selection's width (right/left, and Across in a grid) or height (up/down, and Down in a grid) plus the distance; a gap of 0 makes the edges touch. |
| M6 | An uncalibrated page prompts to set the page scale. | V | S3. redline: no prompt. The unit list is "pt" always plus the page scale's unit when the page has a scale, converted with that scale. |
| M7 | Multiplied markups are independent, not grouped. | V | S3. redline: each copy of a grouped selection gets its own new group; ungrouped selections stay ungrouped. |
| M8 | Multiplied counts. | I | Counts are the point of the feature ("luminaire symbols and counts"). A copy of a count item joins the SOURCE's count (`count_group`), so a row of luminaires is one count with the right total. An ungrouped source is stamped with its own id as `count_group` in the same undo step (as Resume does), so the frontend, the Rust exports and the Markups List all see one count of N+1; a locked ungrouped source cannot be stamped, so the whole Multiply is refused with a message. A count that would pass 10,000 items starts a new count. |
| M9 | Multiplied measurements. | I | Same page, same scale: quantities are copied unchanged. |
| M10 | Multiply a multi-markup selection. | I | The selection's bounding box is the repeat unit: every copy of the set is offset by the same pitch. |

## 3. Align, Distribute, Arrange (order), Group

| # | Behaviour | Label | Source / redline choice |
|---|-----------|-------|--------------------------|
| A1 | Align Left / Top / Right / Bottom / Center / Middle align two or more markups to the primary. | V | S4, S5, S6 |
| A2 | The primary markup is the first one selected; the others move to it. A different primary can be chosen by clicking a markup with green control points. | V | S4 ("first markup selected"), S5, S6. redline: primary = first selected. Choosing another primary after the fact: not built (follow-up). |
| A3 | Which axis "Center" and "Middle" fix. | I | The sources contradict themselves in wording (S4: Center = "vertical centers"; S6: Center = "horizontal center ... to the vertical center"). redline uses the usual meaning: **Center** gives every markup the same horizontal centre (x), **Middle** the same vertical middle (y). Owner-confirmed from Revu 2026-10-12. Markups are held in the page's display space (rotation already applied), so on 90/270 pages Center and Middle follow the screen's x and y too. |
| A4 | Distribute Horizontally / Vertically needs three or more markups and makes "the spaces between their centers" equal. | V | S4, S5, S6. redline: the first and last (by position) stay, the ones between get evenly spaced centres. |
| A5 | Order tools: Bring to Front, Send to Back, Bring Forward, Send Backward; they act on one or more markups and concern "the relative position of overlapping markups". | V | S7 |
| A6 | One "level" for Bring Forward / Send Backward. | U | S7 says "up one level" with two clicks shown. redline: the markup passes the next markup in that direction on the same page that overlaps it (so a click always changes what you see). Several selected markups keep their relative order. |
| A7 | Group / Ungroup of selected markups (Ctrl+G). | V | S8. Already built (Ctrl/Cmd+G, Ctrl/Cmd+Shift+G); unchanged. |
| A8 | Groups align, distribute and re-order as one unit. | I | S8 "behave like a single markup". |
| A9 | Locked markups. | U | redline: a locked markup is not moved or re-ordered (a lock guards the markup's content, and the existing move/delete paths skip locked ones the same way); it still counts as the primary or as a distribute end point. The message names how many were skipped. |
| A10 | Align Width / Height / Size, Center in Document, Flip. | V | S4/S5/S6 list them. NOT built here (outside this item); follow-up. |
| A11 | Keyboard shortcuts for the alignment tools. | U | S5 points to a shortcut list on S4 that the page does not contain (only Flip, Ctrl+Alt+H/V, is stated). redline: buttons only. |
| A12 | Z-order is the document order of the markups on a page (and the order they are written to `/Annots`). | I | Verified against `write_markups_scaled` (markups are appended to each page's `/Annots` in store order). A new backend command `reorder_markups` reorders the store; one undo step per operation. A Legend's implicit rows and "first of the group" display fall out of document order, so re-ordering can change which markup's colour or comment a legend/count row shows first; the totals are unaffected. |

## 4. Where it lives

- **Pure logic:** `src/lib/markup-copy.ts` (clipboard, copies, validation), `src/lib/markup-arrange.ts` (align, distribute, order, multiply geometry), `src/lib/arrange-actions.ts` (store-facing commands).
- **UI:** `src/components/ArrangeBar.svelte` (floating bar at the top of the page view, Select tool), `src/components/MultiplyDialog.svelte`, `src/components/PasteToPagesDialog.svelte`.
- **Keys:** Viewport handles Ctrl/Cmd+C (markups when there is no text selection), X, V, Shift+V, D. They do nothing while a text field has focus.
- **Backend:** `reorder_markups` (Tauri command) -> `MarkupStore::reorder`.

## 5. Follow-ups (named, not built)

- OS-clipboard interchange (paste markups into a second redline window).
- Choose another primary markup; Align Width/Height/Size, Center in Document, Flip.
- Format Painter (listed beside copy/paste in the gap matrix).
- Right-click context menu and Edit menu entries.
- Convert to / Duplicate as another markup type, Offset (Bluebeam Max).
