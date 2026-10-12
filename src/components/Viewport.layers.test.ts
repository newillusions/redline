// @vitest-environment jsdom
/**
 * Layer visibility in the real Viewport (docs/layers.md): mark-ups on a hidden layer are not
 * drawn, selected or hit-tested; the tiles are refetched when the rendered view changes; and
 * the Markups List still lists every mark-up (visibility is display-only).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import Viewport from "./Viewport.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { buildMarkup } from "$lib/markup-tools";
import { buildRows } from "$lib/markups-list";
import { legendFromCountSets } from "$lib/legend";
import { mk } from "$lib/markups-list.test-util";
import type { CountSet, Markup } from "$lib/ipc";
import { dropLayerStore, layerStoreFor } from "$lib/layers-store.svelte";
import type { Layer, LayerModel } from "$lib/layers";

if (typeof PointerEvent === "undefined") {
  (globalThis as Record<string, unknown>).PointerEvent = class PointerEvent extends MouseEvent {
    pointerId: number;
    constructor(type: string, init: PointerEventInit = {}) {
      super(type, init);
      this.pointerId = init.pointerId ?? 1;
    }
  };
}

vi.mock("$lib/ipc", () => ({
  getPageSize: vi.fn(),
  renderTile: vi.fn(),
  processRssMb: vi.fn(),
  getUserIdentity: vi.fn(),
  openDocument: vi.fn(),
  closeDocument: vi.fn(),
  addMarkup: vi.fn(),
  listMarkups: vi.fn(),
  loadMarkups: vi.fn(),
  saveDocument: vi.fn(),
  saveDocumentAs: vi.fn(),
  updateMarkup: vi.fn(),
  deleteMarkup: vi.fn(),
  isMarkupLocked: (m: { annot_flags?: number }) => ((m.annot_flags ?? 0) & 0x80) !== 0,
  isMarkupContentsLocked: (m: { annot_flags?: number }) => ((m.annot_flags ?? 0) & 0x200) !== 0,
  addScale: vi.fn(async () => null),
  listScales: vi.fn(async () => []),
  deleteScale: vi.fn(async () => true),
  exportMarkupList: vi.fn(async () => {}),
}));

const layerIpc = vi.hoisted(() => ({ readLayers: vi.fn(), setLayerView: vi.fn() }));
vi.mock("$lib/layers", async (orig) => ({ ...(await orig<typeof import("$lib/layers")>()), ...layerIpc }));

import * as ipcMocks from "$lib/ipc";

const FAKE_DOC = { doc_id: "d1", path: "/fake.pdf", page_count: 1, was_encrypted: false };
const FAKE_IDENTITY = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "T" };
const FAKE_PAGE_SIZE = { doc_id: "d1", page_index: 0, width_pts: 200, height_pts: 200 };
const AP = { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid" as const, font: null };

function fakeIpc() {
  return { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) };
}

function stubResizeObserver(): (w: number, h: number) => void {
  let cb: ResizeObserverCallback | null = null;
  let target: Element | null = null;
  (globalThis as Record<string, unknown>).ResizeObserver = class {
    constructor(c: ResizeObserverCallback) { cb = c; }
    observe(el: Element) { target = el; }
    unobserve() {}
    disconnect() {}
  };
  return (w, h) => {
    if (cb && target) {
      cb([{
        contentRect: { width: w, height: h, top: 0, left: 0, bottom: h, right: w, x: 0, y: 0, toJSON() { return {}; } } as DOMRectReadOnly,
        target, borderBoxSize: [], contentBoxSize: [], devicePixelContentBoxSize: [],
      }], {} as ResizeObserver);
    }
  };
}

function ptr(target: Element, type: string, x: number, y: number) {
  fireEvent(target, new PointerEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, pointerId: 1 }));
}

async function mountViewport(store: MarkupStore) {
  const triggerResize = stubResizeObserver();
  Element.prototype.setPointerCapture = vi.fn();
  const { container } = render(Viewport, { props: { docInfo: FAKE_DOC, store } });
  await waitFor(() => expect(vi.mocked(ipcMocks.getUserIdentity)).toHaveBeenCalled());
  const containerEl = container.querySelector(".viewport-root") as HTMLElement;
  vi.spyOn(containerEl, "getBoundingClientRect").mockReturnValue({
    left: 0, top: 0, right: 200, bottom: 200, width: 200, height: 200, x: 0, y: 0, toJSON() { return {}; },
  } as DOMRect);
  triggerResize(200, 200);
  const overlay = container.querySelector("svg.markup-overlay") as SVGElement;
  return { container, overlay };
}

function layer(id: number, over: Partial<Layer> = {}): Layer {
  return { id, gen: 0, name: `L${id}`, default_on: true, locked: false, view_intent: true, usage_view_state: null, ...over };
}

const MODEL: LayerModel = {
  layers: [layer(1, { name: "Lighting" }), layer(2, { name: "Ceiling" })],
  tree: [{ kind: "Layer", id: 1, children: [] }, { kind: "Layer", id: 2, children: [] }],
  ocmds: [],
  rb_groups: [],
  has_ocproperties: true,
  truncated: false, layers_truncated: false,
  unlisted: 0,
  view_supported: true,
  view_unsupported_reason: null,
  encrypted: false,
};

/** Rect A (no layer), B (layer 1), C (layer 2); disjoint, so each can be clicked alone.
 *  Screen (zoom 1, page 200x200): A centre (30,30), B centre (100,100), C centre (170,170). */
function seed(store: MarkupStore) {
  const rect = (id: string, minX: number, minY: number, maxX: number, maxY: number, layerId?: number) => {
    const m = buildMarkup({
      markupType: "Rectangle", page: 0,
      geometry: { Rect: { min: { x: minX, y: minY }, max: { x: maxX, y: maxY } } },
      appearance: AP, identity: FAKE_IDENTITY, now: "2026-01-01T00:00:00Z", id,
    });
    if (layerId) m.optional_content = { Reference: [layerId, 0] };
    return m;
  };
  store.markups.push(rect("a", 20, 160, 40, 180), rect("b", 90, 90, 110, 110, 1), rect("c", 160, 20, 180, 40, 2));
}

const rects = (c: HTMLElement) => c.querySelectorAll("svg.markup-overlay rect").length;

describe("Viewport honours the layer view", () => {
  let store: MarkupStore;

  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(FAKE_PAGE_SIZE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0, width_px: 512, height_px: 512,
      zoom: 1, dpr: 1, png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);
    layerIpc.readLayers.mockReset();
    layerIpc.setLayerView.mockReset();
    layerIpc.readLayers.mockResolvedValue({ status: "Loaded", model: MODEL });
    layerIpc.setLayerView.mockResolvedValue(undefined);
    // jsdom has no canvas: give the tile painter a context that accepts every call.
    const ctx = new Proxy({}, { get: (_t, k) => (k === "canvas" ? document.createElement("canvas") : () => undefined), set: () => true });
    HTMLCanvasElement.prototype.getContext = (() => ctx) as unknown as typeof HTMLCanvasElement.prototype.getContext;
    dropLayerStore("d1");
    store = new MarkupStore("d1", fakeIpc());
    seed(store);
  });

  afterEach(() => vi.restoreAllMocks());

  it("draws every mark-up while all layers are shown, and drops those on a hidden layer", async () => {
    const { container } = await mountViewport(store);
    await waitFor(() => expect(layerStoreFor("d1").status).toBe("loaded"));
    await waitFor(() => expect(rects(container)).toBe(3));
    await layerStoreFor("d1").toggle(1);
    await waitFor(() => expect(rects(container)).toBe(2));
    await layerStoreFor("d1").toggle(1);
    await waitFor(() => expect(rects(container)).toBe(3));
  });

  it("a mark-up on a layer the file hides by default is not drawn at open", async () => {
    layerIpc.readLayers.mockResolvedValue({
      status: "Loaded",
      model: { ...MODEL, layers: [layer(1, { default_on: false }), layer(2)] },
    });
    const { container } = await mountViewport(store);
    await waitFor(() => expect(layerStoreFor("d1").status).toBe("loaded"));
    await waitFor(() => expect(rects(container)).toBe(2));
  });

  it("a hidden mark-up cannot be clicked or marquee-selected; a shown one can", async () => {
    const { overlay } = await mountViewport(store);
    await waitFor(() => expect(layerStoreFor("d1").status).toBe("loaded"));
    await layerStoreFor("d1").toggle(1);
    store.activeTool = "select";
    await tick();

    ptr(overlay, "pointerdown", 100, 100); // B's centre: hidden
    ptr(overlay, "pointerup", 100, 100);
    await tick();
    expect(store.selectedIds.has("b")).toBe(false);

    ptr(overlay, "pointerdown", 30, 30); // A: shown
    ptr(overlay, "pointerup", 30, 30);
    await tick();
    expect(store.selectedIds.has("a")).toBe(true);

    ptr(overlay, "pointerdown", 5, 5); // marquee over everything
    ptr(overlay, "pointermove", 195, 195);
    ptr(overlay, "pointerup", 195, 195);
    await tick();
    expect([...store.selectedIds].sort()).toEqual(["a", "c"]);
  });

  it("refetches the tiles when the rendered view changes", async () => {
    await mountViewport(store);
    await waitFor(() => expect(layerStoreFor("d1").status).toBe("loaded"));
    await waitFor(() => expect(vi.mocked(ipcMocks.renderTile)).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 20));
    const before = vi.mocked(ipcMocks.renderTile).mock.calls.length;
    await layerStoreFor("d1").toggle(2);
    await waitFor(() => expect(vi.mocked(ipcMocks.renderTile).mock.calls.length).toBeGreaterThan(before));
  });

  it("visibility never changes the Markups List: every mark-up is still a row", async () => {
    await mountViewport(store);
    await waitFor(() => expect(layerStoreFor("d1").status).toBe("loaded"));
    const before = buildRows(store.markups).map((r) => r.id);
    await layerStoreFor("d1").toggle(1);
    await layerStoreFor("d1").toggle(2);
    expect(buildRows(store.markups).map((r) => r.id)).toEqual(before);
    expect(before).toEqual(["a", "b", "c"]);
  });

  it("a legend still counts mark-ups on a hidden layer: the table text does not change", async () => {
    const SET: CountSet = { id: "set-a", name: "Downlight", color: "#ff0000", symbol: "Square" };
    const count = (id: string, n: number, layerId?: number): Markup => mk(id, {
      markup_type: "MeasurementCount",
      geometry: { Point: { x: 150, y: 50 } },
      appearance: { color: SET.color, line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
      count_set: SET,
      measurement: { scale_ref: null, raw_measure: n, unit: "ea", computed_quantity: n, depth: null, count_value: n, custom_columns: {}, rise_drop: [] },
      optional_content: layerId ? { Reference: [layerId, 0] } : null,
    } as Partial<Markup>);
    const lg = legendFromCountSets([SET], { page: 0, topLeft: { x: 10, y: 190 }, identity: FAKE_IDENTITY, now: "t", id: "L" });
    store = new MarkupStore("d1", fakeIpc());
    store.seed([lg, count("c1", 3, 1), count("c2", 4)]);
    const { container } = await mountViewport(store);
    await waitFor(() => expect(layerStoreFor("d1").status).toBe("loaded"));
    const texts = () => [...container.querySelectorAll("svg.markup-overlay text")].map((t) => t.textContent);
    await waitFor(() => expect(texts()).toContain("7"));
    const polygons = () => container.querySelectorAll("svg.markup-overlay polygon").length;
    const before = polygons();
    await layerStoreFor("d1").toggle(1);
    await waitFor(() => expect(polygons()).toBeLessThan(before)); // the marker on the hidden layer is gone...
    expect(texts()).toContain("7"); // ...and the table still counts it
  });
});
