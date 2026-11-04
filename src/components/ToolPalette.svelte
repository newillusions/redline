<script lang="ts">
  import type { MarkupStore, ToolKind } from "$lib/markup-store.svelte";
  import type { NavigationStore } from "$lib/navigation-store.svelte";
  import type { ToolChestStore } from "$lib/toolchest-store.svelte";
  import type { Tool } from "$lib/ipc";
  import { keyLabelFor } from "$lib/tool-keys";
  import { builtinStamps, savedStampTools } from "$lib/builtin-stamps";
  const {
    store,
    navStore = undefined,
    toolChest = undefined,
    readOnlyView = false,
  }: {
    store: MarkupStore;
    navStore?: NavigationStore;
    /** Source of saved stamp tools for the Stamp picker. */
    toolChest?: ToolChestStore;
    /** True in reading mode: tools cannot draw there, so they are disabled with a hint. */
    readOnlyView?: boolean;
  } = $props();

  const READ_ONLY_HINT = "Switch to page view to mark up";

  // Stamp picker: choosing a stamp arms Drawing-mode placement (next click drops it), the
  // same path as a Tool Chest Drawing-mode tool (Viewport.placeToolCopy).
  const stampsBuiltin = builtinStamps();
  const stampsSaved = $derived(toolChest ? savedStampTools(toolChest.sets) : []);
  const stampActive = $derived(
    store.activeTool === "placeTool" && (store.pendingPlacementTool?.markup_type ?? "").startsWith("Stamp"),
  );
  function pickStamp(tool: Tool) {
    store.pendingPlacementTool = tool;
    store.activeTool = "placeTool";
    store.stampPickerOpen = false;
    void toolChest?.recordRecent(tool).catch(() => {});
  }
  function onPickerKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      store.stampPickerOpen = false;
    }
  }
  // Leaving the document view (reading mode) must not leave the picker open.
  $effect(() => { if (readOnlyView) store.stampPickerOpen = false; });

  // The Link tool is not a mark-up tool: it draws a Link annotation (docs/navigation.md
  // RH1). It rides on the Hand tool, so choosing it selects Hand first; choosing any
  // mark-up tool leaves link mode (Viewport watches for that).
  function toggleLinkTool() {
    if (!navStore) return;
    if (navStore.linkMode) {
      navStore.endLinkMode();
    } else {
      store.activeTool = "hand";
      navStore.beginLinkMode();
    }
  }
  const TOOLS: { kind: ToolKind; label: string; title: string; iconClass?: string }[] = [
    { kind: "hand", label: "✋", title: "Pan (Hand)" },
    { kind: "select", label: "↖", title: "Select / Pointer" },
    { kind: "Rectangle", label: "▢", title: "Rectangle" },
    { kind: "Ellipse", label: "◯", title: "Ellipse" },
    { kind: "Line", label: "╱", title: "Line" },
    { kind: "Arrow", label: "↗", title: "Arrow" },
    { kind: "Highlight", label: "▬", title: "Highlight - freeform box; for text, use Select Text" },
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
    // Text mark-up (PDF /Underline /StrikeOut /Squiggly): drag over text, created on
    // release. Shortcuts follow Revu's: U, D, Shift+U (docs/text-markup-tools.md).
    { kind: "Underline", label: "U", title: "Underline text - drag over text", iconClass: "icon-underline" },
    { kind: "StrikeOut", label: "D", title: "Strikethrough text - drag over text", iconClass: "icon-strike" },
    { kind: "Squiggly", label: "U", title: "Squiggly underline - drag over text", iconClass: "icon-squiggly" },
    { kind: "Polyline", label: "⋁", title: "Polyline" },
    { kind: "Polygon", label: "⬠", title: "Polygon" },
    { kind: "Cloud", label: "☁", title: "Cloud - drag a rectangle, or click the corners; click the first point to close" },
    { kind: "CloudPlus", label: "☁+", title: "Cloud+ - draw a cloud, then click to place its callout text" },
    { kind: "Ink", label: "✎", title: "Pen (freehand)" },
    { kind: "Text", label: "A", title: "Text Box" },
    { kind: "Callout", label: "💬", title: "Callout" },
    { kind: "Note", label: "🗒", title: "Sticky Note - click to place, type the comment" },
    { kind: "calibrate", label: "⚖", title: "Calibrate Scale (two-click)" },
    { kind: "MeasurementLength", label: "↔", title: "Measure Length" },
    { kind: "MeasurementPolylength", label: "⤳", title: "Measure Polylength (click each vertex, double-click or Enter to finish)" },
    { kind: "MeasurementArea", label: "⬛", title: "Measure Area" },
    { kind: "MeasurementCount", label: "⊕", title: "Count" },
    { kind: "MeasurementPerimeter", label: "⬡", title: "Measure Perimeter (click each vertex, double-click or Enter to finish)" },
    { kind: "MeasurementVolume", label: "▦", title: "Measure Volume (click each vertex, double-click or Enter to finish)" },
    { kind: "MeasurementAngle", label: "∠", title: "Measure Angle (click first ray, vertex, second ray)" },
    { kind: "MeasurementRadius", label: "⊙", title: "Measure Radius (drag from centre to edge)" },
  ];
  function toolTitle(t: { kind: ToolKind; title: string }): string {
    const key = keyLabelFor(t.kind);
    // Key goes before the first " - " description, else at the end: "Cloud (C) - drag ...".
    const withKey = !key ? t.title : t.title.includes(" - ") ? t.title.replace(" - ", ` (${key}) - `) : `${t.title} (${key})`;
    return readOnlyView ? `${withKey} - ${READ_ONLY_HINT}` : withKey;
  }
</script>
<svelte:window onpointerdown={(e) => {
  if (store.stampPickerOpen && !(e.target as Element | null)?.closest?.(".stamp-wrap")) store.stampPickerOpen = false;
}} />
<div class="tool-strip" role="toolbar" aria-label="Markup tools">
  {#each TOOLS as t (t.kind)}
    <button
      class="tool-btn"
      class:active={store.activeTool === t.kind}
      class:icon-ibeam={t.iconClass === "icon-ibeam"}
      class:icon-underline={t.iconClass === "icon-underline"}
      class:icon-strike={t.iconClass === "icon-strike"}
      class:icon-squiggly={t.iconClass === "icon-squiggly"}
      title={toolTitle(t)}
      aria-pressed={store.activeTool === t.kind}
      disabled={readOnlyView}
      onclick={() => (store.activeTool = t.kind)}
    >{t.label}</button>
    {#if t.kind === "Squiggly"}
      <div class="stamp-wrap">
        <button
          class="tool-btn"
          class:active={stampActive || store.stampPickerOpen}
          title={readOnlyView ? `Stamp (S) - ${READ_ONLY_HINT}` : "Stamp (S) - choose a stamp, then click to place it"}
          aria-label="Stamp tool"
          aria-haspopup="menu"
          aria-expanded={store.stampPickerOpen}
          aria-pressed={stampActive}
          data-testid="tool-stamp"
          disabled={readOnlyView}
          onclick={() => (store.stampPickerOpen = !store.stampPickerOpen)}
        >&#x1F5B9;</button>
        {#if store.stampPickerOpen}
          <div class="stamp-menu" role="menu" aria-label="Stamps" tabindex="-1" data-testid="stamp-picker" onkeydown={onPickerKeyDown}>
            {#each stampsBuiltin as st (st.id)}
              <button class="stamp-item" role="menuitem" onclick={() => pickStamp(st)}>
                <span class="stamp-swatch" style:background={st.appearance.color}></span>{st.name}
              </button>
            {/each}
            {#if stampsSaved.length > 0}
              <div class="stamp-sep" role="separator">Saved stamps</div>
              {#each stampsSaved as st (st.id)}
                <button class="stamp-item" role="menuitem" onclick={() => pickStamp(st)}>
                  <span class="stamp-swatch" style:background={st.appearance.color}></span>{st.name}
                </button>
              {/each}
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  {/each}
  {#if navStore}
    <button
      class="tool-btn"
      class:active={navStore.linkMode}
      title="Link - drag a rectangle, then choose a page or web address (Esc to stop)"
      aria-label="Link tool"
      aria-pressed={navStore.linkMode}
      disabled={readOnlyView || (!navStore.fileEditable && !navStore.linkMode)}
      onclick={toggleLinkTool}
    >&#x1F517;</button>
  {/if}
  {#if readOnlyView}
    <span class="read-only-hint" role="status" data-testid="palette-read-only-hint">{READ_ONLY_HINT}</span>
  {/if}
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
  .tool-btn:disabled { opacity: 0.45; cursor: not-allowed; }
  .tool-btn:disabled:hover { background: var(--color-bg-active); }
  .read-only-hint {
    align-self: center; margin-left: var(--space-2);
    font-size: var(--font-size-sm); color: var(--color-text-secondary);
  }
  .stamp-wrap { position: relative; display: inline-flex; }
  .stamp-menu {
    position: absolute; top: 100%; left: 0; z-index: 50; margin-top: var(--space-1);
    min-width: 200px; padding: var(--space-1);
    background: var(--color-bg-toolbar); border: 1px solid var(--color-border);
    border-radius: var(--radius-sm); box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
    display: flex; flex-direction: column;
  }
  .stamp-item {
    display: flex; align-items: center; gap: var(--space-2); text-align: left;
    background: none; border: 0; color: var(--color-text); cursor: pointer;
    padding: var(--space-1) var(--space-2); border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
  }
  .stamp-item:hover, .stamp-item:focus-visible { background: var(--color-bg-hover); }
  .stamp-swatch { width: 10px; height: 10px; border-radius: 2px; flex-shrink: 0; }
  .stamp-sep {
    margin-top: var(--space-1); padding: var(--space-1) var(--space-2) 0;
    font-size: var(--font-size-sm); color: var(--color-text-secondary);
    border-top: 1px solid var(--color-border);
  }
  .tool-btn:hover { background: var(--color-bg-hover); }
  .tool-btn.active { background: var(--color-primary); color: var(--color-text-inverse); border-color: var(--color-primary); }
  /* Select Text tool: a serif "I" reads as an I-beam/text-cursor glyph (top/bottom
     serifs) - the app's default sans-serif font renders a bare "I" as a single
     stroke, easy to miss entirely (owner feedback, v0.3.22). font-family here is
     a shape choice, not a colour/token override. */
  /* Text mark-up glyphs: the letter itself carries the decoration, so the button reads as
     what the tool draws (underline / strike-through / wavy underline). */
  .tool-btn.icon-underline { text-decoration: underline; font-weight: 700; }
  .tool-btn.icon-strike { text-decoration: line-through; font-weight: 700; }
  .tool-btn.icon-squiggly { text-decoration: underline wavy; font-weight: 700; }
  .tool-btn.icon-ibeam { font-family: Georgia, "Times New Roman", serif; font-weight: 700; }
</style>
