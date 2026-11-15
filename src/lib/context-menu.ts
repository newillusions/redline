/**
 * Right-click menu model - pure, no Svelte or DOM. Three menus share one component
 * (`ContextMenu.svelte`): on a markup, on the empty page area, and on a Navigator thumbnail.
 * Each builder returns a flat list of items; every `id` maps to a command that already exists
 * in the app (arrange-actions, markup-properties, the viewport zoom presets, the page ops the
 * thumbnails already run), so this module decides what is OFFERED, never what happens.
 */
import type { MarkupStatus } from "./ipc";
import { STATUS_ORDER } from "./markups-list";

export type MenuItem =
  | { kind: "item"; id: string; label: string; shortcut?: string; disabled?: boolean; danger?: boolean }
  | { kind: "separator" }
  | { kind: "heading"; label: string };

export type ActionItem = Extract<MenuItem, { kind: "item" }>;

const item = (id: string, label: string, o: { shortcut?: string; disabled?: boolean; danger?: boolean } = {}): MenuItem =>
  ({ kind: "item", id, label, ...o });
const SEP: MenuItem = { kind: "separator" };

const STATUS_LABEL: Record<MarkupStatus, string> = {
  None: "None", Accepted: "Accepted", Rejected: "Rejected", Cancelled: "Cancelled", Completed: "Completed",
};

/** Id prefix of the status items; the rest of the id is the `MarkupStatus`. */
export const STATUS_ID_PREFIX = "status:";

export function statusFromId(id: string): MarkupStatus | null {
  if (!id.startsWith(STATUS_ID_PREFIX)) return null;
  const s = id.slice(STATUS_ID_PREFIX.length) as MarkupStatus;
  return STATUS_ORDER.includes(s) ? s : null;
}

export interface MarkupMenuState {
  /** Number of selected markups (at least 1). */
  count: number;
  /** Any selected markup is locked: edits are offered greyed out (the store would skip them). */
  anyLocked: boolean;
  /** The one status all selected markups share, when they share one. */
  commonStatus?: MarkupStatus;
  /** Every selected markup has the Locked flag set: the menu then offers Unlock, else Lock. */
  allLocked?: boolean;
  /** A format has been copied (format painter), so Paste Format has something to paste. */
  canPasteFormat?: boolean;
}

export function markupMenuItems(s: MarkupMenuState): MenuItem[] {
  const edit = s.anyLocked;
  const many = s.count > 1;
  return [
    { kind: "heading", label: many ? `Status (${s.count} markups)` : "Status" },
    ...STATUS_ORDER.map((st) =>
      item(STATUS_ID_PREFIX + st, s.commonStatus === st ? `${STATUS_LABEL[st]} (current)` : STATUS_LABEL[st], { disabled: edit }),
    ),
    SEP,
    item("properties", "Properties", {}),
    item("save-as-tool", "Save as Tool...", { disabled: many }),
    item(s.allLocked ? "unlock" : "lock", s.allLocked ? "Unlock" : "Lock"),
    SEP,
    item("copy", "Copy", { shortcut: "Ctrl+C" }),
    item("cut", "Cut", { shortcut: "Ctrl+X", disabled: edit }),
    item("duplicate", "Duplicate", { shortcut: "Ctrl+D" }),
    SEP,
    item("copy-format", "Copy Format", { shortcut: "Ctrl+Shift+C", disabled: many }),
    item("paste-format", "Paste Format", { shortcut: "Ctrl+Alt+V", disabled: edit || !s.canPasteFormat }),
    item("use-as-default", "Use as Default Appearance", { disabled: many }),
    SEP,
    item("bring-to-front", "Bring to Front", { disabled: edit }),
    item("send-to-back", "Send to Back", { disabled: edit }),
    item("hide", "Hide"),
    SEP,
    item("delete", "Delete", { shortcut: "Del", disabled: edit, danger: true }),
  ];
}

export interface CanvasMenuState {
  /** At least one markup is hidden: Show All Markups is offered enabled. */
  hasHidden?: boolean;
  canPaste: boolean;
  hasMarkups: boolean;
  canBack: boolean;
  canForward: boolean;
}

export function canvasMenuItems(s: CanvasMenuState): MenuItem[] {
  return [
    item("paste", "Paste", { shortcut: "Ctrl+V", disabled: !s.canPaste }),
    item("select-all", "Select All on Page", { disabled: !s.hasMarkups }),
    item("hide-all", "Hide All Markups", { disabled: !s.hasMarkups }),
    item("show-all", "Show All Markups", { disabled: !s.hasHidden }),
    SEP,
    item("fit-width", "Fit Width", { shortcut: "Ctrl+0" }),
    item("fit-height", "Fit Height", { shortcut: "Ctrl+9" }),
    item("actual-size", "Actual Size", { shortcut: "Ctrl+8" }),
    SEP,
    item("prev-view", "Previous View", { shortcut: "Alt+Left", disabled: !s.canBack }),
    item("next-view", "Next View", { shortcut: "Alt+Right", disabled: !s.canForward }),
    SEP,
    item("rotate-page-cw", "Rotate Page Clockwise"),
    item("rotate-page-ccw", "Rotate Page Counterclockwise"),
  ];
}

export interface ThumbnailMenuState {
  /** 0-based page the menu was opened on. */
  page: number;
  /** Pages the page-wide actions (Extract, Number) apply to: the selection when the page is in it. */
  targetCount: number;
  canDelete: boolean;
  canRename: boolean;
  canExtract: boolean;
  canNumber: boolean;
}

export function thumbnailMenuItems(s: ThumbnailMenuState): MenuItem[] {
  const pages = s.targetCount > 1 ? `${s.targetCount} Pages` : "Page";
  return [
    item("go-to-page", `Go to Page ${s.page + 1}`),
    SEP,
    item("rotate-cw", "Rotate Clockwise"),
    item("rotate-ccw", "Rotate Counterclockwise"),
    SEP,
    item("extract", `Extract ${pages}...`, { disabled: !s.canExtract }),
    item("number-pages", "Number Pages...", { disabled: !s.canNumber }),
    item("rename-label", "Rename Page Label", { shortcut: "F2", disabled: !s.canRename }),
    SEP,
    item("delete-page", "Delete Page", { disabled: !s.canDelete, danger: true }),
  ];
}

/** Index of the next enabled action in `dir` from `from` (wrapping), or -1 when none is enabled. */
export function nextEnabled(items: readonly MenuItem[], from: number, dir: 1 | -1): number {
  const n = items.length;
  if (n === 0) return -1;
  for (let step = 1; step <= n; step++) {
    const i = (((from + dir * step) % n) + n) % n;
    const it = items[i];
    if (it.kind === "item" && !it.disabled) return i;
  }
  return -1;
}

export function firstEnabled(items: readonly MenuItem[]): number {
  return nextEnabled(items, -1, 1);
}

export function lastEnabled(items: readonly MenuItem[]): number {
  return nextEnabled(items, items.length, -1);
}

/**
 * Top-left for a menu of `w` x `h` opened at (x, y): flipped to the left / above the point
 * when it would overflow, then clamped so it is always fully inside the window.
 */
export function clampMenuPosition(
  x: number, y: number, w: number, h: number, vw: number, vh: number, margin = 4,
): { left: number; top: number } {
  let left = x;
  let top = y;
  if (left + w + margin > vw) left = x - w;
  if (top + h + margin > vh) top = y - h;
  left = Math.max(margin, Math.min(left, vw - w - margin));
  top = Math.max(margin, Math.min(top, vh - h - margin));
  return { left, top };
}
