/**
 * Vector snap-target client (spec §5, v1). Fetches + caches a page's snap-target
 * index (Rust `geometry::build_snap_index`, `Endpoint`/`Midpoint` only — see that
 * module's doc comment for exactly what's populated and why `Intersection`/
 * `ArcCenter` are deferred) and finds the nearest target to a PDF-space point
 * within a tolerance, entirely client-side so a drag gesture never pays an IPC
 * round-trip per pointer-move — `Viewport.svelte` prefetches a page's targets once
 * (on page open / snap-eligible tool activation) and looks up against the cached
 * array on every point capture.
 */
import { invoke } from "@tauri-apps/api/core";
import type { PdfPoint } from "./ipc";

export type SnapKind = "Endpoint" | "Midpoint" | "Intersection" | "ArcCenter";

export interface SnapTarget {
  point: PdfPoint;
  kind: SnapKind;
}

const cache = new Map<string, Promise<SnapTarget[]>>();

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
  const req = Promise.resolve(invoke<SnapTarget[]>("get_page_snap_targets", { docId, pageIndex }))
    .then((result) => result ?? [])
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
      const key = this.cellKeyFor(t.point.x, t.point.y);
      const bucket = this.cells.get(key);
      if (bucket) bucket.push(t);
      else this.cells.set(key, [t]);
    }
  }

  private cellKeyFor(x: number, y: number): string {
    return `${Math.floor(x / this.cellSizePts)},${Math.floor(y / this.cellSizePts)}`;
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
