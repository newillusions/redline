import { describe, it, expect } from "vitest";
import type { Markup, UserRef } from "./ipc";
import {
  expandCountSelection, countGroups, nextRun, resumeRun, splitSelected, splitAll, mergeSelected, setLabelColour,
  MAX_COUNT_GROUP_ITEMS,
} from "./count-groups";

const by: UserRef = { user_id: "u", display_name: "T" };
const NOW = "2026-10-12T10:00:00Z";
let n = 0;
const mint = () => `g-${++n}`;

const SET_A = { id: "setA", name: "Type A", color: "#e02424", symbol: "Circle" as const };
const SET_B = { id: "setB", name: "Type B", color: "#1d70b8", symbol: "Square" as const };

function item(id: string, group: string | null, o: Partial<Markup> = {}): Markup {
  return {
    id, markup_type: "MeasurementCount", page: 0, geometry: { Point: { x: 1, y: 1 } },
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null },
    subject: null, layer: null, contents: null, group_id: null, count_group: group,
    audit: { created_by: by, created_at: NOW, modified_by: by, modified_at: NOW, revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: { scale_ref: null, raw_measure: 1, unit: "ea", computed_quantity: 1, depth: null, count_value: 1, custom_columns: {} },
    count_set: SET_A,
    ...o,
  } as Markup;
}

describe("countGroups", () => {
  it("totals per group; an ungrouped item is a count of one", () => {
    const g = countGroups([item("a", "G"), item("b", "G"), item("c", null)]);
    expect(g.map((x) => [x.key, x.total])).toEqual([["G", 2], ["c", 1]]);
  });
  it("ignores non-count markups", () => {
    const rect = item("r", null, { markup_type: "Rectangle" });
    expect(countGroups([rect])).toEqual([]);
  });
});

describe("nextRun (items placed in one run are one count)", () => {
  it("continues the run on the same page and set", () => {
    const r1 = nextRun(null, 0, "setA", mint);
    const r2 = nextRun(r1, 0, "setA", mint);
    expect(r2.groupId).toBe(r1.groupId);
    expect(r2.size).toBe(2);
  });
  it("starts a new count on page, set change, or when full", () => {
    const r1 = nextRun(null, 0, "setA", mint);
    expect(nextRun(r1, 1, "setA", mint).groupId).not.toBe(r1.groupId);
    expect(nextRun(r1, 0, "setB", mint).groupId).not.toBe(r1.groupId);
    const full = { ...r1, size: MAX_COUNT_GROUP_ITEMS };
    expect(nextRun(full, 0, "setA", mint).groupId).not.toBe(r1.groupId);
  });
});

describe("resumeRun", () => {
  it("returns the run of the selected item's count with its size", () => {
    const r = resumeRun([item("a", "G"), item("b", "G")], "a");
    expect("run" in r && r.run).toEqual({
      groupId: "G", page: 0, setId: "setA", size: 2, subject: null, color: "#e02424",
    });
  });
  it("a legacy ungrouped item resumes as its own count and is STAMPED so all views agree", () => {
    const ms = [item("a", null)];
    const r = resumeRun(ms, "a", by, NOW);
    if (!("run" in r)) throw new Error(r.error);
    expect(r.run.groupId).toBe("a");
    expect(r.pairs).toHaveLength(1);
    expect(r.pairs[0].after.count_group).toBe("a");
    expect(r.pairs[0].after.audit.revision).toBe(1);
    // After applying the stamp and two resumed clicks, Quantities (groupKey) and the
    // Markups List/exports (count_group) see ONE count of 3.
    const placed = [r.pairs[0].after, item("r1", "a"), item("r2", "a")];
    expect(countGroups(placed).map((g) => [g.key, g.total])).toEqual([["a", 3]]);
    expect(new Set(placed.map((m) => m.count_group))).toEqual(new Set(["a"]));
  });
  it("an already grouped item needs no stamp", () => {
    const r = resumeRun([item("a", "G")], "a", by, NOW);
    expect("run" in r && r.pairs).toEqual([]);
  });
  it("refuses a LOCKED legacy ungrouped item (no stamp possible, would split the count)", () => {
    expect("error" in resumeRun([item("a", null, { annot_flags: 0x80 })], "a", by, NOW)).toBe(true);
    expect("error" in resumeRun([item("a", null, { annot_flags: 0x200 })], "a", by, NOW)).toBe(true);
  });
  it("a locked but already grouped item still resumes (nothing to stamp)", () => {
    const r = resumeRun([item("a", "G", { annot_flags: 0x80 })], "a", by, NOW);
    expect("run" in r && r.pairs).toEqual([]);
  });
  it("refuses non-count and unknown ids", () => {
    expect("error" in resumeRun([item("r", null, { markup_type: "Rectangle" })], "r")).toBe(true);
    expect("error" in resumeRun([], "zz")).toBe(true);
  });
});

describe("splitSelected", () => {
  const ms = [item("a", "G"), item("b", "G"), item("c", "G")];
  it("moves the selected items into one new count, leaving the rest", () => {
    const r = splitSelected(ms, ["b", "c"], by, NOW, () => "NEW");
    expect(r.ok && r.pairs.map((p) => [p.after.id, p.after.count_group])).toEqual([["b", "NEW"], ["c", "NEW"]]);
    expect(r.ok && r.pairs[0].after.audit.revision).toBe(1);
    expect(r.ok && r.pairs[0].before.count_group).toBe("G");
  });
  it("totals add up after a split (nothing lost or duplicated)", () => {
    const r = splitSelected(ms, ["c"], by, NOW, () => "NEW");
    if (!r.ok) throw new Error("expected ok");
    const after = ms.map((m) => r.pairs.find((p) => p.after.id === m.id)?.after ?? m);
    const totals = Object.fromEntries(countGroups(after).map((g) => [g.key, g.total]));
    expect(totals).toEqual({ G: 2, NEW: 1 });
  });
  it("refuses selecting the whole count or nothing", () => {
    expect(splitSelected(ms, ["a", "b", "c"], by, NOW, mint).ok).toBe(false);
    expect(splitSelected(ms, [], by, NOW, mint).ok).toBe(false);
  });
});

describe("splitAll", () => {
  it("each selected item becomes its own count, one stays when the whole count is selected", () => {
    let i = 0;
    const r = splitAll([item("a", "G"), item("b", "G"), item("c", "G")], ["a", "b", "c"], by, NOW, () => `N${++i}`);
    if (!r.ok) throw new Error("expected ok");
    expect(r.pairs.map((p) => [p.after.id, p.after.count_group])).toEqual([["b", "N1"], ["c", "N2"]]);
  });
  it("a partial selection moves every selected item out", () => {
    let i = 0;
    const r = splitAll([item("a", "G"), item("b", "G"), item("c", "G")], ["a", "b"], by, NOW, () => `N${++i}`);
    expect(r.ok && r.pairs.length).toBe(2);
  });
  it("refuses when everything is already a count of one", () => {
    expect(splitAll([item("a", null)], ["a"], by, NOW, mint).ok).toBe(false);
  });
});

describe("mergeSelected", () => {
  it("merges whole counts of the same set and page; first count's label/colour win", () => {
    const ms = [
      item("a", "G1", { subject: "Down", appearance: { ...item("x", null).appearance, color: "#111111" } }),
      item("b", "G1"),
      item("c", "G2"),
      item("d", null),
    ];
    const r = mergeSelected(ms, ["b", "c", "d"], by, NOW);
    if (!r.ok) throw new Error("expected ok");
    const after = ms.map((m) => r.pairs.find((p) => p.after.id === m.id)?.after ?? m);
    const groups = countGroups(after);
    expect(groups).toHaveLength(1);
    expect(groups[0].total).toBe(4);
    expect(groups[0].label).toBe("Down");
    expect(after.every((m) => m.appearance.color === "#111111")).toBe(true);
  });
  it("refuses a single count, different sets, different pages", () => {
    expect(mergeSelected([item("a", "G"), item("b", "G")], ["a", "b"], by, NOW).ok).toBe(false);
    expect(mergeSelected([item("a", "G1"), item("b", "G2", { count_set: SET_B })], ["a", "b"], by, NOW).ok).toBe(false);
    expect(mergeSelected([item("a", "G1"), item("b", "G2", { page: 3 })], ["a", "b"], by, NOW).ok).toBe(false);
  });
  it("refuses a merge past the item cap", () => {
    const big = Array.from({ length: MAX_COUNT_GROUP_ITEMS }, (_, i) => item(`i${i}`, "BIG"));
    const r = mergeSelected([...big, item("z", "G2")], ["i0", "z"], by, NOW);
    expect(r.ok).toBe(false);
  });
});

describe("setLabelColour", () => {
  it("labels and recolours the whole count from any one of its items", () => {
    const ms = [item("a", "G"), item("b", "G"), item("c", "H")];
    const pairs = setLabelColour(ms, ["a"], { label: "Exit sign", color: "#00ff00" }, by, NOW);
    expect(pairs.map((p) => p.after.id)).toEqual(["a", "b"]);
    expect(pairs.every((p) => p.after.subject === "Exit sign" && p.after.appearance.color === "#00ff00")).toBe(true);
  });
  it("an empty label clears the subject", () => {
    const pairs = setLabelColour([item("a", "G", { subject: "x" })], ["a"], { label: "" }, by, NOW);
    expect(pairs[0].after.subject).toBeNull();
  });
});

describe("expandCountSelection", () => {
  const ms = [item("a", "G"), item("b", "G"), item("c", "H"), item("r", null, { markup_type: "Rectangle" })];
  it("selecting one item selects its whole count only", () => {
    expect([...expandCountSelection(ms, new Set(["a"]))].sort()).toEqual(["a", "b"]);
  });
  it("leaves other selections untouched", () => {
    const sel = new Set(["r"]);
    expect(expandCountSelection(ms, sel)).toBe(sel);
    expect([...expandCountSelection(ms, new Set(["r", "c"]))].sort()).toEqual(["c", "r"]);
  });
});
