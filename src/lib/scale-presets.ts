/**
 * Drawing-scale presets for measurement viewports (docs/measurement-viewports.md).
 * A preset is a paper:real ratio; `presetRatio` turns it into redline's scale ratio:
 * real-world units per PDF point (1 pt = 1/72 in on paper).
 */

const METRES_PER_POINT = 0.0254 / 72;

const METRES_PER_UNIT: Record<string, number> = { mm: 0.001, cm: 0.01, m: 1, in: 0.0254, ft: 0.3048 };

export interface ScalePreset {
  /** Shown to the user and written as the PDF `/R` scale text. */
  label: string;
  /** Real-world metres per metre on paper (100 for 1:100). */
  factor: number;
  /** Unit the measurements are reported in. */
  unit: string;
}

/** Metric 1:N scales common on lighting and architectural sheets, then imperial ones. */
export const SCALE_PRESETS: ScalePreset[] = [
  ...[1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000].map((n) => ({ label: `1:${n}`, factor: n, unit: "m" })),
  // Architectural imperial: k inches on paper = 1 ft real, i.e. factor 12 / k.
  { label: '1/16" = 1\'-0"', factor: 192, unit: "ft" },
  { label: '1/8" = 1\'-0"', factor: 96, unit: "ft" },
  { label: '1/4" = 1\'-0"', factor: 48, unit: "ft" },
  { label: '1/2" = 1\'-0"', factor: 24, unit: "ft" },
  { label: '1" = 1\'-0"', factor: 12, unit: "ft" },
];

/** Units a preset can report in (all length units the scale math converts). */
export const PRESET_UNITS = ["mm", "cm", "m", "in", "ft"];

/** Real-world `unit` per PDF point for a drawing at 1:`factor`. */
export function presetRatio(factor: number, unit: string): number {
  const mpu = METRES_PER_UNIT[unit];
  if (!mpu || !(factor > 0)) return 0;
  return (factor * METRES_PER_POINT) / mpu;
}

/** What the viewport dialog returns: a preset scale, "calibrate by two points", or (editing) "keep". */
export type ViewportDialogResult =
  | { name: string; mode: "preset"; ratio: number; unit: string; label: string; precision: number }
  | { name: string; mode: "calibrate" }
  | { name: string; mode: "keep" };
