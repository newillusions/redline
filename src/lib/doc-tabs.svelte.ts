/**
 * Multi-document tab store (feat/tabbed-multi-file).
 *
 * Each open PDF lives in a DocTab that carries its own MarkupStore,
 * TakeoffStore, and a ViewportSnapshot (zoom + page + scroll). Only one
 * Viewport component is ever mounted at a time; the snapshot is saved before
 * switching and restored as initialState on the newly-active Viewport.
 *
 * This keeps memory bounded: N open PDFs = N live PDFium handles in Rust,
 * but only 1 live Viewport/tile-cache/render-loop in the frontend. The §20
 * memory work is not regressed.
 */
import type { DocumentInfo } from "./ipc";
import { MarkupStore } from "./markup-store.svelte";
import { TakeoffStore } from "./takeoff-store.svelte";

// ---------------------------------------------------------------------------
// Viewport snapshot (type defined in viewport.ts, re-exported for convenience)
// ---------------------------------------------------------------------------

export type { ViewportSnapshot } from "./viewport";
import type { ViewportSnapshot } from "./viewport";

export const DEFAULT_VIEWPORT_SNAPSHOT: ViewportSnapshot = {
  zoom: 1.0,
  pageIndex: 0,
  scrollX: 0,
  scrollY: 0,
};

// ---------------------------------------------------------------------------
// DocTab record
// ---------------------------------------------------------------------------

export interface DocTab {
  /** Stable identifier, same as DocumentInfo.doc_id. */
  docId: string;
  doc: DocumentInfo;
  store: MarkupStore;
  takeoffStore: TakeoffStore;
  /** Last-known viewport state; restored when this tab is re-activated. */
  viewportSnapshot: ViewportSnapshot;
  /**
   * True if this document required a password to open (manually entered or
   * auto-tried from the known-password store). Drives the "Save Unprotected
   * Copy…" toolbar button's enabled state - never true for a plain PDF.
   */
  isEncrypted: boolean;
}

// ---------------------------------------------------------------------------
// DocTabStore
// ---------------------------------------------------------------------------

export class DocTabStore {
  tabs = $state<DocTab[]>([]);
  activeDocId = $state<string | null>(null);

  /** The currently active tab, or null when no documents are open. */
  get activeTab(): DocTab | null {
    return this.tabs.find((t) => t.docId === this.activeDocId) ?? null;
  }

  /** Find an open tab by file path (used for dedup-open-same-path). */
  findByPath(path: string): DocTab | undefined {
    return this.tabs.find((t) => t.doc.path === path);
  }

  /**
   * Add a new tab and make it active.
   * Caller is responsible for dedup (check findByPath first).
   */
  addTab(
    doc: DocumentInfo,
    store: MarkupStore,
    takeoffStore: TakeoffStore,
    isEncrypted = false,
  ): DocTab {
    const tab: DocTab = {
      docId: doc.doc_id,
      doc,
      store,
      takeoffStore,
      viewportSnapshot: { ...DEFAULT_VIEWPORT_SNAPSHOT },
      isEncrypted,
    };
    this.tabs.push(tab);
    this.activeDocId = doc.doc_id;
    return tab;
  }

  /** Switch to an existing tab by docId. No-op if docId not found. */
  switchTab(docId: string): void {
    if (this.tabs.some((t) => t.docId === docId)) {
      this.activeDocId = docId;
    }
  }

  /**
   * Remove a tab from the list.
   *
   * Returns the next activeDocId after removal:
   *   - If the closed tab was NOT active: the current activeDocId (unchanged).
   *   - If the closed tab WAS active and neighbors remain: left neighbor
   *     preferred, right neighbor as fallback.
   *   - If the closed tab was the last tab: null.
   *
   * Caller is responsible for calling closeDocument(docId) IPC.
   */
  closeTab(docId: string): string | null {
    const idx = this.tabs.findIndex((t) => t.docId === docId);
    if (idx < 0) return this.activeDocId;

    this.tabs = this.tabs.filter((t) => t.docId !== docId);

    // Closed a background tab — active is unaffected.
    if (this.activeDocId !== docId) return this.activeDocId;

    // Closed the active tab.
    if (this.tabs.length === 0) {
      this.activeDocId = null;
      return null;
    }

    // Pick left neighbor if possible, otherwise fall to the tab now at idx
    // (which was the right neighbor before removal).
    const nextIdx = idx > 0 ? idx - 1 : 0;
    this.activeDocId = this.tabs[nextIdx].docId;
    return this.activeDocId;
  }

  /**
   * Reorder tabs by moving the tab at `fromIndex` to `toIndex`.
   *
   * Both indices are clamped to valid bounds before use.
   * Out-of-range `fromIndex` (before clamping to a valid source) is a no-op.
   * Same effective position after clamping is also a no-op.
   * `activeDocId` is preserved regardless of which tab moves.
   */
  moveTab(fromIndex: number, toIndex: number): void {
    const len = this.tabs.length;
    if (len < 2) return;

    // fromIndex must refer to an existing tab; treat out-of-bounds as no-op.
    if (fromIndex < 0 || fromIndex >= len) return;

    // Clamp toIndex to valid range.
    const clampedTo = Math.max(0, Math.min(toIndex, len - 1));
    if (fromIndex === clampedTo) return;

    const next = [...this.tabs];
    const [tab] = next.splice(fromIndex, 1);
    next.splice(clampedTo, 0, tab);
    this.tabs = next;
  }

  /**
   * Save the viewport snapshot for a tab (called on every state change from
   * the active Viewport, so the snapshot is up to date when switching away).
   */
  saveViewportSnapshot(docId: string, snapshot: ViewportSnapshot): void {
    const tab = this.tabs.find((t) => t.docId === docId);
    if (tab) tab.viewportSnapshot = { ...snapshot };
  }
}
