/**
 * Pure keyboard-shortcut resolution, extracted from App.svelte's `handleKeydown` so it's
 * unit-testable without mounting the whole app (App.svelte has no test file of its own -
 * it mounts the license gate, Tauri IPC, and tab store on load, which makes it expensive
 * to render in vitest; the rest of this repo's conflict-avoidance/testability convention
 * is to pull logic like this into a plain `$lib` module - see `recent-docs.ts`, `license.ts`,
 * and `markup-properties.ts`'s `patchStatus`).
 */

/** True when the keydown target is a text-input surface that owns its own native undo
 *  stack (the Text/Callout inline `<textarea>` in Viewport.svelte, any `<input>`, or a
 *  contentEditable element) - app-level Undo/Redo must NOT intercept Ctrl/Cmd+Z there,
 *  or it would both fight the browser's native field-level undo and swallow the keypress
 *  before the field ever sees it. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!target || !(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA") return true;
  // `.isContentEditable` isn't reliably implemented in jsdom (returns `undefined` rather
  // than `false`), so fall back to the attribute itself rather than trust the property -
  // and compare with `===` throughout so this always returns an actual boolean, never an
  // `||`-chain's leftover falsy operand.
  return target.isContentEditable === true || target.getAttribute("contenteditable") === "true";
}

export type UndoRedoAction = "undo" | "redo" | null;

/** Minimal shape App.svelte's real `KeyboardEvent` satisfies - kept narrow so tests don't
 *  need to construct a full DOM KeyboardEvent. */
export interface UndoRedoKeyEvent {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  target?: EventTarget | null;
}

/** Resolves Ctrl/Cmd+Z -> undo, Ctrl/Cmd+Shift+Z and Ctrl/Cmd+Y -> redo. Returns null when
 *  no modifier is held, the key doesn't match, or the target is an editable surface that
 *  should keep its own native undo behaviour (see `isEditableTarget`). */
export function resolveUndoRedoShortcut(e: UndoRedoKeyEvent): UndoRedoAction {
  const mod = e.metaKey || e.ctrlKey;
  if (!mod) return null;
  if (isEditableTarget(e.target ?? null)) return null;

  const key = e.key.toLowerCase();
  if (key === "z") return e.shiftKey ? "redo" : "undo";
  if (key === "y") return "redo";
  return null;
}
