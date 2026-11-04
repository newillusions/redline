import { describe, it, expect } from "vitest";
import { shortcutGroups } from "./shortcuts";
import { TOOL_KEY_BINDINGS } from "./tool-keys";

describe("shortcut table", () => {
  const rows = shortcutGroups().flatMap((g) => g.rows);

  it("has no empty group or row", () => {
    for (const g of shortcutGroups()) {
      expect(g.rows.length).toBeGreaterThan(0);
      for (const r of g.rows) {
        expect(r.keys.trim()).not.toBe("");
        expect(r.label.trim()).not.toBe("");
      }
    }
  });

  it("lists every tool key binding, including the Y alias and Shift forms", () => {
    for (const b of TOOL_KEY_BINDINGS) {
      const keys = `${b.shift ? "Shift+" : ""}${b.key.toUpperCase()}`;
      expect(rows.some((r) => r.keys === keys), `missing ${keys}`).toBe(true);
    }
  });

  it("lists the file, search, undo and mode keys that live in App.svelte", () => {
    const all = rows.map((r) => `${r.keys} ${r.label}`).join("\n");
    for (const needle of ["Save", "Close the tab", "Undo", "Redo", "Search", "reading mode", "Next / previous tab", "format"]) {
      expect(all).toContain(needle);
    }
  });

  it("does not repeat a key combination within the tool group", () => {
    const tools = shortcutGroups().find((g) => g.title.startsWith("Tools"))!.rows.map((r) => r.keys);
    expect(new Set(tools).size).toBe(tools.length);
  });
});
