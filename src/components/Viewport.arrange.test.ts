// @vitest-environment jsdom
/**
 * Viewport keyboard surface for copy / cut / paste / duplicate (docs/arrange.md): the real
 * Viewport with a real MarkupStore. Coordinates as in Viewport.interaction.test.ts: zoom 1,
 * scroll 0, 200x200 pt page, so screen (x, y) is PDF (x, 200 - y).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
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

const key = (k: string, o: KeyboardEventInit = {}) => fireEvent.keyDown(window, { key: k, ctrlKey: true, ...o });
const movePointer = (root: Element, x: number, y: number) =>
  fireEvent(root, new PointerEvent("pointermove", { bubbles: true, clientX: x, clientY: y, pointerId: 1 }));
const mins = (store: MarkupStore) =>
  store.markups.map((m) => (m.geometry as { Rect: { min: { x: number; y: number } } }).Rect.min);

describe("Viewport copy / paste shortcuts", () => {
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
  afterEach(() => vi.restoreAllMocks());

  it("Ctrl+C then Ctrl+V pastes at the pointer: lower-left corner on the cursor, copy selected, one undo step", async () => {
    const { root } = await mountViewport(store);
    store.selectedIds = new Set(["a"]);
    await key("c");
    expect(markupClipboard.size).toBe(1);
    await movePointer(root, 100, 100); // PDF (100, 100)
    await key("v");
    expect(store.markups).toHaveLength(3);
    expect(mins(store)[2]).toEqual({ x: 100, y: 100 });
    expect(store.selectedIds).toEqual(new Set([store.markups[2].id]));
    store.undo();
    expect(store.markups).toHaveLength(2);
  });

  it("Ctrl+Shift+V pastes in place", async () => {
    const { root } = await mountViewport(store);
    store.selectedIds = new Set(["a"]);
    await key("c");
    await movePointer(root, 150, 50);
    await key("v", { shiftKey: true });
    expect(mins(store)[2]).toEqual({ x: 10, y: 10 });
  });

  it("Ctrl+X removes the selection and Ctrl+V brings it back as a new markup", async () => {
    const { root } = await mountViewport(store);
    store.selectedIds = new Set(["b"]);
    await key("x");
    expect(store.markups.map((m) => m.id)).toEqual(["a"]);
    await movePointer(root, 20, 20);
    await key("v");
    expect(store.markups).toHaveLength(2);
    expect(store.markups[1].id).not.toBe("b");
  });

  it("Ctrl+D duplicates the selection 12 pt right and down", async () => {
    await mountViewport(store);
    store.selectedIds = new Set(["a"]);
    await key("d");
    expect(mins(store)[2]).toEqual({ x: 22, y: -2 });
  });

  it("with nothing selected Ctrl+C / Ctrl+X / Ctrl+D change nothing, and an empty clipboard pastes nothing", async () => {
    await mountViewport(store);
    await key("c");
    await key("x");
    await key("d");
    await key("v");
    expect(store.markups).toHaveLength(2);
    expect(markupClipboard.size).toBe(0);
  });

  it("the Arrange bar shows for a selection with the Select tool, and not with another tool", async () => {
    const { container } = await mountViewport(store);
    expect(container.querySelector(".arrange-bar")).toBeNull();
    store.selectedIds = new Set(["a"]);
    await waitFor(() => expect(container.querySelector(".arrange-bar")).not.toBeNull());
    store.activeTool = "Rectangle";
    await waitFor(() => expect(container.querySelector(".arrange-bar")).toBeNull());
  });

  it("a refused command does not take the key from the browser; a command that ran does", async () => {
    await mountViewport(store);
    const ev = () => new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true, cancelable: true });
    const nothing = ev();
    window.dispatchEvent(nothing);
    expect(nothing.defaultPrevented).toBe(false); // "Select a markup to copy." -> native copy proceeds
    store.selectedIds = new Set(["a"]);
    const ran = ev();
    window.dispatchEvent(ran);
    expect(ran.defaultPrevented).toBe(true);
    expect(markupClipboard.size).toBe(1);
  });

  it("text selected in a side panel keeps its native copy", async () => {
    await mountViewport(store);
    store.selectedIds = new Set(["a"]);
    const panel = document.createElement("p");
    panel.textContent = "panel text";
    document.body.appendChild(panel);
    const r = document.createRange();
    r.selectNodeContents(panel);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(r);
    const ev = new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true, cancelable: true });
    window.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
    expect(markupClipboard.size).toBe(0);
    window.getSelection()!.removeAllRanges();
    panel.remove();
  });

  it("a shortcut typed into a text field is left to the field", async () => {
    const { container } = await mountViewport(store);
    store.selectedIds = new Set(["a"]);
    const input = container.ownerDocument.createElement("input");
    container.ownerDocument.body.appendChild(input);
    await fireEvent.keyDown(input, { key: "c", ctrlKey: true });
    expect(markupClipboard.size).toBe(0);
    input.remove();
  });
});
