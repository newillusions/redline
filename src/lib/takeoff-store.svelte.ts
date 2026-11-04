/**
 * Reactive store for scale calibration and active-scale tracking (spec §7 / M3).
 * Svelte 5 runes; no DOM dependencies — fully unit-testable.
 *
 * Calibration state machine (two-point method):
 *   null → waiting_p1 (startCalibration)
 *   waiting_p1 → waiting_p2 (calibrationClickP1)
 *   waiting_p2 → null (calibrationComplete / cancelCalibration)
 */
import type { BBox, PdfPoint, ScaleRecord } from "./ipc";
import { sameSlot, viewportAt, viewportsOn } from "./polylength";

/** What a two-point calibration sets: a new viewport, or an existing viewport's scale. */
export type ViewportCalibration =
  | { kind: "new"; page: number; name: string; bbox: BBox }
  | { kind: "existing"; scaleId: string };

export interface CalibrationParams {
  page: number;
  /** null = DocumentDefault, number = Page-specific */
  appliesToPage: number | null;
  /** Set when calibrating a measurement viewport instead of the page/document. */
  viewport?: ViewportCalibration;
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
  /**
   * Set by the Measurements panel's "Redraw" together with the "viewportRedraw" tool: the next
   * rectangle replaces this viewport's (scale id). Cleared as soon as that tool is left
   * (Viewport.svelte), so a later New viewport never moves an old one.
   */
  redrawViewportId = $state<string | null>(null);

  /**
   * The currently active page/document scale, or null. Never a viewport: a viewport's scale
   * applies only inside its rectangle (see `scaleAt`).
   */
  get activeScale(): ScaleRecord | null {
    if (!this.activeScaleId) return null;
    const s = this.scales.find((x) => x.id === this.activeScaleId) ?? null;
    return s && s.applies_to.kind !== "Viewport" ? s : null;
  }

  /** Every measurement viewport of the document (drawing order). */
  get viewports(): ScaleRecord[] {
    return this.scales.filter((s) => !s.retired && s.applies_to.kind === "Viewport");
  }

  /**
   * Apply a scale change replayed by undo/redo (MarkupStore's `onScale` hook): a record is
   * put in place (a retired one hides the viewport), `null` removes the scale.
   */
  putScale(id: string, rec: ScaleRecord | null): void {
    if (rec) this.addScale(rec);
    else this.deleteScale(id);
  }

  /** The viewports of one page, in drawing order. */
  viewportsOn(page: number): ScaleRecord[] {
    return viewportsOn(this.scales, page);
  }

  /**
   * The scale a NEW measurement whose first point is `p` on `page` is taken under: the
   * viewport containing the point (last-drawn wins where they overlap), else the active
   * page/document scale. docs/measurement-viewports.md.
   */
  scaleAt(page: number, p: PdfPoint | null | undefined): ScaleRecord | null {
    return (p ? viewportAt(this.scales, page, p) : null) ?? this.activeScale;
  }

  /** Populate from the backend on doc open. Activates the first page/document scale. */
  seedScales(records: ScaleRecord[]): void {
    this.scales = records;
    if (this.activeScaleId && !records.some((s) => s.id === this.activeScaleId)) this.activeScaleId = null;
    if (!this.activeScaleId) {
      this.activeScaleId = records.find((s) => s.applies_to.kind !== "Viewport")?.id ?? null;
    }
  }

  /**
   * Insert or replace a scale record. A new calibration of the same target (same page,
   * viewport, or the document default) REPLACES the old record in place, as the backend's
   * `add_scale` does - otherwise the old scale would linger and shadow the new one. A
   * page/document scale becomes the active one; a viewport never does.
   */
  addScale(rec: ScaleRecord): void {
    const hit = (s: ScaleRecord) => s.id === rec.id || sameSlot(s.applies_to, rec.applies_to);
    const idx = this.scales.findIndex(hit);
    if (idx >= 0) {
      this.scales = this.scales.filter((s, i) => i === idx || !hit(s));
      this.scales[idx] = rec;
    } else {
      this.scales = [...this.scales, rec];
    }
    if (rec.applies_to.kind !== "Viewport") this.activeScaleId = rec.id;
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
