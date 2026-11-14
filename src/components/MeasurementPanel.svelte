<script lang="ts">
  /**
   * MeasurementPanel — quantity table for M3 takeoff (spec §7).
   *
   * Shows all markups whose markup_type is a measurement type
   * (MeasurementLength | MeasurementArea | MeasurementCount), grouped by type,
   * with their computed quantity, unit, scale label, and an export button.
   *
   * Props:
   *   store       — MarkupStore (source of measurement markups)
   *   takeoffStore — TakeoffStore (active scale for display)
   *   docId       — for the exportMarkupList IPC call
   */
  import { exportDoneMessage } from "$lib/doc-op-flow";
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import type { TakeoffStore } from "$lib/takeoff-store.svelte";
  import type { Markup, UserRef } from "$lib/ipc";
  import { exportMarkupList, getPageSize, getUserIdentity, updateViewport, type ExportFormat, type ScaleRecord } from "$lib/ipc";
  import { measurementsUsing, reapplyViewport } from "$lib/polylength";
  import { commitViewportChange, drainScaleNotices, reportViewportError } from "$lib/viewport-actions";
  import type { ViewportDialogResult } from "$lib/scale-presets";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import ViewportDialog from "./ViewportDialog.svelte";
  import { legendFromCountSets } from "$lib/legend";
  import { onMount } from "svelte";
  import { countSubtotalsByPage, totalsByUnit } from "$lib/measurement-tools";
  import { countGroups as groupCounts, groupKey, resumeRun } from "$lib/count-groups";
  import { countSymbolRender, COUNT_MARKER_RADIUS } from "$lib/markup-render";
  import { save as saveDialog } from "@tauri-apps/plugin-dialog";
  import CountSetPicker from "./CountSetPicker.svelte";
  import SymbolSearchPanel from "./SymbolSearchPanel.svelte";

  const {
    store,
    takeoffStore,
    docId,
    currentPage = 0,
    pageCount = 1,
    onjump,
  }: {
    store: MarkupStore;
    takeoffStore: TakeoffStore;
    docId: string;
    currentPage?: number;
    /** Pages in the document (Symbol Search "all pages" scope). */
    pageCount?: number;
    onjump?: (target: { page: number; rect: [number, number, number, number] }) => void;
  } = $props();

  const MEASUREMENT_TYPES = new Set([
    "MeasurementLength", "MeasurementArea", "MeasurementCount",
    "MeasurementPerimeter", "MeasurementVolume", "MeasurementAngle", "MeasurementRadius",
  ]);

  const measurements = $derived(
    store.markups.filter((m: Markup) => MEASUREMENT_TYPES.has(m.markup_type))
  );

  // One table row per measurement - except count items: a count (a run of items sharing
  // `count_group`) is ONE row carrying the group's label and total (docs/count-polylength.md).
  const countGroupByKey = $derived(new Map(groupCounts(measurements).map((g) => [g.key, g])));
  const rows = $derived.by(() => {
    const seen = new Set<string>();
    const out: { m: Markup; group: ReturnType<typeof groupCounts>[number] | null }[] = [];
    for (const m of measurements) {
      if (m.markup_type !== "MeasurementCount") {
        out.push({ m, group: null });
        continue;
      }
      const k = groupKey(m);
      if (seen.has(k)) continue;
      seen.add(k);
      out.push({ m, group: countGroupByKey.get(k) ?? null });
    }
    return out;
  });

  let countMessage = $state<string | null>(null);

  // The acting user, so a Resume stamp records them (not the original author) as modifier.
  let identity = $state<UserRef | null>(null);
  onMount(() => {
    getUserIdentity()
      .then((u) => { identity = u; })
      .catch(() => {});
  });

  // A legend of the document's count sets (docs/legends.md R4), placed at the top-left of the
  // current page; new sets are added to it automatically. Select it to edit it.
  async function createCountSetLegend() {
    if (!identity || store.countSets.length === 0) return;
    let size = { width_pts: 612, height_pts: 792 };
    try {
      size = await getPageSize(docId, currentPage);
    } catch {
      /* fall back to a Letter page; the legend can be moved */
    }
    const legend = legendFromCountSets(store.countSets, {
      page: currentPage,
      topLeft: { x: 36, y: size.height_pts - 36 },
      identity,
      now: new Date().toISOString(),
      id: crypto.randomUUID(),
    });
    store.create(legend);
    store.selectedIds = new Set([legend.id]);
    store.activeTool = "select";
  }

  function selectCount(ids: string[]) {
    store.selectedIds = new Set(ids);
    store.activeTool = "select";
  }

  function resumeCount(itemId: string) {
    const r = resumeRun(store.markups, itemId, identity ?? undefined);
    if ("error" in r) {
      countMessage = r.error;
      return;
    }
    store.applyBatch(r.pairs);
    if (r.run.setId) store.setActiveCountSet(r.run.setId);
    store.countRun = r.run;
    store.activeTool = "MeasurementCount";
    countMessage = null;
  }

  // Grouped by unit: metres and feet (or a viewport at another unit) are never summed together.
  const lengthTotals = $derived(totalsByUnit(measurements, "MeasurementLength"));
  const areaTotals = $derived(totalsByUnit(measurements, "MeasurementArea"));

  // Per-set count subtotals (spec §7): group MeasurementCount markups by their count set,
  // with per-page breakdown for the quantities panel.
  const countGroups = $derived(countSubtotalsByPage(measurements));
  const totalCount = $derived(countGroups.reduce((sum, g) => sum + g.count, 0));

  // 14px symbol swatch for the subtotal rows (reuses the live render geometry).
  const SWATCH = 14;
  function swatch(symbol: import("$lib/ipc").CountSymbol) {
    return countSymbolRender(symbol, SWATCH / 2, SWATCH / 2, COUNT_MARKER_RADIUS);
  }

  const scale = $derived(takeoffStore.activeScale);

  // --- Measurement viewports (docs/measurement-viewports.md) ---
  const viewports = $derived(takeoffStore.viewports);
  let viewportMessage = $state<string | null>(null);
  let editingViewport = $state<ScaleRecord | null>(null);
  let deletingViewport = $state<ScaleRecord | null>(null);

  function vpName(v: ScaleRecord): string {
    return v.applies_to.kind === "Viewport" ? v.applies_to.name : v.label;
  }

  /** The scale a measurement row was taken under: a viewport shows its name too. */
  function scaleCell(m: Markup): string {
    const ref = m.measurement?.scale_ref;
    if (!ref) return "raw";
    const s = takeoffStore.scales.find((x) => x.id === ref);
    if (!s) return "-";
    return s.applies_to.kind === "Viewport" ? `${s.applies_to.name} (${s.label})` : s.label;
  }

  async function applyViewportEdit(v: ScaleRecord, r: ViewportDialogResult) {
    editingViewport = null;
    viewportMessage = null;
    try {
      if (r.mode === "preset") {
        const rec = await updateViewport(docId, v.id, {
          name: r.name, ratio: r.ratio, unit: r.unit, label: r.label, precision: r.precision, twoPoint: false,
        });
        // One undo step: the new scale and the measurements taken under it.
        commitViewportChange(store, takeoffStore, v, rec, identity);
      } else {
        if (r.name !== vpName(v)) {
          commitViewportChange(store, takeoffStore, v, await updateViewport(docId, v.id, { name: r.name }), identity);
        }
        if (r.mode === "calibrate" && v.applies_to.kind === "Viewport") {
          takeoffStore.startCalibration({
            page: v.applies_to.page, appliesToPage: null, viewport: { kind: "existing", scaleId: v.id },
          });
          store.activeTool = "calibrate";
          viewportMessage = `Click two points of a known length to calibrate ${r.name}.`;
        }
      }
      void drainScaleNotices(docId);
    } catch (e) {
      reportViewportError("Changing the viewport", e);
    }
  }

  function redrawViewport(v: ScaleRecord) {
    // The redraw tool: leaving it (any other tool, Escape) cancels the redraw.
    store.activeTool = "viewportRedraw";
    takeoffStore.redrawViewportId = v.id;
    viewportMessage = `Drag the new area for ${vpName(v)} (on any page). Its measurements keep their values.`;
  }

  /** Explicit action: measurements inside the viewport are put under its scale. */
  function reapply(v: ScaleRecord) {
    if (!identity) return;
    const pairs = reapplyViewport(store.markups, takeoffStore.scales, v.id, identity, new Date().toISOString());
    store.applyBatch(pairs);
    viewportMessage = pairs.length === 0
      ? `Every measurement inside ${vpName(v)} already uses its scale.`
      : `${pairs.length} measurement${pairs.length === 1 ? "" : "s"} inside ${vpName(v)} now use${pairs.length === 1 ? "s" : ""} its scale.`;
  }

  /**
   * Delete = retire: the viewport disappears (from the page, the list and the PDF on the next
   * save) but the measurements taken under it keep their values. One undo step; Ctrl+Z brings
   * it back.
   */
  function confirmDeleteViewport(v: ScaleRecord) {
    deletingViewport = null;
    viewportMessage = null;
    commitViewportChange(store, takeoffStore, v, { ...v, retired: true }, identity);
  }

  const scaleLabel = $derived(scale ? `${scale.label} (${scale.unit})` : "No scale set");

  let exportError = $state<string | null>(null);
  let exportMessage = $state<string | null>(null);

  async function handleExport(format: ExportFormat) {
    exportError = null;
    exportMessage = null;
    const ext = format === "Xlsx" ? "xlsx" : "csv";
    const dest = await saveDialog({
      filters: [{ name: format === "Xlsx" ? "Excel" : "CSV", extensions: [ext] }],
    });
    if (!dest) return;
    try {
      // The backend exports its own copy of the list; drain pending edits first.
      await store.flush();
      await exportMarkupList(docId, dest, format);
      exportMessage = exportDoneMessage(dest, store.markups.length);
    } catch (e) {
      exportError = `Export failed: ${e instanceof Error ? e.message : String(e)}`;
    }
  }
</script>

<div class="measurement-panel">
  <div class="panel-actions">
    <span class="scale-badge" title="Active scale">{scaleLabel}</span>
    <button class="btn-export" onclick={() => handleExport("Xlsx")} disabled={store.markups.length === 0}>
      Export XLSX
    </button>
    <button class="btn-export" onclick={() => handleExport("Csv")} disabled={store.markups.length === 0}>
      Export CSV
    </button>
    <button class="btn-export" data-action="legend-from-count-sets" onclick={createCountSetLegend}
      disabled={!identity || store.countSets.length === 0}>
      Legend from count sets
    </button>
  </div>

  {#if exportMessage}
    <div class="export-ok" role="status">{exportMessage}</div>
  {/if}

  {#if exportError}
    <div class="export-error">{exportError}</div>
  {/if}

  <SymbolSearchPanel {store} {docId} {currentPage} {pageCount} {identity} {onjump} />

  <section class="viewports" aria-label="Viewports">
    <div class="viewports-head">
      <span class="section-title">Viewports</span>
      <button type="button" class="row-action" data-action="new-viewport"
        title="Drag an area of the sheet that has its own scale"
        onclick={() => { store.activeTool = "viewport"; }}>New viewport</button>
    </div>
    {#if viewports.length === 0}
      <p class="empty-hint">No viewports. Use one where part of a sheet has its own scale, such as a 1:20 detail on a 1:100 plan.</p>
    {:else}
      <table class="quantity-table" aria-label="Measurement viewports">
        <thead><tr><th>Name</th><th>Page</th><th>Scale</th><th></th></tr></thead>
        <tbody>
          {#each viewports as v (v.id)}
            {#if v.applies_to.kind === "Viewport"}
              <tr data-viewport={v.applies_to.viewport_id}>
                <td>{v.applies_to.name}</td>
                <td class="num-col">{v.applies_to.page + 1}</td>
                <td>{v.label} <span class="scale-cell">({v.unit})</span></td>
                <td class="actions-cell">
                  <button type="button" class="row-action" data-action="edit-viewport" onclick={() => (editingViewport = v)}>Edit</button>
                  <button type="button" class="row-action" data-action="redraw-viewport" onclick={() => redrawViewport(v)}>Redraw</button>
                  <button type="button" class="row-action" data-action="reapply-viewport"
                    title="Put the measurements that start inside this area under its scale"
                    onclick={() => reapply(v)} disabled={!identity}>Re-apply</button>
                  <button type="button" class="row-action" data-action="delete-viewport" onclick={() => (deletingViewport = v)}>Delete</button>
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    {/if}
    {#if viewportMessage}<div class="export-ok" role="status">{viewportMessage}</div>{/if}
  </section>

  {#if editingViewport}
    <ViewportDialog title="Edit Viewport" initialName={vpName(editingViewport)} allowKeep
      onCancel={() => (editingViewport = null)}
      onConfirm={(r) => { if (editingViewport) void applyViewportEdit(editingViewport, r); }} />
  {/if}
  {#if deletingViewport}
    {@const dv = deletingViewport}
    {@const n = measurementsUsing(store.markups, dv.id)}
    <ConfirmDialog title="Delete viewport"
      message={`Delete the viewport "${vpName(dv)}"?`}
      hint={n === 0 ? "You can undo this." : `${n} measurement${n === 1 ? "" : "s"} taken under it keep${n === 1 ? "s its" : " their"} current value${n === 1 ? "" : "s"} (${dv.label}); use Re-apply on another viewport to change them. You can undo this.`}
      confirmLabel="Delete"
      onCancel={() => (deletingViewport = null)}
      onConfirm={() => { if (deletingViewport) void confirmDeleteViewport(deletingViewport); }} />
  {/if}

  <CountSetPicker {store} />

  {#if countMessage}<div class="export-error" role="status">{countMessage}</div>{/if}

  {#if measurements.length === 0}
    <p class="empty-hint">No measurements yet. Use the calibrate + measurement tools to add some.</p>
  {:else}
    <table class="quantity-table" aria-label="Measurement quantities">
      <thead>
        <tr>
          <th>Type</th>
          <th>Label</th>
          <th class="num-col">Quantity</th>
          <th>Unit</th>
          <th>Scale</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each rows as { m, group } (m.id)}
          <tr data-count-group={group?.key}>
            <td class="type-cell">{m.markup_type.replace("Measurement", "")}</td>
            <td>{group ? `${group.label} (${group.ids.length} item${group.ids.length === 1 ? "" : "s"})` : (m.contents ?? "")}</td>
            <td class="num-col">
              {m.measurement
                ? group
                  ? group.total.toFixed(0)
                  : m.markup_type === "MeasurementCount"
                  ? (m.measurement.count_value ?? 1).toFixed(0)
                  : m.measurement.computed_quantity.toFixed(2)
                : "-"}
            </td>
            <td>{m.measurement?.unit ?? "-"}</td>
            <td class="scale-cell">
              {scaleCell(m)}
            </td>
            <td class="actions-cell">
              {#if group}
                <button type="button" class="row-action" data-action="resume-count" onclick={() => resumeCount(m.id)}>Resume</button>
                <button type="button" class="row-action" data-action="select-count" onclick={() => selectCount(group.ids)}>Select</button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
      <tfoot>
        {#each lengthTotals.length > 0 ? lengthTotals : [{ unit: scale?.unit ?? "pt", total: 0 }] as t, i (t.unit)}
          <tr class="totals-row" data-total="length" data-unit={t.unit}>
            <td colspan="2">{i === 0 ? "Totals" : ""}</td>
            <td class="num-col">{t.total.toFixed(2)}</td>
            <td>{t.unit} (length)</td>
            <td></td>
          </tr>
        {/each}
        {#each areaTotals.length > 0 ? areaTotals : [{ unit: scale?.unit ?? "pt", total: 0 }] as t (t.unit)}
          <tr class="totals-row" data-total="area" data-unit={t.unit}>
            <td colspan="2"></td>
            <td class="num-col">{t.total.toFixed(2)}</td>
            <td>{t.unit}² (area)</td>
            <td></td>
          </tr>
        {/each}
        {#each countGroups as g (g.setId ?? "__unassigned")}
          {@const r = swatch(g.symbol)}
          <tr class="totals-row count-subtotal">
            <td colspan="2">
              <span class="set-cell">
                <svg width={SWATCH} height={SWATCH} viewBox={`0 0 ${SWATCH} ${SWATCH}`} aria-hidden="true">
                  {#if r.shape === "circle"}
                    <circle cx={r.cx} cy={r.cy} r={r.r} fill={g.color} />
                  {:else if r.shape === "polygon"}
                    <polygon points={r.points} fill={g.color} stroke-linejoin="round" />
                  {:else if r.shape === "cross"}
                    {#each r.lines as ln, i (i)}
                      <line x1={ln.x1} y1={ln.y1} x2={ln.x2} y2={ln.y2}
                        stroke={g.color} stroke-width="2" stroke-linecap="round" />
                    {/each}
                  {/if}
                </svg>
                {g.name}
              </span>
            </td>
            <td class="num-col">{g.count}</td>
            <td>ea (count)</td>
            <td></td>
          </tr>
          {#each g.byPage as bp (bp.page)}
            <tr class="count-page-row">
              <td></td>
              <td class="page-label">Page {bp.page + 1}</td>
              <td class="num-col">{bp.count}</td>
              <td>ea</td>
              <td></td>
            </tr>
          {/each}
        {/each}
        {#if countGroups.length > 1}
          <tr class="totals-row count-grand-total">
            <td colspan="2">All counts</td>
            <td class="num-col">{totalCount}</td>
            <td>ea (count)</td>
            <td></td>
          </tr>
        {/if}
      </tfoot>
    </table>
  {/if}
</div>

<style>
  .actions-cell { white-space: nowrap; }
  .viewports { display: flex; flex-direction: column; gap: var(--space-1); flex-shrink: 0; }
  .viewports-head { display: flex; align-items: center; justify-content: space-between; }
  .section-title { font-size: var(--font-size-sm); font-weight: 600; color: var(--color-text); }
  .row-action:disabled { opacity: 0.45; cursor: not-allowed; }
  .row-action {
    font-size: var(--font-size-xs);
    padding: 0 var(--space-2);
    margin-right: var(--space-1);
    background: var(--color-bg-active);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .measurement-panel {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    overflow: auto;
    height: 100%;
  }

  .panel-actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }

  .scale-badge {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    padding: var(--space-1) var(--space-2);
  }

  .btn-export {
    padding: var(--space-1) var(--space-3);
    background: var(--color-bg-active);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: var(--font-size-sm);
  }

  .btn-export:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }

  .btn-export:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .export-ok {
    padding: var(--space-1) var(--space-3);
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
  }
  .export-error {
    color: var(--color-error, #e02424);
    font-size: var(--font-size-sm);
  }

  .empty-hint {
    color: var(--color-text-muted);
    font-size: var(--font-size-sm);
    margin: 0;
  }

  .quantity-table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--font-size-sm);
  }

  .quantity-table th,
  .quantity-table td {
    text-align: left;
    padding: var(--space-1) var(--space-2);
    border-bottom: 1px solid var(--color-border);
  }

  .quantity-table th {
    background: var(--color-bg-toolbar);
    color: var(--color-text-muted);
    font-weight: 600;
  }

  .num-col {
    text-align: right;
    font-variant-numeric: tabular-nums;
    font-family: monospace;
  }

  .type-cell {
    color: var(--color-primary);
    font-weight: 500;
  }

  .scale-cell {
    color: var(--color-text-muted);
    font-size: var(--font-size-xs, 0.75rem);
  }

  .totals-row td {
    background: var(--color-bg-active);
    font-weight: 600;
    border-top: 2px solid var(--color-border);
  }

  .set-cell {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }

  .set-cell svg {
    display: block;
    flex-shrink: 0;
  }

  .count-grand-total td {
    border-top: 2px solid var(--color-primary);
  }

  .count-page-row td {
    background: var(--color-bg-canvas, var(--color-bg));
    font-weight: 400;
    font-size: var(--font-size-xs, 0.75rem);
    color: var(--color-text-muted);
    border-top: none;
  }

  .page-label {
    padding-left: var(--space-6, 1.5rem);
  }
</style>
