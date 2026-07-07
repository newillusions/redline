<script lang="ts">
  /**
   * Auto-update notification (Tauri updater plugin). Checks for an update a
   * few seconds after launch; on find, shows a modal with release notes and
   * a download-then-relaunch flow. Every check outcome (up-to-date, found,
   * failed) is written to the file log via tauri-plugin-log so a pasted log
   * shows whether the updater ran - silence in the UI means "up to date",
   * never "didn't check".
   */
  import { onMount } from "svelte";
  import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
  import { relaunch } from "@tauri-apps/plugin-process";
  import { info as logInfo, error as logError } from "@tauri-apps/plugin-log";

  let showModal = $state(false);
  let updateInfo = $state<{ version: string; notes: string } | null>(null);
  let downloading = $state(false);
  let downloadProgress = $state(0);
  let downloadedBytes = $state(0);
  let totalBytes = $state(0);
  let readyToInstall = $state(false);
  let error = $state<string | null>(null);
  let updateObject: Update | null = null;

  onMount(() => {
    // Give the app a moment to settle before hitting the network.
    setTimeout(checkForUpdates, 3000);
  });

  async function checkForUpdates() {
    void logInfo("[updater] checking for updates");
    try {
      const update = await check();
      if (update) {
        void logInfo(`[updater] update available: ${update.version} (current ${update.currentVersion})`);
        updateObject = update;
        updateInfo = {
          version: update.version,
          notes: update.body || `New version ${update.version} is available`,
        };
        showModal = true;
      } else {
        void logInfo("[updater] up to date - no newer version on the manifest");
      }
    } catch (e) {
      // Update-check failures are non-fatal and silent in the UI; the file log
      // carries the evidence so a silent failure is distinguishable from up-to-date.
      void logError(`[updater] check failed: ${e instanceof Error ? e.message : String(e)}`);
      console.error("[updater] check failed:", e);
    }
  }

  async function downloadAndInstall() {
    if (!updateObject) return;
    downloading = true;
    error = null;
    try {
      await updateObject.downloadAndInstall((event: DownloadEvent) => {
        switch (event.event) {
          case "Started":
            totalBytes = event.data.contentLength ?? 0;
            downloadedBytes = 0;
            downloadProgress = 0;
            break;
          case "Progress":
            downloadedBytes += event.data.chunkLength;
            if (totalBytes > 0) {
              downloadProgress = Math.round((downloadedBytes / totalBytes) * 100);
            }
            break;
          case "Finished":
            downloadProgress = 100;
            break;
        }
      });
      readyToInstall = true;
      downloading = false;
      void logInfo(`[updater] downloaded + staged ${updateInfo?.version ?? "?"} - awaiting restart`);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      downloading = false;
      void logError(`[updater] download/install failed: ${error}`);
    }
  }

  async function installAndRestart() {
    try {
      await relaunch();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function remindLater() {
    showModal = false;
  }

  function formatBytes(bytes: number): string {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
  }
</script>

{#if showModal && updateInfo}
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <div class="dialog-backdrop" onclick={remindLater} role="presentation">
    <!-- svelte-ignore a11y-click-events-have-key-events -->
    <div
      class="dialog"
      onclick={(e) => e.stopPropagation()}
      role="dialog"
      aria-modal="true"
      aria-label="Update available"
    >
      <h3 class="dialog-title">Update Available</h3>
      <p class="dialog-hint">Version {updateInfo.version}</p>

      <div class="release-notes">{updateInfo.notes}</div>

      {#if error}
        <p class="dialog-error">{error}</p>
      {/if}

      {#if downloading}
        <div class="progress-row">
          <div class="progress-label">
            <span>Downloading...</span>
            <span>{downloadProgress}%</span>
          </div>
          <div class="progress-track">
            <div class="progress-fill" style="width: {downloadProgress}%"></div>
          </div>
          {#if totalBytes > 0}
            <div class="progress-bytes">{formatBytes(downloadedBytes)} / {formatBytes(totalBytes)}</div>
          {/if}
        </div>
      {/if}

      <div class="dialog-actions">
        {#if readyToInstall}
          <button class="btn-primary" onclick={installAndRestart}>Restart Now</button>
        {:else if downloading}
          <button class="btn-secondary" disabled>Downloading...</button>
        {:else}
          <button class="btn-secondary" onclick={remindLater}>Later</button>
          <button class="btn-primary" onclick={downloadAndInstall}>Update Now</button>
        {/if}
      </div>
    </div>
  </div>
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
    min-width: 380px;
    max-width: 480px;
    display: flex; flex-direction: column; gap: var(--space-2);
    box-shadow: 0 8px 32px rgba(0 0 0 / 0.25);
  }
  .dialog-title { margin: 0; font-size: var(--font-size-lg); color: var(--color-text); }
  .dialog-hint { margin: 0; color: var(--color-text-muted); font-size: var(--font-size-sm); }
  .dialog-error { margin: 0; color: var(--color-danger); font-size: var(--font-size-sm); }
  .release-notes {
    background: var(--color-bg-input);
    border-radius: var(--radius-sm);
    padding: var(--space-3);
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
    max-height: 140px;
    overflow-y: auto;
    white-space: pre-wrap;
  }
  .progress-row { display: flex; flex-direction: column; gap: var(--space-1); }
  .progress-label {
    display: flex; justify-content: space-between;
    font-size: var(--font-size-sm); color: var(--color-text-secondary);
  }
  .progress-track {
    width: 100%; height: 6px; border-radius: 3px;
    background: var(--color-bg-input); overflow: hidden;
  }
  .progress-fill { height: 100%; background: var(--color-primary); transition: width 200ms; }
  .progress-bytes { font-size: var(--font-size-xs); color: var(--color-text-muted); text-align: right; }
  .dialog-actions { display: flex; gap: var(--space-2); justify-content: flex-end; margin-top: var(--space-2); }
  .btn-primary {
    padding: var(--space-2) var(--space-4);
    background: var(--color-primary); color: var(--color-text-inverse);
    border: none; border-radius: var(--radius-sm); cursor: pointer; font-size: var(--font-size-base);
  }
  .btn-secondary {
    padding: var(--space-2) var(--space-4);
    background: var(--color-bg-active); color: var(--color-text);
    border: 1px solid var(--color-border); border-radius: var(--radius-sm); cursor: pointer;
    font-size: var(--font-size-base);
  }
  .btn-secondary:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
