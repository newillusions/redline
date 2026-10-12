/**
 * The mark-up clipboard (docs/arrange.md C1-C4): one clipboard for the whole app, so a copy in
 * one open document pastes into another. In-app memory only - nothing is written to the OS
 * clipboard (spec C4). The items are a deep snapshot taken at copy time: later edits to the
 * source never leak into a paste.
 */
import type { Markup } from "./ipc";

export interface ClipboardPayload {
  /** Document order (z-order) of the source. */
  items: Markup[];
  sourceDocId: string;
  /** How many times this payload has been pasted; drives the no-cursor cascade offset. */
  pastes: number;
}

class MarkupClipboard {
  // Raw: the payload can hold thousands of markups and is only ever replaced, never edited.
  payload = $state.raw<ClipboardPayload | null>(null);

  get size(): number { return this.payload?.items.length ?? 0; }

  /** Replace the clipboard with a snapshot of `items`. An empty list leaves it unchanged. */
  set(items: readonly Markup[], sourceDocId: string): void {
    if (items.length === 0) return;
    this.payload = { items: JSON.parse(JSON.stringify(items)) as Markup[], sourceDocId, pastes: 0 };
  }

  notePasted(): void {
    if (this.payload) this.payload = { ...this.payload, pastes: this.payload.pastes + 1 };
  }

  clear(): void { this.payload = null; }
}

export const markupClipboard = new MarkupClipboard();
