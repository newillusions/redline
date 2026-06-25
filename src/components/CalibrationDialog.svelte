<script lang="ts">
  /**
   * Modal dialog shown after the user clicks two calibration points.
   * Accepts the measured pixel distance (PDF pts) and lets the user enter
   * the real-world equivalent distance + unit. Emits onConfirm({ ratio, unit, label })
   * or onCancel.
   */

  const {
    pixelDist,
    onConfirm,
    onCancel,
  }: {
    pixelDist: number;
    onConfirm: (result: { ratio: number; unit: string; label: string; precision: number }) => void;
    onCancel: () => void;
  } = $props();

  let knownDist = $state("");
  let unit = $state("m");

  const numericDist = $derived(parseFloat(knownDist));
  const isValid = $derived(!isNaN(numericDist) && numericDist > 0);

  // ratio = real-world units per PDF point
  const ratio = $derived(isValid ? numericDist / pixelDist : 0);
  const label = $derived(isValid ? `1:${Math.round(1 / ratio)}` : "");

  function confirm() {
    if (!isValid) return;
    onConfirm({ ratio, unit, label, precision: 2 });
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onCancel();
    if (e.key === "Enter" && isValid) confirm();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y-click-events-have-key-events -->
<div class="dialog-backdrop" onclick={onCancel} role="presentation">
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true"
       aria-label="Set calibration scale">
    <h3 class="dialog-title">Set Scale</h3>
    <p class="dialog-hint">
      You drew a line of <strong>{pixelDist.toFixed(1)} pt</strong> in PDF space.
      Enter the real-world length that corresponds to.
    </p>

    <label class="field-label" for="known-dist">Known distance</label>
    <div class="field-row">
      <input
        id="known-dist"
        class="field-input"
        type="number"
        min="0.0001"
        step="any"
        placeholder="e.g. 5000"
        bind:value={knownDist}
        aria-label="Known distance"
      />
      <select class="field-select" bind:value={unit} aria-label="Unit">
        <option value="mm">mm</option>
        <option value="m">m</option>
        <option value="km">km</option>
        <option value="in">in</option>
        <option value="ft">ft</option>
      </select>
    </div>

    {#if isValid}
      <p class="scale-preview">Scale: {label} (1 pt = {ratio.toExponential(3)} {unit})</p>
    {/if}

    <div class="dialog-actions">
      <button class="btn-secondary" onclick={onCancel}>Cancel</button>
      <button class="btn-primary" onclick={confirm} disabled={!isValid}>Set Scale</button>
    </div>
  </div>
</div>

<style>
  .dialog-backdrop {
    position: fixed; inset: 0;
    background: rgba(0 0 0 / 0.45);
    display: flex; align-items: center; justify-content: center;
    z-index: 1000;
  }
  .dialog {
    background: var(--color-bg-panel);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-6);
    min-width: 360px;
    display: flex; flex-direction: column; gap: var(--space-3);
    box-shadow: 0 8px 32px rgba(0 0 0 / 0.25);
  }
  .dialog-title { margin: 0; font-size: var(--font-size-lg); color: var(--color-text); }
  .dialog-hint { margin: 0; color: var(--color-text-muted); font-size: var(--font-size-sm); }
  .field-label { font-size: var(--font-size-sm); color: var(--color-text); }
  .field-row { display: flex; gap: var(--space-2); }
  .field-input {
    flex: 1; padding: var(--space-2) var(--space-3);
    border: 1px solid var(--color-border); border-radius: var(--radius-sm);
    background: var(--color-bg-input); color: var(--color-text);
    font-size: var(--font-size-base);
  }
  .field-select {
    padding: var(--space-2) var(--space-2);
    border: 1px solid var(--color-border); border-radius: var(--radius-sm);
    background: var(--color-bg-input); color: var(--color-text);
    font-size: var(--font-size-base);
  }
  .scale-preview { margin: 0; font-size: var(--font-size-sm); color: var(--color-primary); font-family: monospace; }
  .dialog-actions { display: flex; gap: var(--space-2); justify-content: flex-end; margin-top: var(--space-2); }
  .btn-primary {
    padding: var(--space-2) var(--space-4);
    background: var(--color-primary); color: var(--color-text-inverse);
    border: none; border-radius: var(--radius-sm); cursor: pointer; font-size: var(--font-size-base);
  }
  .btn-primary:disabled { opacity: 0.45; cursor: not-allowed; }
  .btn-secondary {
    padding: var(--space-2) var(--space-4);
    background: var(--color-bg-active); color: var(--color-text);
    border: 1px solid var(--color-border); border-radius: var(--radius-sm); cursor: pointer;
    font-size: var(--font-size-base);
  }
</style>
