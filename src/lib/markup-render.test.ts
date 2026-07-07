import { describe, it, expect } from "vitest";
import { markupToSvg, cloudPath, selectionChrome, countSymbolRender, quadToScreenPolygon, type SvgShape } from "./markup-render";
import type { Markup, MarkupType, Appearance, MarkupGeometry, CountSet } from "./ipc";
import type { ViewportState } from "./viewport";
import { pdfUserSpaceToScreen } from "./viewport";
import type { Bounds } from "./markup-select";
import { HANDLE_IDS } from "./markup-select";

// 100pt-wide square page, zoom 2x, no scroll. PDF y-up -> screen y-down.
const VS: ViewportState = {
  canvasWidthCss: 800, canvasHeightCss: 800, zoom: 2, dpr: 1,
  scrollX: 0, scrollY: 0, pageWidthPts: 100, pageHeightPts: 100,
};

function mk(geometry: MarkupGeometry, markup_type: MarkupType, ap: Partial<Appearance> = {}): Markup {
  return {
    id: "00000000-0000-0000-0000-000000000001",
    markup_type, page: 0, geometry,
    appearance: {
      color: "#ff0000", line_weight: 2, opacity: 1, fill: null,
      line_style: "Solid", font: null, ...ap,
    },
    subject: null, layer: null, contents: null, group_id: null,
    audit: {
      created_by: { user_id: "u", display_name: "U" }, created_at: "",
      modified_by: { user_id: "u", display_name: "U" }, modified_at: "",
      revision: 0, origin: "Desktop",
    },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: null,
  };
}

describe("markupToSvg geometry", () => {
  it("maps a Rect to an svg rect with y-flipped top-left and zoom-scaled size", () => {
    const s = markupToSvg(mk({ Rect: { min: { x: 10, y: 20 }, max: { x: 60, y: 70 } } }, "Rectangle"), VS);
    expect(s.kind).toBe("rect");
    if (s.kind !== "rect") throw new Error("kind");
    expect(s.x).toBeCloseTo(20);
    expect(s.y).toBeCloseTo(60);
    expect(s.width).toBeCloseTo(100);
    expect(s.height).toBeCloseTo(100);
  });

  it("maps a closed type (Polygon) to a polygon and an open type (Line) to a polyline", () => {
    const pts: MarkupGeometry = { Polyline: [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 50 }] };
    expect(markupToSvg(mk(pts, "Polygon"), VS).kind).toBe("polygon");
    // Cloud is now a dedicated "cloud" kind (scalloped path), not a plain polygon.
    expect(markupToSvg(mk(pts, "Cloud"), VS).kind).toBe("cloud");
    expect(markupToSvg(mk(pts, "MeasurementArea"), VS).kind).toBe("polygon");
    expect(markupToSvg(mk(pts, "Line"), VS).kind).toBe("polyline");
    expect(markupToSvg(mk(pts, "Arrow"), VS).kind).toBe("arrow");
    expect(markupToSvg(mk(pts, "Polyline"), VS).kind).toBe("polyline");
  });

  it("emits screen-space point strings for a polyline", () => {
    const s = markupToSvg(mk({ Polyline: [{ x: 0, y: 100 }, { x: 100, y: 100 }] }, "Polyline"), VS);
    if (s.kind !== "polyline") throw new Error("kind");
    expect(s.points).toBe("0,0 200,0");
  });

  it("maps Ink to one screen-space path-points array per stroke", () => {
    const s = markupToSvg(mk({ Ink: [[{ x: 0, y: 100 }, { x: 50, y: 100 }], [{ x: 0, y: 0 }]] }, "Ink"), VS);
    if (s.kind !== "ink") throw new Error("kind");
    expect(s.strokes.length).toBe(2);
    expect(s.strokes[0]).toBe("0,0 100,0");
    expect(s.strokes[1]).toBe("0,200");
  });

  it("maps a Point to a screen-space marker position", () => {
    const s = markupToSvg(mk({ Point: { x: 25, y: 75 } }, "MeasurementCount"), VS);
    if (s.kind !== "point") throw new Error("kind");
    expect(s.x).toBeCloseTo(50);
    expect(s.y).toBeCloseTo(50);
  });

  it("defaults a count point with no set to the Circle symbol", () => {
    const s = markupToSvg(mk({ Point: { x: 25, y: 75 } }, "MeasurementCount"), VS);
    if (s.kind !== "point") throw new Error("kind");
    expect(s.symbol).toBe("Circle");
    expect(s.render.shape).toBe("circle");
  });

  it("renders a count point in its set's symbol", () => {
    const set: CountSet = { id: "s1", name: "Type-A", color: "#0066ff", symbol: "Triangle" };
    const m = { ...mk({ Point: { x: 25, y: 75 } }, "MeasurementCount"), count_set: set };
    const s = markupToSvg(m, VS);
    if (s.kind !== "point") throw new Error("kind");
    expect(s.symbol).toBe("Triangle");
    expect(s.render.shape).toBe("polygon");
    if (s.render.shape !== "polygon") throw new Error("shape");
    // A triangle is 3 vertices → 3 "x,y" tokens.
    expect(s.render.points.trim().split(" ")).toHaveLength(3);
  });

  it("maps a text-anchored Highlight (Quads) to a 'quads' kind, one polygon per line", () => {
    const geom: MarkupGeometry = {
      Quads: [
        [{ x: 0, y: 100 }, { x: 50, y: 100 }, { x: 0, y: 50 }, { x: 50, y: 50 }],
      ],
    };
    const s = markupToSvg(mk(geom, "Highlight"), VS);
    expect(s.kind).toBe("quads");
    if (s.kind !== "quads") throw new Error("kind");
    expect(s.polygons).toHaveLength(1);
    // TL(0,100)->(0,0), TR(50,100)->(100,0), BL(0,50)->(0,100), BR(50,50)->(100,100).
    // Rendered winding order is TL,TR,BR,BL (non-self-intersecting).
    expect(s.polygons[0]).toBe("0,0 100,0 100,100 0,100");
  });

  it("preserves quad count for a multi-line text-anchored Highlight", () => {
    const geom: MarkupGeometry = {
      Quads: [
        [{ x: 0, y: 100 }, { x: 50, y: 100 }, { x: 0, y: 90 }, { x: 50, y: 90 }],
        [{ x: 0, y: 85 }, { x: 30, y: 85 }, { x: 0, y: 75 }, { x: 30, y: 75 }],
      ],
    };
    const s = markupToSvg(mk(geom, "Highlight"), VS);
    if (s.kind !== "quads") throw new Error("kind");
    expect(s.polygons).toHaveLength(2);
  });

  it("text-anchored Highlight renders as a translucent colour wash, no stroke", () => {
    const geom: MarkupGeometry = {
      Quads: [[{ x: 0, y: 100 }, { x: 50, y: 100 }, { x: 0, y: 50 }, { x: 50, y: 50 }]],
    };
    const s = markupToSvg(mk(geom, "Highlight", { color: "#00ff00", opacity: 1 }), VS);
    if (s.kind !== "quads") throw new Error("kind");
    expect(s.fill).toBe("#00ff00");
    expect(s.stroke).toBe("none");
    expect(s.opacity).toBeCloseTo(0.35); // opacity(1) * HIGHLIGHT_FILL_ALPHA(0.35)
  });

  it("quadToScreenPolygon: TL/TR/BL/BR storage order renders as TL,TR,BR,BL winding", () => {
    const quad: [{ x: number; y: number }, { x: number; y: number }, { x: number; y: number }, { x: number; y: number }] =
      [{ x: 0, y: 100 }, { x: 50, y: 100 }, { x: 0, y: 50 }, { x: 50, y: 50 }];
    expect(quadToScreenPolygon(quad, VS)).toBe("0,0 100,0 100,100 0,100");
  });

  it("rectangle-drag Highlight (freeform, non-text) is unchanged: still a plain 'rect' kind", () => {
    // The old Rect-geometry Highlight path (drag-anywhere, e.g. for scans/drawings
    // with no text layer) must keep working exactly as before - only the geometry
    // KEY ("Quads" vs "Rect") decides which rendering path a Highlight markup takes.
    const s = markupToSvg(mk({ Rect: { min: { x: 0, y: 0 }, max: { x: 50, y: 50 } } }, "Highlight"), VS);
    expect(s.kind).toBe("rect");
  });
});

describe("countSymbolRender", () => {
  it("circle is a circle primitive centred at (x,y)", () => {
    const r = countSymbolRender("Circle", 10, 20, 6);
    expect(r).toEqual({ shape: "circle", cx: 10, cy: 20, r: 6 });
  });

  it("square/diamond have 4 vertices, hexagon 6, star 10", () => {
    const poly = (sym: "Square" | "Diamond" | "Hexagon" | "Star") => {
      const r = countSymbolRender(sym, 0, 0, 6);
      if (r.shape !== "polygon") throw new Error("polygon");
      return r.points.trim().split(" ").length;
    };
    expect(poly("Square")).toBe(4);
    expect(poly("Diamond")).toBe(4);
    expect(poly("Hexagon")).toBe(6);
    expect(poly("Star")).toBe(10);
  });

  it("cross is two crossing line segments through the centre box", () => {
    const r = countSymbolRender("Cross", 0, 0, 6);
    if (r.shape !== "cross") throw new Error("cross");
    expect(r.lines).toHaveLength(2);
    expect(r.lines[0]).toEqual({ x1: -6, y1: -6, x2: 6, y2: 6 });
  });

  it("symbol geometry stays within the radius bounding box", () => {
    for (const sym of ["Triangle", "Hexagon", "Star", "Diamond"] as const) {
      const r = countSymbolRender(sym, 100, 100, 6);
      if (r.shape !== "polygon") throw new Error("polygon");
      for (const tok of r.points.trim().split(" ")) {
        const [px, py] = tok.split(",").map(Number);
        expect(Math.abs(px - 100)).toBeLessThanOrEqual(6.01);
        expect(Math.abs(py - 100)).toBeLessThanOrEqual(6.01);
      }
    }
  });
});

describe("markupToSvg appearance", () => {
  it("scales stroke width by zoom (points -> screen px) and passes color/opacity", () => {
    const s = markupToSvg(mk({ Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } }, "Rectangle",
      { line_weight: 3, color: "#00ff00", opacity: 0.5 }), VS);
    expect(s.stroke).toBe("#00ff00");
    expect(s.strokeWidth).toBeCloseTo(6);
    expect(s.opacity).toBe(0.5);
  });

  it("uses 'none' fill when fill is null and the hex when set", () => {
    const noFill = markupToSvg(mk({ Rect: { min: { x: 0, y: 0 }, max: { x: 1, y: 1 } } }, "Rectangle"), VS);
    expect(noFill.fill).toBe("none");
    const filled = markupToSvg(mk({ Rect: { min: { x: 0, y: 0 }, max: { x: 1, y: 1 } } }, "Rectangle",
      { fill: "#0000ff" }), VS);
    expect(filled.fill).toBe("#0000ff");
  });

  it("maps line_style to a dash array (Solid = undefined)", () => {
    const solid = markupToSvg(mk({ Rect: { min: { x: 0, y: 0 }, max: { x: 1, y: 1 } } }, "Rectangle", { line_style: "Solid" }), VS);
    expect(solid.dashArray).toBeUndefined();
    const dashed = markupToSvg(mk({ Rect: { min: { x: 0, y: 0 }, max: { x: 1, y: 1 } } }, "Rectangle", { line_style: "Dashed" }), VS);
    expect(dashed.dashArray).toBeTruthy();
  });

  it("carries the markup id through for hit-testing/keying", () => {
    const s = markupToSvg(mk({ Point: { x: 0, y: 0 } }, "MeasurementCount"), VS);
    expect(s.id).toBe("00000000-0000-0000-0000-000000000001");
  });
});

describe("text + callout rendering", () => {
  it("maps a Text (Rect box) to a text shape at the box top-left, font-scaled by zoom", () => {
    const m = mk({ Rect: { min: { x: 10, y: 20 }, max: { x: 60, y: 40 } } }, "Text",
      { font: { family: "Helvetica", size_pt: 12 } });
    m.contents = "hello";
    const s = markupToSvg(m, VS);
    if (s.kind !== "text") throw new Error("kind");
    expect(s.text).toBe("hello");
    expect(s.fontPx).toBe(12 * VS.zoom);   // scaled by zoom
    // top-left = screen of (min.x, max.y) — verify it matches the transform
    const tl = pdfUserSpaceToScreen(10, 40, VS);
    expect(s.x).toBeCloseTo(tl.x); expect(s.y).toBeCloseTo(tl.y);
    // Box derives from the SAME Rect: width = (60-10)*zoom, height = (40-20)*zoom.
    expect(s.width).toBeCloseTo(50 * VS.zoom);
    expect(s.height).toBeCloseTo(20 * VS.zoom);
    // Outline defaults to the glyph colour when outline_color is unset; fill alpha defaults to 1.
    expect(s.outline).toBe("#ff0000");
    expect(s.fillOpacity).toBe(1);
  });
  it("Text box + glyphs translate together as one unit when the Rect moves", () => {
    const at = (dx: number, dy: number) =>
      markupToSvg(mk({ Rect: { min: { x: 10 + dx, y: 20 + dy }, max: { x: 60 + dx, y: 40 + dy } } }, "Text"), VS);
    const a = at(0, 0), b = at(15, 0);
    if (a.kind !== "text" || b.kind !== "text") throw new Error("kind");
    // Same Rect → box AND text share x/y, and both shift by the same screen delta.
    expect(b.x - a.x).toBeCloseTo(15 * VS.zoom);
    expect(b.y).toBeCloseTo(a.y);
    expect(b.width).toBeCloseTo(a.width); // box size unchanged by translation
  });
  it("Text box uses outline_color + fill_opacity when set (distinct from the glyph colour)", () => {
    const m = mk({ Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } }, "Text",
      { color: "#111111", outline_color: "#00ff00", fill: "#abcdef", fill_opacity: 0.3 });
    const s = markupToSvg(m, VS);
    if (s.kind !== "text") throw new Error("kind");
    expect(s.stroke).toBe("#111111");   // glyph colour (text fill)
    expect(s.outline).toBe("#00ff00");  // box border — distinct from the glyph colour
    expect(s.fill).toBe("#abcdef");     // box fill
    expect(s.fillOpacity).toBe(0.3);    // independent of overall opacity
  });
  it("Text with null contents renders empty text, default 12pt", () => {
    const s = markupToSvg(mk({ Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } }, "Text"), VS);
    if (s.kind !== "text") throw new Error("kind");
    expect(s.text).toBe(""); expect(s.fontPx).toBe(12 * VS.zoom);
  });
  it("maps a Callout (Polyline leader) to a callout shape: leader points + text at the last vertex", () => {
    const m = mk({ Polyline: [{ x: 0, y: 0 }, { x: 50, y: 60 }] }, "Callout");
    m.contents = "see note";
    const s = markupToSvg(m, VS);
    if (s.kind !== "callout") throw new Error("kind");
    expect(s.points.split(" ").length).toBe(2);
    expect(s.text).toBe("see note");
    const anchor = pdfUserSpaceToScreen(50, 60, VS);
    expect(s.x).toBeCloseTo(anchor.x); expect(s.y).toBeCloseTo(anchor.y);
    // Arrowhead at the leader's pointing (target) end: an explicit 3-point polygon.
    expect(s.arrowHead.split(" ").length).toBe(3);
    // Synthesized text box at the anchor end has a positive size.
    expect(s.width).toBeGreaterThan(0);
    expect(s.height).toBeGreaterThan(0);
    // Outline + fill alpha resolve with the same fallbacks as the Text box.
    expect(s.outline).toBe("#ff0000");
    expect(s.fillOpacity).toBe(1);
  });
});

describe("selectionChrome", () => {
  // Page 100x100 pt, zoom 1, no scroll — screen pixel = PDF point with y-flip.
  const VS1: ViewportState = {
    canvasWidthCss: 200, canvasHeightCss: 200, zoom: 1, dpr: 1,
    scrollX: 0, scrollY: 0, pageWidthPts: 100, pageHeightPts: 100,
  };

  // Same page, zoom 2.
  const VS2: ViewportState = { ...VS1, zoom: 2 };

  const b: Bounds = { minX: 10, minY: 20, maxX: 60, maxY: 70 };

  it("box maps correctly at zoom 1", () => {
    const c = selectionChrome(b, VS1, false);
    // PDF minY=20 -> screen y = (100-20)*1 = 80; PDF maxY=70 -> screen y = (100-70)*1 = 30
    // PDF minX=10 -> screen x = 10; PDF maxX=60 -> screen x = 60
    expect(c.box.x).toBeCloseTo(10);
    expect(c.box.y).toBeCloseTo(30);
    expect(c.box.width).toBeCloseTo(50);
    expect(c.box.height).toBeCloseTo(50);
  });

  it("box maps correctly at zoom 2", () => {
    const c = selectionChrome(b, VS2, false);
    // At zoom 2: screen_x = pdfX*2; screen_y = (100-pdfY)*2
    // minX=10->20, maxX=60->120, width=100
    // minY=20->screen_y=(100-20)*2=160; maxY=70->screen_y=(100-70)*2=60, so box.y=60
    expect(c.box.x).toBeCloseTo(20);
    expect(c.box.y).toBeCloseTo(60);
    expect(c.box.width).toBeCloseTo(100);
    expect(c.box.height).toBeCloseTo(100);
  });

  it("showHandles=false -> 0 handles", () => {
    const c = selectionChrome(b, VS1, false);
    expect(c.handles).toHaveLength(0);
  });

  it("showHandles=true -> 8 handles at expected screen coords", () => {
    const c = selectionChrome(b, VS1, true);
    expect(c.handles).toHaveLength(8);
    // All 8 HANDLE_IDS must be present.
    const ids = c.handles.map((h) => h.id);
    for (const id of HANDLE_IDS) {
      expect(ids).toContain(id);
    }
    // Spot-check "nw" handle: PDF (minX, maxY) = (10, 70) -> screen (10, 30).
    const nw = c.handles.find((h) => h.id === "nw")!;
    expect(nw.x).toBeCloseTo(10);
    expect(nw.y).toBeCloseTo(30);
    // Spot-check "se" handle: PDF (maxX, minY) = (60, 20) -> screen (60, 80).
    const se = c.handles.find((h) => h.id === "se")!;
    expect(se.x).toBeCloseTo(60);
    expect(se.y).toBeCloseTo(80);
  });
});

describe("arrow rendering", () => {
  it("Arrow markup returns kind arrow (not polyline), enabling arrowhead marker", () => {
    const pts: MarkupGeometry = { Polyline: [{ x: 0, y: 0 }, { x: 50, y: 50 }] };
    const s = markupToSvg(mk(pts, "Arrow"), VS);
    expect(s.kind).toBe("arrow");
  });
  it("Arrow polyline start matches the equivalent Line start; endpoint is pulled back by head length", () => {
    const pts: MarkupGeometry = { Polyline: [{ x: 0, y: 100 }, { x: 100, y: 100 }] };
    const arrow = markupToSvg(mk(pts, "Arrow"), VS);
    const line  = markupToSvg(mk(pts, "Line"),  VS);
    if (arrow.kind !== "arrow") throw new Error("expected arrow kind");
    if (line.kind !== "polyline") throw new Error("expected polyline kind");
    // Both begin at the same screen-space first point.
    const arrowPts = arrow.points.trim().split(/\s+/);
    const linePts = line.points.trim().split(/\s+/);
    expect(arrowPts[0]).toBe(linePts[0]);
    // Arrow last point is pulled back so the line terminates at the arrowhead base.
    const arrowLast = arrowPts[arrowPts.length - 1].split(",").map(Number);
    const lineLast = linePts[linePts.length - 1].split(",").map(Number);
    expect(arrowLast[0]).toBeLessThan(lineLast[0]);
  });
  it("Line markup remains kind polyline (no arrowhead)", () => {
    const pts: MarkupGeometry = { Polyline: [{ x: 0, y: 0 }, { x: 50, y: 50 }] };
    expect(markupToSvg(mk(pts, "Line"), VS).kind).toBe("polyline");
  });
});

describe("arrow rendering - arrowhead geometry (WKWebView-safe explicit polygon)", () => {
  // VS: zoom=2, pageHeight=100, scrollX=0, scrollY=0, line_weight=2 -> strokeWidth=4
  // Horizontal arrow PDF(0,100)->(100,100) maps to screen (0,0)->(200,0)
  // headLen = max(8, 4*4)=16; halfWidth = max(4, 4*2)=8; base.x = 200-16 = 184

  it("Arrow shape carries an arrowHead polygon field with 3 points — not context-stroke marker", () => {
    const pts: MarkupGeometry = { Polyline: [{ x: 0, y: 100 }, { x: 100, y: 100 }] };
    const s = markupToSvg(mk(pts, "Arrow"), VS);
    if (s.kind !== "arrow") throw new Error("expected arrow kind");
    expect(s.arrowHead).toBeTruthy();
    expect(s.arrowHead.trim().split(/\s+/)).toHaveLength(3);
  });

  it("Arrow head fill color is the markup stroke color — no context-stroke or context-fill", () => {
    const pts: MarkupGeometry = { Polyline: [{ x: 0, y: 100 }, { x: 100, y: 100 }] };
    const s = markupToSvg(mk(pts, "Arrow", { color: "#00ff00" }), VS);
    if (s.kind !== "arrow") throw new Error("expected arrow kind");
    // Viewport.svelte fills the arrowHead polygon with fill={s.stroke}
    expect(s.stroke).toBe("#00ff00");
    expect(s.stroke).not.toMatch(/^context-/);
    expect(s.stroke).not.toBe("none");
  });

  it("Arrow head tip is placed at the screen-space last point of the underlying geometry", () => {
    // PDF(100,100) at zoom 2, pageH=100: screen x=200, y=(100-100)*2=0 -> (200,0)
    const pts: MarkupGeometry = { Polyline: [{ x: 0, y: 100 }, { x: 100, y: 100 }] };
    const s = markupToSvg(mk(pts, "Arrow"), VS);
    if (s.kind !== "arrow") throw new Error("expected arrow kind");
    const [tipStr] = s.arrowHead.trim().split(/\s+/);
    const [tipX, tipY] = tipStr.split(",").map(Number);
    expect(tipX).toBeCloseTo(200, 0);
    expect(tipY).toBeCloseTo(0, 0);
  });

  it("Arrow polyline is shortened so the line terminates at the arrowhead base (not through the tip)", () => {
    const pts: MarkupGeometry = { Polyline: [{ x: 0, y: 100 }, { x: 100, y: 100 }] };
    const s = markupToSvg(mk(pts, "Arrow"), VS);
    if (s.kind !== "arrow") throw new Error("expected arrow kind");
    const parts = s.points.trim().split(/\s+/);
    const lastPt = parts[parts.length - 1].split(",").map(Number);
    // Rendered line must end before tip x=200, pulled back by headLen=16 -> x~184
    expect(lastPt[0]).toBeLessThan(200);
    expect(lastPt[0]).toBeCloseTo(184, 0);
    expect(lastPt[1]).toBeCloseTo(0, 0);
  });
});

describe("ellipse rendering", () => {
  // VS: zoom=2, pageHeight=100, scrollX=0, scrollY=0
  // Rect { min: {x:10,y:20}, max: {x:60,y:70} } in screen space:
  //   screen of (10,20): x=20, y=(100-20)*2=160
  //   screen of (60,70): x=120, y=(100-70)*2=60
  //   cx=(20+120)/2=70, cy=(160+60)/2=110, rx=(120-20)/2=50, ry=(160-60)/2=50

  it("Ellipse markup yields kind:ellipse with cx/cy/rx/ry computed from rect bounds in screen space", () => {
    const s = markupToSvg(mk({ Rect: { min: { x: 10, y: 20 }, max: { x: 60, y: 70 } } }, "Ellipse"), VS);
    expect(s.kind).toBe("ellipse");
    if (s.kind !== "ellipse") throw new Error("expected ellipse kind");
    expect(s.cx).toBeCloseTo(70);
    expect(s.cy).toBeCloseTo(110);
    expect(s.rx).toBeCloseTo(50);
    expect(s.ry).toBeCloseTo(50);
  });

  it("Rectangle markup still yields kind:rect after ellipse special-case (no regression)", () => {
    const s = markupToSvg(mk({ Rect: { min: { x: 10, y: 20 }, max: { x: 60, y: 70 } } }, "Rectangle"), VS);
    expect(s.kind).toBe("rect");
  });

  it("Highlight renders as a translucent wash (colour fill, no border, no text-box outline)", () => {
    const m = mk({ Rect: { min: { x: 10, y: 20 }, max: { x: 60, y: 70 } } }, "Highlight",
      { color: "#ffe600", opacity: 1 });
    const s = markupToSvg(m, VS);
    expect(s.kind).toBe("rect"); // still a rect element (no regression in element type)
    if (s.kind !== "rect") throw new Error("kind");
    expect(s.fill).toBe("#ffe600");      // colour wash, not a solid grey/black box
    expect(s.stroke).toBe("none");       // no border (it's a marker, not a text box)
    expect(s.opacity).toBeLessThan(1);   // translucent
    expect(s).not.toHaveProperty("outline"); // text-box outline treatment must NOT apply
  });
});

describe("cloud rendering", () => {
  it("cloudPath returns a closed arc path through the points", () => {
    const d = cloudPath([{ x: 0, y: 0 }, { x: 40, y: 0 }, { x: 40, y: 40 }], 5);
    expect(d.startsWith("M")).toBe(true);
    expect(d).toContain("A");      // arc bumps
    expect(d.trimEnd().endsWith("Z")).toBe(true); // closed
  });
  it("longer edges get more bumps than shorter ones", () => {
    const shortP = cloudPath([{ x: 0, y: 0 }, { x: 10, y: 0 }], 5);
    const longP = cloudPath([{ x: 0, y: 0 }, { x: 100, y: 0 }], 5);
    const count = (s: string) => (s.match(/A/g) ?? []).length;
    expect(count(longP)).toBeGreaterThan(count(shortP));
  });
  it("maps a Cloud markup to a cloud shape (path), not a plain polygon", () => {
    const m = mk({ Polyline: [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 50 }] }, "Cloud");
    const s = markupToSvg(m, VS);
    expect(s.kind).toBe("cloud");
    if (s.kind !== "cloud") throw new Error("kind");
    expect(typeof s.path).toBe("string");
    expect(s.path.length).toBeGreaterThan(0);
  });

  // The sweep flag must follow the winding so scallops bulge OUTWARD regardless of draw
  // direction. cloudPath receives SCREEN-space (y-DOWN) points; a clockwise-on-screen loop
  // (positive shoelace) → sweep 1, counter-clockwise → sweep 0.
  const sweepFlags = (d: string): number[] =>
    [...d.matchAll(/A [\d.]+ [\d.]+ 0 0 (\d)/g)].map((m) => Number(m[1]));

  it("a clockwise-on-screen triangle uses sweep flag 1 (outward bulge)", () => {
    // (0,0)→(10,0)→(10,10) closed: shoelace = +100 → clockwise on screen.
    const d = cloudPath([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }], 3);
    const flags = sweepFlags(d);
    expect(flags.length).toBeGreaterThan(0);
    expect(flags.every((f) => f === 1)).toBe(true);
  });

  it("the same triangle wound counter-clockwise flips to sweep flag 0 (still outward)", () => {
    // Reversed winding: shoelace = -100 → counter-clockwise on screen.
    const d = cloudPath([{ x: 0, y: 0 }, { x: 10, y: 10 }, { x: 10, y: 0 }], 3);
    const flags = sweepFlags(d);
    expect(flags.length).toBeGreaterThan(0);
    expect(flags.every((f) => f === 0)).toBe(true);
  });

  it("CW and CCW windings of the same polygon choose opposite sweep flags", () => {
    const cw = sweepFlags(cloudPath([{ x: 0, y: 0 }, { x: 20, y: 0 }, { x: 20, y: 20 }, { x: 0, y: 20 }], 4));
    const ccw = sweepFlags(cloudPath([{ x: 0, y: 20 }, { x: 20, y: 20 }, { x: 20, y: 0 }, { x: 0, y: 0 }], 4));
    expect(cw.every((f) => f === 1)).toBe(true);
    expect(ccw.every((f) => f === 0)).toBe(true);
  });
});
