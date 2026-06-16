<script lang="ts">
  import type { MarkupStore, ToolKind } from "$lib/markup-store.svelte";
  const { store }: { store: MarkupStore } = $props();
  const TOOLS: { kind: ToolKind; label: string; title: string }[] = [
    { kind: "hand", label: "✋", title: "Pan (Hand)" },
    { kind: "select", label: "▭", title: "Select" },
    { kind: "Rectangle", label: "▢", title: "Rectangle" },
    { kind: "Ellipse", label: "◯", title: "Ellipse" },
    { kind: "Line", label: "╱", title: "Line" },
    { kind: "Arrow", label: "↗", title: "Arrow" },
    { kind: "Highlight", label: "▬", title: "Highlight" },
    { kind: "Polyline", label: "⋁", title: "Polyline" },
    { kind: "Polygon", label: "⬠", title: "Polygon" },
    { kind: "Cloud", label: "☁", title: "Cloud" },
    { kind: "Ink", label: "✎", title: "Ink (Freehand)" },
    { kind: "Text", label: "A", title: "Text" },
    { kind: "Callout", label: "💬", title: "Callout" },
  ];
</script>
<div class="tool-strip" role="toolbar" aria-label="Markup tools">
  {#each TOOLS as t (t.kind)}
    <button
      class="tool-btn"
      class:active={store.activeTool === t.kind}
      title={t.title}
      aria-pressed={store.activeTool === t.kind}
      onclick={() => (store.activeTool = t.kind)}
    >{t.label}</button>
  {/each}
</div>
<style>
  .tool-strip {
    display: flex; gap: var(--space-1);
    padding: var(--space-1) var(--space-3);
    background: var(--color-bg-toolbar);
    border-bottom: 1px solid var(--color-border);
    flex-shrink: 0;
  }
  .tool-btn {
    background: var(--color-bg-active); border: 1px solid var(--color-border);
    border-radius: var(--radius-sm); color: var(--color-text);
    cursor: pointer; font-size: var(--font-size-base);
    width: var(--space-8); height: var(--space-8); /* no 28px token; --space-8 (32px) is nearest, makes a square button */
    line-height: 1; transition: background 120ms;
  }
  .tool-btn:hover { background: var(--color-bg-hover); }
  .tool-btn.active { background: var(--color-primary); color: var(--color-text-inverse); border-color: var(--color-primary); }
</style>
