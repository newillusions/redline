/**
 * Window-close guard helpers (UX audit F2, 2026-10-10). Closing the window with unsaved
 * mark-ups used to discard them silently; App.svelte now intercepts Tauri's
 * close-requested event and walks the dirty tabs one at a time with SavePromptDialog.
 * The queue logic lives here so it is testable without mounting the app.
 */

/** The doc ids (tab order) whose stores hold unsaved changes. */
export function dirtyDocIds(tabs: ReadonlyArray<{ docId: string; store: { dirty: boolean } }>): string[] {
  return tabs.filter((t) => t.store.dirty).map((t) => t.docId);
}

export type QuitDecision = "save" | "discard" | "cancel";

export type QuitStep =
  | { action: "prompt"; queue: string[] }
  | { action: "quit" }
  | { action: "abort" };

/**
 * Advance the quit flow after the user answers the prompt for `queue[0]`.
 * - "cancel" aborts the whole quit (the window stays open).
 * - "save" / "discard" resolve the head; the next dirty tab is prompted, or the window
 *   closes when none remain. The caller performs the actual save/discard side effect.
 */
export function advanceQuit(queue: readonly string[], decision: QuitDecision): QuitStep {
  if (decision === "cancel") return { action: "abort" };
  const rest = queue.slice(1);
  return rest.length > 0 ? { action: "prompt", queue: rest } : { action: "quit" };
}
