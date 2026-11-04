import { describe, it, expect } from "vitest";
import { activateTool, activateAndRecord, numberedTools } from "./toolchest-activation";
import { MarkupStore, type MarkupIpc } from "./markup-store.svelte";
import type { Tool, ToolSet, Appearance } from "./ipc";

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
    children: [],
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

describe("numbered tools (digit keys)", () => {
  const set = (id: string, n: number): ToolSet => ({
    id,
    name: id,
    tools: Array.from({ length: n }, (_, i) => tool({ id: `${id}-${i}`, name: `${id} ${i}` })),
  });

  it("are the first nine tools of the first set, in order", () => {
    const out = numberedTools([set("a", 12), set("b", 3)]);
    expect(out).toHaveLength(9);
    expect(out[0].id).toBe("a-0");
    expect(out[8].id).toBe("a-8");
  });

  it("are empty with no sets, and short for a short first set", () => {
    expect(numberedTools([])).toEqual([]);
    expect(numberedTools([set("a", 2), set("b", 9)])).toHaveLength(2);
  });
});

describe("activateAndRecord", () => {
  it("activates the tool and records it once, marking the change to be kept out of the plain Recent", () => {
    const store = new MarkupStore("d1", NOOP_IPC);
    store.activeTool = "hand";
    const recorded: string[] = [];
    activateAndRecord(tool({ id: "x", markup_type: "Ellipse" }), store, {
      recordRecent: async (t) => { recorded.push(t.id); },
    });
    expect(store.activeTool).toBe("Ellipse");
    expect(store.skipRecentKind).toBe("Ellipse");
    expect(recorded).toEqual(["x"]);
  });

  it("does not leave a stale skip mark when the tool kind does not change", () => {
    const store = new MarkupStore("d1", NOOP_IPC);
    store.activeTool = "Rectangle";
    activateAndRecord(tool({ markup_type: "Rectangle" }), store, { recordRecent: async () => {} });
    expect(store.skipRecentKind).toBeNull();
  });
});
