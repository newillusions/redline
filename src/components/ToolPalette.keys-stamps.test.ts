// @vitest-environment jsdom
/** Palette: reading-mode disable + hint, and the Stamp picker. */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import ToolPalette from "./ToolPalette.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { ToolChestStore } from "$lib/toolchest-store.svelte";
import { builtinStamps } from "$lib/builtin-stamps";

vi.mock("$lib/ipc", () => ({ recordRecentTool: vi.fn().mockResolvedValue(undefined) }));

function store() {
  return new MarkupStore("d1", { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) });
}

// Mark-up works in continuous scroll (docs/continuous-markup.md): the palette has no
// reading-mode state, so it is the same enabled palette in both views.
describe("ToolPalette in either view", () => {
  it("is enabled, with no read-only hint", () => {
    const { container } = render(ToolPalette, { store: store() });
    const buttons = Array.from(container.querySelectorAll<HTMLButtonElement>(".tool-btn"));
    expect(buttons.length).toBeGreaterThan(20);
    expect(buttons.some((b) => b.disabled)).toBe(false);
    expect(screen.queryByTestId("palette-read-only-hint")).toBeNull();
  });
});

describe("ToolPalette tool titles", () => {
  it("show the Revu key", () => {
    const { container } = render(ToolPalette, { store: store() });
    const title = (t: string) => Array.from(container.querySelectorAll(".tool-btn")).find((b) => b.getAttribute("title")?.startsWith(t))?.getAttribute("title");
    expect(title("Rectangle")).toContain("(R)");
    expect(title("Cloud (")).toContain("(C)");
    expect(title("Text Box")).toContain("(T)");
    expect(title("Callout")).toContain("(Q)");
  });
});

describe("ToolPalette Stamp picker", () => {
  it("opens from the Stamp button and lists the built-in stamps", async () => {
    const s = store();
    render(ToolPalette, { store: s, toolChest: new ToolChestStore() });
    expect(screen.queryByTestId("stamp-picker")).toBeNull();
    await fireEvent.click(screen.getByLabelText("Stamp tool"));
    const items = screen.getAllByRole("menuitem").map((e) => e.textContent?.trim());
    expect(items).toEqual(["Approved", "Rejected", "Revise and Resubmit", "Reviewed", "For Information"]);
  });

  it("opens when the store flag is set (the S key)", async () => {
    const s = store();
    render(ToolPalette, { store: s });
    s.stampPickerOpen = true;
    await tick();
    expect(screen.getByTestId("stamp-picker")).toBeTruthy();
  });

  it("choosing a stamp arms Drawing-mode placement and closes the picker", async () => {
    const s = store();
    render(ToolPalette, { store: s, toolChest: new ToolChestStore() });
    await fireEvent.click(screen.getByLabelText("Stamp tool"));
    await fireEvent.click(screen.getByRole("menuitem", { name: /Approved/ }));
    expect(s.activeTool).toBe("placeTool");
    expect(s.pendingPlacementTool?.id).toBe(builtinStamps()[0].id);
    expect(s.stampPickerOpen).toBe(false);
    expect(screen.getByLabelText("Stamp tool").getAttribute("aria-pressed")).toBe("true");
  });

  it("lists saved stamp tools from the Tool Chest", async () => {
    const s = store();
    const tc = new ToolChestStore();
    const saved = { ...builtinStamps()[0], id: "11111111-1111-4111-8111-111111111111", name: "Site Stamp" };
    tc.sets = [{ id: "set", name: "Mine", tools: [saved] }];
    render(ToolPalette, { store: s, toolChest: tc });
    await fireEvent.click(screen.getByLabelText("Stamp tool"));
    expect(screen.getByRole("menuitem", { name: /Site Stamp/ })).toBeTruthy();
  });

  it("Escape closes the picker", async () => {
    const s = store();
    render(ToolPalette, { store: s });
    await fireEvent.click(screen.getByLabelText("Stamp tool"));
    await fireEvent.keyDown(screen.getByTestId("stamp-picker"), { key: "Escape" });
    expect(s.stampPickerOpen).toBe(false);
  });

  it("is enabled", () => {
    render(ToolPalette, { store: store() });
    expect((screen.getByLabelText("Stamp tool") as HTMLButtonElement).disabled).toBe(false);
  });
});
