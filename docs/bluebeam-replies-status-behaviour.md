# Replies, review status and history: behaviour spec (as of 2026-10-12)

Goal: a reply or status set in redline appears in Bluebeam Revu and Acrobat, and a PDF
reviewed in Revu or Acrobat shows its replies and statuses in redline. Written before the
code, from the viewers' documented behaviour; the code and tests implement exactly this.

Labels: **verified** = read from a raw source this session; **inferred** = from the PDF
spec or prior-art output, not re-checked against a fresh Revu file; **unknown** = not
checked. No Bluebeam software was run, decompiled or instrumented (Bluebeam terms s1.4);
only documentation and PDFs were studied.

## 1. How it behaves in Revu (verified, support.bluebeam.com raw pages, 2026-10-12)

Source: `user-manual/menus/window/markups-list.html`,
`user-manual/tutorials/custom-status-markups-list.html`.

- **Reply.** Markups list > Reply: "Replies to the selected markup. Replies appear on a new
  indented row below the markup in question." Replies are also shown in the Comments
  column; double-click edits the text. Delete on a reply removes the selected reply.
- **Status.** Set from the Status column header menu or right-click > Set Status. Default
  statuses: Accepted, Rejected, Completed, Cancelled, None. Custom states can be created.
  Revu ships two models, Review and Migration, that cannot be altered; custom statuses need
  a custom model first.
- **Locked markups.** A locked markup cannot be moved or changed, but "changing the Status
  or entering a Reply" is still allowed.
- **Spelling.** Revu's docs write "Cancelled". Some tools write "Canceled"; both are read.

## 2. How it is stored (ISO 32000-1 §12.5.6.2-4, inferred; prior art verified by scout)

Prior art read for the layout: `bluebeam-set-status` (MIT; statuses are hidden `/Text`
replies with `/IRT`, `/StateModel`, `/State`, `/F 30`) and pymkup (MIT; markups are plain
annotation dictionaries plus Bluebeam keys). Scout report:
`~/.claude/state/returns/scout-bluebeam-interop.md`.

| Concept | PDF |
|---|---|
| Reply | `/Subtype /Text`, `/IRT` -> parent, `/RT /R`, `/Contents`, `/T` author, `/M` + `/CreationDate`, `/NM` UUID |
| Reply to a reply | same, `/IRT` -> the other reply |
| Status change | a hidden `/Text` reply with `/StateModel (Review)` and `/State (Accepted\|Rejected\|Cancelled\|Completed\|None)` |
| Other state models | same shape, other `/StateModel` (`Marked`/`Unmarked`, Revu's `Migration`, custom). Kept in history, never drive the status |
| Status history | every state annotation, in `/CreationDate` order. Current status = latest Review entry |
| Assignee | no standard form and none found in Revu docs: stays in private `/RLWorkflowExtra` (JSON) |

Written flags: reply `/F 28` (Print, NoZoom, NoRotate; inferred from Acrobat output), state
`/F 30` (adds Hidden; from the prior-art tool). Every reply and state annotation has an
empty `/AP /N` form so no viewer that paints annotations draws a note icon for it
(inferred: Acrobat and Revu show replies in their comment lists, not as page icons).
Text is ASCII, or UTF-16BE with BOM when it contains anything else; the reader also
accepts UTF-8 BOM and plain bytes. Dates are written `D:YYYYMMDDHHmmSSZ`; the reader
honours `+HH'mm'` / `-HH'mm'` offsets, which Revu and Acrobat write.

## 3. Redline behaviour

**Read** (`document::annots::read_markups` + `markup::replies::attach`):
1. A `/Text` annotation with `/IRT` and `/RT` not `/Group` is a reply or state, never a
   markup. (`/RT /Group` stays the grouping link.)
2. It attaches to the markup it transitively answers. A reply to a reply becomes a nested
   reply (`in_reply_to`). A reply to something that is not a markup (a Link, a foreign
   sticky note) is left alone in the PDF and not imported.
3. `workflow.status` = latest Review-model state (`Canceled` accepted). With no state
   annotations the private `/RLStatus` is used, so files from older redline open as before.
4. Legacy `/RLWorkflowExtra` threads are merged in (deduplicated by reply id).
5. Thread and history are sorted oldest first.

**Write** (`write_markups`):
1. Replies and states answering a managed markup are owned by redline: removed and
   re-emitted from the model with `/IRT` pointing at the markup's fresh object, so no
   `/IRT` ever dangles. Replies to unmanaged annotations are untouched.
2. One state annotation per history entry. If `workflow.status` differs from the history's
   latest Review entry (a status set by the MCP tool, or only in `/RLStatus`), one entry is
   synthesised with an id derived from (markup, state, modified time) so repeated saves
   write the same `/NM` and the file does not grow.
3. Unknown Bluebeam keys on a markup (`/BSI*` such as `/BSIColumnData`, `/BBMeasure`,
   `/GroupNesting`, `/NumCounts`, `/CountStyle`, `/CountScale`, `/PitchRun`, `/RiseDrop`,
   `/SlopeType`) are carried verbatim. Page-level `/BSISpaces` is never touched (only
   `/Annots` is rewritten). A value containing an indirect reference is not carried (its
   object id would dangle); none of the known keys use one in the files studied.

**UI** (Properties panel, single selection): status picker (None, Accepted, Rejected,
Completed, Cancelled) with "Set by X, date"; reply thread with nesting, Reply (Ctrl+Enter),
reply-to-reply, Delete (removes the reply and its nested replies); status history list,
newest first, with non-Review models labelled. A status change appends a history entry;
setting the status it already has records nothing. Undo/redo reuse the existing store.

**Migration decision.** Files saved by redline with replies in `/RLWorkflowExtra` still
open with their threads, and the next save converts them to standard replies and stops
writing the thread into the private key (the assignee stays; the key keeps its
`(assignee, thread)` shape with an empty thread so an older redline still parses it).
A legacy `/RLStatus` with no history is converted to one synthesised state annotation.
Tested: `legacy_rlworkflowextra_thread_still_opens_and_migrates_to_standard_replies`.
Cost (inferred from the previous reader, not run): an older redline opening a file saved by
this version does not recognise the reply notes and would import each as a separate Text
markup.

## 3b. Revu interaction facts folded in (scout-revu-behaviour, tutorial transcripts, 2026-10-12)

Reported by the scout from timestamped Revu tutorials (not re-checked by me):
- Reply: right-click the markup (canvas or list) > Reply; the parent shows a speech-bubble
  symbol once it has a reply. Redline's read model exposes `replyCount` for that indicator.
- Status: right-click > Status or double-click the Status cell; built-in shortcuts
  Shift+1..Shift+9 in the order the statuses are displayed; the Status column is off by
  default. Each change is stamped with who and when (matches the history here); a change does
  not create a reply row. Redline's enum now includes Cancelled.
- Not built here (Markups List scope): Shift+N shortcuts, status colours, custom models via
  Manage Status, the Status column, filtering by status. `reviewSummary()` carries what those
  need (status, who/when, reply count, nested rows). A reply can carry its own status in Revu
  (inferred from PDF structure by the scout, unverified); redline attaches every state
  annotation to the root markup, so a state on a reply is kept in that markup's history.
- Unknown (scout): how Revu displays status history in its UI.

## 4. Read model for the Markups List panel

Rust: `Workflow::reply_count()`, `Workflow::latest_review_change()`. MCP `list_markups` /
`search_markups` summaries add `reply_count`, `status_changed_by`, `status_changed_at`.
TypeScript: `src/lib/markup-review.ts` `reviewSummary(markup)` returns
`{ replyCount, status, statusBy, statusAt, lastReply, thread[] with depth }`. A list row
needs only that: Status column, Replies count, status author and date, nested rows.

## 5. Fixture

`src-tauri/tests/fixtures/reviewed-replies-status.pdf`, built by plain lopdf in
`markup::replies::tests::build_reviewed_fixture` in the layout Acrobat/Revu write: a Square
with two replies (one nested), Review history Accepted -> Completed plus a `Marked` entry;
a Circle carrying `/BSIColumnData`, `/BBMeasure`, `/NumCounts` with a `Canceled` state;
page `/BSISpaces`; a Link with a reply. Regenerate:
`cargo test --lib regenerate_reviewed_fixture -- --ignored`. Note it is hand-built from the
spec and prior art, not exported from Revu: it proves redline reads that layout, not that
Revu writes it. Replace or add a real Revu export when one is available.

## 6. Verification status

- Automated (CI): round-trips, history, nesting, migration, UTF-16BE, dangling-`/IRT`,
  repeat-save stability, private-key preservation, foreign-reply isolation, and the
  frontend helpers and panel.
- Independent readers (run during development, 2026-10-12): `qpdf --check` clean on a
  redline re-save of the fixture, and MuPDF (PyMuPDF 1.28.2) lists every reply and state
  annotation with its content and author.
- **Not done: Revu and Acrobat themselves.** The cross-viewer harness
  (`tools/crossviewer/win`) drives Windows viewers by screen capture on the owner's
  workstation and may only run when the owner declares it idle; the Bluebeam leg is
  currently a licence probe, not a renderer. Owner check, 2 minutes in Revu:
  1. Open `reviewed-replies-status.pdf` in redline: select the red square; expect two
     replies (one indented), status Completed "Set by Martin", history of three entries.
  2. Add a reply "Check in Revu" and set status Accepted; save.
  3. Open the saved file in Revu: Markups list should show the square with its replies on
     indented rows and Status Accepted; right-click > Set Status > Rejected, save.
  4. Reopen in redline: status Rejected, history gained the Revu entry, replies intact.
  5. Repeat steps 3-4 in Acrobat (Comments pane shows the replies; status via the
     markup's context menu).
  Report anything that differs (especially a note icon drawn on the page, a missing
  status, or a reply shown unthreaded).

## 6a. Identity of foreign annotations (round 1 review fixes)

- **Revu-style `/NM`.** Revu writes 16-character `/NM` names, not UUIDs. Such an annotation
  gets a stable id derived from its `/NM` (`derive_uuid("nm:"+NM)`) and keeps the original in
  `source_nm`; save writes the original `/NM` back unchanged (Revu matches markups by `/NM`)
  and treats the annotation as managed, so load+save is idempotent. Before this, every
  save of a Revu-authored PDF kept the original and wrote a copy next to it (markups, and with
  this feature their replies and states too). Two annotations sharing one `/NM` get distinct
  ids (`...:dupN`) and still write their shared `/NM`. Same for reply and state `/NM`
  (`Reply.nm`, `StatusChange.nm`). Tests: `revu_style_nm_load_save_is_idempotent_and_keeps_every_nm`
  (3 cycles), `two_annotations_sharing_one_nm_stay_two_markups_over_saves`,
  `uppercase_uuid_nm_is_not_duplicated_either`.
- **State on a reply.** A state whose `/IRT` is a reply is kept in the markup's history with
  `on_reply`, never drives the markup's status, and is written back with `/IRT` to the same
  reply. Test: `a_state_on_a_reply_stays_on_that_reply_and_does_not_drive_the_markup_status`.
- **Missing dates stay missing.** A reply or state with no `/M` / `/CreationDate` reads as the
  fixed `UNDATED` instant (Unix epoch) and is written back with no date. Test:
  `undated_replies_stay_undated_and_never_get_now`.
- **Name-typed `/StateModel` / `/State`** are accepted; reals above f32 range are refused.

- **Deleting a reply that carries a state.** The state is dropped (never re-anchored on the
  markup, where it would become the markup's status); the frontend prunes it with the reply.
  Tests: `deleting_a_reply_that_carries_a_state_drops_the_orphan_state_instead_of_reanchoring_it`,
  `a_state_on_a_state_is_written_under_that_state_and_the_walk_is_linear`.
- **Reader-owned names.** `source_nm`, `Reply.nm` and `StatusChange.nm` come only from the PDF
  reader. `MarkupStore::add` clears them and `update` keeps the stored markup's values by id,
  so a crafted value equal to a foreign annotation's `/NM` (GUI IPC or MCP) cannot make a save
  delete that annotation. Tests: `a_json_supplied_source_nm_cannot_make_a_save_delete_a_foreign_annotation`,
  `an_update_keeps_the_stored_names_and_ignores_inbound_ones`.
- **Undo of a delete (tombstone).** Undo re-sends the deleted markup as an add. `MarkupStore::delete`
  keeps a per-document tombstone of a deleted markup that carried reader-owned names, and
  `add` adopts names only from the tombstone entry for the same id (then drops the entry);
  inbound values are always ignored, and a crafted add whose id equals `derive_uuid("nm:X")`
  gets nothing without a tombstone. Cleared on close and reload. Tests:
  `delete_then_undo_of_a_revu_markup_does_not_duplicate_it_or_its_replies`,
  `a_crafted_add_cannot_claim_a_foreign_nm_even_with_the_derived_id`,
  `a_tombstone_only_restores_what_the_store_held_not_what_the_add_claims`,
  `delete_without_undo_still_removes_the_redline_copy_on_save`,
  `closing_or_reloading_the_document_clears_its_tombstones`.

## 6b. Untrusted input and security (added after review findings)

The PDF and the `Markup` JSON (frontend, MCP) are untrusted:
- **No panics on malformed `/IRT`, `/State`, dates.** Missing or wrong-typed `/IRT`, dangling,
  self-referencing and cyclic chains, bad or oversized `/State`, and hostile dates are skipped
  with one `log::warn!` per read (or parsed to `None`), never indexed unchecked. Chains of any
  length resolve iteratively in linear time with a memo (no depth cap, no recursion), and
  the write-side ownership walk is a linear worklist, so a 30,000-reply chain is handled.
- **No PDF syntax is concatenated.** Every name and string goes through lopdf objects; the
  writer escapes them. Hostile reply text such as `)>> /JS (...)` is stored and read back as
  data (tested through a real save and reload).
- **Foreign keys are allowlisted at both boundaries.** Only the keys in
  `markup::foreign::ALLOWED`, with the expected shape (`/BSIColumnData` = array of strings,
  `/NumCounts` = number, the rest bounded plain data) are read or written; others are dropped
  with a log line. The earlier "any `/BSI*`" prefix rule is gone. Action/script keys (`/A`,
  `/AA`, `/JS`, `/OpenAction`, `/URI`, `/Launch`, ...) are refused at top level and inside
  allowed values; names and nested keys must be `[A-Za-z0-9_.+-]` (max 64), depth max 8, max
  4096 nodes, strings max 64 KiB. Page `/BSISpaces` is not an annotation key and is not carried.

## 6c. Data integrity of a reviewed PDF (as of 2026-10-12)

- **Deleting a foreign markup.** `MarkupStore::delete` records the deleted markup's id and
  original `/NM` in `removed_names`; the save paths (`save_with_markups_removing`,
  `apply_edit_and_save`, `extract_pages_to_new_file`) pass them to `write_markups_removing`,
  which treats those names as redline-owned for that save. The original, its replies and its
  states are stripped (replies are owned through `/IRT`) and, being absent from the model, not
  re-emitted. Undo (`add` of the same id) takes the names back out. The set holds only names of
  markups the store held, never input. Cleared on reload. Tests:
  `deleting_a_revu_markup_removes_the_original_its_replies_and_states_on_save`,
  `undoing_the_delete_restores_the_markup_once_with_its_original_names`,
  `deleting_an_acrobat_uuid_nm_markup_removes_it_and_its_reply_too`,
  `removed_names_only_ever_come_from_markups_the_store_held`.
- **Reply and state keys.** `Reply.extras` / `StatusChange.extras` (`SourceExtras`) carry
  `/Subj`, `/C`, `/RC`, the Bluebeam private keys of `foreign::ALLOWED` (allowlist
  `foreign::Scope::Reply` = `ALLOWED` + `REPLY_EXTRA`) and the pop-up (`/Rect`, `/Open`, `/F`).
  Same trust model as section 6b: read-time capture and write-time re-validation, reader-owned
  (cleared on add, kept by id on update, restored by the tombstone). **`/RC` rule: it is
  written back only while the reply's `/Contents` still equals what was read; after an edit the
  stale `/RC` is dropped (not regenerated), so a viewer shows the new `/Contents`.** The pop-up
  is a fresh `/Popup` annotation parented to the rewritten reply; the old one is dropped with the
  old reply (it used to be kept, dangling). Tests:
  `an_unedited_reply_keeps_subj_c_rc_popup_and_bluebeam_keys_over_three_cycles`,
  `an_edited_reply_keeps_subj_c_and_private_keys_but_drops_the_stale_rich_text`,
  `hostile_reply_keys_are_dropped_on_read_and_refused_on_write`.
- **Duplicate reply `/NM`.** Entries sharing an id get new ids derived from the `/NM`, kind,
  author, time and a rank among identical entries, so the ids do not depend on file order.
  Each keeps its source `/NM`, so the file is written back with the duplicates it had and a
  reload derives the same ids. Tests: `replies_and_states_sharing_one_nm_keep_distinct_stable_identities`,
  `thousands_of_replies_sharing_one_nm_resolve_in_bounded_time_without_loss`.
- **Foreign sticky note `/Rect`.** `Markup.source_rect` keeps a note's `/Rect` when it is not the
  standard 20 x 20 icon box; it is written back while the note is still centred on it (moved
  notes get the normal box). A box smaller than the icon shrinks the icon to fit. Reader-owned,
  finite and at most 1000 pt a side. Tests: `an_unmoved_foreign_sticky_note_keeps_its_rect_over_three_cycles`
  (also on rotated pages), `a_moved_foreign_sticky_note_gets_a_normal_icon_box_at_its_new_place`,
  `a_small_foreign_note_box_is_kept_and_the_icon_shrinks_to_fit_it`, `a_hostile_source_rect_is_ignored`.
- **`/QuadPoints` order.** The model stores TL, TR, BL, BR. A foreign quad that is a strictly
  convex polygon in index order (ISO counter-clockwise from any corner, or clockwise) is
  re-ordered by its bottom edge, found geometrically (lowest midpoint y) in the displayed frame
  (after the page-rotation transform); the Acrobat / Revu bow-tie order and degenerate quads are
  kept as they are. Redline always writes the Acrobat / Revu order. Tests:
  `acrobat_z_order_is_left_alone_and_polygon_orders_are_reordered_from_any_corner`,
  `degenerate_and_hostile_quads_are_returned_unchanged_and_never_panic`,
  `text_markups_in_either_quadpoints_order_load_and_save_to_the_same_geometry`,
  `quadpoints_order_is_resolved_in_the_displayed_frame_on_rotated_pages`.
  Not covered: a `.btx` import reads `/QuadPoints` without this step.

## 7. Known limits

- A deleted foreign markup is removed on save (section 6c), but only the annotations redline
  loaded: a foreign annotation of an unsupported subtype is never touched.
- Reply keys beyond the allowlist of section 6c (`/CA`, `/ExData`, other private keys) are not
  carried. The pop-up is rebuilt with `/Rect`, `/Open` and `/F` only.
- Contents of non-reply markups is still written as raw UTF-8 (pre-existing); only replies
  and states use UTF-16BE for non-ASCII.
- Revu's Migration/custom model state names are unknown; they are preserved but not
  interpreted. Custom Review-model-like statuses outside the five ISO states are not
  surfaced in the picker.
- The picker acts on one markup's own thread; batch status over a multi-selection still
  works (history entry per markup), the thread panel is single-selection only.
