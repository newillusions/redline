// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { loadMultiplyMeasure, persistMultiplyMeasure, MULTIPLY_MEASURE_STORAGE_KEY } from "./multiply-prefs";

describe("multiply measure preference", () => {
  it("defaults to centre-to-centre", () => {
    expect(loadMultiplyMeasure()).toBe("centre");
  });
  it("round-trips the choice", () => {
    persistMultiplyMeasure("gap");
    expect(loadMultiplyMeasure()).toBe("gap");
    persistMultiplyMeasure("centre");
    expect(loadMultiplyMeasure()).toBe("centre");
  });
  it("a corrupt stored value yields the default", () => {
    localStorage.setItem(MULTIPLY_MEASURE_STORAGE_KEY, "{not json");
    expect(loadMultiplyMeasure()).toBe("centre");
    localStorage.setItem(MULTIPLY_MEASURE_STORAGE_KEY, JSON.stringify("edge"));
    expect(loadMultiplyMeasure()).toBe("centre");
  });
  it("a blocked store neither throws nor breaks the default", () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(loadMultiplyMeasure()).toBe("centre");
    expect(() => persistMultiplyMeasure("gap")).not.toThrow();
    get.mockRestore();
    set.mockRestore();
  });
});
