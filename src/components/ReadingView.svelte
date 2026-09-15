<script lang="ts">
  /**
   * Continuous reading mode — all pages in one vertically scrolling column, fit-to-width
   * by default and zoomable (owner request 2026-09-15). A sibling to Viewport.svelte, not
   * a rewrite of it: single-page mode (Viewport.svelte) is completely unchanged by this
   * file, and App.svelte mounts exactly one of the two depending on the persisted
   * `viewer_mode` setting (see App.svelte's "Centre viewport" section).
   *
   * DELIBERATE SCOPING (stated here so it's visible, not buried in a PR description):
   *  - Render model: ONE raster per page, not Viewport.svelte's 512px tile grid. A page
   *    is rasterized whole via a single `renderTile` call (tile_x=0, tile_y=0, tile size
   *    computed by `pageTileSizeCss` to cover the whole page). This is simpler than
   *    porting the multi-tile math to a multi-page virtualized column, and still reuses
   *    the SAME bounded, evicting cache (`BoundedTileCache`, tile-cache.ts) Viewport.svelte
   *    uses for its tiles — just with page index in place of (tile_x, tile_y). At extreme
   *    zoom on a very large sheet the raster is capped (see `pageTileSizeCss`); single-page
   *    mode remains the path for fine detail work at high zoom.
   *  - Virtualization: only pages within `visiblePageRange`'s window (visible + a small
   *    lookahead) are ever fetched or kept cached — `pageCache.evictExcept` proactively
   *    drops anything outside that window on every scroll-driven window change, so a long
   *    document never holds more than a handful of page rasters in memory regardless of
   *    document length (owner constraint: never render all pages at once).
   *  - Zoom fetch key is quantized (`quantizeZoom`, reused from viewport.ts — the exact
   *    fix built for the 2026-07 Windows tile-cache freeze) while the on-screen page BOX
   *    sizes track the live continuous zoom; the `<img>` naturally CSS-scales between
   *    raster refreshes, so zooming still feels smooth without re-fetching on every wheel
   *    tick.
   *  - Read/navigate only in this PR: no markup rendering, creation, or editing in reading
   *    mode. That was not in the four requested reading-mode behaviours (column, page
   *    indicator, thumbnails, extract) and porting markup hit-testing/tool state across a
   *    multi-page virtualized column is a materially larger effort — switching to
   *    single-page mode is the path for markup work, unchanged.
   *  - Page sizes for the whole document are fetched once on mount (chunked, not one
   *    giant `Promise.all`, so an extremely long document doesn't fire hundreds of
   *    concurrent IPC calls at once) — metadata-only, cheap, and the render engine's own
   *    page-handle cache (see render/mod.rs) makes repeat access free either way.
   */
  import { onMount, onDestroy } from "svelte";
  import { getPageSize, renderTile, type DocumentInfo, type RenderedTile } from "$lib/ipc";
  import {
    fitWidthZoom,
    quantizeZoom,
    clampTileDpr,
    parseZoomPercent,
    ZOOM_MIN,
    ZOOM_MAX,
    type ViewportSnapshot,
  } from "$lib/viewport";
  import {
    computePageLayout,
    pageAtScrollTop,
    scrollTopForPage,
    visiblePageRange,
    pageTileSizeCss,
    type PageDims,
    type PageLayout,
  } from "$lib/reading-mode";
  import { BoundedTileCache, DEFAULT_TILE_CACHE_CAP_BYTES } from "$lib/tile-cache";

  // ---------------------------------------------------------------------------
  // Props
  // ---------------------------------------------------------------------------
  const {
    docInfo,
    initialState = undefined,
    onviewportchange = undefined,
    jumpRequest = null,
  }: {
    docInfo: DocumentInfo;
    /** Restored zoom/scroll on mount (tab-switch parity with Viewport.svelte). */
    initialState?: ViewportSnapshot;
    /** Fired on every zoom/scroll change — App.svelte saves it into the tab snapshot,
     *  the same callback contract Viewport.svelte already uses. */
    onviewportchange?: (s: ViewportSnapshot) => void;
    /** Page-jump request (from PageIndicator, or a future thumbnail click) — bump
     *  `nonce` on every request, including a repeat jump to the same page. */
    jumpRequest?: { page: number; nonce: number } | null;
  } = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------
  let scrollEl = $state<HTMLDivElement | null>(null);
  let containerWidth = $state(0);
  let containerHeight = $state(0);
  let scrollTop = $state(initialState?.scrollY ?? 0);
  let zoom = $state(initialState?.zoom ?? 1.0);

  let pageDims = $state<PageDims[]>([]);
  let pageSizesLoaded = $state(false);
  let autoFitDone = false;

  interface PageRaster {
    naturalWidth: number;
    naturalHeight: number;
    src: string;
  }
  /** Reactive projection of `pageCache` for pages in the current render window, at the
   *  current quantized zoom — the template reads this, never the cache directly (a
   *  plain Map's mutations aren't tracked by Svelte's reactivity). */
  let rasterByPage = $state<Record<number, PageRaster>>({});

  const pageCache = new BoundedTileCache<PageRaster>(DEFAULT_TILE_CACHE_CAP_BYTES);
  const pendingKeys = new Set<string>();
  const dpr = clampTileDpr(typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1);

  let renderWindow = $state<{ first: number; last: number }>({ first: 0, last: -1 });
  let lastWindowSignature = "";
  let lastJumpNonce = -1;

  let zoomInputFocused = $state(false);
  let zoomInputDraft = $state("");

  const layout: PageLayout = $derived(computePageLayout(pageDims, zoom));
  const currentPage = $derived(pageAtScrollTop(layout, scrollTop));
  const zoomInputDisplay = $derived(zoomInputFocused ? zoomInputDraft : String(Math.round(zoom * 100)));

  function pageKey(pageIndex: number, zoomMillis: number): string {
    return `${pageIndex},${zoomMillis}`;
  }

  // ---------------------------------------------------------------------------
  // Page sizes (loaded once per doc)
  // ---------------------------------------------------------------------------
  const PAGE_SIZE_FETCH_CHUNK = 25;

  async function loadPageSizes() {
    const count = docInfo.page_count;
    const dims: PageDims[] = new Array(count);
    for (let start = 0; start < count; start += PAGE_SIZE_FETCH_CHUNK) {
      const end = Math.min(count, start + PAGE_SIZE_FETCH_CHUNK);
      const batch = await Promise.all(
        Array.from({ length: end - start }, (_, i) => getPageSize(docInfo.doc_id, start + i)),
      );
      batch.forEach((s, i) => {
        dims[start + i] = { width_pts: s.width_pts, height_pts: s.height_pts };
      });
      pageDims = dims.slice(0, end); // progressive: pages already loaded can render early
    }
    pageSizesLoaded = true;
  }

  // ---------------------------------------------------------------------------
  // Auto-fit-width on first layout after a fresh (never-customized) tab open — see the
  // header comment's "fit-to-width by default" requirement. A snapshot that's still
  // exactly DEFAULT_VIEWPORT_SNAPSHOT (doc-tabs.svelte.ts) means neither viewer mode has
  // customized this tab's view yet; anything else (including a snapshot this same
  // component wrote on an earlier mount) is respected as-is, matching Viewport.svelte's
  // own restore-exactly contract for initialState.
  // ---------------------------------------------------------------------------
  $effect(() => {
    if (autoFitDone || pageDims.length === 0 || containerWidth <= 0) return;
    const isUntouchedDefault =
      (initialState?.zoom ?? 1.0) === 1.0 &&
      (initialState?.pageIndex ?? 0) === 0 &&
      (initialState?.scrollX ?? 0) === 0 &&
      (initialState?.scrollY ?? 0) === 0;
    if (isUntouchedDefault) {
      zoom = fitWidthZoom(pageDims[0].width_pts, containerWidth);
    }
    autoFitDone = true;
  });

  // ---------------------------------------------------------------------------
  // Virtualized render window: which pages to fetch/keep cached right now.
  // ---------------------------------------------------------------------------
  const READING_LOOKAHEAD_PAGES = 1;

  $effect(() => {
    if (layout.topsCss.length === 0 || containerHeight <= 0) return;

    const win = visiblePageRange(layout, scrollTop, containerHeight, READING_LOOKAHEAD_PAGES);
    const quantized = quantizeZoom(zoom, ZOOM_MIN, ZOOM_MAX);
    const zoomMillis = Math.round(quantized * 1000);
    const signature = `${win.first}-${win.last}-${zoomMillis}`;
    if (signature === lastWindowSignature) return;
    lastWindowSignature = signature;
    renderWindow = win;

    const windowIndices: number[] = [];
    for (let i = win.first; i <= win.last; i++) windowIndices.push(i);
    const keepKeys = new Set(windowIndices.map((i) => pageKey(i, zoomMillis)));

    // Drop reactive entries for pages that scrolled out of the window, and proactively
    // evict them from the underlying cache too (see header comment — never hold rasters
    // for off-screen pages indefinitely).
    const nextRasterByPage: Record<number, PageRaster> = {};
    for (const i of windowIndices) {
      if (rasterByPage[i]) nextRasterByPage[i] = rasterByPage[i];
    }
    rasterByPage = nextRasterByPage;
    pageCache.evictExcept((key) => keepKeys.has(key));

    for (const i of windowIndices) {
      void fetchPageRaster(i, quantized, zoomMillis, keepKeys);
    }
  });

  async function fetchPageRaster(
    pageIndex: number,
    zoomAtRequest: number,
    zoomMillis: number,
    protectedKeys: ReadonlySet<string>,
  ) {
    const key = pageKey(pageIndex, zoomMillis);
    const cached = pageCache.get(key);
    if (cached) {
      rasterByPage = { ...rasterByPage, [pageIndex]: cached };
      return;
    }
    if (pendingKeys.has(key)) return;
    pendingKeys.add(key);
    try {
      const dims = pageDims[pageIndex];
      if (!dims) return; // size not loaded yet — a later size-load pass will retry
      const tileSizeCss = pageTileSizeCss(dims.width_pts, dims.height_pts, zoomAtRequest);
      const result: RenderedTile = await renderTile({
        doc_id: docInfo.doc_id,
        page_index: pageIndex,
        tile_size_css: tileSizeCss,
        tile_x: 0,
        tile_y: 0,
        zoom: zoomAtRequest,
        dpr,
      });
      const raster: PageRaster = {
        naturalWidth: result.width_px,
        naturalHeight: result.height_px,
        src: `data:image/png;base64,${result.png_base64}`,
      };
      // Stale guard: only surface this raster if the page is still in the window that
      // requested it (a slow fetch from a window the user has since scrolled past).
      if (protectedKeys.has(key)) {
        pageCache.set(key, raster, protectedKeys);
        rasterByPage = { ...rasterByPage, [pageIndex]: raster };
      }
    } catch (e) {
      console.error(`Reading-mode page ${pageIndex} render failed:`, e);
    } finally {
      pendingKeys.delete(key);
    }
  }

  // ---------------------------------------------------------------------------
  // Scroll / viewport-change reporting
  // ---------------------------------------------------------------------------
  function handleScroll() {
    if (!scrollEl) return;
    scrollTop = scrollEl.scrollTop;
  }

  $effect(() => {
    onviewportchange?.({ zoom, pageIndex: currentPage, scrollX: 0, scrollY: scrollTop });
  });

  // ---------------------------------------------------------------------------
  // Jump requests (PageIndicator, future thumbnail click)
  // ---------------------------------------------------------------------------
  $effect(() => {
    if (!jumpRequest || jumpRequest.nonce === lastJumpNonce) return;
    lastJumpNonce = jumpRequest.nonce;
    if (layout.topsCss.length === 0 || !scrollEl) return;
    const top = scrollTopForPage(layout, jumpRequest.page);
    scrollEl.scrollTop = top;
    scrollTop = top;
  });

  // ---------------------------------------------------------------------------
  // Zoom controls (toolbar overlay — Fit Width, zoom-percent input; wheel-zoom via
  // Ctrl/Cmd+wheel or Shift+wheel, matching docs/navigation.md's existing scheme).
  // Plain wheel/trackpad-swipe is NOT intercepted here — native scroll already IS pan
  // in a continuous column, unlike Viewport.svelte's single fixed page.
  // ---------------------------------------------------------------------------
  function fitWidth() {
    if (pageDims.length === 0) return;
    zoom = fitWidthZoom(pageDims[0].width_pts, containerWidth);
  }

  function commitZoomInput() {
    const parsed = parseZoomPercent(zoomInputDraft, ZOOM_MIN, ZOOM_MAX);
    if (parsed !== null) zoom = parsed;
  }

  function onWheel(e: WheelEvent) {
    const zooming = e.ctrlKey || e.metaKey || e.shiftKey;
    if (!zooming) return; // plain wheel/trackpad-swipe: let the browser scroll natively
    e.preventDefault();
    const factor = Math.min(2, Math.max(0.5, Math.exp(-e.deltaY * 0.0015)));
    zoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, zoom * factor));
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------
  let resizeObserver: ResizeObserver | null = null;

  onMount(() => {
    if (scrollEl) {
      resizeObserver = new ResizeObserver((entries) => {
        const entry = entries[0];
        if (!entry) return;
        containerWidth = entry.contentRect.width;
        containerHeight = entry.contentRect.height;
      });
      resizeObserver.observe(scrollEl);
      scrollEl.scrollTop = scrollTop;
    }
    void loadPageSizes();
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    pageCache.clear();
    pendingKeys.clear();
  });
</script>

<div
  class="reading-scroll"
  data-doc-id={docInfo.doc_id}
  bind:this={scrollEl}
  onscroll={handleScroll}
  onwheel={onWheel}
  role="document"
  aria-label="Continuous PDF reading view — scroll to move between pages"
>
  <div class="reading-content" style={`height:${layout.totalHeightCss}px`}>
    {#if !pageSizesLoaded && pageDims.length === 0}
      <div class="reading-loading">Loading pages…</div>
    {/if}
    {#each Array.from({ length: Math.max(0, renderWindow.last - renderWindow.first + 1) }, (_, k) => renderWindow.first + k) as idx (idx)}
      {#if layout.topsCss[idx] !== undefined}
        <div
          class="reading-page"
          data-page-index={idx}
          style={`top:${layout.topsCss[idx]}px; width:${layout.widthsCss[idx]}px; height:${layout.heightsCss[idx]}px;`}
        >
          {#if rasterByPage[idx]}
            <img src={rasterByPage[idx].src} alt={`Page ${idx + 1}`} class="reading-page-image" />
          {:else}
            <div class="reading-page-placeholder" aria-hidden="true"></div>
          {/if}
        </div>
      {/if}
    {/each}
  </div>

  <!-- Zoom toolbar overlay (mirrors Viewport.svelte's floating zoom controls). -->
  <div class="reading-zoom-toolbar">
    <button class="btn-zoom" title="Fit page width" onclick={fitWidth}>Fit W</button>
    <input
      class="zoom-percent-input"
      type="text"
      inputmode="numeric"
      aria-label="Zoom percent"
      title="Type a zoom percentage and press Enter"
      value={zoomInputDisplay}
      onfocus={() => { zoomInputFocused = true; zoomInputDraft = String(Math.round(zoom * 100)); }}
      onblur={() => { zoomInputFocused = false; }}
      oninput={(e) => { zoomInputDraft = (e.target as HTMLInputElement).value; }}
      onkeydown={(e) => {
        if (e.key === "Enter") { e.preventDefault(); commitZoomInput(); (e.target as HTMLInputElement).blur(); }
        else if (e.key === "Escape") { e.preventDefault(); (e.target as HTMLInputElement).blur(); }
      }}
    />
    <span class="reading-zoom-indicator">{Math.round(zoom * 100)}%</span>
  </div>
</div>

<style>
  .reading-scroll {
    width: 100%;
    height: 100%;
    overflow-y: auto;
    overflow-x: auto;
    position: relative;
    background: var(--color-bg-panel-alt);
  }

  .reading-content {
    position: relative;
    width: 100%;
  }

  .reading-loading {
    position: absolute;
    top: var(--space-4);
    left: 50%;
    transform: translateX(-50%);
    color: var(--color-text-muted);
    font-size: var(--font-size-sm);
  }

  .reading-page {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    background: #fff;
    box-shadow: 0 0 0 1px var(--color-border-subtle);
  }

  .reading-page-image {
    display: block;
    width: 100%;
    height: 100%;
  }

  .reading-page-placeholder {
    width: 100%;
    height: 100%;
    background: var(--color-bg-panel);
  }

  .reading-zoom-toolbar {
    position: absolute;
    bottom: var(--space-3);
    right: var(--space-3);
    display: flex;
    align-items: center;
    gap: var(--space-1);
    background: var(--color-bg-panel);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-1) var(--space-2);
  }

  .btn-zoom {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: 2px var(--space-2);
  }

  .btn-zoom:hover {
    background: var(--color-bg-hover);
  }

  .zoom-percent-input {
    background: var(--color-bg);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    padding: 2px var(--space-1);
    width: 3.5em;
    text-align: center;
  }

  .reading-zoom-indicator {
    color: var(--color-text-muted);
    font-size: var(--font-size-xs);
  }
</style>
