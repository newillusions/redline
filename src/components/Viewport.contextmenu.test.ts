// @vitest-environment jsdom
/**
 * Viewport right-click menu guards (PR #163 review): the real Viewport with a real MarkupStore. Coordinates as in Viewport.interaction.test.ts: zoom 1,
 * scroll 0, 200x200 pt page, so screen (x, y) is PDF (x, 200 - y).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, waitFor, fireEvent, screen, cleanup } from "@testing-library/svelte";
import Viewport from "./Viewport.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { markupClipboard } from "$lib/markup-clipboard.svelte";
import { mk } from "$lib/arrange.test-util";

vi.mock("$lib/ipc", () => ({
  getPageSize: vi.fn(), renderTile: vi.fn(), processRssMb: vi.fn(), getUserIdentity: vi.fn(),
  openDocument: vi.fn(), closeDocument: vi.fn(), addMarkup: vi.fn(), listMarkups: vi.fn(), loadMarkups: vi.fn(),
  saveDocument: vi.fn(), saveDocumentAs: vi.fn(), updateMarkup: vi.fn(), deleteMarkup: vi.fn(),
  isMarkupLocked: (m: { annot_flags?: number }) => ((m.annot_flags ?? 0) & 0x80) !== 0,
  isMarkupContentsLocked: (m: { annot_flags?: number }) => ((m.annot_flags ?? 0) & 0x200) !== 0,
  addScale: vi.fn(), listScales: vi.fn(async () => []), deleteScale: vi.fn(), exportMarkupList: vi.fn(),
}));
import * as ipcMocks from "$lib/ipc";

if (typeof PointerEvent === "undefined") {
  (globalThis as Record<string, unknown>).PointerEvent = class PointerEvent extends MouseEvent {
    pointerId: number;
    constructor(type: string, init: PointerEventInit = {}) { super(type, init); this.pointerId = init.pointerId ?? 1; }
  };
}

const FAKE_DOC = { doc_id: "d1", path: "/fake.pdf", page_count: 3, was_encrypted: false };
const IDENTITY = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "T" };

async function mountViewport(store: MarkupStore) {
  let cb: ResizeObserverCallback | null = null;
  let target: Element | null = null;
  (globalThis as Record<string, unknown>).ResizeObserver = class {
    constructor(c: ResizeObserverCallback) { cb = c; }
    observe(el: Element) { target = el; }
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.setPointerCapture = vi.fn();
  const { container } = render(Viewport, { props: { docInfo: FAKE_DOC, store } });
  await waitFor(() => expect(vi.mocked(ipcMocks.getUserIdentity)).toHaveBeenCalled());
  const root = container.querySelector(".viewport-root") as HTMLElement;
  vi.spyOn(root, "getBoundingClientRect").mockReturnValue({
    left: 0, top: 0, right: 200, bottom: 200, width: 200, height: 200, x: 0, y: 0, toJSON() { return {}; },
  } as DOMRect);
  const r = { contentRect: { width: 200, height: 200 } } as ResizeObserverEntry;
  (cb as ResizeObserverCallback | null)?.([{ ...r, target: target! }], {} as ResizeObserver);
  // The identity resolves asynchronously after getUserIdentity was called.
  await new Promise((res) => setTimeout(res, 0));
  return { container, root };
}


const ptr = (target: Element, type: string, x: number, y: number, button = 0, extra: PointerEventInit = {}) =>
  fireEvent(target, new PointerEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, pointerId: 1, button, ...extra }));

describe("Viewport context menu guards", () => {
  let store: MarkupStore;
  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue({ doc_id: "d1", page_index: 0, width_pts: 200, height_pts: 200 });
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0, width_px: 512, height_px: 512, zoom: 1, dpr: 1, png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(IDENTITY);
    markupClipboard.clear();
    store = new MarkupStore("d1", { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) });
    store.seed([mk("a", { rect: [10, 10, 30, 30] }), mk("b", { rect: [60, 60, 80, 80] })]);
    store.activeTool = "select";
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("a right-button drag on a markup under Select moves nothing; a left drag does (control)", async () => {
    const { container } = await mountViewport(store);
    const overlay = container.querySelector("svg.markup-overlay") as SVGElement;
    const before = JSON.stringify(store.getById("a")!.geometry);
    // Markup a spans screen x 10-30, y 170-190.
    ptr(overlay, "pointerdown", 20, 180, 2, { buttons: 2 });
    ptr(overlay, "pointermove", 70, 130, 2, { buttons: 2 });
    ptr(overlay, "pointerup", 70, 130, 2);
    expect(JSON.stringify(store.getById("a")!.geometry)).toBe(before);
    expect(store.getById("a")!.audit.revision).toBe(7);

    ptr(overlay, "pointerdown", 20, 180, 0, { buttons: 1 });
    ptr(overlay, "pointermove", 70, 130, 0, { buttons: 1 });
    ptr(overlay, "pointerup", 70, 130, 0);
    expect(JSON.stringify(store.getById("a")!.geometry)).not.toBe(before);
  });

  it("a right-button press under a drawing tool starts no markup", async () => {
    const { container } = await mountViewport(store);
    const overlay = container.querySelector("svg.markup-overlay") as SVGElement;
    store.activeTool = "Rectangle";
    ptr(overlay, "pointerdown", 100, 100, 2, { buttons: 2 });
    ptr(overlay, "pointermove", 150, 150, 2, { buttons: 2 });
    ptr(overlay, "pointerup", 150, 150, 2);
    expect(store.markups).toHaveLength(2);
  });

  it("Shift+F10 opens one menu, and not when a panel already handled the key", async () => {
    await mountViewport(store);
    const panelKey = (e: Event) => e.preventDefault();
    document.body.addEventListener("keydown", panelKey);
    await fireEvent.keyDown(document.body, { key: "F10", shiftKey: true });
    expect(screen.queryAllByRole("menu")).toHaveLength(0);
    document.body.removeEventListener("keydown", panelKey);
    await fireEvent.keyDown(document.body, { key: "F10", shiftKey: true });
    expect(screen.getAllByRole("menu")).toHaveLength(1);
  });

  it("Alt+Left steps back a view, but is ignored when a panel already handled it", async () => {
    await mountViewport(store);
    const prev = screen.getByTestId("prev-view") as HTMLButtonElement;
    const next = screen.getByTestId("next-view") as HTMLButtonElement;
    for (let i = 0; i < 6; i++) await fireEvent.keyDown(window, { key: "=", ctrlKey: true }); // 1.1^6 > 1.5x
    await waitFor(() => expect(prev.disabled).toBe(false), { timeout: 1000 });
    const panelKey = (e: Event) => e.preventDefault();
    document.body.addEventListener("keydown", panelKey);
    await fireEvent.keyDown(document.body, { key: "ArrowLeft", altKey: true });
    document.body.removeEventListener("keydown", panelKey);
    expect(next.disabled).toBe(true); // nothing was stepped back
    await fireEvent.keyDown(document.body, { key: "ArrowLeft", altKey: true });
    await waitFor(() => expect(next.disabled).toBe(false));
  });

  it("Paste from the menu lands at the right-click point, not where the pointer is by then", async () => {
    const { container, root } = await mountViewport(store);
    const overlay = container.querySelector("svg.markup-overlay") as SVGElement;
    store.selectedIds = new Set(["a"]);
    await fireEvent.keyDown(window, { key: "c", ctrlKey: true });
    expect(markupClipboard.size).toBe(1);
    store.selectedIds = new Set();
    await fireEvent.contextMenu(overlay, { clientX: 120, clientY: 100, button: 2 }); // PDF (120, 100)
    await fireEvent(root, new PointerEvent("pointermove", { bubbles: true, clientX: 5, clientY: 5, pointerId: 1 })); // moving to the menu item
    await fireEvent.click(screen.getByRole("menuitem", { name: /^Paste/ }));
    expect(store.markups).toHaveLength(3);
    const rect = (store.markups[2].geometry as { Rect: { min: { x: number; y: number } } }).Rect;
    expect(rect.min).toEqual({ x: 120, y: 100 });
  });
});
