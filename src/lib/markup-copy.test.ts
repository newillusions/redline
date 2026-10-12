import { describe, it, expect } from "vitest";
import type { Markup } from "./ipc";
import {
  freshCopy, copySet, planPaste, isUsableMarkup, rescaleMeasurement, newCounts, joinCounts, countSetsOf,
  unionBounds, MAX_COPY_ITEMS,
} from "./markup-copy";
import { BY, NOW, mk, mkCount, scale, seqIds, SET_A } from "./arrange.test-util";

const one = (src: Markup, o: Partial<Parameters<typeof freshCopy>[1]> = {}) =>
  freshCopy(src, { identity: BY, now: NOW, id: "new1", page: 0, dx: 0, dy: 0, groupId: null, countGroup: null, keepLayer: true, ...o });

describe("freshCopy: a new redline-owned markup", () => {
  it("gets a new id, a fresh audit and an empty workflow", () => {
    const src = mk("src", {
      workflow: { status: "Accepted", assignee: { user_id: "a", display_name: "A" }, thread: [{ id: "r1", author: BY, at: NOW, contents: "hi" }], status_history: [{ id: "s1", author: BY, at: NOW, model: "Review", state: "Accepted" }] },
    });
    const c = one(src);
    expect(c.id).toBe("new1");
    expect(c.audit).toEqual({ created_by: BY, created_at: NOW, modified_by: BY, modified_at: NOW, revision: 0, origin: "Desktop" });
    expect(c.workflow).toEqual({ status: "None", assignee: null, thread: [], status_history: [] });
  });

  it("carries no reader-owned or foreign fields", () => {
    const src = {
      ...mk("src"), source_nm: "ABCDEF0123456789", source_rect: [1, 2, 3, 4], foreign_keys: [{ key: "BSIColumnData" }],
      rich_text: "<p>x</p>", raw_da: "0 g", optional_content: { Reference: [9, 0] },
    } as unknown as Markup;
    const c = one(src) as unknown as Record<string, unknown>;
    for (const k of ["source_nm", "source_rect", "rich_text", "raw_da"]) expect(c[k]).toBeUndefined();
    expect(c.foreign_keys).toEqual([]);
  });

  it("a copy in the same document keeps the layer name and /OC; another document gets neither", () => {
    const src = { ...mk("src", { layer: "Lighting" }), optional_content: { Reference: [9, 0] } } as unknown as Markup;
    const same = one(src) as unknown as Record<string, unknown>;
    expect(same.layer).toBe("Lighting");
    expect(same.optional_content).toEqual({ Reference: [9, 0] });
    const other = one(src, { keepLayer: false }) as unknown as Record<string, unknown>;
    expect(other.layer).toBeNull();
    expect(other.optional_content).toBeUndefined();
  });

  it("keeps subject, comment, layer and appearance, translated by (dx, dy); never aliases the source", () => {
    const src = mk("src", { subject: "S", contents: "C", layer: "L", rect: [0, 0, 10, 10] });
    const c = one(src, { dx: 5, dy: -3 });
    expect([c.subject, c.contents, c.layer]).toEqual(["S", "C", "L"]);
    expect(c.geometry).toEqual({ Rect: { min: { x: 5, y: -3 }, max: { x: 15, y: 7 } } });
    expect(c.appearance).toEqual(src.appearance);
    expect(c.appearance).not.toBe(src.appearance);
    expect(src.geometry).toEqual({ Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } });
  });

  it("clears the Locked and LockedContents bits only", () => {
    expect(one(mk("a", { annot_flags: 4 | 0x80 | 0x200 })).annot_flags).toBe(4);
    expect(one(mk("a", { annot_flags: 4 })).annot_flags).toBe(4);
    expect(one(mk("a")).annot_flags).toBeUndefined();
  });

  it("translates a callout's box and extra leaders too", () => {
    const callout = mk("c", {
      markup_type: "Callout", geometry: { Polyline: [{ x: 0, y: 0 }, { x: 10, y: 10 }] },
      callout: { box_min: { x: 10, y: 10 }, box_max: { x: 50, y: 30 }, extra_leaders: [[{ x: 0, y: 5 }, { x: 10, y: 15 }]], autosize: false },
    });
    const c = one(callout, { dx: 100, dy: 0 });
    expect(c.callout?.box_min).toEqual({ x: 110, y: 10 });
    expect(c.callout?.extra_leaders[0][0]).toEqual({ x: 100, y: 5 });
  });

  it("only count items keep a count_group, and take the one given", () => {
    expect(one(mkCount("k", 1, 1, "G"), { countGroup: "G2" }).count_group).toBe("G2");
    expect(one(mk("r", { count_group: "G" } as Partial<Markup>), { countGroup: "G2" }).count_group).toBeNull();
  });
});

describe("rescaleMeasurement: under the target page's scale", () => {
  const len = (page: number) => mk("l", {
    markup_type: "MeasurementLength", page, geometry: { Polyline: [{ x: 0, y: 0 }, { x: 100, y: 0 }] },
    measurement: { scale_ref: "old", raw_measure: 100, unit: "m", computed_quantity: 10, depth: null, count_value: null, custom_columns: {} },
  });
  it("uses the page scale when there is one", () => {
    const r = rescaleMeasurement(len(2), [scale("s2", 0.5, 2), scale("sd", 0.01)]);
    expect(r.measurement).toMatchObject({ scale_ref: "s2", computed_quantity: 50, unit: "m" });
  });
  it("falls back to the document default", () => {
    expect(rescaleMeasurement(len(2), [scale("sd", 0.01)]).measurement).toMatchObject({ scale_ref: "sd", computed_quantity: 1 });
  });
  it("with no scale at all it is unscaled, like a measurement drawn on an uncalibrated page", () => {
    expect(rescaleMeasurement(len(2), []).measurement).toMatchObject({ scale_ref: null, unit: "pt", computed_quantity: 0, raw_measure: 100 });
  });
  it("area scales by ratio squared; count and angle are left alone", () => {
    const area = mk("a", {
      markup_type: "MeasurementArea", geometry: { Polyline: [] },
      measurement: { scale_ref: null, raw_measure: 200, unit: "pt²", computed_quantity: 0, depth: null, count_value: null, custom_columns: {} },
    });
    expect(rescaleMeasurement(area, [scale("s", 0.1)]).measurement?.computed_quantity).toBeCloseTo(2, 9);
    const cnt = mkCount("k", 0, 0, null);
    expect(rescaleMeasurement(cnt, [scale("s", 0.1)])).toBe(cnt);
  });
});

describe("copySet: groups and counts", () => {
  const ctx = (mintId = seqIds()) => ({ identity: BY, now: NOW, mintId, page: 0, dx: 0, dy: 0, countGroupOf: () => null });
  it("a copied group is a NEW group, never joined to the original", () => {
    const out = copySet([mk("a", { group_id: "G" }), mk("b", { group_id: "G" }), mk("c")], ctx());
    expect(out[0].group_id).toBeTruthy();
    expect(out[0].group_id).toBe(out[1].group_id);
    expect(out[0].group_id).not.toBe("G");
    expect(out[2].group_id).toBeNull();
  });
  it("a group left with a single member is dissolved", () => {
    const out = copySet([mk("a", { group_id: "G" })], ctx());
    expect(out[0].group_id).toBeNull();
  });
  it("newCounts: each source count becomes one new count; a count of one stays ungrouped", () => {
    const f = newCounts(seqIds("g"));
    const a = f(mkCount("1", 0, 0, "G")), b = f(mkCount("2", 0, 0, "G")), c = f(mkCount("3", 0, 0, "H")), d = f(mkCount("4", 0, 0, null));
    expect(a).toBe(b);
    expect(a).not.toBe("G");
    expect(c).not.toBe(a);
    expect(d).toBeNull();
  });
  it("joinCounts: a copy joins the source's count; an ungrouped source's key is its own id", () => {
    const sizes = new Map([["G", 3]]);
    const f = joinCounts(sizes, seqIds("g"));
    expect(f(mkCount("1", 0, 0, "G"))).toBe("G");
    expect(f(mkCount("solo", 0, 0, null))).toBe("solo");
    expect(sizes.get("G")).toBe(4);
    expect(f(mk("r"))).toBeNull();
  });
  it("joinCounts: a count at its cap continues in a fresh one", () => {
    const sizes = new Map([["G", 10_000]]);
    const f = joinCounts(sizes, seqIds("g"));
    const g1 = f(mkCount("1", 0, 0, "G"));
    expect(g1).not.toBe("G");
    expect(f(mkCount("2", 0, 0, "G"))).toBe(g1);
  });
  it("countSetsOf lists each set once", () => {
    expect(countSetsOf([mkCount("1", 0, 0, null), mkCount("2", 0, 0, null)])).toEqual([SET_A]);
  });
});

describe("isUsableMarkup: the clipboard is untrusted", () => {
  it("accepts a real markup", () => expect(isUsableMarkup(mk("a"))).toBe(true));
  it.each([
    ["null", null], ["a string", "x"], ["an array", []],
    ["no id", { ...mk("a"), id: "" }],
    ["unknown type", { ...mk("a"), markup_type: "Nuke" }],
    ["fractional page", { ...mk("a"), page: 1.5 }],
    ["negative page", { ...mk("a"), page: -1 }],
    ["NaN coordinate", { ...mk("a"), geometry: { Rect: { min: { x: NaN, y: 0 }, max: { x: 1, y: 1 } } } }],
    ["Infinity coordinate", { ...mk("a"), geometry: { Point: { x: Infinity, y: 0 } } }],
    ["two geometry keys", { ...mk("a"), geometry: { Point: { x: 0, y: 0 }, Rect: { min: { x: 0, y: 0 }, max: { x: 1, y: 1 } } } }],
    ["no geometry", { ...mk("a"), geometry: {} }],
    ["too many vertices", { ...mk("a"), geometry: { Polyline: Array.from({ length: 10_001 }, () => ({ x: 0, y: 0 })) } }],
    ["no appearance", { ...mk("a"), appearance: null }],
    ["non-string comment", { ...mk("a"), contents: 5 }],
    ["NaN line weight", { ...mk("a"), appearance: { ...mk("a").appearance, line_weight: NaN } }],
    ["negative line weight", { ...mk("a"), appearance: { ...mk("a").appearance, line_weight: -1 } }],
    ["opacity above 1", { ...mk("a"), appearance: { ...mk("a").appearance, opacity: 2 } }],
    ["null opacity", { ...mk("a"), appearance: { ...mk("a").appearance, opacity: null } }],
    ["NaN fill opacity", { ...mk("a"), appearance: { ...mk("a").appearance, fill_opacity: NaN } }],
    ["bad line style", { ...mk("a"), appearance: { ...mk("a").appearance, line_style: "Wavy" } }],
    ["non-finite font size", { ...mk("a"), appearance: { ...mk("a").appearance, font: { family: "F", size_pt: Infinity } } }],
    ["huge font size", { ...mk("a"), appearance: { ...mk("a").appearance, font: { family: "F", size_pt: 1e9 } } }],
    ["numeric fill", { ...mk("a"), appearance: { ...mk("a").appearance, fill: 5 } }],
    ["NaN callout box", { ...mk("a"), callout: { box_min: { x: NaN, y: 0 }, box_max: { x: 1, y: 1 }, extra_leaders: [], autosize: false } }],
    ["callout with 1000 leaders", { ...mk("a"), callout: { box_min: { x: 0, y: 0 }, box_max: { x: 1, y: 1 }, extra_leaders: Array.from({ length: 1000 }, () => []), autosize: false } }],
    ["count set without a symbol", { ...mk("a"), count_set: { id: "x", name: "n", color: "#000" } }],
    ["non-finite measurement", { ...mk("a"), measurement: { raw_measure: NaN, computed_quantity: 1, unit: "m" } }],
  ])("rejects %s", (_n, v) => expect(isUsableMarkup(v)).toBe(false));
});

describe("planPaste", () => {
  const base = { identity: BY, now: NOW, sourceDocId: "d1", targetDocId: "d1", pageCount: 5, scales: [], dx: 0, dy: 0 };
  it("creates one new markup per item on each target page", () => {
    const plan = planPaste({ ...base, mintId: seqIds(), items: [mk("a"), mk("b")], pages: [1, 3] });
    expect(plan.creates).toHaveLength(4);
    expect(plan.creates.map((m) => m.page)).toEqual([1, 1, 3, 3]);
    expect(new Set(plan.creates.map((m) => m.id)).size).toBe(4);
  });
  it("skips unusable items and counts them", () => {
    const plan = planPaste({ ...base, mintId: seqIds(), items: [mk("a"), { junk: true }, null], pages: [0] });
    expect(plan.creates).toHaveLength(1);
    expect(plan.skipped).toBe(2);
  });
  it("refuses a page outside the document, no pages, an empty clipboard, and an oversize paste", () => {
    const items = [mk("a")];
    expect(planPaste({ ...base, mintId: seqIds(), items, pages: [5] }).error).toBeTruthy();
    expect(planPaste({ ...base, mintId: seqIds(), items, pages: [-1] }).error).toBeTruthy();
    expect(planPaste({ ...base, mintId: seqIds(), items, pages: [] }).error).toBeTruthy();
    expect(planPaste({ ...base, mintId: seqIds(), items: [], pages: [0] }).error).toBeTruthy();
    expect(planPaste({ ...base, mintId: seqIds(), items: [{ junk: 1 }], pages: [0] }).error).toMatch(/not usable/);
    const big = Array.from({ length: 100 }, (_, i) => mk(`m${i}`));
    const r = planPaste({ ...base, pageCount: 1000, mintId: seqIds(), items: big, pages: Array.from({ length: 101 }, (_, i) => i) });
    expect(r.error).toMatch(String(MAX_COPY_ITEMS));
    expect(r.creates).toEqual([]);
  });
  it("same document and page keeps the measurement; another page recomputes under that page's scale", () => {
    const len = mk("l", {
      markup_type: "MeasurementLength", page: 0, geometry: { Polyline: [{ x: 0, y: 0 }, { x: 100, y: 0 }] },
      measurement: { scale_ref: "s0", raw_measure: 100, unit: "m", computed_quantity: 10, depth: null, count_value: null, custom_columns: {} },
    });
    const same = planPaste({ ...base, mintId: seqIds(), items: [len], pages: [0], scales: [scale("s0", 0.1, 0), scale("s2", 1, 2)] });
    expect(same.creates[0].measurement?.computed_quantity).toBe(10);
    const other = planPaste({ ...base, mintId: seqIds(), items: [len], pages: [2], scales: [scale("s0", 0.1, 0), scale("s2", 1, 2)] });
    expect(other.creates[0].measurement).toMatchObject({ scale_ref: "s2", computed_quantity: 100 });
    const cross = planPaste({ ...base, targetDocId: "d2", mintId: seqIds(), items: [len], pages: [0], scales: [scale("t", 2, 0)] });
    expect(cross.creates[0].measurement).toMatchObject({ scale_ref: "t", computed_quantity: 200 });
  });
  it("keeps layer membership within a document and drops it across documents", () => {
    const src = { ...mk("l", { layer: "Lighting" }), optional_content: { Reference: [9, 0] } } as unknown as Markup;
    const same = planPaste({ ...base, mintId: seqIds(), items: [src], pages: [1] }).creates[0] as unknown as Record<string, unknown>;
    expect([same.layer, same.optional_content]).toEqual(["Lighting", { Reference: [9, 0] }]);
    const cross = planPaste({ ...base, targetDocId: "d2", mintId: seqIds(), items: [src], pages: [1] }).creates[0] as unknown as Record<string, unknown>;
    expect([cross.layer, cross.optional_content]).toEqual([null, undefined]);
  });
  it("reports the count sets of pasted counts", () => {
    const plan = planPaste({ ...base, mintId: seqIds(), items: [mkCount("k", 0, 0, "G")], pages: [0] });
    expect(plan.countSets).toEqual([SET_A]);
  });
});

describe("unionBounds", () => {
  it("spans every item and is null for none", () => {
    expect(unionBounds([mk("a", { rect: [0, 0, 5, 5] }), mk("b", { rect: [10, -2, 20, 3] })])).toEqual({ minX: 0, minY: -2, maxX: 20, maxY: 5 });
    expect(unionBounds([])).toBeNull();
  });
});
