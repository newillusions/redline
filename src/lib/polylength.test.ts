import { describe, it, expect } from "vitest";
import type { Markup, ScaleRecord, UserRef } from "./ipc";
import {
  convertLength, riseDropTotal, polylengthPayload, recomputeMeasurement, recomputeAll,
  setSegmentRiseDrop, splitPolylengthAt, effectiveScale, segmentLengths, scaleFor, isKnownLengthUnit,
} from "./polylength";

const by: UserRef = { user_id: "u", display_name: "T" };
const NOW = "2026-10-12T10:00:00Z";
const scale = (id: string, ratio: number, unit = "m", page: number | null = null): ScaleRecord => ({
  id, applies_to: page === null ? { kind: "DocumentDefault" } : { kind: "Page", page },
  method: "Preset", ratio, unit, label: id, precision: 2,
});

// 3 segments of 1000 pt each: (0,0)-(1000,0)-(1000,1000)-(2000,1000)
const PTS = [{ x: 0, y: 0 }, { x: 1000, y: 0 }, { x: 1000, y: 1000 }, { x: 2000, y: 1000 }];

function polylen(scaleObj: ScaleRecord | null, runs = [] as { segment: number; amount: number; unit: string }[]): Markup {
  return {
    id: "pl", markup_type: "MeasurementLength", page: 0, geometry: { Polyline: PTS },
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null },
    subject: "Cable", layer: null, contents: null, group_id: null,
    audit: { created_by: by, created_at: NOW, modified_by: by, modified_at: NOW, revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: polylengthPayload(PTS, scaleObj, runs),
  } as Markup;
}

describe("unit conversion", () => {
  it("converts known units, passes same unit, refuses unknown pairs", () => {
    expect(convertLength(2, "ft", "m")).toBeCloseTo(0.6096, 9);
    expect(convertLength(1000, "mm", "m")).toBeCloseTo(1, 9);
    expect(convertLength(5, "bay", "bay")).toBe(5);
    expect(convertLength(5, "bay", "m")).toBeNull();
  });
});

describe("rise/drop arithmetic", () => {
  it("rise and drop both add; magnitude only", () => {
    expect(riseDropTotal([{ segment: 0, amount: 2, unit: "m" }, { segment: 1, amount: -1.5, unit: "m" }], "m").total).toBe(3.5);
  });
  it("counts unconvertible and non-finite runs instead of guessing", () => {
    const r = riseDropTotal([{ segment: 0, amount: 5, unit: "bay" }, { segment: 1, amount: NaN, unit: "m" }, { segment: 2, amount: 1, unit: "m" }], "m");
    expect(r).toEqual({ total: 1, unconverted: 2 });
  });
  it("total = path x ratio + rise/drop (3000 pt x 0.001 + 2 + 1 = 6)", () => {
    const s = scale("a", 0.001);
    const p = polylengthPayload(PTS, s, [{ segment: 0, amount: 2, unit: "m" }, { segment: 2, amount: -1, unit: "m" }]);
    expect(p.raw_measure).toBe(3000);
    expect(p.computed_quantity).toBeCloseTo(6, 9);
    expect(p.unit).toBe("m");
  });
  it("no scale: unscaled points, quantity 0", () => {
    const p = polylengthPayload(PTS, null);
    expect(p.unit).toBe("pt");
    expect(p.computed_quantity).toBe(0);
  });
});

describe("totals under calibration", () => {
  it("recalibrating changes the scaled part, not the rise/drop", () => {
    const m = polylen(scale("a", 0.001), [{ segment: 1, amount: 2, unit: "m" }]);
    expect(m.measurement!.computed_quantity).toBeCloseTo(5, 9);
    const r = recomputeMeasurement(m, scale("b", 0.002));
    expect(r.measurement!.computed_quantity).toBeCloseTo(8, 9); // 3000x0.002 + 2
    expect(r.measurement!.scale_ref).toBe("b");
    expect(m.measurement!.computed_quantity).toBeCloseTo(5, 9); // input untouched
  });
  it("a unit change converts the rise/drop (2 ft -> 0.6096 m)", () => {
    const m = polylen(scale("a", 1, "ft"), [{ segment: 0, amount: 2, unit: "ft" }]);
    const r = recomputeMeasurement(m, scale("b", 0.001, "m"));
    expect(r.measurement!.computed_quantity).toBeCloseTo(3 + 0.6096, 9);
  });
  it("area, perimeter, radius and volume follow the creation formulas; angle/count are untouched", () => {
    const base = polylen(null);
    const mk = (t: Markup["markup_type"], raw: number, depth: number | null = null): Markup =>
      ({ ...base, markup_type: t, measurement: { ...base.measurement!, raw_measure: raw, depth, rise_drop: [] } });
    const s = scale("s", 0.01);
    expect(recomputeMeasurement(mk("MeasurementArea", 1e6), s).measurement!.computed_quantity).toBeCloseTo(100, 9);
    expect(recomputeMeasurement(mk("MeasurementPerimeter", 500), s).measurement!.computed_quantity).toBeCloseTo(5, 9);
    expect(recomputeMeasurement(mk("MeasurementRadius", 500), s).measurement!.computed_quantity).toBeCloseTo(5, 9);
    const v = recomputeMeasurement(mk("MeasurementVolume", 1e6, 2), s);
    expect(v.measurement!.computed_quantity).toBeCloseTo(200, 9);
    expect(v.measurement!.unit).toBe("m³");
    const ang = mk("MeasurementAngle", 42);
    expect(recomputeMeasurement(ang, s)).toBe(ang);
  });
  it("rejects a non-positive or non-finite ratio", () => {
    const m = polylen(scale("a", 0.001));
    expect(recomputeMeasurement(m, scale("x", NaN))).toBe(m);
    expect(recomputeMeasurement(m, scale("x", 0))).toBe(m);
  });
});

describe("recomputeAll (recalibration)", () => {
  const a = scale("old", 0.001);
  it("brings measurements with a replaced/dangling/empty scale under the page's scale", () => {
    const stale = polylen(a);
    const none = { ...polylen(null), id: "none" };
    const out = recomputeAll([stale, none], [scale("new", 0.002)], by, NOW);
    expect(out.map((p) => p.after.id).sort()).toEqual(["none", "pl"]);
    expect(out.find((p) => p.after.id === "pl")!.after.measurement!.computed_quantity).toBeCloseTo(6, 9);
    expect(out[0].after.audit.revision).toBe(1);
  });
  it("keeps a measurement deliberately pinned to another existing scale; page scale beats default", () => {
    const pinned = { ...polylen(scale("other", 0.5)), id: "pinned" };
    const onP1 = { ...polylen(a), id: "p1", page: 1 };
    const scales = [scale("def", 0.001), scale("pg1", 0.01, "m", 1), scale("other", 0.5)];
    const out = recomputeAll([pinned, onP1], scales, by, NOW);
    expect(out.map((p) => p.after.id)).toEqual(["p1"]);
    expect(out[0].after.measurement!.computed_quantity).toBeCloseTo(30, 9);
  });
  it("omits markups that would not change, and does nothing without a scale", () => {
    const s = scale("a", 0.001);
    expect(recomputeAll([polylen(s)], [s], by, NOW)).toEqual([]);
    expect(recomputeAll([polylen(s)], [], by, NOW)).toEqual([]);
  });
  it("effectiveScale prefers the page scale", () => {
    expect(effectiveScale([scale("d", 1), scale("p", 2, "m", 3)], 3)!.id).toBe("p");
    expect(effectiveScale([scale("d", 1)], 3)!.id).toBe("d");
    expect(effectiveScale([], 3)).toBeNull();
  });
});

describe("setSegmentRiseDrop", () => {
  const s = scale("a", 0.001);
  it("sets, replaces and clears a segment's run and recomputes", () => {
    const m = polylen(s);
    const r1 = setSegmentRiseDrop(m, 1, 2, "m", s, by, NOW);
    if (!r1.ok) throw new Error(r1.error);
    expect(r1.markup.measurement!.computed_quantity).toBeCloseTo(5, 9);
    const r2 = setSegmentRiseDrop(r1.markup, 1, -4, "m", s, by, NOW);
    if (!r2.ok) throw new Error(r2.error);
    expect(r2.markup.measurement!.rise_drop).toEqual([{ segment: 1, amount: -4, unit: "m" }]);
    expect(r2.markup.measurement!.computed_quantity).toBeCloseTo(7, 9);
    const r3 = setSegmentRiseDrop(r2.markup, 1, 0, "m", s, by, NOW);
    if (!r3.ok) throw new Error(r3.error);
    expect(r3.markup.measurement!.rise_drop).toEqual([]);
    expect(r3.markup.measurement!.computed_quantity).toBeCloseTo(3, 9);
  });
  it("rejects bad segments and hostile amounts", () => {
    const m = polylen(s);
    for (const [seg, amt] of [[3, 1], [-1, 1], [0.5, 1], [0, NaN], [0, Infinity], [0, 1e12]] as const) {
      expect(setSegmentRiseDrop(m, seg, amt, "m", s, by, NOW).ok).toBe(false);
    }
    const area = { ...m, markup_type: "MeasurementArea" } as Markup;
    expect(setSegmentRiseDrop(area, 0, 1, "m", s, by, NOW).ok).toBe(false);
  });
});

describe("splitPolylengthAt", () => {
  const s = scale("a", 0.001);
  const runs = [{ segment: 0, amount: 1, unit: "m" }, { segment: 2, amount: 2, unit: "m" }];
  it("splits geometry at the vertex; both halves sum to the original (raw and total)", () => {
    const m = polylen(s, runs);
    const r = splitPolylengthAt(m, 2, s, by, NOW, () => "NEW");
    if (!r.ok) throw new Error(r.error);
    expect(r.first.id).toBe("pl");
    expect(r.second.id).toBe("NEW");
    expect("Polyline" in r.first.geometry && r.first.geometry.Polyline).toHaveLength(3);
    expect("Polyline" in r.second.geometry && r.second.geometry.Polyline).toHaveLength(2);
    const sum = r.first.measurement!.computed_quantity + r.second.measurement!.computed_quantity;
    expect(sum).toBeCloseTo(m.measurement!.computed_quantity, 9);
    expect(r.first.measurement!.raw_measure + r.second.measurement!.raw_measure).toBeCloseTo(3000, 9);
  });
  it("assigns rise/drop to the half its segment lands in, re-indexing the second", () => {
    const r = splitPolylengthAt(polylen(s, runs), 2, s, by, NOW, () => "NEW");
    if (!r.ok) throw new Error(r.error);
    expect(r.first.measurement!.rise_drop).toEqual([{ segment: 0, amount: 1, unit: "m" }]);
    expect(r.second.measurement!.rise_drop).toEqual([{ segment: 0, amount: 2, unit: "m" }]);
  });
  it("the new half is a clean markup (fresh audit, no reader-owned fields)", () => {
    const m = { ...polylen(s), source_nm: "ABCDEFGHIJKLMNOP", workflow: { status: "Accepted", assignee: null, thread: [] } } as Markup;
    const r = splitPolylengthAt(m, 1, s, by, NOW, () => "NEW");
    if (!r.ok) throw new Error(r.error);
    expect(r.second.source_nm).toBeNull();
    expect(r.second.audit.revision).toBe(0);
    expect(r.second.workflow.status).toBe("None");
    expect(r.first.audit.revision).toBe(1);
  });
  it("only interior vertices are valid", () => {
    const m = polylen(s);
    for (const v of [0, 3, -1, 1.5]) expect(splitPolylengthAt(m, v, s, by, NOW, () => "x").ok).toBe(false);
  });
  it("segmentLengths gives per-segment point lengths", () => {
    expect(segmentLengths(PTS)).toEqual([1000, 1000, 1000]);
  });
});

describe("no scale record available (e.g. the sidecar scale is gone) - never wrong numbers", () => {
  const s = scale("a", 0.001);
  const runs = [{ segment: 0, amount: 1, unit: "m" }, { segment: 2, amount: 2, unit: "m" }];

  it("split with scale=null partitions the total by path length; halves sum exactly", () => {
    const m = polylen(s, runs); // 3 + 1 + 2 = 6 m
    expect(m.measurement!.computed_quantity).toBeCloseTo(6, 9);
    for (const v of [1, 2]) {
      const r = splitPolylengthAt(m, v, null, by, NOW, () => "NEW");
      if (!r.ok) throw new Error(r.error);
      const sum = r.first.measurement!.computed_quantity + r.second.measurement!.computed_quantity;
      expect(sum).toBeCloseTo(6, 9);
      expect(r.first.measurement!.computed_quantity).toBeLessThan(6);
      expect(r.second.measurement!.computed_quantity).toBeLessThan(6);
    }
    // split@1: first = 1000 x 0.001 + 1 = 2 ; second = 2000 x 0.001 + 2 = 4
    const r1 = splitPolylengthAt(m, 1, null, by, NOW, () => "NEW");
    if (!r1.ok) throw new Error(r1.error);
    expect(r1.first.measurement!.computed_quantity).toBeCloseTo(2, 9);
    expect(r1.second.measurement!.computed_quantity).toBeCloseTo(4, 9);
  });

  it("split of an unscaled (0, pt) polylength stays 0 + 0", () => {
    const m = polylen(null);
    const r = splitPolylengthAt(m, 1, null, by, NOW, () => "NEW");
    if (!r.ok) throw new Error(r.error);
    expect(r.first.measurement!.computed_quantity + r.second.measurement!.computed_quantity).toBe(0);
  });

  it("rise/drop with scale=null recomputes from the measurement's own unit", () => {
    const m = polylen(s); // 3 m
    const r = setSegmentRiseDrop(m, 1, 5, "pt", null, by, NOW);
    if (!r.ok) throw new Error(r.error);
    expect(r.markup.measurement!.computed_quantity).toBeCloseTo(8, 9);
    expect(r.markup.measurement!.rise_drop).toEqual([{ segment: 1, amount: 5, unit: "m" }]); // not "pt"
    const cleared = setSegmentRiseDrop(polylen(s, runs), 0, 0, "m", null, by, NOW);
    if (!cleared.ok) throw new Error(cleared.error);
    expect(cleared.markup.measurement!.computed_quantity).toBeCloseTo(5, 9);
  });

  it("rise/drop while genuinely unscaled (unit pt) is refused, never stored as 'pt'", () => {
    const r = setSegmentRiseDrop(polylen(null), 0, 2, "pt", null, by, NOW);
    expect(r.ok).toBe(false);
    expect(!r.ok && r.error).toMatch(/Calibrate a scale/);
    // clearing is still allowed
    expect(setSegmentRiseDrop(polylen(null), 0, 0, "pt", null, by, NOW).ok).toBe(true);
  });

  it("isKnownLengthUnit", () => {
    expect(isKnownLengthUnit("m")).toBe(true);
    expect(isKnownLengthUnit("pt")).toBe(false);
  });
});

describe("scaleFor (pinned scale is never re-pointed)", () => {
  const A = scale("A", 0.001);
  const B = scale("B", 0.01);
  it("uses the measurement's own existing scale_ref over the page scale", () => {
    const m = polylen(B);
    expect(scaleFor(m, [A, B])!.id).toBe("B");
    const r = setSegmentRiseDrop(m, 0, 1, "m", scaleFor(m, [A, B]), by, NOW);
    if (!r.ok) throw new Error(r.error);
    expect(r.markup.measurement!.scale_ref).toBe("B");
    expect(r.markup.measurement!.computed_quantity).toBeCloseTo(31, 9); // 3000 x 0.01 + 1
  });
  it("falls back to the page's scale for a dangling or empty scale_ref", () => {
    expect(scaleFor(polylen(scale("gone", 1)), [A, B])!.id).toBe("A");
    expect(scaleFor(polylen(null), [A])!.id).toBe("A");
    expect(scaleFor(polylen(null), [])).toBeNull();
  });
});
