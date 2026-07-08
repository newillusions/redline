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
    placement_mode: "Properties", geometry: null, stamp: null,
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
