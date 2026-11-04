/**
 * Defensive sanitiser for search-snippet HTML rendered via `{@html}`.
 *
 * Backend contract: folder and Recents snippets are HTML-escaped text with the
 * matched term wrapped in `<b>` (Tantivy `to_html()` and `find_snippet`).
 * This is the second line of defence: only bare `<b>`, `</b>`, `<mark>` and
 * `</mark>` tags (no attributes) survive; every other `<` or `>` is escaped,
 * so no element, attribute or event handler can be injected.
 */
const ALLOWED_TAG = /<\/?(?:b|mark)>/gi;

function escapeAngles(text: string): string {
  return text.replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function sanitizeSnippetHtml(html: string): string {
  let out = "";
  let last = 0;
  for (const m of html.matchAll(ALLOWED_TAG)) {
    const at = m.index ?? 0;
    out += escapeAngles(html.slice(last, at)) + m[0].toLowerCase();
    last = at + m[0].length;
  }
  return out + escapeAngles(html.slice(last));
}
