/**
 * Reactive per-document layer state: the parsed layer model, the VIEW (which groups are
 * hidden), and the selection the panel acts on. Svelte 5 runes; the IPC is in `layers.ts`.
 *
 * The view is session-only. It is rendered by the backend (`set_layer_view`, no file write)
 * and filters which mark-ups the viewer draws. Counts, the Markups List, legends and exports
 * never read it. One store per open document, found by id (`layerStoreFor`), so the panel,
 * the viewer and the properties panel share state without threading a prop through App.
 */
import {
  addLayerToFile, defaultOff, isMarkupShown, isolate, readLayers, sameSet, saveLayersDefault,
  setLayerView, showAll, toggleLayer, setGroups, type LayerModel,
} from "./layers";

export type LayerStatus = "idle" | "loading" | "loaded" | "deferred" | "error";

/** Hooks into the mark-up store, for the two actions that write the file. */
export interface MarkupSync {
  /** Push unsaved mark-up edits to the backend (the file write includes them). */
  flush: () => Promise<void>;
  /** The file write saved the mark-ups: they are no longer unsaved. */
  clearDirty: () => void;
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export class LayerStore {
  readonly docId: string;

  status = $state<LayerStatus>("idle");
  model = $state<LayerModel | null>(null);
  deferredSize = $state(0);
  error = $state<string | null>(null);
  /** The last failed view change, shown in the panel; cleared by the next success. */
  viewError = $state<string | null>(null);
  /** Hidden groups in the current view. Replaced, never mutated. */
  off = $state<ReadonlySet<number>>(new Set());
  /** Bumped when the rendered pages changed (view applied, layers added): viewers refetch tiles. */
  epoch = $state(0);
  /** New mark-ups are created on this layer (Bluebeam "Markup Layer"). */
  markupLayerId = $state<number | null>(null);
  selectedId = $state<number | null>(null);
  collapsed = $state<ReadonlySet<number>>(new Set());
  busy = $state(false);

  /** The view the backend is showing (the last one applied). */
  private applied: ReadonlySet<number> = new Set();
  private pushing = false;
  private again = false;

  constructor(docId: string) {
    this.docId = docId;
  }

  get hasLayers(): boolean {
    return !!this.model && this.model.layers.length > 0;
  }

  /** True when the view is the file's own default configuration. */
  get isFileView(): boolean {
    return !this.model || sameSet(this.off, defaultOff(this.model));
  }

  get canChangeView(): boolean {
    return !!this.model && this.model.view_supported;
  }

  /** Whether a mark-up is drawn in the current view. */
  isShown(m: { optional_content?: unknown }): boolean {
    return isMarkupShown(this.model, this.off, m);
  }

  // --- loading ------------------------------------------------------------

  /** Read the layers. `force` reads a large file that was deferred. */
  async load(force = false): Promise<void> {
    this.status = "loading";
    this.error = null;
    try {
      const r = await readLayers(this.docId, force);
      if (r.status === "Deferred") {
        this.status = "deferred";
        this.deferredSize = r.size;
        return;
      }
      this.model = r.model;
      this.off = defaultOff(r.model);
      this.applied = this.off;
      if (this.markupLayerId !== null && !r.model.layers.some((l) => l.id === this.markupLayerId)) {
        this.markupLayerId = null;
      }
      this.status = "loaded";
    } catch (e) {
      this.status = "error";
      this.error = message(e);
    }
  }

  /** Re-read the model after the file's layer structure changed, keeping the view. */
  private async refresh(): Promise<void> {
    const r = await readLayers(this.docId, true);
    if (r.status !== "Loaded") return;
    const ids = new Set(r.model.layers.map((l) => l.id));
    this.model = r.model;
    this.off = new Set([...this.off].filter((id) => ids.has(id)));
    this.applied = this.off;
  }

  // --- the view -----------------------------------------------------------

  /** Make `next` the view: render it, and keep the previous view if that fails. */
  async setView(next: ReadonlySet<number>): Promise<void> {
    if (!this.model) return;
    this.off = next;
    if (this.pushing) {
      this.again = true;
      return;
    }
    this.pushing = true;
    try {
      do {
        this.again = false;
        const target = this.off;
        if (sameSet(target, this.applied)) continue;
        try {
          await setLayerView(this.docId, this.isFileView ? null : [...target]);
          this.applied = target;
          this.viewError = null;
          this.epoch += 1;
        } catch (e) {
          this.viewError = message(e);
          this.off = this.applied;
          break;
        }
      } while (this.again);
    } finally {
      this.pushing = false;
    }
  }

  async toggle(id: number, withChildren = false): Promise<void> {
    if (!this.model) return;
    await this.setView(toggleLayer(this.model, this.off, id, withChildren));
  }

  async isolateLayer(id: number): Promise<void> {
    if (!this.model) return;
    await this.setView(isolate(this.model, this.off, id));
  }

  async showAllLayers(): Promise<void> {
    if (!this.model) return;
    await this.setView(showAll(this.model, this.off));
  }

  /** Back to the file's own default configuration. */
  async resetToFile(): Promise<void> {
    if (!this.model) return;
    await this.setView(defaultOff(this.model));
  }

  /** Hide a set of layers (used by tests and by "hide all"). */
  async hide(ids: number[]): Promise<void> {
    if (!this.model) return;
    await this.setView(setGroups(this.model, this.off, ids, false));
  }

  // --- actions that write the file ---------------------------------------

  /** Add a blank layer, optionally under `parent`. Writes the file (like a page operation). */
  async addLayer(name: string, parent: number | null, sync: MarkupSync): Promise<number | null> {
    this.busy = true;
    this.viewError = null;
    try {
      await sync.flush();
      const id = await addLayerToFile(this.docId, name, parent);
      sync.clearDirty();
      await this.refresh();
      this.epoch += 1;
      return id;
    } catch (e) {
      this.viewError = message(e);
      return null;
    } finally {
      this.busy = false;
    }
  }

  /** Save the current view as the file's default configuration. Writes the file. */
  async saveAsDefault(sync: MarkupSync): Promise<boolean> {
    this.busy = true;
    this.viewError = null;
    try {
      await sync.flush();
      await saveLayersDefault(this.docId, [...this.off]);
      sync.clearDirty();
      const keep = this.off;
      const r = await readLayers(this.docId, true);
      if (r.status === "Loaded") {
        this.model = r.model;
        this.off = keep;
        this.applied = keep;
      }
      this.epoch += 1;
      return true;
    } catch (e) {
      this.viewError = message(e);
      return false;
    } finally {
      this.busy = false;
    }
  }

  /**
   * The file was replaced under the document (restore, reload): forget the view and the
   * parsed layers, drop the backend overlay, and let the next reader load them afresh.
   */
  async reset(): Promise<void> {
    this.off = new Set();
    this.applied = new Set();
    this.model = null;
    this.status = "idle";
    this.markupLayerId = null;
    this.selectedId = null;
    try {
      await setLayerView(this.docId, null);
    } catch {
      // Nothing to release (or the document is gone): the next read starts clean anyway.
    }
    this.epoch += 1;
  }

  setMarkupLayer(id: number | null): void {
    this.markupLayerId = id;
  }

  toggleCollapsed(id: number): void {
    const next = new Set(this.collapsed);
    if (!next.delete(id)) next.add(id);
    this.collapsed = next;
  }
}

const stores = new Map<string, LayerStore>();

/**
 * A newly created mark-up, placed on the document's "mark-up layer" when one is set and the
 * mark-up carries no layer of its own (Tool Chest tools may). Showing the layer is part of
 * it: what the user just drew must not vanish because its layer is hidden.
 */
export function onNewMarkup<T extends { optional_content?: unknown; layer: string | null }>(
  docId: string,
  m: T,
): T {
  const s = stores.get(docId);
  if (!s || s.markupLayerId === null || !s.model) return m;
  if ((m.optional_content ?? null) !== null || m.layer !== null) return m;
  const l = s.model.layers.find((x) => x.id === s.markupLayerId);
  if (!l) return m;
  if (s.off.has(l.id)) void s.toggle(l.id);
  return { ...m, optional_content: { Reference: [l.id, l.gen] }, layer: l.name };
}

/** The layer store of an open document (created on first use). */
export function layerStoreFor(docId: string): LayerStore {
  let s = stores.get(docId);
  if (!s) {
    s = new LayerStore(docId);
    stores.set(docId, s);
  }
  return s;
}

/** Forget a closed document's layer state. */
export function dropLayerStore(docId: string): void {
  stores.delete(docId);
}
