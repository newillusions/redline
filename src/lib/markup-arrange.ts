/**
 * Align, distribute, order (z-order) and Multiply geometry (docs/arrange.md). Pure: bounds in,
 * moves / id orders / offsets out. No DOM, no Svelte, no clocks.
 *
 * A group is one unit for align, distribute and order ("behave like a single markup", S8 in the
 * spec): its members share a bounding box and move by the same delta.
 */
import { isMarkupContentsLocked, isMarkupLocked, type Markup, type ScaleRecord, type UserRef } from "./ipc";
import { translateMarkup } from "./callout-edit";
import { boundsOf, type Bounds } from "./markup-select";
import { bumpAudit } from "./markup-tools";
import { MAX_COPY_ITEMS, unionBounds } from "./markup-copy";

const EPS = 1e-6;
const isLocked = (m: Markup) => isMarkupLocked(m) || isMarkupContentsLocked(m);

// --- units -----------------------------------------------------------------------------

/** One thing to align or distribute: a lone markup or a whole group. */
export interface Unit {
  members: Markup[];
  bounds: Bounds;
  /** Any member is locked: the unit is never moved. */
  locked: boolean;
}

/**
 * The units of a selection, in the order the user selected them (the first is the primary).
 * `selected` is in selection order; a group is completed from `all` so a partially selected
 * group still moves whole.
 */
export function buildUnits(selected: readonly Markup[], all: readonly Markup[]): Unit[] {
  const units: Unit[] = [];
  const seen = new Set<string>();
  for (const m of selected) {
    const key = m.group_id ?? m.id;
    if (seen.has(key)) continue;
    seen.add(key);
    const members = m.group_id ? all.filter((x) => x.group_id === m.group_id) : [m];
    const bounds = unionBounds(members) ?? boundsOf(m);
    units.push({ members, bounds, locked: members.some(isLocked) });
  }
  return units;
}

export interface Move {
  unit: Unit;
  dx: number;
  dy: number;
}

const centreX = (b: Bounds) => (b.minX + b.maxX) / 2;
const centreY = (b: Bounds) => (b.minY + b.maxY) / 2;

// --- align -----------------------------------------------------------------------------

export type AlignMode = "left" | "center" | "right" | "top" | "middle" | "bottom";

/**
 * Moves that line every unit up with the primary (`units[0]`). Left / Right / Top / Bottom match
 * the edge; **Center** matches the horizontal centre (x), **Middle** the vertical middle (y)
 * (spec A3). y is up: Top is the larger y. Units already in line produce no move.
 */
export function alignMoves(units: readonly Unit[], mode: AlignMode): Move[] {
  if (units.length < 2) return [];
  const ref = units[0].bounds;
  const out: Move[] = [];
  for (const unit of units.slice(1)) {
    const b = unit.bounds;
    let dx = 0;
    let dy = 0;
    switch (mode) {
      case "left": dx = ref.minX - b.minX; break;
      case "right": dx = ref.maxX - b.maxX; break;
      case "center": dx = centreX(ref) - centreX(b); break;
      case "top": dy = ref.maxY - b.maxY; break;
      case "bottom": dy = ref.minY - b.minY; break;
      case "middle": dy = centreY(ref) - centreY(b); break;
    }
    if (Math.abs(dx) > EPS || Math.abs(dy) > EPS) out.push({ unit, dx, dy });
  }
  return out;
}

// --- distribute ------------------------------------------------------------------------

export type DistributeAxis = "horizontal" | "vertical";

/**
 * Moves that make the gaps between the CENTRES of three or more units equal along `axis`. The
 * two outermost units stay; the ones between them are spaced evenly.
 */
export function distributeMoves(units: readonly Unit[], axis: DistributeAxis): Move[] {
  if (units.length < 3) return [];
  const centre = axis === "horizontal" ? centreX : centreY;
  const sorted = [...units].sort((a, b) => centre(a.bounds) - centre(b.bounds));
  const first = centre(sorted[0].bounds);
  const last = centre(sorted[sorted.length - 1].bounds);
  const step = (last - first) / (sorted.length - 1);
  const out: Move[] = [];
  for (let i = 1; i < sorted.length - 1; i++) {
    const d = first + step * i - centre(sorted[i].bounds);
    if (Math.abs(d) > EPS) out.push({ unit: sorted[i], dx: axis === "horizontal" ? d : 0, dy: axis === "vertical" ? d : 0 });
  }
  return out;
}

/**
 * The before/after pairs for `moves` (a moved unit's members all shift by the same delta, audit
 * advanced), plus how many units were left alone because a member is locked.
 */
export function applyMoves(moves: readonly Move[], by: UserRef, now: string): {
  pairs: { before: Markup; after: Markup }[];
  lockedSkipped: number;
} {
  const pairs: { before: Markup; after: Markup }[] = [];
  let lockedSkipped = 0;
  for (const mv of moves) {
    if (mv.unit.locked) { lockedSkipped++; continue; }
    for (const m of mv.unit.members) pairs.push({ before: m, after: bumpAudit(translateMarkup(m, mv.dx, mv.dy), by, now) });
  }
  return { pairs, lockedSkipped };
}

// --- order (z-order) -------------------------------------------------------------------

export type OrderOp = "front" | "back" | "forward" | "backward";

function overlaps(a: Markup, b: Markup): boolean {
  const p = boundsOf(a);
  const q = boundsOf(b);
  return p.minX <= q.maxX + EPS && q.minX <= p.maxX + EPS && p.minY <= q.maxY + EPS && q.minY <= p.maxY + EPS;
}

/**
 * New relative order of one page's markups (`list`, bottom first) after `op` on the markups in
 * `sel`. Front / Back move the selection to the top / bottom keeping its own order. Forward /
 * Backward move each selected markup past the next markup in that direction that overlaps it
 * (spec A6), the topmost (bottommost) selected first so they never pass each other.
 */
export function orderPage(list: readonly Markup[], sel: ReadonlySet<string>, op: OrderOp): string[] {
  if (op === "front") return [...list.filter((m) => !sel.has(m.id)), ...list.filter((m) => sel.has(m.id))].map((m) => m.id);
  if (op === "back") return [...list.filter((m) => sel.has(m.id)), ...list.filter((m) => !sel.has(m.id))].map((m) => m.id);
  const arr = list.slice();
  if (op === "forward") {
    for (let i = arr.length - 1; i >= 0; i--) {
      if (!sel.has(arr[i].id)) continue;
      let j = i + 1;
      while (j < arr.length && (sel.has(arr[j].id) || !overlaps(arr[i], arr[j]))) j++;
      if (j >= arr.length) continue;
      const [m] = arr.splice(i, 1);
      arr.splice(j, 0, m);
    }
  } else {
    for (let i = 0; i < arr.length; i++) {
      if (!sel.has(arr[i].id)) continue;
      let j = i - 1;
      while (j >= 0 && (sel.has(arr[j].id) || !overlaps(arr[i], arr[j]))) j--;
      if (j < 0) continue;
      const [m] = arr.splice(i, 1);
      arr.splice(j, 0, m);
    }
  }
  return arr.map((m) => m.id);
}

export interface OrderChange {
  page: number;
  before: string[];
  after: string[];
}

/**
 * Z-order changes for `op` on the markups in `selectedIds`, one per page that actually changes.
 * Locked markups are not moved (they keep their place and stay in the way); the count of those
 * left alone is returned too.
 */
export function orderChanges(
  all: readonly Markup[],
  selectedIds: ReadonlySet<string>,
  op: OrderOp,
): { changes: OrderChange[]; lockedSkipped: number } {
  const byId = new Map(all.map((m) => [m.id, m]));
  const movable = new Set<string>();
  let lockedSkipped = 0;
  for (const id of selectedIds) {
    const m = byId.get(id);
    if (!m) continue;
    if (isLocked(m)) lockedSkipped++;
    else movable.add(id);
  }
  const pages = new Set<number>();
  for (const id of movable) pages.add(byId.get(id)!.page);
  const changes: OrderChange[] = [];
  for (const page of [...pages].sort((a, b) => a - b)) {
    const list = all.filter((m) => m.page === page);
    const before = list.map((m) => m.id);
    const after = orderPage(list, movable, op);
    if (after.some((id, i) => id !== before[i])) changes.push({ page, before, after });
  }
  return { changes, lockedSkipped };
}

// --- multiply --------------------------------------------------------------------------

export type MultiplyDirection = "right" | "left" | "up" | "down";

export type MultiplySpec =
  /** `count` copies in one of the four cardinal directions, `pitch` PDF pt apart (Revu). */
  | { kind: "line"; direction: MultiplyDirection; count: number; pitch: number }
  /** `cols` x `rows` cells counting the original at the top-left, growing right and down; the
   *  original's cell is not copied. */
  | { kind: "grid"; cols: number; rows: number; pitchX: number; pitchY: number };

export interface Offset {
  dx: number;
  dy: number;
}

export interface MultiplyOffsets {
  offsets: Offset[];
  /** Copies asked for. */
  requested: number;
  /** Of those, left out because they would not fit on the page. */
  clipped: number;
  error?: string;
}

const okCount = (n: unknown): n is number => typeof n === "number" && Number.isInteger(n) && n >= 1;
const okPitch = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n) && n > 0;

/**
 * Offsets of every copy of a unit whose bounding box is `unit`, with copies that would leave
 * the page (`pageW` x `pageH`, PDF pt, origin bottom-left) left out (spec M3). A line stops at the
 * first copy that does not fit. Asking for more than `maxCopies` is refused up front, before
 * anything is generated.
 */
export function multiplyOffsets(
  spec: MultiplySpec,
  unit: Bounds,
  page: { w: number; h: number },
  maxCopies: number = MAX_COPY_ITEMS,
): MultiplyOffsets {
  const bad = (error: string): MultiplyOffsets => ({ offsets: [], requested: 0, clipped: 0, error });
  let cells: Offset[];
  let requested: number;
  if (spec.kind === "line") {
    if (!okCount(spec.count) || !okPitch(spec.pitch)) return bad("Enter a whole number of copies and a distance above zero.");
    if (spec.count > maxCopies) return bad(`At most ${maxCopies} copies at a time.`);
    requested = spec.count;
    const [sx, sy] = spec.direction === "right" ? [1, 0] : spec.direction === "left" ? [-1, 0] : spec.direction === "up" ? [0, 1] : [0, -1];
    cells = Array.from({ length: requested }, (_, k) => ({ dx: sx * spec.pitch * (k + 1), dy: sy * spec.pitch * (k + 1) }));
  } else {
    if (!okCount(spec.cols) || !okCount(spec.rows) || !okPitch(spec.pitchX) || !okPitch(spec.pitchY)) {
      return bad("Enter whole numbers of columns and rows and distances above zero.");
    }
    requested = spec.cols * spec.rows - 1;
    if (requested < 1) return bad("A grid of one cell has nothing to copy.");
    if (requested > maxCopies) return bad(`At most ${maxCopies} copies at a time.`);
    cells = [];
    for (let j = 0; j < spec.rows; j++) {
      for (let i = 0; i < spec.cols; i++) {
        if (i === 0 && j === 0) continue;
        cells.push({ dx: spec.pitchX * i, dy: j === 0 ? 0 : -spec.pitchY * j });
      }
    }
  }
  const fits = (o: Offset) =>
    unit.minX + o.dx >= -EPS && unit.minY + o.dy >= -EPS && unit.maxX + o.dx <= page.w + EPS && unit.maxY + o.dy <= page.h + EPS;
  let offsets: Offset[];
  if (spec.kind === "line") {
    const stop = cells.findIndex((c) => !fits(c));
    offsets = stop < 0 ? cells : cells.slice(0, stop);
  } else {
    offsets = cells.filter(fits);
  }
  return { offsets, requested, clipped: requested - offsets.length };
}

/**
 * A distance typed in `unit` as PDF points: "pt" as it is, or the page scale's own unit divided
 * by its ratio (real-world units per point). Null when the unit is unknown or the value unusable.
 */
export function spacingToPoints(value: number, unit: string, scale: ScaleRecord | null): number | null {
  if (!Number.isFinite(value) || value <= 0) return null;
  if (unit === "pt") return value;
  if (scale && unit === scale.unit && Number.isFinite(scale.ratio) && scale.ratio > 0) return value / scale.ratio;
  return null;
}
