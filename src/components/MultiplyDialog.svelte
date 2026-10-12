<script lang="ts">
  /**
   * MultiplyDialog - Revu's Multiply (docs/arrange.md section 2): direction, distance between
   * copies with a unit, number of copies. "Grid" is redline's extension (columns x rows, growing
   * right and down). The distance is centre to centre by default (the same as the step between
   * corresponding points), or the clear gap between neighbours; the choice is remembered. The
   * unit list is points plus the page scale's own unit when the viewed page has a scale.
   * `onSubmit` returns an error message to keep the dialog open, or null when it worked.
   */
  import type { ScaleRecord } from "$lib/ipc";
  import { spacingToPoints, type MultiplyDirection, type MultiplyMeasure, type MultiplySpec } from "$lib/markup-arrange";
  import { loadMultiplyMeasure, persistMultiplyMeasure } from "$lib/multiply-prefs";

  const {
    scale,
    onSubmit,
    onCancel,
  }: {
    scale: ScaleRecord | null;
    onSubmit: (spec: MultiplySpec) => string | null;
    onCancel: () => void;
  } = $props();

  type Mode = MultiplyDirection | "grid";
  let mode = $state<Mode>("right");
  let copies = $state(3);
  let cols = $state(3);
  let rows = $state(2);
  let spacing = $state(36);
  let spacingV = $state(36);
  let unit = $state("pt");
  let measure = $state<MultiplyMeasure>(loadMultiplyMeasure());
  let error = $state<string | null>(null);

  function submit() {
    const gap = measure === "gap";
    const pitch = spacingToPoints(spacing, unit, scale, gap);
    const pitchV = spacingToPoints(spacingV, unit, scale, gap);
    if (pitch === null || (mode === "grid" && pitchV === null)) {
      error = gap ? "Enter a distance of zero or more." : "Enter a distance above zero.";
      return;
    }
    const spec: MultiplySpec =
      mode === "grid"
        ? { kind: "grid", cols, rows, pitchX: pitch, pitchY: pitchV!, measure }
        : { kind: "line", direction: mode, count: copies, pitch, measure };
    persistMultiplyMeasure(measure);
    error = onSubmit(spec);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); onCancel(); }
    else if (e.key === "Enter") { e.preventDefault(); submit(); }
    e.stopPropagation(); // typing must not reach the viewport's single-key tool shortcuts
  }
</script>

<div class="dialog-backdrop" role="presentation" onclick={onCancel} onkeydown={null}></div>

<dialog open class="multiply" aria-modal="true" aria-label="Multiply" onkeydown={onKey}>
  <p class="title">Multiply</p>

  <label class="row">
    <span>Direction</span>
    <select bind:value={mode} aria-label="Direction">
      <option value="right">Right</option>
      <option value="left">Left</option>
      <option value="up">Up</option>
      <option value="down">Down</option>
      <option value="grid">Grid (right and down)</option>
    </select>
  </label>

  {#if mode === "grid"}
    <label class="row"><span>Columns</span><input type="number" min="1" step="1" bind:value={cols} aria-label="Columns" /></label>
    <label class="row"><span>Rows</span><input type="number" min="1" step="1" bind:value={rows} aria-label="Rows" /></label>
  {:else}
    <label class="row"><span>Copies</span><input type="number" min="1" step="1" bind:value={copies} aria-label="Copies" /></label>
  {/if}

  <label class="row">
    <span>{mode === "grid" ? "Across" : "Distance"}</span>
    <input type="number" min="0" step="any" bind:value={spacing} aria-label="Distance" />
  </label>
  {#if mode === "grid"}
    <label class="row"><span>Down</span><input type="number" min="0" step="any" bind:value={spacingV} aria-label="Distance down" /></label>
  {/if}
  <div class="row" role="radiogroup" aria-label="Distance is measured">
    <span>Measured</span>
    <span class="choices">
      <label title="From the centre of one copy to the centre of the next. The copies step by exactly this distance, whatever their size.">
        <input type="radio" name="multiply-measure" value="centre" bind:group={measure} aria-label="Centre to centre" /> Centre to centre
      </label>
      <label title="The clear space from the edge of one copy to the edge of the next. The step is the copy's width (across) or height (down) plus this distance.">
        <input type="radio" name="multiply-measure" value="gap" bind:group={measure} aria-label="Gap" /> Gap
      </label>
    </span>
  </div>
  <label class="row">
    <span>Unit</span>
    <select bind:value={unit} aria-label="Unit">
      <option value="pt">pt (PDF points)</option>
      {#if scale}<option value={scale.unit}>{scale.unit} (page scale)</option>{/if}
    </select>
  </label>
  <p class="hint">{measure === "gap" ? "Distance is the clear space between one copy and the next." : "Distance is from the centre of one copy to the centre of the next."} Copies that would leave the page are not made.</p>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  <div class="button-row">
    <button class="btn-cancel" onclick={onCancel}>Cancel</button>
    <button class="btn-submit" onclick={submit}>Multiply</button>
  </div>
</dialog>

<style>
  .dialog-backdrop { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.45); z-index: 900; }
  .multiply {
    position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%); z-index: 901;
    border: 1px solid var(--color-border); border-radius: var(--radius-md);
    background: var(--color-bg-panel); color: var(--color-text);
    padding: var(--space-5); min-width: 320px; max-width: 420px; margin: 0;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
  }
  .title { font-size: var(--font-size-base); font-weight: 600; margin: 0 0 var(--space-3); }
  .row { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); margin-bottom: var(--space-2); font-size: var(--font-size-sm); }
  .row input, .row select {
    width: 180px; padding: var(--space-1) var(--space-2); box-sizing: border-box;
    border: 1px solid var(--color-border); border-radius: var(--radius-sm);
    background: var(--color-bg-input); color: var(--color-text); font-size: var(--font-size-sm);
  }
  .choices { display: flex; flex-direction: column; gap: var(--space-1); width: 180px; }
  .choices label { display: flex; align-items: center; gap: var(--space-2); }
  .choices input { width: auto; }
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
