import { describe, it, expect, beforeEach } from "vitest";
import type { Markup, MarkupGeometry } from "./ipc";
import type { MarkupIpc } from "./markup-store.svelte";
import { markupClipboard } from "./markup-clipboard.svelte";
import {
  copySelection, cutSelection, pasteClipboard, pasteToPages, duplicateSelection, multiplySelection,
  alignSelection, distributeSelection, orderSelection, pagesFromRanges, pageScale, type ArrangeCtx,
} from "./arrange-actions";
import { countGroups } from "./count-groups";
import { countSubtotals } from "./measurement-tools";
import { legendFromCountSets, computeRows, sanitizeLegend } from "./legend";
import { BY, NOW, SET_A, mk, mkCount, scale, seqIds, storeWith } from "./arrange.test-util";

const rect = (m: Markup) => (m.geometry as Extract<MarkupGeometry, { Rect: unknown }>).Rect;

// One id source for the whole file, like crypto.randomUUID in the app: ids never repeat across calls.
const mintId = seqIds("id");
function ctxFor(store: ReturnType<typeof storeWith>, o: Partial<ArrangeCtx> = {}): ArrangeCtx {
  return {
    store, identity: BY, scales: [], pageIndex: 0, pageCount: 10, pageWidthPts: 600, pageHeightPts: 400,
    cursor: null, viewCenter: null, now: () => NOW, mintId, ...o,
  };
}
const select = (s: ReturnType<typeof storeWith>, ...ids: string[]) => { s.selectedIds = new Set(ids); };
const ids = (ms: readonly Markup[]) => ms.map((m) => m.id);

beforeEach(() => markupClipboard.clear());

describe("copy / paste", () => {
  it("copy refuses an empty selection; paste refuses an empty clipboard", () => {
    const s = storeWith([mk("a")]);
    expect(copySelection(ctxFor(s)).ok).toBe(false);
    expect(pasteClipboard(ctxFor(s), "cursor").ok).toBe(false);
  });

  it("Ctrl+V puts the lower-left of the clipboard's box on the cursor; the copy is selected", () => {
    const s = storeWith([mk("a", { rect: [10, 10, 30, 20] }), mk("b", { rect: [50, 40, 60, 70] })]);
    select(s, "a", "b");
    copySelection(ctxFor(s));
    const res = pasteClipboard(ctxFor(s, { cursor: { x: 200, y: 100 } }), "cursor");
    expect(res).toEqual({ ok: true, message: "Pasted 2 markups" });
    const [na, nb] = s.markups.slice(2);
    expect(rect(na)).toEqual({ min: { x: 200, y: 100 }, max: { x: 220, y: 110 } });
    expect(rect(nb)).toEqual({ min: { x: 240, y: 130 }, max: { x: 250, y: 160 } });
    expect(s.selectedIds).toEqual(new Set(ids([na, nb])));
    expect(s.markups).toHaveLength(4);
  });

  it("is ONE undo step, and the copies are new redline-owned markups", async () => {
    const calls: Markup[] = [];
    const io: MarkupIpc = { add: async (_d, m) => { calls.push(m); }, update: async () => {}, remove: async () => {} };
    const src = { ...mk("a"), source_nm: "REVU000000000001", foreign_keys: [{ k: 1 }] } as unknown as Markup;
    const s = storeWith([src], "d1", io);
    select(s, "a");
    copySelection(ctxFor(s));
    pasteClipboard(ctxFor(s, { cursor: { x: 5, y: 5 } }), "cursor");
    await s.flush();
    expect(calls).toHaveLength(1);
    expect(calls[0].id).not.toBe("a");
    expect((calls[0] as unknown as Record<string, unknown>).source_nm).toBeUndefined();
    expect(calls[0].foreign_keys).toEqual([]);
    expect(calls[0].audit.created_by).toEqual(BY);
    s.undo();
    expect(s.markups).toHaveLength(1);
    expect(s.canUndo).toBe(false);
  });

  it("the clipboard is a snapshot: editing the source afterwards does not change a later paste", () => {
    const s = storeWith([mk("a", { rect: [0, 0, 10, 10], subject: "before" })]);
    select(s, "a");
    copySelection(ctxFor(s));
    s.update(s.markups[0], { ...s.markups[0], subject: "after" });
    pasteClipboard(ctxFor(s), "inplace");
    expect(s.markups[1].subject).toBe("before");
  });

  it("paste in place keeps the coordinates; onto another page too", () => {
    const s = storeWith([mk("a", { rect: [10, 10, 30, 20] })]);
    select(s, "a");
    copySelection(ctxFor(s));
    pasteClipboard(ctxFor(s, { pageIndex: 3 }), "inplace");
    expect(s.markups[1].page).toBe(3);
    expect(rect(s.markups[1])).toEqual(rect(s.markups[0]));
  });

  it("with no cursor, repeated pastes over the source cascade so a copy never hides exactly under it", () => {
    const s = storeWith([mk("a", { rect: [10, 10, 30, 20] })]);
    select(s, "a");
    copySelection(ctxFor(s));
    pasteClipboard(ctxFor(s), "cursor");
    pasteClipboard(ctxFor(s), "cursor");
    expect(rect(s.markups[1]).min).toEqual({ x: 22, y: -2 });
    expect(rect(s.markups[2]).min).toEqual({ x: 34, y: -14 });
  });

  it("the Paste button centres the clipboard in the view", () => {
    const s = storeWith([mk("a", { rect: [0, 0, 20, 10] })]);
    select(s, "a");
    copySelection(ctxFor(s));
    pasteClipboard(ctxFor(s, { viewCenter: { x: 300, y: 200 } }), "center");
    expect(rect(s.markups[1])).toEqual({ min: { x: 290, y: 195 }, max: { x: 310, y: 205 } });
  });

  it("a copied group pastes as a new group; the original is untouched", () => {
    const s = storeWith([mk("a", { group_id: "G" }), mk("b", { group_id: "G" })]);
    select(s, "a", "b");
    copySelection(ctxFor(s));
    pasteClipboard(ctxFor(s), "inplace");
    const [, , na, nb] = s.markups;
    expect(na.group_id).toBeTruthy();
    expect(na.group_id).toBe(nb.group_id);
    expect(na.group_id).not.toBe("G");
    expect(s.markups[0].group_id).toBe("G");
  });

  it("an unusable page or a missing identity is refused with nothing created", () => {
    const s = storeWith([mk("a")]);
    select(s, "a");
    copySelection(ctxFor(s));
    expect(pasteClipboard(ctxFor(s, { identity: null }), "inplace").ok).toBe(false);
    expect(pasteClipboard(ctxFor(s, { pageIndex: 99 }), "inplace").ok).toBe(false);
    expect(s.markups).toHaveLength(1);
  });
});

describe("cut", () => {
  it("copies then deletes in one undo step; undo brings the markups back", () => {
    const s = storeWith([mk("a"), mk("b")]);
    select(s, "a");
    expect(cutSelection(ctxFor(s))).toEqual({ ok: true, message: "Cut 1 markup" });
    expect(ids(s.markups)).toEqual(["b"]);
    expect(markupClipboard.size).toBe(1);
    s.undo();
    expect(ids(s.markups).sort()).toEqual(["a", "b"]);
  });
  it("locked markups are neither cut nor copied by Cut", () => {
    const s = storeWith([mk("a"), mk("L", { annot_flags: 4 | 0x80 })]);
    select(s, "a", "L");
    expect(cutSelection(ctxFor(s)).message).toBe("Cut 1 markup; 1 locked left in place");
    expect(ids(s.markups)).toEqual(["L"]);
    expect(ids(markupClipboard.payload!.items)).toEqual(["a"]);
  });
  it("refuses when everything selected is locked", () => {
    const s = storeWith([mk("L", { annot_flags: 4 | 0x80 })]);
    select(s, "L");
    expect(cutSelection(ctxFor(s)).ok).toBe(false);
    expect(s.markups).toHaveLength(1);
  });
  it("Copy still copies a locked markup, and the copy is unlocked", () => {
    const s = storeWith([mk("L", { annot_flags: 4 | 0x80 })]);
    select(s, "L");
    copySelection(ctxFor(s));
    pasteClipboard(ctxFor(s), "inplace");
    expect(s.markups[1].annot_flags).toBe(4);
  });
});

describe("paste across documents", () => {
  it("brings the count set along and recomputes measurements under the TARGET page's scale", () => {
    const src = storeWith([
      mkCount("k1", 5, 5, "G"),
      mk("len", {
        markup_type: "MeasurementLength", geometry: { Polyline: [{ x: 0, y: 0 }, { x: 100, y: 0 }] },
        measurement: { scale_ref: "s0", raw_measure: 100, unit: "m", computed_quantity: 10, depth: null, count_value: null, custom_columns: {} },
      }),
    ], "docA");
    select(src, "k1", "len");
    copySelection(ctxFor(src));

    const dst = storeWith([], "docB");
    const before = dst.countSets.length;
    pasteClipboard(ctxFor(dst, { scales: [scale("t", 2, 0)] }), "inplace");
    expect(dst.markups).toHaveLength(2);
    expect(dst.countSets.map((x) => x.id)).toContain(SET_A.id);
    expect(dst.countSets.length).toBe(before + 1);
    const len = dst.markups.find((m) => m.markup_type === "MeasurementLength")!;
    expect(len.measurement).toMatchObject({ scale_ref: "t", computed_quantity: 200, raw_measure: 100 });
    expect(src.markups.find((m) => m.id === "len")!.measurement?.computed_quantity).toBe(10);
  });
});

describe("count marks copied into a count: totals stay right (#138 model)", () => {
  it("a pasted counted room is a NEW count that adds to the total; the old count is unchanged", () => {
    const s = storeWith([mkCount("1", 0, 0, "G"), mkCount("2", 10, 0, "G"), mkCount("3", 20, 0, "G")]);
    select(s, "1", "2", "3");
    copySelection(ctxFor(s));
    pasteClipboard(ctxFor(s, { cursor: { x: 100, y: 100 } }), "cursor");
    const groups = countGroups(s.markups);
    expect(groups.map((g) => g.total)).toEqual([3, 3]);
    expect(groups[0].key).toBe("G");
    expect(groups[1].key).not.toBe("G");
    expect(countSubtotals(s.markups)[0].count).toBe(6);
  });
  it("pasting twice makes two counts", () => {
    const s = storeWith([mkCount("1", 0, 0, "G")]);
    select(s, "1");
    copySelection(ctxFor(s));
    pasteClipboard(ctxFor(s), "inplace");
    pasteClipboard(ctxFor(s), "inplace");
    expect(countGroups(s.markups).map((g) => g.total)).toEqual([1, 1, 1]);
  });
});

describe("paste to pages", () => {
  it("one undo step onto every listed page, in place", () => {
    const s = storeWith([mk("a", { rect: [1, 2, 3, 4] })]);
    select(s, "a");
    copySelection(ctxFor(s));
    const res = pasteToPages(ctxFor(s), [2, 4, 4, 5]);
    expect(res.message).toBe("Pasted 3 markups onto 3 pages");
    expect(s.markups.slice(1).map((m) => m.page)).toEqual([2, 4, 5]);
    s.undo();
    expect(s.markups).toHaveLength(1);
  });
  it("refuses a page outside the document and an over-limit paste", () => {
    const s = storeWith([mk("a")]);
    select(s, "a");
    copySelection(ctxFor(s));
    expect(pasteToPages(ctxFor(s, { pageCount: 3 }), [3]).ok).toBe(false);
    expect(s.markups).toHaveLength(1);
    const many = Array.from({ length: 10_001 }, (_, i) => i);
    expect(pasteToPages(ctxFor(s, { pageCount: 20_000 }), many).ok).toBe(false);
    expect(s.markups).toHaveLength(1);
  });
  it("pagesFromRanges: 1-based ranges, clamped to the document, de-duplicated", () => {
    expect(pagesFromRanges([[1, 3], [2, 5], [9, 99]], 6)).toEqual([0, 1, 2, 3, 4]);
    expect(pagesFromRanges([[1, 999_999_999]], 4)).toEqual([0, 1, 2, 3]);
  });
});

describe("duplicate", () => {
  it("copies 12 pt right and down on the same page and leaves the clipboard alone", () => {
    const s = storeWith([mk("a", { rect: [10, 50, 20, 60], page: 2 })]);
    select(s, "a");
    markupClipboard.set([mk("keep")], "x");
    expect(duplicateSelection(ctxFor(s)).message).toBe("Duplicated 1 markup");
    expect(s.markups[1].page).toBe(2);
    expect(rect(s.markups[1]).min).toEqual({ x: 22, y: 38 });
    expect(markupClipboard.payload!.items[0].id).toBe("keep");
    expect(s.selectedIds).toEqual(new Set([s.markups[1].id]));
  });
});

describe("multiply", () => {
  it("right x 3 makes three independent copies, one undo step, selection moves to the copies", () => {
    const s = storeWith([mk("a", { rect: [10, 10, 30, 30] })]);
    select(s, "a");
    const res = multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 3, pitch: 50 });
    expect(res.message).toBe("Made 3 markups");
    expect(s.markups.slice(1).map((m) => rect(m).min.x)).toEqual([60, 110, 160]);
    expect(s.selectedIds.size).toBe(3);
    s.undo();
    expect(s.markups).toHaveLength(1);
  });

  it("default is centre-to-centre: markups of different widths land the same pitch apart, centre to centre", () => {
    for (const w of [10, 80]) {
      const s = storeWith([mk("a", { rect: [100, 10, 100 + w, 30] })]);
      select(s, "a");
      multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 2, pitch: 120 });
      const centres = s.markups.map((m) => (rect(m).min.x + rect(m).max.x) / 2);
      expect(centres.map((c, i) => c - centres[0] - 120 * i)).toEqual([0, 0, 0]);
    }
  });

  it("gap mode: the clear space between neighbours is the distance, whatever the widths, one undo step", () => {
    for (const w of [10, 80]) {
      const s = storeWith([mk("a", { rect: [100, 10, 100 + w, 30] })]);
      select(s, "a");
      const res = multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 3, pitch: 25, measure: "gap" });
      expect(res.message).toBe("Made 3 markups");
      for (let i = 1; i < s.markups.length; i++) {
        expect(rect(s.markups[i]).min.x - rect(s.markups[i - 1]).max.x).toBe(25);
      }
      s.undo();
      expect(s.markups).toHaveLength(1);
      expect(s.canUndo).toBe(false);
    }
  });

  it("gap mode on a multi-markup selection uses the selection's bounding box", () => {
    const s = storeWith([mk("a", { rect: [0, 100, 10, 110] }), mk("b", { rect: [30, 100, 40, 110] })]);
    select(s, "a", "b");
    multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 1, pitch: 5, measure: "gap" });
    expect(s.markups.slice(2).map((m) => rect(m).min.x)).toEqual([45, 75]);
  });

  it("gap mode keeps count_group stamping and the locked-source refusal", () => {
    const s = storeWith([mkCount("solo", 20, 20, null)]);
    select(s, "solo");
    multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 2, pitch: 10, measure: "gap" });
    expect(countGroups(s.markups)[0].total).toBe(3);
    expect(s.getById("solo")!.count_group).toBe("solo");
    const locked = storeWith([mkCount("lk", 20, 20, null, { annot_flags: 4 | 0x80 })]);
    select(locked, "lk");
    const res = multiplySelection(ctxFor(locked), { kind: "line", direction: "right", count: 2, pitch: 10, measure: "gap" });
    expect(res.ok).toBe(false);
    expect(res.message).toMatch(/locked/);
    expect(locked.markups).toHaveLength(1);
  });

  it("copies of a group are separate groups (Revu: independent, not grouped together)", () => {
    const s = storeWith([mk("a", { group_id: "G", rect: [0, 100, 10, 110] }), mk("b", { group_id: "G", rect: [20, 100, 30, 110] })]);
    select(s, "a", "b");
    multiplySelection(ctxFor(s), { kind: "line", direction: "down", count: 2, pitch: 5 });
    const g = s.markups.slice(2).map((m) => m.group_id);
    expect(new Set(g).size).toBe(2);
    expect(g[0]).toBe(g[1]);
    expect(g[2]).toBe(g[3]);
    expect(g[0]).not.toBe(g[2]);
    expect(g.includes("G")).toBe(false);
  });

  it("copies of a count item join the source's count: one row of luminaires, right total", () => {
    const s = storeWith([mkCount("k", 20, 20, "G"), mkCount("k2", 400, 380, "G")]);
    select(s, "k");
    multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 4, pitch: 30 });
    const g = countGroups(s.markups);
    expect(g).toHaveLength(1);
    expect(g[0].total).toBe(6);
  });
  it("an ungrouped count multiplies into one count that includes the source, and the source is stamped (one undo step)", () => {
    const s = storeWith([mkCount("solo", 20, 20, null)]);
    select(s, "solo");
    multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 3, pitch: 30 });
    const g = countGroups(s.markups);
    expect(g).toHaveLength(1);
    expect(g[0].total).toBe(4);
    expect(s.getById("solo")!.count_group).toBe("solo");
    expect(s.getById("solo")!.audit.revision).toBe(8);
    s.undo();
    expect(s.markups).toHaveLength(1);
    expect(s.getById("solo")!.count_group).toBeNull();
    expect(s.canUndo).toBe(false);
  });

  it("frontend and Rust agree on the count (Rust: count_group None = a count of one, Some = the group)", () => {
    // Rust takeoff::count::collapse_groups / group_totals, stated here: rows keyed by count_group, and
    // an item with no count_group is its own row.
    const rustRows = (ms: Markup[]) => {
      const rows = new Map<string, number>();
      for (const m of ms.filter((x) => x.markup_type === "MeasurementCount")) {
        const k = m.count_group ? `g:${m.count_group}` : `solo:${m.id}`;
        rows.set(k, (rows.get(k) ?? 0) + (m.measurement?.count_value ?? 1));
      }
      return [...rows.values()];
    };
    for (const group of [null, "G"] as const) {
      const s = storeWith([mkCount("k", 20, 20, group)]);
      select(s, "k");
      multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 3, pitch: 30 });
      expect(rustRows(s.markups)).toEqual([4]);
      expect(countGroups(s.markups).map((x) => x.total)).toEqual([4]);
    }
  });

  it("a locked ungrouped count cannot be extended: the Multiply is refused whole", () => {
    const s = storeWith([mkCount("solo", 20, 20, null, { annot_flags: 4 | 0x80 })]);
    select(s, "solo");
    const res = multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 2, pitch: 30 });
    expect(res.ok).toBe(false);
    expect(res.message).toMatch(/locked/);
    expect(s.markups).toHaveLength(1);
    expect(s.getById("solo")!.count_group).toBeNull();
  });

  it("a locked count that already has a group multiplies fine (nothing to stamp)", () => {
    const s = storeWith([mkCount("k", 20, 20, "G", { annot_flags: 4 | 0x80 })]);
    select(s, "k");
    expect(multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 2, pitch: 30 }).ok).toBe(true);
    expect(countGroups(s.markups).map((x) => x.total)).toEqual([3]);
  });

  it("a count Legend row follows: the legend shows the new total", () => {
    const legend = legendFromCountSets([SET_A], { page: 0, topLeft: { x: 300, y: 380 }, identity: BY, now: NOW, id: "leg" });
    const s = storeWith([legend, mkCount("k", 20, 20, "G")]);
    select(s, "k");
    const qty = () => computeRows(0, sanitizeLegend(s.markups[0].legend), s.markups)[0].quantity;
    expect(qty()).toBe(1);
    multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 4, pitch: 30 });
    expect(qty()).toBe(5);
  });

  it("copies that would leave the page are not made, and the message says so", () => {
    const s = storeWith([mk("a", { rect: [10, 10, 30, 30] })]);
    select(s, "a");
    const res = multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 100, pitch: 100 });
    expect(res.ok).toBe(true);
    expect(s.markups).toHaveLength(1 + 5);
    expect(res.message).toBe("Made 5 markups; 95 more would not fit on the page");
  });

  it("refuses: no copy fits, bad numbers, mixed pages, an empty selection", () => {
    const s = storeWith([mk("a", { rect: [10, 10, 30, 30] }), mk("b", { page: 1 })]);
    select(s, "a");
    expect(multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 2, pitch: 5000 }).ok).toBe(false);
    expect(multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 0, pitch: 5 }).ok).toBe(false);
    select(s, "a", "b");
    expect(multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 1, pitch: 5 }).message).toMatch(/one page/);
    select(s);
    expect(multiplySelection(ctxFor(s), { kind: "line", direction: "right", count: 1, pitch: 5 }).ok).toBe(false);
    expect(s.markups).toHaveLength(2);
  });

  it("selection size x copies over the cap is refused whole", () => {
    const many = Array.from({ length: 200 }, (_, i) => mk(`m${i}`, { rect: [0, 0, 1, 1] }));
    const s = storeWith(many);
    select(s, ...ids(many));
    const res = multiplySelection(ctxFor(s, { pageWidthPts: 1e9, pageHeightPts: 1e9 }), { kind: "line", direction: "right", count: 100, pitch: 2 });
    expect(res.ok).toBe(false);
    expect(s.markups).toHaveLength(200);
  });

  it("a measurement multiplied on the same page keeps its quantity", () => {
    const len = mk("len", {
      markup_type: "MeasurementLength", geometry: { Polyline: [{ x: 0, y: 0 }, { x: 100, y: 0 }] },
      measurement: { scale_ref: "s0", raw_measure: 100, unit: "m", computed_quantity: 10, depth: null, count_value: null, custom_columns: {} },
    });
    const s = storeWith([len]);
    select(s, "len");
    multiplySelection(ctxFor(s, { scales: [scale("s0", 0.1, 0)] }), { kind: "line", direction: "up", count: 2, pitch: 20 });
    expect(s.markups.slice(1).map((m) => m.measurement?.computed_quantity)).toEqual([10, 10]);
  });

  it("pageScale reports the scale that applies to the viewed page", () => {
    expect(pageScale({ scales: [scale("p1", 0.5, 1), scale("d", 0.1)], pageIndex: 1 })?.id).toBe("p1");
    expect(pageScale({ scales: [scale("p1", 0.5, 1), scale("d", 0.1)], pageIndex: 0 })?.id).toBe("d");
    expect(pageScale({ scales: [], pageIndex: 0 })).toBeNull();
  });
});

describe("align / distribute / order on the store", () => {
  it("align moves everything to the FIRST selected markup, as one undo step with the audit advanced", () => {
    const s = storeWith([mk("a", { rect: [10, 0, 20, 10] }), mk("b", { rect: [40, 0, 50, 10] }), mk("c", { rect: [70, 0, 80, 10] })]);
    select(s, "b", "a", "c");
    expect(alignSelection(ctxFor(s), "left").message).toBe("Aligned 2 items");
    expect(rect(s.getById("a")!).min.x).toBe(40);
    expect(rect(s.getById("c")!).min.x).toBe(40);
    expect(rect(s.getById("b")!).min.x).toBe(40);
    expect(s.getById("a")!.audit).toMatchObject({ modified_by: BY, revision: 8 });
    expect(s.getById("b")!.audit.revision).toBe(7);
    s.undo();
    expect(rect(s.getById("a")!).min.x).toBe(10);
    expect(rect(s.getById("c")!).min.x).toBe(70);
  });
  it("refuses a single markup; a locked target is left in place and said so", () => {
    const s = storeWith([mk("a", { rect: [10, 0, 20, 10] }), mk("L", { rect: [40, 0, 50, 10], annot_flags: 4 | 0x80 }), mk("c", { rect: [70, 0, 80, 10] })]);
    select(s, "a");
    expect(alignSelection(ctxFor(s), "left").ok).toBe(false);
    select(s, "a", "L", "c");
    expect(alignSelection(ctxFor(s), "left").message).toBe("Aligned 1 item; 1 locked left in place");
    expect(rect(s.getById("L")!).min.x).toBe(40);
    expect(rect(s.getById("c")!).min.x).toBe(10);
  });
  it("distribute needs three and spaces the middle ones", () => {
    const s = storeWith([mk("a", { rect: [0, 0, 10, 10] }), mk("b", { rect: [12, 0, 22, 10] }), mk("c", { rect: [100, 0, 110, 10] })]);
    select(s, "a", "b");
    expect(distributeSelection(ctxFor(s), "horizontal").ok).toBe(false);
    select(s, "a", "b", "c");
    expect(distributeSelection(ctxFor(s), "horizontal").ok).toBe(true);
    expect(rect(s.getById("b")!).min.x).toBeCloseTo(50, 9);
    expect(distributeSelection(ctxFor(s), "horizontal").message).toBe("Already evenly spaced");
  });
  it("order: bring to front is one undo step; markups on other pages keep their slots", () => {
    const s = storeWith([mk("a"), mk("x", { page: 1 }), mk("b"), mk("c")]);
    select(s, "a");
    expect(orderSelection(ctxFor(s), "front").message).toBe("Re-ordered");
    expect(ids(s.markups)).toEqual(["b", "x", "c", "a"]);
    expect(orderSelection(ctxFor(s), "front").message).toBe("Already there");
    s.undo();
    expect(ids(s.markups)).toEqual(["a", "x", "b", "c"]);
  });
  it("order leaves a locked markup in place", () => {
    const s = storeWith([mk("L", { annot_flags: 4 | 0x80 }), mk("b")]);
    select(s, "L");
    expect(orderSelection(ctxFor(s), "front").ok).toBe(false);
    expect(ids(s.markups)).toEqual(["L", "b"]);
  });
});
