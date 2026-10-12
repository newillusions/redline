/**
 * Copies against the REAL Layers code (docs/arrange.md C16, docs/layers.md): a same-document
 * copy stays on its source's layer and hides with it; a paste into another document lands on no
 * layer of the source's file. Only the layer IPC is mocked; LayerStore, MarkupStore and the
 * arrange commands are the real ones.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";

const ipc = vi.hoisted(() => ({
  readLayers: vi.fn(), setLayerView: vi.fn(), addLayerToFile: vi.fn(), saveLayersDefault: vi.fn(),
}));
vi.mock("./layers", async (orig) => ({ ...(await orig<typeof import("./layers")>()), ...ipc }));

import type { Markup } from "./ipc";
import type { Layer, LayerModel } from "./layers";
import { dropLayerStore, layerStoreFor } from "./layers-store.svelte";
import { markupClipboard } from "./markup-clipboard.svelte";
import { copySelection, duplicateSelection, multiplySelection, pasteClipboard, type ArrangeCtx } from "./arrange-actions";
import { BY, NOW, mk, seqIds, storeWith } from "./arrange.test-util";

const layer = (id: number, name: string): Layer => ({ id, gen: 0, name, default_on: true, locked: false, view_intent: true, usage_view_state: null });
const model = (layers: Layer[]): LayerModel => ({
  layers, tree: layers.map((l) => ({ kind: "Layer" as const, id: l.id, children: [] })), ocmds: [], rb_groups: [],
  has_ocproperties: true, truncated: false, layers_truncated: false, unlisted: 0, view_supported: true,
  view_unsupported_reason: null, encrypted: false,
});
async function openLayers(docId: string, layers: Layer[]) {
  ipc.readLayers.mockResolvedValue({ status: "Loaded", model: model(layers) });
  dropLayerStore(docId);
  const s = layerStoreFor(docId);
  await s.load();
  return s;
}

const mint = seqIds("lay");
const ctxFor = (store: ReturnType<typeof storeWith>): ArrangeCtx => ({
  store, identity: BY, scales: [], pageIndex: 0, pageCount: 3, pageWidthPts: 600, pageHeightPts: 400,
  cursor: null, viewCenter: null, now: () => NOW, mintId: mint,
});
const onLayer1 = (id: string): Markup =>
  ({ ...mk(id, { layer: "Lighting", rect: [10, 100, 30, 120] }), optional_content: { Reference: [1, 0] } }) as unknown as Markup;

beforeEach(() => {
  Object.values(ipc).forEach((f) => f.mockReset());
  ipc.setLayerView.mockResolvedValue(undefined);
  markupClipboard.clear();
});

describe("copies and the Layers panel", () => {
  it("a Multiply / Duplicate / same-document paste stays on the source's layer and hides with it", async () => {
    const layers = await openLayers("d1", [layer(1, "Lighting"), layer(2, "Power")]);
    const store = storeWith([onLayer1("a"), mk("plain", { rect: [100, 100, 120, 120] })], "d1");
    store.selectedIds = new Set(["a"]);
    multiplySelection(ctxFor(store), { kind: "line", direction: "right", count: 2, pitch: 50 });
    store.selectedIds = new Set(["a"]);
    duplicateSelection(ctxFor(store));
    store.selectedIds = new Set(["a"]);
    copySelection(ctxFor(store));
    pasteClipboard(ctxFor(store), "inplace");
    const copies = store.markups.filter((m) => m.id !== "a" && m.id !== "plain");
    expect(copies).toHaveLength(4);
    for (const c of copies) {
      expect((c as unknown as { optional_content: unknown }).optional_content).toEqual({ Reference: [1, 0] });
      expect(c.layer).toBe("Lighting");
      expect(layers.isShown(c)).toBe(true);
    }
    await layers.toggle(1); // hide Lighting
    expect(copies.map((c) => layers.isShown(c))).toEqual([false, false, false, false]);
    expect(layers.isShown(store.getById("a")!)).toBe(false);
    expect(layers.isShown(store.getById("plain")!)).toBe(true);
  });

  it("a paste into another document lands on no layer of the source's file", async () => {
    await openLayers("d1", [layer(1, "Lighting")]);
    const dst = await openLayers("d2", [layer(1, "Other"), layer(2, "Power")]);
    await dst.toggle(1); // d2's own object 1 is hidden: a stray /OC reference would hide the paste
    const src = storeWith([onLayer1("a")], "d1");
    src.selectedIds = new Set(["a"]);
    copySelection(ctxFor(src));
    const target = storeWith([], "d2");
    pasteClipboard(ctxFor(target), "inplace");
    const pasted = target.markups[0] as unknown as { optional_content?: unknown; layer: string | null };
    expect(pasted.optional_content ?? null).toBeNull();
    expect(pasted.layer).toBeNull();
    expect(dst.isShown(target.markups[0])).toBe(true);
  });

  it("a paste into a document with a markup layer set joins that layer like any new mark-up", async () => {
    await openLayers("d1", [layer(1, "Lighting")]);
    const dst = await openLayers("d2", [layer(1, "Other"), layer(2, "Markups")]);
    dst.setMarkupLayer(2);
    const src = storeWith([onLayer1("a")], "d1");
    src.selectedIds = new Set(["a"]);
    copySelection(ctxFor(src));
    const target = storeWith([], "d2");
    pasteClipboard(ctxFor(target), "inplace");
    const pasted = target.markups[0] as unknown as { optional_content: unknown; layer: string };
    expect(pasted.optional_content).toEqual({ Reference: [2, 0] });
    expect(pasted.layer).toBe("Markups");
  });
});
