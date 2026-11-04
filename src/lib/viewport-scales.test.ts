/**
 * Measurement viewports (docs/measurement-viewports.md): scale selection by first point,
 * the recompute rule (no silent recalculation), explicit re-apply, presets.
 */
import { describe, it, expect, vi } from "vitest";
import type { BBox, Markup, ScaleRecord, UserRef } from "./ipc";
import {
  measurementAnchor, measurementsUsing, reapplyViewport, recomputeAll, sameSlot, scaleAt, viewportAt,
} from "./polylength";
import { presetRatio, SCALE_PRESETS } from "./scale-presets";
import { TakeoffStore } from "./takeoff-store.svelte";
import { MarkupStore } from "./markup-store.svelte";
import { commitViewportChange } from "./viewport-actions";

const by: UserRef = { user_id: "u", display_name: "T" };
const NOW = "2026-11-04T10:00:00Z";

const def = (id: string, ratio: number): ScaleRecord => ({
  id, applies_to: { kind: "DocumentDefault" }, method: "Preset", ratio, unit: "m", label: id, precision: 2,
});
const page = (id: string, p: number, ratio: number): ScaleRecord => ({
  id, applies_to: { kind: "Page", page: p }, method: "Preset", ratio, unit: "m", label: id, precision: 2,
});
const vp = (id: string, p: number, bbox: BBox, ratio: number): ScaleRecord => ({
  id, applies_to: { kind: "Viewport", page: p, viewport_id: `vp-${id}`, name: id, bbox },
  method: "Preset", ratio, unit: "m", label: "1:20", precision: 2,
});

function length(id: string, start: { x: number; y: number }, raw: number, scaleRef: string | null, qty = 0): Markup {
  return {
    id, markup_type: "MeasurementLength", page: 0,
    geometry: { Polyline: [start, { x: start.x + raw, y: start.y }] },
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null },
    subject: null, layer: null, contents: null, group_id: null,
    audit: { created_by: by, created_at: NOW, modified_by: by, modified_at: NOW, revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: { scale_ref: scaleRef, raw_measure: raw, unit: "m", computed_quantity: qty, depth: null, count_value: null, custom_columns: {} },
  } as Markup;
}

describe("scale selection", () => {
  const scales = [def("d", 0.01), page("p1", 1, 0.02), vp("detail", 0, [100, 100, 200, 200], 0.001)];

  it("uses the viewport under the first point, else the page scale, else the default", () => {
    expect(scaleAt(scales, 0, { x: 150, y: 150 })?.id).toBe("detail");
    expect(scaleAt(scales, 0, { x: 200, y: 100 })?.id).toBe("detail"); // edge counts
    expect(scaleAt(scales, 0, { x: 50, y: 50 })?.id).toBe("d");
    expect(scaleAt(scales, 1, { x: 150, y: 150 })?.id).toBe("p1"); // viewport is page 0 only
    expect(scaleAt(scales, 0, null)?.id).toBe("d");
  });

  it("overlapping viewports: the last-drawn one wins (ISO 32000-1 §12.9)", () => {
    const s = [vp("plan", 0, [0, 0, 500, 500], 0.01), vp("inset", 0, [100, 100, 200, 200], 0.002)];
    expect(viewportAt(s, 0, { x: 150, y: 150 })?.id).toBe("inset");
    expect(viewportAt(s, 0, { x: 300, y: 300 })?.id).toBe("plan");
    expect(viewportAt(s, 0, { x: 600, y: 600 })).toBeNull();
  });

  it("the anchor is the first point of the geometry", () => {
    expect(measurementAnchor(length("a", { x: 5, y: 6 }, 10, null))).toEqual({ x: 5, y: 6 });
    const pt = { ...length("b", { x: 0, y: 0 }, 1, null), geometry: { Point: { x: 7, y: 8 } } } as Markup;
    expect(measurementAnchor(pt)).toEqual({ x: 7, y: 8 });
  });

  it("same slot: by viewport id, page, or default", () => {
    const a = vp("a", 0, [0, 0, 1, 1], 1).applies_to;
    expect(sameSlot(a, { ...a, name: "renamed", bbox: [5, 5, 9, 9] } as typeof a)).toBe(true);
    expect(sameSlot(a, vp("b", 0, [0, 0, 1, 1], 1).applies_to)).toBe(false);
    expect(sameSlot({ kind: "Page", page: 1 }, { kind: "Page", page: 1 })).toBe(true);
    expect(sameSlot({ kind: "DocumentDefault" }, { kind: "Page", page: 1 })).toBe(false);
  });
});

describe("recompute rule: no silent recalculation", () => {
  it("a measurement taken under the default keeps its value when a viewport appears over it", () => {
    const m = length("m", { x: 150, y: 150 }, 1000, "d", 10);
    const scales = [def("d", 0.01), vp("detail", 0, [100, 100, 200, 200], 0.001)];
    expect(recomputeAll([m], scales, by, NOW)).toEqual([]);
  });

  it("recalibrating the default (same id) updates it, the viewport does not take it over", () => {
    const m = length("m", { x: 150, y: 150 }, 1000, "d", 10);
    const pairs = recomputeAll([m], [def("d", 0.02), vp("detail", 0, [100, 100, 200, 200], 0.001)], by, NOW);
    expect(pairs).toHaveLength(1);
    expect(pairs[0].after.measurement?.scale_ref).toBe("d");
    expect(pairs[0].after.measurement?.computed_quantity).toBeCloseTo(20, 9);
  });

  it("an unscaled measurement inside a viewport takes the viewport's scale", () => {
    const m = length("m", { x: 150, y: 150 }, 1000, null, 0);
    const pairs = recomputeAll([m], [def("d", 0.01), vp("detail", 0, [100, 100, 200, 200], 0.001)], by, NOW);
    expect(pairs[0].after.measurement?.scale_ref).toBe("detail");
    expect(pairs[0].after.measurement?.computed_quantity).toBeCloseTo(1, 9);
  });

  it("a viewport moved off a measurement leaves it alone; deleting it falls back", () => {
    const m = length("m", { x: 150, y: 150 }, 1000, "detail", 1);
    const moved = [def("d", 0.01), vp("detail", 0, [300, 300, 400, 400], 0.005)];
    expect(recomputeAll([m], moved, by, NOW)).toEqual([]);
    const deleted = recomputeAll([m], [def("d", 0.01)], by, NOW);
    expect(deleted[0].after.measurement?.scale_ref).toBe("d");
    expect(deleted[0].after.measurement?.computed_quantity).toBeCloseTo(10, 9);
  });

  it("re-apply (explicit) moves the measurements inside a viewport under it, nothing else", () => {
    const scales = [def("d", 0.01), vp("detail", 0, [100, 100, 200, 200], 0.001)];
    const inside = length("in", { x: 150, y: 150 }, 1000, "d", 10);
    const outside = length("out", { x: 500, y: 500 }, 1000, "d", 10);
    const pairs = reapplyViewport([inside, outside], scales, "detail", by, NOW);
    expect(pairs.map((p) => p.before.id)).toEqual(["in"]);
    expect(pairs[0].after.measurement?.computed_quantity).toBeCloseTo(1, 9);
    expect(pairs[0].after.measurement?.scale_ref).toBe("detail");
    expect(reapplyViewport([inside], scales, "d", by, NOW)).toEqual([]); // not a viewport
    expect(measurementsUsing([inside, outside, pairs[0].after], "detail")).toBe(1);
  });
});

describe("TakeoffStore and viewports", () => {
  it("a viewport never becomes the active scale; new measurements use it only inside", () => {
    const s = new TakeoffStore();
    s.addScale(def("d", 0.01));
    s.addScale(vp("detail", 0, [100, 100, 200, 200], 0.001));
    expect(s.activeScaleId).toBe("d");
    expect(s.scaleAt(0, { x: 150, y: 150 })?.id).toBe("detail");
    expect(s.scaleAt(0, { x: 50, y: 50 })?.id).toBe("d");
    expect(s.viewportsOn(0).map((v) => v.id)).toEqual(["detail"]);
    expect(s.viewportsOn(1)).toEqual([]);
  });

  it("an edited viewport replaces its record in place (drawing order kept)", () => {
    const s = new TakeoffStore();
    s.addScale(vp("a", 0, [0, 0, 10, 10], 0.1));
    s.addScale(vp("b", 0, [0, 0, 10, 10], 0.2));
    s.addScale({ ...vp("a", 0, [5, 5, 50, 50], 0.3) });
    expect(s.scales.map((x) => x.id)).toEqual(["a", "b"]);
    expect(s.scales[0].ratio).toBe(0.3);
  });

  it("seeding a document with only viewports leaves no active page scale", () => {
    const s = new TakeoffStore();
    s.seedScales([vp("a", 0, [0, 0, 10, 10], 0.1)]);
    expect(s.activeScale).toBeNull();
    expect(s.viewports).toHaveLength(1);
  });
});

describe("scale presets", () => {
  it("1:100 in metres is 100 x 0.0254/72 m per point; imperial 1/4in = 1ft is 1/18 ft per point", () => {
    expect(presetRatio(100, "m")).toBeCloseTo((100 * 0.0254) / 72, 12);
    expect(presetRatio(100, "mm")).toBeCloseTo((100 * 25.4) / 72, 9);
    const quarter = SCALE_PRESETS.find((p) => p.label.startsWith("1/4"))!;
    expect(presetRatio(quarter.factor, quarter.unit)).toBeCloseTo(1 / 18, 12);
    expect(presetRatio(100, "furlong")).toBe(0);
  });
});

describe("viewport changes are undoable with their measurements (review of PR #166)", () => {
  const ipc = () => ({
    add: async () => {}, update: async () => {}, remove: async () => {},
    putScale: vi.fn(async () => {}),
  });

  it("create + its recompute is one undo frame; undo removes the viewport and restores the quantity", async () => {
    const i = ipc();
    const store = new MarkupStore("d", i);
    const ts = new TakeoffStore();
    ts.seedScales([def("d", 0.01)]);
    store.seed([length("m", { x: 150, y: 150 }, 1000, null, 0)]);
    const v = vp("detail", 0, [100, 100, 200, 200], 0.001);
    commitViewportChange(store, ts, null, v, by);
    expect(ts.viewports.map((x) => x.id)).toEqual(["detail"]);
    expect(store.markups[0].measurement?.computed_quantity).toBeCloseTo(1, 9);
    store.undo();
    expect(ts.viewports).toEqual([]);
    expect(store.markups[0].measurement?.computed_quantity).toBe(0);
    await store.flush();
    expect(i.putScale).toHaveBeenLastCalledWith("d", "detail", null);
    store.redo();
    expect(ts.viewports.map((x) => x.id)).toEqual(["detail"]);
  });

  it("a retired (deleted) viewport is never picked and its measurements keep their values", () => {
    const retired = { ...vp("detail", 0, [100, 100, 200, 200], 0.001), retired: true };
    const scales = [def("d", 0.01), retired];
    expect(scaleAt(scales, 0, { x: 150, y: 150 })?.id).toBe("d");
    const m = length("m", { x: 150, y: 150 }, 1000, "detail", 1);
    expect(recomputeAll([m], scales, by, NOW)).toEqual([]);
    const ts = new TakeoffStore();
    ts.seedScales(scales);
    expect(ts.viewports).toEqual([]);
    expect(ts.scaleAt(0, { x: 150, y: 150 })?.id).toBe("d");
  });
});
