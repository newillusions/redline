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
    for (const title of [
      "Pan (Hand)", "Select / Pointer (V)", "Ellipse", "Line", "Arrow",
      "Highlight (freeform box - for text, use Select Text)",
    ]) {
      expect(screen.getByTitle(title)).toHaveAttribute("aria-pressed", "false");
    }
  });

  it("Select Text sits immediately after Highlight (redline#29 discoverability fix: " +
    "Acrobat/Bluebeam users expect Highlight itself to snap to text)", () => {
    render(ToolPalette, { props: { store } });
    const buttons = screen.getAllByRole("button");
    const titles = buttons.map((b) => b.getAttribute("title"));
    const highlightIdx = titles.findIndex((t) => t?.startsWith("Highlight"));
    expect(highlightIdx).toBeGreaterThanOrEqual(0);
    expect(titles[highlightIdx + 1]).toContain("Select Text");
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

  it("calibrate tool button is present and activatable", async () => {
    const user = userEvent.setup();
    render(ToolPalette, { props: { store } });
    const btn = screen.getByTitle("Calibrate Scale (two-click)");
    expect(btn).toBeTruthy();
    await user.click(btn);
    expect(store.activeTool).toBe("calibrate");
  });

  it("MeasurementLength tool button is present", () => {
    render(ToolPalette, { props: { store } });
    expect(screen.getByTitle("Measure Length")).toBeTruthy();
  });

  it("Select / Pointer tool button is present and activates the select tool", async () => {
    const user = userEvent.setup();
    render(ToolPalette, { props: { store } });
    const btn = screen.getByTitle("Select / Pointer (V)");
    expect(btn).toBeTruthy();
    await user.click(btn);
    expect(store.activeTool).toBe("select");
  });

  it("text mark-up tools sit after Select Text and activate; Note sits after Callout", async () => {
    const user = userEvent.setup();
    const { container } = render(ToolPalette, { props: { store } });
    const titles = Array.from(container.querySelectorAll("button.tool-btn")).map((b) => b.getAttribute("title") ?? "");
    const idx = (prefix: string) => titles.findIndex((t) => t.startsWith(prefix));
    expect(idx("Underline text (U)")).toBe(idx("Select Text") + 1);
    expect(idx("Strikethrough text (D)")).toBe(idx("Select Text") + 2);
    expect(idx("Squiggly underline (Shift+U)")).toBe(idx("Select Text") + 3);
    expect(idx("Sticky Note (N)")).toBe(idx("Callout") + 1);

    for (const [prefix, tool] of [
      ["Underline text (U)", "Underline"], ["Strikethrough text (D)", "StrikeOut"],
      ["Squiggly underline (Shift+U)", "Squiggly"], ["Sticky Note (N)", "Note"],
    ] as const) {
      await user.click(screen.getByTitle(new RegExp("^" + prefix.replace(/[()+]/g, "\\$&"))));
      expect(store.activeTool).toBe(tool);
    }
  });
});
