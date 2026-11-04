import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import { finishesTool } from "./tool-reuse";
import { uiPrefs } from "./ui-prefs.svelte";
import { mk, storeWith } from "./arrange.test-util";

describe("finishesTool", () => {
  it("a draw tool is finished by the markup it makes", () => {
    expect(finishesTool("Rectangle", "Rectangle")).toBe(true);
    expect(finishesTool("MeasurementLength", "MeasurementLength")).toBe(true);
    expect(finishesTool("Rectangle", "Ellipse")).toBe(false);
  });
  it("Cloud+ is finished by its cloud, a Tool Chest drawing tool by any placement", () => {
    expect(finishesTool("CloudPlus", "Cloud")).toBe(true);
    expect(finishesTool("CloudPlus", "Callout")).toBe(false);
    expect(finishesTool("placeTool", "Stamp")).toBe(true);
  });
  it("the Count tool keeps counting", () => {
    expect(finishesTool("MeasurementCount", "MeasurementCount")).toBe(false);
  });
});

describe("MarkupStore with Reuse tools off", () => {
  beforeEach(() => uiPrefs.setReuseTools(true));

  it("by default a tool stays active after a markup is made", () => {
    const s = storeWith([]);
    s.activeTool = "Rectangle";
    s.create(mk("a"));
    expect(s.activeTool).toBe("Rectangle");
  });

  it("with reuse off the tool drops back to Select after the markup is made", () => {
    uiPrefs.setReuseTools(false);
    const s = storeWith([]);
    s.activeTool = "Rectangle";
    s.create(mk("a"));
    expect(s.activeTool).toBe("select");
  });

  it("with reuse off a markup of another kind (a paste) leaves the tool alone", () => {
    uiPrefs.setReuseTools(false);
    const s = storeWith([]);
    s.activeTool = "Ellipse";
    s.create(mk("a"));
    expect(s.activeTool).toBe("Ellipse");
  });

  it("with reuse off the Count tool still keeps counting", () => {
    uiPrefs.setReuseTools(false);
    const s = storeWith([]);
    s.activeTool = "MeasurementCount";
    s.create(mk("a", { markup_type: "MeasurementCount" }));
    expect(s.activeTool).toBe("MeasurementCount");
  });

  describe("persistence", () => {
    afterEach(() => vi.unstubAllGlobals());

    it("the preference is written to localStorage", () => {
      const data = new Map<string, string>();
      vi.stubGlobal("localStorage", { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => void data.set(k, v) });
      uiPrefs.setReuseTools(false);
      expect(data.get("redline.reuseTools")).toBe("0");
      uiPrefs.setReuseTools(true);
      expect(data.get("redline.reuseTools")).toBe("1");
    });

    it("a storage that throws does not break the toggle", () => {
      vi.stubGlobal("localStorage", { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); } });
      expect(() => uiPrefs.setReuseTools(false)).not.toThrow();
      expect(uiPrefs.reuseTools).toBe(false);
    });
  });
});
