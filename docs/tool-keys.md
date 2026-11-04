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

## Checks

`tools/gui-harness-tool-keys.mjs` (real browser, mocked IPC) covers keys, typing guard, Z vs T, the
Stamp picker placing a stamp, Recent, and reading-mode disabling.
