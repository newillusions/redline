import { describe, it, expect } from "vitest";
import { canTurn, pathLength, pathOfMarkup, planAlong, pointAlong, rotateGeometry, rotatePoint } from "./path-array";
import type { MarkupGeometry } from "./ipc";

const P = (x: number, y: number) => ({ x, y });
const open = { points: [P(0, 0), P(100, 0)], closed: false };
const L = { points: [P(0, 0), P(100, 0), P(100, 50)], closed: false }; // length 150
const square = { points: [P(0, 0), P(10, 0), P(10, 10), P(0, 10)], closed: true }; // length 40

const close = (a: number, b: number, d = 1e-9) => expect(Math.abs(a - b)).toBeLessThan(d);

describe("pathOfMarkup", () => {
  const poly = (markup_type: string, pts: [number, number][]) => ({
    markup_type: markup_type as never,
    geometry: { Polyline: pts.map(([x, y]) => P(x, y)) } as MarkupGeometry,
  });
  it("lines and polylines are open; polygons and clouds are closed", () => {
    expect(pathOfMarkup(poly("Line", [[0, 0], [5, 0]]))?.closed).toBe(false);
    expect(pathOfMarkup(poly("Polyline", [[0, 0], [5, 0], [5, 5]]))?.closed).toBe(false);
    expect(pathOfMarkup(poly("Polygon", [[0, 0], [5, 0], [5, 5]]))?.closed).toBe(true);
    expect(pathOfMarkup(poly("Cloud", [[0, 0], [5, 0], [5, 5]]))?.closed).toBe(true);
  });
  it("a one-point polyline is not a path", () => {
    expect(pathOfMarkup(poly("Polyline", [[0, 0]]))).toBeNull();
  });
  it("rectangle is its four corners; ellipse is traced closed with the right perimeter", () => {
    const r = pathOfMarkup({ markup_type: "Rectangle", geometry: { Rect: { min: P(0, 0), max: P(20, 10) } } });
    expect(r?.points).toHaveLength(4);
    expect(pathLength(r!.points, true)).toBe(60);
    const e = pathOfMarkup({ markup_type: "Ellipse", geometry: { Rect: { min: P(-10, -10), max: P(10, 10) } } });
    expect(e?.closed).toBe(true);
    // 96-gon inscribed in r=10: just under 2*pi*10
    expect(pathLength(e!.points, true)).toBeGreaterThan(62.7);
    expect(pathLength(e!.points, true)).toBeLessThan(2 * Math.PI * 10);
  });
  it("ink: a single stroke only; text, counts and stamps are not paths", () => {
    expect(pathOfMarkup({ markup_type: "Ink", geometry: { Ink: [[P(0, 0), P(1, 1)]] } })?.closed).toBe(false);
    expect(pathOfMarkup({ markup_type: "Ink", geometry: { Ink: [[P(0, 0), P(1, 1)], [P(2, 2), P(3, 3)]] } })).toBeNull();
    expect(pathOfMarkup({ markup_type: "Text", geometry: { Rect: { min: P(0, 0), max: P(5, 5) } } })).toBeNull();
    expect(pathOfMarkup({ markup_type: "MeasurementCount", geometry: { Point: P(1, 1) } })).toBeNull();
  });
});

describe("pathLength / pointAlong", () => {
  it("measures open and closed paths", () => {
    expect(pathLength(L.points, false)).toBe(150);
    expect(pathLength(square.points, true)).toBe(40);
  });
  it("walks round a corner and reports the direction (radians, ccw from east)", () => {
    const a = pointAlong(L.points, false, 50);
    expect(a.point).toEqual(P(50, 0));
    close(a.angle, 0);
    const b = pointAlong(L.points, false, 125);
    expect(b.point).toEqual(P(100, 25));
    close(b.angle, Math.PI / 2);
  });
  it("clamps beyond the end and wraps the closing edge of a closed path", () => {
    expect(pointAlong(L.points, false, 999).point).toEqual(P(100, 50));
    const c = pointAlong(square.points, true, 35);
    expect(c.point).toEqual(P(0, 5));
    close(c.angle, -Math.PI / 2);
  });
});

describe("planAlong", () => {
  it("divide: N parts gives N-1 interior marks on an open path", () => {
    const r = planAlong(open, { kind: "divide", count: 4 }, 100);
    expect(r.marks.map((m) => m.point.x)).toEqual([25, 50, 75]);
  });
  it("divide: a closed path gets N marks starting at its first point", () => {
    const r = planAlong(square, { kind: "divide", count: 4 }, 100);
    expect(r.marks.map((m) => m.point)).toEqual([P(0, 0), P(10, 0), P(10, 10), P(0, 10)]);
  });
  it("divide needs 2 or more whole parts", () => {
    expect(planAlong(open, { kind: "divide", count: 1 }, 100).error).toBeTruthy();
    expect(planAlong(open, { kind: "divide", count: 2.5 }, 100).error).toBeTruthy();
  });
  it("measure: every spacing from the start, skipping the start and the exact end on an open path", () => {
    expect(planAlong(open, { kind: "measure", spacing: 30 }, 100).marks.map((m) => m.point.x)).toEqual([30, 60, 90]);
    expect(planAlong(open, { kind: "measure", spacing: 50 }, 100).marks.map((m) => m.point.x)).toEqual([50]);
  });
  it("measure: a closed path marks its first point too", () => {
    expect(planAlong(square, { kind: "measure", spacing: 10 }, 100).marks).toHaveLength(4);
  });
  it("measure refuses a non-positive spacing and an over-limit result before generating it", () => {
    expect(planAlong(open, { kind: "measure", spacing: 0 }, 100).error).toBeTruthy();
    expect(planAlong(open, { kind: "measure", spacing: -3 }, 100).error).toBeTruthy();
    expect(planAlong(open, { kind: "measure", spacing: 1e-3 }, 100).error).toMatch(/limit/);
  });
  it("array: N marks from start to end inclusive on an open path", () => {
    expect(planAlong(open, { kind: "array", count: 5 }, 100).marks.map((m) => m.point.x)).toEqual([0, 25, 50, 75, 100]);
    expect(planAlong(open, { kind: "array", count: 1 }, 100).marks.map((m) => m.point.x)).toEqual([0]);
  });
  it("array: round a closed loop without repeating the start", () => {
    const r = planAlong(square, { kind: "array", count: 8 }, 100);
    expect(r.marks).toHaveLength(8);
    expect(r.marks[1].point).toEqual(P(5, 0));
  });
  it("array refuses more than the limit; a zero-length path is refused", () => {
    expect(planAlong(open, { kind: "array", count: 101 }, 100).error).toMatch(/At most/);
    expect(planAlong({ points: [P(1, 1), P(1, 1)], closed: false }, { kind: "array", count: 3 }, 100).error).toBeTruthy();
  });
  it("reports the start direction for aligning", () => {
    close(planAlong({ points: [P(0, 0), P(0, 10)], closed: false }, { kind: "array", count: 2 }, 10).startAngle, Math.PI / 2);
  });
});

describe("rotation", () => {
  it("rotatePoint is counter-clockwise about a centre", () => {
    const p = rotatePoint(P(11, 5), P(10, 5), Math.PI / 2);
    close(p.x, 10);
    close(p.y, 6);
  });
  it("turns polylines, ink and points; boxes and quads do not turn", () => {
    expect(rotateGeometry({ Polyline: [P(1, 0)] }, P(0, 0), Math.PI)).toMatchObject({ Polyline: [{ x: -1 }] });
    expect(rotateGeometry({ Ink: [[P(1, 0)]] }, P(0, 0), 0)).not.toBeNull();
    expect(rotateGeometry({ Point: P(3, 4) }, P(3, 4), 1)).toEqual({ Point: P(3, 4) });
    expect(rotateGeometry({ Rect: { min: P(0, 0), max: P(1, 1) } }, P(0, 0), 1)).toBeNull();
  });
  it("canTurn excludes callouts and box markups", () => {
    expect(canTurn({ markup_type: "Arrow", geometry: { Polyline: [P(0, 0), P(1, 0)] } })).toBe(true);
    expect(canTurn({ markup_type: "Callout", geometry: { Polyline: [P(0, 0), P(1, 0)] } })).toBe(false);
    expect(canTurn({ markup_type: "Rectangle", geometry: { Rect: { min: P(0, 0), max: P(1, 1) } } })).toBe(false);
  });
});
