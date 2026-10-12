<script lang="ts">
  /**
   * LayersPanel - the PDF's layers (optional content groups): show/hide, "show only",
   * new layers, the layer new mark-ups go on. Behaviour spec: docs/layers.md.
   *
   * Show/hide is a VIEW setting: it changes what is drawn here and is not written to the file
   * unless the user chooses "Save as default". "New layer" and "Save as default" do write the
   * file (like a page operation).
   */
  import { flattenRows, hasChildren } from "$lib/layers";
  import { layerStoreFor, type MarkupSync } from "$lib/layers-store.svelte";

  interface Props {
    docId: string;
    /** Hooks into the mark-up store for the actions that write the file. */
    sync: MarkupSync;
  }

  const { docId, sync }: Props = $props();

  const layers = $derived(layerStoreFor(docId));
  const rows = $derived(layers.model ? flattenRows(layers.model, layers.collapsed) : []);
  const selected = $derived(
    layers.selectedId === null ? null : (layers.model?.layers.find((l) => l.id === layers.selectedId) ?? null),
  );

  let newName = $state("");
  let adding = $state(false);

  // Read the layers when this panel's document changes.
  $effect(() => {
    const s = layers;
    if (s.status === "idle") void s.load(false);
  });

  async function submitNew(asChild: boolean) {
    const name = newName.trim();
    if (!name || layers.busy) return;
    const id = await layers.addLayer(name, asChild ? layers.selectedId : null, sync);
    if (id !== null) {
      newName = "";
      adding = false;
      layers.selectedId = id;
    }
  }

  function onNewKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      void submitNew(false);
    } else if (e.key === "Escape") {
      adding = false;
      newName = "";
    }
  }

  const viewHint = $derived(
    layers.model && !layers.model.view_supported
      ? `Showing and hiding layers is not available for this file (${layers.model.view_unsupported_reason ?? "unsupported"}).`
      : null,
  );
</script>

<div class="layers" data-testid="layers-panel">
  {#if layers.status === "loading" || layers.status === "idle"}
    <p class="layers__hint">Reading layers...</p>
  {:else if layers.status === "deferred"}
    <p class="layers__hint">
      This file is large ({Math.round(layers.deferredSize / (1024 * 1024))} MB); reading its layers can take
      several seconds.
    </p>
    <button class="layers__btn" data-testid="layers-load" onclick={() => layers.load(true)}>Read layers</button>
  {:else if layers.status === "error"}
    <p class="layers__error" role="alert">{layers.error}</p>
    <button class="layers__btn" data-testid="layers-retry" onclick={() => layers.load(true)}>Try again</button>
  {:else if layers.model}
    {#if !layers.hasLayers}
      <p class="layers__hint">This file has no layers.</p>
    {/if}

    <div class="layers__toolbar" role="toolbar" aria-label="Layer actions">
      {#if layers.hasLayers}
        <button
          class="layers__btn"
          data-testid="layers-show-all"
          onclick={() => layers.showAllLayers()}
          disabled={!layers.canChangeView}
          title="Show every layer"
        >Show all</button>
        <button
          class="layers__btn"
          data-testid="layers-isolate"
          onclick={() => selected && layers.isolateLayer(selected.id)}
          disabled={!layers.canChangeView || !selected}
          title="Hide every layer except the selected one and its sub-layers"
        >Show only</button>
        <button
          class="layers__btn"
          data-testid="layers-reset"
          onclick={() => layers.resetToFile()}
          disabled={!layers.canChangeView || layers.isFileView}
          title="Back to the layers the file opens with"
        >Reset</button>
        <button
          class="layers__btn"
          data-testid="layers-save-default"
          onclick={() => layers.saveAsDefault(sync)}
          disabled={layers.busy || layers.isFileView}
          title="Write the current show/hide state into the file as its default"
        >Save as default</button>
      {/if}
      <button
        class="layers__btn"
        data-testid="layers-add"
        onclick={() => (adding = !adding)}
        disabled={layers.busy}
        aria-expanded={adding}
        title="Add a blank layer (saves the file)"
      >New layer</button>
    </div>

    {#if adding}
      <div class="layers__new">
        <input
          class="layers__input"
          data-testid="layers-new-name"
          type="text"
          maxlength="200"
          placeholder="Layer name"
          aria-label="New layer name"
          bind:value={newName}
          onkeydown={onNewKey}
        />
        <button class="layers__btn" data-testid="layers-new-ok" onclick={() => submitNew(false)} disabled={!newName.trim() || layers.busy}>Add</button>
        <button
          class="layers__btn"
          data-testid="layers-new-child"
          onclick={() => submitNew(true)}
          disabled={!newName.trim() || layers.busy || selected === null}
          title="Add under the selected layer"
        >Add child</button>
      </div>
      <p class="layers__note">Adding a layer saves the file, including your mark-ups.</p>
    {/if}

    {#if viewHint}
      <p class="layers__note" data-testid="layers-unsupported">{viewHint}</p>
    {/if}
    {#if layers.viewError}
      <p class="layers__error" role="alert" data-testid="layers-error">{layers.viewError}</p>
    {/if}
    {#if layers.model.truncated}
      <p class="layers__note">This file has more layer structure than redline reads; some is not shown.</p>
    {/if}
    {#if layers.model.unlisted > 0}
      <p class="layers__note">{layers.model.unlisted} layer(s) the file does not list in its layer order are shown at the end.</p>
    {/if}

    {#if layers.hasLayers}
      <ul class="layers__list" role="tree" aria-label="Layers">
        {#each rows as row, i (row.kind + ":" + (row.id ?? i) + ":" + row.label)}
          {#if row.kind === "group"}
            <li class="layers__group" role="treeitem" aria-selected="false" style:padding-left="calc({row.depth} * 14px + 6px)">
              {row.label}
            </li>
          {:else if row.layer}
            {@const l = row.layer}
            {@const shown = !layers.off.has(l.id) || !l.view_intent}
            <li
              class="layers__row"
              class:layers__row--selected={layers.selectedId === l.id}
              class:layers__row--off={!shown}
              role="treeitem"
              aria-selected={layers.selectedId === l.id}
              aria-level={row.depth + 1}
              data-testid="layer-row"
              data-layer-id={l.id}
              style:padding-left="calc({row.depth} * 14px + 4px)"
            >
              {#if hasChildren(layers.model, l.id)}
                <button
                  class="layers__twisty"
                  aria-label={layers.collapsed.has(l.id) ? "Expand" : "Collapse"}
                  aria-expanded={!layers.collapsed.has(l.id)}
                  onclick={() => layers.toggleCollapsed(l.id)}
                >{layers.collapsed.has(l.id) ? "▸" : "▾"}</button>
              {:else}
                <span class="layers__twisty layers__twisty--none"></span>
              {/if}
              <input
                type="checkbox"
                class="layers__check"
                data-testid="layer-toggle"
                aria-label={`Show layer ${l.name}`}
                checked={shown}
                disabled={!layers.canChangeView || l.locked || !l.view_intent}
                onclick={(e) => {
                  // The state decides the checkmark, not the click (it reverts if rendering fails).
                  e.preventDefault();
                  void layers.toggle(l.id, e.shiftKey);
                }}
              />
              <button
                class="layers__name"
                data-testid="layer-name"
                onclick={() => (layers.selectedId = l.id)}
                ondblclick={() => layers.isolateLayer(l.id)}
                title={l.name}
              >{l.name}</button>
              {#if l.locked}
                <span class="layers__badge" title="Locked by the file: its state cannot be changed here" aria-label="locked">locked</span>
              {/if}
              {#if !l.view_intent}
                <span class="layers__badge" title="A design-intent layer: it does not affect what is shown">design</span>
              {/if}
              {#if layers.markupLayerId === l.id}
                <span class="layers__badge layers__badge--markup" title="New mark-ups are placed on this layer" data-testid="markup-layer-badge">mark-ups</span>
              {/if}
            </li>
          {/if}
        {/each}
      </ul>

      <div class="layers__footer">
        <button
          class="layers__btn"
          data-testid="layers-markup-layer"
          onclick={() => selected && layers.setMarkupLayer(layers.markupLayerId === selected.id ? null : selected.id)}
          disabled={!selected}
          title="Place new mark-ups on the selected layer"
        >{selected && layers.markupLayerId === selected.id ? "Stop using for mark-ups" : "Use for new mark-ups"}</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .layers {
    display: flex;
    flex-direction: column;
    gap: var(--space-2, 8px);
    padding: var(--space-2, 8px);
    font-size: var(--text-sm, 13px);
    color: var(--color-text-primary, #1a1a1a);
  }

  .layers__toolbar,
  .layers__new,
  .layers__footer {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1, 4px);
  }

  .layers__input {
    flex: 1;
    min-width: 0;
    padding: var(--space-1, 4px) var(--space-2, 8px);
    border: 1px solid var(--color-border, #d1d5db);
    border-radius: var(--radius-sm, 4px);
    font-size: var(--text-sm, 13px);
    background: var(--color-surface, #fff);
    color: var(--color-text-primary, #1a1a1a);
  }

  .layers__btn {
    padding: var(--space-1, 4px) var(--space-2, 8px);
    background: none;
    border: 1px solid var(--color-border, #d1d5db);
    border-radius: var(--radius-sm, 4px);
    cursor: pointer;
    font-size: var(--text-xs, 11px);
    color: var(--color-text-primary, #1a1a1a);
    white-space: nowrap;
  }

  .layers__btn:hover:not(:disabled) {
    background: var(--color-surface-hover, #f3f4f6);
  }

  .layers__btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .layers__hint,
  .layers__note {
    margin: 0;
    color: var(--color-text-secondary, #6b7280);
    font-size: var(--text-xs, 11px);
  }

  .layers__error {
    margin: 0;
    color: var(--color-error, #dc2626);
    font-size: var(--text-xs, 11px);
    padding: var(--space-1, 4px);
    background: var(--color-error-surface, #fef2f2);
    border-radius: var(--radius-sm, 4px);
  }

  .layers__list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 320px;
    overflow-y: auto;
    border: 1px solid var(--color-border, #e5e7eb);
    border-radius: var(--radius-sm, 4px);
  }

  .layers__group {
    padding: 3px 6px;
    font-size: var(--text-xs, 11px);
    font-weight: 600;
    color: var(--color-text-secondary, #6b7280);
    text-transform: none;
  }

  .layers__row {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px;
    min-height: 26px;
  }

  .layers__row--selected {
    background: var(--color-surface-selected, #e0e7ff);
  }

  .layers__row--off .layers__name {
    color: var(--color-text-tertiary, #9ca3af);
  }

  .layers__twisty {
    width: 16px;
    flex: none;
    border: none;
    background: none;
    cursor: pointer;
    padding: 0;
    color: var(--color-text-secondary, #6b7280);
  }

  .layers__twisty--none {
    cursor: default;
  }

  .layers__check {
    flex: none;
    margin: 0;
  }

  .layers__name {
    flex: 1;
    min-width: 0;
    text-align: left;
    border: none;
    background: none;
    cursor: pointer;
    padding: 2px 4px;
    font-size: var(--text-sm, 13px);
    color: var(--color-text-primary, #1a1a1a);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .layers__badge {
    flex: none;
    font-size: 10px;
    padding: 0 4px;
    border-radius: var(--radius-sm, 4px);
    border: 1px solid var(--color-border, #d1d5db);
    color: var(--color-text-secondary, #6b7280);
  }

  .layers__badge--markup {
    color: var(--color-primary, #2563eb);
    border-color: var(--color-primary, #2563eb);
  }
</style>
