import { describe, it, expect } from "vitest";
import { BoundedTileCache, estimateTileBytes, DEFAULT_TILE_CACHE_CAP_BYTES } from "./tile-cache";

/** Fake decoded image matching the CacheableImage shape (no real HTMLImageElement in node). */
function fakeImg(width: number, height: number) {
  return { naturalWidth: width, naturalHeight: height };
}

// A 1280x1280 tile (TILE_SIZE_CSS=512 * dpr=2.5, the real Windows-freeze reproduction case)
// decodes to ~6.55 MB (1280*1280*4).
const WINDOWS_TILE_W = 1280;
const WINDOWS_TILE_H = 1280;

describe("estimateTileBytes", () => {
  it("estimates decoded RGBA bytes as width * height * 4", () => {
    expect(estimateTileBytes(fakeImg(512, 512))).toBe(512 * 512 * 4);
    expect(estimateTileBytes(fakeImg(WINDOWS_TILE_W, WINDOWS_TILE_H))).toBe(
      WINDOWS_TILE_W * WINDOWS_TILE_H * 4
    );
  });
});

describe("BoundedTileCache - the Windows freeze reproduction", () => {
  it("stays under the byte cap after feeding 200 distinct zoom-level tile sets (4 tiles each)", () => {
    // This directly reproduces the leak: a smooth wheel-zoom gesture that visits 200 distinct
    // fractional zoom levels, each caching a fresh set of 4 large Windows-scale tiles, the way
    // the unbounded Map cache did before this fix (real log: 0.9895, 0.9807, 1.0698, 0.8517...
    // stepping through many distinct zoomMillis values in seconds).
    const cache = new BoundedTileCache<ReturnType<typeof fakeImg>>(DEFAULT_TILE_CACHE_CAP_BYTES);

    for (let zoomLevel = 0; zoomLevel < 200; zoomLevel++) {
      for (let tileIdx = 0; tileIdx < 4; tileIdx++) {
        const key = `0,${tileIdx % 2},${Math.floor(tileIdx / 2)},${zoomLevel}`;
        cache.set(key, fakeImg(WINDOWS_TILE_W, WINDOWS_TILE_H));
      }
    }

    expect(cache.bytes).toBeLessThanOrEqual(DEFAULT_TILE_CACHE_CAP_BYTES);
    // Sanity: an unbounded cache would hold 200 * 4 = 800 tiles (~5.2 GB) here; the bounded
    // cache must hold only a small fraction of that.
    expect(cache.size).toBeLessThan(200 * 4);
  });

  it("evicts least-recently-used entries first, keeping the most recently touched ones", () => {
    // Small cap: room for exactly 2 tiles at a time.
    const tileBytes = estimateTileBytes(fakeImg(100, 100));
    const cache = new BoundedTileCache<ReturnType<typeof fakeImg>>(tileBytes * 2);

    cache.set("a", fakeImg(100, 100));
    cache.set("b", fakeImg(100, 100));
    // Touch "a" so it becomes most-recently-used; "b" is now the LRU entry.
    cache.get("a");
    cache.set("c", fakeImg(100, 100)); // should evict "b", not "a"

    expect(cache.has("a")).toBe(true);
    expect(cache.has("b")).toBe(false);
    expect(cache.has("c")).toBe(true);
  });

  it("never evicts a protected key even under a cap too small for the whole visible set", () => {
    const tileBytes = estimateTileBytes(fakeImg(100, 100));
    // Cap smaller than even 2 tiles combined - forces eviction pressure on every insert.
    const cache = new BoundedTileCache<ReturnType<typeof fakeImg>>(Math.floor(tileBytes * 1.5));
    const protectedKeys = new Set(["visible-1", "visible-2"]);

    cache.set("visible-1", fakeImg(100, 100), protectedKeys);
    cache.set("visible-2", fakeImg(100, 100), protectedKeys);
    cache.set("other", fakeImg(100, 100), protectedKeys);

    // The protected (currently-visible) tiles must survive even though the cap was blown.
    expect(cache.has("visible-1")).toBe(true);
    expect(cache.has("visible-2")).toBe(true);
  });

  it("clear() resets size and bytes to zero", () => {
    const cache = new BoundedTileCache<ReturnType<typeof fakeImg>>(DEFAULT_TILE_CACHE_CAP_BYTES);
    cache.set("a", fakeImg(512, 512));
    cache.set("b", fakeImg(512, 512));
    expect(cache.size).toBe(2);

    cache.clear();

    expect(cache.size).toBe(0);
    expect(cache.bytes).toBe(0);
  });

  it("replacing an existing key updates bytes correctly (no double-counting)", () => {
    const cache = new BoundedTileCache<ReturnType<typeof fakeImg>>(DEFAULT_TILE_CACHE_CAP_BYTES);
    cache.set("a", fakeImg(100, 100));
    const afterFirst = cache.bytes;
    cache.set("a", fakeImg(100, 100)); // same key, same size - should not double-count
    expect(cache.bytes).toBe(afterFirst);
    expect(cache.size).toBe(1);
  });
});
