<script lang="ts">
  /**
   * PDF Viewport — tiled render pipeline (spec §5).
   *
   * Render flow (spec §5):
   *   1. Webview requests visible tiles at current zoom.
   *   2. Rust rasterizes via PDFium at exactly zoom × DPR — never upscaled.
   *   3. Tiles returned as PNG base64 are drawn onto a canvas.
   *   4. Placeholder (grey rect) shown immediately on zoom change; sharp tiles
   *      replace them as they arrive (≤16ms placeholder, ≤250ms sharp — §20).
   *
   * Markup overlay: SVG layer drawn on top of the canvas (M2).
   * Geometry / snapping: queries Rust snap-target index in PDF user space (M2).
   *
   * Svelte 5 runes throughout.
   */
  import { onMount, onDestroy } from "svelte";
  import {
    renderTile,
    getPageSize,
    processRssMb,
    type DocumentInfo,
    type RenderedTile,
  } from "$lib/ipc";
  import {
    TILE_SIZE_CSS,
    visibleTiles,
    type ViewportState,
  } from "$lib/viewport";

  // ---------------------------------------------------------------------------
  // Props
  // ---------------------------------------------------------------------------
  const { docInfo }: { docInfo: DocumentInfo } = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------
  let canvasEl = $state<HTMLCanvasElement | null>(null);
  let containerEl = $state<HTMLDivElement | null>(null);

  let zoom      = $state(1.0);
  let scrollX   = $state(0);
  let scrollY   = $state(0);
  let pageIndex = $state(0);

  let pageWidthPts  = $state(0);
  let pageHeightPts = $state(0);

  // Track in-flight tile renders to avoid duplicate requests
  const pendingTiles = new Set<string>();
  // Drawn tile image data keyed by "tx,ty,zoom_millis"
  const tileCache = new Map<string, HTMLImageElement>();

  let containerWidth  = $state(0);
  let containerHeight = $state(0);

  // Bench stats (surfaced in UI for M1 validation — §20)
  let lastTileMs = $state(0);
  let tileCount  = $state(0);

  // --- §20 GUI-only metrics overlay (toggle with the B key) ---
  // These are the §20 acceptance metrics the headless harness CANNOT measure:
  // interactive pan frame-time, zoom-settle, and live process RSS.
  let benchOverlay  = $state(false);
  let panFrameMs    = $state(0);   // last pan frame delta (ms)
  let panWorstMs    = $state(0);   // worst frame in the current pan gesture (ms)
  let panFps        = $state(0);   // smoothed FPS during pan
  let zoomSettleMs  = $state(0);   // last zoom → all-tiles-sharp settle time (ms)
  let rssMb         = $state(0);   // live process RSS (MB), polled

  let lastFrameTs   = 0;           // rAF timestamp of previous pan frame
  let zoomStartTs   = 0;           // performance.now() at last zoom change (0 = settled)
  let rssTimer: ReturnType<typeof setInterval> | null = null;

  // ---------------------------------------------------------------------------
  // Derived
  // ---------------------------------------------------------------------------
  const pageWidthPx  = $derived(pageWidthPts  * zoom);
  const pageHeightPx = $derived(pageHeightPts * zoom);

  const viewState = $derived<ViewportState>({
    canvasWidthCss:  containerWidth,
    canvasHeightCss: containerHeight,
    zoom,
    dpr: window.devicePixelRatio || 1,
    scrollX,
    scrollY,
    pageWidthPts,
    pageHeightPts,
  });

  // ---------------------------------------------------------------------------
  // Load page size on mount / docInfo change
  // ---------------------------------------------------------------------------
  async function loadPageSize() {
    if (!docInfo) return;
    try {
      const ps = await getPageSize(docInfo.doc_id, pageIndex);
      pageWidthPts  = ps.width_pts;
      pageHeightPts = ps.height_pts;
      // Reset scroll on page change
      scrollX = 0;
      scrollY = 0;
      requestTiles();
    } catch (e) {
      console.error("getPageSize failed:", e);
    }
  }

  // ---------------------------------------------------------------------------
  // Tile loading
  // ---------------------------------------------------------------------------
  function tileKey(tx: number, ty: number, zoomMillis: number): string {
    return `${tx},${ty},${zoomMillis}`;
  }

  function requestTiles() {
    if (!canvasEl || pageWidthPts === 0) return;

    const dpr = window.devicePixelRatio || 1;
    const tiles = visibleTiles(viewState);

    const ctx = canvasEl.getContext("2d");
    if (!ctx) return;

    // Draw placeholders immediately for tiles not yet cached.
    for (const { tx, ty } of tiles) {
      const zoomMillis = Math.round(zoom * dpr * 1000);
      const key = tileKey(tx, ty, zoomMillis);

      if (tileCache.has(key)) {
        // Already rendered — draw it.
        drawTile(ctx, tileCache.get(key)!, tx, ty);
      } else {
        // Draw placeholder immediately (≤16ms spec §20).
        drawPlaceholder(ctx, tx, ty);

        if (!pendingTiles.has(key)) {
          pendingTiles.add(key);
          fetchTile(tx, ty, zoomMillis, dpr, key);
        }
      }
    }
  }

  async function fetchTile(
    tx: number,
    ty: number,
    zoomMillis: number,
    dpr: number,
    key: string
  ) {
    try {
      const result: RenderedTile = await renderTile({
        doc_id: docInfo.doc_id,
        page_index: pageIndex,
        tile_size_css: TILE_SIZE_CSS,
        tile_x: tx,
        tile_y: ty,
        zoom,
        dpr,
      });

      lastTileMs = result.render_ms;
      tileCount += 1;

      // Decode PNG base64 → HTMLImageElement.
      const img = new Image();
      img.src = `data:image/png;base64,${result.png_base64}`;
      await new Promise<void>((resolve, reject) => {
        img.onload = () => resolve();
        img.onerror = reject;
      });

      tileCache.set(key, img);

      // Draw the sharp tile.
      const ctx = canvasEl?.getContext("2d");
      if (ctx) drawTile(ctx, img, tx, ty);
    } catch (e) {
      console.error(`Tile (${tx},${ty}) render failed:`, e);
    } finally {
      pendingTiles.delete(key);
      // After this tile resolves, if a zoom is pending and nothing is in flight,
      // the view has settled — record the zoom-settle time.
      checkZoomSettled();
    }
  }

  function drawTile(
    ctx: CanvasRenderingContext2D,
    img: HTMLImageElement,
    tx: number,
    ty: number
  ) {
    const screenX = tx * TILE_SIZE_CSS - scrollX;
    const screenY = ty * TILE_SIZE_CSS - scrollY;
    ctx.drawImage(img, screenX, screenY);
  }

  function drawPlaceholder(
    ctx: CanvasRenderingContext2D,
    tx: number,
    ty: number
  ) {
    const screenX = tx * TILE_SIZE_CSS - scrollX;
    const screenY = ty * TILE_SIZE_CSS - scrollY;
    const w = Math.min(TILE_SIZE_CSS, pageWidthPx - tx * TILE_SIZE_CSS);
    const h = Math.min(TILE_SIZE_CSS, pageHeightPx - ty * TILE_SIZE_CSS);
    ctx.fillStyle = "#2c2c2e";
    ctx.fillRect(screenX, screenY, w, h);
  }

  // ---------------------------------------------------------------------------
  // Resize observer
  // ---------------------------------------------------------------------------
  let resizeObserver: ResizeObserver | null = null;

  function onResize(entries: ResizeObserverEntry[]) {
    const entry = entries[0];
    if (!entry) return;
    containerWidth  = entry.contentRect.width;
    containerHeight = entry.contentRect.height;

    // Resize the canvas backing store.
    if (canvasEl) {
      const dpr = window.devicePixelRatio || 1;
      canvasEl.width  = containerWidth  * dpr;
      canvasEl.height = containerHeight * dpr;
      const ctx = canvasEl.getContext("2d");
      if (ctx) ctx.scale(dpr, dpr);
    }
    requestTiles();
  }

  // ---------------------------------------------------------------------------
  // Pan (mouse drag)
  // ---------------------------------------------------------------------------
  let dragging = false;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragScrollX0 = 0;
  let dragScrollY0 = 0;

  function onMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    dragging = true;
    dragStartX  = e.clientX;
    dragStartY  = e.clientY;
    dragScrollX0 = scrollX;
    dragScrollY0 = scrollY;
    // Reset per-gesture pan metrics.
    lastFrameTs = performance.now();
    panWorstMs = 0;
  }

  function onMouseMove(e: MouseEvent) {
    if (!dragging) return;
    // §20 pan frame-time: time between successive pan-driven redraws. Each
    // mousemove triggers a requestTiles() redraw, so the delta between moves is
    // the effective interactive frame interval.
    const now = performance.now();
    const dt = now - lastFrameTs;
    lastFrameTs = now;
    if (dt > 0 && dt < 1000) {
      panFrameMs = dt;
      if (dt > panWorstMs) panWorstMs = dt;
      // Smoothed FPS (EMA).
      const inst = 1000 / dt;
      panFps = panFps === 0 ? inst : panFps * 0.8 + inst * 0.2;
    }

    const dx = e.clientX - dragStartX;
    const dy = e.clientY - dragStartY;
    scrollX = Math.max(0, Math.min(pageWidthPx  - containerWidth,  dragScrollX0 - dx));
    scrollY = Math.max(0, Math.min(pageHeightPx - containerHeight, dragScrollY0 - dy));
    requestTiles();
  }

  function onMouseUp() { dragging = false; }

  // Toggle the §20 bench overlay with the "B" key.
  function onKeyDown(e: KeyboardEvent) {
    if (e.key === "b" || e.key === "B") {
      benchOverlay = !benchOverlay;
    }
  }

  /** Called after a tile batch may have settled — if a zoom is pending and no tiles
   *  remain in flight, record the zoom-settle time (§20 ≤250 ms target). */
  function checkZoomSettled() {
    if (zoomStartTs > 0 && pendingTiles.size === 0) {
      zoomSettleMs = Math.round(performance.now() - zoomStartTs);
      zoomStartTs = 0;
    }
  }

  // ---------------------------------------------------------------------------
  // Zoom (wheel)
  // ---------------------------------------------------------------------------
  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const delta = e.deltaY > 0 ? -0.1 : 0.1;
    const newZoom = Math.max(0.1, Math.min(8.0, zoom + delta));
    // §20 zoom-settle: mark the moment of the zoom change; checkZoomSettled()
    // records the elapsed once every visible tile at the new scale is sharp.
    zoomStartTs = performance.now();
    // Invalidate tile cache on zoom change — tiles must be re-rendered at new scale.
    tileCache.clear();
    pendingTiles.clear();
    zoom = newZoom;
    requestTiles();
  }

  // ---------------------------------------------------------------------------
  // Page navigation
  // ---------------------------------------------------------------------------
  function prevPage() {
    if (pageIndex > 0) {
      pageIndex -= 1;
      tileCache.clear();
      pendingTiles.clear();
      loadPageSize();
    }
  }
  function nextPage() {
    if (pageIndex < docInfo.page_count - 1) {
      pageIndex += 1;
      tileCache.clear();
      pendingTiles.clear();
      loadPageSize();
    }
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------
  onMount(() => {
    if (containerEl) {
      resizeObserver = new ResizeObserver(onResize);
      resizeObserver.observe(containerEl);
    }
    window.addEventListener("keydown", onKeyDown);
    // Poll process RSS once per second for the §20 overlay.
    rssTimer = setInterval(async () => {
      try {
        rssMb = await processRssMb();
      } catch {
        rssMb = 0;
      }
    }, 1000);
    loadPageSize();
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    window.removeEventListener("keydown", onKeyDown);
    if (rssTimer) clearInterval(rssTimer);
    tileCache.clear();
    pendingTiles.clear();
  });
</script>

<div
  class="viewport-root"
  bind:this={containerEl}
  onmousedown={onMouseDown}
  onmousemove={onMouseMove}
  onmouseup={onMouseUp}
  onmouseleave={onMouseUp}
  onwheel={onWheel}
  role="application"
  aria-label="PDF viewport — pan with drag, zoom with scroll"
>
  <!-- Tile canvas (Rust-rendered, display only) -->
  <canvas bind:this={canvasEl} class="tile-canvas"></canvas>

  <!-- Markup overlay — SVG, drawn on top of tiles (M2) -->
  <svg class="markup-overlay" aria-hidden="true">
    <!-- Markup paths rendered here in M2 -->
  </svg>

  <!-- Page navigation -->
  <nav class="page-nav">
    <button class="btn-nav" onclick={prevPage} disabled={pageIndex === 0}>‹</button>
    <span class="page-label">
      Page {pageIndex + 1} / {docInfo.page_count}
    </span>
    <button class="btn-nav" onclick={nextPage} disabled={pageIndex >= docInfo.page_count - 1}>›</button>
  </nav>

  <!-- Zoom indicator + bench stats (M1 validation) -->
  <div class="zoom-indicator">
    {Math.round(zoom * 100)}%
    {#if tileCount > 0}
      <span class="bench-stat">last tile: {lastTileMs}ms</span>
    {/if}
    <span class="bench-hint">[B] bench</span>
  </div>

  <!-- §20 bench/FPS overlay (toggle with B). Captures the GUI-only metrics:
       pan frame-time, zoom-settle, live RSS — mapped to §20 thresholds. -->
  {#if benchOverlay}
    <div class="bench-overlay">
      <div class="bench-title">§20 LIVE METRICS</div>
      <div class="bench-row">
        <span>pan frame</span>
        <span class={panFrameMs <= 33 ? "ok" : "warn"}>
          {panFrameMs.toFixed(1)} ms ({panFps.toFixed(0)} fps)
        </span>
      </div>
      <div class="bench-row">
        <span>pan worst</span>
        <span class={panWorstMs <= 33 ? "ok" : "warn"}>{panWorstMs.toFixed(1)} ms</span>
      </div>
      <div class="bench-row">
        <span>zoom settle</span>
        <span class={zoomSettleMs <= 250 ? "ok" : "warn"}>{zoomSettleMs} ms</span>
      </div>
      <div class="bench-row">
        <span>last tile</span>
        <span class={lastTileMs <= 60 ? "ok" : "warn"}>{lastTileMs} ms</span>
      </div>
      <div class="bench-row">
        <span>RSS</span>
        <span class={rssMb <= 2048 ? "ok" : "warn"}>{rssMb.toFixed(0)} MB</span>
      </div>
      <div class="bench-thresholds">
        targets: pan ≤33ms · settle ≤250ms · tile ≤60ms · RSS ≤2048MB
      </div>
    </div>
  {/if}
</div>

<style>
  .viewport-root {
    width: 100%;
    height: 100%;
    position: relative;
    overflow: hidden;
    cursor: grab;
    background: var(--color-bg);
  }
  .viewport-root:active { cursor: grabbing; }

  .tile-canvas {
    position: absolute;
    top: 0; left: 0;
    /* Canvas is sized programmatically; CSS size = container size */
    width: 100%;
    height: 100%;
    image-rendering: pixelated; /* prevent browser upscale blur */
  }

  .markup-overlay {
    position: absolute;
    top: 0; left: 0;
    width: 100%;
    height: 100%;
    pointer-events: none; /* pass mouse events through to the canvas */
  }

  /* --- Page navigation --- */
  .page-nav {
    position: absolute;
    bottom: var(--space-4);
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: var(--space-2);
    background: rgba(26, 26, 28, 0.85);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-lg);
    padding: var(--space-1) var(--space-3);
    backdrop-filter: blur(8px);
  }
  .btn-nav {
    background: none;
    border: none;
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-lg);
    padding: 0 var(--space-2);
    line-height: 1;
    transition: color 120ms;
  }
  .btn-nav:hover:not(:disabled) { color: var(--color-primary); }
  .btn-nav:disabled { color: var(--color-text-muted); cursor: not-allowed; }
  .page-label {
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
    min-width: 100px;
    text-align: center;
  }

  /* --- Zoom indicator / bench stats --- */
  .zoom-indicator {
    position: absolute;
    top: var(--space-3);
    right: var(--space-3);
    background: rgba(26, 26, 28, 0.75);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text-secondary);
    font-size: var(--font-size-xs);
    font-family: var(--font-mono);
    padding: var(--space-1) var(--space-2);
    display: flex;
    gap: var(--space-2);
    pointer-events: none;
  }
  .bench-stat { color: var(--color-text-muted); }
  .bench-hint { color: var(--color-text-muted); opacity: 0.6; }

  /* --- §20 live bench overlay --- */
  .bench-overlay {
    position: absolute;
    top: var(--space-3);
    left: var(--space-3);
    background: rgba(26, 26, 28, 0.9);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    color: var(--color-text-secondary);
    font-family: var(--font-mono);
    font-size: var(--font-size-xs);
    padding: var(--space-2) var(--space-3);
    min-width: 220px;
    pointer-events: none;
    backdrop-filter: blur(8px);
  }
  .bench-title {
    color: var(--color-text);
    font-weight: 600;
    margin-bottom: var(--space-1);
    letter-spacing: 0.06em;
  }
  .bench-row {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
    line-height: 1.5;
  }
  .bench-row .ok   { color: var(--color-success, #3ba55d); }
  .bench-row .warn { color: var(--color-warning, #e0a000); }
  .bench-thresholds {
    margin-top: var(--space-1);
    color: var(--color-text-muted);
    opacity: 0.7;
    font-size: 10px;
  }
</style>
