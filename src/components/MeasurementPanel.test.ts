// @vitest-environment jsdom
/**
 * MeasurementPanel tests — quantity table for M3 takeoff.
 * Tests reactive filtering, totals computation, and empty state.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import MeasurementPanel from "./MeasurementPanel.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { TakeoffStore } from "$lib/takeoff-store.svelte";
import type { CountSet, Markup } from "$lib/ipc";

// Mock IPC modules — dialog and ipc calls are not needed in unit tests.
// COUNT_SYMBOLS is a runtime value consumed by CountSetPicker, so it must be present.
vi.mock("$lib/ipc", () => ({
  exportMarkupList: vi.fn(async () => {}),
  COUNT_SYMBOLS: ["Circle", "Square", "Triangle", "Diamond", "Cross", "Star", "Hexagon"],
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: vi.fn(async () => null),
}));

function fakeIpc() {
  return {
    add: vi.fn(async () => {}),
    update: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
  };
}

const BASE_AUDIT = {
  created_by: { user_id: "u1", display_name: "T" },
  created_at: "2026-01-01T00:00:00Z",
  modified_by: { user_id: "u1", display_name: "T" },
  modified_at: "2026-01-01T00:00:00Z",
  revision: 0,
  origin: "Desktop" as const,
};

function makeMeasurementMarkup(
  id: string,
  type: "MeasurementLength" | "MeasurementArea" | "MeasurementCount",
  computedQty: number,
  unit: string,
  scaleRef: string | null = null,
  countValue: number | null = null
): Markup {
  return {
    id,
    page: 0,
    markup_type: type,
    geometry: { Point: { x: 0, y: 0 } },
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
    subject: null,
    layer: null,
    contents: null,
    group_id: null,
    measurement: {
      scale_ref: scaleRef,
      raw_measure: 1,
      unit,
      computed_quantity: computedQty,
      depth: null,
      count_value: countValue,
      custom_columns: {},
    },
    workflow: { status: "None", assignee: null, thread: [] },
    audit: BASE_AUDIT,
  };
}

describe("MeasurementPanel", () => {
  let store: MarkupStore;
  let takeoffStore: TakeoffStore;

  beforeEach(() => {
    store = new MarkupStore("d1", fakeIpc());
    takeoffStore = new TakeoffStore();
  });

  it("shows empty hint when no measurements", () => {
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    expect(screen.getByText(/no measurements yet/i)).toBeTruthy();
  });

  it("shows quantity table when measurements exist", async () => {
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    store.markups.push(makeMeasurementMarkup("m1", "MeasurementLength", 12.5, "m", "s1"));
    await tick();
    expect(screen.getByRole("table", { name: /measurement quantities/i })).toBeTruthy();
    expect(screen.getByText("Length")).toBeTruthy();
  });

  it("filters out non-measurement markups", async () => {
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    // A regular rectangle markup should not appear.
    store.markups.push({
      id: "rect1",
      page: 0,
      markup_type: "Rectangle",
      geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } },
      appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
      subject: null,
      layer: null,
      contents: null,
      group_id: null,
      measurement: null,
      workflow: { status: "None", assignee: null, thread: [] },
      audit: BASE_AUDIT,
    });
    await tick();
    expect(screen.getByText(/no measurements yet/i)).toBeTruthy();
  });

  it("totals row sums computed quantities across MeasurementLength items", async () => {
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    store.markups.push(makeMeasurementMarkup("m1", "MeasurementLength", 10, "m", "s1"));
    store.markups.push(makeMeasurementMarkup("m2", "MeasurementLength", 5.5, "m", "s1"));
    await tick();
    // Total should be 15.50 in the first totals row.
    expect(screen.getByText("15.50")).toBeTruthy();
  });

  it("shows count items with count_value", async () => {
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    store.markups.push(makeMeasurementMarkup("c1", "MeasurementCount", 1, "ea", "s1", 1));
    store.markups.push(makeMeasurementMarkup("c2", "MeasurementCount", 1, "ea", "s1", 1));
    await tick();
    // The set subtotal row AND the per-page breakdown row both show "2" (total count and page count).
    expect(screen.getAllByText("2").length).toBeGreaterThanOrEqual(1);
  });

  it("shows a separate subtotal row per count set", async () => {
    const setA: CountSet = { id: "set-a", name: "Type-A fixture", color: "#0066ff", symbol: "Triangle" };
    const setB: CountSet = { id: "set-b", name: "Type-B fixture", color: "#00875a", symbol: "Square" };
    const withSet = (id: string, set: CountSet) => ({
      ...makeMeasurementMarkup(id, "MeasurementCount", 1, "ea", "s1", 1),
      count_set: set,
    });
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    store.markups.push(withSet("a1", setA), withSet("a2", setA), withSet("b1", setB));
    await tick();
    // Each set is named with its own subtotal; a grand total appears when >1 set.
    expect(screen.getByText("Type-A fixture")).toBeTruthy();
    expect(screen.getByText("Type-B fixture")).toBeTruthy();
    expect(screen.getByText("All counts")).toBeTruthy();
  });

  it("displays no scale set when takeoffStore has no active scale", () => {
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    expect(screen.getByText("No scale set")).toBeTruthy();
  });

  it("displays scale label when takeoffStore has active scale", async () => {
    const scale = { id: "s1", applies_to: { kind: "DocumentDefault" as const }, method: "TwoPoint" as const,
      ratio: 0.001, unit: "m", label: "1:1000", precision: 2 };
    takeoffStore.addScale(scale);
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    expect(screen.getByText("1:1000 (m)")).toBeTruthy();
  });
});
