/**
 * Window close-request handling, extracted from App.svelte so the ordering is testable.
 *
 * Tauri 2 awaits an async onCloseRequested handler and only then destroys the window
 * (unless `preventDefault` was called before the handler resolved), so awaiting the
 * layout flush here guarantees a pending panel-layout save lands before the window goes.
 * `preventDefault` may run after the await: Tauri checks it once the handler resolves.
 */
export async function handleCloseRequested(
  event: { preventDefault: () => void },
  deps: { flush: () => Promise<void>; dirtyDocIds: () => string[]; setQuitQueue: (ids: string[]) => void },
): Promise<void> {
  try {
    await deps.flush();
  } catch (e) {
    // flush() reports its own failure; never let it block the window from closing.
    console.error("layout flush on close failed:", e);
  }
  const dirty = deps.dirtyDocIds();
  if (dirty.length === 0) return;
  event.preventDefault();
  deps.setQuitQueue(dirty);
}
