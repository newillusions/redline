import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  addLayerToFile, defaultOff, findNode, flattenRows, isMarkupShown, isOcShown, isolate, markupOcRef,
  readLayers, saveLayersDefault, setGroups, setLayerView, showAll, subtreeIds, toggleLayer,
  type Layer, type LayerModel, type LayerNode, type Ocmd,
} from "./layers";

function layer(id: number, over: Partial<Layer> = {}): Layer {
  return { id, gen: 0, name: `L${id}`, default_on: true, locked: false, view_intent: true, usage_view_state: null, ...over };
}

function model(layers: Layer[], tree?: LayerNode[], extra: Partial<LayerModel> = {}): LayerModel {
  return {
    layers,
    tree: tree ?? layers.map((l) => ({ kind: "Layer", id: l.id, children: [] })),
    ocmds: [],
    rb_groups: [],
    has_ocproperties: true,
    truncated: false, layers_truncated: false,
    unlisted: 0,
    view_supported: true,
    view_unsupported_reason: null,
    encrypted: false,
    ...extra,
  };
}

const off = (...ids: number[]) => new Set(ids);

describe("ipc invoke argument keys (Tauri v2 camelCase)", () => {
  const mockInvoke = vi.mocked(invoke);
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined as never);
  });

  it("read_layers -> docId / force", async () => {
    await readLayers("d1", true);
    expect(mockInvoke).toHaveBeenCalledWith("read_layers", { docId: "d1", force: true });
  });
  it("set_layer_view -> docId / off (array, or null for the file as it is)", async () => {
    await setLayerView("d1", [3, 4]);
    expect(mockInvoke).toHaveBeenCalledWith("set_layer_view", { docId: "d1", off: [3, 4] });
    await setLayerView("d1", null);
    expect(mockInvoke).toHaveBeenLastCalledWith("set_layer_view", { docId: "d1", off: null });
  });
  it("add_layer -> docId / name / parent", async () => {
    await addLayerToFile("d1", "Lighting", 9);
    expect(mockInvoke).toHaveBeenCalledWith("add_layer", { docId: "d1", name: "Lighting", parent: 9 });
  });
  it("save_layers_default -> docId / off", async () => {
    await saveLayersDefault("d1", [5]);
    expect(mockInvoke).toHaveBeenCalledWith("save_layers_default", { docId: "d1", off: [5] });
  });
});

describe("defaultOff", () => {
  it("lists the groups the file hides, never a group without View intent", () => {
    const m = model([layer(1), layer(2, { default_on: false }), layer(3, { default_on: false, view_intent: false })]);
    expect([...defaultOff(m)]).toEqual([2]);
  });
});

describe("isOcShown: groups and membership dictionaries (ISO 32000-1 8.11.2)", () => {
  const m = (ocmds: Ocmd[], layers = [layer(1), layer(2), layer(3, { view_intent: false })]) => ({
    ...model(layers),
    ocmds,
  });

  it("a group follows its state; a design-intent group is always shown", () => {
    const mm = m([]);
    expect(isOcShown(mm, off(), 1)).toBe(true);
    expect(isOcShown(mm, off(1), 1)).toBe(false);
    expect(isOcShown(mm, off(3), 3)).toBe(true);
  });

  it("an unknown number (deleted object, not optional content) is shown", () => {
    expect(isOcShown(m([]), off(1, 2), 999)).toBe(true);
  });

  it("policies over the groups: AnyOn (default), AllOn, AnyOff, AllOff", () => {
    const base = { id: 10, ocgs: [1, 2], ve: null };
    const cases: [string, number[], boolean][] = [
      ["AnyOn", [], true], ["AnyOn", [1], true], ["AnyOn", [1, 2], false],
      ["AllOn", [], true], ["AllOn", [1], false],
      ["AnyOff", [], false], ["AnyOff", [2], true],
      ["AllOff", [], false], ["AllOff", [1], false], ["AllOff", [1, 2], true],
    ];
    for (const [policy, hidden, want] of cases) {
      expect(isOcShown(m([{ ...base, policy }]), new Set(hidden), 10), `${policy} hidden=${hidden}`).toBe(want);
    }
  });

  it("a membership dictionary with no existing groups has no effect (shown)", () => {
    expect(isOcShown(m([{ id: 10, policy: "AllOn", ocgs: [77], ve: null }]), off(1, 2), 10)).toBe(true);
    expect(isOcShown(m([{ id: 10, policy: "AllOn", ocgs: [], ve: null }]), off(1, 2), 10)).toBe(true);
  });

  it("a visibility expression wins over the policy", () => {
    // Policy AnyOn over [1] would say "shown while 1 is on"; the expression Not(1) says the opposite.
    const mm = m([{ id: 10, policy: "AnyOn", ocgs: [1], ve: { Not: { Group: 1 } } }]);
    expect(isOcShown(mm, off(), 10)).toBe(false);
    expect(isOcShown(mm, off(1), 10)).toBe(true);
  });

  it("evaluates And / Or / Not correctly", () => {
    const mk = (ve: Ocmd["ve"]) => m([{ id: 10, policy: "AnyOn", ocgs: [], ve }]);
    const or = mk({ Or: [{ Group: 1 }, { Not: { Group: 2 } }] });
    expect(isOcShown(or, off(1), 10)).toBe(false); // 1 off, 2 on -> false OR false
    expect(isOcShown(or, off(1, 2), 10)).toBe(true); // 2 off -> Not(2) true
    expect(isOcShown(or, off(), 10)).toBe(true);
    const and = mk({ And: [{ Group: 1 }, { Group: 2 }] });
    expect(isOcShown(and, off(), 10)).toBe(true);
    expect(isOcShown(and, off(2), 10)).toBe(false);
  });

  it("a runaway expression depth does not recurse without bound", () => {
    let ve: Ocmd["ve"] = { Group: 1 };
    for (let i = 0; i < 500; i++) ve = { Not: ve! };
    expect(() => isOcShown(m([{ id: 10, policy: "AnyOn", ocgs: [], ve }]), off(1), 10)).not.toThrow();
  });
});

describe("mark-up visibility", () => {
  const mm = model([layer(1), layer(2)]);
  it("a mark-up is shown unless its /OC reference is hidden", () => {
    const on1 = { optional_content: { Reference: [1, 0] as [number, number] } };
    const on2 = { optional_content: { Reference: [2, 0] as [number, number] } };
    expect(isMarkupShown(mm, off(1), on1)).toBe(false);
    expect(isMarkupShown(mm, off(1), on2)).toBe(true);
    expect(isMarkupShown(mm, off(1), {})).toBe(true);
    expect(isMarkupShown(mm, off(1), { optional_content: { Text: "A-DOOR" } })).toBe(true);
    expect(isMarkupShown(null, off(1), on1)).toBe(true);
  });
  it("markupOcRef ignores anything that is not a reference", () => {
    expect(markupOcRef({ optional_content: { Reference: [4, 0] } })).toBe(4);
    expect(markupOcRef({ optional_content: { Text: "x" } })).toBeNull();
    expect(markupOcRef({ optional_content: { Reference: ["a", 0] } })).toBeNull();
    expect(markupOcRef({ optional_content: null })).toBeNull();
    expect(markupOcRef({})).toBeNull();
  });
});

describe("changing the view", () => {
  const tree: LayerNode[] = [
    { kind: "Layer", id: 1, children: [{ kind: "Layer", id: 2, children: [] }, { kind: "Layer", id: 3, children: [] }] },
    { kind: "Group", label: "Folder", children: [{ kind: "Layer", id: 4, children: [] }] },
    { kind: "Layer", id: 5, children: [] },
  ];
  const layers = [layer(1), layer(2), layer(3), layer(4), layer(5, { locked: true })];
  const m = model(layers, tree);

  it("toggle flips one layer; with children the sub-layers follow", () => {
    expect(toggleLayer(m, off(), 2, false)).toEqual(off(2));
    expect(toggleLayer(m, off(2), 2, false)).toEqual(off());
    expect(toggleLayer(m, off(), 1, true)).toEqual(off(1, 2, 3));
    expect(toggleLayer(m, off(1, 2, 3), 1, true)).toEqual(off());
  });

  it("a locked layer and a design-intent layer never change", () => {
    expect(toggleLayer(m, off(), 5, false)).toEqual(off());
    const d = model([layer(1, { view_intent: false })]);
    expect(setGroups(d, off(), [1], false)).toEqual(off());
  });

  it("turning on a radio-button member turns the others off", () => {
    const rb = model([layer(1), layer(2), layer(3)], undefined, { rb_groups: [[1, 2, 3]] });
    expect(toggleLayer(rb, off(1, 2, 3), 2, false)).toEqual(off(1, 3));
    expect(toggleLayer(rb, off(3), 2, false)).toEqual(off(2, 3)); // 2 was on: turning it off touches nothing else
    expect(toggleLayer(rb, off(1), 1, false)).toEqual(off(2, 3)); // 1 on: the rest of its group goes off
    expect(toggleLayer(rb, off(), 1, false)).toEqual(off(1)); // turning off: no effect on the rest
  });

  it("isolate shows the layer and its sub-layers only, leaves locked layers as they are", () => {
    expect(isolate(m, off(), 1)).toEqual(off(4)); // 5 locked and on: unchanged, 4 hidden
    expect(isolate(m, off(5), 4)).toEqual(off(1, 2, 3, 5)); // locked 5 stays off
  });

  it("show all turns on every changeable layer", () => {
    expect(showAll(m, off(1, 2, 5))).toEqual(off(5));
  });

  it("subtreeIds and findNode walk groups and layers", () => {
    expect(subtreeIds(tree[0])).toEqual([1, 2, 3]);
    expect(subtreeIds(tree[1])).toEqual([4]);
    expect(findNode(tree, 4)?.kind).toBe("Layer");
    expect(findNode(tree, 99)).toBeNull();
  });
});

describe("rows", () => {
  const tree: LayerNode[] = [
    { kind: "Group", label: "Folder", children: [{ kind: "Layer", id: 4, children: [] }] },
    { kind: "Layer", id: 1, children: [{ kind: "Layer", id: 2, children: [] }] },
  ];
  const m = model([layer(1), layer(2), layer(4)], tree);

  it("flattens in display order with depth, and hides the children of a collapsed layer", () => {
    expect(flattenRows(m, new Set()).map((r) => [r.depth, r.kind, r.label])).toEqual([
      [0, "group", "Folder"], [1, "layer", "L4"], [0, "layer", "L1"], [1, "layer", "L2"],
    ]);
    expect(flattenRows(m, new Set([1])).map((r) => r.label)).toEqual(["Folder", "L4", "L1"]);
  });

  it("skips a tree entry whose layer does not exist", () => {
    const bad = model([layer(1)], [{ kind: "Layer", id: 9, children: [] }, { kind: "Layer", id: 1, children: [] }]);
    expect(flattenRows(bad, new Set()).map((r) => r.id)).toEqual([1]);
  });
});
