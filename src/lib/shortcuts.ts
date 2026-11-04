/**
 * Every keyboard shortcut, in one table: the `?` cheat-sheet over the page and the Settings
 * dialog both render it. The tool letters come from `TOOL_KEY_BINDINGS` so they cannot drift;
 * the rest is a description of what the key handlers in `Viewport.svelte` and `App.svelte`
 * do. Add a binding there, add its row here (`shortcuts.test.ts` guards the tool rows).
 */
import { TOOL_KEY_BINDINGS, NUMBERED_TOOL_COUNT } from "./tool-keys";

export interface ShortcutRow {
  keys: string;
  label: string;
}

export interface ShortcutGroup {
  title: string;
  rows: ShortcutRow[];
}

const row = (keys: string, label: string): ShortcutRow => ({ keys, label });

export function shortcutGroups(): ShortcutGroup[] {
  return [
    {
      title: "Pan & zoom",
      rows: [
        row("Two-finger swipe / mouse wheel", "Pan"),
        row("Pinch (trackpad)", "Zoom at fingers"),
        row("Shift + scroll", "Zoom at cursor"),
        row("⌘/Ctrl + scroll", "Zoom at cursor"),
        row("⌘/Ctrl + / −", "Zoom in / out"),
        row("Space + drag", "Pan with any tool"),
        row("⌘/Ctrl + 0", "Fit width"),
        row("⌘/Ctrl + 9", "Fit height"),
        row("⌘/Ctrl + 8", "Actual size (100%)"),
        row("⌘/Ctrl + ← / →", "Previous / next page"),
        row("Alt + ← / →", "Previous / next view (after a jump or big zoom)"),
        row("Z", "Show/hide the zoom toolbar"),
      ],
    },
    {
      title: "Tools (same keys as Bluebeam Revu)",
      rows: [
        ...TOOL_KEY_BINDINGS.filter((b) => !b.alias).map((b) =>
          row(`${b.shift ? "Shift+" : ""}${b.key.toUpperCase()}`, b.label),
        ),
        row("Y", "Cloud (second key)"),
        row(`1 - ${NUMBERED_TOOL_COUNT}`, "The first nine tools of the first Tool Set (numbered in the Tool Chest)"),
        row("Esc", "Back to Select"),
      ],
    },
    {
      title: "Markups",
      rows: [
        row("⌘/Ctrl + C / X / V", "Copy / cut / paste markups (paste lands at the pointer)"),
        row("⌘/Ctrl + Shift + V", "Paste markups in place"),
        row("⌘/Ctrl + D", "Duplicate the selected markups"),
        row("⌘/Ctrl + Shift + C", "Copy the format of the selected markup (format painter)"),
        row("⌘/Ctrl + Alt + V", "Paste that format onto the selected markups"),
        row("⌘/Ctrl + G / Shift+G", "Group / ungroup the selected markups"),
        row("Delete / Backspace", "Delete the selection (or the selected vertex)"),
        row("Enter", "Finish a multi-point measurement or markup"),
        row("Shift + 1 - 9", "Set the status of the selected row (Markups list)"),
        row("Right-click · Shift+F10", "Context menu for a markup or the page"),
      ],
    },
    {
      title: "Document",
      rows: [
        row("⌘/Ctrl + Z", "Undo"),
        row("⌘/Ctrl + Shift + Z · ⌘/Ctrl + Y", "Redo"),
        row("⌘/Ctrl + S", "Save"),
        row("⌘/Ctrl + W", "Close the tab"),
        row("Ctrl + Tab / Ctrl + Shift + Tab", "Next / previous tab"),
        row("⌘/Ctrl + F", "Search"),
        row("F3 / Shift + F3", "Next / previous search result"),
        row("⌘/Ctrl + Shift + R", "Switch between single-page and reading mode"),
        row("?", "Show this list"),
      ],
    },
  ];
}
