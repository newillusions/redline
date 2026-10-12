// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import CountControls from "./CountControls.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import type { Markup, UserRef } from "$lib/ipc";

vi.mock("$lib/ipc", () => ({
  isMarkupLocked: () => false,
  isMarkupContentsLocked: () => false,
  COUNT_SYMBOLS: ["Circle"],
}));

const by: UserRef = { user_id: "u", display_name: "T" };
const SET = { id: "setA", name: "Type A", color: "#e02424", symbol: "Circle" as const };
const ipc = () => ({ add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) });

function item(id: string, group: string | null, o: Partial<Markup> = {}): Markup {
  return {
    id, markup_type: "MeasurementCount", page: 0, geometry: { Point: { x: 1, y: 1 } },
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null },
    subject: null, layer: null, contents: null, group_id: null, count_group: group,
    audit: { created_by: by, created_at: "", modified_by: by, modified_at: "", revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: { scale_ref: null, raw_measure: 1, unit: "ea", computed_quantity: 1, depth: null, count_value: 1, custom_columns: {} },
    count_set: SET, ...o,
  } as Markup;
}

function setup(markups: Markup[], selected: string[]) {
  const store = new MarkupStore("d", ipc());
  store.seed(markups);
  store.selectedIds = new Set(selected);
  const r = render(CountControls, { props: { store, identity: by } });
  return { store, ...r };
}

describe("CountControls", () => {
  it("renders nothing without count items selected", () => {
    const { container } = setup([item("a", "G")], []);
    expect(container.querySelector("[data-testid=count-controls]")).toBeNull();
  });

  it("shows the whole count's total for a selected item", () => {
    const { getByTestId } = setup([item("a", "G"), item("b", "G"), item("c", "G")], ["a"]);
    expect(getByTestId("count-total").textContent).toContain("3");
  });

  it("Split moves the selected items into a new count (one undo step)", async () => {
    const { store, container } = setup([item("a", "G"), item("b", "G"), item("c", "G")], ["b", "c"]);
    await fireEvent.click(container.querySelector("[data-action=split]")!);
    const groups = new Set(store.markups.map((m) => m.count_group));
    expect(groups.size).toBe(2);
    expect(store.getById("a")!.count_group).toBe("G");
    expect(store.getById("b")!.count_group).toBe(store.getById("c")!.count_group);
    store.undo();
    expect(new Set(store.markups.map((m) => m.count_group))).toEqual(new Set(["G"]));
  });

  it("Merge combines two counts; disabled with one", async () => {
    const { store, container } = setup([item("a", "G"), item("b", "H")], ["a", "b"]);
    await fireEvent.click(container.querySelector("[data-action=merge]")!);
    expect(new Set(store.markups.map((m) => m.count_group)).size).toBe(1);
    const one = setup([item("x", "G")], ["x"]);
    expect((one.container.querySelector("[data-action=merge]") as HTMLButtonElement).disabled).toBe(true);
  });

  it("Merge refuses different sets and says why", async () => {
    const other = { ...SET, id: "setB", name: "Type B" };
    const { store, container } = setup([item("a", "G"), item("b", "H", { count_set: other })], ["a", "b"]);
    await fireEvent.click(container.querySelector("[data-action=merge]")!);
    expect(container.textContent).toMatch(/same type/);
    expect(store.getById("a")!.count_group).toBe("G");
  });

  it("Label and Colour apply to the whole count", async () => {
    const { store, container } = setup([item("a", "G"), item("b", "G"), item("c", "H")], ["a"]);
    const label = container.querySelector("#count-label") as HTMLInputElement;
    await fireEvent.change(label, { target: { value: "Exit sign" } });
    const colour = container.querySelector("#count-colour") as HTMLInputElement;
    await fireEvent.change(colour, { target: { value: "#00ff00" } });
    expect(store.getById("a")!.subject).toBe("Exit sign");
    expect(store.getById("b")!.subject).toBe("Exit sign");
    expect(store.getById("b")!.appearance.color).toBe("#00ff00");
    expect(store.getById("c")!.subject).toBeNull();
  });

  it("Resume arms the Count tool on the selected count", async () => {
    const { store, container } = setup([item("a", "G"), item("b", "G")], ["a"]);
    await fireEvent.click(container.querySelector("[data-action=resume]")!);
    await tick();
    expect(store.activeTool).toBe("MeasurementCount");
    expect(store.countRun).toMatchObject({ groupId: "G", page: 0, setId: "setA", size: 2 });
  });
});
