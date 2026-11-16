// Adapted from redcolumn (Apache-2.0), packages/measure/src/snap.ts, modified
/**
 * Vector snap-target client (spec §5). Fetches + caches a page's snap-target
 * index (Rust `geometry::build_snap_index`: Endpoint, Midpoint, Intersection,
 * ArcCenter, Quadrant - see that module's doc comment) plus its straight segments
 * (for Perpendicular / Nearest, computed here), and finds the nearest target to a PDF-space point
 * within a tolerance, entirely client-side so a drag gesture never pays an IPC
 * round-trip per pointer-move — `Viewport.svelte` prefetches a page's targets once
 * (on page open / snap-eligible tool activation) and looks up against the cached
 * array on every point capture.
 */
import { invoke } from "@tauri-apps/api/core";
import type { PdfPoint } from "./ipc";

/**
 * `Endpoint`, `Midpoint`, `Intersection`, `ArcCenter` and `Quadrant` come from the Rust index;
 * `Perpendicular` and `Nearest` depend on the cursor and are computed here from the page's
 * straight segments (see `findBestSnap`).
 */
export type SnapKind =
  | "Endpoint"
  | "Midpoint"
  | "Intersection"
  | "ArcCenter"
  | "Quadrant"
  | "Perpendicular"
  | "Nearest";

export interface SnapTarget {
  point: PdfPoint;
  kind: SnapKind;
}

/** Set when a page was too dense to index in full (Rust `geometry::SegmentCap`). */
export interface SnapCap {
  maxSegments: number;
  segmentsKept: number;
  subpathsDropped: number;
  subpathsTotal: number;
}

/** Shape of the `get_page_snap_targets` response. */
interface PageSnapTargetsResponse {
  targets: SnapTarget[];
  cap: SnapCap | null;
}

/** A straight page segment `[x1, y1, x2, y2]` in PDF user space. */
export type SnapSegment = readonly [number, number, number, number];

const cache = new Map<string, Promise<SnapTarget[]>>();
/** Cap info per page key, filled when the targets response arrives. */
const capCache = new Map<string, SnapCap>();
const segmentCache = new Map<string, Promise<SnapSegment[]>>();

function cacheKey(docId: string, pageIndex: number): string {
  return `${docId}#${pageIndex}`;
}

/**
 * Fetch (or return the cached) snap-target index for a page. Caches the in-flight
 * promise (not just the resolved value) so concurrent callers for the same page
 * share one IPC call rather than racing duplicate requests.
 */
export function getPageSnapTargets(docId: string, pageIndex: number): Promise<SnapTarget[]> {
  const key = cacheKey(docId, pageIndex);
  const cached = cache.get(key);
  if (cached) return cached;
  // Wrapped in Promise.resolve() rather than calling .then/.catch directly on
  // invoke()'s return value - a test-mocked `invoke` can return `undefined`
  // synchronously (no implementation configured for that call), and Promise.
  // resolve(undefined) is a valid resolved promise while undefined.catch(...)
  // throws. `?? []` guards the same case on the resolved side.
  const req = Promise.resolve(invoke<PageSnapTargetsResponse | SnapTarget[]>("get_page_snap_targets", { docId, pageIndex }))
    .then((result) => {
      if (Array.isArray(result)) return result;
      if (result?.cap) capCache.set(key, result.cap);
      else capCache.delete(key);
      return result?.targets ?? [];
    })
    .catch((err) => {
      // Don't poison the cache with a failed request - a transient error (e.g. the
      // doc closed mid-flight) should let the next attempt retry, not snap-lock the
      // page into permanent failure.
      cache.delete(key);
      throw err;
    });
  cache.set(key, req);
  return req;
}

/** The cap info for a page whose targets have been fetched, or `null` when snapping is
 *  complete on it (or the page has not been fetched). Call after `getPageSnapTargets` resolves. */
export function getPageSnapCap(docId: string, pageIndex: number): SnapCap | null {
  return capCache.get(cacheKey(docId, pageIndex)) ?? null;
}

/**
 * Drop a document's cached snap targets. Call after any docops mutation that can
 * change a page's vector content — flatten/optimize/redact, or page rotate/
 * delete/reorder/insert — so a stale index doesn't outlive the geometry it
 * describes. (The Rust-side cache in `RenderEngine::OpenDoc` is scoped to the
 * open document handle and is naturally dropped on `close_document`; this client
 * cache has no such lifetime tie-in and must be invalidated explicitly.)
 */
export function invalidateSnapCache(docId: string): void {
  const prefix = `${docId}#`;
  for (const key of Array.from(cache.keys())) {
    if (key.startsWith(prefix)) cache.delete(key);
  }
  for (const key of Array.from(capCache.keys())) {
    if (key.startsWith(prefix)) capCache.delete(key);
  }
  for (const key of Array.from(segmentCache.keys())) {
    if (key.startsWith(prefix)) segmentCache.delete(key);
  }
}

/** Fetch (or return the cached) straight segments of a page, for perpendicular and
 *  nearest-on-segment snapping. Same in-flight-promise caching as `getPageSnapTargets`. */
export function getPageSnapSegments(docId: string, pageIndex: number): Promise<SnapSegment[]> {
  const key = cacheKey(docId, pageIndex);
  const cached = segmentCache.get(key);
  if (cached) return cached;
  const req = Promise.resolve(invoke<SnapSegment[]>("get_page_snap_segments", { docId, pageIndex }))
    .then((result) => result ?? [])
    .catch((err) => {
      segmentCache.delete(key);
      throw err;
    });
  segmentCache.set(key, req);
  return req;
}

/**
 * Uniform-grid spatial index over one page's snap targets, so a nearest-within-tolerance
 * query only has to look at the handful of targets near the cursor instead of the whole
 * page (see `findNearestSnap`'s doc comment for why this replaced a linear scan). Bucket
 * size is fixed at construction — correctness never depends on it (every query still
 * expands to cover the full query tolerance), only the constant factor does.
 */
class SnapGrid {
  private readonly cells = new Map<string, SnapTarget[]>();

  constructor(
    targets: readonly SnapTarget[],
    private readonly cellSizePts: number,
  ) {
    for (const t of targets) {
      if (!Number.isFinite(t.point.x) || !Number.isFinite(t.point.y)) continue;
      const key = this.cellKeyFor(t.point.x, t.point.y);
      const bucket = this.cells.get(key);
      if (bucket) bucket.push(t);
      else this.cells.set(key, [t]);
    }
  }

  private cellKeyFor(x: number, y: number): string {
    return `${Math.floor(x / this.cellSizePts)},${Math.floor(y / this.cellSizePts)}`;
  }

  /** Every target within `tolerancePts` of `cursor` (inclusive). */
  within(cursor: PdfPoint, tolerancePts: number): SnapTarget[] {
    const cellRadius = Math.max(1, Math.ceil(tolerancePts / this.cellSizePts));
    const cx = Math.floor(cursor.x / this.cellSizePts);
    const cy = Math.floor(cursor.y / this.cellSizePts);
    const out: SnapTarget[] = [];
    const tol2 = tolerancePts * tolerancePts;
    for (let gx = cx - cellRadius; gx <= cx + cellRadius; gx++) {
      for (let gy = cy - cellRadius; gy <= cy + cellRadius; gy++) {
        for (const t of this.cells.get(`${gx},${gy}`) ?? []) {
          const dx = t.point.x - cursor.x;
          const dy = t.point.y - cursor.y;
          if (dx * dx + dy * dy <= tol2) out.push(t);
        }
      }
    }
    return out;
  }

  /** Nearest target to `cursor` within `tolerancePts`, or `null`. Same tie-break as a full
   *  linear scan (first-seen wins ties, `<=` comparison) since every candidate within range
   *  of `cursor`'s cell block is still checked exactly, just not the whole page. */
  nearest(cursor: PdfPoint, tolerancePts: number): SnapTarget | null {
    const cellRadius = Math.max(1, Math.ceil(tolerancePts / this.cellSizePts));
    const cx = Math.floor(cursor.x / this.cellSizePts);
    const cy = Math.floor(cursor.y / this.cellSizePts);
    let best: SnapTarget | null = null;
    let bestDist2 = tolerancePts * tolerancePts;
    for (let gx = cx - cellRadius; gx <= cx + cellRadius; gx++) {
      for (let gy = cy - cellRadius; gy <= cy + cellRadius; gy++) {
        const bucket = this.cells.get(`${gx},${gy}`);
        if (!bucket) continue;
        for (const t of bucket) {
          const dx = t.point.x - cursor.x;
          const dy = t.point.y - cursor.y;
          const d2 = dx * dx + dy * dy;
          if (d2 <= bestDist2) {
            best = t;
            bestDist2 = d2;
          }
        }
      }
    }
    return best;
  }
}

/** Bucket size for `SnapGrid` (PDF points). Not load-bearing for correctness (see the class
 *  doc comment) - chosen as a round number comfortably larger than `SNAP_TOLERANCE_PX`'s
 *  typical on-screen size converted to PDF points at common zoom levels, so a query usually
 *  touches only the 3x3 neighbourhood (`cellRadius` 1) rather than a wider block. */
const SNAP_GRID_CELL_SIZE_PTS = 50;

/** One `SnapGrid` per targets array, built lazily on first query and reused for every later
 *  query against that SAME array reference - keyed by object identity (a `WeakMap`, so a
 *  page's targets array is GC'd normally once its cache entry in `getPageSnapTargets` is
 *  invalidated/evicted). `Viewport.svelte` fetches a page's targets once and reuses the same
 *  array reference for every pointer-move of a drag, so this reliably builds the grid once
 *  per page-open rather than once per move. */
const gridCache = new WeakMap<readonly SnapTarget[], SnapGrid>();

/**
 * Pure nearest-neighbour lookup — client-side mirror of Rust `PageGeometry::nearest_snap`,
 * which itself queries an `rstar` R-tree (see `src-tauri/src/geometry/mod.rs`). This used to
 * be a plain linear scan on the reasoning that "a typical page's target count is small
 * enough" - measured false on a dense construction-plan page: `e2e/specs/perf-tool-lag.spec.js`
 * against a synthetic ~18k-target/page fixture showed the linear scan costing 7-13ms per
 * pointer-move (most of a 16.7ms frame budget) on Line/Arrow/Angle - the three
 * `SNAP_ELIGIBLE_TOOLS` that capture a point on every move - matching the owner's reported
 * "line, arrow and angle markup tools lag while drawing" exactly (Rectangle/Ellipse/Highlight
 * are not snap-eligible and were never affected). Same fixture with the grid index below:
 * <1ms. Returns `null` when nothing is within `tolerancePts`.
 */
export function findNearestSnap(
  targets: readonly SnapTarget[],
  cursor: PdfPoint,
  tolerancePts: number,
): SnapTarget | null {
  if (targets.length === 0) return null;
  let grid = gridCache.get(targets);
  if (!grid) {
    grid = new SnapGrid(targets, SNAP_GRID_CELL_SIZE_PTS);
    gridCache.set(targets, grid);
  }
  return grid.nearest(cursor, tolerancePts);
}


// ---------------------------------------------------------------------------
// Priority-ranked snapping with cursor-dependent kinds (perpendicular, nearest)
// ---------------------------------------------------------------------------
//
// Adapted from redcolumn (Apache-2.0), packages/measure/src/snap.ts, modified: scoring
// idea (kind priority dominates, distance breaks ties) and the perpendicular-foot /
// nearest-point-on-segment maths, re-expressed over Redline's Rust-built target list.

/** Lower wins when several snaps are within range. */
const SNAP_PRIORITY: Record<SnapKind, number> = {
  Endpoint: 0,
  ArcCenter: 0,
  Intersection: 1,
  Midpoint: 2,
  Quadrant: 2,
  Perpendicular: 2,
  Nearest: 4,
};

/** Nearest point on segment `s` to `p`, and its distance. */
export function nearestOnSegment(
  p: PdfPoint,
  s: SnapSegment,
): { point: PdfPoint; dist: number; t: number } {
  const dx = s[2] - s[0];
  const dy = s[3] - s[1];
  const len2 = dx * dx + dy * dy;
  const t = len2 ? Math.max(0, Math.min(1, ((p.x - s[0]) * dx + (p.y - s[1]) * dy) / len2)) : 0;
  const x = s[0] + t * dx;
  const y = s[1] + t * dy;
  return { point: { x, y }, dist: Math.hypot(x - p.x, y - p.y), t };
}

/** Foot of the perpendicular from `from` onto segment `s`, or null when it falls outside the
 *  segment (or the segment is degenerate). */
export function perpendicularFoot(from: PdfPoint, s: SnapSegment): PdfPoint | null {
  const dx = s[2] - s[0];
  const dy = s[3] - s[1];
  const len2 = dx * dx + dy * dy;
  if (len2 === 0) return null;
  const t = ((from.x - s[0]) * dx + (from.y - s[1]) * dy) / len2;
  if (t < 0 || t > 1) return null;
  return { x: s[0] + t * dx, y: s[1] + t * dy };
}

/** Coordinates at or beyond this magnitude are treated as corrupt. */
const MAX_SANE_COORD = 1e9;
/** A segment needing more than this many grid steps is too long to register. */
const MAX_SEGMENT_STEPS = 512;
/** Total grid cell registrations per page; beyond it further segments are not registered. */
const MAX_GRID_REGISTRATIONS = 2_000_000;

/** Uniform grid over segments; a segment is registered in every cell its length crosses. */
class SegmentGrid {
  private readonly cells = new Map<string, number[]>();
  private stamp: Uint32Array;
  private query = 0;

  constructor(
    private readonly segments: readonly SnapSegment[],
    private readonly cell: number,
  ) {
    this.stamp = new Uint32Array(segments.length);
    let registrations = 0;
    segments.forEach((s, i) => {
      // Hostile or broken content must degrade, never hang: skip non-finite or absurdly long
      // segments (the Rust side already clips to the page; this is the backstop), and stop
      // registering once the per-page budget is spent. Unregistered segments simply do not
      // offer Nearest/Perpendicular; the endpoint-like targets still snap.
      if (registrations >= MAX_GRID_REGISTRATIONS) return;
      if (!(Math.abs(s[0]) < MAX_SANE_COORD && Math.abs(s[1]) < MAX_SANE_COORD && Math.abs(s[2]) < MAX_SANE_COORD && Math.abs(s[3]) < MAX_SANE_COORD)) return;
      const len = Math.hypot(s[2] - s[0], s[3] - s[1]);
      if (!(len <= MAX_SEGMENT_STEPS * (cell / 2))) return;
      const steps = Math.max(1, Math.ceil(len / (cell / 2)));
      let last = "";
      for (let k = 0; k <= steps; k++) {
        const t = k / steps;
        const key = `${Math.floor((s[0] + (s[2] - s[0]) * t) / cell)},${Math.floor((s[1] + (s[3] - s[1]) * t) / cell)}`;
        if (key === last) continue;
        last = key;
        registrations++;
        const bucket = this.cells.get(key);
        if (bucket) {
          if (bucket[bucket.length - 1] !== i) bucket.push(i);
        } else this.cells.set(key, [i]);
      }
    });
  }

  /** Segments passing near `p` (within `radius` plus one cell of slack). */
  near(p: PdfPoint, radius: number): SnapSegment[] {
    this.query = (this.query + 1) >>> 0 || 1;
    const out: SnapSegment[] = [];
    const c = this.cell;
    for (let gx = Math.floor((p.x - radius) / c) - 1; gx <= Math.floor((p.x + radius) / c) + 1; gx++) {
      for (let gy = Math.floor((p.y - radius) / c) - 1; gy <= Math.floor((p.y + radius) / c) + 1; gy++) {
        for (const i of this.cells.get(`${gx},${gy}`) ?? []) {
          if (this.stamp[i] === this.query) continue;
          this.stamp[i] = this.query;
          out.push(this.segments[i]!);
        }
      }
    }
    return out;
  }
}

const segGridCache = new WeakMap<readonly SnapSegment[], SegmentGrid>();

/**
 * Best snap within `tolerancePts` of `cursor`, ranked like CAD object snaps: kind priority
 * dominates (Endpoint/ArcCenter, then Intersection, then Midpoint/Quadrant/Perpendicular, then
 * Nearest) and distance breaks ties within a priority. `Perpendicular` needs `from`, the point
 * the line being drawn starts at; `Nearest` is the closest point on any straight segment.
 * Returns `null` when nothing is in range.
 */
export function findBestSnap(
  targets: readonly SnapTarget[],
  segments: readonly SnapSegment[],
  cursor: PdfPoint,
  tolerancePts: number,
  from: PdfPoint | null = null,
): SnapTarget | null {
  let best: SnapTarget | null = null;
  let bestScore = Infinity;
  const consider = (point: PdfPoint, kind: SnapKind) => {
    const d = Math.hypot(point.x - cursor.x, point.y - cursor.y);
    if (d > tolerancePts) return;
    const score = SNAP_PRIORITY[kind] * tolerancePts * 10 + d;
    if (score < bestScore) {
      bestScore = score;
      best = { point, kind };
    }
  };

  if (targets.length > 0) {
    let grid = gridCache.get(targets);
    if (!grid) {
      grid = new SnapGrid(targets, SNAP_GRID_CELL_SIZE_PTS);
      gridCache.set(targets, grid);
    }
    for (const t of grid.within(cursor, tolerancePts)) consider(t.point, t.kind);
  }

  if (segments.length > 0) {
    let sg = segGridCache.get(segments);
    if (!sg) {
      sg = new SegmentGrid(segments, SNAP_GRID_CELL_SIZE_PTS);
      segGridCache.set(segments, sg);
    }
    for (const s of sg.near(cursor, tolerancePts)) {
      const n = nearestOnSegment(cursor, s);
      if (n.dist <= tolerancePts) consider(n.point, "Nearest");
      if (from) {
        const foot = perpendicularFoot(from, s);
        if (foot) consider(foot, "Perpendicular");
      }
    }
  }
  return best;
}
