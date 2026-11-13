<script lang="ts">
  /**
   * AlongPathDialog - Divide, Measure and Path Array (docs/arrange.md section 6). The first
   * selected markup is the path; the rest are the items placed along it. Divide splits the path
   * into N equal parts, Measure marks every distance, Path Array spreads N copies from the
   * path's start to its end. `onSubmit` returns an error message to keep the dialog open, or
   * null when it worked. Adapted from redcolumn (Apache-2.0), packages/markup/src/cad.ts.
   */
  import type { ScaleRecord } from "$lib/ipc";
  import { spacingToPoints } from "$lib/markup-arrange";
  import type { AlongSpec } from "$lib/path-array";

  const {
    scale,
    onSubmit,
    onCancel,
  }: {
    scale: ScaleRecord | null;
    onSubmit: (spec: AlongSpec, align: boolean) => string | null;
    onCancel: () => void;
  } = $props();

  let kind = $state<AlongSpec["kind"]>("divide");
  let count = $state(4);
  let spacing = $state(100);
  let unit = $state("pt");
  let align = $state(false);
  let error = $state<string | null>(null);

  function submit() {
    if (kind === "measure") {
      const pts = spacingToPoints(spacing, unit, scale);
      if (pts === null) {
        error = "Enter a distance above zero.";
        return;
      }
      error = onSubmit({ kind, spacing: pts }, align);
    } else {
      error = onSubmit({ kind, count }, align);
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); onCancel(); }
    else if (e.key === "Enter") { e.preventDefault(); submit(); }
    e.stopPropagation(); // typing must not reach the viewport's single-key tool shortcuts
  }
</script>

<div class="dialog-backdrop" role="presentation" onclick={onCancel} onkeydown={null}></div>

<dialog open class="along" aria-modal="true" aria-label="Place along path" onkeydown={onKey}>
  <p class="title">Place along path</p>
  <p class="hint">The first selected markup is the path. The others are copied along it; the originals stay where they are.</p>

  <label class="row">
    <span>Method</span>
    <select bind:value={kind} aria-label="Method">
      <option value="divide">Divide into equal parts</option>
      <option value="measure">Measure (every distance)</option>
      <option value="array">Path array (N copies, start to end)</option>
    </select>
  </label>

  {#if kind === "measure"}
    <label class="row"><span>Every</span><input type="number" min="0" step="any" bind:value={spacing} aria-label="Distance" /></label>
    <label class="row">
      <span>Unit</span>
      <select bind:value={unit} aria-label="Unit">
        <option value="pt">pt (PDF points)</option>
        {#if scale}<option value={scale.unit}>{scale.unit} (page scale)</option>{/if}
      </select>
    </label>
  {:else}
    <label class="row">
      <span>{kind === "divide" ? "Parts" : "Copies"}</span>
      <input type="number" min={kind === "divide" ? 2 : 1} step="1" bind:value={count} aria-label={kind === "divide" ? "Parts" : "Copies"} />
    </label>
  {/if}

  <label class="row check">
    <input type="checkbox" bind:checked={align} aria-label="Align to path" />
    <span>Turn copies to follow the path (lines, arrows, polylines and counts only)</span>
  </label>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  <div class="button-row">
    <button class="btn-cancel" onclick={onCancel}>Cancel</button>
    <button class="btn-submit" onclick={submit}>Place</button>
  </div>
</dialog>

<style>
  .dialog-backdrop { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.45); z-index: 900; }
  .along {
    position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%); z-index: 901;
    border: 1px solid var(--color-border); border-radius: var(--radius-md);
    background: var(--color-bg-panel); color: var(--color-text);
    padding: var(--space-5); min-width: 320px; max-width: 420px; margin: 0;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
  }
  .title { font-size: var(--font-size-base); font-weight: 600; margin: 0 0 var(--space-3); }
  .row { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); margin-bottom: var(--space-2); font-size: var(--font-size-sm); }
  .row input:not([type="checkbox"]), .row select {
    width: 180px; padding: var(--space-1) var(--space-2); box-sizing: border-box;
    border: 1px solid var(--color-border); border-radius: var(--radius-sm);
    background: var(--color-bg-input); color: var(--color-text); font-size: var(--font-size-sm);
  }
  .check { justify-content: flex-start; }
  .hint { font-size: var(--font-size-sm); color: var(--color-text-secondary); margin: var(--space-2) 0; }
  .error { font-size: var(--font-size-sm); color: var(--color-danger, #e02424); margin: var(--space-2) 0; }
  .button-row { display: flex; gap: var(--space-2); justify-content: flex-end; margin-top: var(--space-3); }
  .btn-submit, .btn-cancel {
    border-radius: var(--radius-sm); font-size: var(--font-size-sm); padding: var(--space-1) var(--space-4);
    cursor: pointer; border: 1px solid transparent;
  }
  .btn-submit { background: var(--color-primary); color: var(--color-text-inverse); border-color: var(--color-primary); font-weight: 600; }
  .btn-submit:hover { background: var(--color-primary-hover); border-color: var(--color-primary-hover); }
  .btn-cancel { background: var(--color-bg-active); color: var(--color-text-secondary); border-color: var(--color-border); }
  .btn-cancel:hover { background: var(--color-bg-hover); }
</style>
