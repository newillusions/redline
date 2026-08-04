/**
 * Global uncaught-error / unhandled-rejection surface (crash-guard).
 *
 * No DOM, no Svelte, no side effects beyond the two `window` listeners this module
 * installs on request - the caller (ErrorBanner.svelte) owns presentation (banner UI)
 * and persistence (writing to the Tauri file log via `@tauri-apps/plugin-log`, the
 * same pasted-log support flow `UpdateNotification.svelte` already uses). Without
 * this, an uncaught frontend exception fails silently (no console attached in a
 * release build, no crash report) - the user sees a frozen or subtly broken UI with
 * no trace to paste for debugging.
 */

/** Human-readable single-line summary of an uncaught error or unhandled rejection. */
export function formatUncaughtError(event: ErrorEvent | PromiseRejectionEvent): string {
  if (event instanceof ErrorEvent || "message" in event) {
    const e = event as ErrorEvent;
    const loc = e.filename ? ` (${e.filename}:${e.lineno}:${e.colno})` : "";
    return `Uncaught error: ${e.message}${loc}`;
  }
  const reason = (event as PromiseRejectionEvent).reason;
  const text = reason instanceof Error ? reason.message : String(reason);
  return `Unhandled promise rejection: ${text}`;
}

/**
 * Install `window` listeners for uncaught errors and unhandled promise rejections,
 * calling `onError(formattedMessage)` for each. Returns an uninstall function.
 * Does NOT call `preventDefault()` - the browser/webview's own console logging (dev
 * builds) stays intact; this is an additive surface, not a replacement.
 */
export function installGlobalErrorHandlers(onError: (message: string) => void): () => void {
  const handleError = (event: ErrorEvent) => onError(formatUncaughtError(event));
  const handleRejection = (event: PromiseRejectionEvent) => onError(formatUncaughtError(event));

  window.addEventListener("error", handleError);
  window.addEventListener("unhandledrejection", handleRejection);

  return () => {
    window.removeEventListener("error", handleError);
    window.removeEventListener("unhandledrejection", handleRejection);
  };
}
