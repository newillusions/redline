import { describe, it, expect, vi, afterEach } from "vitest";
import { uiPrefs, clampThumbSize, THUMB_MIN, THUMB_MAX, THUMB_DEFAULT } from "./ui-prefs.svelte";

afterEach(() => vi.unstubAllGlobals());

describe("clampThumbSize", () => {
  it("keeps a size in range as a whole number", () => {
    expect(clampThumbSize(120)).toBe(120);
    expect(clampThumbSize("130.4")).toBe(130);
  });
  it("clamps to the range and falls back to the default for junk", () => {
    expect(clampThumbSize(5)).toBe(THUMB_MIN);
    expect(clampThumbSize(9999)).toBe(THUMB_MAX);
    expect(clampThumbSize("wide")).toBe(THUMB_DEFAULT);
    expect(clampThumbSize(NaN)).toBe(THUMB_DEFAULT);
  });
});

describe("uiPrefs.setThumbnailSize", () => {
  it("clamps, holds and persists the size; a throwing storage does not break it", () => {
    const data = new Map<string, string>();
    vi.stubGlobal("localStorage", { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => void data.set(k, v) });
    uiPrefs.setThumbnailSize(100);
    expect(uiPrefs.thumbnailSize).toBe(100);
    expect(data.get("redline.thumbnailSize")).toBe("100");
    uiPrefs.setThumbnailSize(1);
    expect(uiPrefs.thumbnailSize).toBe(THUMB_MIN);
    vi.stubGlobal("localStorage", { getItem: () => { throw new Error("x"); }, setItem: () => { throw new Error("x"); } });
    expect(() => uiPrefs.setThumbnailSize(150)).not.toThrow();
    expect(uiPrefs.thumbnailSize).toBe(150);
  });
});
