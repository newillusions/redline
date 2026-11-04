/**
 * Single-letter tool keys, aligned with Bluebeam Revu's defaults (users are Revu users).
 * Source: https://support.bluebeam.com/revu/resources/keyboard-shortcuts.html (Markup,
 * Measure and Selection tables, fetched 2026-11-04). Pure data + one lookup so the
 * Viewport key handler, the palette tooltips and the cheat-sheet share one table.
 *
 * Deliberate deviations from Revu:
 *  - Y stays as a second Cloud key (it was Redline's own binding before this table).
 *  - U / D / Shift+U (Underline / Strikethrough / Squiggly) are kept; Revu lists no
 *    single-key text-markup binding.
 *  - M picks Measure Length (Revu's M opens a generic "Measure Tool").
 *  - The zoom-toolbar toggle moved from T to Z (T is Text Box in Revu).
 */
import type { ToolKind } from "./markup-store.svelte";

/** A tool key picks a drawing tool, or opens the Stamp picker (S). */
export type ToolKeyAction = ToolKind | "stamp";

export interface ToolKeyBinding {
  /** Lower-case key; compared with `e.key.toLowerCase()`. */
  key: string;
  shift: boolean;
  action: ToolKeyAction;
  /** Human label for the cheat-sheet. */
  label: string;
  /** Revu's own key for the same tool when it differs; shown nowhere, documents the alias. */
  alias?: boolean;
}

export const TOOL_KEY_BINDINGS: readonly ToolKeyBinding[] = [
  { key: "r", shift: false, action: "Rectangle", label: "Rectangle" },
  { key: "e", shift: false, action: "Ellipse", label: "Ellipse" },
  { key: "l", shift: false, action: "Line", label: "Line" },
  { key: "a", shift: false, action: "Arrow", label: "Arrow" },
  { key: "h", shift: false, action: "Highlight", label: "Highlight" },
  { key: "c", shift: false, action: "Cloud", label: "Cloud" },
  { key: "y", shift: false, action: "Cloud", label: "Cloud (alias)", alias: true },
  { key: "k", shift: false, action: "CloudPlus", label: "Cloud+" },
  { key: "q", shift: false, action: "Callout", label: "Callout" },
  { key: "t", shift: false, action: "Text", label: "Text Box" },
  { key: "n", shift: false, action: "Note", label: "Sticky Note" },
  { key: "n", shift: true, action: "Polyline", label: "Polyline" },
  { key: "p", shift: false, action: "Ink", label: "Pen (freehand)" },
  { key: "p", shift: true, action: "Polygon", label: "Polygon" },
  { key: "t", shift: true, action: "selectText", label: "Select Text" },
  { key: "m", shift: false, action: "MeasurementLength", label: "Measure Length" },
  { key: "s", shift: false, action: "stamp", label: "Stamp" },
  { key: "u", shift: false, action: "Underline", label: "Underline" },
  { key: "u", shift: true, action: "Squiggly", label: "Squiggly" },
  { key: "d", shift: false, action: "StrikeOut", label: "Strikethrough" },
  { key: "v", shift: false, action: "select", label: "Select" },
];

interface KeyLike {
  key: string;
  shiftKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
}

/** The action a bare (Shift-only) key press selects, or null. Modified presses never match. */
export function toolForKeyEvent(e: KeyLike): ToolKeyAction | null {
  if (e.metaKey || e.ctrlKey || e.altKey) return null;
  // Decide by Shift, not by the character's case: with Caps Lock on a plain U arrives as "U".
  const k = e.key.toLowerCase();
  return TOOL_KEY_BINDINGS.find((b) => b.key === k && b.shift === e.shiftKey)?.action ?? null;
}

/** Display key for a tool, e.g. "R" or "Shift+P"; null when the tool has no key. */
export function keyLabelFor(action: ToolKeyAction): string | null {
  const b = TOOL_KEY_BINDINGS.find((x) => x.action === action && !x.alias);
  if (!b) return null;
  return `${b.shift ? "Shift+" : ""}${b.key.toUpperCase()}`;
}
