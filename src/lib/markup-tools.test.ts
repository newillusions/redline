import { describe, it, expect } from "vitest";
import { dragDrawGeometry, buildMarkup, bumpAudit, RECT_TOOLS, isDrawTool, MULTI_CLICK_TOOLS, isMultiClickTool, isInkTool, polylineGeometry, inkGeometry, minVertices, isMultiClickComplete, TEXT_TOOLS, isTextTool, textBoxGeometry, calloutGeometry, DEFAULT_TEXT_FONT } from "./markup-tools";
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
