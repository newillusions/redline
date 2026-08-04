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
 * Pure nearest-neighbour lookup — client-side mirror of Rust
 * `PageGeometry::nearest_snap`. Deliberately a linear scan, not spatially
 * accelerated: this is the SAME scale tradeoff the Rust module doc comment
 * makes for why `Intersection` is deferred (page target counts are bounded by
 * Endpoint+Midpoint density, not the full O(n²) pair space), and a typical
 * page's target count is small enough that a scan per pointer-move is cheap.
 * Returns `null` when nothing is within `tolerancePts`.
 */
export function findNearestSnap(
  targets: readonly SnapTarget[],
  cursor: PdfPoint,
  tolerancePts: number,
): SnapTarget | null {
  let best: SnapTarget | null = null;
  let bestDist2 = tolerancePts * tolerancePts;
  for (const t of targets) {
    const dx = t.point.x - cursor.x;
    const dy = t.point.y - cursor.y;
    const d2 = dx * dx + dy * dy;
    if (d2 <= bestDist2) {
      best = t;
      bestDist2 = d2;
    }
  }
  return best;
}
