import { describe, it, expect, vi } from "vitest";
import type { CountSet, Markup, UserRef } from "./ipc";
import { MarkupStore } from "./markup-store.svelte";
import { effectiveRect, legendFromCountSets } from "./legend";
import { mk } from "./markups-list.test-util";

vi.mock("./ipc", async (orig) => ({
  ...(await orig<typeof import("./ipc")>()),
  isMarkupLocked: (m: { annot_flags?: number }) => ((m.annot_flags ?? 0) & 0x80) !== 0,
  isMarkupContentsLocked: () => false,
}));

const by: UserRef = { user_id: "u", display_name: "T" };
const A: CountSet = { id: "a", name: "A", color: "#ff0000", symbol: "Square" };
const B: CountSet = { id: "b", name: "B", color: "#00ff00", symbol: "Circle" };

function count(id: string, set: CountSet, n = 1): Markup {
  return mk(id, {
    markup_type: "MeasurementCount",
    appearance: { color: set.color, line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
    count_set: set,
    measurement: { scale_ref: null, raw_measure: n, unit: "ea", computed_quantity: n, depth: null, count_value: n, custom_columns: {}, rise_drop: [] },
  } as Partial<Markup>);
}

const ipc = () => ({ add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) });
const rectOf = (m: Markup) => ("Rect" in m.geometry ? m.geometry.Rect : null)!;

function storeWith(markups: Markup[]) {
  const i = ipc();
  const store = new MarkupStore("d", i);
  store.seed(markups);
  return { store, ipc: i };
}

describe("MarkupStore.syncLegends", () => {
  const legend = () => legendFromCountSets([A], { page: 0, topLeft: { x: 10, y: 300 }, identity: by, now: "t", id: "L" });

  it("moves the bottom edge with the rows in memory only: no undo entry, no audit bump, not dirty, not mirrored", async () => {
    const l = legend();
    // A stale rectangle: three rows tall although one row is shown.
    const stale = { ...l, geometry: { Rect: { min: { x: 10, y: 100 }, max: { x: 310, y: 300 } } } } as Markup;
    const { store, ipc: i } = storeWith([stale, count("1", A)]);
    expect(store.dirty).toBe(false);
    store.syncLegends();
    const want = effectiveRect(stale, 1)!;
    expect(rectOf(store.getById("L")!).min.y).toBeCloseTo(want.y0, 9);
    expect(rectOf(store.getById("L")!).max.y).toBe(300);
    expect(store.canUndo).toBe(false);
    expect(store.getById("L")!.audit.revision).toBe(0);
    expect(store.dirty).toBe(false);
    await store.flush();
    expect(i.update).not.toHaveBeenCalled();
  });

  it("settles: a second call changes nothing", async () => {
    const { store } = storeWith([legend(), count("1", A), count("2", B)]);
    store.syncLegends();
    const snapshot = JSON.stringify(store.markups);
    store.syncLegends();
    expect(JSON.stringify(store.markups)).toBe(snapshot);
  });

  it("grows when a new count set appears and shrinks back on undo of that marker", async () => {
    const { store } = storeWith([legend(), count("1", A)]);
    store.syncLegends();
    const one = rectOf(store.getById("L")!).min.y;
    store.create(count("2", B)); // legend has auto_count_sets
    store.syncLegends();
    const two = rectOf(store.getById("L")!).min.y;
    expect(two).toBeLessThan(one);
    store.undo();
    store.syncLegends();
    expect(rectOf(store.getById("L")!).min.y).toBeCloseTo(one, 9);
  });

  it("leaves a locked legend and a document without legends alone", async () => {
    const locked = { ...legend(), annot_flags: 0x80, geometry: { Rect: { min: { x: 10, y: 0 }, max: { x: 310, y: 300 } } } } as Markup;
    const { store, ipc: i } = storeWith([locked, count("1", A)]);
    store.syncLegends();
    expect(rectOf(store.getById("L")!).min.y).toBe(0);
    const plain = storeWith([count("1", A)]);
    plain.store.syncLegends();
    expect(plain.store.dirty).toBe(false);
    expect(i.update).not.toHaveBeenCalled();
  });
});
