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
   *  - Mark-up (2026-11-05, docs/continuous-markup.md): every mounted page box hosts a
   *    Viewport.svelte in page-slot mode as its mark-up layer - the same tools, selection,
   *    measurement, viewports and right-click menu as single-page view, acting on that
   *    page. The page under the pointer is the active one (keys and page commands go to
   *    it); while a page holds an unfinished gesture it stays active and mounted and the
   *    other pages ignore the pointer, so a gesture never crosses pages. Slots draw no
   *    tiles, so scrolling still only fetches this file's one raster per page.
   *  - Page sizes for the whole document are fetched once on mount (chunked, not one
   *    giant `Promise.all`, so an extremely long document doesn't fire hundreds of
   *    concurrent IPC calls at once) — metadata-only, cheap, and the render engine's own
   *    page-handle cache (see render/mod.rs) makes repeat access free either way.
   */
  import { onMount, onDestroy } from "svelte";
  import { getPageSize, renderTile, type DocumentInfo, type RenderedTile } from "$lib/ipc";
  import {
    fitWidthZoom,
    fitHeightZoom,
    ACTUAL_SIZE_ZOOM,
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
  import {
    pageLeftsCss,
    readingContentWidthCss,
    pageAtPoint,
    resolveActivePage,
    type SlotCommand,
  } from "$lib/reading-mode";
  import { layerStoreFor } from "$lib/layers-store.svelte";
  import Viewport from "./Viewport.svelte";
  import { uiPrefs } from "$lib/ui-prefs.svelte";
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import { TakeoffStore } from "$lib/takeoff-store.svelte";
  import type { NavigationStore } from "$lib/navigation-store.svelte";
  import type { NavTarget } from "$lib/navigation-ipc";
  import { getUserIdentity, type SearchHit, type UserRef } from "$lib/ipc";

  // ---------------------------------------------------------------------------
  // Props
  // ---------------------------------------------------------------------------
  const {
    docInfo,
    store = undefined,
    takeoffStore = new TakeoffStore(),
    navStore = undefined,
    searchHits = [],
    activeSearchHitIdx = null,
    initialState = undefined,
    onviewportchange = undefined,
    onsearchtext = undefined,
    onfollowlink = undefined,
    onrequestpanel = undefined,
    onpageop = undefined,
    jumpRequest = null,
  }: {
    docInfo: DocumentInfo;
    /** The tab's mark-ups. Without one the column is read-only (no mark-up layer). */
    store?: MarkupStore;
    takeoffStore?: TakeoffStore;
    navStore?: NavigationStore;
    searchHits?: SearchHit[];
    activeSearchHitIdx?: number | null;
    onsearchtext?: (text: string) => void;
    onfollowlink?: (target: NavTarget) => void;
    onrequestpanel?: (panel: "properties" | "toolchest") => void;
    /** A page operation run from a page's menu (rotate) changed the file. */
    onpageop?: () => void;
    /** Restored zoom/scroll on mount (tab-switch parity with Viewport.svelte). */
    initialState?: ViewportSnapshot;
    /** Fired on every zoom/scroll change — App.svelte saves it into the tab snapshot,
     *  the same callback contract Viewport.svelte already uses. */
    onviewportchange?: (s: ViewportSnapshot) => void;
    /** Page-jump request (from PageIndicator, or a future thumbnail click) — bump
     *  `nonce` on every request, including a repeat jump to the same page. */
    jumpRequest?: {
      page: number;
      rect?: [number, number, number, number];
      markupId?: string;
      view?: { left: number; top: number; zoom: number | null };
      nonce: number;
    } | null;
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
  /** One user-identity lookup for every page's mark-up layer (each slot reports a failure),
   *  made when the first layer mounts. */
  let identityLookup: Promise<UserRef> | null = null;
  function sharedIdentity(): Promise<UserRef> {
    if (!identityLookup) {
      identityLookup = getUserIdentity();
      identityLookup.catch(() => {});
    }
    return identityLookup;
  }
  const pendingKeys = new Set<string>();
  const dpr = clampTileDpr(typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1);

  let renderWindow = $state<{ first: number; last: number }>({ first: 0, last: -1 });
  let lastWindowSignature = "";
  let lastJumpNonce = -1;

  let zoomInputFocused = $state(false);
  let zoomInputDraft = $state("");

  const layout: PageLayout = $derived(computePageLayout(pageDims, zoom));
  const contentWidth = $derived(readingContentWidthCss(layout, containerWidth));
  const lefts = $derived(pageLeftsCss(layout, contentWidth));
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

  // Layers: the rendered view is shared with the single-page viewer (backend), so a change
  // there drops these cached pages and refetches the window.
  const layerView = $derived(layerStoreFor(docInfo.doc_id));
  let lastLayerEpoch = 0;

  $effect(() => {
    if (layout.topsCss.length === 0 || containerHeight <= 0) return;

    const win = visiblePageRange(layout, scrollTop, containerHeight, READING_LOOKAHEAD_PAGES);
    const quantized = quantizeZoom(zoom, ZOOM_MIN, ZOOM_MAX);
    const zoomMillis = Math.round(quantized * 1000);
    const layerEpoch = layerView.epoch;
    const signature = `${win.first}-${win.last}-${zoomMillis}-${layerEpoch}`;
    if (signature === lastWindowSignature) return;
    lastWindowSignature = signature;
    if (layerEpoch !== lastLayerEpoch) {
      lastLayerEpoch = layerEpoch;
      pageCache.clear();
      pendingKeys.clear();
    }
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
    const epochAtRequest = layerView.epoch;
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
      if (protectedKeys.has(key) && epochAtRequest === layerView.epoch) {
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
    // Retarget once the scroll settles, not per frame: the page under a resting pointer
    // would otherwise mount its mark-up layer mid-fling (see layerReady).
    if (retargetTimer !== null) clearTimeout(retargetTimer);
    retargetTimer = setTimeout(() => {
      retargetTimer = null;
      retargetAfterScroll();
    }, 100);
  }
  let retargetTimer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    onviewportchange?.({ zoom, pageIndex: currentPage, scrollX: 0, scrollY: scrollTop });
  });

  // ---------------------------------------------------------------------------
  // Jump requests (PageIndicator, future thumbnail click)
  // ---------------------------------------------------------------------------
  $effect(() => {
    const req = jumpRequest;
    if (!req || req.nonce === lastJumpNonce) return;
    if (layout.topsCss.length === 0 || !scrollEl) return;
    lastJumpNonce = req.nonce;
    // A jump moves the view away from the page the pointer was last over.
    if (busyPage === null) hoverPage = null;
    const page = Math.max(0, Math.min(req.page, layout.topsCss.length - 1));
    const dims = pageDims[page];
    let left: number | null = null;
    if (req.view?.zoom && req.view.zoom > 0) zoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, req.view.zoom));
    // Recompute from the (possibly new) zoom: layout is derived, so read it fresh.
    const lay = computePageLayout(pageDims, zoom);
    const lft = pageLeftsCss(lay, readingContentWidthCss(lay, containerWidth));
    let top = lay.topsCss[page];
    if (dims && req.rect) {
      // Centre a search hit's rect in the view.
      const [x0, y0, x1, y1] = req.rect;
      top += (dims.height_pts - (y0 + y1) / 2) * zoom - containerHeight / 2;
      left = lft[page] + ((x0 + x1) / 2) * zoom - containerWidth / 2;
    } else if (dims && req.view) {
      // A bookmark / link destination: its point at the top-left of the view.
      top += (dims.height_pts - req.view.top) * zoom;
      left = lft[page] + req.view.left * zoom;
    }
    scrollEl.scrollTop = Math.max(0, top);
    if (left !== null) scrollEl.scrollLeft = Math.max(0, left);
    scrollTop = scrollEl.scrollTop;
    if (req.markupId && store) {
      // Select tool first: Viewport clears the selection under any other tool.
      store.activeTool = "select";
      store.selectedIds = new Set([req.markupId]);
    }
  });

  // ---------------------------------------------------------------------------
  // Mark-up: which page is active (docs/continuous-markup.md)
  // ---------------------------------------------------------------------------
  /** Last page the pointer was over (null until it has been over one). */
  let hoverPage = $state<number | null>(null);
  /** The page holding an unfinished gesture, if any: it stays active and mounted, and the
   *  other pages ignore the pointer until it is done. */
  let busyPage = $state<number | null>(null);
  /** The page that takes keys and page commands. A remembered hover page counts only while it
   *  is still in the render window: after a jump (search hit, Markups list, thumbnail) or a
   *  long scroll it may be far away and unmounted, and then no page would take the keys. */
  const activePage = $derived(resolveActivePage({ busyPage, hoverPage, currentPage, window: renderWindow }));
  /** Pages with a mounted page box: the render window, plus a busy page scrolled out of it
   *  (unmounting it would drop the half-made mark-up). */
  const mountedPages = $derived.by(() => {
    const out: number[] = [];
    for (let i = renderWindow.first; i <= renderWindow.last; i++) out.push(i);
    if (busyPage !== null && !out.includes(busyPage) && busyPage < layout.topsCss.length) out.push(busyPage);
    return out;
  });

  // A page's mark-up layer is a full Viewport; mounting one costs a few milliseconds, which a
  // fast scroll through a sheet set would pay on every frame that brings a page into the
  // window. So layers mount when the browser is idle (one per idle slice, nearest page
  // first), like tiles sharpening after a zoom: a fling shows the rasters at full frame rate
  // and the mark-ups follow as it settles. The page under the pointer and a busy page mount
  // at once - those are the ones being worked on.
  let layerReady = $state<ReadonlySet<number>>(new Set());
  let layerIdleHandle: number | null = null;
  const requestIdle: (cb: () => void) => number =
    typeof window !== "undefined" && "requestIdleCallback" in window
      ? (cb) => window.requestIdleCallback(cb, { timeout: 250 })
      : (cb) => window.setTimeout(cb, 16);
  const cancelIdle: (h: number) => void =
    typeof window !== "undefined" && "cancelIdleCallback" in window
      ? (h) => window.cancelIdleCallback(h)
      : (h) => window.clearTimeout(h);

  $effect(() => {
    if (!store) return;
    const wanted = mountedPages;
    const ready = layerReady;
    // Drop layers that left the window (their page box is gone anyway).
    const kept = [...ready].filter((p) => wanted.includes(p));
    if (kept.length !== ready.size) {
      layerReady = new Set(kept);
      return;
    }
    const pending = wanted.filter((p) => !ready.has(p));
    if (pending.length === 0 || layerIdleHandle !== null) return;
    const anchor = currentPage;
    pending.sort((a, b) => Math.abs(a - anchor) - Math.abs(b - anchor));
    layerIdleHandle = requestIdle(() => {
      layerIdleHandle = null;
      layerReady = new Set([...layerReady, pending[0]]);
    });
  });

  function hasLayer(page: number): boolean {
    return layerReady.has(page) || page === hoverPage || page === busyPage || page === activePage;
  }

  function pageUnder(clientX: number, clientY: number): number | null {
    if (!scrollEl) return null;
    const r = scrollEl.getBoundingClientRect();
    return pageAtPoint(layout, lefts, clientX - r.left + scrollEl.scrollLeft, clientY - r.top + scrollEl.scrollTop);
  }

  /** Last pointer position over the column: a wheel scroll moves pages under a still pointer. */
  let lastPointer: { x: number; y: number; buttons: number } | null = null;

  function retargetAfterScroll() {
    if (!lastPointer || busyPage !== null || lastPointer.buttons !== 0) return;
    const p = pageUnder(lastPointer.x, lastPointer.y);
    if (p !== null) hoverPage = p;
  }

  function trackPointer(e: PointerEvent) {
    lastPointer = { x: e.clientX, y: e.clientY, buttons: e.buttons };
    // Never retarget mid-drag or mid-gesture: the page the gesture started on keeps it.
    if (busyPage !== null || e.buttons !== 0) return;
    const p = pageUnder(e.clientX, e.clientY);
    if (p !== null) hoverPage = p;
  }

  function onPointerDownCapture(e: PointerEvent) {
    if (busyPage !== null) return;
    const p = pageUnder(e.clientX, e.clientY);
    if (p !== null) hoverPage = p;
  }

  function onSlotBusy(page: number, busy: boolean) {
    if (busy) busyPage = page;
    else if (busyPage === page) busyPage = null;
  }

  function onSlotCommand(cmd: SlotCommand) {
    const dims = pageDims[activePage];
    switch (cmd) {
      case "fit-width": if (dims) zoom = fitWidthZoom(dims.width_pts, containerWidth); return;
      case "fit-height": if (dims) zoom = fitHeightZoom(dims.height_pts, containerHeight); return;
      case "actual-size": zoom = ACTUAL_SIZE_ZOOM; return;
      case "zoom-in": zoom = Math.min(ZOOM_MAX, zoom * 1.1); return;
      case "zoom-out": zoom = Math.max(ZOOM_MIN, zoom / 1.1); return;
      case "prev-page": scrollToPage(currentPage - 1); return;
      case "next-page": scrollToPage(currentPage + 1); return;
    }
  }

  function scrollToPage(page: number) {
    if (!scrollEl || layout.topsCss.length === 0) return;
    scrollEl.scrollTop = scrollTopForPage(layout, page);
    scrollTop = scrollEl.scrollTop;
  }

  // Hand tool: a drag pans the column (the slots leave panning to it).
  let panFrom: { x: number; y: number; left: number; top: number } | null = null;
  function onMouseDown(e: MouseEvent) {
    if (!scrollEl || e.button !== 0 || (store && store.activeTool !== "hand")) return;
    if (e.target instanceof Element && e.target.closest(".reading-zoom-toolbar, button, input, a")) return;
    panFrom = { x: e.clientX, y: e.clientY, left: scrollEl.scrollLeft, top: scrollEl.scrollTop };
    window.addEventListener("mousemove", onPanMove);
    window.addEventListener("mouseup", endPan);
    window.addEventListener("blur", endPan);
  }
  function onPanMove(e: MouseEvent) {
    if (!panFrom || !scrollEl) return;
    scrollEl.scrollLeft = panFrom.left - (e.clientX - panFrom.x);
    scrollEl.scrollTop = panFrom.top - (e.clientY - panFrom.y);
  }
  function endPan() {
    panFrom = null;
    window.removeEventListener("mousemove", onPanMove);
    window.removeEventListener("mouseup", endPan);
    window.removeEventListener("blur", endPan);
  }

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
    endPan();
    if (layerIdleHandle !== null) cancelIdle(layerIdleHandle);
    if (retargetTimer !== null) clearTimeout(retargetTimer);
    pageCache.clear();
    pendingKeys.clear();
  });
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions (pointer tracking and hand-tool
     pan for the mark-up layers; the column itself is a document, not a control) -->
<div
  class="reading-scroll"
  data-doc-id={docInfo.doc_id}
  bind:this={scrollEl}
  onscroll={handleScroll}
  onwheel={onWheel}
  onpointermove={trackPointer}
  onpointerleave={() => { lastPointer = null; if (busyPage === null) hoverPage = null; }}
  onpointerdowncapture={onPointerDownCapture}
  onmousedown={onMouseDown}
  role="document"
  aria-label="Continuous PDF reading view — scroll to move between pages"
>
  <div class="reading-content" style={`height:${layout.totalHeightCss}px; width:${contentWidth}px`}>
    {#if !pageSizesLoaded && pageDims.length === 0}
      <div class="reading-loading">Loading pages…</div>
    {/if}
    {#each mountedPages as idx (idx)}
      {#if layout.topsCss[idx] !== undefined}
        <div
          class="reading-page"
          class:reading-page-active={store !== undefined && idx === activePage}
          data-page-index={idx}
          style={`top:${layout.topsCss[idx]}px; left:${lefts[idx]}px; width:${layout.widthsCss[idx]}px; height:${layout.heightsCss[idx]}px;`}
        >
          {#if rasterByPage[idx]}
            <img src={rasterByPage[idx].src} alt={`Page ${idx + 1}`} class="reading-page-image" draggable="false" />
          {:else}
            <div class="reading-page-placeholder" aria-hidden="true"></div>
          {/if}
          {#if store && hasLayer(idx)}
            <!-- This page's mark-up layer (Viewport in page-slot mode). -->
            <div class="reading-page-markup">
              <Viewport
                {docInfo}
                {store}
                {takeoffStore}
                {navStore}
                {searchHits}
                {activeSearchHitIdx}
                {onsearchtext}
                {onfollowlink}
                {onrequestpanel}
                {onpageop}
                slot={{ page: idx, zoom, active: idx === activePage, inert: busyPage !== null && busyPage !== idx }}
                sharedIdentity={sharedIdentity()}
                onslotbusy={(busy) => onSlotBusy(idx, busy)}
                onslotcommand={onSlotCommand}
              />
            </div>
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
    <button
      class="btn-zoom btn-reuse"
      class:active={uiPrefs.reuseTools}
      aria-pressed={uiPrefs.reuseTools}
      title={uiPrefs.reuseTools
        ? "Reuse tools is on: a tool stays active after each markup. Click to return to Select after each markup."
        : "Reuse tools is off: back to Select after each markup. Click to keep the tool active."}
      onclick={() => uiPrefs.setReuseTools(!uiPrefs.reuseTools)}
    >Reuse</button>
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

  /* No transform here: a mark-up layer's right-click menu is position: fixed, which a
     transformed ancestor would turn into page-relative. */
  .reading-page {
    position: absolute;
    background: #fff;
    box-shadow: 0 0 0 1px var(--color-border-subtle);
  }

  .reading-page-image {
    display: block;
    width: 100%;
    height: 100%;
  }

  .reading-page-markup {
    position: absolute;
    inset: 0;
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

  .btn-reuse.active { color: var(--color-text); }

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
