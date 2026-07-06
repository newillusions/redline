/**
 * Unit tests for the pure settings helpers in settings.ts.
 *
 * Covers: default-filling and recent-color upsert (dedup/cap). IPC wrappers
 * (loadSettings / saveSettings) wrap Tauri invoke calls that cannot run in
 * jsdom/vitest - see recent-docs.test.ts for the precedent on that split.
 */
import { describe, it, expect } from "vitest";
import {
  withDefaults,
  upsertRecentColor,
  DEFAULT_SETTINGS,
  MAX_RECENT_COLORS,
} from "./settings";
import type { AppSettings } from "./settings";

describe("withDefaults", () => {
  it("returns all defaults when given an empty partial", () => {
    expect(withDefaults({})).toEqual(DEFAULT_SETTINGS);
  });

  it("overrides only the fields present in the partial", () => {
    const result = withDefaults({ theme: "light", author_name: "Martin" });
    expect(result.theme).toBe("light");
    expect(result.author_name).toBe("Martin");
    expect(result.measurement_unit).toBe(DEFAULT_SETTINGS.measurement_unit);
    expect(result.recent_colors).toEqual([]);
  });

  it("does not mutate DEFAULT_SETTINGS", () => {
    const frozen: AppSettings = { ...DEFAULT_SETTINGS };
    withDefaults({ theme: "light" });
    expect(DEFAULT_SETTINGS).toEqual(frozen);
  });
});

describe("upsertRecentColor", () => {
  it("prepends a new color to an empty list", () => {
    const result = upsertRecentColor([], "#ff0000");
    expect(result).toEqual(["#ff0000"]);
  });

  it("moves an existing color to the front without duplicating", () => {
    const result = upsertRecentColor(["#111111", "#222222", "#333333"], "#222222");
    expect(result).toEqual(["#222222", "#111111", "#333333"]);
  });

  it("evicts the oldest color when the cap is reached", () => {
    let colors: string[] = [];
    for (let i = 0; i < MAX_RECENT_COLORS + 3; i++) {
      colors = upsertRecentColor(colors, `#${i.toString(16).padStart(6, "0")}`);
    }
    expect(colors.length).toBe(MAX_RECENT_COLORS);
    expect(colors[0]).toBe(`#${(MAX_RECENT_COLORS + 2).toString(16).padStart(6, "0")}`);
  });

  it("does not mutate the input list", () => {
    const original = ["#111111"];
    const frozen = [...original];
    upsertRecentColor(original, "#222222");
    expect(original).toEqual(frozen);
  });
});
