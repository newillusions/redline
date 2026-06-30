import { describe, it, expect } from "vitest";
import { dragDrawGeometry, buildMarkup, bumpAudit, RECT_TOOLS, isDrawTool, MULTI_CLICK_TOOLS, isMultiClickTool, isInkTool, polylineGeometry, inkGeometry, minVertices, isMultiClickComplete, TEXT_TOOLS, isTextTool, textBoxGeometry, calloutGeometry, DEFAULT_TEXT_FONT } from "./markup-tools";
import { patchAppearance } from "./markup-properties";
import type { Appearance, UserRef, PdfPoint } from "./ipc";

const AP: Appearance = { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null };
const USER: UserRef = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "Tester" };

describe("dragDrawGeometry", () => {
  it("normalizes a Rect tool to min/max regardless of drag direction", () => {
    const g = dragDrawGeometry("Rectangle", { x: 60, y: 70 }, { x: 10, y: 20 });
    expect(g).toEqual({ Rect: { min: { x: 10, y: 20 }, max: { x: 60, y: 70 } } });
  });
  it("uses Rect geometry for Ellipse and Highlight too", () => {
    expect("Rect" in dragDrawGeometry("Ellipse", { x: 0, y: 0 }, { x: 5, y: 5 })).toBe(true);
    expect("Rect" in dragDrawGeometry("Highlight", { x: 0, y: 0 }, { x: 5, y: 5 })).toBe(true);
    expect(RECT_TOOLS.has("Rectangle")).toBe(true);
  });
  it("uses a 2-point Polyline (in drag order) for Line and Arrow", () => {
    const g = dragDrawGeometry("Line", { x: 1, y: 2 }, { x: 3, y: 4 });
    expect(g).toEqual({ Polyline: [{ x: 1, y: 2 }, { x: 3, y: 4 }] });
    expect("Polyline" in dragDrawGeometry("Arrow", { x: 0, y: 0 }, { x: 1, y: 1 })).toBe(true);
  });
});

describe("isDrawTool", () => {
  it("is true for drag-draw tools, false for non-draw tools", () => {
    expect(isDrawTool("Rectangle")).toBe(true);
    expect(isDrawTool("hand")).toBe(false);
    expect(isDrawTool("select")).toBe(false);
    expect(isDrawTool("Polyline")).toBe(false);
  });
});

describe("buildMarkup", () => {
  it("builds an envelope with audit from identity, revision 0, created==modified", () => {
    const m = buildMarkup({
      markupType: "Rectangle", page: 2,
      geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 1, y: 1 } } },
      appearance: AP, identity: USER, now: "2026-06-14T00:00:00Z", id: "abc",
    });
    expect(m.id).toBe("abc");
    expect(m.markup_type).toBe("Rectangle");
    expect(m.page).toBe(2);
    expect(m.appearance).toEqual(AP);
    expect(m.audit.created_by).toEqual(USER);
    expect(m.audit.modified_by).toEqual(USER);
    expect(m.audit.created_at).toBe("2026-06-14T00:00:00Z");
    expect(m.audit.modified_at).toBe("2026-06-14T00:00:00Z");
    expect(m.audit.revision).toBe(0);
    expect(m.audit.origin).toBe("Desktop");
    expect(m.workflow).toEqual({ status: "None", assignee: null, thread: [] });
    expect(m.subject).toBeNull();
    expect(m.contents).toBeNull();
    expect(m.measurement).toBeNull();
  });
});

describe("multi-click + ink helpers", () => {
  it("classifies tools", () => {
    expect(isMultiClickTool("Polyline")).toBe(true);
    expect(isMultiClickTool("Polygon")).toBe(true);
    expect(isMultiClickTool("Cloud")).toBe(true);
    expect(isMultiClickTool("Rectangle")).toBe(false);
    expect(isMultiClickTool("hand")).toBe(false);
    expect(isInkTool("Ink")).toBe(true);
    expect(isInkTool("Polyline")).toBe(false);
    expect(MULTI_CLICK_TOOLS.has("Cloud")).toBe(true);
  });
  it("minVertices: polyline 2, polygon/cloud 3", () => {
    expect(minVertices("Polyline")).toBe(2);
    expect(minVertices("Polygon")).toBe(3);
    expect(minVertices("Cloud")).toBe(3);
  });
  it("isMultiClickComplete gates on minVertices", () => {
    expect(isMultiClickComplete("Polyline", [{x:0,y:0}])).toBe(false);
    expect(isMultiClickComplete("Polyline", [{x:0,y:0},{x:1,y:1}])).toBe(true);
    expect(isMultiClickComplete("Polygon", [{x:0,y:0},{x:1,y:1}])).toBe(false);
    expect(isMultiClickComplete("Polygon", [{x:0,y:0},{x:1,y:1},{x:2,y:0}])).toBe(true);
  });
  it("polylineGeometry copies the vertices into a Polyline", () => {
    const verts = [{x:0,y:0},{x:10,y:0},{x:10,y:10}];
    const g = polylineGeometry(verts) as { Polyline: typeof verts };
    expect(g.Polyline).toEqual(verts);
    expect(g.Polyline).not.toBe(verts); // defensive copy
  });
  it("inkGeometry wraps strokes into an Ink", () => {
    const strokes = [[{x:0,y:0},{x:1,y:1}]];
    const g = inkGeometry(strokes) as { Ink: typeof strokes };
    expect(g.Ink).toEqual(strokes);
  });
});

describe("text/callout helpers", () => {
  it("classifies text-entry tools", () => {
    expect(isTextTool("Text")).toBe(true);
    expect(isTextTool("Callout")).toBe(true);
    expect(isTextTool("Rectangle")).toBe(false);
    expect(isTextTool("hand")).toBe(false);
    expect(TEXT_TOOLS.has("Callout")).toBe(true);
  });
  it("textBoxGeometry: Rect with top-left at the anchor (PDF y-up: box extends right + down)", () => {
    const g = textBoxGeometry({ x: 10, y: 100 }, { width: 144, height: 18 }) as { Rect: { min: PdfPoint; max: PdfPoint } };
    expect(g.Rect.min).toEqual({ x: 10, y: 82 });   // y - height
    expect(g.Rect.max).toEqual({ x: 154, y: 100 }); // x + width, y
  });
  it("calloutGeometry: 2-point Polyline target→anchor (anchor is last)", () => {
    const g = calloutGeometry({ x: 0, y: 0 }, { x: 50, y: 60 }) as { Polyline: PdfPoint[] };
    expect(g.Polyline).toEqual([{ x: 0, y: 0 }, { x: 50, y: 60 }]);
  });
  it("DEFAULT_TEXT_FONT is Helvetica 12pt", () => {
    expect(DEFAULT_TEXT_FONT).toEqual({ family: "Helvetica", size_pt: 12 });
  });
  it("buildMarkup carries contents when provided (still null by default)", () => {
    const base = { markupType: "Text" as const, page: 0,
      geometry: textBoxGeometry({ x: 0, y: 0 }), appearance: AP, identity: USER, now: "t", id: "x" };
    expect(buildMarkup(base).contents).toBeNull();
    expect(buildMarkup({ ...base, contents: "hi" }).contents).toBe("hi");
  });
});

describe("dragDrawGeometry - shift constrain", () => {
  it("constrain=true forces equal width and height using the larger magnitude", () => {
    // dx=30, dy=50 -> size=50; constrained to (50,50) box
    const g = dragDrawGeometry("Rectangle", { x: 0, y: 0 }, { x: 30, y: 50 }, { constrain: true }) as { Rect: { min: PdfPoint; max: PdfPoint } };
    const w = g.Rect.max.x - g.Rect.min.x;
    const h = g.Rect.max.y - g.Rect.min.y;
    expect(w).toBeCloseTo(h);
    expect(w).toBeCloseTo(50);
  });

  it("constrain=true all 4 drag quadrants yield equal width and height", () => {
    // a={50,50}, each b gives dx=±30, dy=±40 -> size=40
    const a: PdfPoint = { x: 50, y: 50 };
    const cases: PdfPoint[] = [
      { x: 80, y: 90 },  // right+up
      { x: 20, y: 90 },  // left+up
      { x: 80, y: 10 },  // right+down
      { x: 20, y: 10 },  // left+down
    ];
    for (const b of cases) {
      const g = dragDrawGeometry("Rectangle", a, b, { constrain: true }) as { Rect: { min: PdfPoint; max: PdfPoint } };
      const w = g.Rect.max.x - g.Rect.min.x;
      const h = g.Rect.max.y - g.Rect.min.y;
      expect(w).toBeCloseTo(h, 5);
      expect(w).toBeCloseTo(40, 5);
    }
  });

  it("constrain=true also applies to Ellipse (RECT_TOOL)", () => {
    const g = dragDrawGeometry("Ellipse", { x: 0, y: 0 }, { x: 30, y: 50 }, { constrain: true }) as { Rect: { min: PdfPoint; max: PdfPoint } };
    const w = g.Rect.max.x - g.Rect.min.x;
    const h = g.Rect.max.y - g.Rect.min.y;
    expect(w).toBeCloseTo(h);
  });

  it("constrain absent or false does not alter geometry", () => {
    const g = dragDrawGeometry("Rectangle", { x: 0, y: 0 }, { x: 30, y: 50 }) as { Rect: { min: PdfPoint; max: PdfPoint } };
    expect(g.Rect.max.x - g.Rect.min.x).toBe(30);
    expect(g.Rect.max.y - g.Rect.min.y).toBe(50);
  });
});

describe("bumpAudit", () => {
  it("increments revision and updates modified_by/at, preserves created_by/at, does not mutate input", () => {
    const original = buildMarkup({
      markupType: "Rectangle", page: 0,
      geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } },
      appearance: AP, identity: USER, now: "2026-01-01T00:00:00Z", id: "x",
    });
    const other: UserRef = { user_id: "22222222-2222-2222-2222-222222222222", display_name: "Other" };
    const bumped = bumpAudit(original, other, "2026-06-16T12:00:00Z");

    expect(bumped.audit.revision).toBe(1);
    expect(bumped.audit.modified_by).toEqual(other);
    expect(bumped.audit.modified_at).toBe("2026-06-16T12:00:00Z");
    expect(bumped.audit.created_by).toEqual(USER);
    expect(bumped.audit.created_at).toBe("2026-01-01T00:00:00Z");
    expect(bumped).not.toBe(original);
    expect(original.audit.revision).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// Appearance cloning — shared-reference correctness (regression: per-markup
// appearance isolation bug where buildMarkup stored appearance BY REFERENCE so
// mutating draftAppearance post-creation changed every existing markup).
// ---------------------------------------------------------------------------

describe("buildMarkup — appearance isolation", () => {
  const BASE_GEOM = { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } } as const;

  it("mutating the source Appearance object after build does NOT change the markup", () => {
    const draft: Appearance = {
      color: "#e02424", line_weight: 2, opacity: 1,
      fill: null, line_style: "Solid", font: null,
    };
    const m = buildMarkup({
      markupType: "Rectangle", page: 0, geometry: BASE_GEOM,
      appearance: draft, identity: USER, now: "t", id: "iso-1",
    });

    // Simulate what PropertiesPanel did before the fix: in-place mutation of draftAppearance
    Object.assign(draft, { color: "#0000ff", line_weight: 5 });

    expect(m.appearance.color).toBe("#e02424");
    expect(m.appearance.line_weight).toBe(2);
  });

  it("nested font is a distinct object — mutating source font does NOT change markup", () => {
    const font = { family: "Helvetica", size_pt: 12 };
    const draft: Appearance = {
      color: "#000000", line_weight: 1, opacity: 1,
      fill: null, line_style: "Solid", font,
    };
    const m = buildMarkup({
      markupType: "Text", page: 0, geometry: BASE_GEOM,
      appearance: draft, identity: USER, now: "t", id: "iso-2",
    });

    expect(m.appearance.font).not.toBe(font); // must be a clone, not the same reference
    expect(m.appearance.font).toEqual(font);  // same value

    font.size_pt = 99;
    expect(m.appearance.font!.size_pt).toBe(12); // markup unaffected
  });

  it("two markups built from the same draft appearance are independent", () => {
    const draft: Appearance = {
      color: "#e02424", line_weight: 2, opacity: 1,
      fill: null, line_style: "Solid", font: null,
    };
    const m1 = buildMarkup({
      markupType: "Rectangle", page: 0, geometry: BASE_GEOM,
      appearance: draft, identity: USER, now: "t", id: "iso-3a",
    });
    const m2 = buildMarkup({
      markupType: "Ellipse", page: 0, geometry: BASE_GEOM,
      appearance: draft, identity: USER, now: "t", id: "iso-3b",
    });

    // Mutate draft (what PropertiesPanel did with Object.assign before the fix)
    Object.assign(draft, { color: "#00ff00" });

    expect(m1.appearance.color).toBe("#e02424"); // unchanged
    expect(m2.appearance.color).toBe("#e02424"); // unchanged
    // The two markups own their appearance objects independently
    expect(m1.appearance).not.toBe(m2.appearance);
  });

  it("patchAppearance on one markup never leaks into another (patchAppearance is already immutable)", () => {
    const draft: Appearance = {
      color: "#e02424", line_weight: 2, opacity: 1,
      fill: null, line_style: "Solid", font: null,
    };
    const m1 = buildMarkup({
      markupType: "Rectangle", page: 0, geometry: BASE_GEOM,
      appearance: draft, identity: USER, now: "t", id: "iso-4a",
    });
    const m2 = buildMarkup({
      markupType: "Ellipse", page: 0, geometry: BASE_GEOM,
      appearance: draft, identity: USER, now: "t", id: "iso-4b",
    });

    const m1patched = patchAppearance(m1, { color: "#0000ff" }, USER, "t2");

    expect(m1patched.appearance.color).toBe("#0000ff");
    expect(m2.appearance.color).toBe("#e02424"); // m2 is unaffected
    expect(m1.appearance.color).toBe("#e02424"); // original m1 unaffected (patchAppearance is pure)
  });
});
