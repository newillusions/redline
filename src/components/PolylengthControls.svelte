<script lang="ts">
  /**
   * Polylength commands for one selected length measurement (docs/count-polylength.md):
   * per-segment length and Rise/drop entry, the total (path + rise/drop under the page's
   * scale), and Split at a vertex. Edits are ONE undo step.
   */
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import type { TakeoffStore } from "$lib/takeoff-store.svelte";
  import type { UserRef } from "$lib/ipc";
  import { scaleFor, segmentLengths, setSegmentRiseDrop, splitPolylengthAt, riseDropTotal } from "$lib/polylength";

  const { store, takeoffStore, identity }: {
    store: MarkupStore;
    takeoffStore: TakeoffStore | undefined;
    identity: UserRef | null;
  } = $props();

  const m = $derived(
    store.selectedMarkups.length === 1 &&
      store.selectedMarkups[0].markup_type === "MeasurementLength" &&
      "Polyline" in store.selectedMarkups[0].geometry &&
      store.selectedMarkups[0].geometry.Polyline.length >= 2
      ? store.selectedMarkups[0]
      : null,
  );
  const pts = $derived(m && "Polyline" in m.geometry ? m.geometry.Polyline : []);
  const scale = $derived(m && takeoffStore ? scaleFor(m, takeoffStore.scales) : null);
  const lengths = $derived(segmentLengths(pts));
  const runs = $derived(m?.measurement?.rise_drop ?? []);
  const unit = $derived(scale?.unit ?? m?.measurement?.unit ?? "pt");
  const total = $derived(m?.measurement?.computed_quantity ?? 0);
  const vertical = $derived(scale ? riseDropTotal(runs, scale.unit) : { total: 0, unconverted: 0 });

  let splitVertex = $state(1);
  let message = $state<string | null>(null);
  const now = () => new Date().toISOString();

  function runFor(segment: number): number {
    return runs.find((r) => r.segment === segment)?.amount ?? 0;
  }

  function onRiseDrop(segment: number, e: Event) {
    if (!m || !identity) return;
    const raw = (e.target as HTMLInputElement).value.trim();
    const amount = raw === "" ? 0 : Number(raw);
    const r = setSegmentRiseDrop(m, segment, amount, unit, scale, identity, now());
    if (!r.ok) {
      message = r.error;
      (e.target as HTMLInputElement).value = String(runFor(segment) || "");
      return;
    }
    store.applyBatch([{ before: m, after: r.markup }]);
    message = null;
  }

  function onSplit() {
    if (!m || !identity) return;
    const r = splitPolylengthAt(m, splitVertex, scale, identity, now(), () => crypto.randomUUID());
    if (!r.ok) {
      message = r.error;
      return;
    }
    store.applyEdits({ updates: [{ before: m, after: r.first }], creates: [r.second] });
    message = "Split into two polylengths.";
  }
</script>

{#if m}
  <section class="prop-group" data-testid="polylength-controls">
    <h3 class="prop-group-title">Polylength</h3>
    <p class="total" data-testid="polylength-total">
      Total: <strong>{total.toFixed(scale?.precision ?? 2)} {m.measurement?.unit ?? unit}</strong>
      {#if runs.length > 0}<span class="muted">(incl. rise/drop {vertical.total.toFixed(scale?.precision ?? 2)})</span>{/if}
    </p>
    {#if vertical.unconverted > 0}
      <p class="warn" role="alert">{vertical.unconverted} rise/drop value(s) are in a unit that cannot be converted to {unit} and are not counted.</p>
    {/if}
    {#if !scale}<p class="muted">No scale set: calibrate to get real-world lengths.</p>{/if}
    <table>
      <thead><tr><th>Seg</th><th>Length</th><th>Rise/drop ({unit})</th></tr></thead>
      <tbody>
        {#each lengths as len, i (i)}
          <tr>
            <td>{i + 1}</td>
            <td>{scale ? (len * scale.ratio).toFixed(scale.precision) : len.toFixed(1)}</td>
            <td>
              <input
                type="number"
                step="any"
                aria-label={`Rise or drop for segment ${i + 1}`}
                value={runFor(i) || ""}
                onchange={(e) => onRiseDrop(i, e)}
                disabled={!identity}
              />
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if pts.length >= 3}
      <div class="split">
        <label for="split-vertex">Split at vertex</label>
        <select id="split-vertex" bind:value={splitVertex}>
          {#each pts.slice(1, -1) as _, i (i)}<option value={i + 1}>{i + 1}</option>{/each}
        </select>
        <button type="button" data-action="split-polylength" onclick={onSplit} disabled={!identity}>Split</button>
      </div>
    {/if}
    {#if message}<p class="muted" role="status">{message}</p>{/if}
  </section>
{/if}

<style>
  .prop-group { display: flex; flex-direction: column; gap: var(--space-1); padding: var(--space-2) 0; border-bottom: 1px solid var(--color-border); }
  .prop-group-title { font-size: var(--font-size-sm); font-weight: 600; color: var(--color-text-muted); margin: 0 0 var(--space-1) 0; text-transform: uppercase; letter-spacing: 0.05em; }
  .total { margin: 0; font-size: var(--font-size-sm); }
  .muted { color: var(--color-text-muted); font-size: var(--font-size-xs); margin: 0; }
  .warn { color: var(--color-warning); font-size: var(--font-size-xs); margin: 0; }
  table { width: 100%; border-collapse: collapse; font-size: var(--font-size-sm); }
  th { text-align: left; color: var(--color-text-muted); font-weight: 500; }
  td input { width: 5.5rem; font-size: var(--font-size-sm); }
  .split { display: flex; align-items: center; gap: var(--space-2); font-size: var(--font-size-sm); }
  .split button { font-size: var(--font-size-sm); padding: var(--space-1) var(--space-2); border: 1px solid var(--color-border); border-radius: var(--radius-sm); background: var(--color-bg-panel-alt); color: var(--color-text); cursor: pointer; }
</style>
