// @vitest-environment jsdom
/** ContextMenu: renders the items, keyboard-drives them, closes on Escape / outside press. */
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/svelte";
import ContextMenu from "./ContextMenu.svelte";
import type { MenuItem } from "$lib/context-menu";

const ITEMS: MenuItem[] = [
  { kind: "heading", label: "Status" },
  { kind: "item", id: "a", label: "Alpha" },
  { kind: "item", id: "b", label: "Beta", disabled: true },
  { kind: "separator" },
  { kind: "item", id: "c", label: "Gamma", shortcut: "Ctrl+G" },
];

function mount() {
  const onselect = vi.fn();
  const onclose = vi.fn();
  render(ContextMenu, { props: { items: ITEMS, x: 20, y: 30, label: "Test menu", onselect, onclose } });
  return { onselect, onclose, menu: screen.getByRole("menu", { name: "Test menu" }) };
}

afterEach(cleanup);

describe("ContextMenu", () => {
  it("renders the actions as menuitems and marks disabled ones", () => {
    mount();
    const items = screen.getAllByRole("menuitem");
    expect(items.map((i) => i.textContent?.trim())).toEqual(["Alpha", "Beta", "Gamma Ctrl+G"]);
    expect(items[1].getAttribute("aria-disabled")).toBe("true");
  });

  it("starts on the first enabled item and Arrow keys skip the disabled one", async () => {
    const { menu, onselect } = mount();
    await fireEvent.keyDown(menu, { key: "ArrowDown" });
    await fireEvent.keyDown(menu, { key: "Enter" });
    expect(onselect).toHaveBeenCalledWith("c");
  });

  it("Enter on the first item runs it; Escape closes without selecting", async () => {
    const { menu, onselect, onclose } = mount();
    await fireEvent.keyDown(menu, { key: "Enter" });
    expect(onselect).toHaveBeenCalledWith("a");
    await fireEvent.keyDown(menu, { key: "Escape" });
    expect(onclose).toHaveBeenCalled();
  });

  it("does not run a disabled item when clicked", async () => {
    const { onselect } = mount();
    await fireEvent.click(screen.getAllByRole("menuitem")[1]);
    expect(onselect).not.toHaveBeenCalled();
  });

  it("runs an enabled item on click", async () => {
    const { onselect } = mount();
    await fireEvent.click(screen.getAllByRole("menuitem")[2]);
    expect(onselect).toHaveBeenCalledWith("c");
  });

  it("closes on a press outside, not on a press inside", async () => {
    const { menu, onclose } = mount();
    await fireEvent.pointerDown(menu);
    expect(onclose).not.toHaveBeenCalled();
    await fireEvent.pointerDown(document.body);
    expect(onclose).toHaveBeenCalledTimes(1);
  });

  it("keys do not leak to window handlers while open", async () => {
    const { menu } = mount();
    const seen = vi.fn();
    window.addEventListener("keydown", seen);
    await fireEvent.keyDown(menu, { key: "ArrowDown" });
    window.removeEventListener("keydown", seen);
    expect(seen).not.toHaveBeenCalled();
  });
});
