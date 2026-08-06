/**
 * Shared flush -> op -> reseed sequencing for the DocOps toolbar actions (Flatten /
 * Optimize / Apply Redactions, M5 — spec §8), extracted from App.svelte so it's
 * unit-testable without mounting the whole app (App.svelte has no test file of its own -
 * same conflict-avoidance/testability convention as `recent-docs.ts`, `license.ts`, and
 * `keyboard-shortcuts.ts`).
 *
 * Root-cause fix for the live report "flatten and optimise don't seem to do anything":
 * `flatten_document` strips flattened annotations from the PDF's `/Annots` array on the
 * BACKEND, but `MarkupStore` (the frontend's in-session source of truth, per its own doc
 * comment) is a plain Svelte `$state` array that nothing automatically re-syncs after a
 * docops command completes. Without a reseed:
 * - The SVG overlay keeps rendering the flattened markups as live, selectable, editable
 *   elements — visually indistinguishable from "flatten did nothing" (the raster tile
 *   ALSO now shows the same content baked in, so the two layers coincide and nothing
 *   *looks* different, but the markup is still there in the UI and still editable).
 * - A subsequent save would call `write_markups` with the STALE in-memory list, which
 *   writes every markup it still holds as a fresh indirect annotation object — literally
 *   resurrecting the just-flattened annotation. Flatten would not just *look* undone; a
 *   following save would actually undo it.
 * `loadMarkups(docId)` re-parses (or returns the mtime-cached parse of) the file the
 * backend just rewrote, and `store.seed(...)` replaces the in-memory array outright with
 * no undo entry and no mirror write (see `MarkupStore.seed`'s doc comment) — exactly the
 * "the PDF already has these" load path used when a document is first opened.
 */
import type { Markup } from "./ipc";

/** The subset of `MarkupStore` this helper needs. */
export interface DocOpsStore {
  flush(): Promise<void>;
  seed(markups: Markup[]): void;
}

/** The subset of the IPC surface this helper needs. */
export interface DocOpsIpc {
  loadMarkups(docId: string): Promise<Markup[]>;
}

/**
 * Flush pending in-session markup edits, run a docops backend command (`op`), then
 * reseed `store` from what the backend now reports for `docId`. Returns whatever `op`
 * resolved to (e.g. `flattenDocument`'s flattened-count or `optimizeDocument`'s
 * before/after file size) so the caller can report it.
 *
 * Order matters: `flush()` must run BEFORE `op()` so the operation (flatten/optimize/
 * redact) sees the current markup state, not a stale unflushed one — same contract as
 * `apply_page_edit`'s write-markups-before-op ordering on the Rust side. The reseed must
 * run AFTER `op()` succeeds, from the backend's post-op truth, not before (which would
 * just reload the pre-op state) and not unconditionally (a failed op must not be treated
 * as if it had rewritten the file — `loadMarkups` and `seed` are skipped entirely when
 * `op()` rejects, and the rejection propagates to the caller's existing error handling).
 */
export async function runDocOpAndReseed<T>(
  docId: string,
  store: DocOpsStore,
  ipc: DocOpsIpc,
  op: () => Promise<T>,
): Promise<T> {
  await store.flush();
  const result = await op();
  const markups = await ipc.loadMarkups(docId);
  store.seed(markups);
  return result;
}

/** Human-readable byte size (binary units, one decimal place above 1 KiB) for the
 *  Optimize toolbar action's before/after feedback. */
export function formatBytes(bytes: number): string {
  const abs = Math.abs(bytes);
  if (abs < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unitIndex = -1;
  do {
    value /= 1024;
    unitIndex++;
  } while (Math.abs(value) >= 1024 && unitIndex < units.length - 1);
  return `${value.toFixed(1)} ${units[unitIndex]}`;
}
