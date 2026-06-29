<script lang="ts">
  /**
   * ComparePanel — two-tier PDF version comparison (M6 Phase 1.1, spec §10).
   *
   * Accepts two PDF file paths (passed as props by the parent, which uses the
   * Tauri file-dialog to set them), optional 0-based page indices, and a
   * "Compare" button that calls `comparePages` and displays:
   *   - A color-overlay diff PNG (changed pixels = red, unchanged = grey)
   *   - Tier-1 text summary (character match, delta count)
   *   - Tier-2 pixel summary (changed %, max delta)
   *
   * Spec §10: "Diff rendering: color-channel overlay (old vs new in contrasting
   * colors) + optional change-highlight."
   *
   * Props:
   *   pathA    — absolute path to the "old" PDF (from parent file picker)
   *   pathB    — absolute path to the "new" PDF (from parent file picker)
   */
  import { comparePages, type PageDiffResult } from "$lib/ipc";

  interface Props {
    /** Absolute path to the "old" PDF. Empty string = not yet chosen. */
    pathA?: string;
    /** Absolute path to the "new" PDF. Empty string = not yet chosen. */
    pathB?: string;
  }

  const { pathA = "", pathB = "" }: Props = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let pageA = $state<number>(0);
  let pageB = $state<number>(0);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let result = $state<PageDiffResult | null>(null);

  const canCompare = $derived(pathA.length > 0 && pathB.length > 0 && !busy);

  // ---------------------------------------------------------------------------
  // Actions
  // ---------------------------------------------------------------------------

  async function handleCompare() {
    if (!canCompare) return;
    busy = true;
    error = null;
    result = null;
    try {
      result = await comparePages(pathA, pathB, pageA, pageB);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="compare-panel">
  <div class="compare-files">
    <div class="compare-file-row">
      <span class="compare-file-label">File A (old):</span>
      <span class="compare-file-path">{pathA || "(none selected)"}</span>
    </div>
    <div class="compare-file-row">
      <span class="compare-file-label">File B (new):</span>
      <span class="compare-file-path">{pathB || "(none selected)"}</span>
    </div>
  </div>

  <div class="compare-pages">
    <label class="compare-page-label">
      Page A (0-based)
      <input
        class="compare-page-input"
        data-testid="page-a-input"
        type="number"
        min="0"
        value={pageA}
        oninput={(e) => { pageA = Math.max(0, parseInt((e.target as HTMLInputElement).value) || 0); }}
      />
    </label>
    <label class="compare-page-label">
      Page B (0-based)
      <input
        class="compare-page-input"
        data-testid="page-b-input"
        type="number"
        min="0"
        value={pageB}
        oninput={(e) => { pageB = Math.max(0, parseInt((e.target as HTMLInputElement).value) || 0); }}
      />
    </label>
  </div>

  <button
    class="btn-toolbar btn-compare"
    onclick={handleCompare}
    disabled={!canCompare}
  >
    {busy ? "Comparing…" : "Compare"}
  </button>

  {#if error}
    <p class="compare-error">{error}</p>
  {/if}

  {#if result}
    <div class="compare-result">
      <div class="compare-stats">
        <span class="stat-item">
          Text match: <strong>{result.text_char_match ? "Yes" : "No"}</strong>
        </span>
        {#if result.text_char_match && result.text_delta_count > 0}
          <span class="stat-item">
            Position deltas: <strong>{result.text_delta_count}</strong>
            (RMS {result.text_rms_delta_pts.toFixed(2)} pt)
          </span>
        {/if}
        <span class="stat-item">
          Pixels changed: <strong>{result.changed_pct.toFixed(2)}%</strong>
        </span>
        <span class="stat-item">
          Max delta: <strong>{result.max_pixel_delta}</strong>/255
        </span>
        <span class="stat-item stat-dpi">
          Render DPI: {result.render_dpi}
        </span>
      </div>

      <div class="compare-overlay">
        <img
          src="data:image/png;base64,{result.diff_png_b64}"
          alt="Diff overlay — red pixels changed, grey pixels unchanged"
          class="diff-image"
        />
      </div>
    </div>
  {/if}
</div>

<style>
  .compare-panel {
    display: flex;
    flex-direction: column;
    gap: var(--space-3, 8px);
    padding: var(--space-3, 8px);
    font-size: var(--text-sm, 13px);
    overflow-y: auto;
    height: 100%;
  }

  .compare-files {
    display: flex;
    flex-direction: column;
    gap: var(--space-1, 4px);
  }

  .compare-file-row {
    display: flex;
    gap: var(--space-2, 6px);
    align-items: baseline;
    flex-wrap: wrap;
  }

  .compare-file-label {
    font-weight: 600;
    white-space: nowrap;
    color: var(--color-text-secondary, #666);
    min-width: 90px;
  }

  .compare-file-path {
    font-family: monospace;
    font-size: var(--text-xs, 11px);
    color: var(--color-text, #111);
    word-break: break-all;
  }

  .compare-pages {
    display: flex;
    gap: var(--space-4, 12px);
    align-items: center;
  }

  .compare-page-label {
    display: flex;
    flex-direction: column;
    gap: var(--space-1, 4px);
    font-size: var(--text-xs, 11px);
    color: var(--color-text-secondary, #666);
  }

  .compare-page-input {
    width: 72px;
    padding: var(--space-1, 4px) var(--space-2, 6px);
    border: 1px solid var(--color-border, #ccc);
    border-radius: var(--radius-sm, 3px);
    font-size: var(--text-sm, 13px);
  }

  .compare-error {
    color: var(--color-danger, #c00);
    font-size: var(--text-xs, 11px);
    margin: 0;
    padding: var(--space-1, 4px) var(--space-2, 6px);
    background: var(--color-danger-bg, #fff0f0);
    border-radius: var(--radius-sm, 3px);
  }

  .compare-result {
    display: flex;
    flex-direction: column;
    gap: var(--space-2, 6px);
  }

  .compare-stats {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2, 6px) var(--space-4, 12px);
    padding: var(--space-2, 6px);
    background: var(--color-surface-raised, #f5f5f5);
    border-radius: var(--radius-sm, 3px);
  }

  .stat-item {
    font-size: var(--text-xs, 11px);
    color: var(--color-text-secondary, #555);
  }

  .stat-dpi {
    color: var(--color-text-muted, #999);
  }

  .compare-overlay {
    overflow: auto;
  }

  .diff-image {
    display: block;
    max-width: 100%;
    border: 1px solid var(--color-border, #ddd);
    border-radius: var(--radius-sm, 3px);
  }
</style>
