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

// ---------------------------------------------------------------------------
// Text search types + commands (M4 S3)
// ---------------------------------------------------------------------------

/**
 * A single text-search hit on one page.
 * `rect` is [left, bottom, right, top] in PDF user-space points (y-up, same
 * coordinate system as markups / §5 invariant).
 */
export interface SearchHit {
  page: number;
  /** [left, bottom, right, top] in PDF user-space points. */
  rect: [number, number, number, number];
  snippet: string;
}

/**
 * Search for all occurrences of `query` across every page of an open document.
 * Returns hits ordered by page then occurrence, or an empty array on no match.
 * Returns a rejected promise if `docId` is unknown or PDFium fails.
 */
export async function searchDocument(
  docId: string,
  query: string,
  caseSensitive = false,
  wholeWord = false
): Promise<SearchHit[]> {
  return invoke<SearchHit[]>("search_document", {
    docId,
    query,
    caseSensitive,
    wholeWord,
  });
}

// ---------------------------------------------------------------------------
// Version snapshot types + commands (M4 S2, spec §15/§18)
// ---------------------------------------------------------------------------

/** A persisted version snapshot record (mirrors Rust VersionRecord in sidecar/meta.rs). */
export interface VersionRecord {
  id: string;
  created_at: string; // RFC3339
  label: string | null;
  filename: string;
}

/**
 * Save a version snapshot of the open document before overwriting.
 * Call this before `saveDocument` to capture the pre-save state.
 */
export async function snapshotVersion(
  doc_id: string,
  label: string | null
): Promise<VersionRecord> {
  return invoke<VersionRecord>("snapshot_version", { docId: doc_id, label });
}

/** List version records for the open document, newest first. */
export async function listDocumentVersions(doc_id: string): Promise<VersionRecord[]> {
  return invoke<VersionRecord[]>("list_document_versions", { docId: doc_id });
}

/**
 * Restore a version snapshot back over the live PDF.
 * The render engine is reloaded automatically so tiles refresh.
 */
export async function restoreDocumentVersion(
  doc_id: string,
  version_id: string
): Promise<void> {
  return invoke<void>("restore_document_version", {
    docId: doc_id,
    versionId: version_id,
  });
}

// ---------------------------------------------------------------------------
// Folder full-text search types + commands (M4 S4)
// ---------------------------------------------------------------------------

/**
 * A single result from a folder-wide Tantivy search.
 *
 * `snippet` is an HTML string — matched terms are wrapped in `<b>` tags.
 * Render with `{@html hit.snippet}` in Svelte; the content is generated by
 * Tantivy's SnippetGenerator (no user-supplied HTML, safe to render).
 */
export interface FolderSearchHit {
  file_path: string;
  /** 1-based page number. */
  page_number: number;
  /** HTML snippet with matched terms in <b> tags. */
  snippet: string;
  /** Text extraction source: "lopdf" | "ocr" | "pdfium". */
  source: string;
}

/** Current state of the background folder indexer. */
export type IndexState =
  | { kind: "Idle" }
  | { kind: "Indexing"; current_file: string; progress: number }
  | { kind: "Error"; message: string };

/** Status returned by `getFolderIndexStatus`. */
export interface IndexStatus {
  folder_path: string;
  indexed_files: number;
  indexed_pages: number;
  state: IndexState;
}

/**
 * Open (or reopen) the Tantivy folder index for `folderPath`.
 *
 * Starts a background indexer that crawls all PDFs in the folder and sets
 * up an incremental file watcher.  Poll `getFolderIndexStatus` for progress.
 * Returns the initial status (indexed_files = 0, state = Indexing).
 */
export async function openFolderIndex(folderPath: string): Promise<IndexStatus> {
  return invoke<IndexStatus>("open_folder_index", { folderPath });
}

/**
 * Search the active folder index for `query`.
 *
 * Returns up to `limit` hits (default 50) sorted by relevance.
 * Rejects with an error string if no folder index has been opened yet.
 */
export async function searchFolder(
  query: string,
  limit = 50
): Promise<FolderSearchHit[]> {
  return invoke<FolderSearchHit[]>("search_folder", { query, limit });
}

/**
 * Return the current indexing status (indexed file / page counts + state).
 *
 * Safe to call before `openFolderIndex`; returns an empty idle status in
 * that case.
 */
export async function getFolderIndexStatus(): Promise<IndexStatus> {
  return invoke<IndexStatus>("folder_index_status");
}

// ---------------------------------------------------------------------------
// DocOps commands (M5 — flatten / optimize / redact, spec §8)
// ---------------------------------------------------------------------------

/**
 * Flatten all annotation appearance streams in the open document into page
 * content.  After completion the annotations are baked into the page and are
 * no longer selectable/editable via Redline or any PDF viewer.
 *
 * The Tauri backend atomically rewrites the file and reloads the render engine,
 * so the viewport updates automatically after this call returns.
 *
 * Returns a rejected promise on backend error (unknown doc_id, lopdf parse
 * failure, or atomic-save failure).
 */
export async function flattenDocument(docId: string): Promise<void> {
  return invoke<void>("flatten_document", { docId });
}

/**
 * Optimize the open document by pruning unreferenced objects and (at level 2)
 * Deflate-compressing all compressable streams.
 *
 * Level semantics:
 *   0 — no-op passthrough
 *   1 — prune unused objects only (lossless, fastest)
 *   2 — prune + compress streams (default for the UI "Optimize" action)
 *
 * Deep image downsampling is out of scope for the v1 baseline (spec §8).
 *
 * The Tauri backend atomically rewrites the file and reloads the render engine,
 * so the viewport updates automatically after this call returns.
 *
 * Returns a rejected promise on backend error (unknown doc_id, lopdf parse
 * failure, or atomic-save failure).
 */
export async function optimizeDocument(
  docId: string,
  level: number = 2,
): Promise<void> {
  return invoke<void>("optimize_document", { docId, level });
}
