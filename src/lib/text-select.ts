/**
 * I-beam text-selection tool: IPC wrappers + pure gesture math. A dedicated file
 * (mirrors the recent-docs.ts pattern) so the text-selection feature does not
 * touch ipc.ts's command-wrapper section beyond the one-line MarkupGeometry
 * addition - keeps the conflict surface small while another branch works on
 * the open/save/password path.
 *
 * PDFium is !Send + !Sync and lives on redline's dedicated render thread; these
 * commands are thin passthroughs (see src-tauri/src/commands/text_select.rs).
 */
import { invoke } from "@tauri-apps/api/core";
import { writeText as pluginWriteText } from "@tauri-apps/plugin-clipboard-manager";
import type { PdfPoint } from "./ipc";

// ---------------------------------------------------------------------------
// Types (mirror Rust geometry::Quad / text::TextRangeSelection)
// ---------------------------------------------------------------------------

/**
 * A single quadrilateral in PDF user space, 4 points ordered top-left,
 * top-right, bottom-left, bottom-right. Mirrors Rust `geometry::Quad` - see
 * that type's doc comment for why this order (the de-facto Acrobat/Bluebeam
 * `/QuadPoints` convention, not the PDF spec's literal wording).
 */
export type Quad = [PdfPoint, PdfPoint, PdfPoint, PdfPoint];

/** Mirrors Rust `text::TextRangeSelection`. */
export interface TextRangeSelection {
  /** One quad per visual text line in the range (never merged). */
  quads: Quad[];
  /** Plain-text content of the range, for clipboard copy. */
  text: string;
}

// ---------------------------------------------------------------------------
// IPC wrappers
// ---------------------------------------------------------------------------

/**
 * Hit-test a PDF-user-space point to the nearest character index on a page,
 * within `tolerance` PDF points. Returns null when no character is within
 * tolerance (whitespace, image-only region, or off the text layer entirely).
 */
export async function charIndexAtPoint(
  docId: string,
  pageIndex: number,
  x: number,
  y: number,
  tolerance: number,
): Promise<number | null> {
  return invoke<number | null>("char_index_at_point", { docId, pageIndex, x, y, tolerance });
}

/**
 * Resolve a character range `[start, end)` on a page into the quads for a
 * text-anchored Highlight annotation plus the plain-text content for the
 * clipboard. Returns an empty selection (quads: [], text: "") for a degenerate
 * range - safe to call speculatively during a drag.
 */
export async function getTextSelection(
  docId: string,
  pageIndex: number,
  start: number,
  end: number,
): Promise<TextRangeSelection> {
  return invoke<TextRangeSelection>("get_text_selection", { docId, pageIndex, start, end });
}

/**
 * Copy `text` to the OS clipboard via `@tauri-apps/plugin-clipboard-manager`.
 *
 * NOT `navigator.clipboard.writeText()` - this project registered no clipboard
 * plugin/permission before the text-selection-UX pass (redline#text-select-ux),
 * so the bare Web API call had no Tauri capability grant behind it. It likely
 * worked by accident on macOS (WKWebView serves the standard Clipboard API
 * natively for a user-gesture call with no Tauri involvement) but is unreliable
 * on Windows WebView2 without an explicit permission - exactly the "nothing
 * happens" class of bug this pass exists to fix. `clipboard-manager:allow-write-text`
 * is granted in `src-tauri/capabilities/default.json`.
 */
export async function copyToClipboard(text: string): Promise<void> {
  await pluginWriteText(text);
}

// ---------------------------------------------------------------------------
// Pure gesture math (no DOM, no Tauri - unit-testable in isolation)
// ---------------------------------------------------------------------------

/**
 * Turn a drag's anchor (pointer-down hit) and focus (current pointer position
 * hit) character indices into a normalized `[start, end)` character range,
 * inclusive of BOTH the anchor and focus characters. Handles a backwards drag
 * (focus before anchor) by swapping. A single-character selection (anchor ===
 * focus) yields a 1-character range, not an empty one.
 */
export function selectionRange(anchorChar: number, focusChar: number): { start: number; end: number } {
  const lo = Math.min(anchorChar, focusChar);
  const hi = Math.max(anchorChar, focusChar);
  return { start: lo, end: hi + 1 };
}

/**
 * The pointer cursor for the I-beam tool's overlay, given whether it is
 * currently dragging a selection and whether the last hit-test (hover, or the
 * drag's own anchor/focus resolve) found a character nearby.
 *
 * Owner feedback (v0.3.22): the tool showed a generic crosshair everywhere,
 * with no indication of where text could actually be selected. This never
 * returns "crosshair" - that is the create-tool cursor, and the whole point of
 * this fix is that the I-beam tool must never show it. Over text: the
 * standard text-selection cursor. Off text (mid-drag or hovering): the
 * default arrow - not a "not-allowed" glyph, since off-text is just where the
 * drag hasn't reached text yet, not a forbidden action.
 */
export function cursorForTextTool(hoveringOrDraggingOverText: boolean): "text" | "default" {
  return hoveringOrDraggingOverText ? "text" : "default";
}
