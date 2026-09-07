// @vitest-environment jsdom
/**
 * ToolChestPanel drag-to-reorder tests (spec "Tools & Tool Sets"). The backend
 * `reorder_tools` command + `ToolChestStore.reorderTools` are already fully tested
 * elsewhere (store.rs, ipc.test.ts) - this covers only the NEW drag UI wiring: dragging
 * one tool row onto another must call `reorderTools` with the right front-to-back order.
 */
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import ToolChestPanel from "./ToolChestPanel.svelte";
import { ToolChestStore } from "$lib/toolchest-store.svelte";
import type { Tool, ToolSet } from "$lib/ipc";

const mockReorderTools = vi.fn().mockResolvedValue(undefined);

vi.mock("$lib/ipc", () => ({
  listToolSets: vi.fn().mockResolvedValue([]),
  recentTools: vi.fn().mockResolvedValue([]),
  createToolSet: vi.fn(),
  renameToolSet: vi.fn(),
  deleteToolSet: vi.fn(),
  addToolFromMarkup: vi.fn(),
  deleteTool: vi.fn(),
  reorderTools: (setId: string, toolIds: string[]) => mockReorderTools(setId, toolIds),
  recordRecentTool: vi.fn(),
  importBtx: vi.fn(),
}));

const APPEARANCE: Tool["appearance"] = {
  color: "#000000", line_weight: 1, opacity: 1, fill: null, line_style: "Solid", font: null,
};

function tool(id: string, name: string): Tool {
  return {
    id, name, markup_type: "Rectangle", appearance: APPEARANCE, subject: null,
    placement_mode: "Properties", geometry: null, stamp: null, children: [],
  };
}

function setWithTools(): ToolSet {
  return { id: "set1", name: "My Set", tools: [tool("t1", "First"), tool("t2", "Second"), tool("t3", "Third")] };
}

describe("ToolChestPanel drag-to-reorder", () => {
  it("dragging a row onto another calls reorderTools with the new order", async () => {
    const store = new ToolChestStore();
    store.sets = [setWithTools()];

    const { container } = render(ToolChestPanel, { props: { toolChest: store, markupStore: null } });
    await tick();

    const rows = container.querySelectorAll(".tc-tool-row");
    expect(rows.length).toBe(3);

    // Drag "Third" (t3) onto "First" (t1) - expect it to land before t1.
    await fireEvent.dragStart(rows[2]);
    await fireEvent.dragOver(rows[0]);
    await fireEvent.drop(rows[0]);
    await tick();

    expect(mockReorderTools).toHaveBeenCalledWith("set1", ["t3", "t1", "t2"]);
  });

  it("dragging a row onto itself does not call reorderTools", async () => {
    const store = new ToolChestStore();
    store.sets = [setWithTools()];

    const { container } = render(ToolChestPanel, { props: { toolChest: store, markupStore: null } });
    await tick();
    const rows = container.querySelectorAll(".tc-tool-row");

    await fireEvent.dragStart(rows[0]);
    await fireEvent.dragOver(rows[0]);
    await fireEvent.drop(rows[0]);
    await tick();

    expect(mockReorderTools).not.toHaveBeenCalled();
  });

  it("each tool row has a drag handle and is draggable", async () => {
    const store = new ToolChestStore();
    store.sets = [setWithTools()];

    const { container } = render(ToolChestPanel, { props: { toolChest: store, markupStore: null } });
    await tick();
    const rows = container.querySelectorAll(".tc-tool-row");
    rows.forEach((row) => {
      expect(row.getAttribute("draggable")).toBe("true");
      expect(row.querySelector(".tc-drag-handle")).toBeTruthy();
    });
  });
});

describe("ToolChestPanel — overflow (owner defect, 2026-09-07)", () => {
  /**
   * Owner report (v0.3.17 live use): "the toolchest panels probably need to have
   * vertical scroll bars where needed." Confirmed via tools/gui-harness.mjs with a
   * 12-set/96-tool fixture: content was NOT actually being dropped or clipped out
   * of the DOM — wheel/programmatic scroll both reached the last row — but the
   * scrollbar affordance was invisible (thumb ~22 RGB units from the track, both
   * near-black) so a user had zero visual signal that 94 more tools existed below
   * the fold. Fixed in src/lib/styles.css's global ::-webkit-scrollbar rule
   * (raised contrast + width; not testable here since jsdom doesn't run a real
   * paint/scrollbar engine — verified visually via the harness, see the PR
   * description for before/after screenshots).
   *
   * What IS testable at the component level, and the actual invariant the CSS
   * fix depends on: ToolChestPanel must never silently drop content instead of
   * making it scrollable. This guards against a future regression (e.g. someone
   * adding a max-items slice, or a `{#if i < N}` cap) reintroducing a real data
   * loss where the CSS fix alone couldn't help.
   */
  it("renders every tool set and every tool in the DOM, however many there are", async () => {
    const store = new ToolChestStore();
    store.sets = Array.from({ length: 12 }, (_, si) => ({
      id: `set-${si}`,
      name: `Tool Set ${si + 1}`,
      tools: Array.from({ length: 8 }, (_, ti) => tool(`set-${si}-tool-${ti}`, `Tool ${si + 1}.${ti + 1}`)),
    }));

    const { container } = render(ToolChestPanel, { props: { toolChest: store, markupStore: null } });
    await tick();

    expect(container.querySelectorAll(".tc-set").length).toBe(12);
    expect(container.querySelectorAll(".tc-tool-row").length).toBe(96);
    // Spot-check the very last row (the one that was invisible under the old
    // low-contrast scrollbar) is actually present, not truncated.
    expect(container.textContent).toContain("Tool 12.8");
  });

  it("the panel root is a bounded, scrollable container (overflow-y: auto)", async () => {
    const store = new ToolChestStore();
    store.sets = [setWithTools()];
    const { container } = render(ToolChestPanel, { props: { toolChest: store, markupStore: null } });
    await tick();

    const root = container.querySelector(".toolchest-panel") as HTMLElement;
    expect(root).toBeTruthy();
    expect(getComputedStyle(root).overflowY).toBe("auto");
  });
});
