import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  acceptedByPage, allPages, buildCountMarkups, hitsFromResponse, setAll, symbolSearch,
  templateRectFromMarkup, toggleHit, type SymbolSearchResponse, type SymbolMatch,
} from "./symbol-search";
import { buildMarkup } from "./markup-tools";
import type { Appearance, CountSet, Markup, UserRef } from "./ipc";

const mockInvoke = vi.mocked(invoke);
const by: UserRef = { user_id: "u", display_name: "T" };
const APPEARANCE: Appearance = {
  color: "#112233", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null,
} as Appearance;
const SET: CountSet = { id: "setA", name: "Type A", color: "#e02424", symbol: "Circle" };

const match = (cx: number, cy: number, score = 1): SymbolMatch => ({
  center: [cx, cy], rect: { x: cx - 5, y: cy - 5, w: 10, h: 10 }, angle: 0, score, scale: 1,
});
const response = (): SymbolSearchResponse => ({
  method: "vector", template_segments: 12, skipped: [], truncated: false, elapsed_ms: 5,
  pages: [
    { page: 2, matches: [match(10, 10), match(50, 50)], truncated: false },
    { page: 0, matches: [match(100, 100), match(300, 300)], truncated: false },
  ],
});
const TBOX = { x: 90, y: 90, w: 20, h: 20 };

function rectMarkup(min: { x: number; y: number }, max: { x: number; y: number }): Markup {
  return buildMarkup({
    markupType: "Rectangle", page: 3, geometry: { Rect: { min, max } }, appearance: APPEARANCE,
    identity: by, now: "t", id: "r1",
  });
}

describe("symbolSearch", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("sends one `request` struct with snake_case fields (Rust serde), mode and options", async () => {
    mockInvoke.mockResolvedValue(response() as never);
    await symbolSearch("doc1", 4, TBOX, [0, 1], { rotations: true, minScore: 0.8 });
    expect(mockInvoke).toHaveBeenCalledWith("symbol_search", {
      request: {
        doc_id: "doc1", template_page: 4, rect: TBOX, pages: [0, 1], mode: "auto",
        rotations: true, scales: false, min_score: 0.8,
      },
    });
  });

  it("omits unset thresholds so the Rust defaults apply and sends pages:null for the template page only", async () => {
    mockInvoke.mockResolvedValue(response() as never);
    await symbolSearch("d", 0, TBOX, null);
    const arg = mockInvoke.mock.calls[0][1] as { request: Record<string, unknown> };
    expect(arg.request.pages).toBeNull();
    expect("min_score" in arg.request).toBe(false);
    expect("raster_threshold" in arg.request).toBe(false);
  });

  it("allPages is 0-based and never negative", () => {
    expect(allPages(3)).toEqual([0, 1, 2]);
    expect(allPages(-1)).toEqual([]);
  });
});

describe("templateRectFromMarkup", () => {
  it("normalises a rectangle drawn in any direction", () => {
    expect(templateRectFromMarkup(rectMarkup({ x: 50, y: 80 }, { x: 20, y: 30 }))).toEqual({ x: 20, y: 30, w: 30, h: 50 });
  });
  it("rejects non-rectangles, degenerate boxes and nothing selected", () => {
    const pt = buildMarkup({ markupType: "MeasurementCount", page: 0, geometry: { Point: { x: 1, y: 1 } }, appearance: APPEARANCE, identity: by, now: "t", id: "p" });
    expect(templateRectFromMarkup(pt)).toBeNull();
    expect(templateRectFromMarkup(rectMarkup({ x: 1, y: 1 }, { x: 1, y: 9 }))).toBeNull();
    expect(templateRectFromMarkup(undefined)).toBeNull();
  });
});

describe("review state", () => {
  it("flattens in page order, all accepted, and flags the boxed original", () => {
    const hits = hitsFromResponse(response(), 0, TBOX);
    expect(hits.map((h) => h.key)).toEqual(["0:0", "0:1", "2:0", "2:1"]);
    expect(hits.every((h) => h.accepted)).toBe(true);
    expect(hits.filter((h) => h.isTemplate).map((h) => h.key)).toEqual(["0:0"]);
  });

  it("toggle and bulk set return new arrays and leave the input untouched", () => {
    const hits = hitsFromResponse(response(), 0, TBOX);
    const t = toggleHit(hits, "0:1");
    expect(t.find((h) => h.key === "0:1")!.accepted).toBe(false);
    expect(hits.find((h) => h.key === "0:1")!.accepted).toBe(true);
    const none = setAll(hits, false);
    expect(none.some((h) => h.accepted)).toBe(false);
    const pageTwo = setAll(none, true, (h) => h.page === 2);
    expect([...acceptedByPage(pageTwo)]).toEqual([[2, 2]]);
  });
});

describe("buildCountMarkups", () => {
  let n = 0;
  const opts = (over = {}) => ({
    identity: by, now: "2026-11-13T00:00:00Z", mintId: () => `id${++n}`, set: SET, appearance: APPEARANCE, ...over,
  });
  beforeEach(() => { n = 0; });

  it("places only accepted hits as count items at the match centre, in the set's colour", () => {
    const hits = toggleHit(hitsFromResponse(response(), 0, TBOX), "0:1");
    const ms = buildCountMarkups(hits, opts());
    expect(ms).toHaveLength(3);
    expect(ms.every((m) => m.markup_type === "MeasurementCount")).toBe(true);
    expect(ms[0].geometry).toEqual({ Point: { x: 100, y: 100 } });
    expect(ms[0].appearance.color).toBe("#e02424");
    expect(ms[0].count_set).toEqual(SET);
    expect(ms[0].measurement).toMatchObject({ count_value: 1, unit: "ea" });
  });

  it("one count group per page, shared by that page's items, with the label as subject", () => {
    const ms = buildCountMarkups(hitsFromResponse(response(), 0, TBOX), opts({ label: "Type A downlight" }));
    const g0 = new Set(ms.filter((m) => m.page === 0).map((m) => m.count_group));
    const g2 = new Set(ms.filter((m) => m.page === 2).map((m) => m.count_group));
    expect(g0.size).toBe(1);
    expect(g2.size).toBe(1);
    expect([...g0][0]).not.toBe([...g2][0]);
    expect(ms.every((m) => m.subject === "Type A downlight")).toBe(true);
    expect(new Set(ms.map((m) => m.id)).size).toBe(ms.length);
  });

  it("with no count set the draft appearance is kept", () => {
    const ms = buildCountMarkups(hitsFromResponse(response(), 0, TBOX), opts({ set: null }));
    expect(ms[0].appearance.color).toBe("#112233");
    expect(ms[0].count_set).toBeNull();
  });

  it("nothing accepted places nothing", () => {
    expect(buildCountMarkups(setAll(hitsFromResponse(response(), 0, TBOX), false), opts())).toEqual([]);
  });
});
