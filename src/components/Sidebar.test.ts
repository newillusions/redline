// @vitest-environment jsdom
/**
 * Sidebar tests (owner defect 2026-11-04: panels in the sidebars could not be resized or
 * moved, and clipped their content). Layout maths is covered in panel-layout.test.ts and
 * real scrolling/dragging in tools/gui-harness-sidebar-layout.mjs (jsdom has no layout);
 * this covers the component wiring: what renders where, collapse, the move menu, grip
 * keyboard moves, close, and that every expanded body is the scroll container.
 */
import { describe, it, expect, vi, beforeAll, afterAll } from "vitest";
import { render, fireEvent, within } from "@testing-library/svelte";
import { tick } from "svelte";
import SidebarHarness from "./__test-fixtures__/SidebarHarness.svelte";
import { PanelLayoutStore } from "$lib/panel-layout-store.svelte";
import { PANEL_MIN_HEIGHT } from "$lib/panel-layout";

function setup(props: { searchOpen?: boolean; onclosesearch?: () => void } = {}) {
  const persist = vi.fn().mockResolvedValue(undefined);
  const store = new PanelLayoutStore(persist, 0);
  store.load(null);
  const r = render(SidebarHarness, { props: { store, ...props } });
  const order = (side: "left" | "right") =>
    Array.from(r.getByTestId(`sidebar-${side}`).querySelectorAll<HTMLElement>("[data-panel-id]")).map(
      (e) => e.dataset.panelId,
    );
  return { ...r, store, persist, order };
}

describe("Sidebar", () => {
  it("renders only panels that have content, in layout order, on their own side", () => {
    const { order } = setup();
    expect(order("left")).toEqual(["recent", "toolchest", "layers"]);
    expect(order("right")).toEqual(["properties"]);
  });

  it("shows Search only while it is switched on, with a close button", async () => {
    const onclosesearch = vi.fn();
    const { order, getByLabelText } = setup({ searchOpen: true, onclosesearch });
    expect(order("left")[0]).toBe("search");
    await fireEvent.click(getByLabelText("Close search"));
    expect(onclosesearch).toHaveBeenCalledOnce();
  });

  it("gives each expanded panel a weighted flex share with a minimum height", () => {
    const { getByTestId } = setup();
    const frame = getByTestId("panel-frame-toolchest");
    expect(frame.style.flex).toMatch(/^1 1 0(px)?$/);
    expect(frame.style.minHeight).toBe(`${PANEL_MIN_HEIGHT}px`);
    expect(getByTestId("panel-body-toolchest").className).toContain("panel-frame-body");
  });

  it("collapses and expands from the header, and persists it in the layout", async () => {
    const { getByTestId, queryByTestId, store } = setup();
    const toggle = getByTestId("accordion-tool-chest");
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    await fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(queryByTestId("panel-body-toolchest")).toBeNull();
    expect(store.layout.collapsed.toolchest).toBe(true);
  });

  it("puts a splitter only between neighbouring expanded panels", async () => {
    const { getByTestId, queryByTestId } = setup();
    expect(getByTestId("resize-split-recent")).toBeTruthy();
    expect(getByTestId("resize-split-toolchest")).toBeTruthy();
    expect(queryByTestId("resize-split-layers")).toBeNull(); // last in the sidebar
    await fireEvent.click(getByTestId("accordion-tool-chest"));
    await tick();
    // Recent now pairs with Layers across the collapsed Tool Chest.
    expect(getByTestId("resize-split-recent").getAttribute("aria-label")).toBe("Resize Recent Documents and Layers");
    expect(queryByTestId("resize-split-toolchest")).toBeNull();
  });

  it("moves a panel to the other sidebar and reorders it from the panel menu", async () => {
    const { getByTestId, getByRole, order } = setup();
    await fireEvent.click(getByTestId("panel-menu-layers"));
    await fireEvent.click(getByRole("menuitem", { name: "Move to right sidebar" }));
    expect(order("right")).toEqual(["properties", "layers"]);
    expect(order("left")).toEqual(["recent", "toolchest"]);

    await fireEvent.click(getByTestId("panel-menu-layers"));
    const menu = getByTestId("panel-menu-list");
    expect(within(menu).getByRole("menuitem", { name: "Move down" })).toHaveProperty("disabled", true);
    await fireEvent.click(within(menu).getByRole("menuitem", { name: "Move up" }));
    expect(order("right")).toEqual(["layers", "properties"]);
  });

  it("closes the panel menu with Escape and returns focus to its trigger", async () => {
    const { getByTestId, queryByTestId } = setup();
    const trigger = getByTestId("panel-menu-recent");
    await fireEvent.click(trigger);
    await tick();
    await fireEvent.keyDown(getByTestId("panel-menu-list"), { key: "Escape" });
    expect(queryByTestId("panel-menu-list")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("moves a panel with the arrow keys on its grip", async () => {
    const { getByTestId, order } = setup();
    await fireEvent.keyDown(getByTestId("panel-grip-recent"), { key: "ArrowDown" });
    expect(order("left")).toEqual(["toolchest", "recent", "layers"]);
    await fireEvent.keyDown(getByTestId("panel-grip-recent"), { key: "ArrowRight" });
    expect(order("right")).toEqual(["properties", "recent"]);
  });

  it("resizes the sidebar width from the keyboard on its handle", async () => {
    const { getByTestId, store } = setup();
    const before = store.layout.left.width;
    await fireEvent.keyDown(getByTestId("resize-sidebar-left"), { key: "ArrowRight" });
    expect(store.layout.left.width).toBe(before + 16);
    await fireEvent.keyDown(getByTestId("resize-sidebar-right"), { key: "ArrowLeft", shiftKey: true });
    // The right sidebar's handle is on its left edge: moving it left makes it wider.
    expect(store.layout.right.width).toBe(before + 64);
  });

  it("shows a hint in a sidebar left with no panels", async () => {
    const { getByTestId, getByRole, getByText } = setup();
    await fireEvent.click(getByTestId("panel-menu-properties"));
    await fireEvent.click(getByRole("menuitem", { name: "Move to left sidebar" }));
    expect(getByText(/No panels here/)).toBeTruthy();
  });

  describe("grip drag", () => {
    // jsdom has no PointerEvent; MouseEvent carries clientX/Y/button, which is all the grip reads.
    class TestPointerEvent extends MouseEvent {
      pointerId = 1;
    }
    const original = (window as unknown as { PointerEvent?: unknown }).PointerEvent;
    beforeAll(() => {
      (window as unknown as { PointerEvent: unknown }).PointerEvent = TestPointerEvent;
    });
    afterAll(() => {
      (window as unknown as { PointerEvent: unknown }).PointerEvent = original;
    });

    // jsdom lays everything out at 0,0: x=0 is "over the left sidebar", y=50 is below every panel.
    async function dragRecentDown(grip: HTMLElement) {
      await fireEvent.pointerDown(grip, { button: 0, clientX: 0, clientY: 0 });
      await fireEvent.pointerMove(grip, { clientX: 0, clientY: 50 });
    }

    it("control: releasing the grip over the sidebar commits the move", async () => {
      const { getByTestId, order } = setup();
      const grip = getByTestId("panel-grip-recent");
      await dragRecentDown(grip);
      await fireEvent.pointerUp(grip, { clientX: 0, clientY: 50 });
      expect(order("left")).toEqual(["toolchest", "layers", "recent"]);
    });

    it("pointercancel abandons the drag without moving the panel", async () => {
      const { getByTestId, order, store } = setup();
      const before = store.layout;
      const grip = getByTestId("panel-grip-recent");
      await dragRecentDown(grip);
      await fireEvent.pointerCancel(grip);
      await fireEvent.pointerUp(grip, { clientX: 0, clientY: 50 });
      expect(order("left")).toEqual(["recent", "toolchest", "layers"]);
      expect(store.layout).toBe(before);
    });

    it("Escape cancels an active drag", async () => {
      const { getByTestId, order, store } = setup();
      const before = store.layout;
      const grip = getByTestId("panel-grip-recent");
      await dragRecentDown(grip);
      await fireEvent.keyDown(window, { key: "Escape" });
      await fireEvent.pointerUp(grip, { clientX: 0, clientY: 50 });
      expect(order("left")).toEqual(["recent", "toolchest", "layers"]);
      expect(store.layout).toBe(before);
    });
  });

  it("returns focus to the moved panel's grip after a menu move and announces it", async () => {
    const { getByTestId, getByRole } = setup();
    await fireEvent.click(getByTestId("panel-menu-layers"));
    await fireEvent.click(getByRole("menuitem", { name: "Move to right sidebar" }));
    await tick();
    await tick();
    expect(document.activeElement).toBe(getByTestId("panel-grip-layers"));
    expect(getByTestId("panel-move-status-left").textContent).toContain("Layers panel moved to right sidebar, position 2 of 2");
  });

  it("gives split handles aria-valuenow/min/max", () => {
    const { getByTestId } = setup();
    const h = getByTestId("resize-split-recent");
    expect(h.getAttribute("aria-valuemin")).toBe("0");
    expect(h.getAttribute("aria-valuemax")).toBe("100");
    expect(Number(h.getAttribute("aria-valuenow"))).toBeGreaterThan(0);
  });
});
