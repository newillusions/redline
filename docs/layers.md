# Layers (PDF optional content) - behaviour spec

Written before the build (as of 2026-10-12), then updated with what the tests measured.
Redline's Layers panel is built to this spec.

Labels: **V** verified (a raw source page or a test in this repo, named), **I** inferred
(reasoned, or standard behaviour; not seen in Revu), **U** unknown (the redline choice is
stated and is open to correction). Redline status: **built** unless noted.

Nothing here comes from decompiling or instrumenting Bluebeam software.

Sources (raw pages, fetched 2026-10-12; not summaries):

- `BB-panel` https://support.bluebeam.com/user-manual/menus/window/layers-panel.html
- `BB-vis` https://support.bluebeam.com/user-manual/menus/window/layer-visibility.html
- `BB-add` https://support.bluebeam.com/user-manual/menus/window/add-layers-pdf.html
- `BB-del` https://support.bluebeam.com/user-manual/menus/window/delete-layers.html
- `BB-pdf` https://support.bluebeam.com/user-manual/menus/window/pdf-layers.html
- `BB-how` https://support.bluebeam.com/revu/how-to/use-layers-to-organize-information.html
- `ISO` ISO 32000-1:2008 (PDF 1.7), section 8.11, from
  https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf
  (8.11.2.2 membership dictionaries, 8.11.3.3 `/OC` on annotations, 8.11.4.2 `/OCProperties`,
  8.11.4.3 configuration dictionaries, 8.11.4.4 usage, 8.11.4.5 state determination).

## 0. What the code did before (verified against origin/main 929fea63)

| Fact | Evidence |
|---|---|
| A mark-up's `layer` was a private string, `/RLLayer`, with no tie to any group in the file; a free-text box in Properties edited it. | `markup/annotation.rs` (`RLLayer`), `PropertiesPanel.svelte` (text input `data-field=layer`) |
| `/OC` was carried verbatim and never interpreted (`optional_content`: a reference, or a plain string from a `.btx`). | `markup/mod.rs` `OptionalContent`, `annotation.rs` |
| The save of an unedited foreign annotation already dropped a dangling `/OC` (#140); a mark-up rebuilt from the model did not. | `annots.rs` `preserved_original` (before this change) |
| No panel, no visibility, no `/OCProperties` read or write (grep `OCProperties`: none). | gap matrix 2.11, `App.svelte` comment "Bookmarks/Layers remain unbuilt" |
| Tiles are drawn with `render_annotations(false)`: PDFium paints page content only; mark-ups are an SVG overlay. | `render/mod.rs` `render_tile` |
| pdfium-render 0.8.37 has no optional-content API. | grep of the crate source: no `OCG`/`optional content` outside docs |

So the existing `layer` and `/OC` were two unconnected things. They are now one: the
assignment is `/OC`; `layer` carries the group's name for lists and exports.

## 1. The panel

| Behaviour | Source | Redline |
|---|---|---|
| A Layers panel lists the layers of a layered PDF; child layers sit under a parent with an expand/collapse triangle | V BB-panel ("toggle triangle ... expand or collapse the list of child layers") | "Layers" accordion in the left panel. Rows follow `/Order`; a layer's sub-layers are indented under it with a triangle |
| Show/hide: a box left of the name; eye = visible, empty = hidden | V BB-panel | A checkbox per layer (accessible name "Show layer NAME") |
| Right-click a parent > Show Layer toggles the parent and all children | V BB-panel | Shift-click on the checkbox toggles the layer and its sub-layers; a plain click toggles one layer. No right-click menu (U for its contents) |
| Isolate: show only the selected layer and its children, hide all others | V BB-panel, BB-vis | "Show only" (button on the selected row; double-click on a name does the same) |
| Show All Layers; Reset Layers to the selected configuration | V BB-panel | "Show all" and "Reset" (back to the file's own default configuration) |
| Show Print Layers / Show Export Layers / Show Layers on Page Only / alphabetical sort | V BB-panel | Not built (follow-up) |
| Drag a layer to reorder or re-parent it | V BB-panel | Not built (follow-up). `/Order` is only appended to |
| Layer Properties (title, default state, locked, view/print/export/zoom) | V BB-panel | Not built. `Locked` is honoured read-only (below) |
| Locked: "the title and default state cannot be changed from the layers list" | V BB-panel; V ISO 8.11.4.3 `/Locked` ("cannot be changed through the user interface") | A locked layer's checkbox is disabled and badged "locked"; "Show all" and "Show only" leave locked layers as they are |
| Layers inside `/Order` with a text label are collection headings, not layers (no toggle) | V ISO 8.11.4.3 (Order); V BB-panel ("New Layer Group ... do not have a toggle box") | Rendered as a bold heading row with no checkbox |
| An unlabelled nested array lists the sub-layers of the group before it | V ISO 8.11.4.3 Example 2 | Same |
| Groups missing from `/Order` are not presented | V ISO 8.11.4.3 ("Any groups not listed in this array shall not be presented") | **Deviation**: they are listed at the end, with a note ("N layer(s) the file does not list in its layer order"). A producer that omits `/Order` entirely would otherwise show an empty panel. Whether Revu/Acrobat list them is U |
| Radio-button groups: turning one on turns the others off | V ISO 8.11.4.3 (RBGroups) | Honoured on turn-on |
| A layer whose `/Intent` excludes `View` does not affect what is shown | V ISO 8.11.2.3; V test `pdfium_design_intent_groups_do_not_hide` | Listed with a "design" badge and a disabled checkbox |
| Alternate configurations (`/Configs`) and a configuration list | V BB-vis, BB-panel | Not built (follow-up). Only the default configuration (`/D`) is read and written |

## 2. The view is not the file

| Behaviour | Source | Redline |
|---|---|---|
| A layered PDF opens in its Default configuration | V BB-vis; V ISO 8.11.4.5 | The initial view is the file's own `/D` (plus the group's own `ViewState`, section 4) |
| Toggling is a view change; saving a configuration is a separate act ("Save Configuration" on Default) | V BB-vis | Show/hide **never writes the file**. "Save as default" writes the current state into `/D` (`BaseState` removed, `/ON` removed, `/OFF` = hidden groups). It is disabled while the view equals the file's default. Like a page operation it saves the file (with the current mark-ups) and reloads the renderer |
| The default configuration cannot be deleted | V BB-vis | Not applicable (no deletion) |

Idempotence (a rule from the brief): opening and saving without editing layers leaves
`/OCProperties` and every group object unchanged. V tests
`a_markup_on_a_layer_keeps_its_layer_over_three_save_cycles`,
`an_unedited_foreign_markup_keeps_its_oc_and_gets_its_layer_name`,
`adding_a_layer_and_saving_the_default_keep_markups_on_their_layers` (3 cycles each, data
compared, not just counts).

## 3. Creating layers

| Behaviour | Source | Redline |
|---|---|---|
| Add New Layer: Add Before / Add After / Add Child, then a name | V BB-panel, BB-add | "New layer" (appended at the end of the list) and "Add child" (under the selected layer). Before/After not built (follow-up) |
| A new layer is an optional content group | I (PDF layers are OCGs; BB-pdf calls them PDF layers) | `<< /Type /OCG /Name (text) >>`, added to `/OCProperties /OCGs` and `/D /Order`. Created ON; with `BaseState /OFF` it is added to `/ON` |
| `/OCProperties` is created when the file has none | I | Yes |
| An absent or empty `/Order` presents nothing in a strict reader | V ISO 8.11.4.3 | Adding the first layer to a file whose groups were not listed first writes `/Order` with every existing group, so nothing disappears (V `add_layer_keeps_existing_layers_listed_when_order_was_absent`) |
| Names: any text | U | Trimmed, control characters dropped, at most 256 characters; empty is refused. Duplicate names are allowed (U for Revu) |
| Delete layer (with a choice to delete or keep its content and mark-ups) | V BB-del | Not built (follow-up) |
| Add Layer from Page / Import Layer / Export Layer | V BB-panel, BB-add | Not built |

Adding a layer writes the file now (it needs a real object number so mark-ups can point at
it). It flushes the document's unsaved mark-ups first and clears the unsaved flag after, the
way the page operations do. The panel says so ("Adding a layer saves the file, including your
mark-ups"). The stale-source guard applies: the write goes through `apply_page_edit`, which
refuses when the file changed on disk since it was loaded (#141).

## 4. What is drawn (page content)

PDFium has no call to change optional-content visibility, so a view is rendered by loading
the same file with an incremental update appended **in memory** (`layers/overlay.rs`): the
`/D` configuration gets the view's `/OFF` list. PDFium reads the file plus the update through
a `Read + Seek` stream; the file on disk is not opened for writing.

| Fact | Evidence |
|---|---|
| On a real AutoCAD-derived, Bluebeam-saved sheet (8 layers, machine-local corpus `bench/corpus/bb-ref/markup-test-original.pdf`) hiding all layers lightens the page (content on no layer stays), hiding `Wall` changes it, an empty hidden set equals the original pixels, and releasing the view restores them exactly | V `real_bluebeam_reference_file_hides_and_restores_layers` (skips where the corpus is absent, e.g. CI) |
| PDFium honours `/D /OFF` at open | V `pdfium_overlay_can_show_what_the_file_hides` |
| The overlay hides and shows groups for classic-table, xref-stream and object-stream files | V `pdfium_honours_the_default_configuration_and_the_overlay_both_xref_kinds` (three file kinds, hide blue / hide red / hide both / back to the file) |
| PDFium honours a group's own `/Usage /View /ViewState` even with no `/AS` entry, and it outranks `/D` in both directions | V `pdfium_view_state_outranks_the_configuration_and_the_overlay_overrides_it`, `pdfium_view_state_on_outranks_d_off`. The spec ties ViewState to `/AS` (ISO 8.11.4.5); PDFium does not. The panel's initial state follows the renderer so the checkbox always matches the page. Acrobat/Revu agreement: U |
| So the overlay also rewrites a group whose ViewState disagrees with the view (object `n g obj` with `ViewState` set) | V the same tests |
| PDFium reads the stream with one `read` call per block and takes a short count as the data, so the reader must fill a buffer across the file/update seam | V found by test: before the fix every PDFium test failed; `overlay_reader_matches_the_concatenated_bytes` pins it |
| A failed load keeps the current view; the new document is probed (page count, page 0) before the old one is dropped | V `pdfium_a_failed_view_keeps_the_current_one` (reader refusal), engine code |
| Scope of the overlay: unencrypted, starts with `%PDF-`, under 4 GiB, under the engine's mmap threshold | I; otherwise the panel lists the layers read-only with the reason shown (V `LayersPanel` test "cannot be rendered") |
| A save, page operation or restore replaces the renderer's document and drops the view; the backend re-applies the user's view before the next tile (`ensure_view`, keyed on a generation counter bumped by every open/close), and re-reads the file's layers first when the file was rewritten since | V `commands::layers::tests::the_view_survives_a_reopen_and_a_rewrite_of_the_file` (real PDFium through the real `RenderHandle`: hide, reopen -> view gone, `ensure_view` -> hidden again, add a layer + rewrite + reopen -> hidden again, release -> shown) |
| Reading mode (continuous) refetches on a view change | V `ReadingView` test |
| PDFium's text search/selection ignores optional content | I (not changed here): text on a hidden layer is still found |

## 5. Mark-ups and layers

| Behaviour | Source | Redline |
|---|---|---|
| Assign a mark-up to a layer, or to a new layer, from its context menu | V BB-add | Properties panel, Layer field: a list of the document's layers (or "(none)"). Selecting sets `/OC` to that group and `layer` to its name, as one undo step (also for a multi-selection). With no layers in the file the field stays a free-text label |
| The Markup Layer: new mark-ups are created on it, shown by an icon after the name | V BB-panel, BB-how | "Use for new mark-ups" on the selected layer; a "mark-ups" badge marks it. Applies to every mark-up created in the session (`MarkupStore.create/createMany`) unless the mark-up already carries a layer (a Tool Chest tool may) |
| If the Markup Layer is hidden when a mark-up is created | U | The layer is switched on, so what the user draws does not vanish (I) |
| Mark-ups on a hidden layer are hidden | V BB-pdf ("control the visibility of everything on that layer ... markups") | Not drawn, not hit-tested, not marquee-selected, no selection chrome. A group move can still carry a hidden member (I) |
| Layers appear in the Markups list "to organize and process mark-ups by layer" | V BB-pdf | The Layer column (already there) shows the group's **current** name for every mark-up on a layer (it wins over a stale `/RLLayer` label: V `the_groups_current_name_wins_over_a_stale_label`), and the filter menu has a **Layer** facet (shown when any mark-up has a layer). The facet and the legend filter use the same key, the group's object number; two groups with one name are two entries ("Name (#12)"). The export of the current view follows the filter |
| Legends | I | A legend can be limited to one layer (Properties > legend > Layer). The Rust and TypeScript engines agree on a new case in the shared fixture. A legend that is not limited counts every mark-up whatever the view |
| **Numbers never change with visibility** (a rule from the brief) | V | Hiding a layer changes only what is drawn. The Markups List, legend tables, Quantities panel and exports read the mark-up store, not the view. V `Viewport.layers.test.ts` ("a legend still counts mark-ups on a hidden layer", "visibility never changes the Markups List") |
| Flatten of a mark-up on a layer flattens onto that layer | V BB-add ("If you flatten a markup that is associated with a layer, it will flatten to that layer") | The baked appearance form carries the annotation's `/OC` (V `flatten_keeps_a_markup_on_its_layer`); a dangling `/OC` is not carried |
| A mark-up whose `/OC` is a membership dictionary (OCMD) | V ISO 8.11.2.2 | Visibility is evaluated (`/VE`, else `/P` over the groups that exist; an OCMD with no existing group is always visible). Properties shows it as "other" (read-only) until a real layer is chosen |
| A mark-up whose `/OC` is a plain string (from a `.btx`) | V `markup/mod.rs` | Unchanged; shown as a label ("other") |
| Copy/paste keeps the layer | V BB-pdf | A pasted mark-up is a clone, so it keeps `/OC` (I; not separately tested) |

### Save rules for `/OC` (consistent with #140/#141)

- An unedited foreign annotation is written back from its original, `/OC` included (V
  `an_unedited_foreign_markup_keeps_its_oc_and_gets_its_layer_name`). Filling `layer` from the
  group name at read time happens before the "as loaded" fingerprint is taken, so it does not
  make a foreign mark-up count as edited.
- A `/OC` reference is kept when it resolves to a dictionary that is `/Type /OCG`, `/Type
  /OCMD`, or listed in `/OCProperties /OCGs` (a group without `/Type` is non-conforming, but
  the panel presents it as a layer, so an assignment to it must survive: V
  `a_typeless_group_listed_in_ocgs_keeps_assignments_and_foreign_oc_over_three_cycles`). It is
  dropped (a missing object, a non-dictionary, or a dictionary that is neither, e.g. a hostile
  file aiming it at the page: V `a_typeless_dictionary_not_listed_in_ocgs_is_still_not_optional_content`)
  on write, for preserved originals (#140 dropped a missing one) **and** for mark-ups rebuilt from
  the model (new). V
  `a_dangling_oc_reference_is_dropped_on_write_for_rebuilt_and_preserved_markups`.
- Deleting a mark-up leaves its layer; undoing the delete restores it on that layer. V
  `deleting_a_markup_leaves_its_layer_and_an_undo_puts_it_back_on_it`.

## 6. Untrusted input: bounds

`/OCProperties` comes from the PDF. Every walk is bounded, nothing panics on a malformed
entry (it is skipped), strings are text only (Svelte text nodes; a name such as
`<img onerror=...>` is shown literally: V `LayersPanel` test).

| Structure | Bound | Test |
|---|---|---|
| Groups read | 5,000 (`truncated` and `layers_truncated` set). A file past the bound lists the first 5,000 but refuses the view and "Save as default" (they rewrite the whole configuration and would switch the unread groups on) | `hostile_more_groups_than_the_bound_are_truncated`, `a_file_with_more_groups_than_are_read_refuses_the_view_and_save_default` |
| `/Order` elements walked | 20,000 | the same walk; budget exhaustion sets `truncated` |
| `/Order` depth | 16 | `hostile_deeply_nested_order_is_cut_at_the_depth_bound` (200 levels) |
| `/Order` cycles (an indirect array containing itself) | cut by a path check; each layer listed once | `hostile_order_that_references_itself_terminates` |
| Membership dictionaries | 2,000 | by constant |
| `/VE` depth / nodes | 8 / 64; a violating expression is dropped (the dictionary falls back to `/P`) | `hostile_visibility_expression_cycles_and_depth_are_bounded` |
| Evaluation depth in the frontend | 16 | `a runaway expression depth does not recurse without bound` |
| Radio-button groups | 256 groups x 256 members | by constant |
| Names | control characters dropped, 256 characters | `hostile_names_are_clean_bounded_text` |
| Non-group `/OCGs` entries, wrong types everywhere | skipped, no panic | `hostile_ocgs_entries_that_are_not_groups_are_skipped`, `hostile_malformed_vectors_do_not_panic` |

The overlay writes only objects it built from the parsed dictionaries, through its own
serialiser (names escaped, strings written as hex, streams refused). Offsets above 4 GiB are
refused. The written file is untouched by a view; the structural edits use the existing
atomic temp + rename path.

## 7. Data model

```
LayerModel { layers: [Layer], tree: [LayerNode], ocmds: [Ocmd], rb_groups, has_ocproperties,
             truncated, unlisted, view_supported, view_unsupported_reason, encrypted }
Layer      { id (object number), gen, name, default_on, locked, view_intent, usage_view_state }
LayerNode  = Layer { id, children } | Group { label, children }
Ocmd       { id, policy, ocgs, ve }       OcExpr = Group | And | Or | Not
```

The view is a set of hidden group ids held by the frontend (`LayerStore.off`) and sent to the
backend as `set_layer_view(docId, off | null)`; `null` means "the file as it is". Commands:
`read_layers` (large files, over 24 MB, wait for the user), `set_layer_view`, `add_layer`,
`save_layers_default`. A mark-up's `Markup.optional_content` is `{ Reference: [n, g] }` and
`LegendProps.layer` is a group's object number.

## 8. Not built (follow-ups, named)

Alternate configurations (`/Configs`) and a configuration list; reorder/re-parent by drag;
Add Before/After; delete layer; rename; layer properties dialog (print/export/zoom usage);
Show Print/Export layers; layers-on-page-only; add layer from page / import / export;
"Create New Layer" from a mark-up's context menu; tool sets bound to layers; the MCP tools
(`create_markup` still sets only the `layer` label); layer-aware text search; stamps.

## 9. Owed (not verified here)

- A live run in the app (`cargo tauri dev`): toggle, save, page operation, restore with a view
  active, on a real CAD export. The pieces and the re-apply after a reopen are tested; the
  Tauri command wiring and the webview are not exercised by an automated test.
- A human check in Bluebeam Revu and Acrobat that an `/OC`-assigned mark-up and a redline-made
  layer behave as expected there (same "owed, not assumed" posture as G9).
- Windows: the overlay reader is plain `Read + Seek`; not run on Windows.

## 10. Test index

Rust (`src-tauri/src/layers/tests.rs`, 41 tests, and 2 in `commands/layers.rs`; PDFium ones need `PDFIUM_DYNAMIC_LIB_PATH`
and `--test-threads=1`): model reading, hostile input, overlay bytes (classic / xref stream),
reader seam, PDFium rendering, edits, mark-ups on layers (3 cycles), structural edits with
mark-ups. `docops` flatten (2), `markup::legend` shared fixture (+1 case).
Frontend: `layers.test.ts`, `layers-store.test.ts`, `LayersPanel.test.ts`,
`Viewport.layers.test.ts`, plus additions to `PropertiesPanel`, `MarkupsListPanel`,
`markups-list`, `LegendControls`, `ReadingView`, `legend` tests.
