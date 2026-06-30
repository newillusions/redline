/**
 * Most-Recently-Used document list — client-side helpers + IPC wrappers.
 *
 * Separation of concerns:
 *   - `upsertMru` is a pure function (testable, no Tauri dependency).
 *   - `loadRecentDocs` / `saveRecentDocs` / `checkFileExists` wrap Tauri invoke.
 *
 * The list is persisted via the Rust `load_recent_docs` / `save_recent_docs`
 * commands, which write to `<app-data-dir>/recent-docs.json`.
 */
import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------------------
// Types (mirror Rust MruEntry)
// ---------------------------------------------------------------------------

export interface RecentDoc {
  /** Absolute path to the PDF file. */
  path: string;
  /** Filename component only (e.g. "floor-plan.pdf"). */
  file_name: string;
  /** RFC3339 timestamp of the last successful open. */
  last_opened: string;
  /** Page count at time of open (optional — available from DocumentInfo). */
  page_count?: number;
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/** Maximum number of entries the MRU list is allowed to hold. */
export const MAX_RECENT = 20;

// ---------------------------------------------------------------------------
// Pure list helper
// ---------------------------------------------------------------------------

/**
 * Return a new MRU list with `entry` at the front.
 *
 * - If an entry with the same `path` already exists it is removed first (dedup).
 * - The result is capped at `maxItems` (default: `MAX_RECENT`).
 * - The input list is NOT mutated; a new array is returned.
 */
export function upsertMru(
  list: RecentDoc[],
  entry: RecentDoc,
  maxItems: number = MAX_RECENT,
): RecentDoc[] {
  const filtered = list.filter((e) => e.path !== entry.path);
  const updated = [entry, ...filtered];
  return updated.length > maxItems ? updated.slice(0, maxItems) : updated;
}

// ---------------------------------------------------------------------------
// IPC wrappers
// ---------------------------------------------------------------------------

/**
 * Load the MRU list from the Rust backend (persisted in app-data-dir).
 * Returns an empty array if no history has been saved yet.
 */
export async function loadRecentDocs(): Promise<RecentDoc[]> {
  return invoke<RecentDoc[]>("load_recent_docs");
}

/**
 * Persist the MRU list to the Rust backend.
 * The caller is responsible for calling `upsertMru` before this to ensure
 * the list is sorted and capped.
 */
export async function saveRecentDocs(entries: RecentDoc[]): Promise<void> {
  return invoke<void>("save_recent_docs", { entries });
}

/**
 * Ask the Rust backend whether the file at `path` still exists on disk.
 * Used by the history panel to visually grey out missing files.
 */
export async function checkFileExists(path: string): Promise<boolean> {
  return invoke<boolean>("check_file_exists", { path });
}
