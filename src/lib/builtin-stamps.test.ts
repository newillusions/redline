import { describe, it, expect } from "vitest";
import { builtinStamps, savedStampTools } from "./builtin-stamps";
import type { Tool, ToolSet } from "./ipc";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

describe("builtinStamps", () => {
  const stamps = builtinStamps();

  it("offers the five review stamps", () => {
    expect(stamps.map((s) => s.name)).toEqual([
      "Approved", "Rejected", "Revise and Resubmit", "Reviewed", "For Information",
    ]);
  });

  it("are Drawing-mode dynamic stamps with a rect template and the label as text", () => {
    for (const s of stamps) {
      expect(s.markup_type).toBe("StampDynamic");
      expect(s.placement_mode).toBe("Drawing");
      expect(s.geometry && "Rect" in s.geometry).toBe(true);
      expect(s.stamp && "Dynamic" in s.stamp ? s.stamp.Dynamic.base_text : null).toBe(s.name.toUpperCase());
      expect(s.children).toEqual([]);
    }
  });

  it("have unique, valid UUID ids (Rust Tool.id is a Uuid)", () => {
    const ids = stamps.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids) expect(id).toMatch(UUID);
  });
});

describe("savedStampTools", () => {
  const base = builtinStamps()[0];
  const mk = (id: string, markup_type: Tool["markup_type"], geometry: Tool["geometry"] = base.geometry): Tool =>
    ({ ...base, id, markup_type, geometry });
  const sets: ToolSet[] = [
    { id: "s1", name: "A", tools: [mk("1", "Stamp"), mk("2", "Rectangle"), mk("3", "StampDynamic", null)] },
    { id: "s2", name: "B", tools: [mk("4", "StampDynamic")] },
  ];
  it("lists stamp tools with a placeable geometry from every set", () => {
    expect(savedStampTools(sets).map((t) => t.id)).toEqual(["1", "4"]);
  });
});
