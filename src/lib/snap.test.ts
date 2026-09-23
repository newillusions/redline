import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { getPageSnapTargets, invalidateSnapCache, findNearestSnap } from "./snap";
import type { SnapTarget } from "./snap";
import type { PdfPoint } from "./ipc";

const mockInvoke = vi.mocked(invoke);

describe("getPageSnapTargets", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("calls get_page_snap_targets with camelCase docId/pageIndex (Tauri v2 arg-casing convention)", async () => {
    mockInvoke.mockResolvedValue([] as never);
    await getPageSnapTargets("doc1", 2);
    expect(mockInvoke).toHaveBeenCalledWith("get_page_snap_targets", { docId: "doc1", pageIndex: 2 });
  });

  it("caches the result - a second call for the same page does not invoke again", async () => {
    mockInvoke.mockResolvedValue([{ point: { x: 1, y: 2 }, kind: "Endpoint" }] as never);
    const a = await getPageSnapTargets("doc1", 0);
    const b = await getPageSnapTargets("doc1", 0);
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(a).toBe(b);
  });

  it("caches per (doc_id, page_index) - different pages invoke separately", async () => {
    mockInvoke.mockResolvedValue([] as never);
    await getPageSnapTargets("docMulti", 0);
    await getPageSnapTargets("docMulti", 1);
    await getPageSnapTargets("docMulti2", 0);
    expect(mockInvoke).toHaveBeenCalledTimes(3);
  });

  it("does not cache a rejected request - a later call retries", async () => {
    mockInvoke.mockRejectedValueOnce(new Error("render thread gone"));
    await expect(getPageSnapTargets("docErr", 0)).rejects.toThrow("render thread gone");

    mockInvoke.mockResolvedValueOnce([] as never);
    await expect(getPageSnapTargets("docErr", 0)).resolves.toEqual([]);
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });

  it("concurrent callers for the same page share one in-flight request", async () => {
    let resolveFn!: (v: SnapTarget[]) => void;
    mockInvoke.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveFn = resolve;
      }) as never,
    );
    const p1 = getPageSnapTargets("docConcurrent", 0);
    const p2 = getPageSnapTargets("docConcurrent", 0);
    resolveFn([]);
    await Promise.all([p1, p2]);
    expect(mockInvoke).toHaveBeenCalledTimes(1);
  });

  it("invalidateSnapCache forces a refetch for that doc_id only", async () => {
    mockInvoke.mockResolvedValue([] as never);
    await getPageSnapTargets("docA", 0);
    await getPageSnapTargets("docB", 0);
    invalidateSnapCache("docA");
    await getPageSnapTargets("docA", 0);
    await getPageSnapTargets("docB", 0);
    // docA refetched (2 calls total for it), docB still cached (1 call).
    expect(mockInvoke).toHaveBeenCalledTimes(3);
  });
});

describe("findNearestSnap", () => {
  const targets: SnapTarget[] = [
    { point: { x: 0, y: 0 }, kind: "Endpoint" },
    { point: { x: 10, y: 0 }, kind: "Endpoint" },
    { point: { x: 5, y: 0 }, kind: "Midpoint" },
  ];

  it("returns the nearest target within tolerance", () => {
    const hit = findNearestSnap(targets, { x: 0.5, y: 0 }, 2);
    expect(hit?.point).toEqual({ x: 0, y: 0 });
    expect(hit?.kind).toBe("Endpoint");
  });

  it("returns null when nothing is within tolerance", () => {
    expect(findNearestSnap(targets, { x: 100, y: 100 }, 2)).toBeNull();
  });

  it("returns null for an empty target list", () => {
    expect(findNearestSnap([], { x: 0, y: 0 }, 100)).toBeNull();
  });

  it("prefers the closer of two targets both within tolerance", () => {
    // (0.5,0) is closer to (0,0) than to (5,0)'s midpoint target under a
    // generous tolerance covering both.
    const hit = findNearestSnap(targets, { x: 0.5, y: 0 }, 10);
    expect(hit?.point).toEqual({ x: 0, y: 0 });
  });

  it("is exactly-at-tolerance inclusive (boundary case)", () => {
    // Distance from (0,0) to (2,0) is exactly 2.
    const hit = findNearestSnap([{ point: { x: 0, y: 0 }, kind: "Endpoint" }], { x: 2, y: 0 }, 2);
    expect(hit).not.toBeNull();
  });

  it("finds a target several grid cells away when tolerance is larger than one cell (SNAP_GRID_CELL_SIZE_PTS is 50)", () => {
    // Cursor and target are ~120pts apart (spans 2-3 grid cells) with a tolerance that
    // covers it - exercises the multi-cell cellRadius expansion, not just the 3x3 default.
    const hit = findNearestSnap([{ point: { x: 0, y: 0 }, kind: "Endpoint" }], { x: 0, y: 120 }, 150);
    expect(hit?.point).toEqual({ x: 0, y: 0 });
  });

  it("matches a brute-force linear scan on dense random data (grid-index correctness)", () => {
    // Regression guard for the grid-index rewrite (2026-09, tool-lag fix): the grid must
    // return the exact same nearest target a full scan would, for arbitrary point clouds -
    // not just the hand-picked cases above. Seeded PRNG so failures are reproducible.
    let seed = 42;
    const rand = () => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed / 0x7fffffff;
    };
    const dense: SnapTarget[] = Array.from({ length: 2000 }, () => ({
      point: { x: rand() * 1000, y: rand() * 1000 },
      kind: "Endpoint" as const,
    }));
    const bruteForce = (cursor: PdfPoint, tolerancePts: number): SnapTarget | null => {
      let best: SnapTarget | null = null;
      let bestDist2 = tolerancePts * tolerancePts;
      for (const t of dense) {
        const dx = t.point.x - cursor.x;
        const dy = t.point.y - cursor.y;
        const d2 = dx * dx + dy * dy;
        if (d2 <= bestDist2) {
          best = t;
          bestDist2 = d2;
        }
      }
      return best;
    };
    for (let i = 0; i < 50; i++) {
      const cursor = { x: rand() * 1000, y: rand() * 1000 };
      const tolerance = 5 + rand() * 100;
      const expected = bruteForce(cursor, tolerance);
      const actual = findNearestSnap(dense, cursor, tolerance);
      expect(actual?.point).toEqual(expected?.point);
    }
  });

  it("rebuilds its index for a different array reference rather than reusing a stale grid", () => {
    // Two distinct arrays with the same single point at different cache-relevant positions -
    // querying the second must not return a hit cached against the first's identity.
    const a: SnapTarget[] = [{ point: { x: 0, y: 0 }, kind: "Endpoint" }];
    const b: SnapTarget[] = [{ point: { x: 500, y: 500 }, kind: "Endpoint" }];
    expect(findNearestSnap(a, { x: 0, y: 0 }, 2)?.point).toEqual({ x: 0, y: 0 });
    expect(findNearestSnap(b, { x: 500, y: 500 }, 2)?.point).toEqual({ x: 500, y: 500 });
    expect(findNearestSnap(b, { x: 0, y: 0 }, 2)).toBeNull();
  });
});
