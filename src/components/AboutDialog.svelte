<script lang="ts">
  /**
   * About page (owner-reported gap: "the app's current version isn't
   * findable anywhere in the UI"). Shows the live app version, a manual
   * "Check for Updates" action, and - "at least during dev stages" - a
   * rollback affordance that lists past releases and can install any of
   * them.
   *
   * Rollback is a REAL downgrade, not a relabeled reinstall of latest: see
   * `src-tauri/src/updater_rollback.rs`'s module doc for the mechanism
   * (each release's `update.json` - including its CI-signed platform
   * signatures - stays reachable forever at the exact commit that published
   * it; rolling back points a custom updater endpoint at that commit-pinned
   * manifest instead of the branch-head one). This is shipped ungated
   * (visible in every build, not just `cargo tauri dev`) because "dev
   * stages" here reads as the product's current internal/pre-1.0 lifecycle,
   * not a compile-time debug flag - there is no customer-facing release
   * yet. Revisit the gating once redline has an external user base.
   */
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { check as checkForUpdate, type Update } from "@tauri-apps/plugin-updater";
  import { relaunch } from "@tauri-apps/plugin-process";
  import { listAvailableReleases, rollbackToVersion, type ReleaseInfo } from "$lib/updater-rollback";
  import ConfirmDialog from "./ConfirmDialog.svelte";

  const { onClose }: { onClose: () => void } = $props();

  // --- App info ---------------------------------------------------------
  let appVersion = $state<string | null>(null);

  // --- Check for updates --------------------------------------------------
  type CheckStatus = "idle" | "checking" | "up-to-date" | "available" | "error";
  let checkStatus = $state<CheckStatus>("idle");
  let checkError = $state<string | null>(null);
  let foundUpdate = $state<Update | null>(null);
  let installing = $state(false);
  let installError = $state<string | null>(null);
  let installed = $state(false);

  async function runCheckForUpdates() {
    checkStatus = "checking";
    checkError = null;
    try {
      const update = await checkForUpdate();
      if (update) {
        foundUpdate = update;
        checkStatus = "available";
      } else {
        checkStatus = "up-to-date";
      }
    } catch (e) {
      checkError = e instanceof Error ? e.message : String(e);
      checkStatus = "error";
    }
  }

  async function installFoundUpdate() {
    if (!foundUpdate) return;
    installing = true;
    installError = null;
    try {
      await foundUpdate.downloadAndInstall();
      installed = true;
    } catch (e) {
      installError = e instanceof Error ? e.message : String(e);
    } finally {
      installing = false;
    }
  }

  // --- Release history / rollback -----------------------------------------
  let releases = $state<ReleaseInfo[]>([]);
  let releasesLoading = $state(true);
  let releasesError = $state<string | null>(null);

  let rollbackTarget = $state<ReleaseInfo | null>(null);
  let rollbackBusy = $state(false);
  let rollbackError = $state<string | null>(null);
  let rollbackDone = $state(false);

  async function loadReleases() {
    releasesLoading = true;
    releasesError = null;
    try {
      releases = await listAvailableReleases();
    } catch (e) {
      releasesError = e instanceof Error ? e.message : String(e);
    } finally {
      releasesLoading = false;
    }
  }

  function requestRollback(release: ReleaseInfo) {
    rollbackError = null;
    rollbackTarget = release;
  }

  async function confirmRollback() {
    if (!rollbackTarget) return;
    const target = rollbackTarget;
    rollbackTarget = null;
    rollbackBusy = true;
    rollbackError = null;
    try {
      await rollbackToVersion(target.manifest_url, target.version);
      rollbackDone = true;
    } catch (e) {
      rollbackError = e instanceof Error ? e.message : String(e);
    } finally {
      rollbackBusy = false;
    }
  }

  async function relaunchNow() {
    try {
      await relaunch();
    } catch (e) {
      rollbackError = e instanceof Error ? e.message : String(e);
      installError = e instanceof Error ? e.message : String(e);
    }
  }

  /** Format an RFC3339 timestamp as a readable local date - mirrors
   * VersionPanel.svelte's fmtDate. */
  function fmtDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
    } catch {
      return iso;
    }
  }

  onMount(async () => {
    appVersion = await getVersion();
    await loadReleases();
  });

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onClose();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y-click-events-have-key-events -->
<div class="dialog-backdrop" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label="About Redline">
    <h3 class="dialog-title">About Redline</h3>

    <p class="version-line">Version {appVersion ?? "…"}</p>
    <p class="channel-line">Update channel: GitHub — newillusions/redline (main branch manifest)</p>

    <hr class="section-divider" />
    <h4 class="section-title">Updates</h4>

    {#if checkStatus === "idle"}
      <button class="btn-secondary" onclick={runCheckForUpdates} type="button">Check for Updates</button>
    {:else if checkStatus === "checking"}
      <p class="dialog-hint">Checking for updates…</p>
    {:else if checkStatus === "up-to-date"}
      <p class="dialog-hint">You're up to date.</p>
      <button class="btn-secondary" onclick={runCheckForUpdates} type="button">Check again</button>
    {:else if checkStatus === "error"}
      <p class="dialog-error">Check failed: {checkError}</p>
      <button class="btn-secondary" onclick={runCheckForUpdates} type="button">Retry</button>
    {:else if checkStatus === "available" && foundUpdate}
      <p class="dialog-hint">v{foundUpdate.currentVersion} &rarr; v{foundUpdate.version} available.</p>
      {#if installError}
        <p class="dialog-error">{installError}</p>
      {/if}
      {#if installed}
        <button class="btn-primary" onclick={relaunchNow} type="button">Restart Now</button>
      {:else}
        <button class="btn-primary" onclick={installFoundUpdate} disabled={installing} type="button">
          {installing ? "Downloading…" : "Download & Install"}
        </button>
      {/if}
    {/if}

    <hr class="section-divider" />
    <h4 class="section-title">Roll back to a previous release</h4>
    <p class="dialog-hint">
      Installs an older build and relaunches. Available while Redline is still in active development - use with
      care.
    </p>

    {#if releasesLoading}
      <p class="dialog-hint">Loading release history…</p>
    {:else if releasesError}
      <p class="dialog-error">Could not load release history: {releasesError}</p>
      <button class="btn-secondary" onclick={loadReleases} type="button">Retry</button>
    {:else if releases.length === 0}
      <p class="dialog-hint">No release history found.</p>
    {:else}
      {#if rollbackError}
        <p class="dialog-error">Rollback failed: {rollbackError}</p>
      {/if}
      {#if rollbackDone}
        <p class="dialog-hint">Installed. Restart to finish rolling back.</p>
        <button class="btn-primary" onclick={relaunchNow} type="button">Restart Now</button>
      {:else}
        <ul class="release-list" role="list">
          {#each releases as release (release.version)}
            <li class="release-item" data-testid="release-item">
              <div class="release-item-info">
                <span class="release-item-version">v{release.version}</span>
                <span class="release-item-date">{fmtDate(release.pub_date)}</span>
              </div>
              <button
                class="btn-danger"
                onclick={() => requestRollback(release)}
                disabled={rollbackBusy}
                type="button"
              >
                {rollbackBusy ? "Working…" : "Roll back"}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}

    <div class="dialog-actions">
      <button class="btn-secondary" onclick={onClose} type="button">Close</button>
    </div>
  </div>
</div>

{#if rollbackTarget}
  <ConfirmDialog
    title="Roll back to v{rollbackTarget.version}?"
    message="This downloads and installs Redline v{rollbackTarget.version}, then requires a restart. Any markups or documents you haven't saved will be lost when it relaunches."
    confirmLabel="Roll back"
    cancelLabel="Cancel"
    onConfirm={confirmRollback}
    onCancel={() => (rollbackTarget = null)}
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
    min-width: 400px;
    max-width: 480px;
    max-height: 80vh;
    overflow-y: auto;
    display: flex; flex-direction: column; gap: var(--space-2);
    box-shadow: 0 8px 32px rgba(0 0 0 / 0.25);
  }
  .dialog-title { margin: 0 0 var(--space-2); font-size: var(--font-size-lg); color: var(--color-text); }
  .dialog-hint { margin: 0; color: var(--color-text-muted); font-size: var(--font-size-sm); }
  .dialog-error { margin: 0; color: var(--color-danger); font-size: var(--font-size-sm); }
  .version-line { margin: 0; color: var(--color-text); font-size: var(--font-size-base); font-weight: 600; }
  .channel-line { margin: 0; color: var(--color-text-muted); font-size: var(--font-size-xs); }

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
  .btn-danger {
    padding: var(--space-1) var(--space-3);
    background: transparent;
    color: var(--color-danger, #dc2626);
    border: 1px solid var(--color-danger, #dc2626);
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: var(--font-size-sm);
    white-space: nowrap;
    flex-shrink: 0;
  }
  .btn-danger:hover:not(:disabled) { background: var(--color-danger-bg, rgba(220, 38, 38, 0.1)); }
  .btn-danger:disabled { opacity: 0.45; cursor: not-allowed; }

  .release-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    max-height: 220px;
    overflow-y: auto;
  }
  .release-item {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
  }
  .release-item-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .release-item-version { font-size: var(--font-size-sm); color: var(--color-text); font-weight: 500; }
  .release-item-date { font-size: var(--font-size-xs); color: var(--color-text-muted); }
</style>
