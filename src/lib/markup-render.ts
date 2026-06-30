/**
 * Pure mapping from a Markup (PDF user space, f64) to an SVG shape descriptor in
 * screen space (CSS px). No DOM, no Svelte - unit-tested in isolation. Viewport.svelte
 * maps these descriptors to SVG elements in the markup overlay (spec §5/§6).
 *
 * line_weight is in PDF points; it is scaled by zoom so a 2pt line looks 2pt at any zoom.
 */
import type { Markup, MarkupType, PdfPoint } from "./ipc";
import { pdfUserSpaceToScreen, type ViewportState } from "./viewport";
import { type Bounds, type HandleId, HANDLE_IDS, handleAnchors } from "./markup-select";

interface SvgStyle {
  id: string;
  stroke: string;
  strokeWidth: number;
  opacity: number;
  fill: string;
  dashArray?: string;
}

export type SvgShape =
  | (SvgStyle & { kind: "rect"; x: number; y: number; width: number; height: number })
  | (SvgStyle & { kind: "ellipse"; cx: number; cy: number; rx: number; ry: number })
  | (SvgStyle & { kind: "polygon"; points: string })
  | (SvgStyle & { kind: "polyline"; points: string })
  | (SvgStyle & {
      kind: "arrow";
      /** Shortened polyline — terminates at the arrowhead base, not through the tip. */
      points: string;
      /** Explicit arrowhead triangle (3-point polygon string). Filled with `stroke` color.
       *  WKWebView does not support `fill="context-stroke"` on SVG markers, so the head
       *  is computed in screen space and rendered as a plain `<polygon>`. */
      arrowHead: string;
    })
  | (SvgStyle & { kind: "cloud"; path: string })
  | (SvgStyle & { kind: "ink"; strokes: string[] })
  | (SvgStyle & { kind: "point"; x: number; y: number })
  | (SvgStyle & { kind: "text"; x: number; y: number; text: string; fontPx: number })
  | (SvgStyle & { kind: "callout"; points: string; x: number; y: number; text: string; fontPx: number });

const CLOSED_TYPES: ReadonlySet<MarkupType> = new Set<MarkupType>([
  "Polygon", "Cloud", "MeasurementArea", "MeasurementPerimeter", "MeasurementVolume",
]);

/** True for markup types whose Polyline geometry forms a closed loop (last → first). */
export function isClosedMarkupType(t: MarkupType): boolean {
  return CLOSED_TYPES.has(t);
}

function dashFor(style: string, w: number): string | undefined {
  if (style === "Dashed") return `${w * 3},${w * 2}`;
  if (style === "Dotted") return `${w},${w * 2}`;
  return undefined;
}

function styleOf(m: Markup, v: ViewportState): SvgStyle {
  const strokeWidth = m.appearance.line_weight * v.zoom;
  return {
    id: m.id,
    stroke: m.appearance.color,
    strokeWidth,
    opacity: m.appearance.opacity,
    fill: m.appearance.fill ?? "none",
    dashArray: dashFor(m.appearance.line_style, strokeWidth),
  };
}

/**
 * Revision-cloud path: walk each closed edge placing outward semicircular arc "bumps"
 * (~2r apart). Screen-space points in, SVG path `d` out. Aesthetic only (not measured).
 */
export function cloudPath(pts: { x: number; y: number }[], r: number): string {
  if (pts.length < 2) return "";
  // Choose the SVG arc sweep flag from the polygon winding so the scallops always bulge to
  // the EXTERIOR regardless of draw direction. The incoming points are screen space (y-DOWN
  // via pdfUserSpaceToScreen), so the shoelace sign is inverted vs. the y-up convention:
  // a clockwise-on-screen loop has POSITIVE signed area. sweep=1 (clockwise arc) bulges to
  // the exterior of a clockwise loop; for a counter-clockwise loop we flip to sweep=0.
  // (Bug: the flag was hardcoded to 1, so CCW-drawn clouds bulged inward.)
  let area2 = 0;
  for (let i = 0; i < pts.length; i++) {
    const a = pts[i], b = pts[(i + 1) % pts.length];
    area2 += a.x * b.y - b.x * a.y;
  }
  const sweep = area2 > 0 ? 1 : 0;
  const loop = [...pts, pts[0]];
  let d = `M ${pts[0].x.toFixed(2)} ${pts[0].y.toFixed(2)}`;
  for (let i = 0; i < loop.length - 1; i++) {
    const a = loop[i], b = loop[i + 1];
    const len = Math.hypot(b.x - a.x, b.y - a.y) || 1;
    const bumps = Math.max(1, Math.round(len / (r * 2)));
    const ux = (b.x - a.x) / len, uy = (b.y - a.y) / len;
    const step = len / bumps;
    let cx = a.x, cy = a.y;
    for (let j = 0; j < bumps; j++) {
      const nx = cx + ux * step, ny = cy + uy * step;
      const rad = (step / 2).toFixed(2);
      d += ` A ${rad} ${rad} 0 0 ${sweep} ${nx.toFixed(2)} ${ny.toFixed(2)}`;
      cx = nx; cy = ny;
    }
  }
  return d + " Z";
}

function pointsStr(pts: { x: number; y: number }[], v: ViewportState): string {
  return pts
    .map((p) => {
      const s = pdfUserSpaceToScreen(p.x, p.y, v);
      return `${+s.x.toFixed(3)},${+s.y.toFixed(3)}`;
    })
    .join(" ");
}

const DEFAULT_FONT_PT = 12;

/**
 * Compute an explicit arrowhead triangle for the end of an arrow polyline.
 * WKWebView does not support `fill="context-stroke"` on SVG markers, so the head
 * must be a standalone `<polygon>` filled with the actual stroke color.
 *
 * Returns:
 *   shortPoints - polyline points string with the last segment shortened to the
 *                 arrowhead base so the shaft does not run through the head.
 *   arrowHead   - space-separated "x,y" polygon string for the filled triangle
 *                 (tip, left barb, right barb in screen space).
 */
function arrowHeadData(
  screenPts: { x: number; y: number }[],
  strokeWidth: number,
): { shortPoints: string; arrowHead: string } {
  const fmt = (p: { x: number; y: number }) => `${+p.x.toFixed(3)},${+p.y.toFixed(3)}`;

  if (screenPts.length < 2) {
    return { shortPoints: screenPts.map(fmt).join(" "), arrowHead: "" };
  }

  const tip  = screenPts[screenPts.length - 1];
  const prev = screenPts[screenPts.length - 2];
  const dx   = tip.x - prev.x;
  const dy   = tip.y - prev.y;
  const len  = Math.hypot(dx, dy);

  if (len < 0.001) {
    return { shortPoints: screenPts.map(fmt).join(" "), arrowHead: "" };
  }

  // Unit forward (shaft direction) and left-perpendicular in screen space (y-down).
  const ux = dx / len;
  const uy = dy / len;
  const nx = -uy;
  const ny =  ux;

  // Head dimensions proportional to stroke width, with a minimum so thin lines still
  // have a visible head.
  const headLen   = Math.max(8, strokeWidth * 4);
  const halfWidth = Math.max(4, strokeWidth * 2);

  // Triangle: tip at the true line endpoint, base pulled back along the shaft.
  const base = { x: tip.x - headLen * ux, y: tip.y - headLen * uy };
  const lb   = { x: base.x + halfWidth * nx, y: base.y + halfWidth * ny };
  const rb   = { x: base.x - halfWidth * nx, y: base.y - halfWidth * ny };

  const arrowHead = `${fmt(tip)} ${fmt(lb)} ${fmt(rb)}`;

  // Shorten the rendered polyline: replace the last vertex with the arrowhead base
  // so the shaft terminates cleanly at the back of the head.
  const shortened = [...screenPts.slice(0, -1), base];
  const shortPoints = shortened.map(fmt).join(" ");

  return { shortPoints, arrowHead };
}

// ---------------------------------------------------------------------------
// SelectionChrome — screen-space chrome for the selection overlay
// ---------------------------------------------------------------------------

/** Screen-space bounding box and optional resize handle positions. */
export interface SelectionChrome {
  /** Bounding box in screen pixels. */
  box: { x: number; y: number; width: number; height: number };
  /** Resize handle positions in screen pixels (empty when !showHandles). */
  handles: { id: HandleId; x: number; y: number }[];
}

/**
 * Map a PDF-space selection Bounds to screen-space chrome. Handles are included
 * only when showHandles is true (single Rect-geometry markup selected).
 *
 * PDF y-up vs screen y-down: map all 4 corners through pdfUserSpaceToScreen
 * and derive box from min/max of mapped coords (do not assume sign of height).
 */
export function selectionChrome(b: Bounds, v: ViewportState, showHandles: boolean): SelectionChrome {
  // Map all 4 corners to screen space.
  const tl = pdfUserSpaceToScreen(b.minX, b.maxY, v); // top-left in screen (maxY = top in PDF y-up)
  const tr = pdfUserSpaceToScreen(b.maxX, b.maxY, v);
  const bl = pdfUserSpaceToScreen(b.minX, b.minY, v);
  const br = pdfUserSpaceToScreen(b.maxX, b.minY, v);

  const xs = [tl.x, tr.x, bl.x, br.x];
  const ys = [tl.y, tr.y, bl.y, br.y];
  const minX = Math.min(...xs);
  const minY = Math.min(...ys);
  const maxX = Math.max(...xs);
  const maxY = Math.max(...ys);

  const box = { x: minX, y: minY, width: maxX - minX, height: maxY - minY };

  const handles: SelectionChrome["handles"] = [];
  if (showHandles) {
    const anchors = handleAnchors(b);
    for (const id of HANDLE_IDS) {
      const pt = anchors[id];
      const s = pdfUserSpaceToScreen(pt.x, pt.y, v);
      handles.push({ id, x: s.x, y: s.y });
    }
  }

  return { box, handles };
}

// ---------------------------------------------------------------------------
// VertexChrome — screen-space per-vertex editing handles for multipoint markups
// ---------------------------------------------------------------------------

/** A draggable vertex handle (one per Polyline point), in screen pixels. */
export interface VertexHandle {
  /** Index into the markup's Polyline points. */
  index: number;
  x: number;
  y: number;
}

/** A midpoint "insert" handle, sitting at the centre of segment `segmentIndex`. */
export interface MidpointHandle {
  /** The segment (vertex segmentIndex → segmentIndex+1) this midpoint splits. */
  segmentIndex: number;
  x: number;
  y: number;
}

export interface VertexChrome {
  vertices: VertexHandle[];
  midpoints: MidpointHandle[];
}

/**
 * Map a multipoint markup's PDF-space vertices to screen-space editing handles:
 * one handle per vertex plus one midpoint handle per segment (for inserting a new
 * vertex). For a closed shape the loop includes the closing segment (last → first).
 */
export function vertexChrome(pts: PdfPoint[], v: ViewportState, closed: boolean): VertexChrome {
  const screen = pts.map((p) => pdfUserSpaceToScreen(p.x, p.y, v));
  const vertices: VertexHandle[] = screen.map((s, i) => ({ index: i, x: s.x, y: s.y }));
  const midpoints: MidpointHandle[] = [];
  const segCount = closed ? screen.length : screen.length - 1;
  for (let i = 0; i < segCount; i++) {
    const a = screen[i];
    const b = screen[(i + 1) % screen.length];
    midpoints.push({ segmentIndex: i, x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });
  }
  return { vertices, midpoints };
}

export function markupToSvg(m: Markup, v: ViewportState): SvgShape {
  const style = styleOf(m, v);
  const g = m.geometry;

  const fontPx = (m.appearance.font?.size_pt ?? DEFAULT_FONT_PT) * v.zoom;
  if (m.markup_type === "Text" && "Rect" in g) {
    const tl = pdfUserSpaceToScreen(g.Rect.min.x, g.Rect.max.y, v); // PDF top-left (y-up)
    return { ...style, kind: "text", x: tl.x, y: tl.y, text: m.contents ?? "", fontPx };
  }
  if (m.markup_type === "Callout" && "Polyline" in g) {
    const last = g.Polyline[g.Polyline.length - 1] ?? { x: 0, y: 0 };
    const anchor = pdfUserSpaceToScreen(last.x, last.y, v);
    return { ...style, kind: "callout", points: pointsStr(g.Polyline, v),
      x: anchor.x, y: anchor.y, text: m.contents ?? "", fontPx };
  }

  if ("Rect" in g && m.markup_type === "Ellipse") {
    const a = pdfUserSpaceToScreen(g.Rect.min.x, g.Rect.min.y, v);
    const b = pdfUserSpaceToScreen(g.Rect.max.x, g.Rect.max.y, v);
    const cx = (a.x + b.x) / 2;
    const cy = (a.y + b.y) / 2;
    const rx = Math.abs(b.x - a.x) / 2;
    const ry = Math.abs(a.y - b.y) / 2;
    return { ...style, kind: "ellipse", cx, cy, rx, ry };
  }
  if ("Rect" in g) {
    const a = pdfUserSpaceToScreen(g.Rect.min.x, g.Rect.min.y, v);
    const b = pdfUserSpaceToScreen(g.Rect.max.x, g.Rect.max.y, v);
    const x = Math.min(a.x, b.x);
    const y = Math.min(a.y, b.y);
    return { ...style, kind: "rect", x, y, width: Math.abs(b.x - a.x), height: Math.abs(a.y - b.y) };
  }
  if ("Polyline" in g && m.markup_type === "Cloud") {
    const screen = g.Polyline.map((p) => pdfUserSpaceToScreen(p.x, p.y, v));
    return { ...style, kind: "cloud", path: cloudPath(screen, Math.max(4, 6 * v.zoom)) };
  }
  if ("Polyline" in g && m.markup_type === "Arrow") {
    const screen = g.Polyline.map((p) => pdfUserSpaceToScreen(p.x, p.y, v));
    const { shortPoints, arrowHead } = arrowHeadData(screen, style.strokeWidth);
    return { ...style, kind: "arrow", points: shortPoints, arrowHead };
  }
  if ("Polyline" in g) {
    const points = pointsStr(g.Polyline, v);
    return { ...style, kind: CLOSED_TYPES.has(m.markup_type) ? "polygon" : "polyline", points };
  }
  if ("Ink" in g) {
    return { ...style, kind: "ink", strokes: g.Ink.map((stroke) => pointsStr(stroke, v)) };
  }
  const s = pdfUserSpaceToScreen(g.Point.x, g.Point.y, v);
  return { ...style, kind: "point", x: s.x, y: s.y };
}
