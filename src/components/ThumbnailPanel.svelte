<script lang="ts">
  /**
   * ThumbnailPanel — vertical strip of page thumbnails with drag-to-reorder.
   *
   * M4 S1: page thumbnail strip, drag-to-reorder, delete, rotate controls.
   * Calls page-op IPC functions on user action.
   */
  import { reorderPages, deletePage, rotatePage } from "$lib/ipc";

  interface Props {
    docId: string;
    pageCount: number;
    /** Optional callback when pages change (e.g. to trigger re-render). */
    onPageOp?: () => void;
  }

  const { docId, pageCount, onPageOp }: Props = $props();

  // ---------------------------------------------------------------------------
  // Drag-to-reorder state
  // ---------------------------------------------------------------------------

  let dragSrcIdx = $state<number | null>(null);
  let dragOverIdx = $state<number | null>(null);

  function handleDragStart(e: DragEvent, idx: number) {
    dragSrcIdx = idx;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", String(idx));
    }
  }

  function handleDragOver(e: DragEvent, idx: number) {
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = "move";
    }
    dragOverIdx = idx;
  }

  function handleDragLeave() {
    dragOverIdx = null;
  }

  function handleDragEnd() {
    dragSrcIdx = null;
    dragOverIdx = null;
  }

  async function handleDrop(e: DragEvent, targetIdx: number) {
    e.preventDefault();
    const src = dragSrcIdx;
    dragSrcIdx = null;
    dragOverIdx = null;
    if (src === null || src === targetIdx) return;

    // Build new_order permutation: move src page to targetIdx.
    const order = Array.from({ length: pageCount }, (_, i) => i);
    order.splice(src, 1);
    order.splice(targetIdx, 0, src);

    await reorderPages({ doc_id: docId, new_order: order });
    onPageOp?.();
  }

  // ---------------------------------------------------------------------------
  // Rotation
  // ---------------------------------------------------------------------------

  async function handleRotate(idx: number, degrees: number) {
    await rotatePage({ doc_id: docId, page_idx: idx, degrees });
    onPageOp?.();
  }

  // ---------------------------------------------------------------------------
  // Delete with confirmation
  // ---------------------------------------------------------------------------

  async function handleDelete(idx: number) {
    if (pageCount <= 1) return; // guard: cannot delete the only page
    const confirmed = window.confirm(`Delete page ${idx + 1}? This cannot be undone.`);
    if (!confirmed) return;
    await deletePage({ doc_id: docId, page_idx: idx });
    onPageOp?.();
  }
</script>

<aside class="thumbnail-panel" aria-label="Page thumbnails">
  {#each Array.from({ length: pageCount }, (_, i) => i) as idx (idx)}
    <div
      class="thumbnail"
      class:drag-over={dragOverIdx === idx}
      class:drag-src={dragSrcIdx === idx}
      draggable="true"
      aria-label={`Page ${idx + 1}`}
      role="listitem"
      ondragstart={(e) => handleDragStart(e, idx)}
      ondragover={(e) => handleDragOver(e, idx)}
      ondragleave={handleDragLeave}
      ondragend={handleDragEnd}
      ondrop={(e) => handleDrop(e, idx)}
    >
      <div class="thumbnail-number">{idx + 1}</div>
      <div class="thumbnail-preview" aria-hidden="true">
        <!-- Placeholder: future PDFium tile render goes here -->
        <span class="preview-placeholder">p{idx + 1}</span>
      </div>
      <div class="thumbnail-controls">
        <button
          class="ctrl-btn"
          title="Rotate 90° clockwise"
          aria-label={`Rotate page ${idx + 1} 90 degrees clockwise`}
          onclick={() => handleRotate(idx, 90)}
        >↻</button>
        <button
          class="ctrl-btn danger"
          title="Delete page"
          aria-label={`Delete page ${idx + 1}`}
          disabled={pageCount <= 1}
          onclick={() => handleDelete(idx)}
        >✕</button>
      </div>
    </div>
  {/each}
</aside>

<style>
  .thumbnail-panel {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2);
    background: var(--color-bg-panel);
    border-right: 1px solid var(--color-border);
    overflow-y: auto;
    width: var(--panel-left-width);
    flex-shrink: 0;
  }

  .thumbnail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-2);
    background: var(--color-bg-panel-alt);
    border: 1px solid var(--color-border-subtle);
    border-radius: var(--radius-md);
    cursor: grab;
    user-select: none;
    transition: border-color 120ms, background 120ms;
  }

  .thumbnail:hover {
    border-color: var(--color-border);
    background: var(--color-bg-hover);
  }

  .thumbnail.drag-over {
    border-color: var(--color-primary);
    background: var(--color-bg-active);
  }

  .thumbnail.drag-src {
    opacity: 0.5;
  }

  .thumbnail-number {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    align-self: flex-start;
  }

  .thumbnail-preview {
    width: 100%;
    aspect-ratio: 3 / 4;
    background: var(--color-bg);
    border: 1px solid var(--color-border-subtle);
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .preview-placeholder {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
  }

  .thumbnail-controls {
    display: flex;
    gap: var(--space-1);
    width: 100%;
    justify-content: flex-end;
  }

  .ctrl-btn {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: 2px var(--space-1);
    line-height: 1;
    transition: background 120ms;
  }

  .ctrl-btn:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }

  .ctrl-btn.danger:hover:not(:disabled) {
    background: var(--color-danger);
    border-color: var(--color-danger);
    color: var(--color-text-inverse);
  }

  .ctrl-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }
</style>
