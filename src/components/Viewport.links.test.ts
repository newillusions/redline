// @vitest-environment jsdom
/**
 * Viewport links (docs/navigation.md RH1-RH3) and destination jumps (RB2): the Hand tool
 * follows links, the Link tool draws them, other tools leave them alone, and a bookmark's
 * destination can set the view. Same mounting harness as Viewport.jump.test.ts.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import Viewport from "./Viewport.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { NavigationStore } from "$lib/navigation-store.svelte";
import type { LinkView, NavigationInfo, NavTarget } from "$lib/navigation-ipc";

// jsdom lacks PointerEvent: a MouseEvent carries the coordinates the handlers read.
if (typeof PointerEvent === "undefined") {
  (globalThis as Record<string, unknown>).PointerEvent = class PointerEvent extends MouseEvent {
    pointerId: number;
    constructor(type: string, init: PointerEventInit = {}) {
      super(type, init as MouseEventInit);
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
  addScale: vi.fn(async () => ({
    id: "scale-1", applies_to: { kind: "DocumentDefault" }, method: "TwoPoint",
    ratio: 0.001, unit: "m", label: "1:1000", precision: 2,
  })),
  listScales: vi.fn(async () => []),
  deleteScale: vi.fn(async () => true),
  exportMarkupList: vi.fn(async () => {}),
}));

import * as ipcMocks from "$lib/ipc";

const FAKE_DOC = { doc_id: "d1", path: "/fake.pdf", page_count: 3, was_encrypted: false };
const FAKE_IDENTITY = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "T" };
// 200x200pt page — matches Viewport.interaction.test.ts's coordinate convention.
const PAGE_SIZE = { doc_id: "d1", page_index: 0, width_pts: 200, height_pts: 200 };

function fakeIpc() {
  return {
    add: vi.fn(async () => {}),
    update: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
  };
}

function stubResizeObserver(): (w: number, h: number) => void {
  let capturedCb: ResizeObserverCallback | null = null;
  let capturedTarget: Element | null = null;
  (globalThis as Record<string, unknown>).ResizeObserver = class {
    constructor(cb: ResizeObserverCallback) { capturedCb = cb; }
    observe(el: Element) { capturedTarget = el; }
    unobserve() {}
    disconnect() {}
  };
  return (w: number, h: number) => {
    if (capturedCb && capturedTarget) {
      const entry: ResizeObserverEntry = {
        contentRect: { width: w, height: h, top: 0, left: 0, bottom: h, right: w, x: 0, y: 0, toJSON() { return {}; } } as DOMRectReadOnly,
        target: capturedTarget,
        borderBoxSize: [],
        contentBoxSize: [],
        devicePixelContentBoxSize: [],
      };
      capturedCb([entry], {} as ResizeObserver);
    }
  };
}

async function mountViewport(
  store: MarkupStore,
  extraProps: Record<string, unknown> = {}
) {
  const triggerResize = stubResizeObserver();
  Element.prototype.setPointerCapture = vi.fn();

  const { container, rerender } = render(Viewport, {
    props: { docInfo: FAKE_DOC, store, ...extraProps },
  });

  await waitFor(() => {
    expect(vi.mocked(ipcMocks.getUserIdentity)).toHaveBeenCalled();
  });

  const containerEl = container.querySelector(".viewport-root") as HTMLElement;
  if (containerEl) {
    vi.spyOn(containerEl, "getBoundingClientRect").mockReturnValue({
      left: 0, top: 0, right: 200, bottom: 200,
      width: 200, height: 200, x: 0, y: 0,
      toJSON() { return {}; },
    } as DOMRect);
    triggerResize(200, 200);
  }
  await tick();

  return { container, containerEl, rerender };
}

beforeEach(() => {
  vi.mocked(ipcMocks.getPageSize).mockResolvedValue(PAGE_SIZE);
  vi.mocked(ipcMocks.renderTile).mockResolvedValue({
    doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0,
    width_px: 512, height_px: 512, zoom: 1, dpr: 1,
    png_base64: "", render_ms: 1,
  });
  vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
  vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);
});


function pageLabel(container: HTMLElement): string {
  return container.querySelector(".page-label")?.textContent?.trim() ?? "";
}

function zoomPercent(container: HTMLElement): string {
  const input = container.querySelector('input[aria-label="Zoom percent"]') as HTMLInputElement | null;
  return input ? `${input.value}%` : "";
}

const goto2: NavTarget = { kind: "page", page: 2, view: { kind: "fit" } };

function navWith(links: LinkView[]): NavigationStore {
  const nav = new NavigationStore("d1");
  const info: NavigationInfo = {
    page_count: 3,
    outline: { nodes: [], total: 0, broken: 0, read_only: null },
    labels: ["", "", ""],
    links,
    links_truncated: false,
    read_only: null,
    undo_depth: 0,
    redo_depth: 0,
  };
  nav.info = info;
  nav.loaded = true;
  return nav;
}

const onPage0: LinkView = { id: "obj:5:0", page: 0, rect: [20, 20, 120, 80], target: goto2 };
const onPage1: LinkView = { id: "obj:6:0", page: 1, rect: [20, 20, 120, 80], target: goto2 };

describe("Viewport links", () => {
  it("the Hand tool shows this page's links as clickable regions and follows a click", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const nav = navWith([onPage0, onPage1]);
    const onfollowlink = vi.fn();
    const { container } = await mountViewport(store, { navStore: nav, onfollowlink });
    expect(store.activeTool).toBe("hand");
    const hits = container.querySelectorAll(".link-hit");
    expect(hits.length).toBe(1); // only the link on page 1
    await fireEvent.click(hits[0]);
    expect(onfollowlink).toHaveBeenCalledWith(goto2);
  });

  it("other tools leave links alone", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const nav = navWith([onPage0]);
    const { container } = await mountViewport(store, { navStore: nav, onfollowlink: vi.fn() });
    store.activeTool = "select";
    await tick();
    expect(container.querySelector(".link-hit")).toBeNull();
    expect(container.querySelector(".link-overlay")).toBeNull();
  });

  it("the Link tool draws a link area and hands it to the dialog", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const nav = navWith([]);
    const { container } = await mountViewport(store, { navStore: nav, onfollowlink: vi.fn() });
    nav.beginLinkMode();
    await tick();
    const svg = container.querySelector(".link-overlay.draw") as SVGElement;
    expect(svg).toBeTruthy();
    svg.dispatchEvent(new PointerEvent("pointerdown", { clientX: 20, clientY: 20, button: 0, bubbles: true, pointerId: 1 }));
    svg.dispatchEvent(new PointerEvent("pointermove", { clientX: 120, clientY: 90, bubbles: true, pointerId: 1 }));
    svg.dispatchEvent(new PointerEvent("pointerup", { clientX: 120, clientY: 90, bubbles: true, pointerId: 1 }));
    await tick();
    expect(nav.pendingLink).not.toBeNull();
    expect(nav.pendingLink!.page).toBe(0);
    const [l, b, r, t] = nav.pendingLink!.rect;
    expect(r - l).toBeGreaterThan(10);
    expect(t - b).toBeGreaterThan(10);
    expect(nav.linkMode).toBe(false);
  });

  it("choosing a mark-up tool ends link mode", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const nav = navWith([]);
    await mountViewport(store, { navStore: nav, onfollowlink: vi.fn() });
    nav.beginLinkMode();
    await tick();
    expect(nav.linkMode).toBe(true);
    store.activeTool = "Rectangle";
    await tick();
    expect(nav.linkMode).toBe(false);
  });

  it("works without a NavigationStore (no overlay)", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const { container } = await mountViewport(store);
    expect(container.querySelector(".link-overlay")).toBeNull();
  });
});

describe("Viewport jumpRequest - destination view", () => {
  it("a destination with a zoom changes the zoom and switches page", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const { container, rerender } = await mountViewport(store);
    await rerender({
      docInfo: FAKE_DOC,
      store,
      jumpRequest: { page: 1, view: { left: 10, top: 150, zoom: 3 }, nonce: 1 },
    });
    await waitFor(() => expect(pageLabel(container)).toBe("Page 2 / 3"));
    await waitFor(() => expect(zoomPercent(container)).toBe("300%"));
  });

  it("a destination without a zoom keeps the zoom", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const { container, rerender } = await mountViewport(store);
    const before = zoomPercent(container);
    await rerender({
      docInfo: FAKE_DOC,
      store,
      jumpRequest: { page: 0, view: { left: 10, top: 150, zoom: null }, nonce: 1 },
    });
    await tick();
    expect(zoomPercent(container)).toBe(before);
  });
});
