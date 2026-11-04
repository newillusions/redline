/**
 * Previous view / Next view (Revu and PDF-XChange: Alt+Left / Alt+Right). A capped pair of
 * stacks of where the reader was: page, zoom and scroll. A view is recorded BEFORE a jump
 * (link, bookmark, search hit, Markups-list row, page jump) or a large zoom change, so
 * "Previous view" returns to the plan the reader was on. Pure; no Svelte, no DOM.
 */

export interface ViewEntry {
  pageIndex: number;
  zoom: number;
  scrollX: number;
  scrollY: number;
}

/** Most views kept per direction; the oldest are dropped first. */
export const MAX_VIEW_HISTORY = 50;
/** A zoom change by at least this factor (either way) counts as a view change worth recording. */
export const LARGE_ZOOM_FACTOR = 1.5;
/** Two views on the same page closer than this (CSS px of scroll, and zoom ratio) are the same view. */
const SAME_SCROLL_PX = 24;
const SAME_ZOOM_RATIO = 1.02;

export function sameView(a: ViewEntry, b: ViewEntry): boolean {
  if (a.pageIndex !== b.pageIndex) return false;
  const hi = Math.max(a.zoom, b.zoom);
  const lo = Math.min(a.zoom, b.zoom);
  if (lo <= 0 || hi / lo > SAME_ZOOM_RATIO) return false;
  return Math.abs(a.scrollX - b.scrollX) <= SAME_SCROLL_PX && Math.abs(a.scrollY - b.scrollY) <= SAME_SCROLL_PX;
}

/** True when `to` is a zoom change large enough to record `from` first. */
export function isLargeZoomChange(from: number, to: number): boolean {
  if (!(from > 0) || !(to > 0)) return false;
  const r = to / from;
  return r >= LARGE_ZOOM_FACTOR || r <= 1 / LARGE_ZOOM_FACTOR;
}

export class ViewHistory {
  #back: ViewEntry[] = [];
  #forward: ViewEntry[] = [];
  readonly #cap: number;

  constructor(cap: number = MAX_VIEW_HISTORY) {
    this.#cap = Math.max(1, Math.floor(cap));
  }

  get backCount(): number { return this.#back.length; }
  get forwardCount(): number { return this.#forward.length; }
  get canBack(): boolean { return this.#back.length > 0; }
  get canForward(): boolean { return this.#forward.length > 0; }

  /** Remember `current` as the place to return to, before leaving it. Clears the forward stack. */
  record(current: ViewEntry): void {
    const top = this.#back[this.#back.length - 1];
    if (!top || !sameView(top, current)) {
      this.#back.push({ ...current });
      if (this.#back.length > this.#cap) this.#back.shift();
    }
    this.#forward = [];
  }

  /** Step back: returns the view to show, remembering `current` for Next view. Null when empty. */
  back(current: ViewEntry): ViewEntry | null {
    const target = this.#back.pop();
    if (!target) return null;
    this.#forward.push({ ...current });
    if (this.#forward.length > this.#cap) this.#forward.shift();
    return target;
  }

  /** Step forward: returns the view to show, remembering `current` for Previous view. Null when empty. */
  forward(current: ViewEntry): ViewEntry | null {
    const target = this.#forward.pop();
    if (!target) return null;
    this.#back.push({ ...current });
    if (this.#back.length > this.#cap) this.#back.shift();
    return target;
  }

  clear(): void {
    this.#back = [];
    this.#forward = [];
  }
}
