// Adapted from redcolumn (Apache-2.0), packages/markup/src/cad.ts, modified
/**
 * Divide, Measure and Path Array: placing items evenly along a line, polyline, polygon,
 * rectangle, ellipse or freehand stroke (docs/arrange.md section 6). Pure: geometry in,
 * placements out. No DOM, no Svelte, no store.
 *
 * Coordinates are PDF user space (y up), so a direction angle is in radians counter-clockwise
 * from east (redcolumn works in a y-down page space with degrees clockwise; the maths is the
 * same, the sign convention is converted here).
 */
import type { Markup, MarkupGeometry, PdfPoint } from "./ipc";
import { isClosedMarkupType } from "./markup-render";

const EPS = 1e-6;
/** Segments used to trace an ellipse's outline as a path. */
const ELLIPSE_STEPS = 96;

export interface PathOf {
  points: PdfPoint[];
  closed: boolean;
}

/**
 * The path a markup runs along, or null when it is not a line or outline. Lines, arrows,
 * polylines and polygons (clouds, measurement outlines) use their vertices; a rectangle is its
 * four corners; an ellipse is traced; a freehand stroke is its single stroke. Text, stamps,
 * highlights, counts and the like are not paths.
 */
export function pathOfMarkup(m: Pick<Markup, "markup_type" | "geometry">): PathOf | null {
  const g = m.geometry;
  if ("Polyline" in g) {
    const closed = isClosedMarkupType(m.markup_type) && g.Polyline.length >= 3;
    return g.Polyline.length >= 2 ? { points: g.Polyline.map(copy), closed } : null;
  }
  if ("Ink" in g) {
    return g.Ink.length === 1 && g.Ink[0].length >= 2 ? { points: g.Ink[0].map(copy), closed: false } : null;
  }
  if ("Rect" in g && (m.markup_type === "Rectangle" || m.markup_type === "Ellipse")) {
    const { min, max } = g.Rect;
    if (!(max.x - min.x > EPS) || !(max.y - min.y > EPS)) return null;
    if (m.markup_type === "Rectangle") {
      return {
        points: [{ x: min.x, y: min.y }, { x: max.x, y: min.y }, { x: max.x, y: max.y }, { x: min.x, y: max.y }],
        closed: true,
      };
    }
    const cx = (min.x + max.x) / 2;
    const cy = (min.y + max.y) / 2;
    const rx = (max.x - min.x) / 2;
    const ry = (max.y - min.y) / 2;
    const points: PdfPoint[] = [];
    for (let k = 0; k < ELLIPSE_STEPS; k++) {
      const t = (k / ELLIPSE_STEPS) * Math.PI * 2;
      points.push({ x: cx + rx * Math.cos(t), y: cy + ry * Math.sin(t) });
    }
    return { points, closed: true };
  }
  return null;
}

const copy = (p: PdfPoint): PdfPoint => ({ x: p.x, y: p.y });
const dist = (a: PdfPoint, b: PdfPoint) => Math.hypot(b.x - a.x, b.y - a.y);

/** Total length of a path (a closed path includes the edge back to its start). */
export function pathLength(points: readonly PdfPoint[], closed: boolean): number {
  let l = 0;
  const n = closed ? points.length : points.length - 1;
  for (let i = 0; i < n; i++) l += dist(points[i], points[(i + 1) % points.length]);
  return l;
}

export interface Along {
  point: PdfPoint;
  /** Direction of travel there, radians counter-clockwise from east. */
  angle: number;
}

/** Where a path is `s` along it, and its direction there. */
export function pointAlong(points: readonly PdfPoint[], closed: boolean, s: number): Along {
  const n = closed ? points.length : points.length - 1;
  let left = s;
  for (let i = 0; i < n; i++) {
    const a = points[i];
    const b = points[(i + 1) % points.length];
    const l = dist(a, b);
    if (left <= l + EPS || i === n - 1) {
      const t = l ? Math.min(1, Math.max(0, left / l)) : 0;
      return { point: { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t }, angle: Math.atan2(b.y - a.y, b.x - a.x) };
    }
    left -= l;
  }
  return { point: copy(points[0] ?? { x: 0, y: 0 }), angle: 0 };
}

export type AlongSpec =
  /** Divide: split the path into `count` equal parts; marks at the division points between
   *  them (an open path has `count - 1`; a closed one has `count`, starting at its first point). */
  | { kind: "divide"; count: number }
  /** Measure: a mark every `spacing` PDF pt from the start (an open path's start is skipped;
   *  a closed path's first point is marked). */
  | { kind: "measure"; spacing: number }
  /** Path Array: `count` marks spread evenly from the path's start to its end (a closed path:
   *  round the loop without repeating the start). */
  | { kind: "array"; count: number };

export interface AlongPlan {
  marks: Along[];
  /** Direction at the path's start, the reference for turning items along the path. */
  startAngle: number;
  error?: string;
}

const wholeAtLeast = (n: unknown, min: number): n is number => typeof n === "number" && Number.isInteger(n) && n >= min;

/**
 * The marks `spec` puts along a path. Asking for more than `maxItems` marks is refused up
 * front, before anything is generated.
 */
export function planAlong(path: PathOf, spec: AlongSpec, maxItems: number): AlongPlan {
  const bad = (error: string): AlongPlan => ({ marks: [], startAngle: 0, error });
  const { points, closed } = path;
  const total = pathLength(points, closed);
  if (!(total > EPS)) return bad("The path has no length.");
  const start = pointAlong(points, closed, 0);
  const at: number[] = [];
  if (spec.kind === "divide") {
    if (!wholeAtLeast(spec.count, 2)) return bad("Enter a whole number of parts, 2 or more.");
    if (spec.count > maxItems + 1) return bad(`At most ${maxItems} marks at a time.`);
    for (let k = closed ? 0 : 1; k < spec.count; k++) at.push((total * k) / spec.count);
  } else if (spec.kind === "measure") {
    if (!(typeof spec.spacing === "number" && Number.isFinite(spec.spacing) && spec.spacing > EPS)) {
      return bad("Enter a distance above zero.");
    }
    const expected = Math.floor((total - EPS) / spec.spacing) + (closed ? 1 : 0);
    if (expected > maxItems) return bad(`That would place about ${expected} marks; the limit is ${maxItems}.`);
    for (let s = closed ? 0 : spec.spacing; s < total - EPS; s += spec.spacing) at.push(s);
  } else {
    if (!wholeAtLeast(spec.count, 1)) return bad("Enter a whole number of copies, 1 or more.");
    if (spec.count > maxItems) return bad(`At most ${maxItems} copies at a time.`);
    for (let k = 0; k < spec.count; k++) {
      at.push(closed ? (total * k) / spec.count : spec.count > 1 ? (total * k) / (spec.count - 1) : 0);
    }
  }
  return { marks: at.map((s) => pointAlong(points, closed, s)), startAngle: start.angle };
}

/** Rotate `p` about `c` by `theta` radians counter-clockwise. */
export function rotatePoint(p: PdfPoint, c: PdfPoint, theta: number): PdfPoint {
  const cos = Math.cos(theta);
  const sin = Math.sin(theta);
  const dx = p.x - c.x;
  const dy = p.y - c.y;
  return { x: c.x + dx * cos - dy * sin, y: c.y + dx * sin + dy * cos };
}

/**
 * `g` turned by `theta` about `c`, or null when the geometry cannot turn (boxes and text
 * quads are axis-aligned in redline's model). A lone point turns to itself about its own
 * position, so count symbols only take the translation.
 */
export function rotateGeometry(g: MarkupGeometry, c: PdfPoint, theta: number): MarkupGeometry | null {
  if ("Point" in g) return { Point: rotatePoint(g.Point, c, theta) };
  if ("Polyline" in g) return { Polyline: g.Polyline.map((p) => rotatePoint(p, c, theta)) };
  if ("Ink" in g) return { Ink: g.Ink.map((s) => s.map((p) => rotatePoint(p, c, theta))) };
  return null;
}

/** True when `m` can be turned to follow a path (Callouts carry extra box geometry, so no). */
export function canTurn(m: Pick<Markup, "markup_type" | "geometry">): boolean {
  return m.markup_type !== "Callout" && rotateGeometry(m.geometry, { x: 0, y: 0 }, 0) !== null;
}
