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
  return invoke<void>("close_document", { doc_id });
}

// ---------------------------------------------------------------------------
// Render commands
// ---------------------------------------------------------------------------

export async function renderTile(req: TileRequest): Promise<RenderedTile> {
  return invoke<RenderedTile>("render_tile", { req });
}

export async function getPageCount(doc_id: string): Promise<number> {
  return invoke<number>("get_page_count", { doc_id });
}

export async function getPageSize(
  doc_id: string,
  page_index: number
): Promise<PageSize> {
  return invoke<PageSize>("get_page_size", { doc_id, page_index });
}

// ---------------------------------------------------------------------------
// Diagnostics (in-app §20 bench overlay)
// ---------------------------------------------------------------------------

/** Current process RSS in MB (read in Rust; webview can't see process memory). */
export async function processRssMb(): Promise<number> {
  return invoke<number>("process_rss_mb");
}
