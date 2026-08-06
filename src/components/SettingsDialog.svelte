<script lang="ts">
  /**
   * Application settings dialog (spec §15 extension) - theme, default tool,
   * measurement unit, author display name, and (2026-08-05) the License
   * section: view the activation code on file, its current state, re-
   * activate with a new code, or remove the license from this device.
   * Loads current settings + license info on mount, settings persist on
   * Save; the License section acts immediately on each action (there's no
   * "Save" step for it - re-activation/removal are already committed by the
   * backend the moment they succeed). Follows the CalibrationDialog modal
   * pattern.
   */
  import { onMount } from "svelte";
  import { loadSettings, saveSettings, withDefaults } from "$lib/settings";
  import type { AppSettings, Theme, MeasurementUnit } from "$lib/settings";
  import { getLicenseInfo, activateLicense, deactivateLicense } from "$lib/license";
  import type { LicenseInfo, LicenseState } from "$lib/license";
  import ConfirmDialog from "./ConfirmDialog.svelte";

  const {
    onClose,
    onLicenseChanged,
  }: {
    onClose: () => void;
    /** Called whenever this dialog changes the license state (re-activate or
     * remove) so the caller (App.svelte) can update its own gate state -
     * removing the license here must re-lock the app behind ActivationGate. */
    onLicenseChanged?: (state: LicenseState) => void;
  } = $props();

  // --- License section state -----------------------------------------------
  let licenseInfo = $state<LicenseInfo | null>(null);
  let licenseLoading = $state(true);
  let licenseError = $state<string | null>(null);
  let reactivateCode = $state("");
  let reactivating = $state(false);
  let codeCopied = $state(false);
  let showDeactivateConfirm = $state(false);
  let deactivating = $state(false);

  function describeLicenseState(state: LicenseState): string {
    switch (state.state) {
      case "valid":
        return `Active — ${state.staff_id}. ${state.days_remaining} day(s) remaining on the current token; renews automatically each launch.`;
      case "grace": {
        const expired = new Date(state.expired_at).toLocaleDateString(undefined, {
          year: "numeric",
          month: "short",
          day: "numeric",
        });
        return `Offline since ${expired} — running on a grace period, ${state.days_remaining} day(s) left before this device locks.`;
      }
      case "expired":
        return "Expired — this device is locked out. Enter a new activation code below.";
      case "revoked":
        return `Revoked by the administrator (${state.reason}). Enter a new activation code below.`;
      case "invalid":
        return `Could not be verified (${state.reason}). Enter a new activation code below.`;
      case "missing":
        return "Not activated on this device.";
    }
  }

  async function refreshLicenseInfo() {
    try {
      licenseInfo = await getLicenseInfo();
    } catch (e) {
      licenseError = `Failed to load license info: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  async function copyCode() {
    if (!licenseInfo?.code) return;
    try {
      await navigator.clipboard.writeText(licenseInfo.code);
      codeCopied = true;
      setTimeout(() => (codeCopied = false), 1500);
    } catch {
      // Clipboard access can be denied by the OS/webview - the code is still
      // visible in the read-only field for manual copy, so this is silent.
    }
  }

  async function reactivate() {
    const code = reactivateCode.trim();
    if (!code || reactivating) return;
    reactivating = true;
    licenseError = null;
    try {
      const state = await activateLicense(code);
      if (state.state === "valid") {
        reactivateCode = "";
        onLicenseChanged?.(state);
        await refreshLicenseInfo();
      } else if (state.state === "invalid") {
        licenseError = `Activation refused (${state.reason}).`;
      } else {
        licenseError = "Activation did not complete. Check the code and try again.";
      }
    } catch (e) {
      licenseError = e instanceof Error ? e.message : String(e);
    } finally {
      reactivating = false;
    }
  }

  /** Local removal must work offline and always succeed - see
   * `deactivate_license`'s doc comment. The post-removal info refresh is
   * best-effort only: if it fails (e.g. now offline), fall back to a
   * synthetic "missing" bundle built from the already-known-good result
   * rather than leaving stale info on screen or surfacing an error for a
   * step that already succeeded. */
  async function confirmDeactivate() {
    showDeactivateConfirm = false;
    deactivating = true;
    licenseError = null;
    try {
      const state = await deactivateLicense();
      onLicenseChanged?.(state);
      try {
        licenseInfo = await getLicenseInfo();
      } catch {
        licenseInfo = { code: null, device_fingerprint: licenseInfo?.device_fingerprint ?? "", state };
      }
    } catch (e) {
      licenseError = e instanceof Error ? e.message : String(e);
    } finally {
      deactivating = false;
    }
  }

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

    licenseLoading = true;
    await refreshLicenseInfo();
    licenseLoading = false;
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

    <hr class="section-divider" />
    <h4 class="section-title">License</h4>

    {#if licenseLoading}
      <p class="dialog-hint">Loading license info...</p>
    {:else if licenseInfo}
      {#if licenseError}
        <p class="dialog-error">{licenseError}</p>
      {/if}

      <p class="license-status">{describeLicenseState(licenseInfo.state)}</p>

      {#if licenseInfo.code}
        <label class="field-label" for="settings-license-code">Activation code on file</label>
        <div class="license-code-row">
          <input
            id="settings-license-code"
            class="field-input field-full"
            type="text"
            readonly
            value={licenseInfo.code}
          />
          <button class="btn-secondary" onclick={copyCode} type="button">
            {codeCopied ? "Copied" : "Copy"}
          </button>
        </div>
      {/if}

      <label class="field-label" for="settings-license-reactivate">Re-activate with a new code</label>
      <div class="license-code-row">
        <input
          id="settings-license-reactivate"
          class="field-input field-full"
          type="text"
          autocomplete="off"
          spellcheck="false"
          placeholder="Enter activation code"
          bind:value={reactivateCode}
          disabled={reactivating}
        />
        <button
          class="btn-primary"
          onclick={reactivate}
          disabled={reactivating || !reactivateCode.trim()}
          type="button"
        >
          {reactivating ? "Activating..." : "Reactivate"}
        </button>
      </div>

      {#if licenseInfo.code}
        <button
          class="btn-danger"
          onclick={() => (showDeactivateConfirm = true)}
          disabled={deactivating}
          type="button"
        >
          {deactivating ? "Removing..." : "Remove license from this device"}
        </button>
      {/if}
    {/if}

    <div class="dialog-actions">
      <button class="btn-secondary" onclick={onClose} disabled={saving}>Cancel</button>
      <button class="btn-primary" onclick={save} disabled={loading || saving}>
        {saving ? "Saving..." : "Save"}
      </button>
    </div>
  </div>
</div>

{#if showDeactivateConfirm}
  <ConfirmDialog
    title="Remove license from this device?"
    message="This device will be deactivated immediately and locked back to the activation screen. This works offline and cannot be undone from here - you'll need a valid activation code to use Redline again."
    confirmLabel="Yes"
    cancelLabel="No"
    onConfirm={confirmDeactivate}
    onCancel={() => (showDeactivateConfirm = false)}
  />
{/if}

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

  .section-divider {
    border: none;
    border-top: 1px solid var(--color-border);
    margin: var(--space-4) 0 var(--space-2);
  }
  .section-title {
    margin: 0 0 var(--space-2);
    font-size: var(--font-size-sm);
    font-weight: 600;
    color: var(--color-text);
  }
  .license-status {
    margin: 0 0 var(--space-2);
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
    line-height: 1.4;
  }
  .license-code-row {
    display: flex;
    gap: var(--space-2);
    align-items: center;
  }
  .license-code-row .field-input { flex: 1; }
  .btn-danger {
    margin-top: var(--space-3);
    padding: var(--space-2) var(--space-4);
    background: transparent;
    color: var(--color-danger, #dc2626);
    border: 1px solid var(--color-danger, #dc2626);
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: var(--font-size-sm);
  }
  .btn-danger:hover:not(:disabled) { background: var(--color-danger-bg, rgba(220, 38, 38, 0.1)); }
  .btn-danger:disabled { opacity: 0.45; cursor: not-allowed; }
</style>
