/**
 * Pure mapping from a Markup (PDF user space, f64) to an SVG shape descriptor in
 * screen space (CSS px). No DOM, no Svelte - unit-tested in isolation. Viewport.svelte
 * maps these descriptors to SVG elements in the markup overlay (spec §5/§6).
 *
 * line_weight is in PDF points; it is scaled by zoom so a 2pt line looks 2pt at any zoom.
 */
import type { Markup, MarkupType } from "./ipc";
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
  | (SvgStyle & { kind: "polygon"; points: string })
  | (SvgStyle & { kind: "polyline"; points: string })
  | (SvgStyle & { kind: "cloud"; path: string })
  | (SvgStyle & { kind: "ink"; strokes: string[] })
  | (SvgStyle & { kind: "point"; x: number; y: number })
  | (SvgStyle & { kind: "text"; x: number; y: number; text: string; fontPx: number })
  | (SvgStyle & { kind: "callout"; points: string; x: number; y: number; text: string; fontPx: number });

const CLOSED_TYPES: ReadonlySet<MarkupType> = new Set<MarkupType>([
  "Polygon", "Cloud", "MeasurementArea", "MeasurementPerimeter", "MeasurementVolume",
]);

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
      d += ` A ${rad} ${rad} 0 0 1 ${nx.toFixed(2)} ${ny.toFixed(2)}`;
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
