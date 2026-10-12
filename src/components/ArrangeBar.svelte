<script lang="ts">
  /**
   * ArrangeBar - copy / cut / paste, Duplicate, Multiply, Align, Distribute and Order for the
   * selected markups (docs/arrange.md). Shown over the page while the Select tool is active.
   * Every button calls a command in `$lib/arrange-actions` and shows its one-line result; the
   * keyboard shortcuts in Viewport call the same commands.
   */
  import { onDestroy } from "svelte";
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import { markupClipboard } from "$lib/markup-clipboard.svelte";
  import {
    alignSelection, copySelection, cutSelection, distributeSelection, duplicateSelection, multiplySelection,
    orderSelection, pageScale, pasteClipboard, pasteToPages, type ActionResult, type ArrangeCtx,
  } from "$lib/arrange-actions";
  import type { AlignMode, OrderOp } from "$lib/markup-arrange";
  import MultiplyDialog from "./MultiplyDialog.svelte";
  import PasteToPagesDialog from "./PasteToPagesDialog.svelte";

  const {
    store,
    getCtx,
    notice = null,
  }: {
    store: MarkupStore;
    /** Fresh command context (pointer, view centre, scales, page) at the moment of a click. */
    getCtx: () => ArrangeCtx;
    /** A result produced elsewhere (a keyboard shortcut) to show in the bar. */
    notice?: string | null;
  } = $props();

  let dialog = $state<"multiply" | "pages" | null>(null);
  let local = $state<string | null>(null);
  let timer: ReturnType<typeof setTimeout> | null = null;

  const selected = $derived(store.selectedIds.size);
  const canPaste = $derived(markupClipboard.size > 0);
  const status = $derived(local ?? notice);

  function say(r: ActionResult) {
    local = r.message;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => { local = null; }, 3500);
  }
  onDestroy(() => { if (timer) clearTimeout(timer); });

  const run = (f: (c: ArrangeCtx) => ActionResult) => say(f(getCtx()));

  const ALIGN: { mode: AlignMode; label: string; title: string }[] = [
    { mode: "left", label: "Left", title: "Align left edges to the first selected" },
    { mode: "center", label: "Center", title: "Align horizontal centres (same x, a common vertical line) to the first selected" },
    { mode: "right", label: "Right", title: "Align right edges to the first selected" },
    { mode: "top", label: "Top", title: "Align top edges to the first selected" },
    { mode: "middle", label: "Middle", title: "Align vertical middles (same y, a common horizontal line) to the first selected" },
    { mode: "bottom", label: "Bottom", title: "Align bottom edges to the first selected" },
  ];
  const ORDER: { op: OrderOp; label: string; title: string }[] = [
    { op: "front", label: "Front", title: "Bring to front" },
    { op: "forward", label: "Forward", title: "Bring forward one level" },
    { op: "backward", label: "Backward", title: "Send backward one level" },
    { op: "back", label: "Back", title: "Send to back" },
  ];
</script>

<div class="arrange-bar" role="toolbar" aria-label="Copy, align and arrange">
  <div class="group" role="group" aria-label="Copy and paste">
    <button class="btn" disabled={selected < 1} title="Copy (Ctrl/Cmd+C)" onclick={() => run(copySelection)}>Copy</button>
    <button class="btn" disabled={selected < 1} title="Cut (Ctrl/Cmd+X)" onclick={() => run(cutSelection)}>Cut</button>
    <button class="btn" disabled={!canPaste} title="Paste (Ctrl/Cmd+V pastes at the pointer; Ctrl/Cmd+Shift+V in place)" onclick={() => run((c) => pasteClipboard(c, "center"))}>Paste</button>
    <button class="btn" disabled={!canPaste} title="Paste in place onto chosen pages" onclick={() => (dialog = "pages")}>Paste to pages...</button>
    <button class="btn" disabled={selected < 1} title="Duplicate (Ctrl/Cmd+D)" onclick={() => run(duplicateSelection)}>Duplicate</button>
    <button class="btn" disabled={selected < 1} title="Repeat the selection in a row, column or grid" onclick={() => (dialog = "multiply")}>Multiply...</button>
  </div>
  <div class="group" role="group" aria-label="Align">
    <span class="label">Align</span>
    {#each ALIGN as a (a.mode)}
      <button class="btn" disabled={selected < 2} title={a.title} onclick={() => run((c) => alignSelection(c, a.mode))}>{a.label}</button>
    {/each}
  </div>
  <div class="group" role="group" aria-label="Distribute">
    <span class="label">Distribute</span>
    <button class="btn" disabled={selected < 3} title="Space centres evenly, left to right" onclick={() => run((c) => distributeSelection(c, "horizontal"))}>Horizontally</button>
    <button class="btn" disabled={selected < 3} title="Space centres evenly, top to bottom" onclick={() => run((c) => distributeSelection(c, "vertical"))}>Vertically</button>
  </div>
  <div class="group" role="group" aria-label="Order">
    <span class="label">Order</span>
    {#each ORDER as o (o.op)}
      <button class="btn" disabled={selected < 1} title={o.title} onclick={() => run((c) => orderSelection(c, o.op))}>{o.label}</button>
    {/each}
  </div>
  {#if status}<div class="status" role="status" aria-live="polite">{status}</div>{/if}
</div>

{#if dialog === "multiply"}
  <MultiplyDialog
    scale={pageScale(getCtx())}
    onCancel={() => (dialog = null)}
    onSubmit={(spec) => {
      const r = multiplySelection(getCtx(), spec);
      if (!r.ok) return r.message;
      say(r);
      dialog = null;
      return null;
    }}
  />
{:else if dialog === "pages"}
  <PasteToPagesDialog
    pageCount={getCtx().pageCount}
    currentPage={getCtx().pageIndex}
    onCancel={() => (dialog = null)}
    onSubmit={(pages) => {
      const r = pasteToPages(getCtx(), pages);
      if (!r.ok) return r.message;
      say(r);
      dialog = null;
      return null;
    }}
  />
{/if}

<style>
  .arrange-bar {
    position: absolute;
    top: var(--space-2);
    left: 50%;
    transform: translateX(-50%);
    z-index: 25;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
    gap: var(--space-2) var(--space-4);
    max-width: calc(100% - 2 * var(--space-4));
    background: var(--color-bg-panel);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-1) var(--space-3);
    box-shadow: 0 4px 16px rgba(0 0 0 / 0.3);
  }
  .group { display: flex; align-items: center; gap: var(--space-1); }
  .label { font-size: var(--font-size-sm); color: var(--color-text-secondary); margin-right: var(--space-1); }
  .btn {
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg-active);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: var(--font-size-sm);
    white-space: nowrap;
  }
  .btn:hover:not(:disabled) { background: var(--color-bg-hover); }
  .btn:disabled { opacity: 0.45; cursor: default; }
  .status { font-size: var(--font-size-sm); color: var(--color-text); }
</style>
