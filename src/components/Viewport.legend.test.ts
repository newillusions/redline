// @vitest-environment jsdom
/**
 * Legends drawn by the real Viewport.svelte over a real MarkupStore (docs/legends.md): the
 * table shows the computed rows, follows added/edited/deleted mark-ups without a manual
 * refresh, and its stored rectangle is kept equal to the table that is drawn.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import Viewport from "./Viewport.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { legendFromCountSets } from "$lib/legend";
import { mk } from "$lib/markups-list.test-util";
import type { CountSet, Markup } from "$lib/ipc";

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

import * as ipcMocks from "$lib/ipc";



/** Let queued promise callbacks (mocked IPC replies) run. */
const settle = () => new Promise<void>((r) => setTimeout(r, 0));

const FAKE_DOC = { doc_id: "d1", path: "/fake.pdf", page_count: 1, was_encrypted: false };
const FAKE_IDENTITY = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "T" };
const FAKE_PAGE_SIZE = { doc_id: "d1", page_index: 0, width_pts: 400, height_pts: 400 };

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

async function mountViewport(store: MarkupStore) {
  const triggerResize = stubResizeObserver();
  Element.prototype.setPointerCapture = vi.fn();
  const { container } = render(Viewport, { props: { docInfo: FAKE_DOC, store } });
  await waitFor(() => expect(vi.mocked(ipcMocks.getUserIdentity)).toHaveBeenCalled());
  const containerEl = container.querySelector(".viewport-root") as HTMLElement;
  vi.spyOn(containerEl, "getBoundingClientRect").mockReturnValue({
    left: 0, top: 0, right: 400, bottom: 400, width: 400, height: 400, x: 0, y: 0, toJSON() { return {}; },
  } as DOMRect);
  triggerResize(400, 400);
  const overlay = container.querySelector("svg.markup-overlay") as SVGElement;
  return { container, overlay };
}

const SET: CountSet = { id: "set-a", name: "Downlight", color: "#ff0000", symbol: "Square" };

function count(id: string, n: number): Markup {
  return mk(id, {
    markup_type: "MeasurementCount",
    geometry: { Point: { x: 300, y: 50 } },
    appearance: { color: SET.color, line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
    count_set: SET,
    measurement: { scale_ref: null, raw_measure: n, unit: "ea", computed_quantity: n, depth: null, count_value: n, custom_columns: {}, rise_drop: [] },
  } as Partial<Markup>);
}

function legend(): Markup {
  // A stale rectangle (four rows tall) on purpose: the viewport must fit it to the one row shown.
  const l = legendFromCountSets([SET], { page: 0, topLeft: { x: 10, y: 380 }, identity: FAKE_IDENTITY, now: "t", id: "L" });
  return { ...l, geometry: { Rect: { min: { x: 10, y: 200 }, max: { x: 310, y: 380 } } } } as Markup;
}

const texts = (c: HTMLElement) => [...c.querySelectorAll("svg.markup-overlay text")].map((t) => t.textContent);

describe("Legend in the Viewport", () => {
  let store: MarkupStore;
  let ipc: ReturnType<typeof fakeIpc>;

  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(FAKE_PAGE_SIZE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0, width_px: 512, height_px: 512,
      zoom: 1, dpr: 1, png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);
    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
    store.seed([legend(), count("c1", 3), count("c2", 4)]);
  });

  afterEach(() => vi.restoreAllMocks());

  it("draws the title, the row and its computed quantity as text", async () => {
    const { container } = await mountViewport(store);
    await waitFor(() => expect(texts(container)).toContain("Downlight"));
    const t = texts(container);
    expect(t).toContain("Count legend");
    expect(t).toContain("7");
    expect(t).toContain("ea");
    expect(container.querySelector("svg.markup-overlay polygon")).not.toBeNull(); // the Square symbol
  });

  it("fits the stored rectangle to the table without dirtying the freshly opened document", async () => {
    await mountViewport(store);
    await waitFor(() => {
      const g = store.getById("L")!.geometry;
      expect("Rect" in g && g.Rect.min.y).toBeGreaterThan(300); // one row tall, not four
    });
    const g = store.getById("L")!.geometry;
    expect("Rect" in g && g.Rect.max.y).toBe(380);
    await settle();
    expect(ipc.update).not.toHaveBeenCalled();
    expect(store.dirty).toBe(false);
    expect(store.canUndo).toBe(false);
  });

  it("follows an added, edited and deleted count without any manual refresh", async () => {
    const { container } = await mountViewport(store);
    await waitFor(() => expect(texts(container)).toContain("7"));
    store.create(count("c3", 5));
    await waitFor(() => expect(texts(container)).toContain("12"));
    const c1 = store.getById("c1")!;
    store.update(c1, { ...c1, measurement: { ...c1.measurement!, count_value: 10 } });
    await waitFor(() => expect(texts(container)).toContain("19"));
    store.delete("c2");
    await waitFor(() => expect(texts(container)).toContain("15"));
    store.undo(); // bring c2 back
    await waitFor(() => expect(texts(container)).toContain("19"));
  });

  it("a reload (the store re-seeded from the changed file) re-syncs the legend table and rectangle", async () => {
    const { container } = await mountViewport(store);
    await waitFor(() => expect(texts(container)).toContain("7"));
    // What "reload and keep my mark-ups" does: seed the store with what the file now holds.
    store.seed([legend(), count("c1", 1), count("c2", 1), count("c9", 2)]);
    await waitFor(() => expect(texts(container)).toContain("4"));
    const g = store.getById("L")!.geometry;
    expect("Rect" in g && g.Rect.min.y).toBeGreaterThan(300); // refit again, one row tall
    expect(store.dirty).toBe(false);
  });

  it("renders hostile mark-up text as text, not HTML", async () => {
    const evil = count("evil", 1);
    evil.subject = '<img src=x onerror="window.__legendpwn=1">';
    store.create(evil);
    const { container } = await mountViewport(store);
    await settle();
    expect(container.querySelector("img")).toBeNull();
    expect((window as unknown as { __legendpwn?: number }).__legendpwn).toBeUndefined();
  });
});
