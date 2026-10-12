import { describe, it, expect, vi, beforeEach } from "vitest";

const ipc = vi.hoisted(() => ({
  readLayers: vi.fn(),
  setLayerView: vi.fn(),
  addLayerToFile: vi.fn(),
  saveLayersDefault: vi.fn(),
}));

vi.mock("./layers", async (orig) => ({ ...(await orig<typeof import("./layers")>()), ...ipc }));

import { LayerStore, dropLayerStore, layerStoreFor, onNewMarkup } from "./layers-store.svelte";
import type { Layer, LayerModel } from "./layers";

function layer(id: number, over: Partial<Layer> = {}): Layer {
  return { id, gen: 0, name: `L${id}`, default_on: true, locked: false, view_intent: true, usage_view_state: null, ...over };
}

function model(layers: Layer[], over: Partial<LayerModel> = {}): LayerModel {
  return {
    layers,
    tree: layers.map((l) => ({ kind: "Layer" as const, id: l.id, children: [] })),
    ocmds: [],
    rb_groups: [],
    has_ocproperties: true,
    truncated: false, layers_truncated: false,
    unlisted: 0,
    view_supported: true,
    view_unsupported_reason: null,
    encrypted: false,
    ...over,
  };
}

const sync = () => ({ flush: vi.fn(async () => {}), clearDirty: vi.fn() });

async function loaded(m: LayerModel, docId = "d1"): Promise<LayerStore> {
  ipc.readLayers.mockResolvedValue({ status: "Loaded", model: m });
  dropLayerStore(docId);
  const s = layerStoreFor(docId);
  await s.load();
  return s;
}

beforeEach(() => {
  Object.values(ipc).forEach((f) => f.mockReset());
  ipc.setLayerView.mockResolvedValue(undefined);
});

describe("LayerStore view", () => {
  it("starts in the file's own view and renders nothing until the user changes it", async () => {
    const s = await loaded(model([layer(1), layer(2, { default_on: false })]));
    expect([...s.off]).toEqual([2]);
    expect(s.isFileView).toBe(true);
    expect(ipc.setLayerView).not.toHaveBeenCalled();
  });

  it("hiding a layer renders the view; returning to the default releases the overlay (null)", async () => {
    const s = await loaded(model([layer(1), layer(2)]));
    await s.toggle(1);
    expect(ipc.setLayerView).toHaveBeenLastCalledWith("d1", [1]);
    expect(s.isFileView).toBe(false);
    expect(s.epoch).toBe(1);
    await s.toggle(1);
    expect(ipc.setLayerView).toHaveBeenLastCalledWith("d1", null);
    expect(s.isFileView).toBe(true);
    expect(s.epoch).toBe(2);
  });

  it("a failed render keeps the previous view and reports the error", async () => {
    const s = await loaded(model([layer(1), layer(2)]));
    ipc.setLayerView.mockRejectedValueOnce(new Error("layer views are not available: encrypted file"));
    await s.toggle(1);
    expect([...s.off]).toEqual([]);
    expect(s.viewError).toContain("encrypted file");
    expect(s.epoch).toBe(0);
    await s.toggle(1); // a later success clears the error
    expect(s.viewError).toBeNull();
    expect([...s.off]).toEqual([1]);
  });

  it("changes made while a render is in flight are applied after it, newest last", async () => {
    const s = await loaded(model([layer(1), layer(2), layer(3)]));
    let release: () => void = () => {};
    ipc.setLayerView.mockImplementationOnce(() => new Promise<void>((r) => (release = r)));
    const first = s.toggle(1);
    const second = s.toggle(2); // arrives while the first render is pending
    release();
    await first;
    await second;
    expect(ipc.setLayerView.mock.calls.map((c) => c[1])).toEqual([[1], [1, 2]]);
    expect([...s.off].sort()).toEqual([1, 2]);
  });

  it("isolate, show all and reset", async () => {
    const s = await loaded(model([layer(1), layer(2, { default_on: false }), layer(3)]));
    await s.isolateLayer(3);
    expect([...s.off].sort()).toEqual([1, 2]);
    await s.showAllLayers();
    expect([...s.off]).toEqual([]);
    await s.resetToFile();
    expect([...s.off]).toEqual([2]);
    expect(s.isFileView).toBe(true);
  });

  it("a file that cannot be overlaid reports it and does not allow changing the view", async () => {
    const s = await loaded(model([layer(1)], { view_supported: false, view_unsupported_reason: "encrypted file" }));
    expect(s.canChangeView).toBe(false);
  });

  it("a deferred (large) file reads nothing until asked", async () => {
    ipc.readLayers.mockResolvedValueOnce({ status: "Deferred", size: 90_000_000 });
    dropLayerStore("big");
    const s = layerStoreFor("big");
    await s.load(false);
    expect(s.status).toBe("deferred");
    expect(s.model).toBeNull();
    expect(ipc.readLayers).toHaveBeenCalledWith("big", false);
  });

  it("a read error is reported, not thrown", async () => {
    ipc.readLayers.mockRejectedValueOnce(new Error("could not read layers"));
    dropLayerStore("bad");
    const s = layerStoreFor("bad");
    await s.load();
    expect(s.status).toBe("error");
    expect(s.error).toBe("could not read layers");
  });
});

describe("mark-up visibility follows the view and nothing else does", () => {
  it("isShown hides a mark-up on a hidden layer; lists and counts read the mark-ups, not the view", async () => {
    const s = await loaded(model([layer(1), layer(2)]));
    const onOne = { optional_content: { Reference: [1, 0] as [number, number] } };
    expect(s.isShown(onOne)).toBe(true);
    await s.toggle(1);
    expect(s.isShown(onOne)).toBe(false);
    expect(s.isShown({})).toBe(true);
  });
});

describe("actions that write the file", () => {
  it("add layer flushes mark-ups first, clears dirty after, re-reads and keeps the view", async () => {
    const s = await loaded(model([layer(1), layer(2)]));
    await s.toggle(2);
    const sy = sync();
    ipc.addLayerToFile.mockResolvedValue(9);
    ipc.readLayers.mockResolvedValue({ status: "Loaded", model: model([layer(1), layer(2), layer(9, { name: "New" })]) });
    const id = await s.addLayer("New", null, sy);
    expect(id).toBe(9);
    expect(sy.flush.mock.invocationCallOrder[0]).toBeLessThan(ipc.addLayerToFile.mock.invocationCallOrder[0]);
    expect(ipc.addLayerToFile).toHaveBeenCalledWith("d1", "New", null);
    expect(sy.clearDirty).toHaveBeenCalledTimes(1);
    expect(s.model?.layers.map((l) => l.id)).toEqual([1, 2, 9]);
    expect([...s.off]).toEqual([2]);
    expect(s.busy).toBe(false);
  });

  it("a failed add leaves the mark-ups dirty and reports the error", async () => {
    const s = await loaded(model([layer(1)]));
    const sy = sync();
    ipc.addLayerToFile.mockRejectedValue(new Error("a layer needs a name"));
    expect(await s.addLayer(" ", null, sy)).toBeNull();
    expect(sy.clearDirty).not.toHaveBeenCalled();
    expect(s.viewError).toContain("needs a name");
  });

  it("save as default writes the hidden set and keeps the view", async () => {
    const s = await loaded(model([layer(1), layer(2)]));
    await s.toggle(1);
    const sy = sync();
    ipc.saveLayersDefault.mockResolvedValue(undefined);
    ipc.readLayers.mockResolvedValue({ status: "Loaded", model: model([layer(1, { default_on: false }), layer(2)]) });
    expect(await s.saveAsDefault(sy)).toBe(true);
    expect(ipc.saveLayersDefault).toHaveBeenCalledWith("d1", [1]);
    expect(sy.clearDirty).toHaveBeenCalled();
    expect(s.isFileView).toBe(true); // the view now equals the file's default
  });
});

describe("new mark-ups go on the mark-up layer", () => {
  const base = { layer: null as string | null, optional_content: undefined as unknown };

  it("assigns the layer's reference and name, and shows the layer if it was hidden", async () => {
    const s = await loaded(model([layer(1), layer(2, { name: "Lighting" })]));
    await s.toggle(2);
    s.setMarkupLayer(2);
    const out = onNewMarkup("d1", { ...base });
    expect(out.optional_content).toEqual({ Reference: [2, 0] });
    expect(out.layer).toBe("Lighting");
    await vi.waitFor(() => expect([...s.off]).toEqual([]));
  });

  it("leaves a mark-up that already has a layer, and does nothing without a mark-up layer", async () => {
    const s = await loaded(model([layer(1)]));
    expect(onNewMarkup("d1", { ...base })).toEqual(base);
    s.setMarkupLayer(1);
    const own = { layer: "A-DOOR", optional_content: { Text: "A-DOOR" } };
    expect(onNewMarkup("d1", own)).toBe(own);
    expect(onNewMarkup("other-doc", { ...base })).toEqual(base);
  });

  it("forgets a mark-up layer that no longer exists after a re-read", async () => {
    const s = await loaded(model([layer(1), layer(2)]));
    s.setMarkupLayer(2);
    ipc.readLayers.mockResolvedValue({ status: "Loaded", model: model([layer(1)]) });
    await s.load();
    expect(s.markupLayerId).toBeNull();
  });
});

describe("reset after the file was replaced", () => {
  it("drops the view and the parsed layers and releases the overlay", async () => {
    const s = await loaded(model([layer(1), layer(2)]));
    await s.toggle(1);
    ipc.setLayerView.mockClear();
    await s.reset();
    expect(ipc.setLayerView).toHaveBeenCalledWith("d1", null);
    expect(s.model).toBeNull();
    expect(s.status).toBe("idle");
    expect([...s.off]).toEqual([]);
    ipc.readLayers.mockResolvedValue({ status: "Loaded", model: model([layer(7)]) });
    await s.load();
    expect(s.model?.layers.map((l) => l.id)).toEqual([7]);
  });

  it("does not throw when there is no document to release", async () => {
    const s = await loaded(model([layer(1)]));
    ipc.setLayerView.mockRejectedValueOnce(new Error("unknown doc_id"));
    await expect(s.reset()).resolves.toBeUndefined();
  });
});
