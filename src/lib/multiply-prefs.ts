/**
 * The Multiply dialog's distance meaning (centre to centre or gap), remembered per viewer like
 * the other tool prefs ($lib/viewport.ts toolbar prefs, $lib/search-store.svelte.ts scope).
 * Never throws: a missing, corrupt or blocked store just yields the default.
 */
import type { MultiplyMeasure } from "./markup-arrange";

export const MULTIPLY_MEASURE_STORAGE_KEY = "redline.multiply.measure";
export const DEFAULT_MULTIPLY_MEASURE: MultiplyMeasure = "centre";

export function loadMultiplyMeasure(): MultiplyMeasure {
  try {
    if (typeof localStorage === "undefined") return DEFAULT_MULTIPLY_MEASURE;
    const raw = localStorage.getItem(MULTIPLY_MEASURE_STORAGE_KEY);
    if (!raw) return DEFAULT_MULTIPLY_MEASURE;
    const v: unknown = JSON.parse(raw);
    return v === "gap" || v === "centre" ? v : DEFAULT_MULTIPLY_MEASURE;
  } catch {
    return DEFAULT_MULTIPLY_MEASURE;
  }
}

export function persistMultiplyMeasure(measure: MultiplyMeasure): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(MULTIPLY_MEASURE_STORAGE_KEY, JSON.stringify(measure));
  } catch {
    // best-effort
  }
}
