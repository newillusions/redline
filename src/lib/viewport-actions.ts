/**
 * Measurement viewport changes as undoable steps (docs/measurement-viewports.md). Each
 * create / edit / redraw / delete is ONE undo frame holding the viewport change and the
 * measurement updates it implies, so undo restores both together - no measurement is ever
 * left pointing at a scale that is gone, and nothing is recalculated behind the user's back.
 */
import type { ScaleRecord, UserRef } from "./ipc";
import type { MarkupStore } from "./markup-store.svelte";
import type { TakeoffStore } from "./takeoff-store.svelte";
import { recomputeAll } from "./polylength";
import { surfaceError } from "./error-surface";

/** The scale list with `after` put in place of `before` (or appended), as the store will hold it. */
function withChange(scales: ScaleRecord[], before: ScaleRecord | null, after: ScaleRecord | null): ScaleRecord[] {
  const id = (after ?? before)?.id;
  const i = scales.findIndex((s) => s.id === id);
  if (!after) return scales.filter((s) => s.id !== id);
  if (i < 0) return [...scales, after];
  const out = scales.slice();
  out[i] = after;
  return out;
}

/**
 * Record a viewport change (already persisted by the command that returned `after`) as one
 * undo frame. Measurements are recomputed under the changed scales (the recompute rule
 * leaves measurements taken under another scale alone); a delete (`after.retired`) changes
 * no measurement - they keep the values they were taken with.
 */
export function commitViewportChange(
  store: MarkupStore,
  takeoff: TakeoffStore,
  before: ScaleRecord | null,
  after: ScaleRecord | null,
  identity: UserRef | null,
): void {
  // One tab = one MarkupStore + one TakeoffStore: replays (undo/redo) land on this tab's scales.
  store.onScale ??= (id, rec) => takeoff.putScale(id, rec);
  const deleting = !after || after.retired === true;
  const pairs = !deleting && identity
    ? recomputeAll(store.markups, withChange(takeoff.scales, before, after), identity, new Date().toISOString())
    : [];
  store.applyScaleChange(before, after, pairs);
}

/** Show a failed viewport action in the error banner (never only the console). */
export function reportViewportError(action: string, e: unknown): void {
  surfaceError(`${action} failed: ${e instanceof Error ? e.message : String(e)}`);
}
