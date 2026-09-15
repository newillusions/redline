/**
 * Pure helpers for the "extract selected pages into a new PDF" feature (owner request
 * 2026-09-15, PR-C). Kept separate from reading-mode.ts and ipc.ts - this is UI-facing
 * formatting/path logic, not viewer layout or an IPC wrapper - and fully unit tested
 * without touching Tauri.
 */

/**
 * Dedupe and sort 0-based page indices ascending - the canonical extraction order.
 * Matches how every other PDF page-extraction tool behaves: the result preserves the
 * document's original page order regardless of the order pages were selected/clicked
 * in (a ctrl-click multi-select can arrive in any order; a shift-click range already
 * happens to be ascending, but this normalizes both the same way).
 */
export function sortedUniquePageIndices(indices: Iterable<number>): number[] {
  return Array.from(new Set(indices)).sort((a, b) => a - b);
}

/**
 * Format 0-based page indices as a human-readable 1-based range string, e.g.
 * `[0, 1, 2, 4, 6, 7]` -> `"1-3, 5, 7-8"`. Sorts and dedupes first (via
 * sortedUniquePageIndices), so caller order/duplicates never affect the result.
 */
export function formatPageRanges(indices: Iterable<number>): string {
  const sorted = sortedUniquePageIndices(indices);
  if (sorted.length === 0) return "";

  const parts: string[] = [];
  let start = sorted[0];
  let prev = sorted[0];

  const flush = () => {
    parts.push(start === prev ? `${start + 1}` : `${start + 1}-${prev + 1}`);
  };

  for (let i = 1; i < sorted.length; i++) {
    const cur = sorted[i];
    if (cur === prev + 1) {
      prev = cur;
      continue;
    }
    flush();
    start = cur;
    prev = cur;
  }
  flush();

  return parts.join(", ");
}

/**
 * Suggest a destination path for an extracted PDF, alongside the source document:
 * `/a/b/plan.pdf` -> `/a/b/plan-extracted.pdf`. Handles both `/` and `\` separators (the
 * app runs on macOS and Windows) and a source with no extension or no directory.
 */
export function suggestExtractedPath(sourcePath: string): string {
  const sepIdx = Math.max(sourcePath.lastIndexOf("/"), sourcePath.lastIndexOf("\\"));
  const dir = sepIdx >= 0 ? sourcePath.slice(0, sepIdx + 1) : "";
  const base = sepIdx >= 0 ? sourcePath.slice(sepIdx + 1) : sourcePath;
  const dotIdx = base.lastIndexOf(".");
  const stem = dotIdx > 0 ? base.slice(0, dotIdx) : base;
  const ext = dotIdx > 0 ? base.slice(dotIdx) : ".pdf";
  return `${dir}${stem}-extracted${ext}`;
}
