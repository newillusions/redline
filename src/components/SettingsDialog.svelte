<script lang="ts">
  /**
   * Application settings dialog (spec §15 extension) - theme, default tool,
   * measurement unit, and author display name. Loads current settings on
   * mount and persists on Save. Follows the CalibrationDialog modal pattern.
   */
  import { onMount } from "svelte";
  import { loadSettings, saveSettings, withDefaults } from "$lib/settings";
  import type { AppSettings, Theme, MeasurementUnit } from "$lib/settings";

  const { onClose }: { onClose: () => void } = $props();

  const DEFAULT_TOOL_OPTIONS: { value: string; label: string }[] = [
    { value: "", label: "Last used" },
    { value: "select", label: "Select / Pointer" },
    { value: "Rectangle", label: "Rectangle" },
    { value: "Ellipse", label: "Ellipse" },
    { value: "Line", label: "Line" },
    { value: "Arrow", label: "Arrow" },
    { value: "Highlight", label: "Highlight" },
    { value: "Polyline", label: "Polyline" },
    { value: "Polygon", label: "Polygon" },
    { value: "Cloud", label: "Cloud" },
    { value: "Ink", label: "Ink (Freehand)" },
    { value: "Text", label: "Text" },
    { value: "Callout", label: "Callout" },
  ];

  let loading = $state(true);
  let saving = $state(false);
  let error = $state<string | null>(null);

  let theme = $state<Theme>("dark");
  let defaultTool = $state("");
  let measurementUnit = $state<MeasurementUnit>("m");
  let authorName = $state("");

  onMount(async () => {
    try {
      const settings = withDefaults(await loadSettings());
      theme = settings.theme;
      defaultTool = settings.default_tool ?? "";
      measurementUnit = settings.measurement_unit;
      authorName = settings.author_name;
    } catch (e) {
      error = `Failed to load settings: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      loading = false;
    }
  });

  async function save() {
    error = null;
    saving = true;
    try {
      const settings: AppSettings = {
        theme,
        default_tool: defaultTool || null,
        measurement_unit: measurementUnit,
        author_name: authorName,
        last_window: null,
        recent_colors: [],
      };
      // Preserve fields this dialog does not edit (last_window, recent_colors)
      // by merging over whatever is currently persisted.
      const current = await loadSettings();
      await saveSettings({ ...current, ...settings });
      onClose();
    } catch (e) {
      error = `Failed to save settings: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      saving = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onClose();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y-click-events-have-key-events -->
<div class="dialog-backdrop" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <div
    class="dialog"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
    aria-modal="true"
    aria-label="Settings"
  >
    <h3 class="dialog-title">Settings</h3>

    {#if loading}
      <p class="dialog-hint">Loading...</p>
    {:else}
      {#if error}
        <p class="dialog-error">{error}</p>
      {/if}

      <label class="field-label" for="settings-theme">Theme</label>
      <select id="settings-theme" class="field-select field-full" bind:value={theme}>
        <option value="dark">Dark</option>
        <option value="light">Light</option>
        <option value="system">System</option>
      </select>

      <label class="field-label" for="settings-default-tool">Default tool</label>
      <select id="settings-default-tool" class="field-select field-full" bind:value={defaultTool}>
        {#each DEFAULT_TOOL_OPTIONS as opt (opt.value)}
          <option value={opt.value}>{opt.label}</option>
        {/each}
      </select>

      <label class="field-label" for="settings-unit">Measurement unit</label>
      <select id="settings-unit" class="field-select field-full" bind:value={measurementUnit}>
        <option value="mm">mm</option>
        <option value="m">m</option>
        <option value="km">km</option>
        <option value="in">in</option>
        <option value="ft">ft</option>
      </select>

      <label class="field-label" for="settings-author">Author display name</label>
      <input
        id="settings-author"
        class="field-input field-full"
        type="text"
        placeholder="Shown on new markups and comments"
        bind:value={authorName}
      />
    {/if}

    <div class="dialog-actions">
      <button class="btn-secondary" onclick={onClose} disabled={saving}>Cancel</button>
      <button class="btn-primary" onclick={save} disabled={loading || saving}>
        {saving ? "Saving..." : "Save"}
      </button>
    </div>
  </div>
</div>

<style>
  .dialog-backdrop {
    position: fixed; inset: 0;
    background: rgba(0 0 0 / 0.45);
    display: flex; align-items: center; justify-content: center;
    z-index: 1000;
  }
  .dialog {
    background: var(--color-bg-panel);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-6);
    min-width: 360px;
    display: flex; flex-direction: column; gap: var(--space-2);
    box-shadow: 0 8px 32px rgba(0 0 0 / 0.25);
  }
  .dialog-title { margin: 0 0 var(--space-2); font-size: var(--font-size-lg); color: var(--color-text); }
  .dialog-hint { margin: 0; color: var(--color-text-muted); font-size: var(--font-size-sm); }
  .dialog-error {
    margin: 0 0 var(--space-2);
    color: var(--color-danger);
    font-size: var(--font-size-sm);
  }
  .field-label { font-size: var(--font-size-sm); color: var(--color-text); margin-top: var(--space-2); }
  .field-select, .field-input {
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--color-border); border-radius: var(--radius-sm);
    background: var(--color-bg-input); color: var(--color-text);
    font-size: var(--font-size-base);
  }
  .field-full { width: 100%; }
  .dialog-actions { display: flex; gap: var(--space-2); justify-content: flex-end; margin-top: var(--space-4); }
  .btn-primary {
    padding: var(--space-2) var(--space-4);
    background: var(--color-primary); color: var(--color-text-inverse);
    border: none; border-radius: var(--radius-sm); cursor: pointer; font-size: var(--font-size-base);
  }
  .btn-primary:disabled { opacity: 0.45; cursor: not-allowed; }
  .btn-secondary {
    padding: var(--space-2) var(--space-4);
    background: var(--color-bg-active); color: var(--color-text);
    border: 1px solid var(--color-border); border-radius: var(--radius-sm); cursor: pointer;
    font-size: var(--font-size-base);
  }
  .btn-secondary:disabled { opacity: 0.45; cursor: not-allowed; }
</style>
