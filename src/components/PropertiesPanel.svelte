<script lang="ts">
  import { onMount } from "svelte";
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import type { UserRef } from "$lib/ipc";
  import { getUserIdentity } from "$lib/ipc";
  import { patchAppearance, patchFields, commonValue, FONT_FAMILIES, FONT_SIZES } from "$lib/markup-properties";

  const { store }: { store: MarkupStore } = $props();

  let identity = $state<UserRef | null>(null);

  onMount(() => {
    getUserIdentity()
      .then((u) => { identity = u; })
      .catch(() => {});
  });

  const selected = $derived(store.selectedMarkups);
  const mode = $derived(selected.length === 0 ? "draft" : "selection");

  // ---------------------------------------------------------------------------
  // Commit helpers
  // ---------------------------------------------------------------------------

  function by(fallback: UserRef): UserRef {
    return identity ?? fallback;
  }

  function commitAppearancePatch(patch: Parameters<typeof patchAppearance>[1]) {
    if (mode === "draft") {
      Object.assign(store.draftAppearance, patch);
    } else {
      const now = new Date().toISOString();
      const pairs = selected.map((m) => ({
        before: m,
        after: patchAppearance(m, patch, by(m.audit.modified_by), now),
      }));
      store.applyBatch(pairs);
    }
  }

  function commitFieldPatch(patch: Parameters<typeof patchFields>[1]) {
    const now = new Date().toISOString();
    const pairs = selected.map((m) => ({
      before: m,
      after: patchFields(m, patch, by(m.audit.modified_by), now),
    }));
    store.applyBatch(pairs);
  }

  // ---------------------------------------------------------------------------
  // Control value resolution
  // ---------------------------------------------------------------------------

  function colorValue(): string {
    if (mode === "draft") return store.draftAppearance.color;
    return commonValue(selected, (m) => m.appearance.color) ?? "";
  }

  function lineWeightValue(): number | "" {
    if (mode === "draft") return store.draftAppearance.line_weight;
    const v = commonValue(selected, (m) => m.appearance.line_weight);
    return v ?? "";
  }

  function opacityValue(): number {
    if (mode === "draft") return store.draftAppearance.opacity;
    return commonValue(selected, (m) => m.appearance.opacity) ?? 1;
  }

  function opacityDisplay(): string {
    const v = opacityValue();
    return `${Math.round((v as number) * 100)}%`;
  }

  function fillValue(): string {
    if (mode === "draft") return store.draftAppearance.fill ?? "#ffffff";
    return commonValue(selected, (m) => m.appearance.fill ?? "") ?? "#ffffff";
  }

  function noFillChecked(): boolean {
    if (mode === "draft") return store.draftAppearance.fill === null;
    const v = commonValue(selected, (m) => m.appearance.fill);
    // if all null -> checked; if undefined (mixed) -> leave unchecked
    return v === null;
  }

  function lineStyleValue(): string {
    if (mode === "draft") return store.draftAppearance.line_style;
    return commonValue(selected, (m) => m.appearance.line_style) ?? "";
  }

  function fontFamilyValue(): string {
    if (mode === "draft") return store.draftAppearance.font?.family ?? "Helvetica";
    return commonValue(selected, (m) => m.appearance.font?.family ?? "") ?? "";
  }

  function fontSizeValue(): number | "" {
    if (mode === "draft") return store.draftAppearance.font?.size_pt ?? 12;
    const v = commonValue(selected, (m) => m.appearance.font?.size_pt ?? 0);
    return v ?? "";
  }

  function contentsValue(): string {
    return commonValue(selected, (m) => m.contents ?? "") ?? "";
  }

  function subjectValue(): string {
    return commonValue(selected, (m) => m.subject ?? "") ?? "";
  }

  function layerValue(): string {
    return commonValue(selected, (m) => m.layer ?? "") ?? "";
  }

  // ---------------------------------------------------------------------------
  // Event handlers
  // ---------------------------------------------------------------------------

  function onColorInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    commitAppearancePatch({ color: v });
  }

  function onLineWeightInput(e: Event) {
    const raw = parseFloat((e.target as HTMLInputElement).value);
    if (!isNaN(raw)) commitAppearancePatch({ line_weight: raw });
  }

  function onOpacityInput(e: Event) {
    const raw = parseFloat((e.target as HTMLInputElement).value);
    if (!isNaN(raw)) commitAppearancePatch({ opacity: raw });
  }

  function onFillColorInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    if (!noFillChecked()) commitAppearancePatch({ fill: v });
  }

  function onNoFillChange(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    if (checked) {
      commitAppearancePatch({ fill: null });
    } else {
      // restore a default fill color
      commitAppearancePatch({ fill: "#ffffff" });
    }
  }

  function onLineStyleChange(e: Event) {
    const v = (e.target as HTMLSelectElement).value as "Solid" | "Dashed" | "Dotted";
    commitAppearancePatch({ line_style: v });
  }

  function onFontFamilyChange(e: Event) {
    const family = (e.target as HTMLSelectElement).value;
    const currentFont = mode === "draft"
      ? store.draftAppearance.font
      : commonValue(selected, (m) => m.appearance.font);
    const size_pt = currentFont?.size_pt ?? 12;
    commitAppearancePatch({ font: { family, size_pt } });
  }

  function onFontSizeChange(e: Event) {
    const size_pt = parseFloat((e.target as HTMLSelectElement).value);
    if (isNaN(size_pt)) return;
    const currentFont = mode === "draft"
      ? store.draftAppearance.font
      : commonValue(selected, (m) => m.appearance.font);
    const family = currentFont?.family ?? "Helvetica";
    commitAppearancePatch({ font: { family, size_pt } });
  }

  function onContentsInput(e: Event) {
    const v = (e.target as HTMLTextAreaElement).value;
    commitFieldPatch({ contents: v || null });
  }

  function onSubjectInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    commitFieldPatch({ subject: v || null });
  }

  function onLayerInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    commitFieldPatch({ layer: v || null });
  }
</script>

<div class="properties-panel">
  {#if mode === "draft"}
    <p class="panel-mode-hint muted">Defaults for new markups</p>
  {:else}
    <p class="panel-mode-hint selected-count">{selected.length} markup(s) selected</p>
  {/if}

  <!-- Appearance group -->
  <section class="prop-group">
    <h3 class="prop-group-title">Appearance</h3>

    <!-- Color -->
    <div class="prop-row">
      <label for="prop-color" class="prop-label">Color</label>
      <div class="prop-control color-row">
        <div class="color-swatch" style="background: {colorValue() || 'transparent'}"></div>
        <input
          id="prop-color"
          type="color"
          data-field="color"
          data-indeterminate={colorValue() === "" ? "true" : undefined}
          value={colorValue() || "#000000"}
          oninput={onColorInput}
        />
      </div>
    </div>

    <!-- Line weight -->
    <div class="prop-row">
      <label for="prop-line-weight" class="prop-label">Line weight (pt)</label>
      <input
        id="prop-line-weight"
        type="number"
        data-field="line_weight"
        min="0"
        step="0.5"
        value={lineWeightValue()}
        oninput={onLineWeightInput}
        class="prop-number"
      />
    </div>

    <!-- Opacity -->
    <div class="prop-row">
      <label for="prop-opacity" class="prop-label">Opacity</label>
      <div class="prop-control opacity-row">
        <input
          id="prop-opacity"
          type="range"
          data-field="opacity"
          min="0"
          max="1"
          step="0.05"
          value={opacityValue()}
          oninput={onOpacityInput}
          class="prop-range"
        />
        <span class="opacity-readout muted">{opacityDisplay()}</span>
      </div>
    </div>

    <!-- Fill -->
    <div class="prop-row">
      <label for="prop-fill" class="prop-label">Fill</label>
      <div class="prop-control fill-row">
        <input
          id="prop-fill"
          type="color"
          data-field="fill_color"
          value={fillValue()}
          disabled={noFillChecked()}
          oninput={onFillColorInput}
        />
        <label class="no-fill-label">
          <input
            type="checkbox"
            data-field="no_fill"
            checked={noFillChecked()}
            onchange={onNoFillChange}
          />
          No fill
        </label>
      </div>
    </div>

    <!-- Line style -->
    <div class="prop-row">
      <label for="prop-line-style" class="prop-label">Line style</label>
      <select
        id="prop-line-style"
        data-field="line_style"
        value={lineStyleValue()}
        onchange={onLineStyleChange}
        class="prop-select"
      >
        {#if !lineStyleValue()}
          <option value="" disabled selected>Mixed</option>
        {/if}
        <option value="Solid">Solid</option>
        <option value="Dashed">Dashed</option>
        <option value="Dotted">Dotted</option>
      </select>
    </div>
  </section>

  <!-- Font group (shown in both draft and selection mode) -->
  <section class="prop-group">
    <h3 class="prop-group-title">Font</h3>

    <div class="prop-row">
      <label for="prop-font-family" class="prop-label">Family</label>
      <select
        id="prop-font-family"
        data-field="font_family"
        value={fontFamilyValue()}
        onchange={onFontFamilyChange}
        class="prop-select"
      >
        {#if !fontFamilyValue()}
          <option value="" disabled selected>Mixed</option>
        {/if}
        {#each FONT_FAMILIES as family (family)}
          <option value={family}>{family}</option>
        {/each}
      </select>
    </div>

    <div class="prop-row">
      <label for="prop-font-size" class="prop-label">Size (pt)</label>
      <select
        id="prop-font-size"
        data-field="font_size"
        value={fontSizeValue()}
        onchange={onFontSizeChange}
        class="prop-select"
      >
        {#if !fontSizeValue()}
          <option value="" disabled selected>Mixed</option>
        {/if}
        {#each FONT_SIZES as size (size)}
          <option value={size}>{size}</option>
        {/each}
      </select>
    </div>
  </section>

  <!-- Text fields (selection mode only) -->
  {#if mode === "selection"}
    <section class="prop-group">
      <h3 class="prop-group-title">Content</h3>

      <div class="prop-row prop-row-stacked">
        <label for="prop-contents" class="prop-label">Contents</label>
        <textarea
          id="prop-contents"
          data-field="contents"
          rows="3"
          value={contentsValue()}
          oninput={onContentsInput}
          class="prop-textarea"
          placeholder={contentsValue() === "" && selected.length > 1 ? "Mixed" : ""}
        ></textarea>
      </div>

      <div class="prop-row">
        <label for="prop-subject" class="prop-label">Subject</label>
        <input
          id="prop-subject"
          type="text"
          data-field="subject"
          value={subjectValue()}
          oninput={onSubjectInput}
          class="prop-text"
          placeholder={subjectValue() === "" && selected.length > 1 ? "Mixed" : ""}
        />
      </div>

      <div class="prop-row">
        <label for="prop-layer" class="prop-label">Layer</label>
        <input
          id="prop-layer"
          type="text"
          data-field="layer"
          value={layerValue()}
          oninput={onLayerInput}
          class="prop-text"
          placeholder={layerValue() === "" && selected.length > 1 ? "Mixed" : ""}
        />
      </div>
    </section>
  {/if}
</div>

<style>
  .properties-panel {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    overflow-y: auto;
    flex: 1;
  }

  .panel-mode-hint {
    font-size: var(--font-size-sm);
    margin: 0 0 var(--space-1) 0;
    font-weight: 500;
  }

  .panel-mode-hint.muted {
    color: var(--color-text-muted);
  }

  .panel-mode-hint.selected-count {
    color: var(--color-text);
  }

  .prop-group {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-2) 0;
    border-bottom: 1px solid var(--color-border);
  }

  .prop-group:last-child {
    border-bottom: none;
  }

  .prop-group-title {
    font-size: var(--font-size-sm);
    font-weight: 600;
    color: var(--color-text-muted);
    margin: 0 0 var(--space-1) 0;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .prop-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--space-7);
  }

  .prop-row-stacked {
    flex-direction: column;
    align-items: flex-start;
  }

  .prop-label {
    flex: 0 0 auto;
    min-width: 90px;
    font-size: var(--font-size-sm);
    color: var(--color-text);
  }

  .prop-control {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  .color-row {
    gap: var(--space-2);
  }

  .color-swatch {
    width: var(--space-5);
    height: var(--space-5);
    border-radius: var(--radius-sm);
    border: 1px solid var(--color-border);
    flex-shrink: 0;
  }

  .opacity-row {
    gap: var(--space-2);
  }

  .prop-range {
    flex: 1;
    accent-color: var(--color-primary);
  }

  .opacity-readout {
    flex: 0 0 auto;
    font-size: var(--font-size-sm);
    min-width: 36px;
    text-align: right;
  }

  .fill-row {
    gap: var(--space-2);
  }

  .no-fill-label {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--font-size-sm);
    color: var(--color-text);
    cursor: pointer;
  }

  .prop-number,
  .prop-text {
    flex: 1;
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg-input, var(--color-bg-active));
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
  }

  .prop-select {
    flex: 1;
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg-input, var(--color-bg-active));
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
  }

  .prop-textarea {
    width: 100%;
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg-input, var(--color-bg-active));
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    resize: vertical;
    font-family: inherit;
  }

  .muted {
    color: var(--color-text-muted);
  }
</style>
