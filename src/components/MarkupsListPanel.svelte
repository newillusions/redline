<script lang="ts">
  /**
   * MarkupsListPanel - Bluebeam-Revu-style table of every mark-up in the document.
   * Behaviour spec: docs/markups-list.md (each row labelled verified / inferred / unknown).
   *
   * Rows are virtualised (fixed height, only the visible window is in the DOM) so 5,000
   * mark-ups stay responsive. All cell text comes from the PDF and is rendered as text
   * (`{...}` interpolation), never as HTML.
   */
  import { onMount } from "svelte";
  import { save as saveDialog } from "@tauri-apps/plugin-dialog";
  import type { Markup, MarkupStatus, UserRef } from "$lib/ipc";
  import { exportMarkupsView, getUserIdentity, isMarkupContentsLocked, isMarkupLocked } from "$lib/ipc";
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import { boundsOf } from "$lib/markup-select";
  import { expandCountSelection } from "$lib/count-groups";
  import { patchStatus } from "$lib/markup-properties";
  import { exportDoneMessage, exportFormatFromPath } from "$lib/doc-op-flow";
  import {
    STATUS_ORDER, applyRowClick, buildRows, cellText, columnLabel, columnKind, COLUMN_DEFS,
    customColumnNames, customKey, defaultColumnConfig, displayItems, emptyFilter, filterRows,
    isFilterActive, jumpTarget, moveColumn, nextSort, normalizeColumnConfig, sortRows,
    statusForShortcut, toggleColumn, visibleColumns, windowRange,
    type ColumnConfig, type ListFilter, type SortState,
  } from "$lib/markups-list";

  const {
    store,
    docId,
    onjump,
  }: {
    store: MarkupStore;
    docId: string;
    /** Navigate the viewport to a mark-up (zero-based page + PDF-space rect). */
    onjump?: (target: { page: number; rect: [number, number, number, number]; markupId: string }) => void;
  } = $props();

  const ROW_H = 24;
  const FALLBACK_VIEWPORT_H = 360;
  const STORAGE_KEY = "redline.markupsList.columns.v1";

  let identity = $state<UserRef | null>(null);
  onMount(() => {
    getUserIdentity().then((u) => { identity = u; }).catch(() => {});
  });

  function loadConfig(): unknown {
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      return raw ? JSON.parse(raw) : null;
    } catch {
      return null;
    }
  }
  function saveConfig(c: ColumnConfig) {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(c)); } catch { /* storage unavailable */ }
  }

  let config = $state<ColumnConfig>(normalizeColumnConfig(loadConfig()));
  let sort = $state<SortState | null>(null);
  let filter = $state<ListFilter>(emptyFilter());
  let expanded = $state<Set<string>>(new Set());
  let anchor = $state<string | null>(null);
  let scrollTop = $state(0);
  let viewportH = $state(0);
  let scroller = $state<HTMLDivElement | null>(null);
  let editingStatusId = $state<string | null>(null);
  let message = $state<string | null>(null);
  let error = $state<string | null>(null);
  let fromList = false;

  const allRows = $derived(buildRows(store.markups));
  const customNames = $derived(customColumnNames(allRows));
  // Custom columns appear in the picker once the document has them (hidden until ticked).
  const effective = $derived(normalizeColumnConfig(config, customNames));
  const cols = $derived(visibleColumns(effective));
  const authors = $derived([...new Set(allRows.map((r) => r.author))].sort());
  const types = $derived([...new Set(allRows.map((r) => r.typeLabel))].sort());
  /** One entry per layer identity; groups that share a name are told apart by their number. */
  const layerChoices = $derived.by(() => {
    const byKey = new Map<string, string>();
    for (const r of allRows) if (!byKey.has(r.layerKey)) byKey.set(r.layerKey, r.layer);
    const count = new Map<string, number>();
    for (const l of byKey.values()) count.set(l, (count.get(l) ?? 0) + 1);
    return [...byKey]
      .map(([key, name]) => ({
        key,
        label: name === "" ? "(none)" : (count.get(name) ?? 0) > 1 ? `${name} (#${key.slice(3)})` : name,
      }))
      .sort((a, b) => a.label.localeCompare(b.label));
  });
  const rows = $derived(sortRows(filterRows(allRows, filter), sort));
  const items = $derived(displayItems(rows, expanded));
  const win = $derived(windowRange(scrollTop, viewportH || FALLBACK_VIEWPORT_H, ROW_H, items.length));
  const slice = $derived(items.slice(win.start, win.end));
  const viewIds = $derived(rows.map((r) => r.id));
  const gridCols = $derived(
    "26px " + cols.map((k) => `${COLUMN_DEFS.find((c) => c.key === k)?.width ?? 110}px`).join(" "),
  );
  const totalW = $derived(
    26 + cols.reduce((n, k) => n + (COLUMN_DEFS.find((c) => c.key === k)?.width ?? 110), 0),
  );
  const selectedCount = $derived(store.selectedIds.size);

  function setConfig(c: ColumnConfig) {
    config = c;
    saveConfig(c);
  }

  // ---- selection + jump -------------------------------------------------------------

  function select(ids: Set<string>) {
    fromList = true;
    // The viewport clears selection whenever the active tool is not "select".
    store.activeTool = "select";
    store.selectedIds = expandCountSelection(store.markups, ids);
  }

  function jumpTo(m: Markup) {
    onjump?.({ ...jumpTarget(m, boundsOf(m)), markupId: m.id });
  }

  function onRowClick(e: MouseEvent, row: { id: string; markup: Markup }) {
    const res = applyRowClick(store.selectedIds, anchor, viewIds, row.id, {
      ctrl: e.ctrlKey || e.metaKey,
      shift: e.shiftKey,
    });
    anchor = res.anchor;
    select(res.ids);
    if (!e.ctrlKey && !e.metaKey && !e.shiftKey) jumpTo(row.markup);
    scroller?.focus();
  }

  function onRowDblClick(row: { id: string; markup: Markup }) {
    select(new Set([row.id]));
    jumpTo(row.markup);
  }

  // Scroll the list to a selection made on the canvas (not one the list itself made).
  $effect(() => {
    const ids = store.selectedIds;
    if (fromList) { fromList = false; return; }
    if (ids.size !== 1 || !scroller) return;
    const [id] = ids;
    const idx = items.findIndex((it) => it.kind === "markup" && it.row.id === id);
    if (idx < 0) return;
    const top = idx * ROW_H;
    const h = viewportH || FALLBACK_VIEWPORT_H;
    if (top < scroller.scrollTop || top + ROW_H > scroller.scrollTop + h - ROW_H) {
      scroller.scrollTop = Math.max(0, top - h / 2);
    }
  });

  function onKeyDown(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "SELECT" || t.tagName === "TEXTAREA")) return;
    const st = statusForShortcut(e);
    if (st) {
      e.preventDefault();
      setStatus(st);
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (rows.length === 0) return;
      const cur = anchor ? viewIds.indexOf(anchor) : -1;
      const next = Math.max(0, Math.min(rows.length - 1, cur + (e.key === "ArrowDown" ? 1 : -1)));
      const row = rows[next];
      anchor = row.id;
      select(new Set([row.id]));
      jumpTo(row.markup);
      if (scroller) {
        const top = items.findIndex((it) => it.kind === "markup" && it.row.id === row.id) * ROW_H;
        const h = viewportH || FALLBACK_VIEWPORT_H;
        if (top < scroller.scrollTop) scroller.scrollTop = top;
        else if (top + ROW_H > scroller.scrollTop + h - ROW_H) scroller.scrollTop = top + ROW_H - h + ROW_H;
      }
    }
  }

  // ---- status -----------------------------------------------------------------------

  /** Set `status` on the selection. Skips locked mark-ups (the store reports them). */
  function setStatus(status: MarkupStatus, ids?: Set<string>) {
    const targets = (ids ? store.markups.filter((m) => ids.has(m.id)) : store.selectedMarkups);
    if (targets.length === 0) return;
    const now = new Date().toISOString();
    error = null;
    // The store skips locked mark-ups; say so here (its own mirrorError is cleared on drain).
    const locked = targets.filter((m) => isMarkupLocked(m) || isMarkupContentsLocked(m)).length;
    if (locked > 0) error = `${locked} locked mark-up${locked === 1 ? "" : "s"} skipped.`;
    store.applyBatch(
      targets.map((m) => ({ before: m, after: patchStatus(m, status, identity ?? m.audit.modified_by, now) })),
    );
  }

  function onStatusPick(e: Event, id: string) {
    const v = (e.currentTarget as HTMLSelectElement).value as MarkupStatus;
    editingStatusId = null;
    setStatus(v, store.selectedIds.has(id) ? undefined : new Set([id]));
  }

  // ---- filter helpers ---------------------------------------------------------------

  function toggleIn<T>(set: ReadonlySet<T>, v: T): Set<T> {
    const n = new Set(set);
    if (n.has(v)) n.delete(v);
    else n.add(v);
    return n;
  }

  function toggleExpanded(id: string) {
    expanded = toggleIn(expanded, id);
  }

  // ---- export -----------------------------------------------------------------------

  async function doExport(ext: "csv" | "xlsx") {
    error = null;
    message = null;
    if (rows.length === 0) return;
    const dest = await saveDialog({
      title: "Export mark-ups list",
      defaultPath: `markups.${ext}`,
      filters: [{ name: ext === "xlsx" ? "Excel workbook" : "CSV", extensions: [ext] }],
    });
    if (!dest) return;
    try {
      await store.flush();
      const n = await exportMarkupsView(docId, dest, exportFormatFromPath(dest), viewIds, cols);
      message = exportDoneMessage(dest, n);
    } catch (e) {
      error = `Export failed: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  function onScroll(e: Event) {
    scrollTop = (e.currentTarget as HTMLElement).scrollTop;
  }

  $effect(() => {
    if (!scroller) return;
    viewportH = scroller.clientHeight;
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => { if (scroller) viewportH = scroller.clientHeight; });
    ro.observe(scroller);
    return () => ro.disconnect();
  });

  const allColumnKeys = $derived(effective.order);
</script>

<div class="markups-list" data-testid="markups-list">
  <div class="ml-toolbar">
    <input
      class="ml-search"
      type="search"
      placeholder="Search comments, subject, author, replies"
      aria-label="Search mark-ups"
      maxlength="200"
      bind:value={filter.text}
    />
    <details class="ml-menu">
      <summary>Status{filter.statuses.size ? ` (${filter.statuses.size})` : ""}</summary>
      <div class="ml-pop">
        {#each STATUS_ORDER as s (s)}
          <label><input type="checkbox" checked={filter.statuses.has(s)}
            onchange={() => (filter = { ...filter, statuses: toggleIn(filter.statuses, s) })} /> {s}</label>
        {/each}
      </div>
    </details>
    <details class="ml-menu">
      <summary>Author{filter.authors.size ? ` (${filter.authors.size})` : ""}</summary>
      <div class="ml-pop">
        {#each authors as a (a)}
          <label><input type="checkbox" checked={filter.authors.has(a)}
            onchange={() => (filter = { ...filter, authors: toggleIn(filter.authors, a) })} /> {a || "(none)"}</label>
        {/each}
      </div>
    </details>
    <details class="ml-menu">
      <summary>Type{filter.types.size ? ` (${filter.types.size})` : ""}</summary>
      <div class="ml-pop">
        {#each types as t (t)}
          <label><input type="checkbox" checked={filter.types.has(t)}
            onchange={() => (filter = { ...filter, types: toggleIn(filter.types, t) })} /> {t}</label>
        {/each}
      </div>
    </details>
    {#if layerChoices.length > 1 || (layerChoices.length === 1 && layerChoices[0].key !== "name:")}
      <details class="ml-menu" data-testid="ml-layer-filter">
        <summary>Layer{filter.layers.size ? ` (${filter.layers.size})` : ""}</summary>
        <div class="ml-pop">
          {#each layerChoices as lc (lc.key)}
            <label><input type="checkbox" checked={filter.layers.has(lc.key)}
              onchange={() => (filter = { ...filter, layers: toggleIn(filter.layers, lc.key) })} /> {lc.label}</label>
          {/each}
        </div>
      </details>
    {/if}
    <input
      class="ml-pages"
      type="text"
      placeholder="Pages 1-3, 7"
      aria-label="Filter by page"
      maxlength="100"
      bind:value={filter.pages}
    />
    {#if isFilterActive(filter)}
      <button class="ml-btn" onclick={() => (filter = emptyFilter())}>Clear</button>
    {/if}
    <details class="ml-menu">
      <summary>Columns</summary>
      <div class="ml-pop ml-cols">
        {#each allColumnKeys as k, i (k)}
          <div class="ml-col-row">
            <label><input type="checkbox" checked={effective.visible.includes(k)}
              onchange={() => setConfig(toggleColumn(effective, k))} /> {columnLabel(k)}</label>
            <button class="ml-mini" aria-label={`Move ${columnLabel(k)} left`} disabled={i === 0}
              onclick={() => setConfig(moveColumn(effective, k, -1))}>&uarr;</button>
            <button class="ml-mini" aria-label={`Move ${columnLabel(k)} right`} disabled={i === allColumnKeys.length - 1}
              onclick={() => setConfig(moveColumn(effective, k, 1))}>&darr;</button>
          </div>
        {/each}
        <button class="ml-btn" onclick={() => setConfig(defaultColumnConfig())}>Reset columns</button>
      </div>
    </details>
    <select
      class="ml-status-set"
      aria-label="Set status of selected mark-ups"
      disabled={selectedCount === 0}
      value=""
      onchange={(e) => { const v = e.currentTarget.value as MarkupStatus; e.currentTarget.value = ""; if (v) setStatus(v); }}
    >
      <option value="">Set status{selectedCount ? ` (${selectedCount})` : ""}</option>
      {#each STATUS_ORDER as s, i (s)}
        <option value={s}>{s} (Shift+{i + 1})</option>
      {/each}
    </select>
    <button class="ml-btn" disabled={rows.length === 0} onclick={() => doExport("csv")}>Export CSV</button>
    <button class="ml-btn" disabled={rows.length === 0} onclick={() => doExport("xlsx")}>Export XLSX</button>
    <span class="ml-count" aria-live="polite">{rows.length === allRows.length ? `${allRows.length} mark-ups` : `${rows.length} of ${allRows.length}`}</span>
  </div>

  {#if message}<div class="ml-ok" role="status">{message}</div>{/if}
  {#if error}<div class="ml-err" role="alert">{error}</div>{/if}

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="ml-scroller"
    role="grid"
    aria-label="Markups list"
    aria-rowcount={items.length + 1}
    aria-multiselectable="true"
    tabindex="0"
    bind:this={scroller}
    onscroll={onScroll}
    onkeydown={onKeyDown}
  >
    <div class="ml-inner" style:width="{totalW}px">
      <div class="ml-head" role="row" style:grid-template-columns={gridCols}>
        <span class="ml-cell" role="columnheader"></span>
        {#each cols as k (k)}
          <button
            class="ml-cell ml-hcell"
            role="columnheader"
            aria-sort={sort?.key === k ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}
            class:num={columnKind(k) === "number"}
            onclick={() => (sort = nextSort(sort, k))}
            title={k === "date" || k === "modified" || k === "status_at" ? "Sort (times are UTC)" : "Sort"}
          >{columnLabel(k)}{sort?.key === k ? (sort.dir === "asc" ? " ▲" : " ▼") : ""}</button>
        {/each}
      </div>
      <div class="ml-body" style:height="{items.length * ROW_H}px">
        {#if items.length === 0}
          <p class="ml-empty">{allRows.length === 0 ? "No mark-ups in this document." : "No mark-ups match the filter."}</p>
        {/if}
        {#each slice as it, i (it.kind === "markup" ? it.row.id : it.row.id + "/" + it.replyId)}
          {@const top = (win.start + i) * ROW_H}
          {#if it.kind === "markup"}
            {@const sel = store.selectedIds.has(it.row.id)}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="ml-row"
              class:selected={sel}
              role="row"
              tabindex="-1"
              aria-selected={sel}
              data-markup-id={it.row.id}
              style:top="{top}px"
              style:grid-template-columns={gridCols}
              onclick={(e) => onRowClick(e, it.row)}
              ondblclick={() => onRowDblClick(it.row)}
            >
              <span class="ml-cell ml-twist" role="gridcell">
                {#if it.row.replies > 0}
                  <button
                    class="ml-mini"
                    aria-label={expanded.has(it.row.id) ? "Hide replies" : "Show replies"}
                    aria-expanded={expanded.has(it.row.id)}
                    onclick={(e) => { e.stopPropagation(); toggleExpanded(it.row.id); }}
                  >{expanded.has(it.row.id) ? "▾" : "▸"}</button>
                {/if}
              </span>
              {#each cols as k (k)}
                {#if k === "status" && editingStatusId === it.row.id}
                  <span class="ml-cell" role="gridcell">
                    <!-- svelte-ignore a11y_autofocus -->
                    <select
                      class="ml-cell-select"
                      aria-label="Status"
                      autofocus
                      value={it.row.status}
                      onclick={(e) => e.stopPropagation()}
                      ondblclick={(e) => e.stopPropagation()}
                      onchange={(e) => onStatusPick(e, it.row.id)}
                      onblur={() => (editingStatusId = null)}
                    >
                      {#each STATUS_ORDER as s (s)}<option value={s}>{s}</option>{/each}
                    </select>
                  </span>
                {:else if k === "status"}
                  <span
                    class="ml-cell status-{it.row.status}"
                    role="gridcell"
                    tabindex="-1"
                    title="Double-click to change"
                    ondblclick={(e) => { e.stopPropagation(); select(new Set([it.row.id])); editingStatusId = it.row.id; }}
                  >{cellText(it.row, k)}</span>
                {:else if k === "colour"}
                  <span class="ml-cell" role="gridcell"><i class="ml-swatch" style:background={/^#[0-9a-fA-F]{3,8}$/.test(it.row.colour) ? it.row.colour : "transparent"}></i>{it.row.colour}</span>
                {:else}
                  <span class="ml-cell" class:num={columnKind(k) === "number"} role="gridcell" title={cellText(it.row, k)}>{cellText(it.row, k)}</span>
                {/if}
              {/each}
            </div>
          {:else}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="ml-row ml-reply"
              role="row"
              tabindex="-1"
              data-reply-of={it.row.id}
              style:top="{top}px"
              style:grid-template-columns={gridCols}
              onclick={() => onRowClick(new MouseEvent("click"), it.row)}
              ondblclick={() => onRowDblClick(it.row)}
            >
              <span class="ml-cell" role="gridcell"></span>
              {#each cols as k (k)}
                {#if k === "comment"}
                  <span class="ml-cell" role="gridcell" title={it.text} style:padding-left="{8 + it.depth * 14}px">&#8627; {it.text}</span>
                {:else if k === "author"}
                  <span class="ml-cell" role="gridcell">{it.author}</span>
                {:else if k === "date"}
                  <span class="ml-cell" role="gridcell">{it.at}</span>
                {:else}
                  <span class="ml-cell" role="gridcell"></span>
                {/if}
              {/each}
            </div>
          {/if}
        {/each}
      </div>
    </div>
  </div>
</div>

<style>
  .markups-list { display: flex; flex-direction: column; height: 100%; min-height: 0; font-size: var(--font-size-sm); }
  .ml-toolbar { display: flex; flex-wrap: wrap; gap: var(--space-2); align-items: center; padding: var(--space-1) var(--space-2); border-bottom: 1px solid var(--color-border); background: var(--color-bg-toolbar); }
  .ml-search { width: 220px; }
  .ml-pages { width: 110px; }
  .ml-btn, .ml-status-set, .ml-menu summary { background: var(--color-bg-panel-alt); color: var(--color-text); border: 1px solid var(--color-border); border-radius: var(--radius-sm); padding: 2px var(--space-2); font-size: var(--font-size-sm); cursor: pointer; }
  .ml-btn:disabled, .ml-status-set:disabled { opacity: 0.5; cursor: default; }
  .ml-menu { position: relative; }
  .ml-menu summary { list-style: none; }
  .ml-pop { position: absolute; z-index: 20; top: 100%; left: 0; min-width: 160px; max-height: 240px; overflow: auto; background: var(--color-bg-panel); border: 1px solid var(--color-border); border-radius: var(--radius-md); padding: var(--space-2); display: flex; flex-direction: column; gap: var(--space-1); }
  .ml-col-row { display: flex; align-items: center; gap: var(--space-1); justify-content: space-between; }
  .ml-mini { background: none; border: none; color: var(--color-text-secondary); cursor: pointer; padding: 0 4px; }
  .ml-mini:disabled { opacity: 0.3; cursor: default; }
  .ml-count { margin-left: auto; color: var(--color-text-muted); }
  .ml-ok { padding: 2px var(--space-2); color: var(--color-accent); }
  .ml-err { padding: 2px var(--space-2); color: var(--color-danger); }
  .ml-scroller { flex: 1; min-height: 0; overflow: auto; position: relative; outline: none; }
  .ml-scroller:focus-visible { box-shadow: inset 0 0 0 1px var(--color-primary); }
  .ml-inner { min-width: 100%; }
  .ml-head { display: grid; position: sticky; top: 0; z-index: 5; height: 24px; background: var(--color-bg-toolbar); border-bottom: 1px solid var(--color-border); }
  .ml-hcell { background: none; border: none; color: var(--color-text-muted); font-weight: 600; text-align: left; cursor: pointer; padding: 0 var(--space-2); font-size: var(--font-size-sm); }
  .ml-hcell:hover { color: var(--color-text); }
  .ml-body { position: relative; }
  .ml-row { display: grid; position: absolute; left: 0; right: 0; height: 24px; align-items: center; border-bottom: 1px solid var(--color-border-subtle); cursor: default; }
  .ml-row:hover { background: var(--color-bg-hover); }
  .ml-row.selected { background: var(--color-bg-active); box-shadow: inset 2px 0 0 var(--color-primary); }
  .ml-reply { color: var(--color-text-secondary); }
  .ml-cell { padding: 0 var(--space-2); overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .ml-cell.num { text-align: right; font-variant-numeric: tabular-nums; }
  .ml-twist { padding: 0; text-align: center; }
  .ml-cell-select { width: 100%; font-size: var(--font-size-sm); }
  .ml-swatch { display: inline-block; width: 10px; height: 10px; margin-right: 4px; border: 1px solid var(--color-border); }
  .status-Accepted { color: var(--color-status-accepted); }
  .status-Rejected { color: var(--color-status-rejected); }
  .status-Completed { color: var(--color-status-completed); }
  .status-Cancelled { color: var(--color-text-muted); }
  .ml-empty { padding: var(--space-3); color: var(--color-text-muted); }
</style>
