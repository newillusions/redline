/**
 * Reactive store for scale calibration and active-scale tracking (spec §7 / M3).
 * Svelte 5 runes; no DOM dependencies — fully unit-testable.
 *
 * Calibration state machine (two-point method):
 *   null → waiting_p1 (startCalibration)
 *   waiting_p1 → waiting_p2 (calibrationClickP1)
 *   waiting_p2 → null (calibrationComplete / cancelCalibration)
 */
import type { PdfPoint, ScaleRecord } from "./ipc";

export interface CalibrationParams {
  page: number;
  /** null = DocumentDefault, number = Page-specific */
  appliesToPage: number | null;
}

export type CalibrationStep = "waiting_p1" | "waiting_p2";

export interface CalibrationState {
  step: CalibrationStep;
  params: CalibrationParams;
  p1?: PdfPoint;
  p2?: PdfPoint;
}

export class TakeoffStore {
  scales = $state<ScaleRecord[]>([]);
  activeScaleId = $state<string | null>(null);
  calibrationState = $state<CalibrationState | null>(null);

  /** The currently active scale record, or null. */
  get activeScale(): ScaleRecord | null {
    if (!this.activeScaleId) return null;
    return this.scales.find((s) => s.id === this.activeScaleId) ?? null;
  }

  /** Populate from the backend on doc open. Activates the first scale. */
  seedScales(records: ScaleRecord[]): void {
    this.scales = records;
    if (records.length > 0 && !this.activeScaleId) {
      this.activeScaleId = records[0].id;
    }
  }

  /** Insert or replace a scale record. Sets it as active. */
  addScale(rec: ScaleRecord): void {
    const idx = this.scales.findIndex((s) => s.id === rec.id);
    if (idx >= 0) {
      this.scales[idx] = rec;
    } else {
      this.scales = [...this.scales, rec];
    }
    this.activeScaleId = rec.id;
  }

  /** Remove a scale by id. Clears activeScaleId if it was the active one. */
  deleteScale(id: string): void {
    this.scales = this.scales.filter((s) => s.id !== id);
    if (this.activeScaleId === id) {
      this.activeScaleId = this.scales[0]?.id ?? null;
    }
  }

  // --- Calibration state machine ---

  startCalibration(params: CalibrationParams): void {
    this.calibrationState = { step: "waiting_p1", params };
  }

  calibrationClickP1(p: PdfPoint): void {
    if (this.calibrationState?.step !== "waiting_p1") return;
    this.calibrationState = { ...this.calibrationState, step: "waiting_p2", p1: p };
  }

  /**
   * Called when the user clicks the second point. Returns the pixel distance in PDF
   * space for the caller to show the "enter known distance" dialog.
   * Does NOT finalize — call `finalizeCalibration` after the user enters the distance.
   */
  calibrationClickP2(p2: PdfPoint): { p1: PdfPoint; p2: PdfPoint; pixelDist: number } | null {
    if (this.calibrationState?.step !== "waiting_p2" || !this.calibrationState.p1) return null;
    const p1 = this.calibrationState.p1;
    const dx = p2.x - p1.x;
    const dy = p2.y - p1.y;
    const pixelDist = Math.sqrt(dx * dx + dy * dy);
    this.calibrationState = { ...this.calibrationState, p2 };
    return { p1, p2, pixelDist };
  }

  cancelCalibration(): void {
    this.calibrationState = null;
  }
}
