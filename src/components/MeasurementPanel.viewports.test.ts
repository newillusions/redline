// @vitest-environment jsdom
/**
 * Measurement viewports in the Measurements panel (docs/measurement-viewports.md): the list,
 * Edit (preset scale -> update_viewport), Redraw, Re-apply (explicit, one undo step) and
 * Delete (confirmed, then measurements fall back).
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/svelte";
import { tick } from "svelte";
import MeasurementPanel from "./MeasurementPanel.svelte";
import ViewportDialog from "./ViewportDialog.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { TakeoffStore } from "$lib/takeoff-store.svelte";
import type { BBox, Markup, ScaleRecord } from "$lib/ipc";
import { updateViewport } from "$lib/ipc";

vi.mock("$lib/ipc", () => ({
  exportMarkupList: vi.fn(async () => {}),
  deleteScale: vi.fn(async () => true),
  updateViewport: vi.fn(async (_doc: string, id: string, c: Record<string, unknown>) => ({
    id,
    applies_to: { kind: "Viewport", page: 0, viewport_id: "vp-1", name: (c.name as string) ?? "Detail A", bbox: [100, 100, 200, 200] },
    method: "Preset", ratio: (c.ratio as number) ?? 0.001, unit: (c.unit as string) ?? "m", label: (c.label as string) ?? "1:20", precision: 2,
  })),
  getPageSize: vi.fn(async () => ({ width_pts: 612, height_pts: 792 })),
  isMarkupLocked: () => false,
  isMarkupContentsLocked: () => false,
  getUserIdentity: vi.fn(async () => ({ user_id: "actor", display_name: "Actor" })),
  COUNT_SYMBOLS: ["Circle", "Square", "Triangle", "Diamond", "Cross", "Star", "Hexagon"],
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(async () => null) }));

const AUDIT = {
  created_by: { user_id: "u1", display_name: "T" }, created_at: "2026-01-01T00:00:00Z",
  modified_by: { user_id: "u1", display_name: "T" }, modified_at: "2026-01-01T00:00:00Z",
  revision: 0, origin: "Desktop" as const,
};

const def: ScaleRecord = { id: "d", applies_to: { kind: "DocumentDefault" }, method: "TwoPoint", ratio: 0.01, unit: "m", label: "1:100", precision: 2 };
const detail = (bbox: BBox = [100, 100, 200, 200]): ScaleRecord => ({
  id: "vp", applies_to: { kind: "Viewport", page: 0, viewport_id: "vp-1", name: "Detail A", bbox },
  method: "Preset", ratio: 0.001, unit: "m", label: "1:20", precision: 2,
});

function length(id: string, x: number, scaleRef: string, qty: number): Markup {
  return {
    id, page: 0, markup_type: "MeasurementLength",
    geometry: { Polyline: [{ x, y: 150 }, { x: x + 1000, y: 150 }] },
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
    subject: null, layer: null, contents: null, group_id: null,
    measurement: { scale_ref: scaleRef, raw_measure: 1000, unit: "m", computed_quantity: qty, depth: null, count_value: null, custom_columns: {} },
    workflow: { status: "None", assignee: null, thread: [] }, audit: AUDIT,
  } as Markup;
}

const fakeIpc = () => ({
  add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}),
  putScale: vi.fn(async () => {}),
});

describe("MeasurementPanel viewports", () => {
  let store: MarkupStore;
  let takeoffStore: TakeoffStore;

  let ipc: ReturnType<typeof fakeIpc>;

  beforeEach(() => {
    vi.clearAllMocks();
    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
    takeoffStore = new TakeoffStore();
    takeoffStore.seedScales([def, detail()]);
  });

  it("lists viewports with page and scale, and names the viewport in a measurement's Scale cell", async () => {
    store.markups.push(length("m1", 150, "vp", 1));
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    await tick();
    const table = screen.getByRole("table", { name: /measurement viewports/i });
    const row = within(table).getByText("Detail A").closest("tr")!;
    expect(row.textContent).toContain("1:20");
    expect(row.querySelector(".num-col")?.textContent?.trim()).toBe("1");
    expect(screen.getByRole("table", { name: /measurement quantities/i }).textContent).toContain("Detail A (1:20)");
  });

  it("Redraw arms the redraw tool for that viewport; New viewport uses the plain viewport tool", async () => {
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    await fireEvent.click(screen.getByRole("button", { name: "Redraw" }));
    expect(store.activeTool).toBe("viewportRedraw");
    expect(takeoffStore.redrawViewportId).toBe("vp");
    await fireEvent.click(screen.getByRole("button", { name: "New viewport" }));
    expect(store.activeTool).toBe("viewport"); // not the redraw tool: a drag makes a NEW viewport
  });

  it("Re-apply puts measurements starting inside under the viewport scale as one undo step", async () => {
    store.markups.push(length("in", 150, "d", 10), length("out", 500, "d", 10));
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    await new Promise((r) => setTimeout(r, 0)); // identity resolves
    await tick();
    const applyBatch = vi.spyOn(store, "applyBatch");
    await fireEvent.click(screen.getByRole("button", { name: "Re-apply" }));
    expect(applyBatch).toHaveBeenCalledTimes(1);
    const pairs = applyBatch.mock.calls[0][0] as { before: Markup; after: Markup }[];
    expect(pairs.map((p) => p.before.id)).toEqual(["in"]);
    expect(pairs[0].after.measurement?.computed_quantity).toBeCloseTo(1, 9);
    expect(screen.getByRole("status").textContent).toMatch(/1 measurement inside Detail A now uses its scale/);
  });

  it("Delete asks first, keeps the measurements' values, and one undo brings the viewport back", async () => {
    store.markups.push(length("in", 150, "vp", 1));
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    await new Promise((r) => setTimeout(r, 0));
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(ipc.putScale).not.toHaveBeenCalled();
    expect(screen.getByText(/1 measurement taken under it keeps its current value \(1:20\)/)).toBeTruthy();
    const dialog = screen.getByRole("dialog");
    await fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await store.flush();
    // Retired, not removed: hidden from the list and never picked, but still resolvable.
    expect(takeoffStore.viewports).toHaveLength(0);
    expect(takeoffStore.scales.find((s) => s.id === "vp")?.retired).toBe(true);
    expect(ipc.putScale).toHaveBeenCalledWith("d1", "vp", expect.objectContaining({ id: "vp", retired: true }));
    const m = store.markups.find((x) => x.id === "in")!;
    expect(m.measurement?.scale_ref).toBe("vp");
    expect(m.measurement?.computed_quantity).toBe(1);
    // Undo restores the viewport (and the backend gets it back).
    store.undo();
    await store.flush();
    expect(takeoffStore.viewports.map((v) => v.id)).toEqual(["vp"]);
    const last = ipc.putScale.mock.calls.at(-1) as unknown as [string, string, ScaleRecord];
    expect([last[1], last[2].id, last[2].retired ?? false]).toEqual(["vp", "vp", false]);
  });

  it("Edit with a drawing scale updates the viewport (same scale id) and its measurements follow", async () => {
    store.markups.push(length("in", 150, "vp", 1));
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    await new Promise((r) => setTimeout(r, 0));
    await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const dialog = screen.getByRole("dialog", { name: /edit viewport/i });
    await fireEvent.click(within(dialog).getByLabelText(/drawing scale/i, { selector: "input" }));
    await fireEvent.change(within(dialog).getByLabelText("Drawing scale", { selector: "select" }), { target: { value: "6" } }); // 1:50
    const applyBatch = vi.spyOn(store, "applyBatch");
    await fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await tick();
    await tick();
    expect(updateViewport).toHaveBeenCalledWith("d1", "vp", expect.objectContaining({ label: "1:50", unit: "m", twoPoint: false }));
    expect(takeoffStore.scales.find((s) => s.id === "vp")?.label).toBe("1:50");
    expect(applyBatch).not.toHaveBeenCalled(); // not a separate frame: the scale change carries it
    const q = () => store.markups.find((x) => x.id === "in")!.measurement!.computed_quantity;
    expect(q()).toBeCloseTo(1000 * (50 * 0.0254) / 72, 9);
    // ONE undo restores the old scale AND the old quantity together.
    store.undo();
    expect(takeoffStore.scales.find((s) => s.id === "vp")?.label).toBe("1:20");
    expect(q()).toBe(1);
    store.redo();
    expect(takeoffStore.scales.find((s) => s.id === "vp")?.label).toBe("1:50");
  });

  it("a failed edit is shown in the error banner, not only the console", async () => {
    vi.mocked(updateViewport).mockRejectedValueOnce(new Error("sidecar is read-only"));
    const seen: string[] = [];
    const onErr = (e: Event) => seen.push(String((e as CustomEvent<string>).detail));
    window.addEventListener("redline:surface-error", onErr);
    render(MeasurementPanel, { props: { store, takeoffStore, docId: "d1" } });
    await fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const dialog = screen.getByRole("dialog", { name: /edit viewport/i });
    await fireEvent.click(within(dialog).getByLabelText(/drawing scale/i, { selector: "input" }));
    await fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await new Promise((r) => setTimeout(r, 0));
    window.removeEventListener("redline:surface-error", onErr);
    expect(seen).toEqual(["Changing the viewport failed: sidecar is read-only"]);
  });
});

describe("ViewportDialog", () => {
  it("returns a preset ratio, or asks to calibrate", async () => {
    const onConfirm = vi.fn();
    render(ViewportDialog, { props: { initialName: "Viewport 1", onConfirm, onCancel: vi.fn() } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onConfirm).toHaveBeenLastCalledWith(expect.objectContaining({
      name: "Viewport 1", mode: "preset", label: "1:20", unit: "m", ratio: expect.closeTo((20 * 0.0254) / 72, 12),
    }));
    await fireEvent.click(screen.getByLabelText(/calibrate: click two points/i));
    await fireEvent.click(screen.getByRole("button", { name: /next: calibrate/i }));
    expect(onConfirm).toHaveBeenLastCalledWith({ name: "Viewport 1", mode: "calibrate" });
  });

  it("refuses an empty name", async () => {
    render(ViewportDialog, { props: { initialName: "", onConfirm: vi.fn(), onCancel: vi.fn() } });
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
