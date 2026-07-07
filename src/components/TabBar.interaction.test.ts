// @vitest-environment jsdom
/**
 * TabBar interaction tests.
 *
 * Covers the pointer-capture regression (fix/tab-close-and-save-prompt):
 *   - Clicking the × close button fires `ontabclose` with the correct docId.
 *   - Dragging the tab body (not the close button) still calls `onmoveTab`.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import TabBar from "./TabBar.svelte";
import type { DocTab } from "$lib/doc-tabs.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { TakeoffStore } from "$lib/takeoff-store.svelte";

// jsdom lacks PointerEvent — minimal shim.
if (typeof PointerEvent === "undefined") {
  (globalThis as Record<string, unknown>).PointerEvent = class PointerEvent extends MouseEvent {
    pointerId: number;
    pointerType: string;
    constructor(type: string, init: PointerEventInit = {}) {
      super(type, init as MouseEventInit);
      this.pointerId = init.pointerId ?? 1;
      this.pointerType = init.pointerType ?? "mouse";
    }
  };
}

// setPointerCapture is not implemented in jsdom — stub it.
if (!HTMLElement.prototype.setPointerCapture) {
  HTMLElement.prototype.setPointerCapture = vi.fn();
}

function fakeIpc() {
  return {
    add: vi.fn(async () => {}),
    update: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
  };
}

function makeTab(id: string, path = `/docs/${id}.pdf`): DocTab {
  return {
    docId: id,
    doc: { doc_id: id, path, page_count: 1, was_encrypted: false },
    store: new MarkupStore(id, fakeIpc()),
    takeoffStore: new TakeoffStore(),
    viewportSnapshot: { zoom: 1, pageIndex: 0, scrollX: 0, scrollY: 0 },
    isEncrypted: false,
  };
}

describe("TabBar — × close button", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("clicking × on tab fires ontabclose with the correct docId", async () => {
    const ontabclose = vi.fn();
    const tab = makeTab("doc1", "/path/to/doc1.pdf");

    const { getAllByRole } = render(TabBar, {
      props: {
        tabs: [tab],
        activeDocId: "doc1",
        ontabclick: vi.fn(),
        ontabclose,
        onmoveTab: vi.fn(),
      },
    });

    const closeButtons = getAllByRole("button");
    expect(closeButtons.length).toBe(1);

    await fireEvent.click(closeButtons[0]);
    await tick();

    expect(ontabclose).toHaveBeenCalledOnce();
    expect(ontabclose).toHaveBeenCalledWith("doc1");
  });

  it("clicking × on the second tab fires ontabclose for that tab's docId", async () => {
    const ontabclose = vi.fn();
    const tab1 = makeTab("doc1");
    const tab2 = makeTab("doc2");

    const { getAllByRole } = render(TabBar, {
      props: {
        tabs: [tab1, tab2],
        activeDocId: "doc1",
        ontabclick: vi.fn(),
        ontabclose,
        onmoveTab: vi.fn(),
      },
    });

    const closeButtons = getAllByRole("button");
    expect(closeButtons.length).toBe(2);

    // Click the second tab's close button.
    await fireEvent.click(closeButtons[1]);
    await tick();

    expect(ontabclose).toHaveBeenCalledOnce();
    expect(ontabclose).toHaveBeenCalledWith("doc2");
  });

  it("pointerdown on the tab body (not close button) captures the pointer for DnD", async () => {
    const tab = makeTab("doc1");
    const { container } = render(TabBar, {
      props: {
        tabs: [tab],
        activeDocId: "doc1",
        ontabclick: vi.fn(),
        ontabclose: vi.fn(),
        onmoveTab: vi.fn(),
      },
    });

    const tabEl = container.querySelector(".tab") as HTMLElement;
    expect(tabEl).not.toBeNull();

    // Stub setPointerCapture on this specific element.
    const capture = vi.fn();
    tabEl.setPointerCapture = capture;

    await fireEvent(tabEl, new PointerEvent("pointerdown", {
      pointerId: 1,
      pointerType: "mouse",
      button: 0,
      clientX: 50,
      bubbles: true,
    }));
    await tick();

    // setPointerCapture SHOULD be called when clicking the tab body.
    expect(capture).toHaveBeenCalledWith(1);
  });

  it("pointerdown on the close button does NOT capture the pointer", async () => {
    const tab = makeTab("doc1");
    const { container } = render(TabBar, {
      props: {
        tabs: [tab],
        activeDocId: "doc1",
        ontabclick: vi.fn(),
        ontabclose: vi.fn(),
        onmoveTab: vi.fn(),
      },
    });

    const tabEl = container.querySelector(".tab") as HTMLElement;
    const closeBtn = container.querySelector(".tab-close") as HTMLElement;
    expect(tabEl).not.toBeNull();
    expect(closeBtn).not.toBeNull();

    // Stub setPointerCapture on the tab element (close button click bubbles to tab).
    const capture = vi.fn();
    tabEl.setPointerCapture = capture;

    // Fire pointerdown originating from the close button (bubbles to tab).
    await fireEvent(closeBtn, new PointerEvent("pointerdown", {
      pointerId: 1,
      pointerType: "mouse",
      button: 0,
      clientX: 50,
      bubbles: true,
    }));
    await tick();

    // setPointerCapture must NOT be called when the source is the close button.
    expect(capture).not.toHaveBeenCalled();
  });
});
