<script lang="ts">
  /**
   * Legend commands (docs/legends.md): create a legend from the selected mark-ups, add the
   * selection to an existing legend, and edit a selected legend (title, scope, columns, rows).
   * Every edit is ONE undo step through the store; the table itself is computed, never stored.
   */
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import type { Markup, UserRef } from "$lib/ipc";
  import { getPageSize } from "$lib/ipc";
  import { layerStoreFor } from "$lib/layers-store.svelte";
  import {
    LEGEND_COLUMNS, addMarkupsToLegend, computeRows, formatPageSpec, legendFromSelection, parsePageSpec,
    patchLegend, patchRow, removeRow, sanitizeLegend, type LegendColumn, type LegendRow,
  } from "$lib/legend";

  const { store, identity, currentPage = 0 }: {
    store: MarkupStore;
    identity: UserRef | null;
    /** Page a new legend is placed on when the selection does not decide it. */
    currentPage?: number;
  } = $props();

  const now = () => new Date().toISOString();
  const selected = $derived(store.selectedMarkups);
  const legends = $derived(store.markups.filter((m) => m.markup_type === "Legend" && !!m.legend));
  const legend = $derived(selected.length === 1 && selected[0].markup_type === "Legend" && selected[0].legend ? selected[0] : null);
  const sources = $derived(selected.filter((m) => m.markup_type !== "Legend"));

  let message = $state<string | null>(null);
  let addTarget = $state("");
  let pageText = $state("");
  let pageError = $state(false);

  const def = $derived(legend ? sanitizeLegend(legend.legend) : null);

  // The layers a legend can be limited to (only when the document has layers).
  const docLayers = $derived(layerStoreFor(store.docId).model?.layers ?? []);
  function onLayerFilter(e: Event) {
    const v = (e.target as HTMLSelectElement).value;
    patch({ layer: v === "" ? null : Number(v) });
  }

  // Rows for the editor: every entry (hidden ones included, so they can be shown again), then
  // the automatic count-set rows. An entry that splits into several rows (one per unit) is one
  // editor row whose quantity lists each part.
  type EditorRow = { row: LegendRow; quantity: string };
  const editorRows = $derived.by<EditorRow[]>(() => {
    if (!legend || !def) return [];
    const rows = computeRows(
      legend.page,
      { ...def, hide_empty: false, entries: def.entries.map((e) => ({ ...e, hidden: false })) },
      store.markups,
    );
    const out: EditorRow[] = [];
    for (const row of rows) {
      const part = `${row.quantity_text} ${row.unit}`.trim();
      const prev = out[out.length - 1];
      if (row.entry !== null && prev && prev.row.entry === row.entry) prev.quantity += ` + ${part}`;
      else out.push({ row, quantity: part });
    }
    return out;
  });

  $effect(() => {
    if (def && typeof def.scope === "object") pageText = formatPageSpec(def.scope);
  });

  function apply(before: Markup, after: Markup) {
    if (after === before) return;
    store.applyBatch([{ before, after }]);
  }

  /** Top-left of a new legend: the page's top-left corner, with a margin. */
  function place(m: Markup) {
    store.create(m);
    store.selectedIds = new Set([m.id]);
    store.activeTool = "select";
  }

  async function createFromSelection() {
    if (!identity || sources.length === 0) return;
    const page = sources[0].page;
    let size = { width_pts: 612, height_pts: 792 };
    try {
      size = await getPageSize(store.docId, page);
    } catch {
      /* a Letter-sized fallback keeps the command usable; the user can move the legend */
    }
    const m = legendFromSelection(sources, {
      page,
      topLeft: { x: 36, y: size.height_pts - 36 },
      identity,
      now: now(),
      id: crypto.randomUUID(),
    });
    if (!m) {
      message = "Select at least one mark-up to list.";
      return;
    }
    place(m);
    message = null;
  }

  function addToLegend() {
    if (!identity) return;
    const target = legends.find((l) => l.id === addTarget) ?? legends[0];
    if (!target) return;
    const after = addMarkupsToLegend(target, sources, identity, now());
    if (after === target) {
      message = "Already listed in that legend.";
      return;
    }
    apply(target, after);
    message = null;
  }

  function patch(p: Partial<NonNullable<typeof def>>) {
    if (!legend || !identity) return;
    apply(legend, patchLegend(legend, p, identity, now()));
  }

  function toggleColumn(c: LegendColumn, on: boolean) {
    if (!def) return;
    const cols = LEGEND_COLUMNS.filter((x) => (x === c ? on : def.columns.includes(x)));
    if (cols.length === 0) return; // a legend keeps at least one column
    patch({ columns: cols });
  }

  function onScope(e: Event) {
    const v = (e.target as HTMLSelectElement).value;
    if (v === "CurrentPage" || v === "AllPages") patch({ scope: v });
    else patch({ scope: { Pages: [[currentPage + 1, currentPage + 1]] } });
    pageError = false;
  }

  function onPages(e: Event) {
    const ranges = parsePageSpec((e.target as HTMLInputElement).value);
    pageError = ranges === null;
    if (ranges) patch({ scope: { Pages: ranges } });
  }

  function editRow(row: LegendRow, field: "label" | "description" | "hidden", value: string | boolean) {
    if (!legend || !identity || !def) return;
    const cur = row.entry === null ? null : def.entries[row.entry];
    const next = {
      label: cur?.label ?? null,
      description: cur?.description ?? null,
      hidden: cur?.hidden ?? false,
    };
    if (field === "hidden") next.hidden = value as boolean;
    else next[field] = (value as string) === "" ? null : (value as string);
    apply(legend, patchRow(legend, row, next, identity, now()));
  }

  function dropRow(row: LegendRow) {
    if (!legend || !identity) return;
    apply(legend, removeRow(legend, row, identity, now()));
  }
</script>

{#if sources.length > 0 && !legend}
  <section class="prop-group" data-testid="legend-create">
    <h3 class="prop-group-title">Legend</h3>
    <button type="button" data-action="legend-from-selection" onclick={createFromSelection} disabled={!identity}>
      New legend from selection
    </button>
    {#if legends.length > 0}
      <div class="row">
        <select aria-label="Add selection to legend" bind:value={addTarget}>
          {#each legends as l (l.id)}<option value={l.id}>{l.legend?.title || "Legend"}</option>{/each}
        </select>
        <button type="button" data-action="legend-add" onclick={addToLegend} disabled={!identity}>Add</button>
      </div>
    {/if}
    {#if message}<p class="muted" role="status">{message}</p>{/if}
  </section>
{/if}

{#if legend && def}
  <section class="prop-group" data-testid="legend-controls">
    <h3 class="prop-group-title">Legend</h3>
    <label class="field">Title
      <input type="text" aria-label="Legend title" maxlength="200" value={def.title}
        onchange={(e) => patch({ title: (e.target as HTMLInputElement).value })} disabled={!identity} />
    </label>
    <label class="check"><input type="checkbox" checked={def.show_title}
      onchange={(e) => patch({ show_title: (e.target as HTMLInputElement).checked })} disabled={!identity} /> Show title</label>
    <label class="check"><input type="checkbox" checked={def.show_headers}
      onchange={(e) => patch({ show_headers: (e.target as HTMLInputElement).checked })} disabled={!identity} /> Show column headers</label>
    <label class="field">Source pages
      <select aria-label="Source pages" value={typeof def.scope === "string" ? def.scope : "Custom"}
        onchange={onScope} disabled={!identity}>
        <option value="CurrentPage">This page</option>
        <option value="AllPages">All pages</option>
        <option value="Custom">Custom range</option>
      </select>
    </label>
    {#if docLayers.length > 0 || def.layer != null}
      <label class="field">Layer
        <select aria-label="Layer" data-testid="legend-layer" value={def.layer == null ? "" : String(def.layer)}
          onchange={onLayerFilter} disabled={!identity}>
          <option value="">All layers</option>
          {#each docLayers as l (l.id)}
            <option value={String(l.id)}>{l.name}</option>
          {/each}
          {#if def.layer != null && !docLayers.some((l) => l.id === def.layer)}
            <option value={String(def.layer)}>(missing layer)</option>
          {/if}
        </select>
      </label>
    {/if}
    {#if typeof def.scope === "object"}
      <input type="text" aria-label="Page range" placeholder="1-3, 5, 9" value={pageText}
        class:invalid={pageError} onchange={onPages} disabled={!identity} />
      {#if pageError}<p class="warn" role="alert">Enter pages like 1-3, 5, 9.</p>{/if}
    {/if}
    <fieldset class="cols">
      <legend>Columns</legend>
      {#each LEGEND_COLUMNS as c (c)}
        <label class="check"><input type="checkbox" checked={def.columns.includes(c)}
          onchange={(e) => toggleColumn(c, (e.target as HTMLInputElement).checked)} disabled={!identity} /> {c}</label>
      {/each}
    </fieldset>
    <label class="field">Font size (pt)
      <input type="number" min="4" max="72" step="0.5" aria-label="Legend font size" value={def.font_size_pt}
        onchange={(e) => patch({ font_size_pt: Number((e.target as HTMLInputElement).value) })} disabled={!identity} />
    </label>
    <label class="check"><input type="checkbox" checked={def.hide_empty}
      onchange={(e) => patch({ hide_empty: (e.target as HTMLInputElement).checked })} disabled={!identity} /> Hide rows with nothing counted</label>
    <label class="check"><input type="checkbox" checked={def.auto_count_sets}
      onchange={(e) => patch({ auto_count_sets: (e.target as HTMLInputElement).checked })} disabled={!identity} /> Add new count sets automatically</label>

    <table>
      <thead><tr><th>Subject</th><th>Description</th><th>Qty</th><th>Hide</th><th></th></tr></thead>
      <tbody>
        {#each editorRows as er, i (i)}
          {@const row = er.row}
          {@const entry = row.entry === null ? null : def.entries[row.entry]}
          <tr data-testid="legend-row">
            <td>
              <input type="text" aria-label={`Subject for row ${i + 1}`} placeholder={row.subject}
                value={entry?.label ?? ""} maxlength="500"
                onchange={(e) => editRow(row, "label", (e.target as HTMLInputElement).value)} disabled={!identity} />
            </td>
            <td>
              <input type="text" aria-label={`Description for row ${i + 1}`} placeholder={row.description}
                value={entry?.description ?? ""} maxlength="500"
                onchange={(e) => editRow(row, "description", (e.target as HTMLInputElement).value)} disabled={!identity} />
            </td>
            <td class="num">{er.quantity}</td>
            <td><input type="checkbox" aria-label={`Hide row ${i + 1}`} checked={entry?.hidden ?? false}
              onchange={(e) => editRow(row, "hidden", (e.target as HTMLInputElement).checked)} disabled={!identity} /></td>
            <td>
              {#if row.entry !== null}
                <button type="button" aria-label={`Remove row ${i + 1}`} onclick={() => dropRow(row)} disabled={!identity}>x</button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if editorRows.length === 0}<p class="muted">No rows yet. Select mark-ups and use "Add" to list them.</p>{/if}
  </section>
{/if}

<style>
  .prop-group { display: flex; flex-direction: column; gap: var(--space-1); padding: var(--space-2) 0; border-bottom: 1px solid var(--color-border); }
  .prop-group-title { font-size: var(--font-size-sm); font-weight: 600; color: var(--color-text-muted); margin: 0 0 var(--space-1) 0; text-transform: uppercase; letter-spacing: 0.05em; }
  .muted { color: var(--color-text-muted); font-size: var(--font-size-xs); margin: 0; }
  .warn { color: var(--color-warning); font-size: var(--font-size-xs); margin: 0; }
  .row { display: flex; gap: var(--space-2); align-items: center; }
  .field { display: flex; flex-direction: column; gap: var(--space-1); font-size: var(--font-size-sm); }
  .check { display: flex; align-items: center; gap: var(--space-1); font-size: var(--font-size-sm); }
  .cols { border: 1px solid var(--color-border); border-radius: var(--radius-sm); padding: var(--space-1) var(--space-2); display: flex; flex-wrap: wrap; gap: var(--space-1) var(--space-3); }
  .cols legend { font-size: var(--font-size-xs); color: var(--color-text-muted); }
  .invalid { border-color: var(--color-warning); }
  table { width: 100%; border-collapse: collapse; font-size: var(--font-size-sm); }
  th { text-align: left; color: var(--color-text-muted); font-weight: 500; }
  td input[type="text"] { width: 100%; box-sizing: border-box; font-size: var(--font-size-sm); }
  td.num { white-space: nowrap; }
  button { font-size: var(--font-size-sm); padding: var(--space-1) var(--space-2); border: 1px solid var(--color-border); border-radius: var(--radius-sm); background: var(--color-bg-panel-alt); color: var(--color-text); cursor: pointer; }
</style>
