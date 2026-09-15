<script lang="ts">
  /**
   * ExtractPagesDialog - confirms the page selection then runs a native Save-As dialog
   * to pick a destination for a NEW PDF containing just those pages (owner request
   * 2026-09-15, PR-C - final piece of the reading-mode/thumbnails/extract feature set).
   *
   * Follows the ConfirmDialog modal pattern (native <dialog>, CSS custom properties
   * only) rather than inventing a new one. The extraction itself never touches the
   * source document - see extract_pages_to_new_file's doc comment in
   * src-tauri/src/commands/document.rs.
   */
  import { extractPages } from "$lib/ipc";
  import { save as saveDialog } from "@tauri-apps/plugin-dialog";
  import {
    sortedUniquePageIndices,
    formatPageRanges,
    suggestExtractedPath,
  } from "$lib/extract-pages";

  const {
    docId,
    docPath,
    pageIndices,
    onClose,
    onExtracted,
  }: {
    docId: string;
    /** Source document's file path - used only to suggest a destination filename. */
    docPath: string;
    /** 0-based selected page indices, any order/duplicates (as clicked) - normalized
     *  to ascending, deduped order before extraction (see sortedUniquePageIndices). */
    pageIndices: number[];
    onClose: () => void;
    /** Called with the destination path after a successful extraction. */
    onExtracted?: (destPath: string) => void;
  } = $props();

  const ordered = $derived(sortedUniquePageIndices(pageIndices));
  const rangeLabel = $derived(formatPageRanges(pageIndices));

  let extracting = $state(false);
  let error = $state<string | null>(null);

  async function handleExtract() {
    error = null;
    let dest: string | null;
    try {
      dest = await saveDialog({
        defaultPath: suggestExtractedPath(docPath),
        filters: [{ name: "PDF Documents", extensions: ["pdf"] }],
      });
    } catch (e) {
      error = `Could not open the save dialog: ${e instanceof Error ? e.message : String(e)}`;
      return;
    }
    if (!dest) return; // user cancelled the native dialog

    extracting = true;
    try {
      await extractPages({ doc_id: docId, page_indices: ordered, dest_path: dest });
      onExtracted?.(dest);
      onClose();
    } catch (e) {
      error = `Extraction failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      extracting = false;
    }
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape" && !extracting) {
      e.preventDefault();
      onClose();
    }
  }
</script>

<div class="dialog-backdrop" role="presentation" onclick={() => !extracting && onClose()} onkeydown={null}></div>

<dialog
  open
  class="extract-prompt"
  aria-modal="true"
  aria-label="Extract pages"
  onkeydown={handleKeyDown}
>
  <p class="prompt-message">
    Extract {ordered.length} page{ordered.length === 1 ? "" : "s"} into a new PDF?
  </p>
  <p class="prompt-hint">Pages: {rangeLabel}</p>
  {#if error}
    <p class="prompt-error">{error}</p>
  {/if}

  <div class="button-row">
    <button class="btn-confirm" onclick={handleExtract} disabled={extracting || ordered.length === 0}>
      {extracting ? "Extracting…" : "Extract to New PDF…"}
    </button>
    <button class="btn-cancel" onclick={onClose} disabled={extracting}>Cancel</button>
  </div>
</dialog>

<style>
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 900;
  }

  .extract-prompt {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 901;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-panel);
    color: var(--color-text);
    padding: var(--space-5);
    min-width: 340px;
    max-width: 480px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
    outline: none;
    margin: 0;
  }

  .prompt-message {
    font-size: var(--font-size-base);
    font-weight: 600;
    margin: 0 0 var(--space-2);
    color: var(--color-text);
  }

  .prompt-hint {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    margin: 0 0 var(--space-5);
  }

  .prompt-error {
    font-size: var(--font-size-sm);
    color: var(--color-danger);
    margin: 0 0 var(--space-4);
  }

  .button-row {
    display: flex;
    gap: var(--space-2);
    justify-content: flex-end;
  }

  .btn-confirm,
  .btn-cancel {
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-4);
    cursor: pointer;
    transition: background 100ms, border-color 100ms;
    border: 1px solid transparent;
  }

  .btn-confirm {
    background: var(--color-primary);
    color: var(--color-text-inverse);
    border-color: var(--color-primary);
    font-weight: 600;
  }
  .btn-confirm:hover:not(:disabled) {
    background: var(--color-primary-hover);
    border-color: var(--color-primary-hover);
  }
  .btn-confirm:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-cancel {
    background: var(--color-bg-active);
    color: var(--color-text-secondary);
    border-color: var(--color-border);
  }
  .btn-cancel:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }
  .btn-cancel:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
