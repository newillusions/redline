import { describe, it, expect } from "vitest";
import { markupToSvg, cloudPath, selectionChrome, type SvgShape } from "./markup-render";
import type { Markup, MarkupType, Appearance, MarkupGeometry } from "./ipc";
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
    subject: null, layer: null, contents: null,
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
    expect(markupToSvg(mk(pts, "Arrow"), VS).kind).toBe("polyline");
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
});
