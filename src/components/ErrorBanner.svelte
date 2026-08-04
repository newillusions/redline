<script lang="ts">
  /**
   * Crash-guard banner. Mounts the global uncaught-error / unhandled-rejection
   * listeners (src/lib/error-surface.ts), writes every occurrence to the persistent
   * Tauri file log (same pasted-log support flow UpdateNotification.svelte already
   * uses for update-check outcomes), and shows a dismissible banner so a frontend
   * exception is never silent - previously an uncaught error left the UI in an
   * unexplained broken state with nothing to paste for debugging.
   */
  import { onMount, onDestroy } from "svelte";
  import { error as logError } from "@tauri-apps/plugin-log";
  import { installGlobalErrorHandlers } from "$lib/error-surface";

  let messages = $state<string[]>([]);
  let uninstall: (() => void) | null = null;

  function record(message: string) {
    void logError(`[crash-guard] ${message}`);
    console.error("[crash-guard]", message);
    // Cap the visible list - a burst of errors from one root cause shouldn't
    // fill the screen; the file log still has the full history.
    messages = [...messages, message].slice(-5);
  }

  function dismiss(index: number) {
    messages = messages.filter((_, i) => i !== index);
  }

  onMount(() => {
    uninstall = installGlobalErrorHandlers(record);
  });

  onDestroy(() => {
    uninstall?.();
  });
</script>

{#if messages.length > 0}
  <div class="error-banner-stack" role="alert" aria-live="assertive">
    {#each messages as message, i (i)}
      <div class="error-banner">
        <span class="error-banner-text">{message}</span>
        <button
          class="error-banner-dismiss"
          onclick={() => dismiss(i)}
          aria-label="Dismiss error"
        >
          ×
        </button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .error-banner-stack {
    position: fixed;
    top: var(--space-3);
    right: var(--space-3);
    z-index: 2000;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 420px;
  }
  .error-banner {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    background: var(--color-danger-bg, var(--color-bg-panel));
    border: 1px solid var(--color-danger);
    border-radius: var(--radius-sm);
    padding: var(--space-2) var(--space-3);
    box-shadow: 0 4px 16px rgba(0 0 0 / 0.25);
  }
  .error-banner-text {
    flex: 1;
    font-size: var(--font-size-sm);
    color: var(--color-text);
    word-break: break-word;
  }
  .error-banner-dismiss {
    flex: 0 0 auto;
    background: none;
    border: none;
    color: var(--color-text-muted);
    font-size: var(--font-size-lg);
    line-height: 1;
    cursor: pointer;
    padding: 0;
  }
  .error-banner-dismiss:hover {
    color: var(--color-text);
  }
</style>
