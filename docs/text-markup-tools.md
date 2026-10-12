# Text mark-up tools and sticky Note - behaviour spec

Tools: **Underline**, **Strikethrough** (`StrikeOut`), **Squiggly**, **Note** (sticky note).
Written before the code, from how Revu behaves, not from the feature list (owner lesson
2026-10-12: mark-up tools built from the docs were unusable for a long time).

Labels: **[verified]** = stated on a raw Bluebeam page fetched 2026-10-12 (URL given).
**[inferred]** = reasoned from PDF-spec knowledge or the repo, not read from Bluebeam.
**[unknown]** = not checked; the owner check at the end settles it.

## Sources (raw pages, fetched with curl, not summaries)

- Edit and review text (Underline / Squiggly / Strikethrough gestures):
  https://support.bluebeam.com/user-manual/menus/tools/edit-review-text.html
- Advanced Text toolbar (where the three tools live):
  https://support.bluebeam.com/user-manual/toolbars/advanced-text-toolbar.html
- Note tool: https://support.bluebeam.com/user-manual/menus/tools/note-tool.html
- Highlight tool (the text-snapping gesture these tools share with Revu's Highlighter):
  https://support.bluebeam.com/user-manual/menus/tools/highlight-tool.html
- Highlight and Select Text troubleshooting (snap-to-text, Ctrl override, multi-line caveat):
  https://support.bluebeam.com/revu/troubleshooting/highlight-and-select-text-arent-working.html
- Keyboard shortcuts: https://support.bluebeam.com/revu/resources/keyboard-shortcuts.html

## Underline / Strikethrough / Squiggly

| Aspect | Behaviour | Label |
|---|---|---|
| Creation gesture | Text-selection based. Pick the tool, drag over text, release: the selected text is marked up on release. No Enter, no confirmation. | [verified] edit-review-text.html: "Drag to select text to be underlined. Release the mouse button. The selected text will be underlined." |
| Multi-line | One stroke per text line. Geometry is one quad per line (`/QuadPoints`), never merged into a bounding box. | [inferred] ISO 32000-1 12.5.6.10; same geometry redline already uses for text Highlight |
| No text under the cursor | Nothing is created. The cursor stays an arrow; after the drag the "No text selected ... scanned or image-only PDF ... Auto-OCR" hint appears (same copy the Select Text tool gives). Redline does not fall back to a freehand line: Revu documents no freehand mode for these three tools. | redline behaviour; absence of a freehand mode [inferred] from the pages above, which describe only text selection |
| Modifier keys | None implemented. Revu's transcripts show Ctrl (or Alt) suppressing text-grab and Ctrl+Shift axis-locking for the HIGHLIGHTER's freeform mode; nothing shows a freeform mode for Underline / Strikethrough / Squiggly, so none is invented here (see Gaps). Plain Ctrl+U / Cmd+N etc. are NOT bound (left to the OS and the app). | Highlighter Ctrl/Ctrl+Shift [verified, scout-revu-behaviour 2.3, vrLxtG-sHSc@158-219]; none for these three [unknown] |
| Hover cursor | I-beam over selectable text, arrow elsewhere (shared with Select Text). | [inferred] from highlight-and-select-text-arent-working.html ("cursor will change to an I-beam and snap to the text") |
| After creation | The markup exists and the tool stays armed for the next selection. Click it later with the Select tool to select it (hit area = its text-line quads). Revu notes text-mode highlights "cannot be moved or resized"; redline currently lets a selected quad markup be moved/resized like its existing Highlight (see Divergences). | stays-armed [inferred]; Revu move/resize rule [verified] (highlight-tool.html) |
| Shortcuts | `U` Underline, `D` Strikethrough, `Shift+U` Squiggly. Ignored while typing in a text field and when Ctrl/Cmd/Alt is held. | [verified] keyboard-shortcuts.html |
| Properties exposed | Colour, Weight (rule thickness, default 1 pt), Opacity, Subject, Contents (comment), Status, Layer, Author (read-only). Fill / Line style are hidden because they do nothing here. | colour/opacity/subject/status are the properties the task named; Revu exposes the same set on markups [inferred] |
| Default look | Draft colour and opacity; weight forced to 1 pt (the shared 2 pt draft weight is a shape outline). Underline = rule on the quad's bottom edge; Strikethrough = rule through mid-height; Squiggly = zig-zag on the bottom edge. | redline choice |

### PDF output (Bluebeam / Acrobat)

`/Subtype /Underline | /StrikeOut | /Squiggly`, `/QuadPoints` (8 numbers per line, order
top-left, top-right, bottom-left, bottom-right - the de-facto Acrobat/Bluebeam order),
`/C` colour, `/CA` = the opacity, `/Contents`, `/Subj`, `/T` author, `/Rect` = `/AP` BBox.
A strict viewer regenerates the rule from `/C` + `/CA` + `/QuadPoints`; an `/AP`-honouring
viewer paints our `/AP` (opaque; `/CA` supplies the alpha once, no double-dimming) - the same
arrangement redline already uses for Highlight [inferred from the Highlight comments in
`annotation.rs`; proven only for PDFium, see Evidence].

Foreign Underline / StrikeOut / Squiggly annotations (no `/RL*` keys) now import as their own
types with their `/QuadPoints`, colour, `/CA` opacity, contents, subject and author. Before
this change they were skipped on import and left untouched in the file.

## Note (sticky note)

| Aspect | Behaviour | Label |
|---|---|---|
| Creation | Click where the note goes. An icon is placed and its pop-up (an inline text box) opens at once, ready for typing. | [verified] note-tool.html: "Select a location on the document to place the Note. A Note icon is placed on the PDF and a pop-up window appears." |
| Empty note | Closing the pop-up with no text keeps the icon (no contents). | [inferred] note-tool.html describes hiding the pop-up, not discarding the note |
| Hide pop-up | Esc or click away. Esc keeps what was typed. | Esc [verified] note-tool.html; keeping the text [inferred] |
| Reopen | Double-click the icon (Select tool). | [verified] "Double-click the Note icon to bring up the pop-up window." |
| Hover tooltip with the contents | NOT implemented. | [verified] Revu does it; follow-up |
| Selection | Select tool, click the icon (hit area = the 20 pt icon box). Move/delete with the normal select-tool commands. | [inferred] |
| Delete | Select + Delete. | [verified] note-tool.html |
| Shortcut | `N`. | [verified] keyboard-shortcuts.html |
| Properties | Colour (icon colour), Opacity, Icon (Note, Comment, Key, Help, NewParagraph, Paragraph, Insert), Subject, Contents, Status, Author. | [verified] note-tool.html lists Color, Opacity, Icon |
| Default look | Yellow icon (`#ffd400`), a coloured box with three text lines. Revu's default Note is yellow. | yellow default [unknown]; chosen to read as a sticky note |
| Printing | Notes/comment boxes do not print by default in Revu. Redline writes the standard Print flag (`/F 4`) like every other markup. | [verified] Revu default (note-tool.html); redline flag [unknown] effect |

### PDF output

`/Subtype /Text` (a standard PDF text annotation, not `/FreeText`), `/Contents`, `/Name`
(icon), `/Open` (pop-up state, round-tripped; no UI toggle), `/C` colour, `/CA` opacity,
`/T`, `/Subj`, `/Rect` = a 20x20 pt box centred on the click point, `/AP` icon. No `/Popup`
is written. Foreign `/Text` annotations now import as Notes (icon name and `/Open` kept);
before this change they imported as FreeText boxes and were rewritten as FreeText on save.

## Gaps from Revu's tutorial transcripts (scout-revu-behaviour, sections 2.3 and 2.4)

- **Double-click a word marks that word** [verified, vuVwE45ZOk8@83-90, @126-130]. Not implemented: needs word boundaries from the text layer. Triple-click line/paragraph [unknown].
- **Drag covers the words and the gap between them** [verified, vuVwE45ZOk8@126-139]. Redline's range quads are per text line, so gaps inside a line are covered [inferred from the quad construction, not measured against Revu].
- **Ctrl / Alt = freeform, Ctrl+Shift = axis-locked** is documented for the Highlighter only [verified, vrLxtG-sHSc]. Whether Underline / Strikethrough / Squiggly have a freeform mode is [unknown]; not implemented, not guessed.
- **"Reuse Tools" preference** keeps a tool armed [low-confidence web snippets, unverified]; redline keeps these tools armed always.
- **Selected text copied into the Comments column** for text-grab markups [verified for the Highlighter, vrLxtG-sHSc@232-266]; not implemented here.
- **Note**: transcript coverage is thin. Only "select the tool, click where you'd like the markup, then start typing" is verified [BbBylKOedOw@58-68], matching the implementation. Icon appearance, default size, pop-up open/close rules, whether text lives in `/Contents` with a `/Popup`, edit gesture and keyboard shortcut are not in the transcripts; the note-tool.html help page remains the source for Esc, double-click, N and the property list.

## Divergences from Revu (named, not hidden)

1. Text markups can be moved/resized as quads like the existing Highlight; Revu forbids it for
   text-mode markups.
2. No Note hover tooltip; no `/Popup` written; one generic icon is drawn for every icon name
   (the name itself round-trips, so Bluebeam/Acrobat draw their own glyph).
3. Revu's "Copy selected text into Text Markups" preference (selected text copied into the
   markup's comment) is not implemented.
4. Underline / Strikethrough / Squiggly are in the main tool strip, not a separate "Advanced
   Text" toolbar.
5. Review Text (replace / insert text edits) is out of scope.

## Evidence in this PR

- Rust: write/read round trip of each subtype incl. `/QuadPoints` and Note `/Name` `/Open`
  (`markup::annotation`, `document::annots`); foreign Underline/Text load and re-save keep
  `/QuadPoints /C /CA /Contents /T /Subj /Name /Open`; the 24-type fidelity matrix and
  multi-cycle (write, edit, reload, reload) harness now include the four types.
- PDFium strict-reader render (`render::tests::strict_reader_paints_text_markup_and_note_at_the_authored_geometry`):
  rules land on the authored quad edges, squiggle stays on the baseline band, note icon box is
  centred on its point. PDFium honours `/AP`; it is not Bluebeam.
- `qpdf --check` is clean on the emitted `Underline.pdf`, `StrikeOut.pdf`, `Squiggly.pdf`,
  `Note.pdf`, `AllTypes.pdf`.
- Frontend: tool palette, shortcuts, drag-over-text creation on release, no-text hint, Note
  placement / Esc / reopen, properties panel, render parity with the Rust geometry.

## Not verified: Bluebeam Revu / Acrobat rendering [unknown]

Revu and Acrobat regenerate these annotations from the dictionary, so a PDFium render does not
settle how they look there. No genuine Bluebeam-authored Underline / Squiggly / Note reference
file is available on the build machine (`bench/corpus/bb-ref/` is empty here), and the
cross-viewer harness (`tools/crossviewer`) captures the screen of the owner's workstation
(mr-desktop), which the workspace rule "screen captures only when the owner says idle" bars
running unattended. Owner command (run when the workstation is idle):

```bash
cd src-tauri && REDLINE_CROSSVIEWER_OUT=/tmp/redline-crossviewer \
  cargo test --lib -- --ignored --nocapture emit_crossviewer_corpus
cd .. && node tools/crossviewer/run-crossviewer.mjs
```

Look at `Underline.pdf`, `StrikeOut.pdf`, `Squiggly.pdf`, `Note.pdf` and `AllTypes.pdf` in the
Acrobat and Revu renders, and answer: (1) do the rules sit on the text baseline / mid-height at
the right opacity; (2) does Revu list each as Underline / Strikethrough / Squiggly / Note in its
Markups List with the author and comment; (3) does the Note show a Comment icon (name
round-trip) and open its pop-up with the contents; (4) does a Revu edit (move, recolour) and
re-open in redline keep the markup as the same type.

## Hostile input (security hardening)

`/QuadPoints` and annotation rectangles from a PDF are untrusted. Limits, all applied on read
(`annotation.rs`) and again when the appearance / SVG is built (`appearance.rs`,
`markup-render.ts`): at most 2,000 quads per annotation (the rest are ignored); a quad with any
non-finite coordinate is dropped; total squiggle vertices per annotation are capped at 50,000
(each quad gets `max(2, 50000 / quads)` segments, so a long baseline gets a coarser wave, never
more vertices); squiggle weight clamps to 0.5..100 and a non-finite weight counts as 1; a
baseline whose length overflows degrades to a straight edge; a Note with a non-finite point draws
nothing, and an overflowing foreign `/Rect` parks the Note at the page origin.
