<script lang="ts">
  /**
   * CountSetPicker — create and select Count "sets" / categories (spec §7).
   *
   * Each set has its own colour + symbol; the active set drives new count markers (placed
   * with the Count tool) so distinct item types are counted and tallied separately. Sets are
   * document-scoped (held in MarkupStore.countSets) and embedded on each count annotation so
   * they round-trip through the PDF — see markup-store.svelte.ts / Rust CountSet.
   *
   * Discoverable near the measurement UI (rendered at the top of the Takeoff panel).
   */
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import { COUNT_SYMBOLS, type CountSymbol } from "$lib/ipc";
  import { countSymbolRender, COUNT_MARKER_RADIUS } from "$lib/markup-render";

  const { store }: { store: MarkupStore } = $props();

  let newName = $state("");
  let newSymbol = $state<CountSymbol>("Square");
  let newColor = $state("#1d70b8");

  // A 16px symbol swatch centred at (8,8). Reuses the live render geometry.
  const SWATCH = 16;
  function swatch(symbol: CountSymbol) {
    return countSymbolRender(symbol, SWATCH / 2, SWATCH / 2, COUNT_MARKER_RADIUS);
  }

  function addSet() {
    const name = newName.trim() || `Count ${store.countSets.length + 1}`;
    store.addCountSet(name, newSymbol, newColor);
    newName = "";
  }
</script>

<div class="count-sets" aria-label="Count sets">
  <div class="cs-header">Count sets</div>

  <ul class="cs-list" role="listbox" aria-label="Active count set">
    {#each store.countSets as set (set.id)}
      {@const r = swatch(set.symbol)}
      <li>
        <button
          class="cs-chip"
          class:active={store.activeCountSetId === set.id}
          role="option"
          aria-selected={store.activeCountSetId === set.id}
          title={`Use "${set.name}" for new counts`}
          onclick={() => store.setActiveCountSet(set.id)}
        >
          <svg width={SWATCH} height={SWATCH} viewBox={`0 0 ${SWATCH} ${SWATCH}`} aria-hidden="true">
            {#if r.shape === "circle"}
              <circle cx={r.cx} cy={r.cy} r={r.r} fill={set.color} />
            {:else if r.shape === "polygon"}
              <polygon points={r.points} fill={set.color} stroke-linejoin="round" />
            {:else if r.shape === "cross"}
              {#each r.lines as ln, i (i)}
                <line x1={ln.x1} y1={ln.y1} x2={ln.x2} y2={ln.y2}
                  stroke={set.color} stroke-width="2" stroke-linecap="round" />
              {/each}
            {/if}
          </svg>
          <span class="cs-name">{set.name}</span>
        </button>
      </li>
    {/each}
  </ul>

  <form class="cs-create" onsubmit={(e) => { e.preventDefault(); addSet(); }}>
    <input
      class="cs-input"
      type="text"
      placeholder="New set name"
      aria-label="New count set name"
      bind:value={newName}
    />
    <select class="cs-select" aria-label="Symbol" bind:value={newSymbol}>
      {#each COUNT_SYMBOLS as sym (sym)}
        <option value={sym}>{sym}</option>
      {/each}
    </select>
    <input
      class="cs-color"
      type="color"
      aria-label="Colour"
      bind:value={newColor}
    />
    <button class="cs-add" type="submit">Add set</button>
  </form>
</div>

<style>
  .count-sets {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-toolbar);
    flex-shrink: 0;
  }

  .cs-header {
    font-size: var(--font-size-sm);
    font-weight: 600;
    color: var(--color-text-muted);
  }

  .cs-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }

  .cs-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg-active);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: var(--font-size-sm);
  }

  .cs-chip:hover {
    background: var(--color-bg-hover);
  }

  .cs-chip.active {
    border-color: var(--color-primary);
    box-shadow: 0 0 0 1px var(--color-primary) inset;
  }

  .cs-chip svg {
    display: block;
  }

  .cs-name {
    line-height: 1;
  }

  .cs-create {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    flex-wrap: wrap;
  }

  .cs-input {
    flex: 1 1 8rem;
    min-width: 6rem;
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
  }

  .cs-select {
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
  }

  .cs-color {
    width: var(--space-8);
    height: var(--space-8);
    padding: 0;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg);
    cursor: pointer;
  }

  .cs-add {
    padding: var(--space-1) var(--space-3);
    background: var(--color-bg-active);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: var(--font-size-sm);
  }

  .cs-add:hover {
    background: var(--color-bg-hover);
  }
</style>
