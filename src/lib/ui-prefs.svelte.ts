/**
 * Small per-machine UI preferences kept in localStorage (not the Rust settings file: they
 * are conveniences, safe to lose). Every storage access is guarded: it can throw (private
 * window, blocked site data) and the app must work without it.
 */

const REUSE_KEY = "redline.reuseTools";
const THUMB_KEY = "redline.thumbnailSize";

/** Thumbnail column width range in CSS px: small grids for 100+ sheet sets, large for detail. */
export const THUMB_MIN = 70;
export const THUMB_MAX = 240;
export const THUMB_DEFAULT = 180;

/** `raw` as a thumbnail size: a whole number inside the range, else the default. */
export function clampThumbSize(raw: unknown): number {
  const n = typeof raw === "number" ? raw : Number(raw);
  if (!Number.isFinite(n)) return THUMB_DEFAULT;
  return Math.min(THUMB_MAX, Math.max(THUMB_MIN, Math.round(n)));
}

function readThumb(): number {
  try {
    const v = globalThis.localStorage?.getItem(THUMB_KEY);
    return v == null ? THUMB_DEFAULT : clampThumbSize(v);
  } catch {
    return THUMB_DEFAULT;
  }
}

function readReuse(): boolean {
  try {
    return globalThis.localStorage?.getItem(REUSE_KEY) !== "0";
  } catch {
    return true;
  }
}

class UiPrefs {
  /**
   * Reuse tools (Revu's "Reuse Tools"): true = a draw tool stays active after each markup
   * (Redline's behaviour so far, the default); false = it drops back to Select once the
   * markup is made, for placing one item and then editing it.
   */
  reuseTools = $state(readReuse());

  /**
   * Minimum width of a thumbnail in the Pages panel (CSS px). The panel lays thumbnails out
   * in as many columns of at least this width as fit, so a small size shows many sheets.
   */
  thumbnailSize = $state(readThumb());

  setThumbnailSize(value: number): void {
    this.thumbnailSize = clampThumbSize(value);
    try {
      globalThis.localStorage?.setItem(THUMB_KEY, String(this.thumbnailSize));
    } catch {
      // Not persisted: the choice still holds for this session.
    }
  }

  setReuseTools(value: boolean): void {
    this.reuseTools = value;
    try {
      globalThis.localStorage?.setItem(REUSE_KEY, value ? "1" : "0");
    } catch {
      // Not persisted: the choice still holds for this session.
    }
  }
}

export const uiPrefs = new UiPrefs();
