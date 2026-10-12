import { describe, it, expect } from "vitest";
import type { CountSet, Markup, UserRef } from "./ipc";
import type { ViewportState } from "./viewport";
import { markupToSvg } from "./markup-render";
import { computeRows, layout, legendFromCountSets, sanitizeLegend } from "./legend";
import { mk } from "./markups-list.test-util";

const VS: ViewportState = {
  canvasWidthCss: 2000, canvasHeightCss: 2000, zoom: 2, dpr: 1, scrollX: 0, scrollY: 0, pageWidthPts: 612, pageHeightPts: 792,
};
const by: UserRef = { user_id: "u", display_name: "T" };
const A: CountSet = { id: "a", name: "Downlight", color: "#ff0000", symbol: "Square" };

function count(id: string, set: CountSet, n: number): Markup {
  return mk(id, {
    markup_type: "MeasurementCount",
    appearance: { color: set.color, line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
    count_set: set,
    measurement: { scale_ref: null, raw_measure: n, unit: "ea", computed_quantity: n, depth: null, count_value: n, custom_columns: {}, rise_drop: [] },
  } as Partial<Markup>);
}

function legend(over: Partial<Markup> = {}): Markup {
  return { ...legendFromCountSets([A], { page: 0, topLeft: { x: 50, y: 700 }, identity: by, now: "t", id: "L" }), ...over };
}

type LegendShape = Extract<ReturnType<typeof markupToSvg>, { kind: "legend" }>;
const draw = (m: Markup, ctx: Markup[]) =>
  markupToSvg(m, VS, computeRows(m.page, sanitizeLegend(m.legend), ctx)) as LegendShape;

describe("markupToSvg: Legend", () => {
  it("draws the title, headers, the computed row and its symbol", () => {
    const s = draw(legend(), [count("1", A, 3), count("2", A, 4)]);
    expect(s.kind).toBe("legend");
    const texts = s.texts.map((t) => t.text);
    expect(texts).toEqual(["Count legend", "", "Subject", "Quantity", "Unit", "Downlight", "7", "ea"].filter((t) => t !== ""));
    expect(s.symbols).toHaveLength(1);
    expect(s.symbols[0]).toMatchObject({ kind: "count", color: "#ff0000" });
  });

  it("is placed at the legend's top-left and scaled by the zoom", () => {
    const s = draw(legend(), [count("1", A, 1)]);
    const lay = layout(sanitizeLegend(legend().legend), 300, 1);
    expect(s.x).toBe(100); // 50 pt x zoom 2
    expect(s.y).toBe((792 - 700) * 2);
    expect(s.width).toBeCloseTo(lay.width * 2, 9);
    expect(s.height).toBeCloseTo(lay.height * 2, 9);
    expect(s.texts[0].fontPx).toBeCloseTo(lay.font * 2, 9);
  });

  it("recomputes from the mark-ups passed in: change one and the text changes", () => {
    const before = draw(legend(), [count("1", A, 1)]).texts.map((t) => t.text);
    const after = draw(legend(), [count("1", A, 1), count("2", A, 8)]).texts.map((t) => t.text);
    expect(before).toContain("1");
    expect(after).toContain("9");
  });

  it("falls back to a plain rectangle when the definition is missing", () => {
    const shape = markupToSvg({ ...legend(), legend: null }, VS, []);
    expect(shape.kind).toBe("rect");
  });

  it("bounds the number of drawn texts for a hostile 256-row legend", () => {
    const sets = Array.from({ length: 256 }, (_, i) => ({ ...A, id: `s${i}`, name: "n".repeat(600) }));
    const l = legendFromCountSets(sets, { page: 0, topLeft: { x: 0, y: 700 }, identity: by, now: "t", id: "L" });
    const s = draw(l, sets.map((x, i) => count(`m${i}`, x, 1)));
    expect(s.texts.length).toBeLessThanOrEqual(6 + 256 * 3);
    expect(s.texts.every((t) => t.text.length <= 100)).toBe(true);
  });

  it("an unusable colour (non-ASCII, wrong length) is drawn as black, never passed through", () => {
    const evil: CountSet = { ...A, color: "#a\u00e9aaa" };
    const m = count("1", evil, 1);
    m.appearance.color = "#b\u00e9";
    const l = legendFromCountSets([evil], { page: 0, topLeft: { x: 50, y: 700 }, identity: by, now: "t", id: "L" });
    const s = draw(l, [m]);
    expect(s.symbols[0]).toMatchObject({ kind: "count", color: "#000000" });
  });
});
