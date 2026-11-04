// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { recentToolFor } from "./recent-tools";
import type { Appearance } from "./ipc";

const recordRecentTool = vi.fn().mockResolvedValue(undefined);
vi.mock("./ipc", () => ({ recordRecentTool: (t: unknown) => recordRecentTool(t) }));

import { ToolChestStore } from "./toolchest-store.svelte";

const APP: Appearance = { color: "#112233", line_weight: 3, opacity: 1, fill: null, line_style: "Solid", font: null };

beforeEach(() => recordRecentTool.mockClear());

describe("recentToolFor", () => {
  it("builds a Properties-mode tool carrying the draft appearance, with a UUID id", () => {
    const t = recentToolFor("Rectangle", APP)!;
    expect(t.markup_type).toBe("Rectangle");
    expect(t.placement_mode).toBe("Properties");
    expect(t.appearance.color).toBe("#112233");
    expect(t.id).toMatch(/^71000000-0000-4000-8000-[0-9a-f]{12}$/);
  });
  it("gives each tool a distinct id", () => {
    expect(recentToolFor("Rectangle", APP)!.id).not.toBe(recentToolFor("Ellipse", APP)!.id);
  });
  it("skips navigation / setup tools", () => {
    for (const k of ["hand", "select", "selectText", "calibrate", "placeTool", "CloudPlus"] as const) {
      expect(recentToolFor(k, APP)).toBeNull();
    }
  });
});

describe("ToolChestStore.noteActiveTool", () => {
  it("records a palette/key tool in Recent", async () => {
    const s = new ToolChestStore();
    await s.noteActiveTool("Rectangle", APP);
    expect(recordRecentTool).toHaveBeenCalledTimes(1);
    expect(s.recent.map((t) => t.name)).toEqual(["Rectangle"]);
  });
  it("does not record navigation tools", async () => {
    const s = new ToolChestStore();
    await s.noteActiveTool("hand", APP);
    expect(recordRecentTool).not.toHaveBeenCalled();
  });
  it("skip=true records nothing", async () => {
    const s = new ToolChestStore();
    await s.noteActiveTool("Rectangle", APP, true);
    expect(recordRecentTool).not.toHaveBeenCalled();
  });
  it("re-using a tool moves it to the front without duplicating", async () => {
    const s = new ToolChestStore();
    await s.noteActiveTool("Rectangle", APP);
    await s.noteActiveTool("Ellipse", APP);
    await s.noteActiveTool("Rectangle", APP);
    expect(s.recent.map((t) => t.name)).toEqual(["Rectangle", "Ellipse"]);
  });
});
