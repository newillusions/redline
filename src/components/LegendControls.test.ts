// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import LegendControls from "./LegendControls.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import type { CountSet, Markup, UserRef } from "$lib/ipc";
import { computeRows, legendFromCountSets, legendFromSelection, sanitizeLegend } from "$lib/legend";
import { mk } from "$lib/markups-list.test-util";

vi.mock("$lib/ipc", () => ({
  isMarkupLocked: () => false,
  isMarkupContentsLocked: () => false,
  getPageSize: vi.fn(async () => ({ doc_id: "d", page_index: 0, width_pts: 1000, height_pts: 800 })),
}));

const by: UserRef = { user_id: "u", display_name: "T" };
const ipc = () => ({ add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) });
const A: CountSet = { id: "set-a", name: "Downlight", color: "#ff0000", symbol: "Square" };
const B: CountSet = { id: "set-b", name: "Track", color: "#0000ff", symbol: "Circle" };

function count(id: string, set: CountSet, n = 1, page = 0): Markup {
  return mk(id, {
    markup_type: "MeasurementCount",
    page,
    appearance: { color: set.color, line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
    count_set: set,
    measurement: { scale_ref: null, raw_measure: n, unit: "ea", computed_quantity: n, depth: null, count_value: n, custom_columns: {}, rise_drop: [] },
  } as Partial<Markup>);
}

function setup(markups: Markup[], selectedIds: string[], extra: Record<string, unknown> = {}) {
  const i = ipc();
  const store = new MarkupStore("d", i);
  store.seed(markups);
  store.selectedIds = new Set(selectedIds);
  const r = render(LegendControls, { props: { store, identity: by, ...extra } });
  return { store, ipc: i, ...r };
}

function legendOf(store: MarkupStore): Markup {
  return store.markups.find((m) => m.markup_type === "Legend")!;
}

describe("LegendControls: creating", () => {
  it("New legend from selection lists the distinct types, places it on the page and selects it (one undo)", async () => {
    const { store, container } = setup([count("1", A), count("2", A), count("3", B)], ["1", "2", "3"]);
    await fireEvent.click(container.querySelector("[data-action=legend-from-selection]")!);
    await vi.waitFor(() => expect(store.markups.filter((m) => m.markup_type === "Legend")).toHaveLength(1));
    const l = legendOf(store);
    expect(l.legend!.entries).toHaveLength(2);
    expect(store.selectedIds.has(l.id)).toBe(true);
    // Top-left of the 1000 x 800 page, 36 pt in.
    expect("Rect" in l.geometry && l.geometry.Rect.min.x).toBe(36);
    expect("Rect" in l.geometry && l.geometry.Rect.max.y).toBe(764);
    store.undo();
    expect(store.markups.some((m) => m.markup_type === "Legend")).toBe(false);
  });

  it("is offered nothing when the selection is empty", () => {
    const { container } = setup([count("1", A)], []);
    expect(container.querySelector("[data-action=legend-from-selection]")).toBeNull();
  });

  it("Add puts the selection's rows into an existing legend without duplicating", async () => {
    const legend = legendFromCountSets([A], { page: 0, topLeft: { x: 0, y: 500 }, identity: by, now: "t", id: "L" });
    const cloud = mk("r", { markup_type: "Cloud" } as Partial<Markup>);
    const { store, container } = setup([legend, count("1", A), cloud], ["1", "r"]);
    await fireEvent.click(container.querySelector("[data-action=legend-add]")!);
    expect(legendOf(store).legend!.entries).toHaveLength(2);
    await fireEvent.click(container.querySelector("[data-action=legend-add]")!);
    expect(legendOf(store).legend!.entries).toHaveLength(2);
    expect(container.textContent).toMatch(/Already listed/);
  });
});

describe("LegendControls: editing a selected legend", () => {
  function withLegend() {
    const legend = legendFromCountSets([A, B], { page: 0, topLeft: { x: 0, y: 500 }, identity: by, now: "t", id: "L" });
    return setup([legend, count("1", A, 2), count("2", B, 1)], ["L"], { currentPage: 3 });
  }

  it("shows one editor row per entry with the live quantity", () => {
    const { container } = withLegend();
    const rows = container.querySelectorAll("[data-testid=legend-row]");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("2 ea");
    expect(rows[1].textContent).toContain("1 ea");
  });

  it("title, headers, font size and columns are single undoable edits", async () => {
    const { store, container } = withLegend();
    await fireEvent.change(container.querySelector('input[aria-label="Legend title"]')!, { target: { value: "  Luminaires " } });
    expect(legendOf(store).legend!.title).toBe("Luminaires");
    await fireEvent.change(container.querySelector('input[aria-label="Legend font size"]')!, { target: { value: "500" } });
    expect(legendOf(store).legend!.font_size_pt).toBe(72);
    const pagesBox = [...container.querySelectorAll("fieldset.cols input")].find((i) => i.parentElement!.textContent!.includes("Pages")) as HTMLInputElement;
    await fireEvent.click(pagesBox);
    expect(legendOf(store).legend!.columns).toContain("Pages");
    // The last remaining column cannot be switched off.
    for (const c of ["Symbol", "Subject", "Quantity", "Unit"]) {
      const box = [...container.querySelectorAll("fieldset.cols input")].find((i) => i.parentElement!.textContent!.trim() === c) as HTMLInputElement;
      await fireEvent.click(box);
    }
    expect(legendOf(store).legend!.columns.length).toBeGreaterThanOrEqual(1);
    store.undo();
    store.undo();
    expect(legendOf(store).legend!.title).toBe("Luminaires");
  });

  it("source pages: this page / all pages / custom range, and an invalid range is refused", async () => {
    const { store, container } = withLegend();
    const sel = container.querySelector('select[aria-label="Source pages"]') as HTMLSelectElement;
    await fireEvent.change(sel, { target: { value: "CurrentPage" } });
    expect(legendOf(store).legend!.scope).toBe("CurrentPage");
    await fireEvent.change(sel, { target: { value: "Custom" } });
    expect(legendOf(store).legend!.scope).toEqual({ Pages: [[4, 4]] }); // the page being viewed (index 3)
    await tick();
    const box = container.querySelector('input[aria-label="Page range"]') as HTMLInputElement;
    await fireEvent.change(box, { target: { value: "1-3, 5" } });
    expect(legendOf(store).legend!.scope).toEqual({ Pages: [[1, 3], [5, 5]] });
    await fireEvent.change(box, { target: { value: "nonsense" } });
    expect(container.textContent).toMatch(/Enter pages like/);
    expect(legendOf(store).legend!.scope).toEqual({ Pages: [[1, 3], [5, 5]] });
  });

  it("row edits: subject and description overrides, hide and show, remove", async () => {
    const { store, container } = withLegend();
    await fireEvent.change(container.querySelector('input[aria-label="Subject for row 1"]')!, { target: { value: "Recessed" } });
    await fireEvent.change(container.querySelector('input[aria-label="Description for row 1"]')!, { target: { value: "IP44 downlight" } });
    let rows = computeRows(0, sanitizeLegend(legendOf(store).legend), store.markups);
    expect(rows[0]).toMatchObject({ subject: "Recessed", description: "IP44 downlight" });
    await fireEvent.click(container.querySelector('input[aria-label="Hide row 2"]')!);
    rows = computeRows(0, sanitizeLegend(legendOf(store).legend), store.markups);
    expect(rows).toHaveLength(1);
    // The hidden row stays in the editor so it can be shown again.
    expect(container.querySelectorAll("[data-testid=legend-row]")).toHaveLength(2);
    await fireEvent.click(container.querySelector('input[aria-label="Hide row 2"]')!);
    expect(computeRows(0, sanitizeLegend(legendOf(store).legend), store.markups)).toHaveLength(2);
    await fireEvent.click(container.querySelector('button[aria-label="Remove row 2"]')!);
    expect(legendOf(store).legend!.entries).toHaveLength(1);
  });

  it("an automatic count-set row becomes an explicit entry on its first edit", async () => {
    const legend = legendFromCountSets([A], { page: 0, topLeft: { x: 0, y: 500 }, identity: by, now: "t", id: "L" });
    const { store, container } = setup([legend, count("1", A), count("2", B, 3)], ["L"]);
    expect(container.querySelectorAll("[data-testid=legend-row]")).toHaveLength(2);
    expect(container.querySelector('button[aria-label="Remove row 2"]')).toBeNull(); // automatic rows have no remove
    await fireEvent.change(container.querySelector('input[aria-label="Subject for row 2"]')!, { target: { value: "Track light" } });
    expect(legendOf(store).legend!.entries).toHaveLength(2);
    expect(computeRows(0, sanitizeLegend(legendOf(store).legend), store.markups)[1].subject).toBe("Track light");
  });

  it("an entry measured in two units is one editor row listing both quantities, never one summed number", () => {
    const len = (id: string, q: number, unit: string, page: number): Markup =>
      mk(id, {
        markup_type: "MeasurementLength",
        page,
        appearance: { color: "#445566", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
        measurement: { scale_ref: null, raw_measure: 1, unit, computed_quantity: q, depth: null, count_value: null, custom_columns: {}, rise_drop: [] },
      } as Partial<Markup>);
    const ms = [len("a", 5, "m", 0), len("b", 7.5, "ft", 1)];
    const legend = legendFromSelection([ms[0]], { page: 0, topLeft: { x: 0, y: 500 }, identity: by, now: "t", id: "L" })!;
    const { container } = setup([legend, ...ms], ["L"]);
    const rows = container.querySelectorAll("[data-testid=legend-row]");
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain("5.00 m + 7.50 ft");
    expect(rows[0].textContent).not.toContain("12.50");
  });

  it("every cell of hostile mark-up text is rendered as text, never HTML", () => {
    const evil = count("1", A);
    evil.subject = '<img src=x onerror="window.__pwned=1">';
    const legend = legendFromCountSets([A], { page: 0, topLeft: { x: 0, y: 500 }, identity: by, now: "t", id: "L" });
    const { container } = setup([legend, evil], ["L"]);
    expect(container.querySelector("img")).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
    const input = container.querySelector('input[aria-label="Subject for row 1"]') as HTMLInputElement;
    expect(input.placeholder).toBe('<img src=x onerror="window.__pwned=1">');
  });
});
