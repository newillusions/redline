// @vitest-environment jsdom
/**
 * ReadingView tests — continuous reading mode.
 *
 * Strategy mirrors Viewport.interaction.test.ts: mock $lib/ipc, stub ResizeObserver to
 * drive containerWidth/Height synchronously, mount the real component. Covers: fit-width
 * default on a fresh tab, respecting a restored non-default snapshot, virtualization
 * (only the visible+lookahead window is fetched, never every page), jump requests, the
 * viewport-change callback contract, and the zoom-percent input / Ctrl+wheel zoom.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent, waitFor } from "@testing-library/svelte";
import ReadingView from "./ReadingView.svelte";

vi.mock("$lib/ipc", () => ({
  getPageSize: vi.fn(),
  renderTile: vi.fn(),
}));

import * as ipcMocks from "$lib/ipc";

const FAKE_DOC = { doc_id: "d1", path: "/fake.pdf", page_count: 10, was_encrypted: false };

/** Every page is a 200x200pt square unless overridden. */
function stubPageSizes(count: number, dims: { width_pts: number; height_pts: number } = { width_pts: 200, height_pts: 200 }) {
  vi.mocked(ipcMocks.getPageSize).mockImplementation(async (docId: string, pageIndex: number) => ({
    doc_id: docId,
    page_index: pageIndex,
    ...dims,
  }));
}

function stubRenderTile() {
  vi.mocked(ipcMocks.renderTile).mockImplementation(async (req) => ({
    doc_id: req.doc_id,
    page_index: req.page_index,
    tile_x: req.tile_x,
    tile_y: req.tile_y,
    width_px: 100,
    height_px: 100,
    zoom: req.zoom,
    dpr: req.dpr,
    // 1x1 transparent PNG — content doesn't matter, only that <img> gets a src.
    png_base64:
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
    render_ms: 1,
  }));
}

/** Stub ResizeObserver (same shape as Viewport.interaction.test.ts's helper). */
function stubResizeObserver(): (w: number, h: number) => void {
  let capturedCb: ResizeObserverCallback | null = null;
  let capturedTarget: Element | null = null;

  (globalThis as Record<string, unknown>).ResizeObserver = class {
    constructor(cb: ResizeObserverCallback) {
      capturedCb = cb;
    }
    observe(el: Element) {
      capturedTarget = el;
    }
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

beforeEach(() => {
  stubPageSizes(10);
  stubRenderTile();
});

describe("ReadingView — fit-width default", () => {
  it("computes a fit-width zoom on a fresh (never-customized) tab", async () => {
    const triggerResize = stubResizeObserver();
    const { container } = render(ReadingView, { props: { docInfo: FAKE_DOC } });
    triggerResize(400, 800); // container narrower than the 200pt page at zoom 1

    await waitFor(() => {
      expect(vi.mocked(ipcMocks.getPageSize)).toHaveBeenCalled();
    });
    await waitFor(() => {
      const indicator = container.querySelector(".reading-zoom-indicator");
      // fitWidthZoom(200, 400) = 2.0 -> 200%. Not the untouched-default 100%.
      expect(indicator?.textContent).toBe("200%");
    });
  });

  it("respects a restored non-default snapshot instead of auto-fitting", async () => {
    const triggerResize = stubResizeObserver();
    const { container } = render(ReadingView, {
      props: { docInfo: FAKE_DOC, initialState: { zoom: 0.5, pageIndex: 3, scrollX: 0, scrollY: 500 } },
    });
    triggerResize(400, 800);

    await waitFor(() => {
      const indicator = container.querySelector(".reading-zoom-indicator");
      expect(indicator?.textContent).toBe("50%");
    });
  });
});

describe("ReadingView — virtualization", () => {
  it("never fetches every page of a 10-page document from a small viewport", async () => {
    const triggerResize = stubResizeObserver();
    render(ReadingView, {
      props: { docInfo: FAKE_DOC, initialState: { zoom: 1.0, pageIndex: 0, scrollX: 0, scrollY: 0 } },
    });
    triggerResize(300, 300); // small viewport relative to 10 stacked 200pt pages

    await waitFor(() => {
      expect(vi.mocked(ipcMocks.renderTile)).toHaveBeenCalled();
    });

    const requestedPages = new Set(
      vi.mocked(ipcMocks.renderTile).mock.calls.map(([req]) => req.page_index),
    );
    expect(requestedPages.size).toBeLessThan(10);
    // Page 0 (top of a fresh-open document) must be among them.
    expect(requestedPages.has(0)).toBe(true);
    // A page far down the document must NOT have been fetched yet.
    expect(requestedPages.has(9)).toBe(false);
  });

  it("fetches a page whose top is scrolled into the small viewport", async () => {
    const triggerResize = stubResizeObserver();
    const { container } = render(ReadingView, {
      props: { docInfo: FAKE_DOC, initialState: { zoom: 1.0, pageIndex: 0, scrollX: 0, scrollY: 0 } },
    });
    triggerResize(300, 300);
    await waitFor(() => expect(vi.mocked(ipcMocks.renderTile)).toHaveBeenCalled());
    vi.mocked(ipcMocks.renderTile).mockClear();

    const scrollEl = container.querySelector(".reading-scroll") as HTMLDivElement;
    // 10 pages of 200pt height at zoom 1 + 12px gap => page 5 top is at 5*(200+12)=1060.
    Object.defineProperty(scrollEl, "scrollTop", { value: 1060, writable: true });
    await fireEvent.scroll(scrollEl);

    await waitFor(() => {
      const requestedPages = new Set(
        vi.mocked(ipcMocks.renderTile).mock.calls.map(([req]) => req.page_index),
      );
      expect(requestedPages.has(5)).toBe(true);
    });
  });
});

describe("ReadingView — jump requests", () => {
  // zoom: 2.0 here (not 1.0) deliberately avoids the DEFAULT_VIEWPORT_SNAPSHOT sentinel
  // ({zoom:1.0, pageIndex:0, scrollX:0, scrollY:0}) that triggers auto-fit-to-width on
  // mount (see ReadingView's "untouched default" effect) — this test wants a known,
  // stable zoom to compute the expected scroll offset against, not a container-width-
  // dependent one.
  it("scrolls to the requested page's top offset", async () => {
    const triggerResize = stubResizeObserver();
    const { container, rerender } = render(ReadingView, {
      props: {
        docInfo: FAKE_DOC,
        initialState: { zoom: 2.0, pageIndex: 0, scrollX: 0, scrollY: 0 },
        jumpRequest: null,
      },
    });
    triggerResize(300, 300);
    await waitFor(() => expect(vi.mocked(ipcMocks.renderTile)).toHaveBeenCalled());

    const scrollEl = container.querySelector(".reading-scroll") as HTMLDivElement;
    let currentScrollTop = 0;
    Object.defineProperty(scrollEl, "scrollTop", {
      get: () => currentScrollTop,
      set: (v) => { currentScrollTop = v; },
    });

    await rerender({
      docInfo: FAKE_DOC,
      initialState: { zoom: 2.0, pageIndex: 0, scrollX: 0, scrollY: 0 },
      jumpRequest: { page: 4, nonce: 1 },
    });

    await waitFor(() => {
      // Page 4's top at zoom 2, 200pt pages (400px each), 12px gap: 4 * 412 = 1648.
      expect(currentScrollTop).toBe(1648);
    });
  });
});

describe("ReadingView — onviewportchange", () => {
  it("reports zoom/pageIndex/scroll on mount", async () => {
    const triggerResize = stubResizeObserver();
    const onviewportchange = vi.fn();
    render(ReadingView, {
      props: {
        docInfo: FAKE_DOC,
        initialState: { zoom: 1.0, pageIndex: 0, scrollX: 0, scrollY: 0 },
        onviewportchange,
      },
    });
    triggerResize(300, 300);

    await waitFor(() => {
      expect(onviewportchange).toHaveBeenCalledWith(
        expect.objectContaining({ zoom: 1.0, pageIndex: 0, scrollX: 0 }),
      );
    });
  });
});

describe("ReadingView — zoom controls", () => {
  it("Fit W button recomputes zoom from page 0's width", async () => {
    const triggerResize = stubResizeObserver();
    const { container } = render(ReadingView, {
      props: { docInfo: FAKE_DOC, initialState: { zoom: 1.0, pageIndex: 0, scrollX: 0, scrollY: 0 } },
    });
    triggerResize(400, 300);
    await waitFor(() => expect(vi.mocked(ipcMocks.getPageSize)).toHaveBeenCalled());

    const fitBtn = Array.from(container.querySelectorAll("button")).find((b) => b.textContent === "Fit W")!;
    await fireEvent.click(fitBtn);

    await waitFor(() => {
      const indicator = container.querySelector(".reading-zoom-indicator");
      expect(indicator?.textContent).toBe("200%"); // fitWidthZoom(200, 400) = 2.0
    });
  });

  it("typing a zoom percent and pressing Enter commits it", async () => {
    const triggerResize = stubResizeObserver();
    const { container } = render(ReadingView, {
      props: { docInfo: FAKE_DOC, initialState: { zoom: 1.0, pageIndex: 0, scrollX: 0, scrollY: 0 } },
    });
    triggerResize(300, 300);
    await waitFor(() => expect(vi.mocked(ipcMocks.getPageSize)).toHaveBeenCalled());

    const input = container.querySelector(".zoom-percent-input") as HTMLInputElement;
    await fireEvent.focus(input);
    await fireEvent.input(input, { target: { value: "150" } });
    await fireEvent.keyDown(input, { key: "Enter" });
    await fireEvent.blur(input);

    await waitFor(() => {
      const indicator = container.querySelector(".reading-zoom-indicator");
      expect(indicator?.textContent).toBe("150%");
    });
  });

  it("Ctrl+wheel zooms and prevents the default scroll", async () => {
    const triggerResize = stubResizeObserver();
    const { container } = render(ReadingView, {
      props: { docInfo: FAKE_DOC, initialState: { zoom: 1.0, pageIndex: 0, scrollX: 0, scrollY: 0 } },
    });
    triggerResize(300, 300);
    await waitFor(() => expect(vi.mocked(ipcMocks.getPageSize)).toHaveBeenCalled());

    const scrollEl = container.querySelector(".reading-scroll") as HTMLDivElement;
    const notCancelled = await fireEvent.wheel(scrollEl, { deltaY: -100, ctrlKey: true });
    expect(notCancelled).toBe(false); // fireEvent returns false when preventDefault() was called

    await waitFor(() => {
      const indicator = container.querySelector(".reading-zoom-indicator");
      expect(indicator?.textContent).not.toBe("100%");
    });
  });

  it("a plain wheel event (no modifier) does not prevent default (native scroll)", async () => {
    const triggerResize = stubResizeObserver();
    const { container } = render(ReadingView, {
      props: { docInfo: FAKE_DOC, initialState: { zoom: 1.0, pageIndex: 0, scrollX: 0, scrollY: 0 } },
    });
    triggerResize(300, 300);
    await waitFor(() => expect(vi.mocked(ipcMocks.getPageSize)).toHaveBeenCalled());

    const scrollEl = container.querySelector(".reading-scroll") as HTMLDivElement;
    const notCancelled = await fireEvent.wheel(scrollEl, { deltaY: 100 });
    expect(notCancelled).toBe(true);
  });
});
