// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import PolylengthControls from "./PolylengthControls.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { TakeoffStore } from "$lib/takeoff-store.svelte";
import { polylengthPayload } from "$lib/polylength";
import type { Markup, UserRef } from "$lib/ipc";

vi.mock("$lib/ipc", () => ({
  isMarkupLocked: () => false,
  isMarkupContentsLocked: () => false,
}));

const by: UserRef = { user_id: "u", display_name: "T" };
const SCALE = { id: "s", applies_to: { kind: "DocumentDefault" as const }, method: "Preset" as const, ratio: 0.001, unit: "m", label: "t", precision: 2 };
const PTS = [{ x: 0, y: 0 }, { x: 1000, y: 0 }, { x: 1000, y: 1000 }, { x: 2000, y: 1000 }];
const ipc = () => ({ add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) });

function polylen(): Markup {
  return {
    id: "pl", markup_type: "MeasurementLength", page: 0, geometry: { Polyline: PTS },
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null },
    subject: null, layer: null, contents: null, group_id: null,
    audit: { created_by: by, created_at: "", modified_by: by, modified_at: "", revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: polylengthPayload(PTS, SCALE),
  } as Markup;
}

function setup() {
  const store = new MarkupStore("d", ipc());
  store.seed([polylen()]);
  store.selectedIds = new Set(["pl"]);
  const takeoffStore = new TakeoffStore();
  takeoffStore.addScale(SCALE);
  const r = render(PolylengthControls, { props: { store, takeoffStore, identity: by } });
  return { store, ...r };
}

describe("PolylengthControls", () => {
  it("shows the total under the page's scale and one row per segment", () => {
    const { getByTestId, container } = setup();
    expect(getByTestId("polylength-total").textContent).toContain("3.00 m");
    expect(container.querySelectorAll("tbody tr")).toHaveLength(3);
  });

  it("entering a rise/drop adds it to the total (one undo step)", async () => {
    const { store, container, getByTestId } = setup();
    const input = container.querySelector('input[aria-label="Rise or drop for segment 2"]') as HTMLInputElement;
    await fireEvent.change(input, { target: { value: "-2.5" } });
    expect(store.getById("pl")!.measurement!.computed_quantity).toBeCloseTo(5.5, 9);
    expect(store.getById("pl")!.measurement!.rise_drop).toEqual([{ segment: 1, amount: -2.5, unit: "m" }]);
    expect(getByTestId("polylength-total").textContent).toContain("5.50 m");
    store.undo();
    expect(store.getById("pl")!.measurement!.computed_quantity).toBeCloseTo(3, 9);
  });

  it("rejects an oversized value and keeps the total", async () => {
    const { store, container } = setup();
    const input = container.querySelector('input[aria-label="Rise or drop for segment 1"]') as HTMLInputElement;
    await fireEvent.change(input, { target: { value: "1e12" } });
    expect(store.getById("pl")!.measurement!.rise_drop).toEqual([]);
    expect(container.textContent).toMatch(/between -1e9 and 1e9/);
  });

  it("Split at vertex makes two polylengths whose totals sum to the original", async () => {
    const { store, container } = setup();
    const sel = container.querySelector("#split-vertex") as HTMLSelectElement;
    await fireEvent.change(sel, { target: { value: "2" } });
    await fireEvent.click(container.querySelector("[data-action=split-polylength]")!);
    expect(store.markups).toHaveLength(2);
    const sum = store.markups.reduce((n, m) => n + m.measurement!.computed_quantity, 0);
    expect(sum).toBeCloseTo(3, 9);
    store.undo();
    expect(store.markups).toHaveLength(1);
  });
});
