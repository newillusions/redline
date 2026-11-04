/**
 * Reactive holder for the sidebar/panel layout (see panel-layout.ts) plus its persistence.
 *
 * Every change goes through `update`, which applies a pure layout function and schedules
 * one debounced save - a sidebar drag fires dozens of updates a second, the settings file
 * is written once when it settles. `persist` is injected so tests (and a future storage
 * backend) need no Tauri; App.svelte passes a read-modify-write of AppSettings so the
 * layout never clobbers fields other code saved in between.
 *
 * Persistence is guarded: nothing is written until `load` has succeeded (a failed or
 * unrecognised stored value is left on disk untouched, never replaced by the default), and
 * a failed save keeps the change dirty so the next flush or change retries it. Both
 * failures are reported through `onError` so the user is told.
 */
import { defaultLayout, layoutProblem, normalizeLayout, type PanelLayout } from "./panel-layout";

export const LAYOUT_SAVE_DEBOUNCE_MS = 400;

export class PanelLayoutStore {
  layout = $state<PanelLayout>(defaultLayout());

  #persist: (layout: PanelLayout) => Promise<void>;
  #debounceMs: number;
  #onError: (message: string) => void;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #ready = false;
  #dirty = false;

  constructor(
    persist: (layout: PanelLayout) => Promise<void>,
    debounceMs = LAYOUT_SAVE_DEBOUNCE_MS,
    onError: (message: string) => void = (m) => console.error(m),
  ) {
    this.#persist = persist;
    this.#debounceMs = debounceMs;
    this.#onError = onError;
  }

  /** True once a stored value was read (or found absent); saving is disabled before that. */
  get ready(): boolean {
    return this.#ready;
  }

  /**
   * Adopt a persisted value (validated). Does not trigger a save. A value carrying an
   * unrecognised `version` (written by a newer build) is NOT adopted and saving stays
   * disabled, so the stored layout is never overwritten by this session's default. A damaged
   * version-1 value is replaced by the default in memory only, and reported.
   */
  load(raw: unknown): void {
    if (raw && typeof raw === "object" && "version" in raw && (raw as { version: unknown }).version !== 1) {
      this.#onError(
        "Panel layout was saved by a different version of Redline; using the default layout for this session and leaving the saved one untouched.",
      );
      return;
    }
    this.layout = normalizeLayout(raw);
    this.#ready = true;
    // A damaged value is reported once; it stays on disk until the user changes the layout
    // (load() never marks the store dirty, so nothing is written before then).
    const problem = raw === null || raw === undefined ? null : layoutProblem(raw);
    if (problem) {
      this.#onError(
        `The saved panel layout is damaged (${problem}); using the default layout. The saved value is kept until you change the layout.`,
      );
    }
  }

  /** The stored value could not be read: keep the default for this session, never save over it. */
  loadFailed(e: unknown): void {
    this.#onError(`Could not load the panel layout (using the default; saved layout left untouched): ${errText(e)}`);
  }

  update(fn: (layout: PanelLayout) => PanelLayout): void {
    const next = fn(this.layout);
    if (next === this.layout) return;
    this.layout = next;
    this.#dirty = true;
    this.#schedule();
  }

  /** Back to the default layout (persisted like any other change). */
  reset(): void {
    this.update(() => defaultLayout());
  }

  /** Write any pending change now (e.g. before the window closes). */
  async flush(): Promise<void> {
    if (this.#timer !== null) {
      clearTimeout(this.#timer);
      this.#timer = null;
    }
    if (this.#dirty) await this.#save();
  }

  #schedule(): void {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.#save();
    }, this.#debounceMs);
  }

  async #save(): Promise<void> {
    if (!this.#ready) return;
    this.#dirty = false;
    try {
      await this.#persist($state.snapshot(this.layout) as PanelLayout);
    } catch (e) {
      // The layout still applies for this session; stay dirty so flush()/the next change retries.
      this.#dirty = true;
      this.#onError(`Could not save the panel layout (will retry): ${errText(e)}`);
    }
  }
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
