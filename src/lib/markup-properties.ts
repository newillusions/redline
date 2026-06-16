/**
 * Pure property-patch and indeterminate-value helpers for the PropertiesPanel.
 * No DOM, no Svelte, no side effects. The panel component stays thin by delegating
 * all mutation logic here.
 */
import type { Markup, Appearance, UserRef } from "./ipc";
import { bumpAudit } from "./markup-tools";

/** Base-14 font families offered by the picker (reliable cross-viewer /DA rendering). */
export const FONT_FAMILIES: readonly string[] = ["Helvetica", "Times", "Courier"];

/** Font sizes (pt) offered by the picker. */
export const FONT_SIZES: readonly number[] = [8, 9, 10, 11, 12, 14, 18, 24, 36];

/**
 * Return a clone of `m` with a shallow appearance patch applied and the audit
 * trail bumped. The font field can be set via `patch = { font: {family, size_pt} }`
 * or cleared with `patch = { font: null }`. No mutation of the input.
 */
export function patchAppearance(
  m: Markup,
  patch: Partial<Appearance>,
  by: UserRef,
  now: string,
): Markup {
  return bumpAudit(
    { ...m, appearance: { ...m.appearance, ...patch } },
    by,
    now,
  );
}

/**
 * Return a clone of `m` with only the supplied text fields applied and the audit
 * trail bumped. Keys absent from `patch` are left unchanged on the clone. An
 * explicit `null` value in `patch` does set the corresponding field to null.
 * No mutation of the input.
 */
export function patchFields(
  m: Markup,
  patch: { contents?: string | null; subject?: string | null; layer?: string | null },
  by: UserRef,
  now: string,
): Markup {
  const updated: Markup = { ...m };
  if ("contents" in patch) updated.contents = patch.contents ?? null;
  if ("subject" in patch) updated.subject = patch.subject ?? null;
  if ("layer" in patch) updated.layer = patch.layer ?? null;
  return bumpAudit(updated, by, now);
}

/**
 * Return the shared projected value across all `markups` when every element
 * projects the same value under strict equality (===), or `undefined` when the
 * list is empty or the values differ.
 *
 * Use primitive projections (color strings, numbers, family strings) for
 * reliable equality; object projections will always return `undefined` unless
 * they share the same reference.
 */
export function commonValue<T>(markups: Markup[], get: (m: Markup) => T): T | undefined {
  if (markups.length === 0) return undefined;
  const first = get(markups[0]);
  for (let i = 1; i < markups.length; i++) {
    if (get(markups[i]) !== first) return undefined;
  }
  return first;
}
