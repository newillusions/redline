<script lang="ts">
  /**
   * PasteToPagesDialog - paste the clipboard in place onto a typed list of pages (docs/arrange.md
   * C9): `1-3, 5` (1-based), or every page. `onSubmit` returns an error message to keep the
   * dialog open, or null when it worked.
   */
  import { parsePageSpec } from "$lib/legend";
  import { pagesFromRanges } from "$lib/arrange-actions";

  const {
    pageCount,
    currentPage,
    onSubmit,
    onCancel,
  }: {
    pageCount: number;
    /** 0-based page being viewed; proposed as the starting text. */
    currentPage: number;
    onSubmit: (pages: number[]) => string | null;
    onCancel: () => void;
  } = $props();

  let text = $state(String(currentPage + 1));
  let error = $state<string | null>(null);

  function submit(all: boolean) {
    if (all) {
      error = onSubmit(Array.from({ length: pageCount }, (_, i) => i));
      return;
    }
    const ranges = parsePageSpec(text);
    const pages = ranges ? pagesFromRanges(ranges, pageCount) : [];
    if (pages.length === 0) {
      error = `Enter page numbers between 1 and ${pageCount}, such as 1-3, 5.`;
      return;
    }
    error = onSubmit(pages);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); onCancel(); }
    else if (e.key === "Enter") { e.preventDefault(); submit(false); }
    e.stopPropagation(); // typing must not reach the viewport's single-key tool shortcuts
  }
</script>

<div class="dialog-backdrop" role="presentation" onclick={onCancel} onkeydown={null}></div>

<dialog open class="paste-pages" aria-modal="true" aria-label="Paste to pages" onkeydown={onKey}>
  <p class="title">Paste to pages</p>
  <label class="field" for="paste-pages-input">Pages (1 to {pageCount})</label>
  <!-- svelte-ignore a11y_autofocus -->
  <input id="paste-pages-input" class="input" type="text" bind:value={text} autofocus aria-label="Pages" />
  <p class="hint">The markups are pasted at the same position on each page.</p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="button-row">
    <button class="btn-cancel" onclick={onCancel}>Cancel</button>
    <button class="btn-cancel" onclick={() => submit(true)}>All pages</button>
    <button class="btn-submit" onclick={() => submit(false)}>Paste</button>
  </div>
</dialog>

<style>
  .dialog-backdrop { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.45); z-index: 900; }
  .paste-pages {
    position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%); z-index: 901;
    border: 1px solid var(--color-border); border-radius: var(--radius-md);
    background: var(--color-bg-panel); color: var(--color-text);
    padding: var(--space-5); min-width: 320px; max-width: 420px; margin: 0;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
  }
  .title { font-size: var(--font-size-base); font-weight: 600; margin: 0 0 var(--space-3); }
  .field { display: block; font-size: var(--font-size-sm); margin-bottom: var(--space-1); }
  .input {
    width: 100%; padding: var(--space-2) var(--space-3); box-sizing: border-box;
    border: 1px solid var(--color-border); border-radius: var(--radius-sm);
    background: var(--color-bg-input); color: var(--color-text); font-size: var(--font-size-base);
  }
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
