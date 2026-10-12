<script lang="ts">
  /**
   * Count commands for the selected count items (docs/count-polylength.md): Label, Colour,
   * Resume, Split, Split all, Merge. Shown in the Properties panel when the selection
   * contains MeasurementCount items. All edits are ONE undo step (`MarkupStore.applyBatch`).
   */
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import type { UserRef } from "$lib/ipc";
  import {
    countGroups, groupKey, resumeRun, splitSelected, splitAll, mergeSelected, setLabelColour,
  } from "$lib/count-groups";

  const { store, identity }: { store: MarkupStore; identity: UserRef | null } = $props();

  const items = $derived(store.selectedMarkups.filter((m) => m.markup_type === "MeasurementCount"));
  const groups = $derived(countGroups(store.markups));
  const selectedGroups = $derived(
    [...new Set(items.map(groupKey))].map((k) => groups.find((g) => g.key === k)!).filter(Boolean),
  );
  const total = $derived(selectedGroups.reduce((n, g) => n + g.total, 0));
  const label = $derived(
    selectedGroups.length === 1 ? selectedGroups[0].label : selectedGroups.length > 1 ? "" : "",
  );
  const color = $derived(selectedGroups.length > 0 ? selectedGroups[0].color : "#e02424");

  let message = $state<string | null>(null);

  const now = () => new Date().toISOString();
  const mint = () => crypto.randomUUID();

  function run(
    result: ReturnType<typeof splitSelected>,
    done: string,
  ) {
    if (!result.ok) {
      message = result.error;
      return;
    }
    store.applyBatch(result.pairs);
    message = done;
  }

  function onLabel(e: Event) {
    if (!identity) return;
    const v = (e.target as HTMLInputElement).value.trim();
    store.applyBatch(setLabelColour(store.markups, items.map((m) => m.id), { label: v }, identity, now()));
    if (store.countRun && selectedGroups.some((g) => g.key === store.countRun!.groupId)) {
      store.countRun = { ...store.countRun, subject: v === "" ? null : v };
    }
    message = null;
  }

  function onColour(e: Event) {
    if (!identity) return;
    const c = (e.target as HTMLInputElement).value;
    store.applyBatch(setLabelColour(store.markups, items.map((m) => m.id), { color: c }, identity, now()));
    if (store.countRun && selectedGroups.some((g) => g.key === store.countRun!.groupId)) {
      store.countRun = { ...store.countRun, color: c };
    }
    message = null;
  }

  function onResume() {
    const r = resumeRun(store.markups, items[0]?.id ?? "", identity ?? undefined);
    if ("error" in r) {
      message = r.error;
      return;
    }
    store.applyBatch(r.pairs);
    const g = groups.find((x) => x.key === r.run.groupId);
    if (g?.setId) store.setActiveCountSet(g.setId);
    store.countRun = r.run;
    store.activeTool = "MeasurementCount";
    message = "Resumed: click to add to this count.";
  }

  const ids = () => items.map((m) => m.id);
  const onSplit = () => identity && run(splitSelected(store.markups, ids(), identity, now(), mint), "Split into a new count.");
  const onSplitAll = () => identity && run(splitAll(store.markups, ids(), identity, now(), mint), "Split into single counts.");
  const onMerge = () => identity && run(mergeSelected(store.markups, ids(), identity, now()), "Merged into one count.");
</script>

{#if items.length > 0}
  <section class="prop-group" data-testid="count-controls">
    <h3 class="prop-group-title">Count</h3>
    <p class="count-total" data-testid="count-total">
      {selectedGroups.length === 1 ? "Count" : `${selectedGroups.length} counts`}: <strong>{total}</strong>
      ({items.length} selected)
    </p>
    <div class="row">
      <label for="count-label">Label</label>
      <input
        id="count-label"
        type="text"
        value={label}
        placeholder={selectedGroups.length > 1 ? "Mixed" : "Count"}
        maxlength="200"
        onchange={onLabel}
        disabled={!identity}
      />
    </div>
    <div class="row">
      <label for="count-colour">Colour</label>
      <input id="count-colour" type="color" value={color} onchange={onColour} disabled={!identity} />
    </div>
    <div class="buttons">
      <button type="button" data-action="resume" onclick={onResume} disabled={!identity || selectedGroups.length !== 1}
        title="Keep adding items to this count">Resume</button>
      <button type="button" data-action="split" onclick={onSplit} disabled={!identity}
        title="Selected items become a new count">Split</button>
      <button type="button" data-action="split-all" onclick={onSplitAll} disabled={!identity}
        title="Each selected item becomes its own count">Split all</button>
      <button type="button" data-action="merge" onclick={onMerge} disabled={!identity || selectedGroups.length < 2}
        title="Combine the selected counts (same type and page)">Merge</button>
    </div>
    {#if message}<p class="message" role="status">{message}</p>{/if}
  </section>
{/if}

<style>
  .prop-group { display: flex; flex-direction: column; gap: var(--space-1); padding: var(--space-2) 0; border-bottom: 1px solid var(--color-border); }
  .prop-group-title { font-size: var(--font-size-sm); font-weight: 600; color: var(--color-text-muted); margin: 0 0 var(--space-1) 0; text-transform: uppercase; letter-spacing: 0.05em; }
  .count-total { margin: 0; font-size: var(--font-size-sm); color: var(--color-text); }
  .row { display: flex; align-items: center; gap: var(--space-2); }
  .row label { flex: 0 0 4.5rem; font-size: var(--font-size-sm); color: var(--color-text-secondary); }
  .row input[type="text"] { flex: 1; min-width: 0; font-size: var(--font-size-sm); }
  .buttons { display: flex; flex-wrap: wrap; gap: var(--space-1); }
  .buttons button { font-size: var(--font-size-sm); padding: var(--space-1) var(--space-2); border: 1px solid var(--color-border); border-radius: var(--radius-sm); background: var(--color-bg-panel-alt); color: var(--color-text); cursor: pointer; }
  .buttons button:disabled { opacity: 0.5; cursor: default; }
  .message { margin: 0; font-size: var(--font-size-xs); color: var(--color-text-muted); }
</style>
