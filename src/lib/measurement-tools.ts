/**
 * Pure measurement math in PDF user space (spec §7). Mirrors the Rust functions
 * in src-tauri/src/takeoff/math.rs — both must produce identical results.
 * No DOM, no Svelte, no clocks. Import and test directly.
 */
import type { PdfPoint, ScaleRecord } from "./ipc";

/**
 * Sum of segment lengths along a polyline, in PDF points (raw_measure for length types).
 */
export function measureLength(pts: PdfPoint[]): number {
  if (pts.length < 2) return 0;
  let total = 0;
  for (let i = 1; i < pts.length; i++) {
    const dx = pts[i].x - pts[i - 1].x;
    const dy = pts[i].y - pts[i - 1].y;
    total += Math.sqrt(dx * dx + dy * dy);
  }
  return total;
}

/**
 * Shoelace area of a polygon, in PDF points² (raw_measure for area types).
 * Works with CW or CCW winding.
 */
export function measureArea(pts: PdfPoint[]): number {
  if (pts.length < 3) return 0;
  let sum = 0;
  const n = pts.length;
  for (let i = 0; i < n; i++) {
    const j = (i + 1) % n;
    sum += pts[i].x * pts[j].y - pts[j].x * pts[i].y;
  }
  return Math.abs(sum) / 2;
}

/**
 * Convert a raw_measure to a display string using the given scale.
 *
 * @param rawMeasure  Scale-independent value in PDF points (length) or points² (area).
 * @param scale       The calibration scale to apply.
 * @param isArea      If true, applies ratio² (area conversion). If false, applies ratio.
 */
export function formatQuantity(rawMeasure: number, scale: ScaleRecord, isArea: boolean): string {
  const factor = isArea ? scale.ratio * scale.ratio : scale.ratio;
  const value = rawMeasure * factor;
  const formatted = value.toFixed(scale.precision);
  const unit = isArea ? `${scale.unit}²` : scale.unit;
  return `${formatted} ${unit}`;
}
