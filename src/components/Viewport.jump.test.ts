// @vitest-environment jsdom
/**
 * Viewport jumpRequest tests (search parity: click-to-navigate + highlight).
 *
 * Mirrors Viewport.interaction.test.ts's mounting harness (mock $lib/ipc,
 * stub ResizeObserver, real Viewport + real MarkupStore).
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import Viewport from "./Viewport.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { buildMarkup } from "$lib/markup-tools";

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

describe("Viewport jumpRequest — page navigation", () => {
  it("switches to the target page when jumpRequest names a different page", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const { container, rerender } = await mountViewport(store);
    expect(pageLabel(container)).toBe("Page 1 / 3");

    await rerender({
      docInfo: FAKE_DOC,
      store,
      jumpRequest: { page: 2, nonce: 1 },
    });
    await waitFor(() => {
      expect(pageLabel(container)).toBe("Page 3 / 3");
    });
  });

  it("clamps an out-of-range page to the last valid page", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const { container, rerender } = await mountViewport(store);

    await rerender({
      docInfo: FAKE_DOC,
      store,
      jumpRequest: { page: 99, nonce: 1 },
    });
    await waitFor(() => {
      expect(pageLabel(container)).toBe("Page 3 / 3");
    });
  });

  it("re-fires on a repeat request to the SAME page when nonce is bumped", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const { container, rerender } = await mountViewport(store);

    await rerender({ docInfo: FAKE_DOC, store, jumpRequest: { page: 1, nonce: 1 } });
    await waitFor(() => expect(pageLabel(container)).toBe("Page 2 / 3"));

    // Navigate away, then request page 1 again with a fresh nonce — must apply again.
    await rerender({ docInfo: FAKE_DOC, store, jumpRequest: { page: 2, nonce: 2 } });
    await waitFor(() => expect(pageLabel(container)).toBe("Page 3 / 3"));

    await rerender({ docInfo: FAKE_DOC, store, jumpRequest: { page: 1, nonce: 3 } });
    await waitFor(() => expect(pageLabel(container)).toBe("Page 2 / 3"));
  });
});

describe("Viewport jumpRequest — text-hit rect centering", () => {
  it("centers on a rect on the CURRENT page without changing pageIndex or throwing", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const { container, rerender } = await mountViewport(store);
    expect(pageLabel(container)).toBe("Page 1 / 3");

    await rerender({
      docInfo: FAKE_DOC,
      store,
      jumpRequest: { page: 0, rect: [10, 10, 50, 50], nonce: 1 },
    });
    await tick();
    // Same page — centerOnRect's synchronous branch, not the page-switch/loadPageSize one.
    expect(pageLabel(container)).toBe("Page 1 / 3");
  });

  it("centers on a rect after switching pages (async loadPageSize branch)", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const { container, rerender } = await mountViewport(store);

    await rerender({
      docInfo: FAKE_DOC,
      store,
      jumpRequest: { page: 1, rect: [10, 10, 50, 50], nonce: 1 },
    });
    await waitFor(() => {
      expect(pageLabel(container)).toBe("Page 2 / 3");
    });
  });
});

describe("Viewport jumpRequest — markup selection", () => {
  it("selects the markup named by markupId so existing selection chrome highlights it", async () => {
    const store = new MarkupStore("d1", fakeIpc());
    const m = buildMarkup({
      markupType: "Rectangle",
      page: 0,
      geometry: { Rect: { min: { x: 10, y: 10 }, max: { x: 50, y: 50 } } },
      appearance: {
        color: "#ff0000", line_weight: 2, opacity: 1, fill: null,
        line_style: "Solid", font: null,
      },
      identity: FAKE_IDENTITY,
      now: "2026-01-01T00:00:00.000Z",
      id: "m-target",
    });
    store.seed([m]);
    expect(store.selectedIds.size).toBe(0);

    const { rerender } = await mountViewport(store);
    await rerender({
      docInfo: FAKE_DOC,
      store,
      jumpRequest: { page: 0, markupId: "m-target", nonce: 1 },
    });

    await waitFor(() => {
      expect(store.selectedIds.has("m-target")).toBe(true);
    });
  });
});
