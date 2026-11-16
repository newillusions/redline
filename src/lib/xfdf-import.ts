/**
 * XFDF import: the IPC wrapper, the file picker and the one-line summary.
 *
 * Kept out of `ipc.ts` (a hot shared file). The backend only builds the markups (nothing is
 * added to its store); the caller adds them with `MarkupStore.createMany`, so the whole import
 * is ONE undo step.
 */
import { invoke } from "@tauri-apps/api/core";
import type { Markup } from "./ipc";

/** Mirror of Rust `XfdfImport` (camelCase). */
export interface XfdfImportResult {
  markups: Markup[];
  /** Skipped: the document already has a markup with that name (or the file repeats it). */
  skippedExisting: number;
  /** Not imported: unsupported element, page out of range, bad numbers, orphaned reply. */
  ignored: number;
  /** Imported as the plain shape because the file cannot carry the payload (measurement, legend). */
  simplified: number;
  /** Malformed annotations the parser skipped (first 20): "annotation N: reason". */
  warnings?: string[];
}

/** Read `path` and build the markups it would add to `docId` (nothing is added yet). */
export async function importMarkupsXfdf(docId: string, path: string, pageCount: number): Promise<XfdfImportResult> {
  return invoke<XfdfImportResult>("import_markups_xfdf", { docId, path, pageCount });
}

/** Ask for an XFDF file; `null` when cancelled. The dialog plugin is loaded on use. */
export async function pickXfdfFile(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Import XFDF comments",
    multiple: false,
    directory: false,
    filters: [{ name: "XFDF comments", extensions: ["xfdf"] }],
  });
  return typeof picked === "string" ? picked : null;
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** One line for the status bar after an import. */
export function importSummary(r: XfdfImportResult): string {
  const parts = [`Imported ${plural(r.markups.length, "mark-up", "mark-ups")}`];
  if (r.skippedExisting > 0) parts.push(`${r.skippedExisting} already in the document`);
  if (r.simplified > 0) parts.push(`${r.simplified} as plain shapes`);
  if (r.ignored > 0) parts.push(`${r.ignored} not imported`);
  return parts.join("; ");
}
