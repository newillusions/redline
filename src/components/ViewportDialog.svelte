<script lang="ts">
  /**
   * Create or edit a measurement viewport (docs/measurement-viewports.md): a name plus its
   * scale - a preset drawing scale, or "calibrate by two points" (the caller then runs the
   * normal calibration on the page and asks for the known distance).
   */
  import { PRESET_UNITS, SCALE_PRESETS, presetRatio, type ViewportDialogResult } from "$lib/scale-presets";

  const {
    title = "New Viewport",
    initialName,
    allowKeep = false,
    onConfirm,
    onCancel,
  }: {
    title?: string;
    initialName: string;
    /** Editing an existing viewport: offer "keep the current scale". */
    allowKeep?: boolean;
    onConfirm: (r: ViewportDialogResult) => void;
    onCancel: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  let name = $state(initialName);
  // svelte-ignore state_referenced_locally
  let mode = $state<"preset" | "calibrate" | "keep">(allowKeep ? "keep" : "preset");
  let presetIndex = $state(SCALE_PRESETS.findIndex((p) => p.label === "1:20"));
  let unit = $state(SCALE_PRESETS.find((p) => p.label === "1:20")!.unit);

  const preset = $derived(SCALE_PRESETS[presetIndex]);
  const ratio = $derived(preset ? presetRatio(preset.factor, unit) : 0);
  const valid = $derived(name.trim().length > 0 && (mode !== "preset" || ratio > 0));

  function choosePreset(i: number) {
    presetIndex = i;
    unit = SCALE_PRESETS[i].unit;
  }

  function confirm() {
    if (!valid) return;
    const n = name.trim();
    if (mode === "preset") onConfirm({ name: n, mode, ratio, unit, label: preset.label, precision: 2 });
    else onConfirm({ name: n, mode });
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onCancel();
    if (e.key === "Enter" && valid) confirm();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="dialog-backdrop" onclick={onCancel} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" tabindex="-1" aria-modal="true" aria-label={title}>
    <h3 class="dialog-title">{title}</h3>
    <p class="dialog-hint">
      Measurements that start inside this area use its scale. Measurements already on the
      sheet keep their values.
    </p>

    <label class="field-label" for="viewport-name">Name</label>
    <input id="viewport-name" class="field-input" type="text" bind:value={name} aria-label="Viewport name" />

    <fieldset class="scale-modes">
      <legend class="field-label">Scale</legend>
      {#if allowKeep}
        <label><input type="radio" name="vp-mode" value="keep" bind:group={mode} /> Keep the current scale</label>
      {/if}
      <label><input type="radio" name="vp-mode" value="preset" bind:group={mode} /> Drawing scale</label>
      {#if mode === "preset"}
        <div class="field-row">
          <select class="field-select" aria-label="Drawing scale" value={presetIndex}
            onchange={(e) => choosePreset(Number((e.currentTarget as HTMLSelectElement).value))}>
            {#each SCALE_PRESETS as p, i (p.label)}
              <option value={i}>{p.label}</option>
            {/each}
          </select>
          <select class="field-select" aria-label="Unit" bind:value={unit}>
            {#each PRESET_UNITS as u (u)}<option value={u}>{u}</option>{/each}
          </select>
        </div>
      {/if}
      <label><input type="radio" name="vp-mode" value="calibrate" bind:group={mode} /> Calibrate: click two points of a known length</label>
    </fieldset>

    <div class="dialog-actions">
      <button class="btn-secondary" onclick={onCancel}>Cancel</button>
      <button class="btn-primary" onclick={confirm} disabled={!valid}>
        {mode === "calibrate" ? "Next: calibrate" : "Save"}
      </button>
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
  .scale-modes {
    border: none; margin: 0; padding: 0;
    display: flex; flex-direction: column; gap: var(--space-2);
    font-size: var(--font-size-sm); color: var(--color-text);
  }
  .field-input {
    padding: var(--space-2) var(--space-3);
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
