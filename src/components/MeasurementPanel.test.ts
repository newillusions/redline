// @vitest-environment jsdom
/**
 * MeasurementPanel tests — quantity table for M3 takeoff.
 * Tests reactive filtering, totals computation, and empty state.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { fireEvent } from "@testing-library/svelte";
import MeasurementPanel from "./MeasurementPanel.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { TakeoffStore } from "$lib/takeoff-store.svelte";
import type { CountSet, Markup } from "$lib/ipc";

// Mock IPC modules — dialog and ipc calls are not needed in unit tests.
// COUNT_SYMBOLS is a runtime value consumed by CountSetPicker, so it must be present.
vi.mock("$lib/ipc", () => ({
  exportMarkupList: vi.fn(async () => {}),
  isMarkupLocked: (m: { annot_flags?: number }) => ((m.annot_flags ?? 0) & 0x80) !== 0,
  isMarkupContentsLocked: (m: { annot_flags?: number }) => ((m.annot_flags ?? 0) & 0x200) !== 0,
  getUserIdentity: vi.fn(async () => ({ user_id: "actor", display_name: "Actor" })),
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

  it("prints two decimals for scaled and unscaled measurements, like the exports and Markups List", async () => {
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    store.markups.push(makeMeasurementMarkup("m1", "MeasurementLength", 12.5, "pt", null));
    store.markups.push(makeMeasurementMarkup("m2", "MeasurementLength", 3.14159, "m", "s1"));
    await tick();
    const cells = [...document.querySelectorAll("tbody td.num-col")].map((c) => c.textContent?.trim());
    expect(cells).toEqual(["12.50", "3.14"]);
  });

  it("a count with no recorded value counts once (the same rule as the Markups List and exports)", async () => {
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    store.markups.push(makeMeasurementMarkup("c1", "MeasurementCount", 1, "ea", null, null));
    await tick();
    const row = document.querySelector("tbody tr")!;
    expect(row.querySelector("td.num-col")!.textContent!.trim()).toBe("1");
    const subtotal = document.querySelector("tfoot tr.count-subtotal td.num-col")!;
    expect(subtotal.textContent!.trim()).toBe("1");
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

  it("enables the export buttons for a review mark-up even with no measurements (UX audit F3)", async () => {
    render(MeasurementPanel, { store, takeoffStore, docId: "d1" });
    expect((screen.getByRole("button", { name: /export xlsx/i }) as HTMLButtonElement).disabled).toBe(true);
    const review = { ...makeMeasurementMarkup("r1", "MeasurementLength", 0, "m"), markup_type: "Rectangle" as const, measurement: null };
    store.seed([review]);
    await tick();
    expect((screen.getByRole("button", { name: /export xlsx/i }) as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByRole("button", { name: /export csv/i }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("reports a successful export with the destination path (UX audit round 2)", async () => {
    vi.mocked(saveDialog).mockResolvedValueOnce("/out/list.xlsx");
    const review = { ...makeMeasurementMarkup("r1", "MeasurementLength", 0, "m"), markup_type: "Rectangle" as const, measurement: null };
    store.seed([review]);
    render(MeasurementPanel, { store, takeoffStore, docId: "d1" });
    await fireEvent.click(screen.getByRole("button", { name: /export xlsx/i }));
    expect(await screen.findByText("Exported 1 mark-up to /out/list.xlsx.")).toBeTruthy();
  });

  it("Resume on a legacy ungrouped count stamps count_group so every view shows one count", async () => {
    store.seed([makeMeasurementMarkup("legacy", "MeasurementCount", 1, "ea", null, 1)]);
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    await tick();
    await fireEvent.click(document.querySelector('[data-action="resume-count"]')!);
    expect(store.getById("legacy")!.count_group).toBe("legacy");
    expect(store.countRun?.groupId).toBe("legacy");
    expect(store.activeTool).toBe("MeasurementCount");
  });

  it("Resume stamps the ACTING user as modified_by, not the original author (R2-1)", async () => {
    store.seed([makeMeasurementMarkup("legacy", "MeasurementCount", 1, "ea", null, 1)]);
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    await tick();
    await tick();
    await fireEvent.click(document.querySelector('[data-action="resume-count"]')!);
    expect(store.getById("legacy")!.audit.modified_by.user_id).toBe("actor");
  });

  it("Resume on a locked legacy count is refused with a message and arms no run (R2-2)", async () => {
    store.seed([{ ...makeMeasurementMarkup("legacy", "MeasurementCount", 1, "ea", null, 1), annot_flags: 0x80 }]);
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    await tick();
    await fireEvent.click(document.querySelector('[data-action="resume-count"]')!);
    expect(store.countRun).toBeNull();
    expect(store.getById("legacy")!.count_group ?? null).toBeNull();
    expect(screen.getByText(/locked, so it cannot be resumed/)).toBeTruthy();
  });
});
