// @vitest-environment jsdom
/**
 * Text mark-up tools (Underline / StrikeOut / Squiggly) and the sticky Note, driven through
 * the real Viewport.svelte with a real MarkupStore. Behaviour under test is the spec in
 * docs/text-markup-tools.md: drag over text creates the markup on release (no Enter), no text
 * under the drag creates nothing and shows the "No text selected" hint, N places a Note and
 * opens its pop-up, Esc keeps the typed note text, U / D / Shift+U / N pick the tools.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import Viewport from "./Viewport.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";

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

// The two text-layer IPC calls the drag gesture makes. Everything else in text-select is real.
vi.mock("$lib/text-select", async (orig) => ({
  ...(await orig<typeof import("$lib/text-select")>()),
  charIndexAtPoint: vi.fn(),
  getTextSelection: vi.fn(),
}));

import * as ipcMocks from "$lib/ipc";
import * as textSelect from "$lib/text-select";

const FAKE_DOC = { doc_id: "d1", path: "/fake.pdf", page_count: 1, was_encrypted: false };
const FAKE_IDENTITY = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "T" };
const FAKE_PAGE_SIZE = { doc_id: "d1", page_index: 0, width_pts: 200, height_pts: 200 };

const QUAD: [{ x: number; y: number }, { x: number; y: number }, { x: number; y: number }, { x: number; y: number }] = [
  { x: 40, y: 150 }, { x: 120, y: 150 }, { x: 40, y: 138 }, { x: 120, y: 138 },
];

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

/** Let queued promise callbacks (mocked IPC replies) run. */
const settle = () => new Promise<void>((r) => setTimeout(r, 0));

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

describe("text mark-up tools + sticky Note", () => {
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
    vi.mocked(textSelect.charIndexAtPoint).mockResolvedValue(3);
    vi.mocked(textSelect.getTextSelection).mockResolvedValue({ quads: [QUAD], text: "hello world" });
    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
  });

  afterEach(() => vi.restoreAllMocks());

  for (const tool of ["Underline", "StrikeOut", "Squiggly"] as const) {
    it(`${tool}: dragging over text creates the markup on release, with one quad per text line`, async () => {
      const { overlay } = await mountViewport(store);
      store.activeTool = tool;
      await tick();

      ptr(overlay, "pointerdown", 50, 50);
      await settle(); // the anchor character resolves (async IPC) before the drag continues
      ptr(overlay, "pointermove", 110, 50);
      await settle();
      ptr(overlay, "pointerup", 110, 50);

      await waitFor(() => expect(store.markups.length).toBe(1));
      const m = store.markups[0];
      expect(m.markup_type).toBe(tool);
      expect(m.geometry).toEqual({ Quads: [QUAD] });
      // A text rule is thin by default (the shared 2pt draft weight is a shape outline).
      expect(m.appearance.line_weight).toBe(1);
      expect(m.appearance.fill).toBeNull();
      expect(store.activeTool).toBe(tool); // stays armed for the next selection
      await waitFor(() => expect(ipc.add).toHaveBeenCalledTimes(1));
    });
  }

  it("dragging where there is no text creates nothing and shows the no-text hint", async () => {
    vi.mocked(textSelect.charIndexAtPoint).mockResolvedValue(null);
    vi.mocked(textSelect.getTextSelection).mockResolvedValue({ quads: [], text: "" });
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Underline";
    await tick();

    ptr(overlay, "pointerdown", 50, 50);
    ptr(overlay, "pointermove", 110, 50);
    ptr(overlay, "pointerup", 110, 50);

    await waitFor(() => expect(container.textContent).toContain("No text selected"));
    expect(store.markups.length).toBe(0);
    expect(ipc.add).not.toHaveBeenCalled();
  });

  it("the Select Text (I-beam) tool still does NOT create on release (Enter commits a Highlight)", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "selectText";
    await tick();

    ptr(overlay, "pointerdown", 50, 50);
    ptr(overlay, "pointermove", 110, 50);
    ptr(overlay, "pointerup", 110, 50);
    await tick();
    await new Promise((r) => setTimeout(r, 20));
    expect(store.markups.length).toBe(0);
  });

  it("Note: a click places the icon at the click point and opens the pop-up editor", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Note";
    await tick();

    await fireEvent.click(overlay, { clientX: 60, clientY: 80 }); // PDF (60, 120)
    await tick();

    expect(store.markups.length).toBe(1);
    const m = store.markups[0];
    expect(m.markup_type).toBe("Note");
    expect(m.geometry).toEqual({ Point: { x: 60, y: 120 } });
    expect(m.note).toEqual({ icon: "Note", open: false });
    expect(m.appearance.color).toBe("#ffd400"); // Revu's default Note is yellow
    expect(container.querySelector("textarea.text-editor")).not.toBeNull();
  });

  it("Note: typed text is kept when the pop-up is hidden with Esc (Revu), and the empty note survives a blur", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Note";
    await tick();
    await fireEvent.click(overlay, { clientX: 60, clientY: 80 });
    await tick();

    const ta = container.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    ta.value = "check the clearance";
    await fireEvent.input(ta);
    await fireEvent.keyDown(ta, { key: "Escape" });
    await tick();

    expect(container.querySelector("textarea.text-editor")).toBeNull();
    expect(store.markups[0].contents).toBe("check the clearance");

    // A second Note, closed with no text: the icon is kept (contents stay null).
    await fireEvent.click(overlay, { clientX: 100, clientY: 100 });
    await tick();
    const ta2 = container.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    await fireEvent.blur(ta2);
    await tick();
    expect(store.markups.length).toBe(2);
    expect(store.markups[1].contents).toBeNull();
  });

  it("Note: double-click on the icon under the select tool reopens the pop-up with its text", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Note";
    await tick();
    await fireEvent.click(overlay, { clientX: 60, clientY: 80 });
    await tick();
    const ta = container.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    ta.value = "see detail 4";
    await fireEvent.input(ta);
    await fireEvent.blur(ta);
    await tick();

    store.activeTool = "select";
    await tick();
    await fireEvent.dblClick(overlay, { clientX: 62, clientY: 78 });
    await tick();
    const reopened = container.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    expect(reopened).not.toBeNull();
    expect(reopened.value).toBe("see detail 4");
  });

  it("single-key shortcuts follow Revu: U underline, D strikethrough, Shift+U squiggly, N note", async () => {
    await mountViewport(store);
    for (const [key, tool] of [["u", "Underline"], ["d", "StrikeOut"], ["U", "Squiggly"], ["n", "Note"]] as const) {
      store.activeTool = "hand";
      await tick();
      await fireEvent.keyDown(window, { key, shiftKey: key === "U" });
      await tick();
      expect(store.activeTool).toBe(tool);
    }
  });

  it("Caps Lock does not change the shortcut meaning: plain U is Underline, Shift+U is Squiggly", async () => {
    await mountViewport(store);
    // Caps Lock on: plain U reports key "U" with shiftKey false; Shift+U reports "u".
    store.activeTool = "hand";
    await fireEvent.keyDown(window, { key: "U", shiftKey: false });
    await tick();
    expect(store.activeTool).toBe("Underline");
    store.activeTool = "hand";
    await fireEvent.keyDown(window, { key: "u", shiftKey: true });
    await tick();
    expect(store.activeTool).toBe("Squiggly");
    // D and N are case-insensitive too.
    for (const [key, tool] of [["D", "StrikeOut"], ["N", "Note"]] as const) {
      store.activeTool = "hand";
      await fireEvent.keyDown(window, { key });
      await tick();
      expect(store.activeTool).toBe(tool);
    }
  });

  it("modifier chords do not switch tools (Ctrl+U / Cmd+N stay free for the OS and the app)", async () => {
    await mountViewport(store);
    store.activeTool = "hand";
    await fireEvent.keyDown(window, { key: "u", ctrlKey: true });
    await fireEvent.keyDown(window, { key: "n", metaKey: true });
    await tick();
    expect(store.activeTool).toBe("hand");
  });
});
