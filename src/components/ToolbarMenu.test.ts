// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import ToolbarMenu from "./ToolbarMenu.svelte";

function items() {
  return [
    { id: "one", label: "First", onselect: vi.fn() },
    { id: "two", label: "Second", disabled: true, onselect: vi.fn() },
    { id: "three", label: "Third", separator: true, onselect: vi.fn() },
  ];
}

describe("ToolbarMenu", () => {
  it("is closed until the trigger is clicked", async () => {
    const { queryByRole, getByRole } = render(ToolbarMenu, { props: { label: "Document", items: items() } });
    expect(queryByRole("menu")).toBeNull();
    await fireEvent.click(getByRole("button", { name: /document/i }));
    expect(queryByRole("menu")).not.toBeNull();
    expect(getByRole("button", { name: /document/i }).getAttribute("aria-expanded")).toBe("true");
  });

  it("runs the chosen item and closes", async () => {
    const list = items();
    const { getByRole, queryByRole } = render(ToolbarMenu, { props: { label: "Document", items: list } });
    await fireEvent.click(getByRole("button", { name: /document/i }));
    await fireEvent.click(getByRole("menuitem", { name: "First" }));
    expect(list[0].onselect).toHaveBeenCalledOnce();
    expect(queryByRole("menu")).toBeNull();
  });

  it("does not run a disabled item", async () => {
    const list = items();
    const { getByRole } = render(ToolbarMenu, { props: { label: "Document", items: list } });
    await fireEvent.click(getByRole("button", { name: /document/i }));
    await fireEvent.click(getByRole("menuitem", { name: "Second" }));
    expect(list[1].onselect).not.toHaveBeenCalled();
  });

  it("closes on Escape and on an outside pointer press", async () => {
    const { getByRole, queryByRole } = render(ToolbarMenu, { props: { label: "Document", items: items() } });
    await fireEvent.click(getByRole("button", { name: /document/i }));
    await fireEvent.keyDown(getByRole("menu"), { key: "Escape" });
    expect(queryByRole("menu")).toBeNull();

    await fireEvent.click(getByRole("button", { name: /document/i }));
    expect(queryByRole("menu")).not.toBeNull();
    document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    await tick();
    expect(queryByRole("menu")).toBeNull();
  });

  it("renders a separator before an item that asks for one", async () => {
    const { getByRole, container } = render(ToolbarMenu, { props: { label: "Document", items: items() } });
    await fireEvent.click(getByRole("button", { name: /document/i }));
    expect(container.querySelectorAll('[role="separator"]').length).toBe(1);
  });

  it("disables the trigger when asked", () => {
    const { getByRole } = render(ToolbarMenu, { props: { label: "Document", items: items(), disabled: true } });
    expect((getByRole("button", { name: /document/i }) as HTMLButtonElement).disabled).toBe(true);
  });
});
