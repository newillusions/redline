// @vitest-environment jsdom
/** The Link tool in the tool palette (docs/navigation.md RH1). */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import ToolPalette from "./ToolPalette.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { NavigationStore } from "$lib/navigation-store.svelte";

function store() {
  return new MarkupStore("d1", { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) });
}

function editableNav() {
  const nav = new NavigationStore("d1");
  nav.info = {
    page_count: 1, outline: { nodes: [], total: 0, broken: 0, read_only: null }, labels: [""],
    links: [], links_truncated: false, read_only: null, undo_depth: 0, redo_depth: 0,
  };
  return nav;
}

describe("ToolPalette Link tool", () => {
  it("is absent without a NavigationStore", () => {
    render(ToolPalette, { store: store() });
    expect(screen.queryByLabelText("Link tool")).toBeNull();
  });

  it("starts link mode on the Hand tool and toggles off", async () => {
    const s = store();
    s.activeTool = "Rectangle";
    const nav = editableNav();
    render(ToolPalette, { store: s, navStore: nav });
    await fireEvent.click(screen.getByLabelText("Link tool"));
    expect(nav.linkMode).toBe(true);
    expect(s.activeTool).toBe("hand");
    expect(screen.getByLabelText("Link tool").getAttribute("aria-pressed")).toBe("true");
    await fireEvent.click(screen.getByLabelText("Link tool"));
    expect(nav.linkMode).toBe(false);
  });

  it("is disabled for a file that cannot be edited", () => {
    const nav = editableNav();
    nav.info!.read_only = "password-protected";
    render(ToolPalette, { store: store(), navStore: nav });
    expect((screen.getByLabelText("Link tool") as HTMLButtonElement).disabled).toBe(true);
  });
});
