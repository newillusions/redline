/**
 * Copy / paste / duplicate / multiply, pure half (docs/arrange.md): turn source markups into
 * NEW redline-owned markups. No DOM, no Svelte, no clocks or UUIDs beyond the injected `now` /
 * `mintId`, so every rule is unit-testable.
 *
 * A copy is a new markup, not a clone of the old one's identity: new id (the `/NM` follows
 * from it), fresh audit, empty workflow, and none of the reader-owned or foreign fields the
 * source may carry (the backend strips reader-owned fields from every `add_markup` too; this
 * is the first line, not the only one). Measurements are recomputed under the TARGET page's
 * scale, count items join the right count, groups stay groups (new ids).
 *
 * The clipboard content is treated as untrusted: `isUsableMarkup` is a structural guard run on
 * every item before it is copied, so damaged or hostile data is skipped and counted, never a
 * panic or a poisoned document.
 */
import type {
  Appearance, CountSet, Markup, MarkupGeometry, MarkupType, PdfPoint, ScaleRecord, UserRef,
} from "./ipc";
import { translateMarkup } from "./callout-edit";
import { boundsOf, type Bounds } from "./markup-select";
import { effectiveScale, recomputeMeasurement, MAX_POLYLENGTH_VERTICES } from "./polylength";
import { sanitizeLegend } from "./legend";
import { groupKey, MAX_COUNT_GROUP_ITEMS } from "./count-groups";

/** Most markups one paste / duplicate / multiply may create (all pages together). */
export const MAX_COPY_ITEMS = 10_000;
/** Offset (PDF pt) of a Duplicate and of a repeated keyboard paste with no cursor position. */
export const CASCADE_OFFSET_PT = 12;

/** `/F` bits cleared on a copy: Locked (128) and LockedContents (512), ISO 32000-1 table 165. */
const LOCK_FLAGS = 0x80 | 0x200;
const MAX_POINTS = 100_000;
const MAX_TEXT = 1_000_000;

const MARKUP_TYPES: ReadonlySet<string> = new Set<MarkupType>([
  "Text", "Callout", "Cloud", "Rectangle", "Ellipse", "Polygon", "Line", "Polyline", "Arrow",
  "Highlight", "Ink", "Stamp", "StampDynamic", "Underline", "StrikeOut", "Squiggly", "Note",
  "MeasurementLength", "MeasurementPerimeter", "MeasurementArea", "MeasurementVolume",
  "MeasurementCount", "MeasurementAngle", "MeasurementRadius", "Legend",
]);

// --- validation ------------------------------------------------------------------------

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const pt = (v: unknown): v is PdfPoint => isObj(v) && finite(v.x) && finite(v.y);
const optText = (v: unknown): boolean => v === null || v === undefined || (typeof v === "string" && v.length <= MAX_TEXT);

function validGeometry(g: unknown): g is MarkupGeometry {
  if (!isObj(g)) return false;
  const keys = Object.keys(g);
  if (keys.length !== 1) return false;
  if ("Point" in g) return pt(g.Point);
  if ("Rect" in g) return isObj(g.Rect) && pt(g.Rect.min) && pt(g.Rect.max);
  if ("Polyline" in g) return Array.isArray(g.Polyline) && g.Polyline.length <= MAX_POLYLENGTH_VERTICES && g.Polyline.every(pt);
  if ("Ink" in g) {
    if (!Array.isArray(g.Ink)) return false;
    let n = 0;
    for (const s of g.Ink) {
      if (!Array.isArray(s) || !s.every(pt)) return false;
      n += s.length;
      if (n > MAX_POINTS) return false;
    }
    return true;
  }
  if ("Quads" in g) {
    return Array.isArray(g.Quads) && g.Quads.length <= MAX_POINTS / 4
      && g.Quads.every((q) => Array.isArray(q) && q.length === 4 && q.every(pt));
  }
  return false;
}

const inRange = (v: unknown, lo: number, hi: number): boolean => finite(v) && v >= lo && v <= hi;
const optRange = (v: unknown, lo: number, hi: number): boolean => v === null || v === undefined || inRange(v, lo, hi);
const optString = (v: unknown): boolean => v === null || v === undefined || typeof v === "string";

/** Appearance numbers must be finite and sane: JSON turns NaN into null, which the backend then
 *  refuses (review round 1, F4). */
function validAppearance(a: unknown): boolean {
  if (!isObj(a) || typeof a.color !== "string") return false;
  if (!inRange(a.line_weight, 0, 1000) || !inRange(a.opacity, 0, 1)) return false;
  if (!optRange(a.fill_opacity, 0, 1)) return false;
  if (!optString(a.fill) || !optString(a.outline_color)) return false;
  if (a.line_style !== "Solid" && a.line_style !== "Dashed" && a.line_style !== "Dotted") return false;
  const f = a.font;
  if (f !== null && f !== undefined) {
    if (!isObj(f) || typeof f.family !== "string" || !inRange(f.size_pt, 0, 1000)) return false;
  }
  return true;
}

function validCallout(c: unknown): boolean {
  if (c === null || c === undefined) return true;
  if (!isObj(c) || !pt(c.box_min) || !pt(c.box_max) || !Array.isArray(c.extra_leaders)) return false;
  return c.extra_leaders.length <= 16 && c.extra_leaders.every((l) => Array.isArray(l) && l.length <= 8 && l.every(pt));
}

/** Structural guard for a markup arriving from a clipboard (untrusted): every field this module
 *  reads is present and well-typed, every number is finite and every collection is bounded. */
export function isUsableMarkup(v: unknown): v is Markup {
  if (!isObj(v)) return false;
  if (typeof v.id !== "string" || v.id.length === 0 || v.id.length > 64) return false;
  if (typeof v.markup_type !== "string" || !MARKUP_TYPES.has(v.markup_type)) return false;
  if (!Number.isInteger(v.page) || (v.page as number) < 0 || (v.page as number) > 1_000_000) return false;
  if (!validGeometry(v.geometry)) return false;
  if (!validAppearance(v.appearance)) return false;
  if (!validCallout(v.callout)) return false;
  if (v.note !== null && v.note !== undefined && (!isObj(v.note) || typeof v.note.icon !== "string")) return false;
  if (!isObj(v.audit) || !isObj(v.workflow)) return false;
  if (!optText(v.subject) || !optText(v.layer) || !optText(v.contents)) return false;
  if (v.group_id !== null && v.group_id !== undefined && typeof v.group_id !== "string") return false;
  if (v.count_group !== null && v.count_group !== undefined && typeof v.count_group !== "string") return false;
  if (v.annot_flags !== undefined && !finite(v.annot_flags)) return false;
  const meas = v.measurement;
  if (meas !== null && meas !== undefined) {
    if (!isObj(meas) || !finite(meas.raw_measure) || !finite(meas.computed_quantity)) return false;
    if (typeof meas.unit !== "string") return false;
  }
  const cs = v.count_set;
  if (cs !== null && cs !== undefined) {
    if (!isObj(cs) || typeof cs.id !== "string" || typeof cs.name !== "string" || typeof cs.color !== "string" || typeof cs.symbol !== "string") return false;
  }
  return true;
}

// --- bounds ----------------------------------------------------------------------------

/** Union of the bounds of `items`; null when empty. */
export function unionBounds(items: readonly Markup[]): Bounds | null {
  let out: Bounds | null = null;
  for (const m of items) {
    const b = boundsOf(m);
    out = out
      ? { minX: Math.min(out.minX, b.minX), minY: Math.min(out.minY, b.minY), maxX: Math.max(out.maxX, b.maxX), maxY: Math.max(out.maxY, b.maxY) }
      : b;
  }
  return out;
}

// --- one copy --------------------------------------------------------------------------

function clone<T>(v: T): T {
  return v === undefined ? v : (JSON.parse(JSON.stringify(v)) as T);
}

const SCALED_TYPES: ReadonlySet<string> = new Set([
  "MeasurementLength", "MeasurementPerimeter", "MeasurementRadius", "MeasurementArea", "MeasurementVolume",
]);
const UNSCALED_UNIT: Record<string, string> = {
  MeasurementLength: "pt", MeasurementPerimeter: "pt", MeasurementRadius: "pt",
  MeasurementArea: "pt²", MeasurementVolume: "pt³",
};

/**
 * `m` (already on its target page) measured under that page's scale: recomputed with the
 * scale that applies there, or with no scale at all, in the same unscaled shape a measurement
 * drawn on an uncalibrated page has (`scale_ref` null, unit in points, quantity 0). Angle and
 * count are scale-independent and returned as they are.
 */
export function rescaleMeasurement(m: Markup, scales: readonly ScaleRecord[]): Markup {
  if (!m.measurement || !SCALED_TYPES.has(m.markup_type)) return m;
  const scale = effectiveScale([...scales], m.page);
  if (scale) return recomputeMeasurement(m, scale);
  return {
    ...m,
    measurement: { ...m.measurement, scale_ref: null, unit: UNSCALED_UNIT[m.markup_type] ?? "pt", computed_quantity: 0 },
  };
}

export interface CopyOne {
  identity: UserRef;
  now: string;
  id: string;
  page: number;
  dx: number;
  dy: number;
  groupId: string | null;
  countGroup: string | null;
  /** Recompute a measurement under these scales (target page / document differs); omit to leave it. */
  rescaleWith?: readonly ScaleRecord[];
  /** Keep the source's layer name and `/OC` (a copy within the same document). */
  keepLayer: boolean;
}

/** One new, redline-owned markup copied from `src` (see the module comment for what that means). */
export function freshCopy(src: Markup, o: CopyOne): Markup {
  const moved = translateMarkup(clone(src), o.dx, o.dy);
  const m: Markup & Record<string, unknown> = { ...moved };
  m.id = o.id;
  m.page = o.page;
  m.group_id = o.groupId;
  m.count_group = src.markup_type === "MeasurementCount" ? o.countGroup : null;
  m.audit = {
    created_by: o.identity, created_at: o.now, modified_by: o.identity, modified_at: o.now,
    revision: 0, origin: "Desktop",
  };
  m.workflow = { status: "None", assignee: null, thread: [], status_history: [] };
  m.foreign_keys = [];
  delete m.source_nm;
  delete m.source_rect;
  delete m.rich_text;
  delete m.raw_da;
  // Layer membership (docs/arrange.md C16): a copy in the SAME document stays on the source's
  // layer - name and `/OC` reference, so it hides with it. Another document has no such group
  // (the reference points into the source file), so the copy lands on no layer there.
  if (!o.keepLayer) {
    delete m.optional_content;
    m.layer = null;
  }
  if (src.annot_flags !== undefined) m.annot_flags = src.annot_flags & ~LOCK_FLAGS;
  const app: Appearance = m.appearance;
  m.appearance = { ...app, font: app.font ? { ...app.font } : app.font };
  if (m.legend) m.legend = sanitizeLegend(m.legend);
  return o.rescaleWith ? rescaleMeasurement(m, o.rescaleWith) : m;
}

// --- a set of copies -------------------------------------------------------------------

export interface CopyContext {
  identity: UserRef;
  now: string;
  mintId: () => string;
}

export interface CopySetOptions extends CopyContext {
  page: number;
  dx: number;
  dy: number;
  /** Decide the `count_group` of a copied count item (see `newCounts` / Multiply's `joinCounts`). */
  countGroupOf: (src: Markup) => string | null;
  /** Recompute measurements under these scales; omit when the copy sits where the source did. */
  rescaleWith?: readonly ScaleRecord[];
  /** Keep the source's layer (same document); default true. */
  keepLayer?: boolean;
}

/**
 * One copy of the whole of `items`. Groups stay groups under NEW ids (one id per source group);
 * a group left with a single member is dissolved. The order of `items` is kept.
 */
export function copySet(items: readonly Markup[], o: CopySetOptions): Markup[] {
  const groupMap = new Map<string, string>();
  const out = items.map((src) => {
    let groupId: string | null = null;
    if (src.group_id) {
      groupId = groupMap.get(src.group_id) ?? o.mintId();
      groupMap.set(src.group_id, groupId);
    }
    return freshCopy(src, {
      identity: o.identity, now: o.now, id: o.mintId(), page: o.page, dx: o.dx, dy: o.dy,
      groupId, countGroup: o.countGroupOf(src), rescaleWith: o.rescaleWith, keepLayer: o.keepLayer ?? true,
    });
  });
  const members = new Map<string, number>();
  for (const m of out) if (m.group_id) members.set(m.group_id, (members.get(m.group_id) ?? 0) + 1);
  return out.map((m) => (m.group_id && members.get(m.group_id)! < 2 ? { ...m, group_id: null } : m));
}

/**
 * `countGroupOf` for a paste: every source count becomes a NEW count (one new group id per
 * source group), so pasting a counted room again adds a count, it does not grow the old one.
 * A legacy count of one stays ungrouped.
 */
export function newCounts(mintId: () => string): (src: Markup) => string | null {
  const map = new Map<string, string>();
  return (src) => {
    if (!src.count_group) return null;
    const g = map.get(src.count_group) ?? mintId();
    map.set(src.count_group, g);
    return g;
  };
}

/** The count sets used by `ms`, once each. */
export function countSetsOf(ms: readonly Markup[]): CountSet[] {
  const byId = new Map<string, CountSet>();
  for (const m of ms) if (m.count_set && !byId.has(m.count_set.id)) byId.set(m.count_set.id, m.count_set);
  return [...byId.values()];
}

// --- paste -----------------------------------------------------------------------------

export interface PastePlan {
  creates: Markup[];
  countSets: CountSet[];
  /** Clipboard items that failed validation and were left out. */
  skipped: number;
  error?: string;
}

export interface PasteOptions extends CopyContext {
  /** The clipboard's markups (untrusted). */
  items: readonly unknown[];
  sourceDocId: string;
  targetDocId: string;
  /** Pages to paste onto (0-based, validated here against `pageCount`). */
  pages: readonly number[];
  pageCount: number;
  scales: readonly ScaleRecord[];
  dx: number;
  dy: number;
}

/**
 * Plan a paste of `items` onto each of `pages` (same offset on each). Refuses as a whole, with
 * `error`, when a page is out of range or the result would exceed `MAX_COPY_ITEMS`; otherwise
 * invalid items are skipped and counted.
 */
export function planPaste(o: PasteOptions): PastePlan {
  const fail = (error: string): PastePlan => ({ creates: [], countSets: [], skipped: 0, error });
  const valid = o.items.filter(isUsableMarkup) as Markup[];
  const skipped = o.items.length - valid.length;
  if (valid.length === 0) return { creates: [], countSets: [], skipped, error: skipped > 0 ? "The clipboard content is not usable." : "Nothing to paste." };
  if (o.pages.length === 0) return fail("Choose at least one page.");
  if (o.pages.some((p) => !Number.isInteger(p) || p < 0 || p >= o.pageCount)) return fail("A target page is outside the document.");
  if (valid.length * o.pages.length > MAX_COPY_ITEMS) {
    return fail(`That would create ${valid.length * o.pages.length} markups; the limit is ${MAX_COPY_ITEMS}.`);
  }
  const creates: Markup[] = [];
  for (const page of o.pages) {
    // Same document and page as the source: the measurements are already right. Anywhere else
    // they are recomputed under the target page's scale.
    const sameSpot = valid.every((m) => m.page === page) && o.sourceDocId === o.targetDocId;
    creates.push(
      ...copySet(valid, {
        identity: o.identity, now: o.now, mintId: o.mintId, page, dx: o.dx, dy: o.dy,
        countGroupOf: newCounts(o.mintId), rescaleWith: sameSpot ? undefined : o.scales,
        keepLayer: o.sourceDocId === o.targetDocId,
      }),
    );
  }
  return { creates, countSets: countSetsOf(creates), skipped };
}

// --- multiply: count membership ---------------------------------------------------------

/**
 * `countGroupOf` for Multiply: a copy of a count item joins the SOURCE's count (`count_group`,
 * or the source's own id when it was ungrouped, which `groupKey` already treats as that count),
 * so a row of luminaires is one count with the right total. A count that would pass
 * `MAX_COUNT_GROUP_ITEMS` continues in a fresh one. `sizes` is the live size of each count.
 */
export function joinCounts(
  sizes: Map<string, number>,
  mintId: () => string,
): (src: Markup) => string | null {
  const overflow = new Map<string, string>();
  return (src) => {
    if (src.markup_type !== "MeasurementCount") return null;
    const key = overflow.get(groupKey(src)) ?? groupKey(src);
    const size = sizes.get(key) ?? 0;
    if (size >= MAX_COUNT_GROUP_ITEMS) {
      const next = mintId();
      overflow.set(groupKey(src), next);
      sizes.set(next, 1);
      return next;
    }
    sizes.set(key, size + 1);
    return key;
  };
}
