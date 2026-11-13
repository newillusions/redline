import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  getPageSnapTargets,
  getPageSnapSegments,
  invalidateSnapCache,
  findNearestSnap,
  findBestSnap,
  nearestOnSegment,
  perpendicularFoot,
} from "./snap";
import type { SnapSegment } from "./snap";
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

describe("getPageSnapSegments", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("calls get_page_snap_segments and caches per page; invalidate refetches", async () => {
    mockInvoke.mockResolvedValue([[0, 0, 10, 0]] as never);
    const a = await getPageSnapSegments("segdoc", 1);
    const b = await getPageSnapSegments("segdoc", 1);
    expect(mockInvoke).toHaveBeenCalledWith("get_page_snap_segments", { docId: "segdoc", pageIndex: 1 });
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(a).toBe(b);
    invalidateSnapCache("segdoc");
    await getPageSnapSegments("segdoc", 1);
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });

  it("does not cache a rejected request", async () => {
    mockInvoke.mockRejectedValueOnce(new Error("gone"));
    await expect(getPageSnapSegments("segErr", 0)).rejects.toThrow("gone");
    mockInvoke.mockResolvedValueOnce([] as never);
    await expect(getPageSnapSegments("segErr", 0)).resolves.toEqual([]);
  });
});

describe("nearestOnSegment / perpendicularFoot", () => {
  const seg: SnapSegment = [0, 0, 10, 0];

  it("projects onto the interior and clamps to the ends", () => {
    expect(nearestOnSegment({ x: 4, y: 3 }, seg)).toMatchObject({ point: { x: 4, y: 0 }, dist: 3 });
    expect(nearestOnSegment({ x: -5, y: 0 }, seg).point).toEqual({ x: 0, y: 0 });
    expect(nearestOnSegment({ x: 50, y: 2 }, seg).point).toEqual({ x: 10, y: 0 });
  });

  it("handles a zero-length segment", () => {
    expect(nearestOnSegment({ x: 3, y: 4 }, [1, 1, 1, 1]).dist).toBe(Math.hypot(2, 3));
    expect(perpendicularFoot({ x: 3, y: 4 }, [1, 1, 1, 1])).toBeNull();
  });

  it("perpendicular foot exists only within the segment", () => {
    expect(perpendicularFoot({ x: 4, y: 9 }, seg)).toEqual({ x: 4, y: 0 });
    expect(perpendicularFoot({ x: 14, y: 9 }, seg)).toBeNull();
    expect(perpendicularFoot({ x: -1, y: 9 }, seg)).toBeNull();
  });
});

describe("findBestSnap", () => {
  const t = (x: number, y: number, kind: SnapTarget["kind"]): SnapTarget => ({ point: { x, y }, kind });

  it("returns null with nothing in range", () => {
    expect(findBestSnap([t(100, 100, "Endpoint")], [[0, 0, 10, 0]], { x: 50, y: 50 }, 5)).toBeNull();
    expect(findBestSnap([], [], { x: 0, y: 0 }, 5)).toBeNull();
  });

  it("kind priority beats distance: an endpoint beats a closer midpoint", () => {
    const targets = [t(0, 0, "Midpoint"), t(3, 0, "Endpoint")];
    expect(findBestSnap(targets, [], { x: 0.5, y: 0 }, 5)?.kind).toBe("Endpoint");
  });

  it("intersection beats midpoint and quadrant; arc centre ranks with endpoint", () => {
    expect(findBestSnap([t(1, 0, "Midpoint"), t(2, 0, "Intersection")], [], { x: 1, y: 0 }, 5)?.kind).toBe("Intersection");
    expect(findBestSnap([t(0, 0, "Quadrant"), t(2, 0, "ArcCenter")], [], { x: 0, y: 0 }, 5)?.kind).toBe("ArcCenter");
  });

  it("within one priority the closer target wins", () => {
    const hit = findBestSnap([t(4, 0, "Quadrant"), t(1, 0, "Midpoint")], [], { x: 0, y: 0 }, 5);
    expect(hit?.point).toEqual({ x: 1, y: 0 });
  });

  it("falls back to Nearest on a segment when no vertex-like target is in range", () => {
    const hit = findBestSnap([], [[0, 0, 100, 0]], { x: 40, y: 2 }, 5);
    expect(hit).toEqual({ point: { x: 40, y: 0 }, kind: "Nearest" });
  });

  it("a static target outranks Nearest on the same line", () => {
    const hit = findBestSnap([t(41, 0, "Midpoint")], [[0, 0, 100, 0]], { x: 40, y: 1 }, 5);
    expect(hit?.kind).toBe("Midpoint");
  });

  it("Perpendicular needs a from-point and beats Nearest when in range", () => {
    const seg: SnapSegment = [0, 0, 100, 0];
    expect(findBestSnap([], [seg], { x: 40, y: 1 }, 5, null)?.kind).toBe("Nearest");
    const hit = findBestSnap([], [seg], { x: 41, y: 1 }, 5, { x: 40, y: 30 });
    expect(hit).toEqual({ point: { x: 40, y: 0 }, kind: "Perpendicular" });
  });

  it("no Perpendicular when the foot falls off the segment", () => {
    const hit = findBestSnap([], [[0, 0, 10, 0]], { x: 11, y: 1 }, 5, { x: 30, y: 30 });
    expect(hit?.kind).toBe("Nearest");
  });

  it("finds a segment that passes through the cursor's cell without an end there", () => {
    // Long diagonal registers in every cell it crosses (cell is 50pt).
    const hit = findBestSnap([], [[0, 0, 400, 400]], { x: 200, y: 201 }, 4);
    expect(hit?.kind).toBe("Nearest");
    expect(hit?.point.x).toBeCloseTo(200.5, 5);
  });
});

describe("hostile geometry cannot hang the snap grid", () => {
  const timed = (fn: () => void) => {
    const t0 = performance.now();
    fn();
    return performance.now() - t0;
  };

  it("a segment with 1e12 coordinates is skipped quickly; the rest still snap", () => {
    const segs: SnapSegment[] = [[0, 0, 1e12, 1e12], [-1e12, 5, 1e12, 5], [0, 0, 100, 0]];
    let hit: ReturnType<typeof findBestSnap> = null;
    const ms = timed(() => { hit = findBestSnap([], segs, { x: 50, y: 1 }, 4); });
    expect(ms).toBeLessThan(1000);
    expect(hit).toEqual({ point: { x: 50, y: 0 }, kind: "Nearest" });
  });

  it("NaN and Infinity coordinates are ignored, in segments and targets", () => {
    const segs: SnapSegment[] = [[NaN, 0, 10, 0], [0, 0, Infinity, 0], [0, 0, 100, 0]];
    const targets: SnapTarget[] = [{ point: { x: NaN, y: 1 }, kind: "Endpoint" }, { point: { x: 3, y: 0 }, kind: "Midpoint" }];
    let hit: ReturnType<typeof findBestSnap> = null;
    const ms = timed(() => { hit = findBestSnap(targets, segs, { x: 3, y: 1 }, 4); });
    expect(ms).toBeLessThan(1000);
    expect(hit).toEqual({ point: { x: 3, y: 0 }, kind: "Midpoint" });
  });

  it("100k tiny segments build and query within a time bound", () => {
    const segs: SnapSegment[] = Array.from({ length: 100_000 }, (_, i) => {
      const x = (i % 400) * 3;
      const y = Math.floor(i / 400) * 3;
      return [x, y, x + 0.5, y + 0.5] as const;
    });
    let hit: ReturnType<typeof findBestSnap> = null;
    const ms = timed(() => {
      hit = findBestSnap([], segs, { x: 30.2, y: 30.2 }, 2);
      findBestSnap([], segs, { x: 600, y: 90 }, 2);
    });
    expect(ms).toBeLessThan(3000);
    expect(hit).not.toBeNull();
  });

  it("past the registration budget it degrades to unregistered segments, never hangs", () => {
    // Many long (but allowed) segments exceed the 2M-cell budget: must still finish.
    const segs: SnapSegment[] = Array.from({ length: 3000 }, (_, i) => [0, i * 0.01, 12000, i * 0.01] as const);
    const ms = timed(() => { findBestSnap([], segs, { x: 5, y: 5 }, 4); });
    expect(ms).toBeLessThan(6000);
  });
});
