<script lang="ts">
  /**
   * NumberPagesDialog - Revu's "Page Numbering and Labeling": Style, Prefix, Start and a
   * Page Range. Style none with a blank prefix removes the labels of the range; style none
   * with a prefix makes a text-only label (docs/navigation.md L3, L4, RL2).
   */
  import PageRangePicker from "./PageRangePicker.svelte";
  import type { LabelStyle } from "$lib/navigation-ipc";
  import { LABEL_STYLES, previewLabels } from "$lib/navigation";

  const {
    pageCount,
    currentPage,
    selectedPages = [],
    onConfirm,
    onCancel,
  }: {
    pageCount: number;
    currentPage: number | null;
    selectedPages?: readonly number[];
    onConfirm: (r: { pages: number[]; style: LabelStyle | null; prefix: string; start: number }) => void;
    onCancel: () => void;
  } = $props();

  let styleValue = $state<LabelStyle | "none">("D");
  let prefix = $state("");
  let startText = $state("1");
  let pages = $state<number[] | null>(null);

  const style = $derived<LabelStyle | null>(styleValue === "none" ? null : styleValue);
  const start = $derived(/^\d{1,9}$/.test(startText.trim()) ? Number(startText.trim()) : null);
  const removing = $derived(style === null && prefix.trim() === "");
  const valid = $derived(pages !== null && pages.length > 0 && (start !== null && start >= 1 || style === null));
  const preview = $derived(
    removing || start === null ? [] : previewLabels(style, prefix, start, Math.min(3, pages?.length ?? 3)),
  );

  function confirm() {
    if (!valid || pages === null) return;
    onConfirm({ pages, style, prefix, start: start ?? 1 });
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement) && valid) {
      e.preventDefault();
      confirm();
    }
  }
</script>

<div class="dialog-backdrop" role="presentation" onclick={onCancel}></div>
<dialog open class="number-pages" aria-modal="true" aria-label="Number pages" onkeydown={onKeyDown}>
  <h3>Number pages</h3>
  <div class="grid">
    <label for="np-style">Style</label>
    <select id="np-style" bind:value={styleValue}>
      {#each LABEL_STYLES as s (s.value)}<option value={s.value}>{s.label}</option>{/each}
    </select>
    <label for="np-prefix">Prefix</label>
    <input id="np-prefix" type="text" bind:value={prefix} maxlength="64" placeholder="E-" />
    <label for="np-start">Start</label>
    <input id="np-start" type="text" inputmode="numeric" bind:value={startText} disabled={style === null} />
  </div>
  <PageRangePicker {pageCount} {currentPage} {selectedPages} onchange={(p) => (pages = p)} />
  <p class="preview" aria-live="polite">
    {#if removing}
      Removes the labels of these pages.
    {:else if preview.length > 0}
      Gives: {preview.join(", ")}{(pages?.length ?? 0) > preview.length ? ", ..." : ""}
    {:else}
      Enter a start number of 1 or more.
    {/if}
  </p>
  <div class="buttons">
    <button class="btn-confirm" onclick={confirm} disabled={!valid}>{removing ? "Remove labels" : "OK"}</button>
    <button class="btn-cancel" onclick={onCancel}>Cancel</button>
  </div>
</dialog>

<style>
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 900;
  }
  .number-pages {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 901;
    margin: 0;
    min-width: 340px;
    max-width: 440px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-panel);
    color: var(--color-text);
    padding: var(--space-5);
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  h3 {
    margin: 0;
    font-size: var(--font-size-base);
  }
  .grid {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: var(--space-2) var(--space-3);
    align-items: center;
    font-size: var(--font-size-sm);
  }
  .grid input,
  .grid select {
    background: var(--color-bg);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    padding: 2px var(--space-2);
    font-size: var(--font-size-sm);
  }
  .preview {
    margin: 0;
    min-height: 1.4em;
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
  .btn-confirm,
  .btn-cancel {
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-4);
    cursor: pointer;
    border: 1px solid transparent;
  }
  .btn-confirm {
    background: var(--color-primary);
    color: var(--color-text-inverse);
    font-weight: 600;
  }
  .btn-confirm:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn-cancel {
    background: var(--color-bg-active);
    color: var(--color-text-secondary);
    border-color: var(--color-border);
  }
</style>
