/**
 * IPC bridge — typed wrappers around Tauri invoke calls.
 *
 * All render/document operations go through here.
 * Matches the Rust command signatures in src-tauri/src/commands/.
 */
import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------------------
// Types (mirrors Rust structs)
// ---------------------------------------------------------------------------

export interface DocumentInfo {
  doc_id: string;
  path: string;
  page_count: number;
}

export interface PageSize {
  doc_id: string;
  page_index: number;
  width_pts: number;
  height_pts: number;
}

export interface TileRequest {
  doc_id: string;
  page_index: number;
  tile_size_css: number;
  tile_x: number;
  tile_y: number;
  zoom: number;
  dpr: number;
}

export interface RenderedTile {
  doc_id: string;
  page_index: number;
  tile_x: number;
  tile_y: number;
  width_px: number;
  height_px: number;
  zoom: number;
  dpr: number;
  png_base64: string;
  render_ms: number;
}

// ---------------------------------------------------------------------------
// Document commands
// ---------------------------------------------------------------------------

export async function openDocument(path: string): Promise<DocumentInfo> {
  return invoke<DocumentInfo>("open_document", { path });
}

export async function closeDocument(doc_id: string): Promise<void> {
  return invoke<void>("close_document", { docId: doc_id });
}

// ---------------------------------------------------------------------------
// Render commands
// ---------------------------------------------------------------------------

export async function renderTile(req: TileRequest): Promise<RenderedTile> {
  return invoke<RenderedTile>("render_tile", { req });
}

export async function getPageCount(doc_id: string): Promise<number> {
  return invoke<number>("get_page_count", { docId: doc_id });
}

export async function getPageSize(
  doc_id: string,
  page_index: number
): Promise<PageSize> {
  return invoke<PageSize>("get_page_size", { docId: doc_id, pageIndex: page_index });
}

// ---------------------------------------------------------------------------
// Diagnostics (in-app §20 bench overlay)
// ---------------------------------------------------------------------------

/** Current process RSS in MB (read in Rust; webview can't see process memory). */
export async function processRssMb(): Promise<number> {
  return invoke<number>("process_rss_mb");
}

// ---------------------------------------------------------------------------
// Markup types (mirrors src-tauri/src/markup/mod.rs serde JSON shapes)
// ---------------------------------------------------------------------------

export interface PdfPoint {
  x: number;
  y: number;
}

export interface UserRef {
  user_id: string;
  display_name: string;
}

export type MarkupType =
  | "Text"
  | "Callout"
  | "Cloud"
  | "Rectangle"
  | "Ellipse"
  | "Polygon"
  | "Line"
  | "Polyline"
  | "Arrow"
  | "Highlight"
  | "Ink"
  | "Stamp"
  | "StampDynamic"
  | "MeasurementLength"
  | "MeasurementPerimeter"
  | "MeasurementArea"
  | "MeasurementVolume"
  | "MeasurementCount"
  | "MeasurementAngle"
  | "MeasurementRadius";

export type MarkupGeometry =
  | { Point: PdfPoint }
  | { Rect: { min: PdfPoint; max: PdfPoint } }
  | { Polyline: PdfPoint[] }
  | { Ink: PdfPoint[][] };

export interface Appearance {
  color: string;
  line_weight: number;
  opacity: number;
  fill: string | null;
  line_style: "Solid" | "Dashed" | "Dotted";
  font: { family: string; size_pt: number } | null;
}

export interface MarkupAudit {
  created_by: UserRef;
  created_at: string; // RFC3339
  modified_by: UserRef;
  modified_at: string;
  revision: number;
  origin: "Desktop" | "FieldApp";
}

export interface MarkupWorkflow {
  status: "None" | "Accepted" | "Rejected" | "Completed";
  assignee: UserRef | null;
  thread: unknown[];
}

export interface Markup {
  id: string;
  markup_type: MarkupType;
  page: number;
  geometry: MarkupGeometry;
  appearance: Appearance;
  subject: string | null;
  layer: string | null;
  contents: string | null;
  /** Flat group membership (G8). Markups sharing the same non-null group_id move together. */
  group_id: string | null;
  audit: MarkupAudit;
  workflow: MarkupWorkflow;
  measurement: MeasurementPayload | null;
}

// ---------------------------------------------------------------------------
// Markup + save commands
// ---------------------------------------------------------------------------

// addMarkup/listMarkups: consumed by the S2 markup-authoring UI; backend commands already live.
export async function addMarkup(doc_id: string, markup: Markup): Promise<void> {
  return invoke<void>("add_markup", { docId: doc_id, markup });
}

export async function listMarkups(doc_id: string): Promise<Markup[]> {
  return invoke<Markup[]>("list_markups", { docId: doc_id });
}

/** Pull existing PDF annotations into the store (call once after open). */
export async function loadMarkups(doc_id: string): Promise<Markup[]> {
  return invoke<Markup[]>("load_markups", { docId: doc_id });
}

export async function saveDocument(doc_id: string): Promise<void> {
  return invoke<void>("save_document", { docId: doc_id });
}

export async function saveDocumentAs(doc_id: string, new_path: string): Promise<void> {
  return invoke<void>("save_document_as", { docId: doc_id, newPath: new_path });
}

export async function updateMarkup(doc_id: string, markup: Markup): Promise<void> {
  return invoke<void>("update_markup", { docId: doc_id, markup });
}

export async function deleteMarkup(doc_id: string, markup_id: string): Promise<void> {
  return invoke<void>("delete_markup", { docId: doc_id, markupId: markup_id });
}

/** Persisted app user identity (generated on first run). */
export async function getUserIdentity(): Promise<UserRef> {
  return invoke<UserRef>("get_user_identity");
}

// ---------------------------------------------------------------------------
// Takeoff / scale types (mirrors Rust src-tauri/src/takeoff/scale.rs)
// ---------------------------------------------------------------------------

export type ScaleTarget =
  | { kind: "Page"; page: number }
  | { kind: "DocumentDefault" };

export type ScaleMethod = "TwoPoint" | "Preset";

export interface ScaleRecord {
  id: string;
  applies_to: ScaleTarget;
  method: ScaleMethod;
  /** Real-world units per PDF point (multiply raw_measure by this). */
  ratio: number;
  unit: string;
  label: string;
  precision: number;
}

export interface MeasurementPayload {
  scale_ref: string | null;
  raw_measure: number;
  unit: string;
  computed_quantity: number;
  depth: number | null;
  count_value: number | null;
  custom_columns: Record<string, string>;
}

export type ExportFormat = "Xlsx" | "Csv";

// ---------------------------------------------------------------------------
// Takeoff IPC wrappers
// ---------------------------------------------------------------------------

/** Add (or replace) a calibration scale for the document. Returns the created record. */
export async function addScale(
  doc_id: string,
  appliesToPage: number | null,
  ratio: number,
  unit: string,
  label: string,
  precision: number
): Promise<ScaleRecord> {
  return invoke<ScaleRecord>("add_scale", {
    docId: doc_id,
    appliesToPage,
    ratio,
    unit,
    label,
    precision,
  });
}

/** List all scale records for the document. */
export async function listScales(doc_id: string): Promise<ScaleRecord[]> {
  return invoke<ScaleRecord[]>("list_scales", { docId: doc_id });
}

/** Delete a scale by id. Returns true if found. */
export async function deleteScale(doc_id: string, scale_id: string): Promise<boolean> {
  return invoke<boolean>("delete_scale", { docId: doc_id, scaleId: scale_id });
}

/** Export the markup list to XLSX or CSV. `path` is the absolute output file path. */
export async function exportMarkupList(
  doc_id: string,
  path: string,
  format: ExportFormat
): Promise<void> {
  return invoke<void>("export_markup_list", { docId: doc_id, path, format });
}

// ---------------------------------------------------------------------------
// Page operation commands (M4 S1)
// ---------------------------------------------------------------------------

export interface RotatePageArgs {
  doc_id: string;
  page_idx: number;
  degrees: number;
}

export interface DeletePageArgs {
  doc_id: string;
  page_idx: number;
}

export interface ReorderPagesArgs {
  doc_id: string;
  new_order: number[];
}

export interface InsertBlankPageArgs {
  doc_id: string;
  at: number;
  width: number;
  height: number;
}

/** Rotate a page by `degrees` (multiple of 90, incremental). 0-based page index. */
export async function rotatePage(args: RotatePageArgs): Promise<void> {
  return invoke<void>("rotate_page", {
    docId: args.doc_id,
    pageIdx: args.page_idx,
    degrees: args.degrees,
  });
}

/** Delete a page (0-based index). Errors if the document has only one page. */
export async function deletePage(args: DeletePageArgs): Promise<void> {
  return invoke<void>("delete_page", {
    docId: args.doc_id,
    pageIdx: args.page_idx,
  });
}

/** Reorder pages. `new_order` is a permutation of `0..pageCount` (0-based). */
export async function reorderPages(args: ReorderPagesArgs): Promise<void> {
  return invoke<void>("reorder_pages", {
    docId: args.doc_id,
    newOrder: args.new_order,
  });
}

/** Insert a blank page at position `at` (0-based). `at == pageCount` appends. */
export async function insertBlankPage(args: InsertBlankPageArgs): Promise<void> {
  return invoke<void>("insert_blank_page", {
    docId: args.doc_id,
    at: args.at,
    width: args.width,
    height: args.height,
  });
}
