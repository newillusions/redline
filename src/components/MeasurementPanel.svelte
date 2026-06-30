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
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import type { TakeoffStore } from "$lib/takeoff-store.svelte";
  import type { Markup } from "$lib/ipc";
  import { exportMarkupList, type ExportFormat } from "$lib/ipc";
  import { countSubtotalsByPage } from "$lib/measurement-tools";
  import { countSymbolRender, COUNT_MARKER_RADIUS } from "$lib/markup-render";
  import { save as saveDialog } from "@tauri-apps/plugin-dialog";
  import CountSetPicker from "./CountSetPicker.svelte";

  const {
    store,
    takeoffStore,
    docId,
  }: { store: MarkupStore; takeoffStore: TakeoffStore; docId: string } = $props();

  const MEASUREMENT_TYPES = new Set(["MeasurementLength", "MeasurementArea", "MeasurementCount"]);

  const measurements = $derived(
    store.markups.filter((m: Markup) => MEASUREMENT_TYPES.has(m.markup_type))
  );

  const totalLength = $derived(
    measurements
      .filter((m: Markup) => m.markup_type === "MeasurementLength")
      .reduce((sum: number, m: Markup) => sum + (m.measurement?.computed_quantity ?? 0), 0)
  );

  const totalArea = $derived(
    measurements
      .filter((m: Markup) => m.markup_type === "MeasurementArea")
      .reduce((sum: number, m: Markup) => sum + (m.measurement?.computed_quantity ?? 0), 0)
  );

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
  const scaleLabel = $derived(scale ? `${scale.label} (${scale.unit})` : "No scale set");

  let exportError = $state<string | null>(null);

  async function handleExport(format: ExportFormat) {
    exportError = null;
    const ext = format === "Xlsx" ? "xlsx" : "csv";
    const dest = await saveDialog({
      filters: [{ name: format === "Xlsx" ? "Excel" : "CSV", extensions: [ext] }],
    });
    if (!dest) return;
    try {
      await exportMarkupList(docId, dest, format);
    } catch (e) {
      exportError = `Export failed: ${e instanceof Error ? e.message : String(e)}`;
    }
  }
</script>

<div class="measurement-panel">
  <div class="panel-actions">
    <span class="scale-badge" title="Active scale">{scaleLabel}</span>
    <button class="btn-export" onclick={() => handleExport("Xlsx")} disabled={measurements.length === 0}>
      Export XLSX
    </button>
    <button class="btn-export" onclick={() => handleExport("Csv")} disabled={measurements.length === 0}>
      Export CSV
    </button>
  </div>

  {#if exportError}
    <div class="export-error">{exportError}</div>
  {/if}

  <CountSetPicker {store} />

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
        </tr>
      </thead>
      <tbody>
        {#each measurements as m (m.id)}
          <tr>
            <td class="type-cell">{m.markup_type.replace("Measurement", "")}</td>
            <td>{m.contents ?? ""}</td>
            <td class="num-col">
              {m.measurement
                ? m.markup_type === "MeasurementCount"
                  ? (m.measurement.count_value ?? 0).toFixed(0)
                  : m.measurement.computed_quantity.toFixed(m.measurement.scale_ref ? 2 : 1)
                : "-"}
            </td>
            <td>{m.measurement?.unit ?? "-"}</td>
            <td class="scale-cell">
              {m.measurement?.scale_ref ? (scale?.label ?? "-") : "raw"}
            </td>
          </tr>
        {/each}
      </tbody>
      <tfoot>
        <tr class="totals-row">
          <td colspan="2">Totals</td>
          <td class="num-col">{totalLength.toFixed(2)}</td>
          <td>{scale?.unit ?? "pt"} (length)</td>
          <td></td>
        </tr>
        <tr class="totals-row">
          <td colspan="2"></td>
          <td class="num-col">{totalArea.toFixed(2)}</td>
          <td>{scale ? `${scale.unit}²` : "pt²"} (area)</td>
          <td></td>
        </tr>
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
