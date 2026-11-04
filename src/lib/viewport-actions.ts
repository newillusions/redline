/**
 * Measurement viewport changes as undoable steps (docs/measurement-viewports.md). Each
 * create / edit / redraw / delete is ONE undo frame holding the viewport change and the
 * measurement updates it implies, so undo restores both together - no measurement is ever
 * left pointing at a scale that is gone, and nothing is recalculated behind the user's back.
 */
import { takeScaleNotices, type ScaleRecord, type UserRef } from "./ipc";
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

/**
 * Show the document's queued scale notices (e.g. viewports that could not be saved next to
 * the file, or could not follow a page operation) in the error banner. Call after any scale
 * operation or page operation: the backend queues them, nothing else drains them.
 */
export async function drainScaleNotices(docId: string): Promise<void> {
  try {
    (await takeScaleNotices(docId)).forEach((n) => surfaceError(n));
  } catch {
    // A failed read of the notices must not turn a finished operation into an error.
  }
}

/** What a rectangle drag with the viewport tool means. */
export type ViewportDragOutcome =
  | { kind: "new" }
  | { kind: "redraw"; old: ScaleRecord }
  | { kind: "vanished" };

/**
 * A "viewportRedraw" drag replaces the rectangle of the viewport being redrawn. If that
 * viewport is gone by the time the drag ends (deleted, undone, its page removed) the drag
 * must say so - never fall through to creating a new viewport, which opens a dialog the
 * user did not ask for.
 */
export function viewportDragOutcome(
  tool: string,
  redrawViewportId: string | null,
  scales: ScaleRecord[],
): ViewportDragOutcome {
  if (tool !== "viewportRedraw") return { kind: "new" };
  const old = redrawViewportId
    ? scales.find((x) => x.id === redrawViewportId && !x.retired && x.applies_to.kind === "Viewport")
    : undefined;
  return old ? { kind: "redraw", old } : { kind: "vanished" };
}

export const VIEWPORT_VANISHED_MESSAGE =
  "The viewport you were redrawing no longer exists (it was deleted or undone), so nothing was changed. Draw a new viewport instead.";

/** Show a failed page rotation in the error banner (never only the console). */
export function reportRotateError(pageIndex: number, e: unknown): void {
  surfaceError(`Could not rotate page ${pageIndex + 1}: ${e instanceof Error ? e.message : String(e)}`);
}
