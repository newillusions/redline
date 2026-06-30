import { describe, it, expect } from "vitest";
import {
  measureLength,
  measureArea,
  formatQuantity,
  countSubtotals,
  countSubtotalsByPage,
} from "./measurement-tools";
import type { CountSet, Markup, ScaleRecord } from "./ipc";

function countMarkup(id: string, count: number, set: CountSet | null): Markup {
  return {
    id,
    page: 0,
    markup_type: "MeasurementCount",
    geometry: { Point: { x: 0, y: 0 } },
    appearance: { color: set?.color ?? "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
    subject: null, layer: null, contents: null, group_id: null,
    audit: {
      created_by: { user_id: "u", display_name: "U" }, created_at: "",
      modified_by: { user_id: "u", display_name: "U" }, modified_at: "",
      revision: 0, origin: "Desktop",
    },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: { scale_ref: null, raw_measure: 1, unit: "ea", computed_quantity: 1, depth: null, count_value: count, custom_columns: {} },
    count_set: set,
  };
}

describe("countSubtotals", () => {
  const A: CountSet = { id: "set-a", name: "Type-A", color: "#0066ff", symbol: "Triangle" };
  const B: CountSet = { id: "set-b", name: "Type-B", color: "#00875a", symbol: "Square" };

  it("groups count markups by set and sums count_value per set", () => {
    const rows = countSubtotals([
      countMarkup("1", 1, A),
      countMarkup("2", 1, B),
      countMarkup("3", 1, A),
    ]);
    expect(rows).toHaveLength(2);
    const a = rows.find((r) => r.setId === "set-a")!;
    expect(a).toMatchObject({ name: "Type-A", color: "#0066ff", symbol: "Triangle", count: 2 });
    expect(rows.find((r) => r.setId === "set-b")!.count).toBe(1);
  });

  it("buckets count markups with no set under a single Unassigned group (setId null)", () => {
    const rows = countSubtotals([countMarkup("1", 1, null), countMarkup("2", 1, null)]);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ setId: null, name: "Unassigned", count: 2 });
  });

  it("ignores non-count measurement and non-measurement markups", () => {
    const rect = { ...countMarkup("r", 1, A), markup_type: "Rectangle" as const };
    expect(countSubtotals([rect])).toHaveLength(0);
  });
});

/** Helper: count markup with explicit page number. */
function countMarkupOnPage(id: string, count: number, set: CountSet | null, page: number): Markup {
  return { ...countMarkup(id, count, set), page };
}

describe("countSubtotalsByPage", () => {
  const A: CountSet = { id: "set-a", name: "Type-A", color: "#0066ff", symbol: "Triangle" };
  const B: CountSet = { id: "set-b", name: "Type-B", color: "#00875a", symbol: "Square" };

  it("groups by set and page, totals match sum of all pages", () => {
    const rows = countSubtotalsByPage([
      countMarkupOnPage("a1", 1, A, 0),
      countMarkupOnPage("a2", 1, A, 1),
      countMarkupOnPage("a3", 1, A, 0),
    ]);
    expect(rows).toHaveLength(1);
    const aRow = rows[0];
    expect(aRow.setId).toBe("set-a");
    expect(aRow.count).toBe(3);
    expect(aRow.byPage).toHaveLength(2);
    const page0 = aRow.byPage.find((bp) => bp.page === 0)!;
    const page1 = aRow.byPage.find((bp) => bp.page === 1)!;
    expect(page0.count).toBe(2);
    expect(page1.count).toBe(1);
  });

  it("each set gets its own per-page breakdown", () => {
    const rows = countSubtotalsByPage([
      countMarkupOnPage("a1", 2, A, 0),
      countMarkupOnPage("b1", 3, B, 0),
      countMarkupOnPage("b2", 1, B, 2),
    ]);
    expect(rows).toHaveLength(2);
    const bRow = rows.find((r) => r.setId === "set-b")!;
    expect(bRow.count).toBe(4);
    expect(bRow.byPage).toHaveLength(2);
    expect(bRow.byPage.find((bp) => bp.page === 2)!.count).toBe(1);
  });

  it("byPage entries are sorted ascending by page", () => {
    const rows = countSubtotalsByPage([
      countMarkupOnPage("a1", 1, A, 3),
      countMarkupOnPage("a2", 1, A, 1),
      countMarkupOnPage("a3", 1, A, 0),
    ]);
    const pages = rows[0].byPage.map((bp) => bp.page);
    expect(pages).toEqual([0, 1, 3]);
  });

  it("markers with no set go into an Unassigned group with page breakdown", () => {
    const rows = countSubtotalsByPage([
      countMarkupOnPage("u1", 1, null, 0),
      countMarkupOnPage("u2", 1, null, 2),
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0].setId).toBeNull();
    expect(rows[0].count).toBe(2);
    expect(rows[0].byPage).toHaveLength(2);
  });

  it("single page produces one byPage entry equal to set total", () => {
    const rows = countSubtotalsByPage([
      countMarkupOnPage("a1", 5, A, 0),
    ]);
    expect(rows[0].byPage).toHaveLength(1);
    expect(rows[0].byPage[0]).toEqual({ page: 0, count: 5 });
  });

  it("ignores non-MeasurementCount markups", () => {
    const rect = { ...countMarkupOnPage("r", 1, A, 0), markup_type: "Rectangle" as const };
    expect(countSubtotalsByPage([rect])).toHaveLength(0);
  });
});

function scale(ratio: number, unit = "m", precision = 2): ScaleRecord {
  return {
    id: "sc1",
    applies_to: { kind: "DocumentDefault" },
    method: "Preset",
    ratio,
    unit,
    label: "1:1000",
    precision,
  };
}

describe("measureLength", () => {
  it("returns 0 for a single point", () => {
    expect(measureLength([{ x: 0, y: 0 }])).toBe(0);
  });

  it("computes 3-4-5 hypotenuse", () => {
    expect(measureLength([{ x: 0, y: 0 }, { x: 3, y: 4 }])).toBeCloseTo(5, 9);
  });

  it("sums two segments", () => {
    const pts = [{ x: 0, y: 0 }, { x: 1, y: 0 }, { x: 1, y: 1 }];
    expect(measureLength(pts)).toBeCloseTo(2, 9);
  });
});

describe("measureArea", () => {
  it("returns 0 for fewer than 3 points", () => {
    expect(measureArea([{ x: 0, y: 0 }, { x: 1, y: 0 }])).toBe(0);
  });

  it("computes unit square area", () => {
    const pts = [{ x: 0, y: 0 }, { x: 1, y: 0 }, { x: 1, y: 1 }, { x: 0, y: 1 }];
    expect(measureArea(pts)).toBeCloseTo(1, 9);
  });

  it("computes right triangle area", () => {
    const pts = [{ x: 0, y: 0 }, { x: 6, y: 0 }, { x: 0, y: 4 }];
    expect(measureArea(pts)).toBeCloseTo(12, 9);
  });
});

describe("formatQuantity", () => {
  it("formats a length with the scale unit", () => {
    const s = scale(0.001, "m", 2);
    // raw 5000 pts × 0.001 = 5.00 m
    expect(formatQuantity(5000, s, false)).toBe("5.00 m");
  });

  it("formats an area (ratio²) with unit²", () => {
    const s = scale(0.001, "m", 2);
    // raw 1_000_000 pts² × 0.001² = 1.00 m²
    expect(formatQuantity(1_000_000, s, true)).toBe("1.00 m²");
  });

  it("respects precision 0", () => {
    const s = scale(1 / 100, "cm", 0);
    expect(formatQuantity(300, s, false)).toBe("3 cm");
  });
});
