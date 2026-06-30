/**
 * Pure measurement math in PDF user space (spec §7). Mirrors the Rust functions
 * in src-tauri/src/takeoff/math.rs — both must produce identical results.
 * No DOM, no Svelte, no clocks. Import and test directly.
 */
import type { PdfPoint, ScaleRecord, Markup, CountSymbol } from "./ipc";

/** A per-set count subtotal row for the measurement panel. */
export interface CountSubtotal {
  /** Set id, or `null` for count markers with no set assignment. */
  setId: string | null;
  name: string;
  color: string;
  symbol: CountSymbol;
  /** Summed count_value across this set's MeasurementCount markups. */
  count: number;
}

/**
 * Group MeasurementCount markups by their count set and sum count_value per group.
 * Markers with no set fall into a single "Unassigned" group (setId null). Groups are
 * returned in first-seen order so the panel is stable. Pure — unit-tested in isolation.
 */
export function countSubtotals(markups: Markup[]): CountSubtotal[] {
  const groups = new Map<string | null, CountSubtotal>();
  for (const m of markups) {
    if (m.markup_type !== "MeasurementCount") continue;
    const cs = m.count_set ?? null;
    const key = cs?.id ?? null;
    let row = groups.get(key);
    if (!row) {
      row = {
        setId: key,
        name: cs?.name ?? "Unassigned",
        color: cs?.color ?? m.appearance.color,
        symbol: cs?.symbol ?? "Circle",
        count: 0,
      };
      groups.set(key, row);
    }
    row.count += m.measurement?.count_value ?? 0;
  }
  return [...groups.values()];
}

/** One page's tally within a count set. */
export interface CountPageBreakdown {
  /** 0-based page index. */
  page: number;
  count: number;
}

/** Per-set count row with an additional per-page breakdown. */
export interface CountSubtotalWithPages extends CountSubtotal {
  /** Per-page count tallies, sorted ascending by page. */
  byPage: CountPageBreakdown[];
}

/**
 * Like `countSubtotals` but also breaks each set's total down by page.
 * Returns one entry per set (including an "Unassigned" entry for markers with
 * no set), with `byPage` sorted ascending. Pure — unit-tested in isolation.
 */
export function countSubtotalsByPage(markups: Markup[]): CountSubtotalWithPages[] {
  const groups = new Map<string | null, CountSubtotalWithPages>();
  for (const m of markups) {
    if (m.markup_type !== "MeasurementCount") continue;
    const cs = m.count_set ?? null;
    const key = cs?.id ?? null;
    let row = groups.get(key);
    if (!row) {
      row = {
        setId: key,
        name: cs?.name ?? "Unassigned",
        color: cs?.color ?? m.appearance.color,
        symbol: cs?.symbol ?? "Circle",
        count: 0,
        byPage: [],
      };
      groups.set(key, row);
    }
    const val = m.measurement?.count_value ?? 0;
    row.count += val;
    const existing = row.byPage.find((bp) => bp.page === m.page);
    if (existing) {
      existing.count += val;
    } else {
      row.byPage.push({ page: m.page, count: val });
    }
  }
  for (const row of groups.values()) {
    row.byPage.sort((a, b) => a.page - b.page);
  }
  return [...groups.values()];
}

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
