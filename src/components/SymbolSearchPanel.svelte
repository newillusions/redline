<script lang="ts">
  /**
   * Symbol Search: find every copy of one luminaire symbol on the page or the whole sheet set,
   * review each hit (accept or reject), then place the accepted ones as count markers in the
   * active count set.
   *
   * How to box the symbol: draw a Rectangle markup tightly around ONE clean symbol and leave it
   * selected (Bluebeam's "Get Rectangle"). The matching itself runs in Rust, off the UI thread
   * (`src/lib/symbol-search.ts`).
   */
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import type { UserRef } from "$lib/ipc";
  import {
    acceptedByPage, allPages, buildCountMarkups, hitsFromResponse, setAll, symbolSearch,
    templateRectFromMarkup, toggleHit, type Hit, type SymbolSearchResponse,
  } from "$lib/symbol-search";

  const {
    store,
    docId,
    currentPage = 0,
    pageCount = 1,
    identity,
    onjump,
  }: {
    store: MarkupStore;
    docId: string;
    currentPage?: number;
    pageCount?: number;
    identity: UserRef | null;
    onjump?: (target: { page: number; rect: [number, number, number, number] }) => void;
  } = $props();

  /** Strictness maps to the vector line share and the raster correlation. */
  const STRICTNESS = {
    loose: { minScore: 0.8, rasterThreshold: 0.65, label: "Loose" },
    normal: { minScore: 0.9, rasterThreshold: 0.75, label: "Normal" },
    exact: { minScore: 1.0, rasterThreshold: 0.9, label: "Exact" },
  } as const;
  type Strictness = keyof typeof STRICTNESS;

  let scope = $state<"page" | "all">("page");
  let rotations = $state(false);
  let scales = $state(false);
  let strictness = $state<Strictness>("normal");
  let label = $state("");

  let busy = $state(false);
  let error = $state<string | null>(null);
  let message = $state<string | null>(null);
  let response = $state<SymbolSearchResponse | null>(null);
  let hits = $state<Hit[]>([]);

  // The boxed symbol: exactly one selected Rectangle.
  const selected = $derived(store.selectedMarkups);
  const template = $derived(selected.length === 1 ? templateRectFromMarkup(selected[0]) : null);
  const templatePage = $derived(selected.length === 1 ? selected[0].page : currentPage);

  const acceptedCount = $derived(hits.filter((h) => h.accepted).length);
  const pagesWithHits = $derived([...acceptedByPage(hits).keys()].length);
  const grouped = $derived.by(() => {
    const out = new Map<number, Hit[]>();
    for (const h of hits) out.set(h.page, [...(out.get(h.page) ?? []), h]);
    return [...out];
  });

  async function find() {
    if (!template || busy) return;
    busy = true;
    error = null;
    message = null;
    const s = STRICTNESS[strictness];
    try {
      const r = await symbolSearch(
        docId, templatePage, template, scope === "all" ? allPages(pageCount) : null,
        { rotations, scales, minScore: s.minScore, rasterThreshold: s.rasterThreshold },
      );
      response = r;
      hits = hitsFromResponse(r, templatePage, template);
      if (hits.length === 0) message = "No matching symbols. Try Loose, or box the symbol more tightly.";
    } catch (e) {
      response = null;
      hits = [];
      // Tauri rejects with the command's Err(String), not an Error object.
      error = `Symbol search failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      busy = false;
    }
  }

  function place() {
    if (!identity || acceptedCount === 0) return;
    const ms = buildCountMarkups(hits, {
      identity,
      now: new Date().toISOString(),
      mintId: () => crypto.randomUUID(),
      set: store.activeCountSet,
      appearance: store.draftAppearance,
      label: label.trim() || null,
    });
    store.createMany(ms); // one undo step for the whole batch
    message = `Placed ${ms.length} count marker${ms.length === 1 ? "" : "s"} in ${store.activeCountSet?.name ?? "the count"}.`;
    hits = [];
    response = null;
  }

  function show(h: Hit) {
    const r = h.match.rect;
    onjump?.({ page: h.page, rect: [r.x, r.y, r.x + r.w, r.y + r.h] });
  }

  function clear() {
    hits = [];
    response = null;
    message = null;
    error = null;
  }
</script>

<section class="symbol-search" aria-label="Symbol search" data-testid="symbol-search">
  <div class="head">
    <span class="section-title">Symbol search</span>
  </div>

  {#if template}
    <p class="hint" data-testid="ss-template">
      Symbol box: page {templatePage + 1}, {template.w.toFixed(0)} x {template.h.toFixed(0)} pt.
    </p>
  {:else}
    <p class="hint" data-testid="ss-template">
      Draw a Rectangle tightly around one symbol and keep it selected.
    </p>
  {/if}

  <div class="controls">
    <label>Scope
      <select bind:value={scope} data-testid="ss-scope">
        <option value="page">This page</option>
        <option value="all" disabled={pageCount < 2}>All {pageCount} pages</option>
      </select>
    </label>
    <label>Match
      <select bind:value={strictness} data-testid="ss-strictness">
        {#each Object.entries(STRICTNESS) as [k, v] (k)}<option value={k}>{v.label}</option>{/each}
      </select>
    </label>
    <label class="check"><input type="checkbox" bind:checked={rotations} data-testid="ss-rotations" /> Turned copies</label>
    <label class="check"><input type="checkbox" bind:checked={scales} data-testid="ss-scales" /> Other sizes</label>
    <button type="button" class="btn" data-testid="ss-find" onclick={find} disabled={!template || busy}>
      {busy ? "Searching..." : "Find symbols"}
    </button>
  </div>

  {#if error}<div class="err" role="alert" data-testid="ss-error">{error}</div>{/if}
  {#if message}<div class="note" role="status" data-testid="ss-message">{message}</div>{/if}

  {#if response && hits.length === 0 && response.skipped.length > 0}
    <ul class="skipped" data-testid="ss-skipped-list">
      {#each response.skipped as sk (sk.page)}
        <li>Page {sk.page + 1}: {sk.reason}</li>
      {/each}
    </ul>
  {/if}

  {#if response && hits.length > 0}
    <div class="summary" data-testid="ss-summary">
      {hits.length} found on {grouped.length} page{grouped.length === 1 ? "" : "s"}
      ({response.method === "vector" ? "line work" : "image match"}, {response.elapsed_ms} ms).
      {acceptedCount} accepted.
      {#if response.skipped.length > 0}
        <span class="warn" data-testid="ss-skipped">{response.skipped.length} page{response.skipped.length === 1 ? "" : "s"} skipped.</span>
      {/if}
    </div>
    {#if response.truncated}
      <div class="warn" role="status" data-testid="ss-truncated">
        Results cut short (limit or time reached), so some copies may be missing. Box the symbol more
        tightly, or search fewer pages.
      </div>
    {/if}
    {#if response.skipped.length > 0}
      <ul class="skipped" data-testid="ss-skipped-list">
        {#each response.skipped as sk (sk.page)}
          <li>Page {sk.page + 1}: {sk.reason}</li>
        {/each}
      </ul>
    {/if}
    <div class="bulk">
      <button type="button" class="row-action" onclick={() => (hits = setAll(hits, true))}>Accept all</button>
      <button type="button" class="row-action" onclick={() => (hits = setAll(hits, false))}>Reject all</button>
      <button type="button" class="row-action" onclick={clear}>Discard</button>
    </div>
    <div class="list" role="list" data-testid="ss-list">
      {#each grouped as [page, rows] (page)}
        <div class="page-head">Page {page + 1} <span class="count">({rows.filter((r) => r.accepted).length}/{rows.length})</span></div>
        {#each rows as h (h.key)}
          <div class="hit" class:rejected={!h.accepted} role="listitem" data-hit={h.key}>
            <label>
              <input type="checkbox" checked={h.accepted} onchange={() => (hits = toggleHit(hits, h.key))} />
              <span>
                ({h.match.center[0].toFixed(0)}, {h.match.center[1].toFixed(0)})
                {Math.round(h.match.score * 100)}%{h.isTemplate ? " - your box" : ""}{h.match.angle !== 0 ? ` - turned ${Math.round(h.match.angle)} deg` : ""}
              </span>
            </label>
            <button type="button" class="row-action" onclick={() => show(h)}>Show</button>
          </div>
        {/each}
      {/each}
    </div>
    <div class="commit">
      <input type="text" placeholder="Count label (optional)" bind:value={label} data-testid="ss-label" />
      <button type="button" class="btn primary" data-testid="ss-place" onclick={place}
        disabled={!identity || acceptedCount === 0}>
        Place {acceptedCount} count{acceptedCount === 1 ? "" : "s"}{pagesWithHits > 1 ? ` on ${pagesWithHits} pages` : ""}
      </button>
    </div>
  {/if}
</section>

<style>
  .symbol-search { display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-2) var(--space-3); border-top: 1px solid var(--color-border); }
  .section-title { font-size: var(--font-size-sm); font-weight: 600; color: var(--color-text); }
  .hint, .note, .summary { margin: 0; font-size: var(--font-size-sm); color: var(--color-text-secondary); }
  .err { color: var(--color-error, #e02424); font-size: var(--font-size-sm); }
  .warn { color: var(--color-warning, #b45309); }
  .skipped { margin: 0; padding-left: var(--space-4); font-size: var(--font-size-xs); color: var(--color-warning, #b45309); }
  .controls { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); font-size: var(--font-size-sm); }
  .controls label { display: flex; align-items: center; gap: var(--space-1); color: var(--color-text); }
  .btn, .row-action {
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-3);
    background: var(--color-bg-active);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .row-action { font-size: var(--font-size-xs); padding: 0 var(--space-2); }
  .btn.primary { background: var(--color-primary); color: var(--color-on-primary, #fff); border-color: var(--color-primary); }
  .btn:disabled, .row-action:disabled { opacity: 0.45; cursor: not-allowed; }
  .bulk { display: flex; gap: var(--space-1); }
  .list { max-height: 220px; overflow: auto; border: 1px solid var(--color-border); border-radius: var(--radius-sm); }
  .page-head { position: sticky; top: 0; background: var(--color-bg-active); padding: 0 var(--space-2); font-size: var(--font-size-xs); font-weight: 600; }
  .page-head .count { font-weight: 400; color: var(--color-text-muted); }
  .hit { display: flex; align-items: center; justify-content: space-between; padding: 0 var(--space-2); font-size: var(--font-size-xs); }
  .hit label { display: flex; align-items: center; gap: var(--space-1); cursor: pointer; }
  .hit.rejected span { text-decoration: line-through; color: var(--color-text-muted); }
  .commit { display: flex; gap: var(--space-2); align-items: center; }
  .commit input[type="text"] { flex: 1; min-width: 0; font-size: var(--font-size-sm); }
</style>
