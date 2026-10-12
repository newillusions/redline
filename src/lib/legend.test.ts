import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { CountSet, Markup, UserRef } from "./ipc";
import {
  MAX_ENTRIES, MAX_FONT_PT, MAX_PAGES_CELL_CHARS, MAX_TITLE_CHARS, MIN_FONT_PT,
  addMarkupsToLegend, cellText, clipCell, computeRows, defaultLegendProps, effectiveRect,
  entriesFromMarkups, fitLegendGeometry, fixed2, formatPageSpec, keyOf, layout, legendFromCountSets,
  legendFromSelection, newLegend, pagesText, parsePageSpec, patchLegend, patchRow, removeRow,
  sanitizeLegend, type LegendEntry, type LegendProps,
} from "./legend";
import { mk } from "./markups-list.test-util";
import { buildRows, measurementText } from "./markups-list";
import { countSubtotalsByPage } from "./measurement-tools";
import { countGroups } from "./count-groups";
import { normaliseMeasurements, recomputeAll } from "./polylength";

const by: UserRef = { user_id: "u", display_name: "T" };
const NOW = "2026-10-12T00:00:00Z";

const fixture = JSON.parse(
  readFileSync(fileURLToPath(new URL("../../src-tauri/tests/fixtures/legend-agreement.json", import.meta.url)), "utf8"),
);

type Item = {
  type: Markup["markup_type"]; page: number; color: string; fill?: string; set?: string;
  count_value?: number; computed?: number; unit?: string; subject?: string; contents?: string;
  /** Object number of the layer the mark-up is on (its /OC reference). */
  oc?: number;
};

function sets(): Record<string, CountSet> {
  return fixture.sets as Record<string, CountSet>;
}

function fromItem(it: Item, i: number): Markup {
  const meas = it.type === "MeasurementCount" || it.computed !== undefined
    ? {
        scale_ref: null, raw_measure: 0, unit: it.unit ?? "ea", computed_quantity: it.computed ?? 0, depth: null,
        count_value: it.count_value ?? null, custom_columns: {}, rise_drop: [],
      }
    : null;
  return mk(`f${i}`, {
    markup_type: it.type,
    page: it.page,
    appearance: { color: it.color, line_weight: 2, opacity: 1, fill: it.fill ?? null, line_style: "Solid", font: null },
    subject: it.subject ?? null,
    contents: it.contents ?? null,
    count_set: it.set ? sets()[it.set] : null,
    measurement: meas,
    optional_content: it.oc ? { Reference: [it.oc, 0] } : null,
  } as Partial<Markup>);
}

function fixtureProps(raw: Record<string, unknown>): LegendProps {
  const copy = JSON.parse(JSON.stringify(raw));
  for (const e of copy.entries ?? []) {
    if (e.key.count_set) e.key.count_set = sets()[e.key.count_set].id;
  }
  return sanitizeLegend(copy);
}

describe("shared fixture (the same file the Rust engine runs)", () => {
  const markups = (fixture.items as Item[]).map(fromItem);

  for (const c of fixture.cases) {
    it(`rows: ${c.name}`, () => {
      const rows = computeRows(c.legend_page, fixtureProps(c.props), markups);
      expect(rows).toHaveLength(c.rows.length);
      rows.forEach((r, i) => {
        const w = c.rows[i];
        expect(r.entry).toBe(w.entry);
        expect(r.subject).toBe(w.subject);
        expect(r.description).toBe(w.description);
        expect(r.quantity_text).toBe(w.quantity_text);
        expect(r.unit).toBe(w.unit);
        expect(r.pages).toBe(w.pages);
        expect(r.matched).toBe(w.matched);
        if (w.color !== undefined) expect(r.color).toBe(w.color);
      });
    });
  }

  it("layouts", () => {
    for (const l of fixture.layouts) {
      const got = layout(fixtureProps(l.props), l.width, l.rows);
      expect(got.width).toBeCloseTo(l.width_out, 9);
      expect(got.height).toBeCloseTo(l.height, 6);
      expect(got.columns).toHaveLength(l.columns.length);
      got.columns.forEach((c, i) => {
        expect(c.column).toBe(l.columns[i][0]);
        expect(c.x).toBeCloseTo(l.columns[i][1], 6);
        expect(c.w).toBeCloseTo(l.columns[i][2], 6);
      });
    }
  });

  it("clipping", () => {
    for (const c of fixture.clips) expect(clipCell(c.text, c.width, c.font)).toBe(c.out);
  });

  it("rounding", () => {
    for (const [q, text] of fixture.fixed2) expect(fixed2(q)).toBe(text);
  });
});

function countItem(id: string, set: CountSet, n: number, page = 0, extra: Partial<Markup> = {}): Markup {
  return mk(id, {
    markup_type: "MeasurementCount",
    page,
    appearance: { color: set.color, line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
    count_set: set,
    measurement: { scale_ref: null, raw_measure: n, unit: "ea", computed_quantity: n, depth: null, count_value: n, custom_columns: {}, rise_drop: [] },
    ...extra,
  } as Partial<Markup>);
}

function setOf(name: string, color = "#ff0000"): CountSet {
  return { id: crypto.randomUUID(), name, color, symbol: "Square" };
}

function entryOf(m: Markup): LegendEntry {
  return { key: keyOf(m), label: null, description: null, hidden: false, symbol: null };
}

function legendProps(entries: LegendEntry[], over: Partial<LegendProps> = {}): LegendProps {
  return sanitizeLegend({ ...defaultLegendProps(), scope: "AllPages", entries, ...over });
}

describe("auto-update", () => {
  it("rows follow add, edit and delete of the source mark-ups", () => {
    const s = setOf("A");
    let ms = [countItem("c1", s, 1)];
    const p = legendProps([entryOf(ms[0])]);
    expect(computeRows(0, p, ms)[0].quantity_text).toBe("1");
    ms = [...ms, countItem("c2", s, 1)];
    expect(computeRows(0, p, ms)[0].quantity_text).toBe("2");
    ms = ms.map((m) => (m.id === "c2" ? { ...m, measurement: { ...m.measurement!, count_value: 5 } } : m));
    expect(computeRows(0, p, ms)[0].quantity_text).toBe("6");
    ms = ms.filter((m) => m.id !== "c1");
    expect(computeRows(0, p, ms)[0].quantity_text).toBe("5");
    const none = computeRows(0, p, []);
    expect(none[0].quantity_text).toBe("0");
    expect(none[0].matched).toBe(0);
  });

  it("scope: this page, all pages, custom range; the legend is never counted", () => {
    const s = setOf("A");
    const ms = [countItem("a", s, 1, 0), countItem("b", s, 2, 1), countItem("c", s, 4, 2)];
    const e = [entryOf(ms[0])];
    expect(computeRows(0, legendProps(e, { scope: "CurrentPage" }), ms)[0].quantity_text).toBe("1");
    expect(computeRows(1, legendProps(e, { scope: "CurrentPage" }), ms)[0].quantity_text).toBe("2");
    expect(computeRows(0, legendProps(e, { scope: "AllPages" }), ms)[0].quantity_text).toBe("7");
    expect(computeRows(0, legendProps(e, { scope: { Pages: [[2, 3]] } }), ms)[0].quantity_text).toBe("6");
    const legend = newLegend(legendProps(e), { page: 0, topLeft: { x: 0, y: 100 }, identity: by, now: NOW, id: "L" });
    expect(computeRows(0, legendProps(e), [...ms, legend])[0].quantity_text).toBe("7");
  });

  it("hidden and hide-empty rows are dropped; automatic count sets follow the explicit entries", () => {
    const a = setOf("A");
    const b = setOf("B", "#00ff00");
    const ms = [countItem("b1", b, 2), countItem("a1", a, 1), countItem("b2", b, 1)];
    const p = legendProps([entryOf(ms[1]), { ...entryOf(countItem("z", setOf("Z"), 1)) }], { auto_count_sets: true, hide_empty: true });
    const rows = computeRows(0, p, ms);
    expect(rows.map((r) => r.subject)).toEqual(["A", "B"]);
    expect(rows[1].entry).toBeNull();
    expect(rows[1].quantity_text).toBe("3");
    const hidden = legendProps([{ ...entryOf(ms[1]), hidden: true }]);
    expect(computeRows(0, hidden, ms)).toHaveLength(0);
  });

  it("follows the mark-up text, an override wins, an edit of the mark-up shows through", () => {
    const s = setOf("Set name");
    const c = countItem("c", s, 1, 0, { subject: "From markup", contents: " note " });
    const p = legendProps([entryOf(c)]);
    expect(computeRows(0, p, [c])[0]).toMatchObject({ subject: "From markup", description: "note" });
    expect(computeRows(0, p, [{ ...c, subject: "Renamed in the list" }])[0].subject).toBe("Renamed in the list");
    const over = legendProps([{ ...entryOf(c), label: "Own", description: "Own text" }]);
    expect(computeRows(0, over, [c])[0]).toMatchObject({ subject: "Own", description: "Own text" });
  });
});

describe("the numbers equal the other quantity surfaces", () => {
  it("counts: Quantities panel set subtotal, group totals and the Markups List row totals", () => {
    const a = setOf("A");
    const g1 = "00000000-0000-0000-0000-0000000000a1";
    const g2 = "00000000-0000-0000-0000-0000000000a2";
    const ms = [
      countItem("1", a, 1, 0, { count_group: g1 }),
      countItem("2", a, 1, 0, { count_group: g1 }),
      countItem("3", a, 1, 0, { count_group: g2 }),
      countItem("4", a, 1, 2),
    ];
    const rows = computeRows(0, legendProps([entryOf(ms[0])]), ms);
    const panel = countSubtotalsByPage(ms).find((s) => s.setId === a.id)!;
    expect(rows[0].quantity).toBe(panel.count);
    const grouped = countGroups(ms).reduce((n, g) => n + g.total, 0);
    expect(rows[0].quantity).toBe(grouped);
    const listTotal = buildRows(ms).reduce((n, r) => n + (r.measurementValue ?? 0), 0);
    expect(rows[0].quantity).toBe(listTotal);
    expect(rows[0].quantity_text).toBe("4");
  });

  it("a count with no recorded value counts once on every surface", () => {
    const a = setOf("A");
    const blank = (id: string, extra: Partial<Markup> = {}) =>
      countItem(id, a, 1, 0, {
        measurement: { scale_ref: null, raw_measure: 0, unit: "ea", computed_quantity: 0, depth: null, count_value: null, custom_columns: {}, rise_drop: [] },
        ...extra,
      });
    const ms = [blank("1"), blank("2"), blank("3", { count_group: "00000000-0000-0000-0000-0000000000b1" })];
    const rows = computeRows(0, legendProps([entryOf(ms[0])]), ms);
    expect(rows[0].quantity).toBe(3);
    expect(countSubtotalsByPage(ms)[0].count).toBe(3);
    expect(buildRows(ms).reduce((n, r) => n + (r.measurementValue ?? 0), 0)).toBe(3);
    expect(countGroups(ms).reduce((n, g) => n + g.total, 0)).toBe(3);
  });

  it("lengths: the Markups List measurement values, also after a recalibration and on reopen", () => {
    const len = (id: string, raw: number, page: number): Markup =>
      mk(id, {
        markup_type: "MeasurementLength",
        page,
        measurement: { scale_ref: "old", raw_measure: raw, unit: "m", computed_quantity: 1, depth: null, count_value: null, custom_columns: {}, rise_drop: [{ segment: 0, amount: 2, unit: "m" }] },
      } as Partial<Markup>);
    const ms = [len("a", 1000, 0), len("b", 500, 1)];
    const scale = { id: "recal", applies_to: { kind: "DocumentDefault" as const }, method: "TwoPoint" as const, ratio: 0.005, unit: "m", label: "r", precision: 2 };
    const p = legendProps([entryOf(ms[0])]);
    // The app recomputes on recalibration as one edit, the store keeps the result ...
    const after = ms.map((m) => recomputeAll(ms, [scale], by, NOW).find((x) => x.before.id === m.id)?.after ?? m);
    const rows = computeRows(0, p, after);
    const listSum = buildRows(after).reduce((n, r) => n + (r.measurementValue ?? 0), 0);
    expect(rows[0].quantity_text).toBe(fixed2(listSum));
    expect(rows[0].quantity_text).toBe("11.50"); // (1000 + 500) x 0.005 + 2 x 2
    // ... and on reopen the seed normalises to the same numbers.
    const reopened = normaliseMeasurements(ms, [scale]);
    expect(computeRows(0, p, reopened)[0].quantity_text).toBe("11.50");
    expect(buildRows(reopened).map((r) => measurementText(r.markup).text)).toEqual(["7.00 m", "4.50 m"]);
  });
});

describe("hostile input", () => {
  it("clamps a hostile definition", () => {
    const hostile = sanitizeLegend({
      title: "x".repeat(10_000) + "\u0000\n",
      font_size_pt: NaN,
      columns: [] as never,
      scope: { Pages: [[0, 3], [5, 2], [1.5, 2]] },
      entries: Array.from({ length: 2000 }, (_, i) => ({
        key: { markup_type: "Cloud" as const, color: "#" + i.toString(16).padStart(6, "0"), fill: "javascript" },
        label: "l".repeat(9999),
        description: "\u0007bell",
      })),
    });
    expect([...hostile.title]).toHaveLength(MAX_TITLE_CHARS);
    expect(hostile.font_size_pt).toBe(9);
    expect(hostile.columns).toEqual(defaultLegendProps().columns);
    expect(hostile.scope).toBe("CurrentPage");
    expect(hostile.entries).toHaveLength(MAX_ENTRIES);
    expect(hostile.entries.every((e) => e.key.fill === null)).toBe(true);
    expect(hostile.entries[0].description).toBe("bell");
    expect(sanitizeLegend({ font_size_pt: 1e9 }).font_size_pt).toBe(MAX_FONT_PT);
    expect(sanitizeLegend({ font_size_pt: -5 }).font_size_pt).toBe(MIN_FONT_PT);
    expect(sanitizeLegend({ entries: [{ key: { markup_type: "Legend", color: "#000000" } }] }).entries).toHaveLength(0);
    expect(sanitizeLegend(null as never)).toEqual(defaultLegendProps());
  });

  it("thousands of mark-ups against 256 entries stay inside a time budget", () => {
    const setsList = Array.from({ length: 256 }, (_, i) => setOf(`S${i}`));
    const ms = Array.from({ length: 6000 }, (_, i) => countItem(`m${i}`, setsList[i % 256], 1, i % 7));
    const p = legendProps(setsList.map((s) => entryOf(countItem("k", s, 1))));
    const t = performance.now();
    const rows = computeRows(0, p, ms);
    expect(performance.now() - t).toBeLessThan(2000);
    expect(rows).toHaveLength(256);
    expect(rows.reduce((n, r) => n + r.quantity, 0)).toBe(6000);
  });

  it("huge count values cannot produce NaN or negative totals; a NaN length is ignored", () => {
    const s = setOf("A");
    const a = countItem("a", s, 4_000_000_000);
    const b = countItem("b", s, 4_000_000_000);
    expect(computeRows(0, legendProps([entryOf(a)]), [a, b])[0].quantity_text).toBe("8000000000");
    const l = (q: number) => mk("l" + q, { markup_type: "MeasurementLength", measurement: { scale_ref: null, raw_measure: 1, unit: "m", computed_quantity: q, depth: null, count_value: null, custom_columns: {}, rise_drop: [] } } as Partial<Markup>);
    const ls = [l(2), l(NaN), l(Infinity)];
    expect(computeRows(0, legendProps([entryOf(ls[0])]), ls)[0].quantity_text).toBe("2.00");
  });

  it("pages cell and clipping stay bounded", () => {
    expect(pagesText([0, 1, 2, 4, 9])).toBe("1-3, 5, 10");
    const many = pagesText(Array.from({ length: 200 }, (_, i) => i * 2));
    expect([...many].length).toBeLessThanOrEqual(MAX_PAGES_CELL_CHARS);
    expect(many.endsWith("...")).toBe(true);
    expect(clipCell("x".repeat(500), 100, 9).endsWith("...")).toBe(true);
    expect(clipCell("abc", Number.NaN, 9)).toBe("");
  });
});

describe("page spec", () => {
  it("parses ranges and refuses garbage (never 'everything')", () => {
    expect(parsePageSpec("1-3, 5, 9")).toEqual([[1, 3], [5, 5], [9, 9]]);
    expect(parsePageSpec("3-1")).toBeNull();
    expect(parsePageSpec("0")).toBeNull();
    expect(parsePageSpec("a, b")).toBeNull();
    expect(parsePageSpec("")).toBeNull();
    expect(formatPageSpec({ Pages: [[1, 3], [5, 5]] })).toBe("1-3, 5");
    expect(formatPageSpec("AllPages")).toBe("");
  });
});

describe("creating and editing", () => {
  const ctx = { page: 0, topLeft: { x: 40, y: 700 }, identity: by, now: NOW, id: "L1" };

  it("from a selection: one row per distinct type/appearance, none for a lone legend", () => {
    const s = setOf("A");
    const sel = [countItem("1", s, 1), countItem("2", s, 1), mk("r", { markup_type: "Cloud" } as Partial<Markup>)];
    const l = legendFromSelection(sel, ctx)!;
    expect(l.markup_type).toBe("Legend");
    expect(l.legend!.entries).toHaveLength(2);
    expect(l.legend!.scope).toBe("AllPages");
    expect(legendFromSelection([], ctx)).toBeNull();
  });

  it("from count sets: a row per set at zero, new sets are picked up automatically", () => {
    const a = setOf("A");
    const l = legendFromCountSets([a], ctx);
    expect(l.legend!.auto_count_sets).toBe(true);
    expect(computeRows(0, l.legend!, [])).toHaveLength(1);
    const b = setOf("B", "#00ff00");
    const rows = computeRows(0, l.legend!, [countItem("b", b, 2)]);
    expect(rows.map((r) => r.subject)).toEqual(["A", "B"]);
  });

  it("adds markups to a legend without duplicating rows; patches are one audit bump", () => {
    const s = setOf("A");
    const l = legendFromCountSets([s], ctx);
    expect(addMarkupsToLegend(l, [countItem("1", s, 1)], by, NOW)).toBe(l);
    const cloud = mk("r", { markup_type: "Cloud" } as Partial<Markup>);
    const added = addMarkupsToLegend(l, [cloud], by, NOW);
    expect(added.legend!.entries).toHaveLength(2);
    expect(added.audit.revision).toBe(l.audit.revision + 1);
    const titled = patchLegend(added, { title: "  Key  ", font_size_pt: 1000 }, by, NOW);
    expect(titled.legend!.title).toBe("Key");
    expect(titled.legend!.font_size_pt).toBe(MAX_FONT_PT);
  });

  it("row edits make an automatic row explicit; remove drops an explicit row", () => {
    const a = setOf("A");
    const b = setOf("B", "#00ff00");
    const l = legendFromCountSets([a], ctx);
    const ms = [countItem("a", a, 1), countItem("b", b, 2)];
    const rows = computeRows(0, l.legend!, ms);
    const auto = rows[1];
    expect(auto.entry).toBeNull();
    const edited = patchRow(l, auto, { label: "Track", description: "On track", hidden: false }, by, NOW);
    expect(edited.legend!.entries).toHaveLength(2);
    expect(computeRows(0, edited.legend!, ms)[1]).toMatchObject({ entry: 1, subject: "Track", description: "On track" });
    const hidden = patchRow(edited, computeRows(0, edited.legend!, ms)[1], { label: "Track", description: "On track", hidden: true }, by, NOW);
    expect(computeRows(0, hidden.legend!, ms)).toHaveLength(1);
    const removed = removeRow(edited, computeRows(0, edited.legend!, ms)[0], by, NOW);
    expect(removed.legend!.entries).toHaveLength(1);
    expect(entriesFromMarkups([], edited.legend!.entries)).toEqual([]);
  });

  it("fits the stored rectangle to the rows: top-left kept, bottom follows", () => {
    const s = setOf("A");
    const l = legendFromCountSets([s], ctx);
    const one = effectiveRect(l, 1)!;
    const five = effectiveRect(l, 5)!;
    expect(one.y1).toBe(five.y1);
    expect(five.y0).toBeLessThan(one.y0);
    const fitted = fitLegendGeometry(l, [countItem("a", s, 1)]);
    expect(fitLegendGeometry(fitted, [countItem("a", s, 1)])).toBe(fitted); // stable: same object
    const grown = fitLegendGeometry(fitted, [countItem("a", s, 1), countItem("b", setOf("B"), 1)]);
    expect(grown).not.toBe(fitted);
    expect("Rect" in grown.geometry && grown.geometry.Rect.max.y).toBe(700);
    expect(cellText(computeRows(0, l.legend!, [])[0], "Quantity")).toBe("0");
  });
});
