import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { getPageSnapTargets, invalidateSnapCache, findNearestSnap } from "./snap";
import type { SnapTarget } from "./snap";

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
});
