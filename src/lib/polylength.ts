/**
 * Polylength rise/drop, split, and scale recompute (docs/count-polylength.md).
 * Pure math + before/after builders; mirrors src-tauri/src/takeoff/math.rs (both must
 * agree). `raw_measure` stays the scale-independent path length in PDF points;
 * `computed_quantity = raw * ratio + sum(|rise/drop| converted to the scale unit)`.
 */
import type { Markup, MeasurementPayload, PdfPoint, RiseDrop, ScaleRecord, UserRef } from "./ipc";
import { bumpAudit } from "./markup-tools";
import { measureLength } from "./measurement-tools";

/** Hard limits (mirror Rust MAX_POLYLENGTH_VERTICES / MAX_RISE_DROP_AMOUNT). */
export const MAX_POLYLENGTH_VERTICES = 10_000;
export const MAX_RISE_DROP_AMOUNT = 1e9;

const METRES_PER_UNIT: Record<string, number> = {
  mm: 0.001, cm: 0.01, m: 1, km: 1000, in: 0.0254, '"': 0.0254, ft: 0.3048, "'": 0.3048, yd: 0.9144,
};

function norm(u: string): string {
  return u.trim().toLowerCase();
}

/** Convert a length between units; `null` when the pair is unknown (same unit passes through). */
export function convertLength(amount: number, from: string, to: string): number | null {
  if (norm(from) === norm(to)) return amount;
  const f = METRES_PER_UNIT[norm(from)];
  const t = METRES_PER_UNIT[norm(to)];
  if (f === undefined || t === undefined) return null;
  return (amount * f) / t;
}

/** Sum of rise/drop magnitudes in `unit`; runs that cannot be converted are counted, not guessed. */
export function riseDropTotal(runs: RiseDrop[] | undefined, unit: string): { total: number; unconverted: number } {
  let total = 0;
  let unconverted = 0;
  for (const r of runs ?? []) {
    const v = Number.isFinite(r.amount) ? convertLength(Math.abs(r.amount), r.unit, unit) : null;
    if (v === null || !Number.isFinite(v)) unconverted++;
    else total += v;
  }
  return { total, unconverted };
}

/** True for a length unit the conversion table knows (so a rise/drop in it can be converted). */
export function isKnownLengthUnit(u: string): boolean {
  return METRES_PER_UNIT[norm(u)] !== undefined;
}

/**
 * The scale a measurement belongs to: its own `scale_ref` when that scale still exists
 * (a deliberately pinned scale is never re-pointed), else the page's effective scale.
 */
export function scaleFor(m: Markup, scales: ScaleRecord[]): ScaleRecord | null {
  const ref = m.measurement?.scale_ref;
  return (ref ? scales.find((s) => s.id === ref) : undefined) ?? effectiveScale(scales, m.page);
}

/**
 * Quantity for a polylength with `raw` path points and `runs`, derived from the
 * measurement's OWN data (no scale needed): ratio = (computed - its rise/drop) / raw, in the
 * measurement's own unit. Used when no scale record is available (e.g. the sidecar scale is
 * gone) so split and rise/drop never copy or leave a stale total. Halves of a split sum
 * exactly to the original.
 */
export function quantityFromOwnData(meas: MeasurementPayload, raw: number, runs: RiseDrop[]): number {
  const vOld = riseDropTotal(meas.rise_drop, meas.unit).total;
  const ratio = meas.raw_measure > 0 ? (meas.computed_quantity - vOld) / meas.raw_measure : 0;
  return raw * ratio + riseDropTotal(runs, meas.unit).total;
}

/** Per-segment lengths of a polyline, in PDF points. */
export function segmentLengths(pts: PdfPoint[]): number[] {
  const out: number[] = [];
  for (let i = 1; i < pts.length; i++) out.push(measureLength([pts[i - 1], pts[i]]));
  return out;
}

/** Measurement payload for a polylength under `scale` (null scale = unscaled points). */
export function polylengthPayload(
  pts: PdfPoint[],
  scale: ScaleRecord | null,
  runs: RiseDrop[] = [],
): MeasurementPayload {
  const raw = measureLength(pts);
  const vertical = scale ? riseDropTotal(runs, scale.unit).total : 0;
  return {
    scale_ref: scale?.id ?? null,
    raw_measure: raw,
    unit: scale?.unit ?? "pt",
    computed_quantity: scale ? raw * scale.ratio + vertical : 0,
    depth: null,
    count_value: null,
    custom_columns: {},
    rise_drop: runs,
  };
}

/** Recompute one measurement markup under `scale` (clone; input untouched). */
export function recomputeMeasurement(m: Markup, scale: ScaleRecord): Markup {
  const meas = m.measurement;
  if (!meas || !Number.isFinite(scale.ratio) || scale.ratio <= 0) return m;
  const r = scale.ratio;
  let quantity: number;
  let unit = scale.unit;
  switch (m.markup_type) {
    case "MeasurementLength":
      quantity = meas.raw_measure * r + riseDropTotal(meas.rise_drop, scale.unit).total;
      break;
    case "MeasurementRadius":
    case "MeasurementPerimeter":
      quantity = meas.raw_measure * r;
      break;
    case "MeasurementArea":
      quantity = meas.raw_measure * r * r;
      break;
    case "MeasurementVolume":
      quantity = meas.raw_measure * r * r * (meas.depth ?? 1);
      unit = `${scale.unit}³`;
      break;
    default:
      return m; // angle and count are scale-independent
  }
  return { ...m, measurement: { ...meas, computed_quantity: quantity, unit, scale_ref: scale.id } };
}

/** The scale that applies to a page: page-specific first, else the document default. */
export function effectiveScale(scales: ScaleRecord[], page: number): ScaleRecord | null {
  return (
    scales.find((s) => s.applies_to.kind === "Page" && s.applies_to.page === page) ??
    scales.find((s) => s.applies_to.kind === "DocumentDefault") ??
    null
  );
}

/**
 * Before/after pairs bringing every scaled measurement under the scale that applies to
 * its page. A measurement taken under a different scale that still exists keeps it (the
 * user chose it); one with no scale or a replaced/dangling `scale_ref` is recomputed.
 * Unchanged markups are omitted.
 */
export function recomputeAll(
  markups: Markup[],
  scales: ScaleRecord[],
  by: UserRef,
  now: string,
): { before: Markup; after: Markup }[] {
  const out: { before: Markup; after: Markup }[] = [];
  for (const m of markups) {
    if (!m.measurement) continue;
    const scale = effectiveScale(scales, m.page);
    if (!scale) continue;
    const ref = m.measurement.scale_ref;
    if (ref && ref !== scale.id && scales.some((s) => s.id === ref)) continue;
    const next = recomputeMeasurement(m, scale);
    if (next === m) continue;
    const a = next.measurement!;
    const b = m.measurement;
    if (a.computed_quantity === b.computed_quantity && a.unit === b.unit && a.scale_ref === b.scale_ref) continue;
    out.push({ before: m, after: bumpAudit(next, by, now) });
  }
  return out;
}

export type EditResult = { ok: true; markup: Markup } | { ok: false; error: string };

/** Set (or with amount 0, clear) the rise/drop on one segment, then recompute under `scale`. */
export function setSegmentRiseDrop(
  m: Markup,
  segment: number,
  amount: number,
  unit: string,
  scale: ScaleRecord | null,
  by: UserRef,
  now: string,
): EditResult {
  if (m.markup_type !== "MeasurementLength" || !m.measurement || !("Polyline" in m.geometry)) {
    return { ok: false, error: "Rise/drop applies to length measurements." };
  }
  const segments = m.geometry.Polyline.length - 1;
  if (!Number.isInteger(segment) || segment < 0 || segment >= segments) {
    return { ok: false, error: "No such segment." };
  }
  if (!Number.isFinite(amount) || Math.abs(amount) > MAX_RISE_DROP_AMOUNT) {
    return { ok: false, error: "Enter a number between -1e9 and 1e9." };
  }
  if (!scale && amount !== 0 && !isKnownLengthUnit(m.measurement.unit)) {
    return { ok: false, error: "Calibrate a scale before adding rise/drop." };
  }
  const runUnit = scale ? unit : m.measurement.unit;
  const kept = (m.measurement.rise_drop ?? []).filter((r) => r.segment !== segment);
  const runs = amount === 0 ? kept : [...kept, { segment, amount, unit: runUnit }];
  runs.sort((a, b) => a.segment - b.segment);
  const next: Markup = { ...m, measurement: { ...m.measurement, rise_drop: runs } };
  const out: Markup = scale
    ? recomputeMeasurement(next, scale)
    : {
        ...next,
        measurement: {
          ...next.measurement!,
          computed_quantity: quantityFromOwnData(m.measurement, m.measurement.raw_measure, runs),
        },
      };
  return { ok: true, markup: bumpAudit(out, by, now) };
}

/**
 * Split a polylength at interior vertex `vertex` (1 <= vertex <= n-2). The first half keeps
 * this markup's identity; the second half is a new markup. Rise/drop runs go to the half
 * their segment lands in (the second half re-indexed). Both are recomputed under `scale`.
 */
export function splitPolylengthAt(
  m: Markup,
  vertex: number,
  scale: ScaleRecord | null,
  by: UserRef,
  now: string,
  mintId: () => string,
): { ok: true; first: Markup; second: Markup } | { ok: false; error: string } {
  if (m.markup_type !== "MeasurementLength" || !m.measurement || !("Polyline" in m.geometry)) {
    return { ok: false, error: "Only length measurements can be split." };
  }
  const pts = m.geometry.Polyline;
  if (!Number.isInteger(vertex) || vertex < 1 || vertex > pts.length - 2) {
    return { ok: false, error: "Pick an interior vertex to split at." };
  }
  const runs = m.measurement.rise_drop ?? [];
  const half = (points: PdfPoint[], r: RiseDrop[]): MeasurementPayload => {
    const raw = measureLength(points);
    return {
      ...m.measurement!,
      raw_measure: raw,
      rise_drop: r,
      // Partition the existing total by path length; recomputed under `scale` below when
      // there is one. Never copy the whole total into both halves.
      computed_quantity: quantityFromOwnData(m.measurement!, raw, r),
    };
  };
  const firstPts = pts.slice(0, vertex + 1);
  const secondPts = pts.slice(vertex);
  const firstRuns = runs.filter((r) => r.segment < vertex);
  const secondRuns = runs.filter((r) => r.segment >= vertex).map((r) => ({ ...r, segment: r.segment - vertex }));
  let first: Markup = { ...m, geometry: { Polyline: firstPts }, measurement: half(firstPts, firstRuns) };
  let second: Markup = {
    ...m,
    id: mintId(),
    geometry: { Polyline: secondPts },
    measurement: half(secondPts, secondRuns),
    audit: { ...m.audit, created_by: by, created_at: now, modified_by: by, modified_at: now, revision: 0 },
    workflow: { status: "None", assignee: null, thread: [] },
    group_id: null,
    foreign_keys: [],
    source_nm: null,
    source_rect: null,
    source_ref: null,
    source_popup: null,
  } as Markup;
  if (scale) {
    first = recomputeMeasurement(first, scale);
    second = recomputeMeasurement(second, scale);
  }
  return { ok: true, first: bumpAudit(first, by, now), second };
}
