import { describe, it, expect, beforeEach } from "vitest";
import type { Markup } from "./ipc";
import { markupClipboard } from "./markup-clipboard.svelte";
import { placeAlongPath, type ArrangeCtx } from "./arrange-actions";
import { countGroups } from "./count-groups";
import { BY, NOW, mk, mkCount, seqIds, storeWith } from "./arrange.test-util";

const mintId = seqIds("pa");
function ctxFor(store: ReturnType<typeof storeWith>): ArrangeCtx {
  return {
    store, identity: BY, scales: [], pageIndex: 0, pageCount: 3, pageWidthPts: 600, pageHeightPts: 400,
    cursor: null, viewCenter: null, now: () => NOW, mintId,
  };
}
const select = (s: ReturnType<typeof storeWith>, ...ids: string[]) => { s.selectedIds = new Set(ids); };
const P = (x: number, y: number) => ({ x, y });
const pathLine = (id = "path", pts = [P(0, 100), P(100, 100)]) =>
  mk(id, { markup_type: "Polyline", geometry: { Polyline: pts } });
const pos = (m: Markup) => (m.geometry as { Point: { x: number; y: number } }).Point;

beforeEach(() => markupClipboard.clear());

describe("placeAlongPath", () => {
  it("divide: places count symbols at the division points, centre on each mark, one undo step", async () => {
    const s = storeWith([pathLine(), mkCount("c1", 500, 300, null)], "d", {
      add: async () => {}, update: async () => {}, remove: async () => {}, reorder: async () => {},
    });
    select(s, "path", "c1");
    const r = placeAlongPath(ctxFor(s), { kind: "divide", count: 4 }, false);
    expect(r).toEqual({ ok: true, message: "Placed 3 markups at 3 marks along the path" });
    const added = s.markups.slice(2);
    expect(added.map((m) => pos(m).x)).toEqual([25, 50, 75]);
    expect(added.every((m) => pos(m).y === 100)).toBe(true);
    expect(s.markups[1].geometry).toEqual({ Point: P(500, 300) }); // original stays put
    expect(s.selectedIds).toEqual(new Set(added.map((m) => m.id)));
    expect(new Set(added.map((m) => m.id)).size).toBe(3);
    s.undo(); // the whole placement is ONE undo step
    expect(s.markups.map((m) => m.id)).toEqual(["path", "c1"]);
  });

  it("counts join the source's count: the total is the source plus every copy", () => {
    const s = storeWith([pathLine(), mkCount("c1", 0, 0, null)]);
    select(s, "path", "c1");
    placeAlongPath(ctxFor(s), { kind: "array", count: 5 }, false);
    const groups = countGroups(s.markups);
    expect(groups).toHaveLength(1);
    expect(groups[0].ids).toHaveLength(6);
    expect(s.getById("c1")!.count_group).toBe("c1");
  });

  it("measure: every spacing along the path", () => {
    const s = storeWith([pathLine(), mkCount("c1", 0, 0, "g")]);
    select(s, "path", "c1");
    placeAlongPath(ctxFor(s), { kind: "measure", spacing: 30 }, false);
    expect(s.markups.slice(2).map((m) => pos(m).x)).toEqual([30, 60, 90]);
  });

  it("the first selected markup is the path, whichever order they sit in the document", () => {
    const s = storeWith([mkCount("c1", 0, 0, null), pathLine()]);
    select(s, "path", "c1");
    expect(placeAlongPath(ctxFor(s), { kind: "array", count: 2 }, false).ok).toBe(true);
    select(s, "c1", "path"); // a count is not a path
    const r = placeAlongPath(ctxFor(s), { kind: "array", count: 2 }, false);
    expect(r.ok).toBe(false);
    expect(r.message).toMatch(/must be a line/);
  });

  it("a multi-item selection moves as a set: its bounding-box centre lands on each mark", () => {
    const a = mk("a", { markup_type: "Line", geometry: { Polyline: [P(0, 0), P(10, 0)] } });
    const b = mk("b", { markup_type: "Line", geometry: { Polyline: [P(0, 20), P(10, 20)] } });
    const s = storeWith([pathLine("path", [P(100, 100), P(300, 100)]), a, b]);
    select(s, "path", "a", "b");
    placeAlongPath(ctxFor(s), { kind: "array", count: 2 }, false);
    const [a1, b1, a2, b2] = s.markups.slice(3);
    expect(a1.geometry).toEqual({ Polyline: [P(95, 90), P(105, 90)] });
    expect(b1.geometry).toEqual({ Polyline: [P(95, 110), P(105, 110)] });
    expect(a2.geometry).toEqual({ Polyline: [P(295, 90), P(305, 90)] });
    expect(b2.geometry).toEqual({ Polyline: [P(295, 110), P(305, 110)] });
  });

  it("align turns copies with the path direction, relative to the start", () => {
    const item = mk("i", { markup_type: "Line", geometry: { Polyline: [P(-5, 0), P(5, 0)] } });
    // path goes east then north; array of 3 marks: start, the corner area, the end (heading north)
    const s = storeWith([pathLine("path", [P(100, 100), P(200, 100), P(200, 200)]), item]);
    select(s, "path", "i");
    placeAlongPath(ctxFor(s), { kind: "array", count: 3 }, true);
    const [first, mid, last] = s.markups.slice(2).map((m) => (m.geometry as { Polyline: { x: number; y: number }[] }).Polyline);
    // first mark: east, no turn, centred (100,100)
    expect(first[0].x).toBeCloseTo(95); expect(first[1].x).toBeCloseTo(105);
    // mark 2 at (200,100) is on the first leg's end: still east
    expect(mid[0].y).toBeCloseTo(100); expect(mid[1].y).toBeCloseTo(100);
    // last mark at (200,200) heading north: the item is vertical
    expect(last[0].x).toBeCloseTo(200); expect(last[1].x).toBeCloseTo(200);
    expect(Math.abs(last[1].y - last[0].y)).toBeCloseTo(10);
  });

  it("align refuses items that cannot turn, creating nothing", () => {
    const s = storeWith([pathLine(), mk("box", { rect: [0, 0, 10, 10] })]);
    select(s, "path", "box");
    const r = placeAlongPath(ctxFor(s), { kind: "array", count: 3 }, true);
    expect(r.ok).toBe(false);
    expect(s.markups).toHaveLength(2);
    expect(placeAlongPath(ctxFor(s), { kind: "array", count: 3 }, false).ok).toBe(true); // fine without align
  });

  it("refuses: nothing selected, only a path, items on another page, a locked count, an over-limit spec", () => {
    const s = storeWith([pathLine(), mkCount("c1", 0, 0, null), mkCount("c2", 0, 0, null, { page: 1 })]);
    expect(placeAlongPath(ctxFor(s), { kind: "array", count: 2 }, false).ok).toBe(false);
    select(s, "path");
    expect(placeAlongPath(ctxFor(s), { kind: "array", count: 2 }, false).ok).toBe(false);
    select(s, "path", "c2");
    expect(placeAlongPath(ctxFor(s), { kind: "array", count: 2 }, false).message).toMatch(/one page/);
    select(s, "path", "c1");
    expect(placeAlongPath(ctxFor(s), { kind: "array", count: 20000 }, false).message).toMatch(/At most/);
    expect(s.markups).toHaveLength(3);
    const locked = storeWith([pathLine(), mkCount("c1", 0, 0, null, { annot_flags: 128 })]);
    select(locked, "path", "c1");
    expect(placeAlongPath(ctxFor(locked), { kind: "array", count: 2 }, false).message).toMatch(/locked/);
  });

  it("a closed path (rectangle) is walked round without repeating the start", () => {
    const rect = mk("r", { rect: [0, 0, 40, 40] });
    const s = storeWith([rect, mkCount("c1", 500, 300, "g")]);
    select(s, "r", "c1");
    placeAlongPath(ctxFor(s), { kind: "array", count: 4 }, false);
    expect(s.markups.slice(2).map((m) => [pos(m).x, pos(m).y])).toEqual([[0, 0], [40, 0], [40, 40], [0, 40]]);
  });
});
