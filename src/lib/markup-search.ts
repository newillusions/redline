/**
 * Client-side text search over already-loaded markup annotations (comments /
 * FreeText contents + subject line) — spec parity item "match and improve on
 * Bluebeam": Bluebeam gates markup-text search behind its higher tiers, this
 * is the differentiator named in the search-parity dispatch.
 *
 * Deliberately scoped to markups the frontend already holds in memory
 * (MarkupStore.markups, loaded via loadMarkups() at document-open time) —
 * covers "current document" and "all open documents" search scope. Folder
 * scope does NOT include markup text (the Tantivy index only carries page
 * text extracted by lopdf, not annotation dictionaries) — named as a
 * follow-up in the PR body rather than half-shipped here.
 */
import type { Markup } from "./ipc";

/** A single markup-content search hit. */
export interface MarkupSearchHit {
  markupId: string;
  /** Zero-based page index (matches Markup.page / SearchHit.page). */
  page: number;
  /** Plain-text snippet with the match's surrounding context (no HTML). */
  snippet: string;
  /** Which field matched — surfaced in the result list as the hit's label. */
  field: "contents" | "subject";
}

const SNIPPET_RADIUS = 60;

function buildSnippet(text: string, matchStart: number, matchLen: number): string {
  const start = Math.max(0, matchStart - SNIPPET_RADIUS);
  const end = Math.min(text.length, matchStart + matchLen + SNIPPET_RADIUS);
  const prefix = start > 0 ? "…" : "";
  const suffix = end < text.length ? "…" : "";
  return prefix + text.slice(start, end).replace(/\s+/g, " ").trim() + suffix;
}

function findMatchIndex(
  haystack: string,
  needle: string,
  caseSensitive: boolean,
  wholeWord: boolean
): number {
  const h = caseSensitive ? haystack : haystack.toLowerCase();
  const n = caseSensitive ? needle : needle.toLowerCase();
  if (!wholeWord) {
    return h.indexOf(n);
  }
  // Whole-word: scan indexOf occurrences and check word boundaries on both sides.
  let from = 0;
  while (from <= h.length) {
    const idx = h.indexOf(n, from);
    if (idx === -1) return -1;
    const before = idx > 0 ? h[idx - 1] : " ";
    const after = idx + n.length < h.length ? h[idx + n.length] : " ";
    const isWordChar = (c: string) => /\w/.test(c);
    if (!isWordChar(before) && !isWordChar(after)) {
      return idx;
    }
    from = idx + 1;
  }
  return -1;
}

/**
 * Search `markups` for `query` across each markup's `contents` (comment text)
 * and `subject` (tool/subject label). Returns hits ordered by page, then by
 * the order markups appear in the input array — mirrors searchDocument's
 * "page then occurrence" contract so callers can merge/sort consistently.
 */
export function searchMarkupContents(
  markups: readonly Markup[],
  query: string,
  caseSensitive = false,
  wholeWord = false
): MarkupSearchHit[] {
  const q = query.trim();
  if (!q) return [];

  const hits: MarkupSearchHit[] = [];
  for (const m of markups) {
    const fields: Array<{ field: MarkupSearchHit["field"]; text: string | null }> = [
      { field: "contents", text: m.contents },
      { field: "subject", text: m.subject },
    ];
    for (const { field, text } of fields) {
      if (!text) continue;
      const idx = findMatchIndex(text, q, caseSensitive, wholeWord);
      if (idx === -1) continue;
      hits.push({
        markupId: m.id,
        page: m.page,
        snippet: buildSnippet(text, idx, q.length),
        field,
      });
    }
  }

  return hits.sort((a, b) => a.page - b.page);
}
