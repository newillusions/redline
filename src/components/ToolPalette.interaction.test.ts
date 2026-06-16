// @vitest-environment jsdom
/**
 * ToolPalette interaction tests.
 *
 * Mount the real ToolPalette with a real MarkupStore; drive button clicks via
 * @testing-library/user-event; assert store state and ARIA attributes update.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import ToolPalette from "./ToolPalette.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";

function fakeIpc() {
  return {
    add: vi.fn(async () => {}),
    update: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
  };
}

describe("ToolPalette", () => {
  let store: MarkupStore;

  beforeEach(() => {
    store = new MarkupStore("doc-tp", fakeIpc());
  });

  it("clicking Rectangle activates it in the store", async () => {
    const user = userEvent.setup();
    render(ToolPalette, { props: { store } });

    const btn = screen.getByTitle("Rectangle");
    await user.click(btn);

    expect(store.activeTool).toBe("Rectangle");
  });

  it("active button has aria-pressed=true", async () => {
    const user = userEvent.setup();
    render(ToolPalette, { props: { store } });

    const btn = screen.getByTitle("Rectangle");
    await user.click(btn);

    // @testing-library sets aria-pressed as a string attribute
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  it("inactive buttons have aria-pressed=false", async () => {
    const user = userEvent.setup();
    render(ToolPalette, { props: { store } });

    await user.click(screen.getByTitle("Rectangle"));

    // Every other tool button must report aria-pressed=false
    for (const title of ["Pan (Hand)", "Select", "Ellipse", "Line", "Arrow", "Highlight"]) {
      expect(screen.getByTitle(title)).toHaveAttribute("aria-pressed", "false");
    }
  });

  it("switching tools deactivates the previous one", async () => {
    const user = userEvent.setup();
    render(ToolPalette, { props: { store } });

    await user.click(screen.getByTitle("Rectangle"));
    expect(store.activeTool).toBe("Rectangle");

    await user.click(screen.getByTitle("Ellipse"));
    expect(store.activeTool).toBe("Ellipse");
    expect(screen.getByTitle("Rectangle")).toHaveAttribute("aria-pressed", "false");
  });

  it("default active tool is hand", () => {
    render(ToolPalette, { props: { store } });
    expect(store.activeTool).toBe("hand");
    expect(screen.getByTitle("Pan (Hand)")).toHaveAttribute("aria-pressed", "true");
  });
});
