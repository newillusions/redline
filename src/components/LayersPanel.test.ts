// @vitest-environment jsdom
/**
 * LayersPanel over a real LayerStore: lists the layer tree, hides/shows by checkbox, respects
 * locks, "Show only", the file-writing actions ("New layer", "Save as default") and the
 * large-file / unsupported / error states. IPC is mocked at the `$lib/layers` boundary.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";

const ipc = vi.hoisted(() => ({
  readLayers: vi.fn(),
  setLayerView: vi.fn(),
  addLayerToFile: vi.fn(),
  saveLayersDefault: vi.fn(),
}));
vi.mock("$lib/layers", async (orig) => ({ ...(await orig<typeof import("$lib/layers")>()), ...ipc }));

import LayersPanel from "./LayersPanel.svelte";
import { dropLayerStore, layerStoreFor } from "$lib/layers-store.svelte";
import type { Layer, LayerModel, LayerNode } from "$lib/layers";

function layer(id: number, over: Partial<Layer> = {}): Layer {
  return { id, gen: 0, name: `Layer ${id}`, default_on: true, locked: false, view_intent: true, usage_view_state: null, ...over };
}

function model(layers: Layer[], tree?: LayerNode[], over: Partial<LayerModel> = {}): LayerModel {
  return {
    layers,
    tree: tree ?? layers.map((l) => ({ kind: "Layer" as const, id: l.id, children: [] })),
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

async function mount(m: LayerModel | { status: string; size?: number }, s = sync()) {
  ipc.readLayers.mockResolvedValue("layers" in m ? { status: "Loaded", model: m } : m);
  dropLayerStore("d1");
  const view = render(LayersPanel, { docId: "d1", sync: s });
  await waitFor(() => expect(layerStoreFor("d1").status).not.toBe("idle"));
  await waitFor(() => expect(layerStoreFor("d1").status).not.toBe("loading"));
  return { ...view, sync: s };
}

beforeEach(() => {
  Object.values(ipc).forEach((f) => f.mockReset());
  ipc.setLayerView.mockResolvedValue(undefined);
});

describe("LayersPanel", () => {
  it("lists layers in /Order with groups and sub-layers, and a checkbox each", async () => {
    const tree: LayerNode[] = [
      { kind: "Group", label: "Discipline", children: [{ kind: "Layer", id: 1, children: [{ kind: "Layer", id: 2, children: [] }] }] },
      { kind: "Layer", id: 3, children: [] },
    ];
    await mount(model([layer(1, { name: "Lighting" }), layer(2, { name: "Emergency" }), layer(3, { name: "Ceiling" })], tree));
    expect(screen.getByText("Discipline")).toBeInTheDocument();
    const rows = screen.getAllByTestId("layer-row");
    expect(rows.map((r) => r.getAttribute("aria-level"))).toEqual(["2", "3", "1"]);
    expect(screen.getByLabelText("Show layer Lighting")).toBeChecked();
  });

  it("unchecking a layer hides it in the view and re-checking restores the file's view", async () => {
    await mount(model([layer(1, { name: "Lighting" }), layer(2, { name: "Ceiling" })]));
    const box = screen.getByLabelText("Show layer Lighting");
    await fireEvent.click(box);
    await waitFor(() => expect(ipc.setLayerView).toHaveBeenLastCalledWith("d1", [1]));
    await waitFor(() => expect(screen.getByLabelText("Show layer Lighting")).not.toBeChecked());
    await fireEvent.click(screen.getByLabelText("Show layer Lighting"));
    await waitFor(() => expect(ipc.setLayerView).toHaveBeenLastCalledWith("d1", null));
    await waitFor(() => expect(screen.getByLabelText("Show layer Lighting")).toBeChecked());
  });

  it("a layer the render failed to hide stays checked and the reason is shown", async () => {
    await mount(model([layer(1, { name: "Lighting" })]));
    ipc.setLayerView.mockRejectedValueOnce(new Error("layer views are not available: file changed"));
    await fireEvent.click(screen.getByLabelText("Show layer Lighting"));
    expect(await screen.findByTestId("layers-error")).toHaveTextContent("file changed");
    expect(screen.getByLabelText("Show layer Lighting")).toBeChecked();
  });

  it("a locked layer cannot be toggled and says so", async () => {
    await mount(model([layer(1, { name: "Base", locked: true })]));
    expect(screen.getByLabelText("Show layer Base")).toBeDisabled();
    expect(screen.getByLabelText("locked")).toBeInTheDocument();
  });

  it("'Show only' hides every other layer; Reset returns to the file's view", async () => {
    await mount(model([layer(1, { name: "A" }), layer(2, { name: "B" }), layer(3, { name: "C" })]));
    expect(screen.getByTestId("layers-isolate")).toBeDisabled(); // nothing selected
    await fireEvent.click(screen.getAllByTestId("layer-name")[1]);
    await fireEvent.click(screen.getByTestId("layers-isolate"));
    await waitFor(() => expect(ipc.setLayerView).toHaveBeenLastCalledWith("d1", [1, 3]));
    await fireEvent.click(screen.getByTestId("layers-reset"));
    await waitFor(() => expect(ipc.setLayerView).toHaveBeenLastCalledWith("d1", null));
  });

  it("a file whose view cannot be rendered lists the layers but disables the toggles", async () => {
    await mount(model([layer(1, { name: "A" })], undefined, { view_supported: false, view_unsupported_reason: "encrypted file" }));
    expect(screen.getByLabelText("Show layer A")).toBeDisabled();
    expect(screen.getByTestId("layers-unsupported")).toHaveTextContent("encrypted file");
  });

  it("a design-intent layer is listed but its toggle is disabled", async () => {
    await mount(model([layer(1, { name: "Grid", view_intent: false })]));
    expect(screen.getByLabelText("Show layer Grid")).toBeDisabled();
    expect(screen.getByText("design")).toBeInTheDocument();
  });

  it("'New layer' flushes mark-ups, adds the layer, and clears the dirty flag", async () => {
    const { sync: s } = await mount(model([layer(1, { name: "A" })]));
    ipc.addLayerToFile.mockResolvedValue(7);
    ipc.readLayers.mockResolvedValue({ status: "Loaded", model: model([layer(1, { name: "A" }), layer(7, { name: "Markups" })]) });
    await fireEvent.click(screen.getByTestId("layers-add"));
    await fireEvent.input(screen.getByTestId("layers-new-name"), { target: { value: "Markups" } });
    await fireEvent.click(screen.getByTestId("layers-new-ok"));
    await waitFor(() => expect(ipc.addLayerToFile).toHaveBeenCalledWith("d1", "Markups", null));
    await waitFor(() => expect(screen.getByLabelText("Show layer Markups")).toBeInTheDocument());
    expect(s.flush).toHaveBeenCalled();
    expect(s.clearDirty).toHaveBeenCalled();
  });

  it("'Add child' adds under the selected layer", async () => {
    await mount(model([layer(1, { name: "A" })]));
    ipc.addLayerToFile.mockResolvedValue(8);
    await fireEvent.click(screen.getAllByTestId("layer-name")[0]);
    await fireEvent.click(screen.getByTestId("layers-add"));
    await fireEvent.input(screen.getByTestId("layers-new-name"), { target: { value: "Sub" } });
    await fireEvent.click(screen.getByTestId("layers-new-child"));
    await waitFor(() => expect(ipc.addLayerToFile).toHaveBeenCalledWith("d1", "Sub", 1));
  });

  it("'Save as default' is only offered when the view differs from the file's, and writes the hidden set", async () => {
    await mount(model([layer(1, { name: "A" }), layer(2, { name: "B" })]));
    expect(screen.getByTestId("layers-save-default")).toBeDisabled();
    await fireEvent.click(screen.getByLabelText("Show layer B"));
    await waitFor(() => expect(screen.getByTestId("layers-save-default")).toBeEnabled());
    ipc.saveLayersDefault.mockResolvedValue(undefined);
    await fireEvent.click(screen.getByTestId("layers-save-default"));
    await waitFor(() => expect(ipc.saveLayersDefault).toHaveBeenCalledWith("d1", [2]));
  });

  it("marks the layer new mark-ups go on, and can clear it", async () => {
    await mount(model([layer(1, { name: "A" }), layer(2, { name: "B" })]));
    await fireEvent.click(screen.getAllByTestId("layer-name")[1]);
    await fireEvent.click(screen.getByTestId("layers-markup-layer"));
    expect(screen.getByTestId("markup-layer-badge")).toBeInTheDocument();
    expect(layerStoreFor("d1").markupLayerId).toBe(2);
    await fireEvent.click(screen.getByTestId("layers-markup-layer"));
    expect(screen.queryByTestId("markup-layer-badge")).toBeNull();
  });

  it("a large file waits for the user, then reads", async () => {
    await mount({ status: "Deferred", size: 80 * 1024 * 1024 });
    expect(screen.getByText(/80 MB/)).toBeInTheDocument();
    ipc.readLayers.mockResolvedValue({ status: "Loaded", model: model([layer(1, { name: "A" })]) });
    await fireEvent.click(screen.getByTestId("layers-load"));
    await waitFor(() => expect(screen.getByLabelText("Show layer A")).toBeInTheDocument());
    expect(ipc.readLayers).toHaveBeenLastCalledWith("d1", true);
  });

  it("a file with no layers says so and still offers 'New layer'", async () => {
    await mount(model([], undefined, { has_ocproperties: false }));
    expect(screen.getByText("This file has no layers.")).toBeInTheDocument();
    expect(screen.getByTestId("layers-add")).toBeEnabled();
    expect(screen.queryByTestId("layers-show-all")).toBeNull();
  });

  it("layer names are text: markup in a name is not interpreted", async () => {
    await mount(model([layer(1, { name: "<img src=x onerror=alert(1)>" })]));
    expect(document.querySelector("img")).toBeNull();
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
  });
});
