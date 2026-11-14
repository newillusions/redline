/**
 * Hide / Show markups: a display-only view state, like the layer view (see `layers-store.svelte.ts`).
 * A hidden markup is neither drawn nor hit-tested on the canvas, but it stays in the document, the
 * Markups List, counts, legends and exports, and hiding never touches the markup store, so it makes
 * no undo step and does not dirty the document: the saved PDF bytes are unchanged. The set lives
 * per open document for the session and is not written to the file.
 *
 * The helpers are pure and return a NEW set (the store replaces its `$state`, never mutates).
 */

export type HiddenSet = ReadonlySet<string>;

/** `hidden` plus `ids`. */
export function hideMarkups(hidden: HiddenSet, ids: Iterable<string>): HiddenSet {
  const next = new Set(hidden);
  for (const id of ids) next.add(id);
  return next;
}

/** `hidden` minus `ids`. */
export function showMarkups(hidden: HiddenSet, ids: Iterable<string>): HiddenSet {
  const next = new Set(hidden);
  for (const id of ids) next.delete(id);
  return next;
}

export function isMarkupHidden(hidden: HiddenSet, m: { id?: string }): boolean {
  return m.id !== undefined && hidden.has(m.id);
}

/** Hidden ids that still exist among `liveIds` (a deleted markup drops out of the count). */
export function hiddenCount(hidden: HiddenSet, liveIds: Iterable<string>): number {
  let n = 0;
  for (const id of liveIds) if (hidden.has(id)) n++;
  return n;
}

/** `selected` without the markups in `hidden`: a hidden markup must not stay selected, or Delete,
 *  the arrow keys and Arrange would act on something the user cannot see. */
export function withoutHidden(selected: ReadonlySet<string>, hidden: HiddenSet): Set<string> {
  return new Set([...selected].filter((id) => !hidden.has(id)));
}
