/**
 * Format painter and "use as default" (Revu: Format Painter Ctrl+Shift+C; Acrobat: Make
 * Properties Default). Copy the look of one markup, paste it onto others, or make it the
 * look of everything drawn next. Only the look changes: geometry, text, status and the rest
 * are untouched, and each paste is ONE undo step. Pure apart from the held format.
 */
import { isMarkupContentsLocked, isMarkupLocked, type Appearance } from "./ipc";
import type { ActionResult, ArrangeCtx } from "./arrange-actions";
import { patchAppearance } from "./markup-properties";

/** The format held between a copy and a paste (one for the whole app). */
export const formatClipboard: { format: Appearance | null } = { format: null };

function cloneAppearance(a: Appearance): Appearance {
  return { ...a, font: a.font ? { ...a.font } : a.font };
}

/** Copy the look of the one selected markup. */
export function copyFormat(c: ArrangeCtx): ActionResult {
  const sel = c.store.selectedMarkups;
  if (sel.length === 0) return { ok: false, message: "Select a markup to copy its format." };
  if (sel.length > 1) return { ok: false, message: "Select one markup to copy its format from." };
  formatClipboard.format = cloneAppearance(sel[0].appearance);
  return { ok: true, message: "Format copied. Select markups and paste the format (Ctrl+Alt+V)." };
}

/**
 * Paste the copied look onto the selected markups. The font comes across only to markups that
 * already have one (text-bearing types), so a rectangle never gains a font; locked markups are
 * skipped and counted.
 */
export function pasteFormat(c: ArrangeCtx): ActionResult {
  const held = formatClipboard.format;
  if (!held) return { ok: false, message: "Copy a format first (Ctrl+Shift+C on a markup)." };
  if (!c.identity) return { ok: false, message: "The user identity is not loaded yet." };
  const targets = c.store.selectedMarkups;
  if (targets.length === 0) return { ok: false, message: "Select the markups to paste the format onto." };
  const now = (c.now ?? (() => new Date().toISOString()))();
  const locked = targets.filter((m) => isMarkupLocked(m) || isMarkupContentsLocked(m));
  const open = targets.filter((m) => !locked.includes(m));
  c.store.applyBatch(
    open.map((m) => {
      const { font, ...look } = cloneAppearance(held);
      return { before: m, after: patchAppearance(m, m.appearance.font ? { ...look, font } : look, c.identity!, now) };
    }),
  );
  const base = `Format pasted onto ${open.length} markup${open.length === 1 ? "" : "s"}`;
  return { ok: open.length > 0, message: locked.length > 0 ? `${base}; ${locked.length} locked skipped` : base };
}

/** Make the selected markup's look the look of everything drawn from now on. */
export function useAsDefault(c: ArrangeCtx): ActionResult {
  const sel = c.store.selectedMarkups;
  if (sel.length !== 1) return { ok: false, message: "Select one markup to use its look as the default." };
  c.store.draftAppearance = cloneAppearance(sel[0].appearance);
  return { ok: true, message: "New markups will use this look." };
}
