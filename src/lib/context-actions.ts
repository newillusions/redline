/**
 * What the right-click menu commands do to the markup store (`context-menu.ts` decides what is
 * offered). Everything routes to a command that already exists: `arrange-actions` for copy /
 * cut / duplicate / paste / order, `patchStatus` for review status. Nothing here touches the DOM.
 */
import { isMarkupContentsLocked, isMarkupLocked, type MarkupStatus } from "./ipc";
import {
  copySelection, cutSelection, duplicateSelection, orderSelection, pasteClipboard,
  type ActionResult, type ArrangeCtx,
} from "./arrange-actions";
import { patchStatus } from "./markup-properties";
import { statusFromId } from "./context-menu";

/** Set `status` on the selected markups as ONE undo step; locked ones are skipped and counted. */
export function setSelectionStatus(c: ArrangeCtx, status: MarkupStatus): ActionResult {
  if (!c.identity) return { ok: false, message: "The user identity is not loaded yet." };
  const targets = c.store.selectedMarkups;
  if (targets.length === 0) return { ok: false, message: "Select a markup first." };
  const now = (c.now ?? (() => new Date().toISOString()))();
  const locked = targets.filter((m) => isMarkupLocked(m) || isMarkupContentsLocked(m)).length;
  c.store.applyBatch(targets.map((m) => ({ before: m, after: patchStatus(m, status, c.identity!, now) })));
  const n = targets.length - locked;
  const base = `Status ${status} on ${n} markup${n === 1 ? "" : "s"}`;
  return { ok: n > 0, message: locked > 0 ? `${base}; ${locked} locked skipped` : base };
}

/** Select every markup shown on the current page (Select tool must already be active). */
export function selectAllOnPage(c: ArrangeCtx, shownIds: readonly string[]): ActionResult {
  if (shownIds.length === 0) return { ok: false, message: "No markups on this page." };
  c.store.selectedIds = new Set(shownIds);
  return { ok: true, message: `Selected ${shownIds.length} markup${shownIds.length === 1 ? "" : "s"}` };
}

/**
 * Run the store-level menu commands. Returns null for ids the caller handles itself
 * (Properties, Save as Tool, delete, zoom presets, view history, select-all).
 */
export function runMarkupCommand(id: string, c: ArrangeCtx): ActionResult | null {
  const status = statusFromId(id);
  if (status) return setSelectionStatus(c, status);
  switch (id) {
    case "copy": return copySelection(c);
    case "cut": return cutSelection(c);
    case "duplicate": return duplicateSelection(c);
    case "bring-to-front": return orderSelection(c, "front");
    case "send-to-back": return orderSelection(c, "back");
    case "paste": return pasteClipboard(c, "cursor");
    default: return null;
  }
}
