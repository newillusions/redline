<script lang="ts">
  import type { MarkupStore, ToolKind } from "$lib/markup-store.svelte";
  const { store }: { store: MarkupStore } = $props();
  const TOOLS: { kind: ToolKind; label: string; title: string; iconClass?: string }[] = [
    { kind: "hand", label: "✋", title: "Pan (Hand)" },
    { kind: "select", label: "↖", title: "Select / Pointer (V)" },
    { kind: "Rectangle", label: "▢", title: "Rectangle" },
    { kind: "Ellipse", label: "◯", title: "Ellipse" },
    { kind: "Line", label: "╱", title: "Line" },
    { kind: "Arrow", label: "↗", title: "Arrow" },
    { kind: "Highlight", label: "▬", title: "Highlight (freeform box - for text, use Select Text)" },
    // Placed immediately after Highlight (not with the other draw tools) so the
    // text-anchored alternative is discoverable right where users look for it -
    // Acrobat/Bluebeam users expect "Highlight" itself to snap to text (redline#29).
    //
    // The label is a serif capital I, styled via .icon-ibeam below - a sans-serif
    // "I" (the app's default UI font) renders as a bare vertical stroke, nearly
    // invisible against the button and indistinguishable from an empty button
    // (owner feedback, v0.3.22: "the icon on the toolbar isn't obvious"). A serif
    // face gives it top/bottom serifs, the actual shape a text-cursor/I-beam
    // glyph is named for - recognisable at a glance and distinct from the Text
    // tool's "A" (Callout icon, positioned well away from this one already).
    {
      kind: "selectText",
      label: "I",
      title: "Select Text (drag to select; Enter highlights, Ctrl/Cmd+C copies)",
      iconClass: "icon-ibeam",
    },
    { kind: "Polyline", label: "⋁", title: "Polyline" },
    { kind: "Polygon", label: "⬠", title: "Polygon" },
    { kind: "Cloud", label: "☁", title: "Cloud" },
    { kind: "Ink", label: "✎", title: "Ink (Freehand)" },
    { kind: "Text", label: "A", title: "Text" },
    { kind: "Callout", label: "💬", title: "Callout" },
    { kind: "calibrate", label: "⚖", title: "Calibrate Scale (two-click)" },
    { kind: "MeasurementLength", label: "↔", title: "Measure Length" },
    { kind: "MeasurementArea", label: "⬛", title: "Measure Area" },
    { kind: "MeasurementCount", label: "⊕", title: "Count" },
    { kind: "MeasurementPerimeter", label: "⬡", title: "Measure Perimeter (click each vertex, double-click or Enter to finish)" },
    { kind: "MeasurementVolume", label: "▦", title: "Measure Volume (click each vertex, double-click or Enter to finish)" },
    { kind: "MeasurementAngle", label: "∠", title: "Measure Angle (click first ray, vertex, second ray)" },
    { kind: "MeasurementRadius", label: "⊙", title: "Measure Radius (drag from centre to edge)" },
  ];
</script>
<div class="tool-strip" role="toolbar" aria-label="Markup tools">
  {#each TOOLS as t (t.kind)}
    <button
      class="tool-btn"
      class:active={store.activeTool === t.kind}
      class:icon-ibeam={t.iconClass === "icon-ibeam"}
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
  /* Select Text tool: a serif "I" reads as an I-beam/text-cursor glyph (top/bottom
     serifs) - the app's default sans-serif font renders a bare "I" as a single
     stroke, easy to miss entirely (owner feedback, v0.3.22). font-family here is
     a shape choice, not a colour/token override. */
  .tool-btn.icon-ibeam { font-family: Georgia, "Times New Roman", serif; font-weight: 700; }
</style>
