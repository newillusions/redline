import { describe, it, expect } from "vitest";
import { activateTool } from "./toolchest-activation";
import { MarkupStore, type MarkupIpc } from "./markup-store.svelte";
import type { Tool, Appearance } from "./ipc";

const NOOP_IPC: MarkupIpc = {
  add: async () => {},
  update: async () => {},
  remove: async () => {},
};

const APPEARANCE: Appearance = {
  color: "#00ff00",
  line_weight: 4,
  opacity: 0.5,
  fill: "#ffff00",
  line_style: "Dashed",
  font: null,
};

function tool(overrides: Partial<Tool> = {}): Tool {
  return {
    id: "t1",
    name: "My Tool",
    markup_type: "Rectangle",
    appearance: APPEARANCE,
    subject: null,
    placement_mode: "Properties",
    geometry: null,
    stamp: null,
    ...overrides,
  };
}

describe("activateTool (Tool Chest -> MarkupStore wiring)", () => {
  // --- (c) properties mode applies appearance ---

  it("properties mode: applies the tool's appearance and selects the matching draw tool", () => {
    const store = new MarkupStore("d1", NOOP_IPC);
    activateTool(tool({ markup_type: "Rectangle" }), store);

    expect(store.draftAppearance).toEqual(APPEARANCE);
    expect(store.activeTool).toBe("Rectangle");
    expect(store.pendingPlacementTool).toBeNull();
  });

  it("properties mode: appearance is deep-cloned, not shared by reference", () => {
    const store = new MarkupStore("d1", NOOP_IPC);
    const t = tool({ appearance: { ...APPEARANCE, font: { family: "Arial", size_pt: 10 } } });
    activateTool(t, store);

    store.draftAppearance.color = "#000000";
    expect(t.appearance.color).toBe("#00ff00");
    expect(store.draftAppearance.font).not.toBe(t.appearance.font);
  });

  it("properties mode: a markup_type with no matching draw tool still updates appearance, leaves activeTool alone", () => {
    const store = new MarkupStore("d1", NOOP_IPC);
    store.activeTool = "hand";
    activateTool(tool({ markup_type: "StampDynamic" }), store);

    expect(store.draftAppearance).toEqual(APPEARANCE);
    expect(store.activeTool).toBe("hand");
  });

  // --- (c) drawing mode arms pendingPlacementTool + placeTool ---

  it("drawing mode: arms pendingPlacementTool and switches to the placeTool tool kind", () => {
    const store = new MarkupStore("d1", NOOP_IPC);
    const t = tool({ placement_mode: "Drawing", geometry: { Point: { x: 1, y: 2 } } });
    activateTool(t, store);

    expect(store.activeTool).toBe("placeTool");
    expect(store.pendingPlacementTool).toEqual(t);
  });

  it("switching from a drawing-mode tool to a properties-mode tool clears pendingPlacementTool", () => {
    const store = new MarkupStore("d1", NOOP_IPC);
    activateTool(tool({ placement_mode: "Drawing", geometry: { Point: { x: 0, y: 0 } } }), store);
    expect(store.pendingPlacementTool).not.toBeNull();

    activateTool(tool({ markup_type: "Ellipse", placement_mode: "Properties" }), store);
    expect(store.pendingPlacementTool).toBeNull();
    expect(store.activeTool).toBe("Ellipse");
  });
});
