/**
 * Bounded LRU cache for decoded raster tile images (spec section 20 Windows-freeze fix, 2026-07).
 *
 * Root cause (real Windows GUI session, 250% display scaling -> devicePixelRatio 2.5): the
 * Viewport tile cache was a plain `Map` with no eviction, keyed on
 * `page,tx,ty,zoom_millis`. During a smooth wheel-zoom, `zoom` changes continuously and each
 * distinct fractional value minted a brand-new key - and therefore a brand-new set of
 * ~6.5 MB decoded 1280x1280 `HTMLImageElement`s that were cached forever. A run of a few
 * dozen wheel ticks ballooned this to hundreds of MB within seconds and wedged the WebView2
 * main thread (whole window "Not Responding").
 *
 * This bounds total decoded bytes and evicts least-recently-used entries on insert, while
 * letting the caller protect the keys needed to paint the CURRENT visible tile set so an
 * in-progress render never loses its own tiles mid-paint.
 */

/** Minimal shape needed to estimate decoded byte size - matches HTMLImageElement. */
export interface CacheableImage {
  naturalWidth: number;
  naturalHeight: number;
}

interface TileCacheEntry<T extends CacheableImage> {
  img: T;
  bytes: number;
}

/** Estimated decoded RGBA byte size of an image (width x height x 4 channels). */
export function estimateTileBytes(img: CacheableImage): number {
  return img.naturalWidth * img.naturalHeight * 4;
}

/** Default cache cap: generous headroom above a full visible tile set (a handful of tiles at
 *  a few MB each), small enough to bound worst-case memory growth during a zoom gesture. */
export const DEFAULT_TILE_CACHE_CAP_BYTES = 320 * 1024 * 1024; // 320 MB

/**
 * A byte-bounded LRU cache. Recency is tracked via Map insertion order: `get()` and `set()`
 * both re-insert the key at the end, so the least-recently-used entry is always the first
 * one encountered when iterating for eviction.
 */
export class BoundedTileCache<T extends CacheableImage> {
  private readonly capBytes: number;
  private readonly entries = new Map<string, TileCacheEntry<T>>();
  private totalBytes = 0;

  constructor(capBytes: number = DEFAULT_TILE_CACHE_CAP_BYTES) {
    this.capBytes = capBytes;
  }

  /** Number of cached entries. */
  get size(): number {
    return this.entries.size;
  }

  /** Total estimated decoded bytes currently held. */
  get bytes(): number {
    return this.totalBytes;
  }

  has(key: string): boolean {
    return this.entries.has(key);
  }

  /** Look up an entry and mark it most-recently-used. Returns undefined on a miss. */
  get(key: string): T | undefined {
    const entry = this.entries.get(key);
    if (!entry) return undefined;
    this.entries.delete(key);
    this.entries.set(key, entry); // re-insert at the end = most-recently-used
    return entry.img;
  }

  /**
   * Insert or replace an entry, then evict least-recently-used entries until back under the
   * byte cap. `protectedKeys`, if given, are never evicted - used to guarantee the tile set
   * currently being painted survives eviction even under a very small cap.
   */
  set(key: string, img: T, protectedKeys?: ReadonlySet<string>): void {
    const bytes = estimateTileBytes(img);
    const existing = this.entries.get(key);
    if (existing) this.totalBytes -= existing.bytes;
    this.entries.delete(key);
    this.entries.set(key, { img, bytes }); // insert at the end = most-recently-used
    this.totalBytes += bytes;
    this.evict(protectedKeys);
  }

  private evict(protectedKeys?: ReadonlySet<string>): void {
    if (this.totalBytes <= this.capBytes) return;
    for (const [key, entry] of this.entries) {
      if (this.totalBytes <= this.capBytes) break;
      if (protectedKeys?.has(key)) continue;
      this.entries.delete(key);
      this.totalBytes -= entry.bytes;
    }
  }

  /** Drop everything (page change / component unmount). */
  clear(): void {
    this.entries.clear();
    this.totalBytes = 0;
  }
}
