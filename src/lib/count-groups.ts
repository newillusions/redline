/**
 * Count groups (docs/count-polylength.md): pure operations on MeasurementCount items.
 *
 * A count is the set of items sharing `count_group`; an item with no group is a count of
 * one (its own id is its key). Resume / Split / Merge / Label+Colour return before/after
 * pairs for `MarkupStore.applyBatch` (ONE undo step). No DOM, no Svelte, no clocks beyond
 * the injected `now`.
 */
import type { Markup, UserRef } from "./ipc";
import { bumpAudit } from "./markup-tools";

/** Most items one count may hold (mirrors Rust `MAX_COUNT_GROUP_ITEMS`). */
export const MAX_COUNT_GROUP_ITEMS = 10_000;

export interface CountGroup {
  /** `count_group` of the members, or the lone item's id when ungrouped. */
  key: string;
  ids: string[];
  page: number;
  setId: string | null;
  setName: string;
  label: string;
  color: string;
  total: number;
}

export interface Pair {
  before: Markup;
  after: Markup;
}

/** The in-progress placement run: where the next click's item should be grouped. */
export interface CountRun {
  groupId: string;
  page: number;
  setId: string | null;
  size: number;
  /** Label and colour a resumed count carries on to new items. */
  subject?: string | null;
  color?: string;
}

const isCount = (m: Markup) => m.markup_type === "MeasurementCount";

export function groupKey(m: Markup): string {
  return m.count_group ?? m.id;
}

/** Item value (always 1 from the tool, but a foreign/edited value is honoured). */
function itemValue(m: Markup): number {
  return m.measurement?.count_value ?? 1;
}

/** One entry per count (group), in first-seen order. */
export function countGroups(markups: Markup[]): CountGroup[] {
  const groups = new Map<string, CountGroup>();
  for (const m of markups) {
    if (!isCount(m)) continue;
    const key = groupKey(m);
    let g = groups.get(key);
    if (!g) {
      g = {
        key,
        ids: [],
        page: m.page,
        setId: m.count_set?.id ?? null,
        setName: m.count_set?.name ?? "Unassigned",
        label: m.subject ?? m.count_set?.name ?? "Count",
        color: m.appearance.color,
        total: 0,
      };
      groups.set(key, g);
    }
    g.ids.push(m.id);
    g.total += itemValue(m);
  }
  return [...groups.values()];
}

/**
 * The run a new click belongs to. Continues `run` while the page and set match and the
 * count has room; otherwise starts a fresh count. `mintId` supplies the new group id.
 */
export function nextRun(
  run: CountRun | null,
  page: number,
  setId: string | null,
  mintId: () => string,
): CountRun {
  if (run && run.page === page && run.setId === setId && run.size < MAX_COUNT_GROUP_ITEMS) {
    return { ...run, size: run.size + 1 };
  }
  return { groupId: mintId(), page, setId, size: 1 };
}

export type OpResult = { ok: true; pairs: Pair[]; run?: CountRun } | { ok: false; error: string };

/**
 * Resume: continue adding to the count that `itemId` belongs to. A legacy ungrouped item is
 * its own count; resuming it STAMPS its id as `count_group` on that item (returned in
 * `pairs`, apply with `applyBatch`) so every view agrees it is one row with one total.
 * `by` defaults to the item's last modifier (a mechanical regroup, not a new author).
 */
export function resumeRun(
  markups: Markup[],
  itemId: string,
  by?: UserRef,
  now: string = new Date().toISOString(),
): { run: CountRun; pairs: Pair[] } | { error: string } {
  const m = markups.find((x) => x.id === itemId);
  if (!m || !isCount(m)) return { error: "Select a count item to resume." };
  const g = countGroups(markups).find((c) => c.key === groupKey(m));
  if (!g) return { error: "Count not found." };
  if (g.ids.length >= MAX_COUNT_GROUP_ITEMS) {
    return { error: `This count is full (${MAX_COUNT_GROUP_ITEMS} items).` };
  }
  const pairs: Pair[] = m.count_group
    ? []
    : [{ before: m, after: retarget(m, { count_group: g.key }, by ?? m.audit.modified_by, now) }];
  return {
    run: { groupId: g.key, page: g.page, setId: g.setId, size: g.ids.length, subject: m.subject, color: m.appearance.color },
    pairs,
  };
}

function retarget(m: Markup, patch: Partial<Markup>, by: UserRef, now: string): Markup {
  return bumpAudit({ ...m, ...patch }, by, now);
}

/** Split: the selected items become a new count; the rest keep theirs. */
export function splitSelected(
  markups: Markup[],
  selectedIds: Iterable<string>,
  by: UserRef,
  now: string,
  mintId: () => string,
): OpResult {
  const sel = new Set(selectedIds);
  const items = markups.filter((m) => isCount(m) && sel.has(m.id));
  if (items.length === 0) return { ok: false, error: "Select the count items to split off." };
  const keys = new Set(items.map(groupKey));
  const all = countGroups(markups);
  const partial = [...keys].some((k) => {
    const g = all.find((c) => c.key === k);
    return g !== undefined && items.filter((i) => groupKey(i) === k).length < g.ids.length;
  });
  if (!partial && keys.size === 1) {
    return { ok: false, error: "The selection is the whole count; select only some of its items." };
  }
  const groupId = mintId();
  return {
    ok: true,
    pairs: items.map((m) => ({ before: m, after: retarget(m, { count_group: groupId }, by, now) })),
  };
}

/** Split All: every selected item becomes its own count. */
export function splitAll(
  markups: Markup[],
  selectedIds: Iterable<string>,
  by: UserRef,
  now: string,
  mintId: () => string,
): OpResult {
  const sel = new Set(selectedIds);
  const items = markups.filter((m) => isCount(m) && sel.has(m.id));
  if (items.length === 0) return { ok: false, error: "Select the count items to split." };
  // Whole selected counts of one item are already split; skip them.
  const sizes = new Map(countGroups(markups).map((g) => [g.key, g.ids.length]));
  const movable = items.filter((m) => (sizes.get(groupKey(m)) ?? 1) > 1);
  if (movable.length === 0) return { ok: false, error: "Every selected item is already its own count." };
  // The first item of each source count stays; the rest each get their own count.
  const firstOf = new Map<string, string>();
  const pairs: Pair[] = [];
  for (const m of movable) {
    const k = groupKey(m);
    if (!firstOf.has(k) && items.filter((i) => groupKey(i) === k).length === sizes.get(k)) {
      firstOf.set(k, m.id); // whole source count selected: leave one in place
      continue;
    }
    pairs.push({ before: m, after: retarget(m, { count_group: mintId() }, by, now) });
  }
  if (pairs.length === 0) return { ok: false, error: "Nothing to split." };
  return { ok: true, pairs };
}

/**
 * Merge: the whole counts that the selected items belong to become one count. Counts must
 * be on one page and of one set (type); the first selected count's label and colour win.
 */
export function mergeSelected(
  markups: Markup[],
  selectedIds: Iterable<string>,
  by: UserRef,
  now: string,
): OpResult {
  const sel = new Set(selectedIds);
  const picked = markups.filter((m) => isCount(m) && sel.has(m.id));
  const keys: string[] = [];
  for (const m of picked) if (!keys.includes(groupKey(m))) keys.push(groupKey(m));
  if (keys.length < 2) return { ok: false, error: "Select items from two or more counts to merge." };
  const all = countGroups(markups);
  const chosen = keys.map((k) => all.find((g) => g.key === k)!);
  if (new Set(chosen.map((g) => g.page)).size > 1) {
    return { ok: false, error: "Counts on different pages cannot be merged." };
  }
  if (new Set(chosen.map((g) => g.setId)).size > 1) {
    return { ok: false, error: "Only counts of the same type (set) can be merged." };
  }
  const total = chosen.reduce((n, g) => n + g.ids.length, 0);
  if (total > MAX_COUNT_GROUP_ITEMS) {
    return { ok: false, error: `Merged count would exceed ${MAX_COUNT_GROUP_ITEMS} items.` };
  }
  const target = chosen[0];
  const lead = markups.find((m) => m.id === target.ids[0])!;
  const members = new Set(chosen.flatMap((g) => g.ids));
  const pairs = markups
    .filter((m) => members.has(m.id))
    .filter(
      (m) =>
        m.count_group !== target.key ||
        m.subject !== lead.subject ||
        m.appearance.color !== lead.appearance.color,
    )
    .map((m) => ({
      before: m,
      after: retarget(
        m,
        {
          count_group: target.key,
          subject: lead.subject,
          appearance: { ...m.appearance, color: lead.appearance.color },
        },
        by,
        now,
      ),
    }));
  return { ok: true, pairs };
}

/** Set the label (subject) and/or colour of whole counts, given any of their items. */
export function setLabelColour(
  markups: Markup[],
  itemIds: Iterable<string>,
  patch: { label?: string | null; color?: string },
  by: UserRef,
  now: string,
): Pair[] {
  const sel = new Set(itemIds);
  const keys = new Set(markups.filter((m) => isCount(m) && sel.has(m.id)).map(groupKey));
  const out: Pair[] = [];
  for (const m of markups) {
    if (!isCount(m) || !keys.has(groupKey(m))) continue;
    const p: Partial<Markup> = {};
    if (patch.label !== undefined) p.subject = patch.label === "" ? null : patch.label;
    if (patch.color !== undefined) p.appearance = { ...m.appearance, color: patch.color };
    out.push({ before: m, after: retarget(m, p, by, now) });
  }
  return out;
}

/**
 * Selecting any item of a count selects the whole count (the Markups List shows a count as
 * one row). Returns `ids` plus every other item sharing a `count_group` with one of them.
 */
export function expandCountSelection(markups: Markup[], ids: Set<string>): Set<string> {
  const groups = new Set<string>();
  for (const m of markups) if (ids.has(m.id) && isCount(m) && m.count_group) groups.add(m.count_group);
  if (groups.size === 0) return ids;
  const out = new Set(ids);
  for (const m of markups) if (isCount(m) && m.count_group && groups.has(m.count_group)) out.add(m.id);
  return out;
}
