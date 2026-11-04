import { describe, it, expect } from "vitest";
import { toolForKeyEvent, keyLabelFor, TOOL_KEY_BINDINGS } from "./tool-keys";

const ev = (key: string, mods: Partial<{ shiftKey: boolean; metaKey: boolean; ctrlKey: boolean; altKey: boolean }> = {}) =>
  ({ key, shiftKey: false, metaKey: false, ctrlKey: false, altKey: false, ...mods });

describe("tool keys (Bluebeam Revu defaults)", () => {
  // Revu: Rectangle R, Ellipse E, Line L, Arrow A, Highlight H, Cloud C, Cloud+ K, Callout Q,
  // Text Box T, Note N, Pen P, Polygon Shift+P, Polyline Shift+N, Select Text Shift+T,
  // Stamp S, Select V.
  it.each([
    ["r", "Rectangle"], ["e", "Ellipse"], ["l", "Line"], ["a", "Arrow"], ["h", "Highlight"],
    ["c", "Cloud"], ["k", "CloudPlus"], ["q", "Callout"], ["t", "Text"], ["n", "Note"],
    ["p", "Ink"], ["m", "MeasurementLength"], ["s", "stamp"], ["v", "select"],
    ["u", "Underline"], ["d", "StrikeOut"],
  ])("%s picks %s", (key, action) => {
    expect(toolForKeyEvent(ev(key))).toBe(action);
  });

  it("Y stays a second Cloud key", () => {
    expect(toolForKeyEvent(ev("y"))).toBe("Cloud");
  });

  it.each([["P", "Polygon"], ["N", "Polyline"], ["T", "selectText"], ["U", "Squiggly"]])(
    "Shift+%s picks %s", (key, action) => {
      expect(toolForKeyEvent(ev(key, { shiftKey: true }))).toBe(action);
    });

  it("Caps Lock (uppercase key, no Shift) still picks the plain tool", () => {
    expect(toolForKeyEvent(ev("R"))).toBe("Rectangle");
    expect(toolForKeyEvent(ev("U"))).toBe("Underline");
  });

  it("never fires with Ctrl, Meta or Alt held (paste, copy, ... stay native)", () => {
    for (const key of ["c", "v", "r", "t", "s", "a"]) {
      expect(toolForKeyEvent(ev(key, { ctrlKey: true }))).toBeNull();
      expect(toolForKeyEvent(ev(key, { metaKey: true }))).toBeNull();
      expect(toolForKeyEvent(ev(key, { altKey: true }))).toBeNull();
    }
  });

  it("ignores keys with no tool, and Shift variants that are not bound", () => {
    expect(toolForKeyEvent(ev("z"))).toBeNull();
    expect(toolForKeyEvent(ev("Enter"))).toBeNull();
    expect(toolForKeyEvent(ev("R", { shiftKey: true }))).toBeNull();
  });

  it("has no duplicate (key, shift) pair", () => {
    const seen = new Set(TOOL_KEY_BINDINGS.map((b) => `${b.shift ? "S+" : ""}${b.key}`));
    expect(seen.size).toBe(TOOL_KEY_BINDINGS.length);
  });

  it("keyLabelFor gives the primary key, not the alias", () => {
    expect(keyLabelFor("Cloud")).toBe("C");
    expect(keyLabelFor("Polygon")).toBe("Shift+P");
    expect(keyLabelFor("stamp")).toBe("S");
    expect(keyLabelFor("hand")).toBeNull();
  });
});
