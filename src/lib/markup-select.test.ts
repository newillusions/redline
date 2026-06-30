/**
 * Tests for markup-select.ts: hit-testing, marquee selection, geometry transforms,
 * handle anchors, and resize math. All coordinates are PDF user space (y-up, f64).
 */
import { describe, it, expect } from "vitest";
import {
  boundsOf,
  hitTest,
  marqueeHits,
  translateGeometry,
  scaleGeometryToBounds,
  isRectResizable,
  handleAnchors,
  resizeBounds,
  expandSelectionToGroups,
  moveVertex,
  insertVertex,
  deleteVertex,
  HANDLE_IDS,
} from "./markup-select";
import type { Bounds, HandleId } from "./markup-select";
import type { Markup, MarkupGeometry, Appearance, UserRef } from "./ipc";

// ---------------------------------------------------------------------------
// Minimal fixtures
// ---------------------------------------------------------------------------
const AP: Appearance = { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null };
const USER: UserRef = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "Tester" };
const AUDIT = {
  created_by: USER, created_at: "2026-06-16T00:00:00Z",
  modified_by: USER, modified_at: "2026-06-16T00:00:00Z",
  revision: 0, origin: "Desktop" as const,
};
const WORKFLOW = { status: "None" as const, assignee: null, thread: [] };

function mkMarkup(id: string, geometry: Markup["geometry"]): Markup {
  return { id, markup_type: "Rectangle", page: 1, geometry, appearance: AP, subject: null, layer: null, contents: null, group_id: null, audit: AUDIT, workflow: WORKFLOW, measurement: null };
}

const rectMarkup = mkMarkup("r1", { Rect: { min: { x: 10, y: 20 }, max: { x: 60, y: 80 } } });
// Rect with min > max in one axis (to prove normalization)
const invertedRect = mkMarkup("r2", { Rect: { min: { x: 60, y: 80 }, max: { x: 10, y: 20 } } });
const polyMarkup = mkMarkup("p1", { Polyline: [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 40 }] });
const inkMarkup = mkMarkup("i1", { Ink: [
  [{ x: 5, y: 5 }, { x: 15, y: 25 }],
  [{ x: 100, y: 50 }, { x: 120, y: 70 }],
] });
const pointMarkup = mkMarkup("pt1", { Point: { x: 30, y: 40 } });

// ---------------------------------------------------------------------------
// boundsOf
// ---------------------------------------------------------------------------
describe("boundsOf", () => {
  it("Rect: exact min/max from geometry", () => {
    expect(boundsOf(rectMarkup)).toEqual({ minX: 10, minY: 20, maxX: 60, maxY: 80 });
  });

  it("Rect: normalizes when min > max (swaps axes)", () => {
    expect(boundsOf(invertedRect)).toEqual({ minX: 10, minY: 20, maxX: 60, maxY: 80 });
  });

  it("Polyline: AABB over vertices", () => {
    expect(boundsOf(polyMarkup)).toEqual({ minX: 0, minY: 0, maxX: 50, maxY: 40 });
  });

  it("Ink: AABB across ALL strokes' points", () => {
    expect(boundsOf(inkMarkup)).toEqual({ minX: 5, minY: 5, maxX: 120, maxY: 70 });
  });

  it("Point: zero-size bounds at the point", () => {
    expect(boundsOf(pointMarkup)).toEqual({ minX: 30, minY: 40, maxX: 30, maxY: 40 });
  });
});

// ---------------------------------------------------------------------------
// hitTest
// ---------------------------------------------------------------------------
describe("hitTest", () => {
  const tol = 5;

  it("returns id for point inside a Rect", () => {
    // interior hit
    expect(hitTest([rectMarkup], { x: 35, y: 50 }, tol)).toBe("r1");
  });

  it("returns id for point within-tol of a Rect edge", () => {
    // 3 pts outside the left edge (minX=10), within tol=5
    expect(hitTest([rectMarkup], { x: 7, y: 50 }, tol)).toBe("r1");
  });

  it("returns null when outside tol", () => {
    // 20 pts outside, well past tol
    expect(hitTest([rectMarkup], { x: -20, y: 50 }, tol)).toBeNull();
  });

  it("Polyline: hit near segment", () => {
    // segment (0,0)-(50,0): point (25, 3) is 3 pts from the segment, within tol=5
    expect(hitTest([polyMarkup], { x: 25, y: 3 }, tol)).toBe("p1");
  });

  it("Polyline: miss when too far from all segments", () => {
    expect(hitTest([polyMarkup], { x: 25, y: 20 }, tol)).toBeNull();
  });

  it("Point geometry: hit within tol", () => {
    expect(hitTest([pointMarkup], { x: 32, y: 42 }, tol)).toBe("pt1");
  });

  it("Point geometry: miss when outside tol", () => {
    expect(hitTest([pointMarkup], { x: 100, y: 100 }, tol)).toBeNull();
  });

  it("topmost-wins: returns the LATER markup's id when two Rects overlap", () => {
    const a = mkMarkup("bottom", { Rect: { min: { x: 0, y: 0 }, max: { x: 50, y: 50 } } });
    const b = mkMarkup("top",    { Rect: { min: { x: 10, y: 10 }, max: { x: 40, y: 40 } } });
    // b is later in the array => drawn on top => should win
    expect(hitTest([a, b], { x: 20, y: 20 }, tol)).toBe("top");
  });

  it("returns null for empty array", () => {
    expect(hitTest([], { x: 0, y: 0 }, tol)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// marqueeHits
// ---------------------------------------------------------------------------
describe("marqueeHits", () => {
  const fullyInside = mkMarkup("inside", { Rect: { min: { x: 20, y: 30 }, max: { x: 40, y: 60 } } });
  const partial     = mkMarkup("partial", { Rect: { min: { x: 45, y: 30 }, max: { x: 80, y: 60 } } });
  const disjoint    = mkMarkup("out",    { Rect: { min: { x: 200, y: 200 }, max: { x: 300, y: 300 } } });
  const marquee: Bounds = { minX: 0, minY: 0, maxX: 50, maxY: 100 };

  it("includes fully-contained markup", () => {
    expect(marqueeHits([fullyInside], marquee)).toContain("inside");
  });

  it("includes partially-overlapping markup", () => {
    expect(marqueeHits([partial], marquee)).toContain("partial");
  });

  it("excludes disjoint markup", () => {
    expect(marqueeHits([disjoint], marquee)).not.toContain("out");
  });

  it("preserves the markups-array order", () => {
    const hits = marqueeHits([fullyInside, partial, disjoint], marquee);
    expect(hits).toEqual(["inside", "partial"]);
  });

  it("normalizes the marquee rect defensively (min/max swapped)", () => {
    const flipped: Bounds = { minX: 50, minY: 100, maxX: 0, maxY: 0 };
    expect(marqueeHits([fullyInside], flipped)).toContain("inside");
  });
});

// ---------------------------------------------------------------------------
// translateGeometry
// ---------------------------------------------------------------------------
describe("translateGeometry", () => {
  it("translates Rect by (dx, dy)", () => {
    const g = { Rect: { min: { x: 10, y: 20 }, max: { x: 30, y: 40 } } };
    const t = translateGeometry(g, 5, -3) as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } };
    expect(t.Rect.min).toEqual({ x: 15, y: 17 });
    expect(t.Rect.max).toEqual({ x: 35, y: 37 });
  });

  it("translates Polyline verts", () => {
    const g = { Polyline: [{ x: 0, y: 0 }, { x: 10, y: 10 }] };
    const t = translateGeometry(g, 2, 3) as { Polyline: { x: number; y: number }[] };
    expect(t.Polyline).toEqual([{ x: 2, y: 3 }, { x: 12, y: 13 }]);
  });

  it("translates Ink strokes", () => {
    const g = { Ink: [[{ x: 0, y: 0 }, { x: 5, y: 5 }]] };
    const t = translateGeometry(g, 1, -1) as { Ink: { x: number; y: number }[][] };
    expect(t.Ink[0]).toEqual([{ x: 1, y: -1 }, { x: 6, y: 4 }]);
  });

  it("translates Point", () => {
    const g = { Point: { x: 10, y: 20 } };
    const t = translateGeometry(g, 5, 10) as { Point: { x: number; y: number } };
    expect(t.Point).toEqual({ x: 15, y: 30 });
  });

  it("does not mutate the input geometry", () => {
    const g = { Polyline: [{ x: 0, y: 0 }] };
    translateGeometry(g, 99, 99);
    expect(g.Polyline[0]).toEqual({ x: 0, y: 0 });
  });
});

// ---------------------------------------------------------------------------
// scaleGeometryToBounds
// ---------------------------------------------------------------------------
describe("scaleGeometryToBounds", () => {
  it("Rect doubled: corners map exactly", () => {
    const g = { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 20 } } };
    const from: Bounds = { minX: 0, minY: 0, maxX: 10, maxY: 20 };
    const to: Bounds   = { minX: 0, minY: 0, maxX: 20, maxY: 40 };
    const r = scaleGeometryToBounds(g, from, to) as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } };
    expect(r.Rect.min).toEqual({ x: 0, y: 0 });
    expect(r.Rect.max).toEqual({ x: 20, y: 40 });
  });

  it("Polyline verts remap proportionally", () => {
    const g = { Polyline: [{ x: 0, y: 0 }, { x: 10, y: 10 }] };
    const from: Bounds = { minX: 0, minY: 0, maxX: 10, maxY: 10 };
    const to: Bounds   = { minX: 5, minY: 5, maxX: 15, maxY: 15 };
    const r = scaleGeometryToBounds(g, from, to) as { Polyline: { x: number; y: number }[] };
    expect(r.Polyline[0]).toEqual({ x: 5, y: 5 });
    expect(r.Polyline[1]).toEqual({ x: 15, y: 15 });
  });

  it("degenerate: 0-width source axis maps to to.minX safely (no divide-by-zero)", () => {
    const g = { Point: { x: 5, y: 10 } };
    const from: Bounds = { minX: 5, minY: 0, maxX: 5, maxY: 10 }; // zero width
    const to: Bounds   = { minX: 20, minY: 0, maxX: 30, maxY: 10 };
    const r = scaleGeometryToBounds(g, from, to) as { Point: { x: number; y: number } };
    // degenerate x axis: maps to to.minX
    expect(r.Point.x).toBeCloseTo(20);
    // y axis non-degenerate: 10/10 of the way = to.maxY
    expect(r.Point.y).toBeCloseTo(10);
  });
});

// ---------------------------------------------------------------------------
// isRectResizable
// ---------------------------------------------------------------------------
describe("isRectResizable", () => {
  it("true for Rect geometry", () => {
    expect(isRectResizable(rectMarkup)).toBe(true);
  });
  it("false for Polyline", () => {
    expect(isRectResizable(polyMarkup)).toBe(false);
  });
  it("false for Ink", () => {
    expect(isRectResizable(inkMarkup)).toBe(false);
  });
  it("false for Point", () => {
    expect(isRectResizable(pointMarkup)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// handleAnchors (y-up: north = maxY, south = minY)
// ---------------------------------------------------------------------------
describe("handleAnchors", () => {
  const b: Bounds = { minX: 0, minY: 0, maxX: 100, maxY: 60 };

  it("HANDLE_IDS contains exactly the 8 handle names", () => {
    expect(new Set(HANDLE_IDS)).toEqual(new Set(["nw","n","ne","e","se","s","sw","w"]));
    expect(HANDLE_IDS.length).toBe(8);
  });

  it("corners are at the correct corners (PDF y-up)", () => {
    const h = handleAnchors(b);
    expect(h.nw).toEqual({ x: 0,   y: 60 }); // top-left: minX, maxY
    expect(h.ne).toEqual({ x: 100, y: 60 }); // top-right: maxX, maxY
    expect(h.se).toEqual({ x: 100, y: 0  }); // bottom-right: maxX, minY
    expect(h.sw).toEqual({ x: 0,   y: 0  }); // bottom-left: minX, minY
  });

  it("edge midpoints are correct (PDF y-up)", () => {
    const h = handleAnchors(b);
    expect(h.n).toEqual({ x: 50, y: 60 }); // top edge midpoint: midX, maxY
    expect(h.s).toEqual({ x: 50, y: 0  }); // bottom edge midpoint: midX, minY
    expect(h.e).toEqual({ x: 100, y: 30 }); // right edge midpoint: maxX, midY
    expect(h.w).toEqual({ x: 0,   y: 30 }); // left edge midpoint: minX, midY
  });
});

// ---------------------------------------------------------------------------
// resizeBounds
// ---------------------------------------------------------------------------
describe("resizeBounds", () => {
  const b: Bounds = { minX: 0, minY: 0, maxX: 100, maxY: 60 };
  const min = 10;

  it("dragging 'se' corner keeps nw fixed and moves se to p", () => {
    const r = resizeBounds(b, "se", { x: 120, y: -10 }, min);
    expect(r.minX).toBe(0);
    expect(r.minY).toBe(-10);
    expect(r.maxX).toBe(120);
    expect(r.maxY).toBe(60);
  });

  it("dragging 'nw' corner keeps se fixed and moves nw to p", () => {
    const r = resizeBounds(b, "nw", { x: -20, y: 80 }, min);
    expect(r.minX).toBe(-20);
    expect(r.minY).toBe(0);
    expect(r.maxX).toBe(100);
    expect(r.maxY).toBe(80);
  });

  it("dragging 'ne' corner keeps sw fixed", () => {
    const r = resizeBounds(b, "ne", { x: 80, y: 90 }, min);
    expect(r.minX).toBe(0);
    expect(r.minY).toBe(0);
    expect(r.maxX).toBe(80);
    expect(r.maxY).toBe(90);
  });

  it("dragging 'sw' corner keeps ne fixed", () => {
    const r = resizeBounds(b, "sw", { x: 10, y: 10 }, min);
    expect(r.minX).toBe(10);
    expect(r.minY).toBe(10);
    expect(r.maxX).toBe(100);
    expect(r.maxY).toBe(60);
  });

  it("dragging 'n' edge moves maxY only", () => {
    const r = resizeBounds(b, "n", { x: 99999, y: 90 }, min);
    expect(r.minX).toBe(0);
    expect(r.maxX).toBe(100);
    expect(r.maxY).toBe(90);
    expect(r.minY).toBe(0); // unchanged
  });

  it("dragging 's' edge moves minY only", () => {
    const r = resizeBounds(b, "s", { x: 0, y: -10 }, min);
    expect(r.minY).toBe(-10);
    expect(r.maxY).toBe(60);
    expect(r.minX).toBe(0);
    expect(r.maxX).toBe(100);
  });

  it("dragging 'e' edge moves maxX only", () => {
    const r = resizeBounds(b, "e", { x: 150, y: 999 }, min);
    expect(r.maxX).toBe(150);
    expect(r.minX).toBe(0);
  });

  it("dragging 'w' edge moves minX only", () => {
    const r = resizeBounds(b, "w", { x: -30, y: 0 }, min);
    expect(r.minX).toBe(-30);
    expect(r.maxX).toBe(100);
  });

  it("min-size clamp: dragging 'se' past origin clamps to minPts from fixed edge", () => {
    // drag se inward past nw: maxX dragged to -50, should clamp to minX + minPts = 0 + 10 = 10
    const r = resizeBounds(b, "se", { x: -50, y: 70 }, min);
    expect(r.maxX).toBe(min); // clamped: minX(0) + minPts(10)
  });

  it("min-size clamp: dragging 'n' below minY clamps height to minPts", () => {
    // n handle controls maxY; drag below minY(0): clamps to minY + minPts = 0 + 10 = 10
    const r = resizeBounds(b, "n", { x: 0, y: -50 }, min);
    expect(r.maxY).toBe(min); // clamped: minY(0) + minPts(10)
  });
});

// ---------------------------------------------------------------------------
// expandSelectionToGroups
// ---------------------------------------------------------------------------
describe("expandSelectionToGroups", () => {
  // 3-member group (gid "g1") + one loner.
  const GID = "aaaaaaaa-1111-1111-1111-aaaaaaaaaaaa";
  const m1 = { ...mkMarkup("m1", { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } }), group_id: GID };
  const m2 = { ...mkMarkup("m2", { Rect: { min: { x: 20, y: 0 }, max: { x: 30, y: 10 } } }), group_id: GID };
  const m3 = { ...mkMarkup("m3", { Rect: { min: { x: 40, y: 0 }, max: { x: 50, y: 10 } } }), group_id: GID };
  const loner = mkMarkup("loner", { Rect: { min: { x: 100, y: 0 }, max: { x: 110, y: 10 } } });
  const allMarkups = [m1, m2, m3, loner];

  it("selecting one member of a 3-member group returns all 3 ids", () => {
    const result = expandSelectionToGroups(allMarkups, new Set(["m1"]));
    expect(result.has("m1")).toBe(true);
    expect(result.has("m2")).toBe(true);
    expect(result.has("m3")).toBe(true);
    expect(result.size).toBe(3);
  });

  it("selecting an ungrouped markup returns just that id", () => {
    const result = expandSelectionToGroups(allMarkups, new Set(["loner"]));
    expect(result.has("loner")).toBe(true);
    expect(result.size).toBe(1);
  });

  it("mixed selection (one grouped + one ungrouped) returns the full group plus the loner", () => {
    const result = expandSelectionToGroups(allMarkups, new Set(["m2", "loner"]));
    expect(result.has("m1")).toBe(true);
    expect(result.has("m2")).toBe(true);
    expect(result.has("m3")).toBe(true);
    expect(result.has("loner")).toBe(true);
    expect(result.size).toBe(4);
  });

  it("empty input returns an empty set", () => {
    const result = expandSelectionToGroups(allMarkups, new Set());
    expect(result.size).toBe(0);
  });

  it("does not mutate the passed-in set", () => {
    const original = new Set(["m1"]);
    expandSelectionToGroups(allMarkups, original);
    expect(original.size).toBe(1); // unchanged
  });
});

// ---------------------------------------------------------------------------
// Per-vertex editing: moveVertex / insertVertex / deleteVertex
// ---------------------------------------------------------------------------

const poly = (pts: { x: number; y: number }[]): MarkupGeometry => ({ Polyline: pts });

describe("moveVertex", () => {
  const g = poly([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }, { x: 0, y: 10 }]);

  it("moves an interior vertex, leaving the others unchanged", () => {
    const out = moveVertex(g, 1, { x: 99, y: 88 });
    const pts = (out as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(pts[1]).toEqual({ x: 99, y: 88 });
    expect(pts[0]).toEqual({ x: 0, y: 0 });
    expect(pts[2]).toEqual({ x: 10, y: 10 });
    expect(pts[3]).toEqual({ x: 0, y: 10 });
  });

  it("moves the first vertex", () => {
    const pts = (moveVertex(g, 0, { x: -5, y: -5 }) as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(pts[0]).toEqual({ x: -5, y: -5 });
    expect(pts[1]).toEqual({ x: 10, y: 0 });
  });

  it("moves the last vertex", () => {
    const pts = (moveVertex(g, 3, { x: 7, y: 7 }) as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(pts[3]).toEqual({ x: 7, y: 7 });
    expect(pts[2]).toEqual({ x: 10, y: 10 });
  });

  it("does not mutate the input geometry", () => {
    const before = JSON.stringify(g);
    moveVertex(g, 1, { x: 1, y: 1 });
    expect(JSON.stringify(g)).toBe(before);
  });

  it("out-of-range index is a no-op (returns equivalent geometry)", () => {
    expect(moveVertex(g, 9, { x: 1, y: 1 })).toEqual(g);
    expect(moveVertex(g, -1, { x: 1, y: 1 })).toEqual(g);
  });

  it("non-Polyline geometry is returned unchanged", () => {
    const rect: MarkupGeometry = { Rect: { min: { x: 0, y: 0 }, max: { x: 1, y: 1 } } };
    expect(moveVertex(rect, 0, { x: 9, y: 9 })).toBe(rect);
  });
});

describe("insertVertex", () => {
  const g = poly([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }]);

  it("inserts a vertex into the middle of a segment at index segmentIndex+1", () => {
    const pts = (insertVertex(g, 0, { x: 5, y: 0 }) as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(pts).toHaveLength(4);
    expect(pts[0]).toEqual({ x: 0, y: 0 });
    expect(pts[1]).toEqual({ x: 5, y: 0 }); // new vertex lands at index 1
    expect(pts[2]).toEqual({ x: 10, y: 0 });
  });

  it("inserting on the last segment (closing segment) appends the point", () => {
    const pts = (insertVertex(g, 2, { x: 5, y: 5 }) as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(pts).toHaveLength(4);
    expect(pts[3]).toEqual({ x: 5, y: 5 });
  });

  it("out-of-range segment is a no-op", () => {
    expect(insertVertex(g, 5, { x: 1, y: 1 })).toEqual(g);
    expect(insertVertex(g, -1, { x: 1, y: 1 })).toEqual(g);
  });

  it("does not mutate the input geometry", () => {
    const before = JSON.stringify(g);
    insertVertex(g, 0, { x: 5, y: 0 });
    expect(JSON.stringify(g)).toBe(before);
  });
});

describe("deleteVertex", () => {
  it("removes an interior vertex above the floor", () => {
    const g = poly([{ x: 0, y: 0 }, { x: 5, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }]);
    const pts = (deleteVertex(g, 1, 2) as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(pts).toHaveLength(3);
    expect(pts[0]).toEqual({ x: 0, y: 0 });
    expect(pts[1]).toEqual({ x: 10, y: 0 });
  });

  it("removes the first and last vertices", () => {
    const g = poly([{ x: 0, y: 0 }, { x: 5, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }]);
    expect((deleteVertex(g, 0, 2) as { Polyline: unknown[] }).Polyline).toHaveLength(3);
    expect((deleteVertex(g, 3, 2) as { Polyline: unknown[] }).Polyline).toHaveLength(3);
  });

  it("no-ops at the open-line floor (≥2)", () => {
    const g = poly([{ x: 0, y: 0 }, { x: 10, y: 0 }]);
    expect(deleteVertex(g, 0, 2)).toEqual(g); // already at the floor → unchanged
  });

  it("no-ops at the closed-polygon floor (≥3)", () => {
    const g = poly([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 5, y: 10 }]);
    expect(deleteVertex(g, 1, 3)).toEqual(g); // already at the floor → unchanged
  });

  it("allows deletion down to (but not below) the floor", () => {
    const g = poly([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 5, y: 10 }]);
    const pts = (deleteVertex(g, 2, 2) as { Polyline: unknown[] }).Polyline; // 3 → 2, floor 2 ok
    expect(pts).toHaveLength(2);
  });

  it("does not mutate the input geometry", () => {
    const g = poly([{ x: 0, y: 0 }, { x: 5, y: 0 }, { x: 10, y: 0 }]);
    const before = JSON.stringify(g);
    deleteVertex(g, 1, 2);
    expect(JSON.stringify(g)).toBe(before);
  });
});
