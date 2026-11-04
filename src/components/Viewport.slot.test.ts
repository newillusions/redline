// @vitest-environment jsdom
/**
 * Viewport page-slot mode (reading mode's per-page mark-up layer, docs/continuous-markup.md):
 * keys from the active slot only, Space-pan restored once across slots, and a two-point
 * calibration that cannot pin a page busy after its tool is left.
 *
 * Same harness as Viewport.interaction.test.ts: real Viewport + real stores over a mocked IPC.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import Viewport from "./Viewport.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { TakeoffStore } from "$lib/takeoff-store.svelte";

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
  isMarkupLocked: () => false,
  isMarkupContentsLocked: () => false,
}));

import * as ipcMocks from "$lib/ipc";

const FAKE_DOC = { doc_id: "d1", path: "/fake.pdf", page_count: 4, was_encrypted: false };
const FAKE_IDENTITY = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "T" };

function fakeIpc() {
  return { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) };
}

function stubResizeObserver() {
  (globalThis as Record<string, unknown>).ResizeObserver = class {
    constructor(private cb: ResizeObserverCallback) {}
    observe(el: Element) {
      const entry = {
        contentRect: { width: 200, height: 200, top: 0, left: 0, bottom: 200, right: 200, x: 0, y: 0, toJSON() { return {}; } },
        target: el,
      } as unknown as ResizeObserverEntry;
      this.cb([entry], this as unknown as ResizeObserver);
    }
    unobserve() {}
    disconnect() {}
  };
}

type Slot = { page: number; zoom: number; active: boolean; inert: boolean };

async function mountSlot(store: MarkupStore, takeoffStore: TakeoffStore, slot: Slot, onslotbusy?: (b: boolean) => void) {
  Element.prototype.setPointerCapture = vi.fn();
  const r = render(Viewport, { props: { docInfo: FAKE_DOC, store, takeoffStore, slot, onslotbusy } });
  await waitFor(() => expect(vi.mocked(ipcMocks.getPageSize)).toHaveBeenCalled());
  const root = r.container.querySelector(".viewport-root") as HTMLElement;
  vi.spyOn(root, "getBoundingClientRect").mockReturnValue({
    left: 0, top: 0, right: 200, bottom: 200, width: 200, height: 200, x: 0, y: 0, toJSON() { return {}; },
  } as DOMRect);
  await tick();
  return { ...r, root, overlay: r.container.querySelector("svg.markup-overlay") as SVGElement };
}

function pointerDown(target: Element, x: number, y: number) {
  fireEvent(target, new PointerEvent("pointerdown", { bubbles: true, cancelable: true, clientX: x, clientY: y, pointerId: 1 }));
}

const key = (type: "keydown" | "keyup", init: KeyboardEventInit) =>
  window.dispatchEvent(new KeyboardEvent(type, { bubbles: true, cancelable: true, ...init }));

describe("Viewport page-slot mode", () => {
  let store: MarkupStore;
  let takeoff: TakeoffStore;

  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockImplementation(async (docId: string, pageIndex: number) => ({
      doc_id: docId, page_index: pageIndex, width_pts: 200, height_pts: 200,
    }));
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);
    stubResizeObserver();
    store = new MarkupStore("d1", fakeIpc());
    takeoff = new TakeoffStore();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("only the active slot takes tool keys", async () => {
    await mountSlot(store, takeoff, { page: 0, zoom: 1, active: false, inert: false });
    await mountSlot(store, takeoff, { page: 1, zoom: 1, active: false, inert: false });
    key("keydown", { key: "r", code: "KeyR" });
    await tick();
    expect(store.activeTool).toBe("hand");
    await mountSlot(store, takeoff, { page: 2, zoom: 1, active: true, inert: false });
    key("keydown", { key: "r", code: "KeyR" });
    await tick();
    expect(store.activeTool).toBe("Rectangle");
  });

  // Review finding 3 (PR #168): the tool held before Space-pan must come back exactly once,
  // whichever slot sees the key-repeat or the key-up.
  it("Space-pan: a key-repeat seen by another slot does not overwrite the saved tool", async () => {
    store.activeTool = "Rectangle";
    const a = await mountSlot(store, takeoff, { page: 0, zoom: 1, active: true, inert: false });
    const b = await mountSlot(store, takeoff, { page: 1, zoom: 1, active: false, inert: false });
    key("keydown", { key: " ", code: "Space" });
    await tick();
    expect(store.activeTool).toBe("hand");
    // The pointer moves to page 1: it becomes the active slot and sees the auto-repeat.
    await a.rerender({ slot: { page: 0, zoom: 1, active: false, inert: false } });
    await b.rerender({ slot: { page: 1, zoom: 1, active: true, inert: false } });
    key("keydown", { key: " ", code: "Space", repeat: true });
    key("keydown", { key: " ", code: "Space", repeat: true });
    await tick();
    expect(store.activeTool).toBe("hand");
    key("keyup", { key: " ", code: "Space" });
    await tick();
    expect(store.activeTool).toBe("Rectangle");
    expect(store.spacePanTool).toBeNull();
  });

  it("Space-pan: releasing Space after the arming slot unmounted still restores the tool", async () => {
    store.activeTool = "Ellipse";
    const a = await mountSlot(store, takeoff, { page: 0, zoom: 1, active: true, inert: false });
    await mountSlot(store, takeoff, { page: 1, zoom: 1, active: false, inert: false });
    key("keydown", { key: " ", code: "Space" });
    await tick();
    expect(store.activeTool).toBe("hand");
    a.unmount(); // page 0 scrolled out of the render window while Space is held
    key("keyup", { key: " ", code: "Space" });
    await tick();
    expect(store.activeTool).toBe("Ellipse");
  });

  it("Space-pan: window blur while Space is held restores the tool", async () => {
    store.activeTool = "Rectangle";
    await mountSlot(store, takeoff, { page: 0, zoom: 1, active: true, inert: false });
    key("keydown", { key: " ", code: "Space" });
    await tick();
    expect(store.activeTool).toBe("hand");
    window.dispatchEvent(new Event("blur"));
    await tick();
    expect(store.activeTool).toBe("Rectangle");
    expect(store.spacePanTool).toBeNull();
  });

  it("Space-pan: the tab being hidden while Space is held restores the tool", async () => {
    store.activeTool = "Ellipse";
    await mountSlot(store, takeoff, { page: 0, zoom: 1, active: true, inert: false });
    key("keydown", { key: " ", code: "Space" });
    await tick();
    const vis = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    document.dispatchEvent(new Event("visibilitychange"));
    vis.mockRestore();
    await tick();
    expect(store.activeTool).toBe("Ellipse");
    expect(store.spacePanTool).toBeNull();
  });

  // Review finding 2 (PR #168): a first calibration point must not keep its page busy (and
  // every other page inert) once the Calibrate tool is left.
  it("leaving the Calibrate tool after the first point cancels the calibration and frees the page", async () => {
    const busy = vi.fn();
    const s = await mountSlot(store, takeoff, { page: 2, zoom: 1, active: true, inert: false }, busy);
    store.activeTool = "calibrate";
    await tick();
    pointerDown(s.overlay, 20, 20);
    await tick();
    expect(takeoff.calibrationState?.step).toBe("waiting_p2");
    expect(busy).toHaveBeenLastCalledWith(true);
    store.activeTool = "select";
    await tick();
    expect(takeoff.calibrationState).toBeNull();
    expect(busy).toHaveBeenLastCalledWith(false);
  });

  it("a calibration with both points in (scale dialog open) is left to the dialog", async () => {
    const s = await mountSlot(store, takeoff, { page: 0, zoom: 1, active: true, inert: false });
    store.activeTool = "calibrate";
    await tick();
    pointerDown(s.overlay, 20, 20);
    await tick();
    pointerDown(s.overlay, 120, 20);
    await tick();
    expect(takeoff.calibrationState?.p2).toBeDefined();
    store.activeTool = "hand";
    await tick();
    expect(takeoff.calibrationState?.p2).toBeDefined();
  });
});
