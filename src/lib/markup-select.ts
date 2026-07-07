/**
 * Pure selection helpers: hit-testing, marquee selection, geometry transforms,
 * resize-handle anchors, and bounds-resize math. All coordinates are PDF user
 * space (y-up, origin bottom-left, f64). No DOM, no Svelte, no clocks/UUIDs.
 */
import type { Markup, MarkupGeometry, PdfPoint } from "./ipc";

// ---------------------------------------------------------------------------
// Bounds + handle types
// ---------------------------------------------------------------------------

/** Axis-aligned bounding box in PDF user space. */
export interface Bounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

/**
 * The 8 resize handle identifiers.
 * y-up convention: "n" (north) = top = maxY, "s" (south) = bottom = minY.
 */
export type HandleId = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";

/** The 8 handle ids in declaration order. */
export const HANDLE_IDS: readonly HandleId[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

// ---------------------------------------------------------------------------
// expandSelectionToGroups
// ---------------------------------------------------------------------------

/**
 * Given a set of selected markup ids, expand to include ALL markups that share
 * a non-null `group_id` with any selected member. Markups with `group_id == null`
 * contribute only themselves. Returns a new Set; never mutates `ids`.
 */
export function expandSelectionToGroups(markups: Markup[], ids: Set<string>): Set<string> {
  if (ids.size === 0) return new Set();

  // Collect group ids from the selected markups.
  const groupIds = new Set<string>();
  for (const m of markups) {
    if (ids.has(m.id) && m.group_id !== null) {
      groupIds.add(m.group_id);
    }
  }

  if (groupIds.size === 0) {
    // No groups involved — return a copy of the original set.
    return new Set(ids);
  }

  // Expand: include original ids plus all markups sharing any of those group ids.
  const expanded = new Set(ids);
  for (const m of markups) {
    if (m.group_id !== null && groupIds.has(m.group_id)) {
      expanded.add(m.id);
    }
  }
  return expanded;
}

// ---------------------------------------------------------------------------
// boundsOf
// ---------------------------------------------------------------------------

/** Compute the AABB for any markup geometry (in PDF user space). */
export function boundsOf(m: Markup): Bounds {
  const g = m.geometry;
  if ("Rect" in g) {
    return {
      minX: Math.min(g.Rect.min.x, g.Rect.max.x),
      minY: Math.min(g.Rect.min.y, g.Rect.max.y),
      maxX: Math.max(g.Rect.min.x, g.Rect.max.x),
      maxY: Math.max(g.Rect.min.y, g.Rect.max.y),
    };
  }
  if ("Polyline" in g) return _boundsOfPoints(g.Polyline);
  if ("Ink" in g) return _boundsOfPoints(g.Ink.flat());
  if ("Quads" in g) return _boundsOfPoints(g.Quads.flat());
  // Point
  return { minX: g.Point.x, minY: g.Point.y, maxX: g.Point.x, maxY: g.Point.y };
}

function _boundsOfPoints(pts: PdfPoint[]): Bounds {
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const p of pts) {
    if (p.x < minX) minX = p.x;
    if (p.y < minY) minY = p.y;
    if (p.x > maxX) maxX = p.x;
    if (p.y > maxY) maxY = p.y;
  }
  return { minX, minY, maxX, maxY };
}

// ---------------------------------------------------------------------------
// hitTest
// ---------------------------------------------------------------------------

/**
 * Return the id of the topmost markup at `p` (within `tolPts` tolerance),
 * or null if none. Iterates in reverse (last = drawn on top).
 *
 * - Rect: hit when inside or within tolPts of the AABB.
 * - Polyline/Ink: hit when min distance to any segment <= tolPts.
 * - Point: hit when within the axis-aligned bounding box of size tolPts around
 *   the point — Chebyshev (L∞) distance rather than Euclidean, so that clicks
 *   anywhere inside the rendered symbol footprint register (including the
 *   corners of non-circular symbols such as Cross, Square, and Diamond, which
 *   extend to ~1.4 × tolPts from centre under a Euclidean measure).
 */
export function hitTest(markups: Markup[], p: PdfPoint, tolPts: number): string | null {
  for (let i = markups.length - 1; i >= 0; i--) {
    if (_hits(markups[i], p, tolPts)) return markups[i].id;
  }
  return null;
}

function _hits(m: Markup, p: PdfPoint, tol: number): boolean {
  const g = m.geometry;
  if ("Rect" in g) {
    const b = boundsOf(m);
    return (
      p.x >= b.minX - tol && p.x <= b.maxX + tol &&
      p.y >= b.minY - tol && p.y <= b.maxY + tol
    );
  }
  if ("Polyline" in g) {
    const verts = g.Polyline;
    for (let i = 0; i < verts.length - 1; i++) {
      if (_segDistance(p, verts[i], verts[i + 1]) <= tol) return true;
    }
    return false;
  }
  if ("Ink" in g) {
    for (const stroke of g.Ink) {
      for (let i = 0; i < stroke.length - 1; i++) {
        if (_segDistance(p, stroke[i], stroke[i + 1]) <= tol) return true;
      }
    }
    return false;
  }
  if ("Quads" in g) {
    // Hit when inside (or within tol of) any single quad's axis-aligned rect -
    // quads are always axis-aligned (they come from PDFium's per-line text rects).
    for (const q of g.Quads) {
      const xs = q.map((pt) => pt.x);
      const ys = q.map((pt) => pt.y);
      const minX = Math.min(...xs), maxX = Math.max(...xs);
      const minY = Math.min(...ys), maxY = Math.max(...ys);
      if (p.x >= minX - tol && p.x <= maxX + tol && p.y >= minY - tol && p.y <= maxY + tol) {
        return true;
      }
    }
    return false;
  }
  // Point (count markers): bounding-box (Chebyshev) test so every pixel inside
  // the rendered symbol footprint registers as a hit regardless of shape.
  return Math.abs(p.x - g.Point.x) <= tol && Math.abs(p.y - g.Point.y) <= tol;
}

/** Minimum distance from point `p` to line segment `a`-`b`. */
function _segDistance(p: PdfPoint, a: PdfPoint, b: PdfPoint): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const lenSq = dx * dx + dy * dy;
  if (lenSq === 0) return _dist(p, a);
  const t = Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / lenSq));
  return _dist(p, { x: a.x + t * dx, y: a.y + t * dy });
}

function _dist(a: PdfPoint, b: PdfPoint): number {
  const dx = a.x - b.x;
  const dy = a.y - b.y;
  return Math.sqrt(dx * dx + dy * dy);
}

// ---------------------------------------------------------------------------
// marqueeHits
// ---------------------------------------------------------------------------

/**
 * Return the ids of all markups whose AABB overlaps `rect` (inclusive AABB
 * intersection). Preserves markups array order. Normalizes the marquee rect
 * defensively in case min/max are swapped.
 */
export function marqueeHits(markups: Markup[], rect: Bounds): string[] {
  const r = _normalizeRect(rect);
  return markups
    .filter((m) => {
      const b = boundsOf(m);
      return b.maxX >= r.minX && b.minX <= r.maxX &&
             b.maxY >= r.minY && b.minY <= r.maxY;
    })
    .map((m) => m.id);
}

function _normalizeRect(b: Bounds): Bounds {
  return {
    minX: Math.min(b.minX, b.maxX),
    minY: Math.min(b.minY, b.maxY),
    maxX: Math.max(b.minX, b.maxX),
    maxY: Math.max(b.minY, b.maxY),
  };
}

// ---------------------------------------------------------------------------
// translateGeometry
// ---------------------------------------------------------------------------

/** Return a new MarkupGeometry shifted by (dx, dy). Does not mutate input. */
export function translateGeometry(g: MarkupGeometry, dx: number, dy: number): MarkupGeometry {
  if ("Rect" in g) {
    return {
      Rect: {
        min: { x: g.Rect.min.x + dx, y: g.Rect.min.y + dy },
        max: { x: g.Rect.max.x + dx, y: g.Rect.max.y + dy },
      },
    };
  }
  if ("Polyline" in g) {
    return { Polyline: g.Polyline.map((p) => ({ x: p.x + dx, y: p.y + dy })) };
  }
  if ("Ink" in g) {
    return { Ink: g.Ink.map((s) => s.map((p) => ({ x: p.x + dx, y: p.y + dy }))) };
  }
  if ("Quads" in g) {
    return { Quads: g.Quads.map((q) => _mapQuad(q, (p) => ({ x: p.x + dx, y: p.y + dy }))) };
  }
  return { Point: { x: g.Point.x + dx, y: g.Point.y + dy } };
}

/** Map a 4-point Quad through `fn`, preserving the tuple type (no `as` cast). */
function _mapQuad(
  q: [PdfPoint, PdfPoint, PdfPoint, PdfPoint],
  fn: (p: PdfPoint) => PdfPoint,
): [PdfPoint, PdfPoint, PdfPoint, PdfPoint] {
  return [fn(q[0]), fn(q[1]), fn(q[2]), fn(q[3])];
}

// ---------------------------------------------------------------------------
// Per-vertex editing (multipoint markups: Polyline/Polygon/Cloud/Arrow/Measurement)
// ---------------------------------------------------------------------------

/**
 * Return a new geometry with the vertex at `index` replaced by `newPoint`. Only
 * Polyline geometry has movable vertices; any other geometry (or an out-of-range
 * index) is returned unchanged. Never mutates the input.
 */
export function moveVertex(g: MarkupGeometry, index: number, newPoint: PdfPoint): MarkupGeometry {
  if (!("Polyline" in g)) return g;
  if (index < 0 || index >= g.Polyline.length) return g;
  return {
    Polyline: g.Polyline.map((p, i) =>
      i === index ? { x: newPoint.x, y: newPoint.y } : { x: p.x, y: p.y },
    ),
  };
}

/**
 * Insert `point` into a Polyline as a new vertex on segment `segmentIndex` (the
 * segment from vertex `segmentIndex` to `segmentIndex + 1`). The new vertex lands
 * at index `segmentIndex + 1`. For a closed shape, `segmentIndex === length - 1`
 * is the closing segment (last → first); the point is appended. Non-Polyline
 * geometry or an out-of-range segment is returned unchanged. Never mutates input.
 */
export function insertVertex(g: MarkupGeometry, segmentIndex: number, point: PdfPoint): MarkupGeometry {
  if (!("Polyline" in g)) return g;
  const pts = g.Polyline;
  if (segmentIndex < 0 || segmentIndex >= pts.length) return g;
  const copy = pts.map((p) => ({ x: p.x, y: p.y }));
  copy.splice(segmentIndex + 1, 0, { x: point.x, y: point.y });
  return { Polyline: copy };
}

/**
 * Remove the vertex at `index` from a Polyline, unless doing so would drop the
 * point count below `minPoints` (the floor: 2 for open lines/arrows, 3 for closed
 * polygons/clouds). At or below the floor this is a no-op. Non-Polyline geometry or
 * an out-of-range index is returned unchanged. Never mutates input.
 */
export function deleteVertex(g: MarkupGeometry, index: number, minPoints: number): MarkupGeometry {
  if (!("Polyline" in g)) return g;
  const pts = g.Polyline;
  if (pts.length <= minPoints) return g;
  if (index < 0 || index >= pts.length) return g;
  return { Polyline: pts.filter((_, i) => i !== index).map((p) => ({ x: p.x, y: p.y })) };
}

// ---------------------------------------------------------------------------
// scaleGeometryToBounds
// ---------------------------------------------------------------------------

/**
 * Remap every point in `g` from `from` bounds to `to` bounds proportionally.
 * If `from` width or height is 0, that axis maps to the corresponding `to.min*`
 * (avoids divide-by-zero). Returns a new geometry; does not mutate input.
 */
export function scaleGeometryToBounds(g: MarkupGeometry, from: Bounds, to: Bounds): MarkupGeometry {
  const scaleP = (p: PdfPoint): PdfPoint => _scalePoint(p, from, to);
  if ("Rect" in g) {
    return { Rect: { min: scaleP(g.Rect.min), max: scaleP(g.Rect.max) } };
  }
  if ("Polyline" in g) {
    return { Polyline: g.Polyline.map(scaleP) };
  }
  if ("Ink" in g) {
    return { Ink: g.Ink.map((s) => s.map(scaleP)) };
  }
  if ("Quads" in g) {
    return { Quads: g.Quads.map((q) => _mapQuad(q, scaleP)) };
  }
  return { Point: scaleP(g.Point) };
}

function _scalePoint(p: PdfPoint, from: Bounds, to: Bounds): PdfPoint {
  const fromW = from.maxX - from.minX;
  const fromH = from.maxY - from.minY;
  const toW   = to.maxX - to.minX;
  const toH   = to.maxY - to.minY;
  const nx = fromW === 0 ? to.minX : to.minX + (p.x - from.minX) * (toW / fromW);
  const ny = fromH === 0 ? to.minY : to.minY + (p.y - from.minY) * (toH / fromH);
  return { x: nx, y: ny };
}

// ---------------------------------------------------------------------------
// isRectResizable
// ---------------------------------------------------------------------------

/** True when the markup's geometry is a Rect (and thus has 8 resize handles). */
export function isRectResizable(m: Markup): boolean {
  return "Rect" in m.geometry;
}

// ---------------------------------------------------------------------------
// handleAnchors
// ---------------------------------------------------------------------------

/**
 * Return the 8 resize-handle anchor points for `b` in PDF user space.
 *
 * y-up axis convention (origin bottom-left):
 *   "n" (north/top) = maxY   "s" (south/bottom) = minY
 *   "e" (east/right) = maxX  "w" (west/left)  = minX
 */
export function handleAnchors(b: Bounds): Record<HandleId, PdfPoint> {
  const midX = (b.minX + b.maxX) / 2;
  const midY = (b.minY + b.maxY) / 2;
  return {
    nw: { x: b.minX, y: b.maxY },
    n:  { x: midX,   y: b.maxY },
    ne: { x: b.maxX, y: b.maxY },
    e:  { x: b.maxX, y: midY   },
    se: { x: b.maxX, y: b.minY },
    s:  { x: midX,   y: b.minY },
    sw: { x: b.minX, y: b.minY },
    w:  { x: b.minX, y: midY   },
  };
}

// ---------------------------------------------------------------------------
// resizeBounds
// ---------------------------------------------------------------------------

/**
 * Drag `handle` to `p`, moving the corresponding edge(s) while keeping the
 * opposite edge/corner fixed. Clamps so the result is at least `minPts` in
 * each dimension (no negative or flipped bounds).
 *
 * Edge handles (n/e/s/w) move ONE axis only (the other axis component of `p`
 * is ignored). Corner handles (nw/ne/se/sw) move two axes.
 */
export function resizeBounds(b: Bounds, handle: HandleId, p: PdfPoint, minPts: number): Bounds {
  let { minX, minY, maxX, maxY } = b;

  // Edges that move with each handle (fixed edge is the opposite)
  const movesMaxX = handle === "ne" || handle === "e" || handle === "se";
  const movesMinX = handle === "nw" || handle === "w" || handle === "sw";
  const movesMaxY = handle === "nw" || handle === "n" || handle === "ne";
  const movesMinY = handle === "sw" || handle === "s" || handle === "se";

  if (movesMaxX) maxX = Math.max(minX + minPts, p.x);
  if (movesMinX) minX = Math.min(maxX - minPts, p.x);
  if (movesMaxY) maxY = Math.max(minY + minPts, p.y);
  if (movesMinY) minY = Math.min(maxY - minPts, p.y);

  return { minX, minY, maxX, maxY };
}
