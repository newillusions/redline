/**
 * Pure drag-to-reorder math for the Tool Chest panel (spec "Tools & Tool Sets" - the
 * backend `reorder_tools` command already exists and is fully tested; this module is
 * just "given a drag from A onto B, what's the new front-to-back id order" so
 * `ToolChestPanel.svelte`'s drag handlers stay thin and this logic stays unit-testable
 * without mounting a component or simulating real HTML5 DataTransfer objects.
 */

/**
 * Move `draggedId` to sit immediately before `targetId` in `ids`, returning a NEW array
 * (input is never mutated). A no-op (returns `ids` as-is) when `draggedId === targetId`
 * or either id is not present - both are defensive guards against a stale/racy drag
 * event, not expected in normal use.
 */
export function reorderAfterDrag(ids: string[], draggedId: string, targetId: string): string[] {
  if (draggedId === targetId) return ids;
  const from = ids.indexOf(draggedId);
  const to = ids.indexOf(targetId);
  if (from === -1 || to === -1) return ids;

  const next = ids.slice();
  next.splice(from, 1);
  const insertAt = next.indexOf(targetId);
  next.splice(insertAt, 0, draggedId);
  return next;
}
