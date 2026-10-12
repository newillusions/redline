// @vitest-environment jsdom
/**
 * LinkOverlay tests (docs/navigation.md RH1-RH3): following a link, not following after a
 * pan, drawing a new link area, Escape, tiny drags, and what is captured in each mode.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import LinkOverlay from "./LinkOverlay.svelte";
import type { LinkView, NavTarget } from "$lib/navigation-ipc";
import type { ViewportState } from "$lib/viewport";

// jsdom lacks PointerEvent: a MouseEvent carries the coordinates the handlers read.
if (typeof PointerEvent === "undefined") {
  (globalThis as Record<string, unknown>).PointerEvent = class PointerEvent extends MouseEvent {
    pointerId: number;
    constructor(type: string, init: PointerEventInit = {}) {
      super(type, init);
      this.pointerId = init.pointerId ?? 1;
    }
  };
}

const vs: ViewportState = {
  canvasWidthCss: 600,
  canvasHeightCss: 800,
  zoom: 1,
  dpr: 1,
  scrollX: 0,
  scrollY: 0,
  pageWidthPts: 600,
  pageHeightPts: 800,
};

const page2: NavTarget = { kind: "page", page: 2, view: { kind: "fit" } };
const link = (target: NavTarget = page2): LinkView => ({
  id: "obj:9:0",
  page: 0,
  rect: [100, 100, 200, 150], // page space; on screen x 100..200, y 650..700
  target,
});

function ptr(type: string, x: number, y: number) {
  return new PointerEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true, pointerId: 1 });
}

describe("LinkOverlay", () => {
  it("draws nothing and captures nothing when off", () => {
    const { container } = render(LinkOverlay, {
      links: [link()], viewState: vs, mode: "off", onfollow: vi.fn(), oncreate: vi.fn(), oncancel: vi.fn(),
    });
    expect(container.querySelector("svg")).toBeNull();
  });

  it("follow mode: each link is a clickable region at the mapped position", async () => {
    const onfollow = vi.fn();
    const { container } = render(LinkOverlay, {
      links: [link()], viewState: vs, mode: "follow", labels: ["", "", "E-103"], onfollow, oncreate: vi.fn(), oncancel: vi.fn(),
    });
    const hit = container.querySelector(".link-hit")!;
    expect(hit.getAttribute("x")).toBe("100");
    expect(hit.getAttribute("y")).toBe("650");
    expect(hit.getAttribute("width")).toBe("100");
    expect(hit.getAttribute("height")).toBe("50");
    expect(screen.getByLabelText("Page E-103")).toBeTruthy();
    await fireEvent(hit, ptr("pointerdown", 150, 675));
    await fireEvent.click(hit, { clientX: 150, clientY: 675 });
    expect(onfollow).toHaveBeenCalledTimes(1);
    expect(onfollow.mock.calls[0][0].target).toEqual(page2);
  });

  it("a click that ends a pan does not follow the link", async () => {
    const onfollow = vi.fn();
    const { container } = render(LinkOverlay, {
      links: [link()], viewState: vs, mode: "follow", onfollow, oncreate: vi.fn(), oncancel: vi.fn(),
    });
    const hit = container.querySelector(".link-hit")!;
    await fireEvent(hit, ptr("pointerdown", 10, 10));
    await fireEvent.click(hit, { clientX: 150, clientY: 675 });
    expect(onfollow).not.toHaveBeenCalled();
  });

  it("follows links on a zoomed and scrolled page", async () => {
    const onfollow = vi.fn();
    const { container } = render(LinkOverlay, {
      links: [link()], viewState: { ...vs, zoom: 2, scrollX: 50, scrollY: 100 }, mode: "follow",
      onfollow, oncreate: vi.fn(), oncancel: vi.fn(),
    });
    const hit = container.querySelector(".link-hit")!;
    expect(hit.getAttribute("x")).toBe("150"); // 100 * 2 - 50
    expect(hit.getAttribute("y")).toBe("1200"); // (800 - 150) * 2 - 100
    expect(hit.getAttribute("width")).toBe("200");
  });

  it("draw mode: a drag creates a link area in page space and outlines existing links", async () => {
    const oncreate = vi.fn();
    const { container } = render(LinkOverlay, {
      links: [link()], viewState: vs, mode: "draw", onfollow: vi.fn(), oncreate, oncancel: vi.fn(),
    });
    expect(container.querySelector(".link-box")).toBeTruthy();
    expect(container.querySelector(".link-hit")).toBeNull();
    const svg = container.querySelector("svg")!;
    await fireEvent(svg, ptr("pointerdown", 300, 400));
    await fireEvent(svg, ptr("pointermove", 400, 500));
    expect(container.querySelector(".link-drag")).toBeTruthy();
    await fireEvent(svg, ptr("pointerup", 400, 500));
    expect(oncreate).toHaveBeenCalledWith([300, 300, 400, 400]); // y = 800 - screenY
    expect(container.querySelector(".link-drag")).toBeNull();
  });

  it("draw mode: a click or a sliver makes nothing", async () => {
    const oncreate = vi.fn();
    const { container } = render(LinkOverlay, {
      links: [], viewState: vs, mode: "draw", onfollow: vi.fn(), oncreate, oncancel: vi.fn(),
    });
    const svg = container.querySelector("svg")!;
    await fireEvent(svg, ptr("pointerdown", 300, 400));
    await fireEvent(svg, ptr("pointerup", 300, 400));
    await fireEvent(svg, ptr("pointerdown", 300, 400));
    await fireEvent(svg, ptr("pointerup", 301, 500));
    expect(oncreate).not.toHaveBeenCalled();
  });

  it("draw mode: Escape stops, and the mouse-down is kept from the Hand tool's pan", async () => {
    const oncancel = vi.fn();
    const { container } = render(LinkOverlay, {
      links: [], viewState: vs, mode: "draw", onfollow: vi.fn(), oncreate: vi.fn(), oncancel,
    });
    const ev = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    const stop = vi.spyOn(ev, "stopPropagation");
    container.querySelector("svg")!.dispatchEvent(ev);
    expect(stop).toHaveBeenCalled(); // the viewport's pan handler sits above and would start a pan
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(oncancel).toHaveBeenCalledTimes(1);
  });

  it("follow mode ignores Escape", async () => {
    const oncancel = vi.fn();
    render(LinkOverlay, {
      links: [link()], viewState: vs, mode: "follow", onfollow: vi.fn(), oncreate: vi.fn(), oncancel,
    });
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(oncancel).not.toHaveBeenCalled();
  });
});
