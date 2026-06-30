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
  import { onMount, onDestroy, untrack } from "svelte";
  import {
    renderTile,
    getPageSize,
    processRssMb,
    getUserIdentity,
    type DocumentInfo,
    type RenderedTile,
    type Markup,
    type UserRef,
  } from "$lib/ipc";
  import {
    TILE_SIZE_CSS,
    visibleTiles,
    screenToPdfUserSpace,
    wheelZoomFactor,
    fitWidthZoom,
    fitHeightZoom,
    ACTUAL_SIZE_ZOOM,
    type ViewportState,
    type ViewportSnapshot,
  } from "$lib/viewport";
  import {
    markupToSvg, selectionChrome, vertexChrome, isClosedMarkupType,
    type SvgShape, type SelectionChrome, type VertexChrome,
  } from "$lib/markup-render";
  import { MarkupStore } from "$lib/markup-store.svelte";
  import {
    hitTest, marqueeHits, boundsOf, isRectResizable,
    handleAnchors, resizeBounds, translateGeometry, scaleGeometryToBounds,
    expandSelectionToGroups, moveVertex, insertVertex, deleteVertex,
    type Bounds, type HandleId,
  } from "$lib/markup-select";
  import {
    dragDrawGeometry, buildMarkup, bumpAudit, isDrawTool,
    isMultiClickTool, isInkTool, polylineGeometry, inkGeometry,
    isMultiClickComplete, type MultiClickTool,
    isTextTool, textBoxGeometry, calloutGeometry, DEFAULT_TEXT_FONT,
  } from "$lib/markup-tools";
  import { patchGroup } from "$lib/markup-properties";
  import { TakeoffStore } from "$lib/takeoff-store.svelte";
  import { measureLength, measureArea } from "$lib/measurement-tools";
  import { addScale, type MeasurementPayload, type SearchHit } from "$lib/ipc";
  import { pdfUserSpaceToScreen } from "$lib/viewport";
  import CalibrationDialog from "./CalibrationDialog.svelte";

  // ---------------------------------------------------------------------------
  // Props
  // ---------------------------------------------------------------------------
  const {
    docInfo,
    store,
    takeoffStore = new TakeoffStore(),
    searchHits = [],
    activeSearchHitIdx = null,
    initialState = undefined,
    onviewportchange = undefined,
  }: {
    docInfo: DocumentInfo;
    store: MarkupStore;
    takeoffStore?: TakeoffStore;
    /** Search hits from SearchPanel.  Viewport renders highlight rects on the current page. */
    searchHits?: SearchHit[];
    /** Index into searchHits that is currently focused (rendered with a stronger highlight). */
    activeSearchHitIdx?: number | null;
    /**
     * Viewport state to restore on mount (zoom, pageIndex, scrollX, scrollY).
     * Used by tab switching to preserve per-document view position.
     */
    initialState?: ViewportSnapshot;
    /**
     * Called whenever zoom, pageIndex, scrollX, or scrollY changes.
     * App.svelte saves this into the tab's viewportSnapshot so it can be
     * restored when the user switches back to this tab.
     */
    onviewportchange?: (s: ViewportSnapshot) => void;
  } = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------
  let canvasEl = $state<HTMLCanvasElement | null>(null);
  let containerEl = $state<HTMLDivElement | null>(null);

  // Initialised from initialState when the tab is switched back to, so zoom/page/scroll
  // are restored. Default to 1/0/0/0 when opening fresh.
  let zoom      = $state(initialState?.zoom      ?? 1.0);
  let scrollX   = $state(initialState?.scrollX   ?? 0);
  let scrollY   = $state(initialState?.scrollY   ?? 0);
  let pageIndex = $state(initialState?.pageIndex ?? 0);

  // Guard against loadPageSize() resetting scroll to 0 on the very first load
  // when an initialState scroll position should be preserved.
  let _firstPageLoad = true;

  let pageWidthPts  = $state(0);
  let pageHeightPts = $state(0);

  // Track in-flight tile renders to avoid duplicate requests
  const pendingTiles = new Set<string>();
  // Drawn tile image data keyed by "page,tx,ty,zoom_millis"
  const tileCache = new Map<string, HTMLImageElement>();
  // Bumped whenever the view is invalidated (zoom/page change). A fetchTile that started
  // under an older epoch discards its paint on arrival, killing stale-tile races.
  let renderEpoch = 0;

  // Smooth-zoom: during a wheel/keyboard zoom gesture the canvas is CSS-scaled as an instant
  // (blurry) placeholder and the expensive sharp re-render is debounced to gesture-settle
  // (spec §20: placeholder immediately, sharp tiles settle after). lastRender* track the
  // zoom/scroll the canvas bitmap was last sharply rendered at, so the placeholder transform
  // can map it to the live zoom/scroll.
  let lastRenderZoom = 1;
  let lastRenderScrollX = 0;
  let lastRenderScrollY = 0;
  let zoomSettleTimer: ReturnType<typeof setTimeout> | null = null;

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

  // --- Draw gesture state ---
  let identity = $state<UserRef | null>(null);
  let identityError = $state(false);
  let drawing = $state(false);
  let drawStartPdf: { x: number; y: number } | null = null;
  let previewMarkup = $state<Markup | null>(null);

  // --- Multi-click state ---
  let mcVerts = $state<{ x: number; y: number }[]>([]);
  let mcCursor = $state<{ x: number; y: number } | null>(null);

  // --- Ink freehand state ---
  let inkStroke = $state<{ x: number; y: number }[]>([]);

  // --- Inline text editor (Text/Callout) ---
  // editor carries placement info; editorText is a separate $state so bind:value
  // never reaches into a potentially-null object (avoids a Svelte runtime crash when
  // commitEditor sets editor=null while the textarea binding is still live).
  let editor = $state<{
    screenX: number; screenY: number;
    anchorPdf: { x: number; y: number };
    leaderPdf: { x: number; y: number } | null;
  } | null>(null);
  let editorText = $state("");
  let calloutTarget: { x: number; y: number } | null = null; // first Callout click (leader start)

  // --- Select tool gesture state ---
  /** Tolerance in screen pixels: pointer must be within this many px of a markup to hit it. */
  const SELECT_GRAB_PX = 6;

  /** Marquee drag rectangle in screen pixels (null when not dragging). */
  let marquee = $state<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
  /** Whether the marquee was started with Shift held (additive selection). */
  let marqueeAdditive = false;

  // --- Move / resize gesture state ---
  /** Minimum width/height (PDF pts) a rect can be resized to. */
  const MIN_RESIZE_PTS = 4;
  /** Grab radius (screen px) around a handle centre. */
  const HANDLE_GRAB_PX = 8;
  /** Active transform gesture on the selection. */
  let gesture: "none" | "move" | "resize" | "vertex" = "none";
  let moveStartPdf: { x: number; y: number } | null = null;
  let moveOrigins: Markup[] = [];               // committed selected markups at gesture start
  let resizeHandle: HandleId | null = null;
  let resizeOrig: Markup | null = null;         // the single markup being resized
  let resizeOrigBounds: Bounds | null = null;
  /** Live transformed clones rendered in place of the committed markups during a drag. */
  let dragPreview = $state<Markup[] | null>(null);

  // --- Per-vertex editing gesture state (single multipoint markup) ---
  /** The committed markup the vertex edit will undo back to (the `before` of the commit). */
  let vertexBefore: Markup | null = null;
  /** The markup whose vertex is being dragged. For an insert this carries the freshly
   *  inserted vertex; for a plain drag it equals vertexBefore. */
  let vertexWorking: Markup | null = null;
  /** Index (into the working markup's Polyline) of the vertex being dragged. */
  let vertexIndex: number | null = null;
  /** Vertex eligible for keyboard delete (Delete/Backspace) — set when a vertex is engaged. */
  let activeVertex = $state<number | null>(null);

  /** True when any creation tool is active (all tools except hand/select). */
  const isCreateTool = (t = store.activeTool) =>
    isDrawTool(t) || isMultiClickTool(t) || isInkTool(t) || isTextTool(t) ||
    t === "calibrate" || t === "MeasurementLength" || t === "MeasurementArea" || t === "MeasurementCount";

  /** True when the select tool is active. */
  const isSelectTool = (t = store.activeTool): boolean => t === "select";

  /** Overlay captures pointer events for create tools OR select tool. */
  const overlayActive = $derived(isCreateTool() || isSelectTool());

  // --- Calibration dialog state ---
  let showCalibDialog = $state(false);
  let calibDialogDist = $state(0);

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

  // Notify App.svelte whenever the user changes zoom/page/scroll so the
  // per-tab snapshot stays current for tab switching.
  $effect(() => {
    onviewportchange?.({ zoom, pageIndex, scrollX, scrollY });
  });

  // Markups on the current page, mapped to screen-space SVG descriptors.
  // Reactive to viewState (zoom/pan/resize) AND pageIndex AND store.markups,
  // so the overlay stays glued to the page with no manual redraw.
  // During a move/resize drag, the selected markups render at their previewed
  // positions (dragPreview) instead of their committed ones, glued to viewState.
  const pageShapes = $derived.by<SvgShape[]>(() => {
    const previewIds = new Set(dragPreview?.map((m) => m.id) ?? []);
    const committed = store.markups
      .filter((m) => m.page === pageIndex && !previewIds.has(m.id))
      .map((m) => markupToSvg(m, viewState));
    const preview = (dragPreview ?? [])
      .filter((m) => m.page === pageIndex)
      .map((m) => markupToSvg(m, viewState));
    return [...committed, ...preview];
  });

  // Preview shape for the live draw gesture (not committed until pointerup).
  const previewShape = $derived<SvgShape | null>(
    previewMarkup ? markupToSvg(previewMarkup, viewState) : null,
  );

  // Markups on the current page that are selected.
  const selectedOnPage = $derived(
    store.selectedMarkups.filter((m) => m.page === pageIndex),
  );

  // Union bounds of the selection on this page (null when none). Tracks the live
  // dragPreview during a move/resize so the chrome + handles follow the gesture.
  const selectionBounds = $derived.by<Bounds | null>(() => {
    const src = dragPreview ?? selectedOnPage;
    if (src.length === 0) return null;
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const m of src) {
      const b = boundsOf(m);
      if (b.minX < minX) minX = b.minX;
      if (b.minY < minY) minY = b.minY;
      if (b.maxX > maxX) maxX = b.maxX;
      if (b.maxY > maxY) maxY = b.maxY;
    }
    return { minX, minY, maxX, maxY };
  });

  // Show resize handles only when exactly one Rect markup is selected.
  const showHandles = $derived(
    selectedOnPage.length === 1 && isRectResizable(selectedOnPage[0]),
  );

  // Screen-space chrome for the selection overlay.
  const chrome = $derived<SelectionChrome | null>(
    selectionBounds ? selectionChrome(selectionBounds, viewState, showHandles) : null,
  );

  // The single selected multipoint markup (Polyline geometry, vertex-editable). Tracks the
  // live dragPreview so handles follow a vertex drag. Callout's leader Polyline is excluded
  // (its 2-point geometry is a leader, not an editable path). null when not exactly one such.
  const singleMultipoint = $derived.by<Markup | null>(() => {
    const src = dragPreview ?? selectedOnPage;
    if (src.length !== 1) return null;
    const m = src[0];
    return "Polyline" in m.geometry && m.markup_type !== "Callout" ? m : null;
  });

  // Screen-space per-vertex + midpoint handles for the single multipoint markup.
  const vertexHandles = $derived.by<VertexChrome | null>(() => {
    const m = singleMultipoint;
    if (!m || !("Polyline" in m.geometry)) return null;
    return vertexChrome(m.geometry.Polyline, viewState, isClosedMarkupType(m.markup_type));
  });

  /** Min point-count floor for vertex deletion (closed shapes keep ≥3, open keep ≥2). */
  const vertexFloor = (m: Markup): number => (isClosedMarkupType(m.markup_type) ? 3 : 2);

  // Search hit highlight rects for the current page in screen space.
  // Uses the same §5 pdfUserSpaceToScreen transform as markups so highlights
  // stay pixel-accurate at any zoom level.
  const pageSearchHits = $derived(
    searchHits
      .map((h, idx) => ({ hit: h, idx }))
      .filter(({ hit }) => hit.page === pageIndex)
      .map(({ hit, idx }) => {
        const [left, bottom, right, top] = hit.rect;
        const tl = pdfUserSpaceToScreen(left, top, viewState);
        const br = pdfUserSpaceToScreen(right, bottom, viewState);
        return {
          idx,
          x: tl.x,
          y: tl.y,
          width: Math.max(1, br.x - tl.x),
          height: Math.max(1, br.y - tl.y),
          active: idx === activeSearchHitIdx,
        };
      })
  );

  // ---------------------------------------------------------------------------
  // Load page size on mount / docInfo change
  // ---------------------------------------------------------------------------
  async function loadPageSize() {
    if (!docInfo) return;
    try {
      const ps = await getPageSize(docInfo.doc_id, pageIndex);
      pageWidthPts  = ps.width_pts;
      pageHeightPts = ps.height_pts;
      // Reset scroll on page change — but preserve initialState.scrollX/Y on the
      // very first load so tab-switching restores the scroll position correctly.
      if (_firstPageLoad) {
        _firstPageLoad = false;
      } else {
        scrollX = 0;
        scrollY = 0;
      }
      requestTiles();
    } catch (e) {
      console.error("getPageSize failed:", e);
    }
  }

  // ---------------------------------------------------------------------------
  // Tile loading
  // ---------------------------------------------------------------------------
  function tileKey(tx: number, ty: number, zoomMillis: number): string {
    return `${pageIndex},${tx},${ty},${zoomMillis}`;
  }

  function requestTiles() {
    if (!canvasEl || pageWidthPts === 0) return;

    const dpr = window.devicePixelRatio || 1;
    const tiles = visibleTiles(viewState);

    let ctx: CanvasRenderingContext2D | null = null;
    try {
      ctx = canvasEl.getContext("2d");
    } catch {
      return; // no 2D canvas (e.g. jsdom in tests)
    }
    if (!ctx) return;

    // Rendering natively at the current zoom/scroll — drop any zoom placeholder transform
    // and record this as the last sharp render (the basis for the next placeholder).
    canvasEl.style.transform = "none";
    lastRenderZoom = zoom;
    lastRenderScrollX = scrollX;
    lastRenderScrollY = scrollY;

    // Clear the whole backing store (device px) before redrawing so the previous frame's
    // tiles don't ghost when the page shrinks (zoom-out), on pan, or on page-switch.
    ctx.clearRect(0, 0, canvasEl.width, canvasEl.height);

    const epoch = renderEpoch;

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
          fetchTile(tx, ty, zoomMillis, dpr, key, epoch);
        }
      }
    }
  }

  async function fetchTile(
    tx: number,
    ty: number,
    zoomMillis: number,
    dpr: number,
    key: string,
    epoch: number
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

      // Only paint if the view hasn't changed since this fetch began — otherwise a slow
      // tile from a previous page/zoom would land on the current view (page-switch race /
      // zoom ghosting). Pan is exempt: drawTile reads live scroll, so it stays aligned.
      const ctx = canvasEl?.getContext("2d");
      if (ctx && epoch === renderEpoch) drawTile(ctx, img, tx, ty);
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
    // Draw in DEVICE pixels at the bitmap's native size and an integer origin. Because the
    // tile stride (TILE_SIZE_CSS × dpr) is an integer, rounding each tile's origin makes
    // adjacent tiles abut exactly — no sub-pixel seam between blocks. (Backing store is
    // container × dpr; the ctx is NOT dpr-scaled.)
    const dpr = window.devicePixelRatio || 1;
    const dx = Math.round((tx * TILE_SIZE_CSS - scrollX) * dpr);
    const dy = Math.round((ty * TILE_SIZE_CSS - scrollY) * dpr);
    ctx.drawImage(img, dx, dy);
  }

  function drawPlaceholder(
    ctx: CanvasRenderingContext2D,
    tx: number,
    ty: number
  ) {
    // Device-px, matching drawTile so placeholders tile seamlessly under the sharp tiles.
    const dpr = window.devicePixelRatio || 1;
    const dx = Math.round((tx * TILE_SIZE_CSS - scrollX) * dpr);
    const dy = Math.round((ty * TILE_SIZE_CSS - scrollY) * dpr);
    const w = Math.min(TILE_SIZE_CSS, pageWidthPx - tx * TILE_SIZE_CSS) * dpr;
    const h = Math.min(TILE_SIZE_CSS, pageHeightPx - ty * TILE_SIZE_CSS) * dpr;
    ctx.fillStyle = "#2c2c2e";
    ctx.fillRect(dx, dy, Math.ceil(w), Math.ceil(h));
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

    // Resize the canvas backing store (device px). The 2D context is intentionally NOT
    // dpr-scaled — tiles are drawn in device pixels at integer positions (see drawTile) so
    // adjacent tiles abut seamlessly (no sub-pixel join line between blocks).
    if (canvasEl) {
      const dpr = window.devicePixelRatio || 1;
      canvasEl.width  = containerWidth  * dpr;
      canvasEl.height = containerHeight * dpr;
    }
    requestTiles();
  }

  // ---------------------------------------------------------------------------
  // Pan (mouse drag) — only active when no draw tool is capturing
  // ---------------------------------------------------------------------------
  let dragging = false;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragScrollX0 = 0;
  let dragScrollY0 = 0;

  function onMouseDown(e: MouseEvent) {
    // Don't start pan when any creation tool or select tool is active (overlay captures those events)
    if (isCreateTool() || isSelectTool()) return;
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

  // Keyboard handler: bench overlay toggle + multi-click finish/cancel.
  function onKeyDown(e: KeyboardEvent) {
    if (e.key === "b" || e.key === "B") {
      benchOverlay = !benchOverlay;
    }
    // Keyboard zoom (Cmd/Ctrl + = / -), anchored to the viewport centre, plus zoom-snap
    // presets and page navigation. Guard all of these when the text editor is active
    // so Arrow keys don't hijack navigation while the user is typing.
    if (e.metaKey || e.ctrlKey) {
      if (editor) return; // let textarea handle Cmd/Ctrl inside the editor
      if (e.key === "=" || e.key === "+") { e.preventDefault(); applyZoom(zoom * 1.1, containerWidth / 2, containerHeight / 2); return; }
      if (e.key === "-" || e.key === "_") { e.preventDefault(); applyZoom(zoom / 1.1, containerWidth / 2, containerHeight / 2); return; }
      // Fit-width: Cmd/Ctrl+1 (legacy) or Cmd/Ctrl+0
      if (e.key === "1") { e.preventDefault(); fitWidth(); return; }
      if (!e.shiftKey && e.key === "0") { e.preventDefault(); fitWidth(); return; }
      // Fit-height: Cmd/Ctrl+2 (legacy) or Cmd/Ctrl+9
      if (e.key === "2") { e.preventDefault(); fitHeight(); return; }
      if (e.key === "9") { e.preventDefault(); fitHeight(); return; }
      // Actual size (100%): Cmd/Ctrl+Shift+0
      if (e.shiftKey && (e.key === "0" || e.key === ")")) { e.preventDefault(); actualSize(); return; }
      // Page navigation: Cmd/Ctrl+ArrowLeft / ArrowRight
      if (e.key === "ArrowLeft")  { e.preventDefault(); prevPage(); return; }
      if (e.key === "ArrowRight") { e.preventDefault(); nextPage(); return; }
    }
    // Don't fire the shortcuts below when the text editor is open — the textarea
    // handles its own keys (typing must not be hijacked).
    if (editor) return;
    // V — jump straight to the select / pointer tool (no modifier; Cmd/Ctrl+V is paste).
    if (!e.metaKey && !e.ctrlKey && !e.altKey && (e.key === "v" || e.key === "V")) {
      e.preventDefault();
      store.activeTool = "select";
      return;
    }
    // Delete / Backspace: remove the active vertex of a single multipoint markup if one is
    // engaged; otherwise remove the whole selection (one undo frame).
    if ((e.key === "Delete" || e.key === "Backspace") && store.selectedIds.size > 0) {
      e.preventDefault();
      const mp = singleMultipoint;
      if (mp && activeVertex !== null) {
        const after = deleteVertex(mp.geometry, activeVertex, vertexFloor(mp));
        if (JSON.stringify(after) !== JSON.stringify(mp.geometry)) {
          const now = new Date().toISOString();
          store.applyBatch([{ before: mp, after: bumpAudit({ ...mp, geometry: after }, identity ?? mp.audit.modified_by, now) }]);
        }
        activeVertex = null; // index may have shifted; require re-selecting a vertex
        return;
      }
      store.deleteSelected();
      return;
    }
    // Cmd/Ctrl+G: group selected markups (≥2, select tool, identity loaded).
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && e.key === "g" && isSelectTool() && identity) {
      e.preventDefault();
      const targets = store.selectedMarkups;
      if (targets.length >= 2) {
        const gid = crypto.randomUUID();
        const now = new Date().toISOString();
        const pairs = targets.map((m) => ({ before: m, after: patchGroup(m, gid, identity!, now) }));
        store.applyBatch(pairs);
      }
      return;
    }
    // Cmd/Ctrl+Shift+G: ungroup selected markups that belong to a group.
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && (e.key === "g" || e.key === "G") && isSelectTool() && identity) {
      e.preventDefault();
      const targets = store.selectedMarkups.filter((m) => m.group_id !== null);
      if (targets.length > 0) {
        const now = new Date().toISOString();
        const pairs = targets.map((m) => ({ before: m, after: patchGroup(m, null, identity!, now) }));
        store.applyBatch(pairs);
      }
      return;
    }
    if (e.key === "Enter") {
      // MeasurementArea finish via Enter.
      if (store.activeTool === "MeasurementArea" && identity && mcVerts.length >= 3) {
        const raw = measureArea(mcVerts);
        const scale = takeoffStore.activeScale;
        const meas: MeasurementPayload = {
          scale_ref: scale?.id ?? null,
          raw_measure: raw,
          unit: scale?.unit ?? "pt²",
          computed_quantity: scale ? raw * scale.ratio * scale.ratio : 0,
          depth: null,
          count_value: null,
          custom_columns: {},
        };
        const m = buildMarkup({
          markupType: "MeasurementArea",
          page: pageIndex,
          geometry: polylineGeometry(mcVerts),
          appearance: store.draftAppearance,
          identity,
          now: new Date().toISOString(),
          id: crypto.randomUUID(),
        });
        m.measurement = meas;
        store.create(m);
        resetMultiClick();
      } else {
        finishMultiClick();
      }
    }
    if (e.key === "Escape") {
      resetMultiClick();
      cancelDraw();
      store.selectedIds = new Set();
      activeVertex = null;
      // Cancel in-progress calibration.
      if (takeoffStore.calibrationState) {
        takeoffStore.cancelCalibration();
        showCalibDialog = false;
      }
      // After cancelling any in-progress gesture, fall back to the select / pointer tool.
      store.activeTool = "select";
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
  // Zoom (wheel + keyboard) — multiplicative, anchored to a screen point
  // ---------------------------------------------------------------------------
  const ZOOM_MIN = 0.1;
  const ZOOM_MAX = 8.0;

  /** Clamp scroll so the page can't be pushed past its bounds (pins to 0 when smaller). */
  function clampScroll() {
    scrollX = Math.max(0, Math.min(scrollX, Math.max(0, pageWidthPx - containerWidth)));
    scrollY = Math.max(0, Math.min(scrollY, Math.max(0, pageHeightPx - containerHeight)));
  }

  /** Drop cached/in-flight tiles and bump the render epoch so stale async tiles from the
   *  previous zoom/page are discarded on arrival (see fetchTile). */
  function invalidateTiles() {
    tileCache.clear();
    pendingTiles.clear();
    renderEpoch++;
  }

  /** Zoom to `newZoom`, keeping the page point under (anchorX, anchorY) screen px fixed. */
  function applyZoom(newZoom: number, anchorX: number, anchorY: number) {
    newZoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, newZoom));
    if (newZoom === zoom) return;
    const ratio = newZoom / zoom;
    // Keep the anchor point stationary: new scroll = (scroll + anchor)·ratio − anchor.
    scrollX = (scrollX + anchorX) * ratio - anchorX;
    scrollY = (scrollY + anchorY) * ratio - anchorY;
    zoom = newZoom;
    clampScroll();
    // §20 zoom-settle: mark the moment of the zoom change; checkZoomSettled() records the
    // elapsed once every visible tile at the new scale is sharp.
    zoomStartTs = performance.now();
    // Instant blurry feedback now; the sharp re-render is coalesced to gesture-settle so a
    // fast flick doesn't re-rasterise every tile on every event (the high-zoom slowdown).
    applyZoomPlaceholder();
    scheduleZoomCommit();
  }

  /** CSS-scale the last sharp canvas bitmap to approximate the live zoom/scroll instantly. */
  function applyZoomPlaceholder() {
    if (!canvasEl) return;
    const s = zoom / lastRenderZoom;
    const tx = s * lastRenderScrollX - scrollX;
    const ty = s * lastRenderScrollY - scrollY;
    canvasEl.style.transformOrigin = "0 0";
    canvasEl.style.transform = `translate(${tx}px, ${ty}px) scale(${s})`;
  }

  /** Debounce the expensive sharp re-render until the zoom gesture pauses (~120 ms). */
  function scheduleZoomCommit() {
    if (zoomSettleTimer) clearTimeout(zoomSettleTimer);
    zoomSettleTimer = setTimeout(() => {
      zoomSettleTimer = null;
      invalidateTiles(); // requestTiles (next) resets the transform + lastRender*
      requestTiles();
    }, 120);
  }

  // ---------------------------------------------------------------------------
  // Zoom-snap presets — Fit-Width / Fit-Height / Actual-Size (100%)
  // ---------------------------------------------------------------------------
  /**
   * Snap to an absolute zoom level and reposition scroll deterministically for the
   * given mode. Unlike applyZoom (which keeps a screen point fixed under the cursor),
   * a snap repositions: fit-width → top-left, fit-height/actual → horizontally centred.
   * Reuses the same placeholder + debounced sharp re-render path as wheel/keyboard zoom.
   */
  function applySnapZoom(newZoom: number, mode: "width" | "height" | "actual") {
    zoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, newZoom));
    const pageW = pageWidthPts * zoom;
    const pageH = pageHeightPts * zoom;
    const centreX = Math.max(0, (pageW - containerWidth) / 2);
    scrollX = mode === "width" ? 0 : centreX;
    scrollY = mode === "actual" ? Math.max(0, (pageH - containerHeight) / 2) : 0;
    clampScroll();
    zoomStartTs = performance.now();
    applyZoomPlaceholder();
    scheduleZoomCommit();
  }

  /** Fit the page width to the viewport (Cmd/Ctrl+1). */
  function fitWidth() {
    if (pageWidthPts <= 0) return;
    applySnapZoom(fitWidthZoom(pageWidthPts, containerWidth), "width");
  }
  /** Fit the page height to the viewport (Cmd/Ctrl+2). */
  function fitHeight() {
    if (pageHeightPts <= 0) return;
    applySnapZoom(fitHeightZoom(pageHeightPts, containerHeight), "height");
  }
  /** Snap to 1:1 / 100% (Cmd/Ctrl+0). */
  function actualSize() {
    applySnapZoom(ACTUAL_SIZE_ZOOM, "actual");
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    if (!containerEl) return;
    const r = containerEl.getBoundingClientRect();
    // Zoom step proportional to the wheel delta (see wheelZoomFactor) so a fast flick can't
    // rocket to the max in a few events.
    applyZoom(zoom * wheelZoomFactor(e.deltaY), e.clientX - r.left, e.clientY - r.top);
  }

  // ---------------------------------------------------------------------------
  // Page navigation
  // ---------------------------------------------------------------------------
  function prevPage() {
    if (pageIndex > 0) {
      pageIndex -= 1;
      invalidateTiles();
      loadPageSize();
    }
  }
  function nextPage() {
    if (pageIndex < docInfo.page_count - 1) {
      pageIndex += 1;
      invalidateTiles();
      loadPageSize();
    }
  }

  // ---------------------------------------------------------------------------
  // Draw gesture — overlay pointer capture (drag-draw tools)
  // ---------------------------------------------------------------------------

  /** Shared screen-to-PDF conversion from any clientX/Y (pointer or mouse). */
  function clientToPdf(clientX: number, clientY: number): { x: number; y: number } | null {
    if (!containerEl) return null;
    const r = containerEl.getBoundingClientRect();
    return screenToPdfUserSpace(clientX - r.left, clientY - r.top, viewState);
  }

  /** Convert a pointer event to container-local PDF user space. */
  function localPdf(e: PointerEvent): { x: number; y: number } | null {
    return clientToPdf(e.clientX, e.clientY);
  }

  /** Convert a mouse event to container-local PDF user space (for click/dblclick). */
  function localPdfFromMouse(e: MouseEvent): { x: number; y: number } | null {
    return clientToPdf(e.clientX, e.clientY);
  }

  /** Reset all draw state — used by pointerup teardown and pointercancel. */
  function cancelDraw() {
    drawing = false;
    drawStartPdf = null;
    previewMarkup = null;
    inkStroke = [];
  }

  function resetMultiClick() {
    mcVerts = [];
    mcCursor = null;
    previewMarkup = null;
  }

  function cancelEditor() {
    editor = null;
    editorText = "";
    calloutTarget = null;
  }

  function commitEditor() {
    if (!editor || !identity) { cancelEditor(); return; }
    const text = editorText.trim();
    if (!text) { cancelEditor(); return; }
    // Capture all state before canceling (cancelEditor clears editor + editorText).
    const { anchorPdf, leaderPdf } = editor;
    const appearance = { ...store.draftAppearance, font: store.draftAppearance.font ?? DEFAULT_TEXT_FONT };
    let markupType: "Text" | "Callout";
    let geometry;
    if (leaderPdf) {
      markupType = "Callout";
      geometry = calloutGeometry(leaderPdf, anchorPdf);
    } else {
      markupType = "Text";
      geometry = textBoxGeometry(anchorPdf);
    }
    cancelEditor();
    store.create(buildMarkup({
      markupType,
      page: pageIndex,
      geometry,
      appearance,
      identity,
      now: new Date().toISOString(),
      id: crypto.randomUUID(),
      contents: text,
    }));
  }

  function finishMultiClick() {
    const tool = store.activeTool;
    if (!isMultiClickTool(tool) || !identity) return;
    if (!isMultiClickComplete(tool as MultiClickTool, mcVerts)) { resetMultiClick(); return; }
    store.create(buildMarkup({
      markupType: tool as MultiClickTool,
      page: pageIndex,
      geometry: polylineGeometry(mcVerts),
      appearance: store.draftAppearance,
      identity,
      now: new Date().toISOString(),
      id: crypto.randomUUID(),
    }));
    resetMultiClick();
  }

  // Switching tools mid-gesture must not leave dangling in-progress state
  // (orphan verts, a half-drawn rect, or an in-flight ink stroke). Reset all
  // gesture state whenever the active tool changes. All resets are idempotent,
  // so the initial "hand" run is a harmless no-op.
  $effect(() => {
    const tool = store.activeTool; // reactive dependency
    resetMultiClick();
    cancelDraw();
    // Commit any in-progress text/callout annotation before the tool switch so the
    // user's work is not silently discarded. commitEditor() is a no-op when the editor
    // is empty (it falls through to cancelEditor internally).
    // untrack() prevents editor/identity/editorText from being registered as reactive
    // dependencies of this $effect (they're read inside commitEditor). Without it, the
    // effect re-runs when e.g. identity loads and would prematurely cancel a just-opened editor.
    untrack(() => commitEditor());
    // Clear marquee + move/resize/vertex transient state on any tool switch.
    marquee = null;
    marqueeAdditive = false;
    activeVertex = null;
    resetGesture();
    // Clear selection when switching AWAY from the select tool.
    if (!isSelectTool(tool)) {
      store.selectedIds = new Set();
    }
  });

  /** The resize handle (if any) whose screen anchor is within HANDLE_GRAB_PX of (sx, sy). */
  function handleAtScreen(sx: number, sy: number): HandleId | null {
    if (!chrome) return null;
    for (const h of chrome.handles) {
      if (Math.abs(h.x - sx) <= HANDLE_GRAB_PX && Math.abs(h.y - sy) <= HANDLE_GRAB_PX) return h.id;
    }
    return null;
  }

  /** The vertex index (if any) whose screen handle is within HANDLE_GRAB_PX of (sx, sy). */
  function vertexAtScreen(sx: number, sy: number): number | null {
    if (!vertexHandles) return null;
    for (const v of vertexHandles.vertices) {
      if (Math.abs(v.x - sx) <= HANDLE_GRAB_PX && Math.abs(v.y - sy) <= HANDLE_GRAB_PX) return v.index;
    }
    return null;
  }

  /** The segment index (if any) whose midpoint handle is within HANDLE_GRAB_PX of (sx, sy). */
  function midpointAtScreen(sx: number, sy: number): number | null {
    if (!vertexHandles) return null;
    for (const mp of vertexHandles.midpoints) {
      if (Math.abs(mp.x - sx) <= HANDLE_GRAB_PX && Math.abs(mp.y - sy) <= HANDLE_GRAB_PX) return mp.segmentIndex;
    }
    return null;
  }

  /** Clear all move/resize/vertex transient state (does not touch the committed selection). */
  function resetGesture() {
    gesture = "none";
    moveStartPdf = null;
    moveOrigins = [];
    resizeHandle = null;
    resizeOrig = null;
    resizeOrigBounds = null;
    vertexBefore = null;
    vertexWorking = null;
    vertexIndex = null;
    dragPreview = null;
  }

  function onOverlayPointerDown(e: PointerEvent) {
    const tool = store.activeTool;

    // --- SELECT tool branch ---
    if (isSelectTool(tool)) {
      const p = localPdf(e);
      if (!p) return;
      const r = containerEl?.getBoundingClientRect();
      const sx = r ? e.clientX - r.left : e.clientX;
      const sy = r ? e.clientY - r.top : e.clientY;

      // Any select pointerdown clears the keyboard-delete vertex unless a vertex is engaged below.
      activeVertex = null;

      // 1. Resize: pointerdown on a handle of a single resizable selection.
      const handle = showHandles ? handleAtScreen(sx, sy) : null;
      if (handle && selectedOnPage.length === 1) {
        gesture = "resize";
        resizeHandle = handle;
        resizeOrig = selectedOnPage[0];
        resizeOrigBounds = boundsOf(resizeOrig);
        (e.target as Element).setPointerCapture(e.pointerId);
        e.stopPropagation();
        e.preventDefault();
        return;
      }

      // 1b. Vertex editing: when a single multipoint markup is selected, its vertex and
      //     midpoint handles take priority over markup hit-testing.
      const mp = singleMultipoint;
      if (mp) {
        const vi = vertexAtScreen(sx, sy);
        if (vi !== null) {
          if (e.altKey) {
            // Alt-click deletes the vertex (no-op at the min-points floor).
            const after = deleteVertex(mp.geometry, vi, vertexFloor(mp));
            if (JSON.stringify(after) !== JSON.stringify(mp.geometry)) {
              const now = new Date().toISOString();
              store.applyBatch([{ before: mp, after: bumpAudit({ ...mp, geometry: after }, identity ?? mp.audit.modified_by, now) }]);
            }
          } else {
            // Begin dragging this existing vertex.
            gesture = "vertex";
            vertexBefore = mp;
            vertexWorking = mp;
            vertexIndex = vi;
            activeVertex = vi;
            (e.target as Element).setPointerCapture(e.pointerId);
          }
          e.stopPropagation();
          e.preventDefault();
          return;
        }
        const seg = midpointAtScreen(sx, sy);
        if (seg !== null) {
          // Insert a new vertex on this segment, then drag it (one undo frame on release).
          gesture = "vertex";
          vertexBefore = mp;
          vertexWorking = { ...mp, geometry: insertVertex(mp.geometry, seg, p) };
          vertexIndex = seg + 1;
          activeVertex = seg + 1;
          dragPreview = [vertexWorking];
          (e.target as Element).setPointerCapture(e.pointerId);
          e.stopPropagation();
          e.preventDefault();
          return;
        }
      }

      // 2. Hit-test markups on this page (topmost wins).
      const pageMarkups = store.markups.filter((m) => m.page === pageIndex);
      const hit = hitTest(pageMarkups, p, SELECT_GRAB_PX / zoom);
      if (hit !== null) {
        if (e.shiftKey) {
          // Shift-click toggles the whole group; no move.
          const grp = expandSelectionToGroups(store.markups, new Set([hit]));
          const next = new Set(store.selectedIds);
          if (next.has(hit)) {
            for (const id of grp) next.delete(id);
          } else {
            for (const id of grp) next.add(id);
          }
          store.selectedIds = next;
        } else {
          // Select the hit markup's whole group (if not already), then start a move.
          const grp = expandSelectionToGroups(store.markups, new Set([hit]));
          if (!store.selectedIds.has(hit)) store.selectedIds = grp;
          gesture = "move";
          moveStartPdf = p;
          moveOrigins = store.selectedMarkups.filter((m) => m.page === pageIndex);
        }
        (e.target as Element).setPointerCapture(e.pointerId);
      } else {
        // 3. Miss: begin marquee selection.
        if (!e.shiftKey) store.selectedIds = new Set();
        marqueeAdditive = e.shiftKey;
        marquee = { x0: sx, y0: sy, x1: sx, y1: sy };
        (e.target as Element).setPointerCapture(e.pointerId);
      }
      e.stopPropagation();
      e.preventDefault();
      return;
    }

    // --- CALIBRATE tool branch ---
    if (tool === "calibrate") {
      const p = localPdf(e);
      if (!p) return;
      const step = takeoffStore.calibrationState?.step;
      if (!step) {
        // First pointer-down: start calibration if not yet started.
        takeoffStore.startCalibration({ page: pageIndex, appliesToPage: pageIndex });
        takeoffStore.calibrationClickP1(p);
      } else if (step === "waiting_p1") {
        takeoffStore.calibrationClickP1(p);
      } else if (step === "waiting_p2") {
        const result = takeoffStore.calibrationClickP2(p);
        if (result) {
          calibDialogDist = result.pixelDist;
          showCalibDialog = true;
        }
      }
      e.stopPropagation();
      e.preventDefault();
      return;
    }

    // --- MEASUREMENT COUNT tool branch (single click, handled via onOverlayClick) ---
    if (tool === "MeasurementCount") return;

    // --- CREATE tool branch ---
    if (!isCreateTool(tool) || !identity) return;
    // Multi-click tools handle gestures via click/dblclick, not pointer capture.
    if (isMultiClickTool(tool) || tool === "MeasurementArea") return;
    const p = localPdf(e);
    if (!p) return;
    (e.target as Element).setPointerCapture(e.pointerId);
    drawing = true;
    if (isInkTool(tool)) {
      inkStroke = [p];
    } else {
      drawStartPdf = p;
    }
    e.stopPropagation();
    e.preventDefault();
  }

  function onOverlayPointerMove(e: PointerEvent) {
    const tool = store.activeTool;

    // --- SELECT tool: live resize / move / vertex preview, or marquee ---
    if (isSelectTool(tool)) {
      if (gesture === "vertex" && vertexWorking && vertexIndex !== null) {
        const p = localPdf(e);
        if (p) {
          dragPreview = [{ ...vertexWorking, geometry: moveVertex(vertexWorking.geometry, vertexIndex, p) }];
        }
        return;
      }
      if (gesture === "resize" && resizeOrig && resizeHandle && resizeOrigBounds) {
        const p = localPdf(e);
        if (p) {
          const nb = resizeBounds(resizeOrigBounds, resizeHandle, p, MIN_RESIZE_PTS);
          dragPreview = [{ ...resizeOrig, geometry: scaleGeometryToBounds(resizeOrig.geometry, resizeOrigBounds, nb) }];
        }
        return;
      }
      if (gesture === "move" && moveStartPdf) {
        const p = localPdf(e);
        if (p) {
          const dx = p.x - moveStartPdf.x;
          const dy = p.y - moveStartPdf.y;
          dragPreview = moveOrigins.map((m) => ({ ...m, geometry: translateGeometry(m.geometry, dx, dy) }));
        }
        return;
      }
      if (marquee) {
        const r = containerEl?.getBoundingClientRect();
        const sx = r ? e.clientX - r.left : e.clientX;
        const sy = r ? e.clientY - r.top : e.clientY;
        marquee = { ...marquee, x1: sx, y1: sy };
      }
      return;
    }

    if (!drawing || !identity) return;

    if (isInkTool(tool)) {
      const p = localPdf(e);
      if (!p) return;
      // Throttle: skip if within ~1px of the last point.
      const last = inkStroke[inkStroke.length - 1];
      if (last && Math.abs(p.x - last.x) < 1 && Math.abs(p.y - last.y) < 1) return;
      inkStroke = [...inkStroke, p];
      // Live preview for ink (match the commit guard: ≥2 points).
      if (inkStroke.length >= 2) {
        previewMarkup = buildMarkup({
          markupType: "Ink",
          page: pageIndex,
          geometry: inkGeometry([inkStroke]),
          appearance: store.draftAppearance,
          identity,
          now: new Date().toISOString(),
          id: "preview",
        });
      }
      return;
    }

    if (!drawStartPdf || !isDrawTool(tool)) return;
    const p = localPdf(e);
    if (!p) return;
    previewMarkup = buildMarkup({
      markupType: tool,
      page: pageIndex,
      geometry: dragDrawGeometry(tool, drawStartPdf, p, { constrain: e.shiftKey }),
      appearance: store.draftAppearance,
      identity,
      now: new Date().toISOString(),
      id: "preview",
    });
  }

  function onOverlayPointerUp(e: PointerEvent) {
    previewMarkup = null; // clear the live preview unconditionally (no ghost on early-return)
    const tool = store.activeTool;

    // --- SELECT tool: commit resize / move / vertex / marquee ---
    if (isSelectTool(tool)) {
      const now = new Date().toISOString();

      if (gesture === "vertex" && vertexWorking && vertexIndex !== null && vertexBefore) {
        const before = vertexBefore, working = vertexWorking, idx = vertexIndex;
        const p = localPdf(e);
        resetGesture();
        if (p) {
          const after = moveVertex(working.geometry, idx, p);
          // Commit when the geometry actually changed vs. the original committed markup —
          // covers both a vertex drag and a midpoint insert released without a drag.
          if (JSON.stringify(after) !== JSON.stringify(before.geometry)) {
            store.applyBatch([{ before, after: bumpAudit({ ...before, geometry: after }, identity ?? before.audit.modified_by, now) }]);
          }
        }
        return;
      }

      if (gesture === "resize" && resizeOrig && resizeHandle && resizeOrigBounds) {
        const orig = resizeOrig, handle = resizeHandle, ob = resizeOrigBounds;
        const p = localPdf(e);
        resetGesture();
        if (p) {
          const nb = resizeBounds(ob, handle, p, MIN_RESIZE_PTS);
          const geometry = scaleGeometryToBounds(orig.geometry, ob, nb);
          if (JSON.stringify(geometry) !== JSON.stringify(orig.geometry)) {
            const after = bumpAudit({ ...orig, geometry }, identity ?? orig.audit.modified_by, now);
            store.applyBatch([{ before: orig, after }]);
          }
        }
        return;
      }

      if (gesture === "move" && moveStartPdf) {
        const origins = moveOrigins, start = moveStartPdf;
        const p = localPdf(e);
        resetGesture();
        if (p) {
          const dx = p.x - start.x, dy = p.y - start.y;
          if (dx !== 0 || dy !== 0) {
            const pairs = origins.map((m) => ({
              before: m,
              after: bumpAudit({ ...m, geometry: translateGeometry(m.geometry, dx, dy) }, identity ?? m.audit.modified_by, now),
            }));
            store.applyBatch(pairs);
          }
        }
        return;
      }

      if (marquee) {
        const m = marquee;
        marquee = null;
        // Convert marquee screen rect to PDF Bounds via clientToPdf.
        const r = containerEl?.getBoundingClientRect();
        const toClient = (sx: number, sy: number) => ({ x: (r?.left ?? 0) + sx, y: (r?.top ?? 0) + sy });
        const p0 = clientToPdf(toClient(m.x0, m.y0).x, toClient(m.x0, m.y0).y);
        const p1 = clientToPdf(toClient(m.x1, m.y1).x, toClient(m.x1, m.y1).y);
        if (p0 && p1) {
          const pdfBounds: Bounds = {
            minX: Math.min(p0.x, p1.x), minY: Math.min(p0.y, p1.y),
            maxX: Math.max(p0.x, p1.x), maxY: Math.max(p0.y, p1.y),
          };
          const pageMarkups = store.markups.filter((mm) => mm.page === pageIndex);
          const ids = marqueeHits(pageMarkups, pdfBounds);
          if (marqueeAdditive) {
            const next = new Set(store.selectedIds);
            for (const id of ids) next.add(id);
            store.selectedIds = next;
          } else {
            store.selectedIds = new Set(ids);
          }
        }
        marqueeAdditive = false;
      }
      return;
    }


    if (isInkTool(tool)) {
      if (!drawing || !identity) { drawing = false; inkStroke = []; return; }
      const p = localPdf(e);
      if (p) inkStroke = [...inkStroke, p];
      drawing = false;
      if (inkStroke.length >= 2) {
        store.create(buildMarkup({
          markupType: "Ink",
          page: pageIndex,
          geometry: inkGeometry([inkStroke]),
          appearance: store.draftAppearance,
          identity,
          now: new Date().toISOString(),
          id: crypto.randomUUID(),
        }));
      }
      inkStroke = [];
      return;
    }

    // --- MeasurementLength: drag-draw with measurement payload ---
    if (tool === "MeasurementLength") {
      if (!drawing || !drawStartPdf || !identity) { drawing = false; drawStartPdf = null; return; }
      const p = localPdf(e);
      drawing = false;
      const start = drawStartPdf;
      drawStartPdf = null;
      if (!p || (p.x === start.x && p.y === start.y)) return;
      const pts = [start, p];
      const raw = measureLength(pts);
      const scale = takeoffStore.activeScale;
      const meas: MeasurementPayload = {
        scale_ref: scale?.id ?? null,
        raw_measure: raw,
        unit: scale?.unit ?? "pt",
        computed_quantity: scale ? raw * scale.ratio : 0,
        depth: null,
        count_value: null,
        custom_columns: {},
      };
      const m = buildMarkup({
        markupType: "MeasurementLength",
        page: pageIndex,
        geometry: { Polyline: pts },
        appearance: store.draftAppearance,
        identity,
        now: new Date().toISOString(),
        id: crypto.randomUUID(),
      });
      m.measurement = meas;
      store.create(m);
      return;
    }

    if (!drawing || !drawStartPdf || !identity || !isDrawTool(tool)) {
      drawing = false;
      drawStartPdf = null;
      return;
    }
    const p = localPdf(e);
    drawing = false;
    const start = drawStartPdf;
    drawStartPdf = null;
    if (!p || (p.x === start.x && p.y === start.y)) return; // zero-size = no-op
    store.create(buildMarkup({
      markupType: tool,
      page: pageIndex,
      geometry: dragDrawGeometry(tool, start, p, { constrain: e.shiftKey }),
      appearance: store.draftAppearance,
      identity,
      now: new Date().toISOString(),
      id: crypto.randomUUID(),
    }));
  }

  // Multi-click tool gesture handlers (click adds vertex, dblclick finishes).
  function onOverlayClick(e: MouseEvent) {
    // Selection is handled by pointer events, not click.
    if (isSelectTool()) return;
    const tool = store.activeTool;

    // MeasurementCount: single click places a count point.
    if (tool === "MeasurementCount" && identity) {
      const p = localPdfFromMouse(e);
      if (!p) return;
      const scale = takeoffStore.activeScale;
      const meas: MeasurementPayload = {
        scale_ref: scale?.id ?? null,
        raw_measure: 1,
        unit: scale?.unit ?? "ea",
        computed_quantity: 1,
        depth: null,
        count_value: 1,
        custom_columns: {},
      };
      const m = buildMarkup({
        markupType: "MeasurementCount",
        page: pageIndex,
        geometry: { Point: p },
        appearance: store.draftAppearance,
        identity,
        now: new Date().toISOString(),
        id: crypto.randomUUID(),
      });
      m.measurement = meas;
      store.create(m);
      return;
    }

    // MeasurementArea: multi-click polygon (same as Polygon but with measurement payload).
    if (tool === "MeasurementArea" && identity) {
      const p = localPdfFromMouse(e);
      if (!p) return;
      mcVerts = [...mcVerts, p];
      // Update rubber-band preview.
      const vertsForPreview = mcCursor ? [...mcVerts, mcCursor] : mcVerts;
      if (vertsForPreview.length >= 2) {
        previewMarkup = buildMarkup({
          markupType: "MeasurementArea",
          page: pageIndex,
          geometry: polylineGeometry(vertsForPreview),
          appearance: store.draftAppearance,
          identity,
          now: new Date().toISOString(),
          id: "preview",
        });
      }
      return;
    }

    // Text/Callout tool: open the inline editor on click.
    if (isTextTool(tool) && identity) {
      const p = localPdfFromMouse(e);
      if (!p) return;
      if (!containerEl) return;
      const r = containerEl.getBoundingClientRect();
      const screenX = e.clientX - r.left;
      const screenY = e.clientY - r.top;
      if (tool === "Text") {
        editorText = "";
        editor = { screenX, screenY, anchorPdf: p, leaderPdf: null };
      } else {
        // Callout: first click sets leader target; second click opens editor
        if (!calloutTarget) {
          calloutTarget = p;
        } else {
          editorText = "";
          editor = { screenX, screenY, anchorPdf: p, leaderPdf: calloutTarget };
          calloutTarget = null;
        }
      }
      return;
    }

    if (!isMultiClickTool(tool) || !identity) return;
    const p = localPdfFromMouse(e);
    if (!p) return;
    mcVerts = [...mcVerts, p];
    // Update rubber-band preview.
    const mcTool = tool as MultiClickTool;
    const vertsForPreview = mcCursor ? [...mcVerts, mcCursor] : mcVerts;
    if (vertsForPreview.length >= 2) {
      previewMarkup = buildMarkup({
        markupType: mcTool,
        page: pageIndex,
        geometry: polylineGeometry(vertsForPreview),
        appearance: store.draftAppearance,
        identity,
        now: new Date().toISOString(),
        id: "preview",
      });
    }
  }

  function onOverlayDblClick() {
    // No dblclick action under select tool.
    if (isSelectTool()) return;
    const tool = store.activeTool;
    // The browser fires click→click→dblclick, so the dblclick's two constituent
    // clicks each appended a vertex at ~the same point; drop the duplicate before
    // finishing.
    if (mcVerts.length > 0) mcVerts = mcVerts.slice(0, -1);

    // MeasurementArea: finish polygon and add measurement payload.
    if (tool === "MeasurementArea" && identity && mcVerts.length >= 3) {
      const raw = measureArea(mcVerts);
      const scale = takeoffStore.activeScale;
      const meas: MeasurementPayload = {
        scale_ref: scale?.id ?? null,
        raw_measure: raw,
        unit: scale?.unit ?? "pt²",
        computed_quantity: scale ? raw * scale.ratio * scale.ratio : 0,
        depth: null,
        count_value: null,
        custom_columns: {},
      };
      const m = buildMarkup({
        markupType: "MeasurementArea",
        page: pageIndex,
        geometry: polylineGeometry(mcVerts),
        appearance: store.draftAppearance,
        identity,
        now: new Date().toISOString(),
        id: crypto.randomUUID(),
      });
      m.measurement = meas;
      store.create(m);
      resetMultiClick();
      return;
    }

    finishMultiClick();
  }

  function onOverlayMouseMove(e: MouseEvent) {
    if (isSelectTool()) return;
    const tool = store.activeTool;
    const isAreaTool = tool === "MeasurementArea";
    if (!isMultiClickTool(tool) && !isAreaTool) return;
    if (mcVerts.length === 0 || !identity) return;
    const p = localPdfFromMouse(e);
    if (!p) return;
    mcCursor = p;
    // Rubber-band: show current verts + live cursor.
    const vertsForPreview = [...mcVerts, p];
    if (vertsForPreview.length >= 2) {
      previewMarkup = buildMarkup({
        // MeasurementArea uses "MeasurementArea" as the markupType for the preview.
        markupType: isAreaTool ? "MeasurementArea" : (tool as MultiClickTool),
        page: pageIndex,
        geometry: polylineGeometry(vertsForPreview),
        appearance: store.draftAppearance,
        identity,
        now: new Date().toISOString(),
        id: "preview",
      });
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
    // Load user identity for markup authoring. On failure, surface a notice so the
    // crosshair-but-nothing-happens state isn't silent.
    getUserIdentity()
      .then((u) => (identity = u))
      .catch(() => (identityError = true));
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    window.removeEventListener("keydown", onKeyDown);
    if (rssTimer) clearInterval(rssTimer);
    if (zoomSettleTimer) clearTimeout(zoomSettleTimer);
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

  <!-- Markup overlay — SVG, drawn on top of tiles (spec §6).
       Captures pointer events only when a drag-draw tool is active (.capture class).
       Otherwise pointer-events:none so Hand-tool pan flows through to viewport-root. -->
  <svg
    class="markup-overlay"
    class:capture={overlayActive}
    aria-hidden="true"
    onpointerdown={onOverlayPointerDown}
    onpointermove={onOverlayPointerMove}
    onpointerup={onOverlayPointerUp}
    onpointercancel={cancelDraw}
    onclick={onOverlayClick}
    ondblclick={onOverlayDblClick}
    onmousemove={onOverlayMouseMove}
  >
    {#snippet shape(s: SvgShape)}
      <!--
        All shape elements carry pointer-events="none" so that click/pointer events
        (e.g. multi-click polygon placement, select tool's own hit-testing) always
        reach the SVG overlay directly. The select tool uses mathematical hit-testing
        (hitTest) and does not need shape elements to receive events natively.
        WKWebView (Tauri/macOS) does not reliably bubble events from filled SVG child
        elements to a parent onclick handler — pointer-events:none avoids the issue.
      -->
      {#if s.kind === "rect"}
        <rect
          x={s.x} y={s.y} width={s.width} height={s.height}
          stroke={s.stroke} stroke-width={s.strokeWidth}
          fill={s.fill} opacity={s.opacity}
          stroke-dasharray={s.dashArray ?? undefined}
          pointer-events="none"
        />
      {:else if s.kind === "ellipse"}
        <ellipse
          cx={s.cx} cy={s.cy} rx={s.rx} ry={s.ry}
          stroke={s.stroke} stroke-width={s.strokeWidth}
          fill={s.fill} opacity={s.opacity}
          stroke-dasharray={s.dashArray ?? undefined}
          pointer-events="none"
        />
      {:else if s.kind === "polygon"}
        <polygon points={s.points}
          stroke={s.stroke} stroke-width={s.strokeWidth}
          fill={s.fill} opacity={s.opacity}
          stroke-dasharray={s.dashArray ?? undefined}
          pointer-events="none" />
      {:else if s.kind === "cloud"}
        <path d={s.path}
          stroke={s.stroke} stroke-width={s.strokeWidth}
          fill={s.fill} opacity={s.opacity}
          stroke-dasharray={s.dashArray ?? undefined}
          pointer-events="none" />
      {:else if s.kind === "arrow"}
        <!--
          Arrow: shortened shaft + explicit arrowhead triangle.
          fill="context-stroke" on SVG markers is unsupported in macOS WKWebView, so
          the arrowhead is rendered as a plain <polygon> filled with the markup color.
          The polyline ends at the arrowhead base (not the tip) so the shaft does not
          visually protrude through the head.
        -->
        <polyline points={s.points}
          stroke={s.stroke} stroke-width={s.strokeWidth}
          fill="none" opacity={s.opacity}
          stroke-dasharray={s.dashArray ?? undefined}
          pointer-events="none"
        />
        {#if s.arrowHead}
          <polygon points={s.arrowHead}
            fill={s.stroke} stroke="none"
            opacity={s.opacity}
            pointer-events="none"
          />
        {/if}
      {:else if s.kind === "polyline"}
        <polyline points={s.points}
          stroke={s.stroke} stroke-width={s.strokeWidth}
          fill="none" opacity={s.opacity}
          stroke-dasharray={s.dashArray ?? undefined}
          pointer-events="none" />
      {:else if s.kind === "ink"}
        {#each s.strokes as stroke, i (i)}
          <polyline points={stroke}
            stroke={s.stroke} stroke-width={s.strokeWidth}
            fill="none" opacity={s.opacity}
            stroke-linecap="round" stroke-linejoin="round"
            pointer-events="none" />
        {/each}
      {:else if s.kind === "point"}
        <circle cx={s.x} cy={s.y} r="6"
          stroke={s.stroke} stroke-width={s.strokeWidth}
          fill={s.fill === "none" ? s.stroke : s.fill}
          opacity={s.opacity}
          pointer-events="none" />
      {:else if s.kind === "text"}
        <text x={s.x} y={s.y} fill={s.stroke} font-size={s.fontPx}
          dominant-baseline="hanging" opacity={s.opacity}
          pointer-events="none">{s.text}</text>
      {:else if s.kind === "callout"}
        <polyline points={s.points} stroke={s.stroke} stroke-width={s.strokeWidth}
          fill="none" opacity={s.opacity} stroke-dasharray={s.dashArray ?? undefined}
          pointer-events="none" />
        <text x={s.x} y={s.y} fill={s.stroke} font-size={s.fontPx}
          dominant-baseline="hanging" opacity={s.opacity}
          pointer-events="none">{s.text}</text>
      {/if}
    {/snippet}

    {#each pageShapes as s (s.id)}
      {@render shape(s)}
    {/each}

    {#if previewShape}
      {@render shape(previewShape)}
    {/if}

    <!-- Selection chrome: bounding box + resize handles (pointer-events:none so they don't steal gestures). -->
    {#if chrome}
      <rect
        class="selection-box"
        x={chrome.box.x} y={chrome.box.y}
        width={chrome.box.width} height={chrome.box.height}
        pointer-events="none"
      />
      {#each chrome.handles as h (h.id)}
        <rect
          class="selection-handle"
          x={h.x - 4} y={h.y - 4}
          width={8} height={8}
          pointer-events="none"
        />
      {/each}
    {/if}

    <!-- Per-vertex editing handles for a single selected multipoint markup.
         Midpoints (insert a vertex) render under the vertices (drag/delete a vertex).
         Hit-testing is mathematical (vertexAtScreen/midpointAtScreen), so these carry
         pointer-events:none like every other overlay shape. -->
    {#if vertexHandles}
      {#each vertexHandles.midpoints as mp (mp.segmentIndex)}
        <rect
          class="midpoint-handle"
          x={mp.x - 3} y={mp.y - 3}
          width={6} height={6}
          pointer-events="none"
        />
      {/each}
      {#each vertexHandles.vertices as v (v.index)}
        <rect
          class="vertex-handle"
          class:vertex-handle-active={v.index === activeVertex}
          x={v.x - 4} y={v.y - 4}
          width={8} height={8}
          pointer-events="none"
        />
      {/each}
    {/if}

    <!-- Marquee drag preview. -->
    {#if marquee}
      <rect
        class="marquee"
        x={Math.min(marquee.x0, marquee.x1)}
        y={Math.min(marquee.y0, marquee.y1)}
        width={Math.abs(marquee.x1 - marquee.x0)}
        height={Math.abs(marquee.y1 - marquee.y0)}
        pointer-events="none"
      />
    {/if}

    <!-- Search hit highlights (M4 S3). Semi-transparent rects over matched text.
         Active hit uses a stronger fill for the "you are here" indicator. -->
    {#each pageSearchHits as r (r.idx)}
      <rect
        class="search-hit"
        class:search-hit-active={r.active}
        x={r.x} y={r.y}
        width={r.width} height={r.height}
        pointer-events="none"
        aria-hidden="true"
      />
    {/each}
  </svg>

  <!-- Calibration dialog: shown after user clicks two points with the calibrate tool. -->
  {#if showCalibDialog}
    <CalibrationDialog
      pixelDist={calibDialogDist}
      onConfirm={async (result) => {
        showCalibDialog = false;
        takeoffStore.cancelCalibration();
        // Persist the new scale via IPC and add it to the store.
        try {
          const rec = await addScale(docInfo.doc_id, null, result.ratio, result.unit, result.label, result.precision);
          takeoffStore.addScale(rec);
        } catch (e) {
          console.error("addScale failed:", e);
        }
        store.activeTool = "hand";
      }}
      onCancel={() => {
        showCalibDialog = false;
        takeoffStore.cancelCalibration();
        store.activeTool = "hand";
      }}
    />
  {/if}

  <!-- Inline text editor (Text/Callout). Positioned over the overlay at the click point. -->
  {#if editor}
    <textarea
      class="text-editor"
      style="left: {editor.screenX}px; top: {editor.screenY}px;"
      bind:value={editorText}
      onblur={commitEditor}
      onkeydown={(e) => {
        if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); commitEditor(); }
        if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); cancelEditor(); }
      }}
      autofocus
      rows={3}
      cols={20}
    ></textarea>
  {/if}

  <!-- Identity-load failure: a creation tool is selected but authoring can't proceed. -->
  {#if identityError && isCreateTool()}
    <div class="draw-unavailable">
      Markup authoring unavailable — user identity failed to load.
    </div>
  {/if}

  <!-- Page navigation -->
  <nav class="page-nav">
    <button class="btn-nav" onclick={prevPage} disabled={pageIndex === 0}>‹</button>
    <span class="page-label">
      Page {pageIndex + 1} / {docInfo.page_count}
    </span>
    <button class="btn-nav" onclick={nextPage} disabled={pageIndex >= docInfo.page_count - 1}>›</button>
  </nav>

  <!-- Zoom-snap presets: full-width, full-height, 1:1 (key-commands ⌘/Ctrl 1/2/0). -->
  <div class="zoom-controls" role="group" aria-label="Zoom presets">
    <button class="btn-zoom" title="Fit width (⌘/Ctrl 1)" onclick={fitWidth}>Fit W</button>
    <button class="btn-zoom" title="Fit height (⌘/Ctrl 2)" onclick={fitHeight}>Fit H</button>
    <button class="btn-zoom" title="Actual size · 100% (⌘/Ctrl 0)" onclick={actualSize}>100%</button>
  </div>

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
  .markup-overlay.capture {
    pointer-events: auto;
    cursor: crosshair;
  }

  /* Selection bounding box (dashed, no fill). */
  :global(.markup-overlay .selection-box) {
    fill: none;
    stroke: var(--color-primary);
    stroke-width: 1.5px;
    stroke-dasharray: 5, 3;
    opacity: 0.9;
  }

  /* Resize handles: small filled squares. */
  :global(.markup-overlay .selection-handle) {
    fill: var(--color-bg, #fff);
    stroke: var(--color-primary);
    stroke-width: 1.5px;
    opacity: 1;
  }

  /* Per-vertex editing handles (square, filled — one per Polyline point). */
  :global(.markup-overlay .vertex-handle) {
    fill: var(--color-bg, #fff);
    stroke: var(--color-primary);
    stroke-width: 1.5px;
    opacity: 1;
  }
  /* The vertex armed for keyboard delete is filled with the accent colour. */
  :global(.markup-overlay .vertex-handle-active) {
    fill: var(--color-primary);
  }
  /* Midpoint "insert here" handles — smaller, semi-transparent accent squares. */
  :global(.markup-overlay .midpoint-handle) {
    fill: var(--color-primary);
    fill-opacity: 0.4;
    stroke: var(--color-primary);
    stroke-width: 1px;
  }

  /* Marquee drag preview (dashed, no fill). */
  :global(.markup-overlay .marquee) {
    fill: none;
    stroke: var(--color-primary);
    stroke-width: 1px;
    stroke-dasharray: 4, 2;
    opacity: 0.7;
  }

  /* Search hit highlights (M4 S3). Semi-transparent yellow fill, no stroke. */
  :global(.markup-overlay .search-hit) {
    fill: #facc15; /* yellow-400 equivalent */
    fill-opacity: 0.35;
    stroke: none;
  }

  /* Active (focused) search hit: stronger highlight. */
  :global(.markup-overlay .search-hit-active) {
    fill: #f97316; /* orange-500 equivalent */
    fill-opacity: 0.45;
  }

  /* Inline text editor (Text/Callout tool). */
  .text-editor {
    position: absolute;
    z-index: 20;
    min-width: 120px;
    min-height: 2em;
    background: var(--color-bg);
    color: var(--color-text);
    border: 1px solid var(--color-primary);
    border-radius: var(--radius-sm);
    padding: var(--space-1) var(--space-2);
    font-size: var(--font-size-base);
    font-family: var(--font-sans);
    resize: both;
    outline: none;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.4);
  }

  /* Identity-load failure notice (shown while a draw tool is active). */
  .draw-unavailable {
    position: absolute;
    top: var(--space-3);
    left: 50%;
    transform: translateX(-50%);
    background: var(--color-danger);
    color: #fff;
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-3);
    border-radius: var(--radius-sm);
    pointer-events: none;
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

  /* --- Zoom-snap preset buttons --- */
  .zoom-controls {
    position: absolute;
    bottom: var(--space-4);
    right: var(--space-4);
    display: flex;
    gap: var(--space-1);
    background: rgba(26, 26, 28, 0.85);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-lg);
    padding: var(--space-1);
    backdrop-filter: blur(8px);
  }
  .btn-zoom {
    background: none;
    border: none;
    color: var(--color-text-secondary);
    cursor: pointer;
    font-size: var(--font-size-xs);
    font-family: var(--font-mono);
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-sm);
    line-height: 1;
    transition: background 120ms, color 120ms;
  }
  .btn-zoom:hover { background: var(--color-bg-hover); color: var(--color-text); }

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
