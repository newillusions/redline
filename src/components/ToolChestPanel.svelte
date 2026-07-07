<script lang="ts">
  /**
   * Tool Chest panel (spec "Tools & Tool Sets" / "Stamps" / "Importing Bluebeam Tool
   * Sets") - lists Tool Sets + Recent Tools; clicking a tool makes it the active tool
   * (Properties mode applies its appearance to the next drawn markup; Drawing mode arms a
   * click-to-place copy - see `$lib/toolchest-activation`). Also offers "save the selected
   * markup as a tool" and ".btx" import.
   *
   * Surfaced in App.svelte's left panel so it is discoverable alongside Recent Documents -
   * addressing the core complaint that no tool-collection UI existed.
   */
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { ToolChestStore } from "$lib/toolchest-store.svelte";
  import { activateTool } from "$lib/toolchest-activation";
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import type { Tool, ToolSet, PlacementMode, MarkupType } from "$lib/ipc";

  const {
    toolChest,
    markupStore = null,
  }: {
    toolChest: ToolChestStore;
    /** The active document tab's markup store, or null when no document is open. Needed
     *  to activate a tool and to read the current selection for "save as tool". */
    markupStore?: MarkupStore | null;
  } = $props();

  onMount(() => {
    void toolChest.load();
  });

  // ---------------------------------------------------------------------------
  // Tool glyphs (mirrors ToolPalette's icon vocabulary)
  // ---------------------------------------------------------------------------
  const GLYPH: Partial<Record<MarkupType, string>> = {
    Rectangle: "▢", Ellipse: "◯", Line: "╱", Arrow: "↗", Highlight: "▬",
    Polyline: "⋁", Polygon: "⬠", Cloud: "☁", Ink: "✎", Text: "A", Callout: "💬",
    Stamp: "🖃", StampDynamic: "🖃",
    MeasurementLength: "↔", MeasurementArea: "⬛", MeasurementCount: "⊕",
    MeasurementPerimeter: "⬠", MeasurementVolume: "⬛", MeasurementAngle: "∠", MeasurementRadius: "↔",
  };
  function glyph(t: MarkupType): string {
    return GLYPH[t] ?? "●";
  }

  // ---------------------------------------------------------------------------
  // Activation
  // ---------------------------------------------------------------------------
  function handleActivate(tool: Tool) {
    if (!markupStore) return;
    activateTool(tool, markupStore);
    void toolChest.recordRecent(tool);
  }

  // ---------------------------------------------------------------------------
  // Set collapse state (local UI only - not persisted)
  // ---------------------------------------------------------------------------
  let collapsedSets = $state<Set<string>>(new Set());
  function toggleSet(setId: string) {
    const next = new Set(collapsedSets);
    if (next.has(setId)) next.delete(setId);
    else next.add(setId);
    collapsedSets = next;
  }

  // ---------------------------------------------------------------------------
  // New set
  // ---------------------------------------------------------------------------
  let newSetName = $state("");
  let creatingSet = $state(false);
  async function handleCreateSet() {
    const name = newSetName.trim();
    if (!name) return;
    creatingSet = true;
    try {
      await toolChest.createSet(name);
      newSetName = "";
    } finally {
      creatingSet = false;
    }
  }

  // ---------------------------------------------------------------------------
  // .btx import
  // ---------------------------------------------------------------------------
  let importStatus = $state<string | null>(null);
  let importing = $state(false);
  async function handleImport() {
    const selected = await open({
      title: "Import Bluebeam Tool Set",
      filters: [{ name: "Bluebeam Tool Set", extensions: ["btx", "zip"] }],
      multiple: false,
    });
    if (!selected || Array.isArray(selected)) return;
    importing = true;
    importStatus = null;
    try {
      const report = await toolChest.importBtx(selected as string);
      const parts = [`Imported ${report.tools.length} tool${report.tools.length === 1 ? "" : "s"}`];
      if (report.skipped.length > 0) parts.push(`${report.skipped.length} skipped`);
      importStatus = parts.join(", ");
    } catch (e) {
      importStatus = `Import failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      importing = false;
    }
  }

  // ---------------------------------------------------------------------------
  // Save current markup as tool
  // ---------------------------------------------------------------------------
  const selectedMarkup = $derived(
    markupStore && markupStore.selectedMarkups.length === 1 ? markupStore.selectedMarkups[0] : null,
  );
  let saveToolName = $state("");
  let saveTargetSetId = $state<string>("");
  let savePlacementMode = $state<PlacementMode>("Properties");
  let savingTool = $state(false);

  // Keep the target-set picker pointed at a real set once one exists.
  $effect(() => {
    if (!saveTargetSetId && toolChest.sets.length > 0) {
      saveTargetSetId = toolChest.sets[0].id;
    }
  });

  async function handleSaveAsTool() {
    if (!selectedMarkup || !saveTargetSetId || !saveToolName.trim()) return;
    savingTool = true;
    try {
      await toolChest.addToolFromMarkup(saveTargetSetId, selectedMarkup, saveToolName.trim(), savePlacementMode);
      saveToolName = "";
    } finally {
      savingTool = false;
    }
  }

  async function handleDeleteTool(set: ToolSet, tool: Tool) {
    await toolChest.deleteTool(set.id, tool.id);
  }

  async function handleDeleteSet(set: ToolSet) {
    await toolChest.deleteSet(set.id);
  }
</script>

<div class="toolchest-panel" aria-label="Tool Chest">
  <!-- Recent Tools -->
  <div class="tc-section">
    <div class="tc-subheader">Recent</div>
    {#if toolChest.recent.length === 0}
      <p class="tc-hint muted">Tools you use show up here.</p>
    {:else}
      <div class="tc-tool-grid">
        {#each toolChest.recent as tool (tool.id)}
          <button
            class="tc-tool-chip"
            title="{tool.name} ({tool.placement_mode})"
            onclick={() => handleActivate(tool)}
            disabled={!markupStore}
          >
            <span class="tc-glyph">{glyph(tool.markup_type)}</span>
            <span class="tc-tool-name">{tool.name}</span>
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <!-- Tool Sets -->
  <div class="tc-section">
    <div class="tc-subheader-row">
      <span class="tc-subheader">Tool Sets</span>
    </div>

    {#if toolChest.loading}
      <p class="tc-hint muted">Loading…</p>
    {:else if toolChest.error}
      <p class="tc-hint tc-error">{toolChest.error}</p>
    {:else if toolChest.sets.length === 0}
      <p class="tc-hint muted">No tool sets yet - create one below or import a .btx file.</p>
    {/if}

    {#each toolChest.sets as set (set.id)}
      {@const collapsed = collapsedSets.has(set.id)}
      <div class="tc-set">
        <div class="tc-set-header">
          <button class="tc-set-toggle" onclick={() => toggleSet(set.id)} aria-expanded={!collapsed}>
            {collapsed ? "▸" : "▾"} {set.name}
            <span class="tc-set-count">({set.tools.length})</span>
          </button>
          <button class="tc-icon-btn" title="Delete set" onclick={() => handleDeleteSet(set)}>✕</button>
        </div>
        {#if !collapsed}
          {#if set.tools.length === 0}
            <p class="tc-hint muted tc-indent">Empty set.</p>
          {:else}
            <ul class="tc-tool-list">
              {#each set.tools as tool (tool.id)}
                <li class="tc-tool-row">
                  <button
                    class="tc-tool-btn"
                    title="{tool.name} ({tool.placement_mode})"
                    onclick={() => handleActivate(tool)}
                    disabled={!markupStore}
                  >
                    <span class="tc-glyph">{glyph(tool.markup_type)}</span>
                    <span class="tc-tool-name">{tool.name}</span>
                  </button>
                  <button class="tc-icon-btn" title="Delete tool" onclick={() => handleDeleteTool(set, tool)}>✕</button>
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      </div>
    {/each}

    <!-- New set -->
    <div class="tc-new-set">
      <input
        class="tc-text-input"
        type="text"
        placeholder="New set name…"
        bind:value={newSetName}
        onkeydown={(e) => { if (e.key === "Enter") void handleCreateSet(); }}
      />
      <button class="btn-toolbar" onclick={handleCreateSet} disabled={creatingSet || !newSetName.trim()}>
        + Add Set
      </button>
    </div>

    <!-- Import .btx -->
    <button class="btn-toolbar tc-import-btn" onclick={handleImport} disabled={importing}>
      {importing ? "Importing…" : "Import Bluebeam Tool Set (.btx)…"}
    </button>
    {#if importStatus}
      <p class="tc-hint">{importStatus}</p>
    {/if}
  </div>

  <!-- Save current markup as tool -->
  {#if selectedMarkup}
    <div class="tc-section tc-save-section">
      <div class="tc-subheader">Save Selected as Tool</div>
      <input class="tc-text-input" type="text" placeholder="Tool name…" bind:value={saveToolName} />
      <div class="tc-save-row">
        <select class="tc-select" bind:value={saveTargetSetId}>
          {#each toolChest.sets as set (set.id)}
            <option value={set.id}>{set.name}</option>
          {/each}
        </select>
        <select class="tc-select" bind:value={savePlacementMode}>
          <option value="Properties">Properties</option>
          <option value="Drawing">Drawing</option>
        </select>
      </div>
      <button
        class="btn-toolbar"
        onclick={handleSaveAsTool}
        disabled={savingTool || !saveToolName.trim() || !saveTargetSetId || toolChest.sets.length === 0}
      >
        Save as Tool
      </button>
    </div>
  {/if}
</div>

<style>
  .toolchest-panel {
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .tc-section {
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--color-border-subtle);
  }

  .tc-subheader {
    font-size: var(--font-size-xs);
    font-weight: 600;
    text-transform: uppercase;
    color: var(--color-text-secondary);
    margin-bottom: var(--space-2);
  }

  .tc-subheader-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-2);
  }

  .tc-hint {
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
    margin: var(--space-1) 0;
  }

  .tc-hint.muted {
    color: var(--color-text-muted);
  }

  .tc-hint.tc-error {
    color: var(--color-danger);
  }

  .tc-indent {
    padding-left: var(--space-3);
  }

  /* Recent Tools grid */
  .tc-tool-grid {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }

  .tc-tool-chip {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    padding: var(--space-1) var(--space-2);
    font-size: var(--font-size-sm);
    color: var(--color-text);
    cursor: pointer;
    max-width: 140px;
  }

  .tc-tool-chip:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }

  .tc-tool-chip:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Tool Sets */
  .tc-set {
    margin-bottom: var(--space-2);
  }

  .tc-set-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .tc-set-toggle {
    background: none;
    border: none;
    color: var(--color-text);
    font-size: var(--font-size-sm);
    font-weight: 600;
    cursor: pointer;
    padding: var(--space-1) 0;
    flex: 1;
    text-align: left;
  }

  .tc-set-count {
    color: var(--color-text-muted);
    font-weight: 400;
  }

  .tc-icon-btn {
    background: none;
    border: none;
    color: var(--color-text-muted);
    cursor: pointer;
    font-size: var(--font-size-xs);
    padding: var(--space-1);
    line-height: 1;
  }

  .tc-icon-btn:hover {
    color: var(--color-danger);
  }

  .tc-tool-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .tc-tool-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding-left: var(--space-3);
  }

  .tc-tool-btn {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    background: none;
    border: none;
    color: var(--color-text);
    font-size: var(--font-size-sm);
    cursor: pointer;
    padding: var(--space-1) 0;
    flex: 1;
    text-align: left;
    overflow: hidden;
  }

  .tc-tool-btn:hover:not(:disabled) {
    color: var(--color-primary);
  }

  .tc-tool-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .tc-glyph {
    flex-shrink: 0;
  }

  .tc-tool-name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* New set + import */
  .tc-new-set {
    display: flex;
    gap: var(--space-1);
    margin-top: var(--space-2);
  }

  .tc-text-input {
    flex: 1;
    background: var(--color-bg-input, var(--color-bg-active));
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-2);
    width: 100%;
    box-sizing: border-box;
  }

  .tc-import-btn {
    margin-top: var(--space-2);
    width: 100%;
  }

  /* Save-as-tool */
  .tc-save-section {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .tc-save-row {
    display: flex;
    gap: var(--space-1);
  }

  .tc-select {
    flex: 1;
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    padding: var(--space-1);
  }
</style>
