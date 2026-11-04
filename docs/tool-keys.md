# Tool keys, Stamp tool, Recent tools

Added 2026-11-04 (UX benchmark must-fix 1, 3, 5, 6: `/Users/martin/dev-reports/2026-11-04/redline-ux-benchmark.md`).

## Single-letter tool keys (Bluebeam Revu defaults)

Source: <https://support.bluebeam.com/revu/resources/keyboard-shortcuts.html> (Markup, Measure and
Selection tables, read 2026-11-04). Code: `src/lib/tool-keys.ts` (one table shared by the key
handler, the palette tooltips and the `?` cheat-sheet). Keys are ignored while a text input,
textarea, select or the inline text editor has focus, and when Ctrl/Cmd/Alt is held.

| Key | Tool | Revu |
|---|---|---|
| V | Select | V |
| R | Rectangle | R |
| E | Ellipse | E |
| L | Line | L |
| A | Arrow | A |
| H | Highlight | H |
| C | Cloud | C |
| Y | Cloud (second key, kept from before) | - |
| K | Cloud+ | K |
| Q | Callout | Q |
| T | Text Box | T |
| N | Sticky Note | N |
| Shift+N | Polyline | Shift+N |
| P | Pen (freehand) | P |
| Shift+P | Polygon | Shift+P |
| Shift+T | Select Text | Shift+T |
| M | Measure Length | M (generic Measure Tool) |
| S | Stamp picker | S |
| U / Shift+U / D | Underline / Squiggly / Strikethrough | not in Revu's list; kept |

Moved: the zoom-toolbar toggle is now `Z` (was `T`).

Not bound (no Redline tool, or Revu needs Alt chords): Pan (Shift+V), Zoom (Z), Typewriter (W),
Image (I), Eraser, Lasso, Dimension, and the Shift+Alt measure family.

## Stamp tool

The palette has a Stamp button (key `S`) that opens a picker: built-in review stamps (Approved,
Rejected, Revise and Resubmit, Reviewed, For Information) plus any Stamp / StampDynamic tools saved
in Tool Chest sets. Choosing one arms Drawing-mode placement (the same path as a Tool Chest
Drawing-mode tool); the next click on the page drops it. Built-ins are `StampDynamic` tools with
no artwork and no fields (`src/lib/builtin-stamps.ts`), so they draw the bordered-box + label
appearance and the label text is the markup contents. No date / user field yet.

## Reading mode

Reading mode cannot draw. Every palette tool (and the Stamp and Link buttons) is disabled and a
"Switch to page view to mark up" hint shows in the palette.

## Recent tools

Any change of the active draw tool (palette click, tool key, Stamp pick, Tool Chest) is recorded in
the Tool Chest's Recent list (`ToolChestStore.noteActiveTool`, driven from `App.svelte`). Palette
and key tools are stored as Properties-mode tools carrying the current draft appearance with fixed
UUID ids (Rust `Tool.id` is a Uuid), so re-using a tool moves it to the front. A Tool Chest
activation is recorded once, as the real named tool, not twice. Switching document tabs is not
recorded.

## Reuse tools, digit keys, format painter, thumbnails, shortcut table

Added 2026-11-05 (UX benchmark should-fix 2, 3, 4, 5, 8, 7-first-step).

- **Reuse tools** (`lib/ui-prefs.svelte.ts`, `lib/tool-reuse.ts`). On by default: a draw tool stays active after each markup, as
  before. Off: the tool drops back to Select once the markup is made (`MarkupStore.create` /
  `createMany`). The Count tool always keeps counting. Toggle: Settings, or the "Reuse" button on
  the zoom bar. Stored in localStorage (`redline.reuseTools`).
- **Digit keys 1-9** activate the first nine tools of the FIRST Tool Set (Revu: "My Tools" hot
  keys by position). The Tool Chest shows the digit next to each. Bare digits only: not while
  typing, in a dialog or menu, or in reading mode. Cmd/Ctrl+digit stays zoom.
- **Format painter** (`lib/format-painter.ts`). Ctrl/Cmd+Shift+C copies the selected markup's look,
  Ctrl/Cmd+Alt+V pastes it onto the selection (one undo step; the font only reaches markups that
  already have one; locked markups are skipped). The markup right-click menu has Copy Format,
  Paste Format and "Use as Default Appearance" (sets the look of new markups).
- **Thumbnail size**: a slider at the top of the Pages panel sets the minimum thumbnail width
  (70-240 px); thumbnails flow into as many columns as fit. Stored in localStorage.
- **Shortcut table** (`lib/shortcuts.ts`): one list for the `?` overlay and Settings.

## Checks

`tools/gui-harness-tool-keys.mjs` (real browser, mocked IPC) covers keys, typing guard, Z vs T, the
Stamp picker placing a stamp, Recent, and reading-mode disabling.
