import { describe, it, expect } from "vitest";
import type { Markup } from "./ipc";
import {
  buildUnits, alignMoves, distributeMoves, applyMoves, orderPage, orderChanges, multiplyOffsets, spacingToPoints,
} from "./markup-arrange";
import { BY, NOW, mk, scale } from "./arrange.test-util";

const r = (id: string, x0: number, y0: number, x1: number, y1: number, o: Partial<Markup> = {}) => mk(id, { rect: [x0, y0, x1, y1], ...o });
const units = (ms: Markup[], all: Markup[] = ms) => buildUnits(ms, all);
const delta = (mv: { dx: number; dy: number }[]) => mv.map((m) => [m.dx, m.dy]);

describe("align (primary = first selected)", () => {
  const a = r("a", 10, 10, 20, 20), b = r("b", 30, 40, 50, 44), c = r("c", 0, 0, 4, 100);
  it("left / right match the primary's edge", () => {
    expect(delta(alignMoves(units([a, b, c]), "left"))).toEqual([[-20, 0], [10, 0]]);
    expect(delta(alignMoves(units([a, b, c]), "right"))).toEqual([[-30, 0], [16, 0]]);
  });
  it("top / bottom: y is up, top is the larger y", () => {
    expect(delta(alignMoves(units([a, b]), "top"))).toEqual([[0, -24]]);
    expect(delta(alignMoves(units([a, b]), "bottom"))).toEqual([[0, -30]]);
  });
  it("center matches the horizontal centre (x); middle the vertical middle (y)", () => {
    expect(delta(alignMoves(units([a, b]), "center"))).toEqual([[-25, 0]]);
    expect(delta(alignMoves(units([a, b]), "middle"))).toEqual([[0, -27]]);
  });
  it("another primary gives another result", () => {
    expect(delta(alignMoves(units([b, a]), "left"))).toEqual([[20, 0]]);
  });
  it("needs two units and omits units already in line", () => {
    expect(alignMoves(units([a]), "left")).toEqual([]);
    expect(alignMoves(units([a, r("x", 10, 50, 15, 60)]), "left")).toEqual([]);
  });
  it("a group is one unit: members keep their relative layout", () => {
    const g1 = r("g1", 100, 0, 110, 10, { group_id: "G" }), g2 = r("g2", 120, 0, 130, 10, { group_id: "G" });
    const all = [a, g1, g2];
    const u = buildUnits([a, g1], all);
    expect(u).toHaveLength(2);
    expect(u[1].members.map((m) => m.id)).toEqual(["g1", "g2"]);
    const moves = alignMoves(u, "left");
    expect(delta(moves)).toEqual([[-90, 0]]);
    const { pairs } = applyMoves(moves, BY, NOW);
    expect(pairs.map((p) => p.after.geometry)).toEqual([
      { Rect: { min: { x: 10, y: 0 }, max: { x: 20, y: 10 } } },
      { Rect: { min: { x: 30, y: 0 }, max: { x: 40, y: 10 } } },
    ]);
  });
});

describe("distribute", () => {
  it("evens out the centres, keeping the outer two", () => {
    const m = distributeMoves(units([r("a", 0, 0, 10, 10), r("b", 12, 0, 22, 10), r("c", 100, 0, 110, 10)]), "horizontal");
    expect(m).toHaveLength(1);
    expect(m[0].unit.members[0].id).toBe("b");
    expect(m[0].dx).toBeCloseTo(38, 9); // centres 5, 17, 105 -> 5, 55, 105
  });
  it("sorts by position, not selection order", () => {
    const m = distributeMoves(units([r("c", 100, 0, 110, 10), r("a", 0, 0, 10, 10), r("b", 12, 0, 22, 10)]), "horizontal");
    expect(m[0].unit.members[0].id).toBe("b");
  });
  it("vertical moves y only; needs three units", () => {
    const m = distributeMoves(units([r("a", 0, 0, 10, 10), r("b", 0, 12, 10, 22), r("c", 0, 100, 10, 110)]), "vertical");
    expect(m[0].dx).toBe(0);
    expect(m[0].dy).toBeCloseTo(38, 9);
    expect(distributeMoves(units([r("a", 0, 0, 1, 1), r("b", 5, 5, 6, 6)]), "horizontal")).toEqual([]);
  });
});

describe("applyMoves", () => {
  it("bumps the audit of what it moves and never moves a unit with a locked member", () => {
    const a = r("a", 0, 0, 10, 10), b = r("b", 20, 0, 30, 10, { annot_flags: 4 | 0x80 });
    const moves = alignMoves(units([a, b]), "bottom").concat(alignMoves(units([a, r("c", 40, 5, 50, 15)]), "bottom"));
    const { pairs, lockedSkipped } = applyMoves(moves, BY, NOW);
    expect(lockedSkipped).toBe(0); // b already in line: no move produced for it
    expect(pairs).toHaveLength(1);
    expect(pairs[0].after.audit).toMatchObject({ modified_by: BY, modified_at: NOW, revision: 8 });
    const lockedMove = alignMoves(units([a, r("d", 20, 5, 30, 15, { annot_flags: 4 | 0x80 })]), "bottom");
    const res = applyMoves(lockedMove, BY, NOW);
    expect(res.pairs).toEqual([]);
    expect(res.lockedSkipped).toBe(1);
  });
});

describe("order (z-order within a page)", () => {
  const A = r("A", 0, 0, 10, 10), B = r("B", 5, 5, 15, 15), C = r("C", 100, 100, 110, 110), D = r("D", 8, 8, 12, 12);
  const list = [A, B, C, D];
  it("front / back move the selection, keeping its own order", () => {
    expect(orderPage(list, new Set(["A", "C"]), "front")).toEqual(["B", "D", "A", "C"]);
    expect(orderPage(list, new Set(["B", "D"]), "back")).toEqual(["B", "D", "A", "C"]);
  });
  it("forward passes the next OVERLAPPING markup, skipping ones that do not overlap", () => {
    // A overlaps B (and D); C overlaps nothing. A goes above B.
    expect(orderPage(list, new Set(["A"]), "forward")).toEqual(["B", "A", "C", "D"]);
    // B overlaps D (next above, skipping C).
    expect(orderPage(list, new Set(["B"]), "forward")).toEqual(["A", "C", "D", "B"]);
  });
  it("backward is the mirror", () => {
    // D passes C (no overlap) and goes under B, the first markup below it that overlaps.
    expect(orderPage(list, new Set(["D"]), "backward")).toEqual(["A", "D", "B", "C"]);
    expect(orderPage(list, new Set(["B"]), "backward")).toEqual(["B", "A", "C", "D"]);
  });
  it("a top markup cannot go further forward; a bottom one not further back", () => {
    expect(orderPage(list, new Set(["D"]), "forward")).toEqual(["A", "B", "C", "D"]);
    expect(orderPage(list, new Set(["A"]), "backward")).toEqual(["A", "B", "C", "D"]);
  });
  it("several selected markups keep their relative order and never pass each other", () => {
    const l = [r("1", 0, 0, 10, 10), r("2", 0, 0, 10, 10), r("3", 0, 0, 10, 10), r("4", 0, 0, 10, 10)];
    expect(orderPage(l, new Set(["1", "2"]), "forward")).toEqual(["3", "1", "2", "4"]);
    expect(orderPage(l, new Set(["3", "4"]), "backward")).toEqual(["1", "3", "4", "2"]);
  });
  it("orderChanges: one change per page that moves; locked left alone and counted", () => {
    const p1 = r("P1", 0, 0, 10, 10, { page: 1 }), p1b = r("P1b", 0, 0, 10, 10, { page: 1 });
    const locked = r("L", 0, 0, 10, 10, { annot_flags: 4 | 0x80 });
    const all = [A, p1, B, p1b, locked];
    const res = orderChanges(all, new Set(["A", "P1", "L"]), "front");
    expect(res.lockedSkipped).toBe(1);
    expect(res.changes).toEqual([
      { page: 0, before: ["A", "B", "L"], after: ["B", "L", "A"] },
      { page: 1, before: ["P1", "P1b"], after: ["P1b", "P1"] },
    ]);
    expect(orderChanges(all, new Set(["P1b"]), "front").changes).toEqual([]);
    expect(orderChanges(all, new Set(["L"]), "front")).toEqual({ changes: [], lockedSkipped: 1 });
  });
});

describe("multiplyOffsets", () => {
  const unit = { minX: 100, minY: 100, maxX: 120, maxY: 120 };
  const page = { w: 600, h: 400 };
  it("a line in each cardinal direction (y is up, so Down is -y)", () => {
    const o = (direction: "right" | "left" | "up" | "down") => multiplyOffsets({ kind: "line", direction, count: 2, pitch: 50 }, unit, page).offsets;
    expect(o("right")).toEqual([{ dx: 50, dy: 0 }, { dx: 100, dy: 0 }]);
    expect(o("left")).toEqual([{ dx: -50, dy: 0 }, { dx: -100, dy: 0 }]);
    expect(o("up")).toEqual([{ dx: 0, dy: 50 }, { dx: 0, dy: 100 }]);
    expect(o("down")).toEqual([{ dx: 0, dy: -50 }, { dx: 0, dy: -100 }]);
  });
  it("stops at the page edge and reports how many were left out (Revu: only as many as fit)", () => {
    const res = multiplyOffsets({ kind: "line", direction: "right", count: 20, pitch: 100 }, unit, page);
    expect(res.offsets).toHaveLength(4); // x max 120 + 4*100 = 520 <= 600; 5*100 -> 620 > 600
    expect(res.requested).toBe(20);
    expect(res.clipped).toBe(16);
  });
  it("a grid grows right and down from the original, which is not copied", () => {
    const res = multiplyOffsets({ kind: "grid", cols: 3, rows: 2, pitchX: 30, pitchY: 40 }, unit, page);
    expect(res.offsets).toEqual([
      { dx: 30, dy: 0 }, { dx: 60, dy: 0 }, { dx: 0, dy: -40 }, { dx: 30, dy: -40 }, { dx: 60, dy: -40 },
    ]);
    expect(res.requested).toBe(5);
  });
  it("a grid skips cells that fall off the page", () => {
    const res = multiplyOffsets({ kind: "grid", cols: 2, rows: 4, pitchX: 30, pitchY: 60 }, unit, page);
    // rows 0..3 sit at dy 0, -60, -120, -180; the unit's minY is 100, so only rows 0 and 1 fit
    expect(res.offsets).toEqual([{ dx: 30, dy: 0 }, { dx: 0, dy: -60 }, { dx: 30, dy: -60 }]);
    expect(res.requested).toBe(7);
    expect(res.clipped).toBe(4);
  });
  it.each([
    ["zero count", { kind: "line", direction: "right", count: 0, pitch: 10 }],
    ["fractional count", { kind: "line", direction: "right", count: 1.5, pitch: 10 }],
    ["NaN pitch", { kind: "line", direction: "right", count: 2, pitch: NaN }],
    ["negative pitch", { kind: "line", direction: "up", count: 2, pitch: -5 }],
    ["more than the cap", { kind: "line", direction: "right", count: 10_001, pitch: 1 }],
    ["a 1x1 grid", { kind: "grid", cols: 1, rows: 1, pitchX: 1, pitchY: 1 }],
    ["a huge grid", { kind: "grid", cols: 1e9, rows: 1e9, pitchX: 1, pitchY: 1 }],
  ] as const)("refuses %s", (_n, spec) => {
    const res = multiplyOffsets(spec as never, unit, page);
    expect(res.error).toBeTruthy();
    expect(res.offsets).toEqual([]);
  });
});

describe("multiplyOffsets: centre-to-centre (default) vs gap", () => {
  const page = { w: 600, h: 400 };
  const narrow = { minX: 100, minY: 100, maxX: 110, maxY: 120 }; // 10 x 20
  const wide = { minX: 100, minY: 100, maxX: 200, maxY: 120 }; // 100 x 20
  const line = (m: "centre" | "gap" | undefined, direction: "right" | "up", pitch: number) =>
    ({ kind: "line", direction, count: 2, pitch, ...(m ? { measure: m } : {}) }) as const;

  it("centre-to-centre is the default and ignores the markup's width: same step for a narrow and a wide unit", () => {
    for (const u of [narrow, wide]) {
      expect(multiplyOffsets(line(undefined, "right", 50), u, page).offsets).toEqual([{ dx: 50, dy: 0 }, { dx: 100, dy: 0 }]);
      expect(multiplyOffsets(line("centre", "right", 50), u, page).offsets).toEqual([{ dx: 50, dy: 0 }, { dx: 100, dy: 0 }]);
    }
  });
  it("gap adds the unit's own extent along the direction: width for right/left, height for up/down", () => {
    expect(multiplyOffsets(line("gap", "right", 5), narrow, page).offsets).toEqual([{ dx: 15, dy: 0 }, { dx: 30, dy: 0 }]);
    expect(multiplyOffsets(line("gap", "right", 5), wide, page).offsets).toEqual([{ dx: 105, dy: 0 }, { dx: 210, dy: 0 }]);
    expect(multiplyOffsets(line("gap", "up", 5), wide, page).offsets).toEqual([{ dx: 0, dy: 25 }, { dx: 0, dy: 50 }]);
    const l = multiplyOffsets({ kind: "line", direction: "left", count: 1, pitch: 5, measure: "gap" }, narrow, page);
    expect(l.offsets).toEqual([{ dx: -15, dy: 0 }]);
  });
  it("gap 0 is allowed (edges touch); a negative gap is refused", () => {
    expect(multiplyOffsets(line("gap", "right", 0), narrow, page).offsets).toEqual([{ dx: 10, dy: 0 }, { dx: 20, dy: 0 }]);
    expect(multiplyOffsets(line("gap", "right", -1), narrow, page).error).toBeTruthy();
    expect(multiplyOffsets(line("centre", "right", 0), narrow, page).error).toBeTruthy();
  });
  it("a zero-size unit with gap 0 would stack the copies: refused", () => {
    const dot = { minX: 5, minY: 5, maxX: 5, maxY: 5 };
    expect(multiplyOffsets(line("gap", "right", 0), dot, page).error).toBeTruthy();
  });
  it("a grid in gap mode adds width across and height down", () => {
    const res = multiplyOffsets({ kind: "grid", cols: 2, rows: 2, pitchX: 5, pitchY: 7, measure: "gap" }, narrow, page);
    expect(res.offsets).toEqual([{ dx: 15, dy: 0 }, { dx: 0, dy: -27 }, { dx: 15, dy: -27 }]);
  });
  it("a grid in centre mode is unchanged", () => {
    const res = multiplyOffsets({ kind: "grid", cols: 2, rows: 2, pitchX: 30, pitchY: 40, measure: "centre" }, wide, page);
    expect(res.offsets).toEqual([{ dx: 30, dy: 0 }, { dx: 0, dy: -40 }, { dx: 30, dy: -40 }]);
  });
  it("fit is judged against the DISPLAY page size, so a 90/270 page (width and height swapped) clips differently", () => {
    // Portrait 600 x 400 vs the same sheet rotated 90/270 (display 400 x 600): markups live in display space.
    const spec = { kind: "line", direction: "right", count: 10, pitch: 100 } as const;
    const u = { minX: 0, minY: 0, maxX: 20, maxY: 20 };
    expect(multiplyOffsets(spec, u, { w: 600, h: 400 }).offsets).toHaveLength(5);
    expect(multiplyOffsets(spec, u, { w: 400, h: 600 }).offsets).toHaveLength(3);
  });
});

describe("align on rotated pages (display space, spec A3)", () => {
  // Markups are held in PDFium display space (rotation baked in), so Center / Middle read the screen's x / y.
  it.each([90, 270])("page rotated %i: Center gives equal x-centres, Middle equal y-centres, whatever the sizes", () => {
    const a = r("a", 10, 10, 20, 30), b = r("b", 300, 200, 340, 210), c = r("c", 55, 400, 60, 460);
    const cx = (ms: Markup[], mode: "center" | "middle") => {
      const moved = applyMoves(alignMoves(units(ms), mode), BY, NOW).pairs.map((p) => p.after).concat(ms.slice(0, 1));
      return moved.map((m) => {
        const g = (m.geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect;
        return mode === "center" ? (g.min.x + g.max.x) / 2 : (g.min.y + g.max.y) / 2;
      });
    };
    expect(new Set(cx([a, b, c], "center")).size).toBe(1);
    expect(new Set(cx([a, b, c], "middle")).size).toBe(1);
    expect(cx([a, b, c], "center")[0]).toBe(15);
    expect(cx([a, b, c], "middle")[0]).toBe(20);
  });
});

describe("spacingToPoints", () => {
  const s = scale("s", 0.05, 0, "m");
  it("points as they are; the page scale's unit through its ratio", () => {
    expect(spacingToPoints(12, "pt", null)).toBe(12);
    expect(spacingToPoints(1, "m", s)).toBeCloseTo(20, 9);
  });
  it("rejects an unknown unit, no scale, and unusable numbers", () => {
    expect(spacingToPoints(1, "m", null)).toBeNull();
    expect(spacingToPoints(1, "ft", s)).toBeNull();
    expect(spacingToPoints(0, "pt", null)).toBeNull();
    expect(spacingToPoints(NaN, "pt", null)).toBeNull();
  });
  it("zero is accepted only when asked for (a gap of 0), negatives never", () => {
    expect(spacingToPoints(0, "pt", null, true)).toBe(0);
    expect(spacingToPoints(0, "m", scale("s", 0.05, 0, "m"), true)).toBe(0);
    expect(spacingToPoints(-1, "pt", null, true)).toBeNull();
  });
});
