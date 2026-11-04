/**
 * Activating a Tool Chest tool (spec "Tools & Tool Sets" - "the active tool drives markup
 * creation"). Pure w.r.t. IPC: mutates the given `MarkupStore`'s reactive fields directly,
 * no network calls - callers (ToolChestPanel.svelte) separately fire-and-forget the
 * `recordRecent` IPC call, which does not need to block activation.
 */
import type { Tool, ToolSet } from "./ipc";
import { NUMBERED_TOOL_COUNT } from "./tool-keys";
import type { MarkupStore, ToolKind } from "./markup-store.svelte";

/**
 * `MarkupType` values that already have a drawable `ToolKind` counterpart (i.e. the
 * existing tool palette can create geometry for them). Properties-mode activation maps a
 * tool's `markup_type` onto its matching `ToolKind` 1:1 when the name matches one of these.
 *
 * SIMPLIFICATION (named): `Stamp`/`StampDynamic` have no drawing UI in the tool palette yet
 * (a pre-existing gap, not introduced here) - a Properties-mode tool of one of those types
 * still updates `draftAppearance` (so the appearance is ready the moment a compatible tool
 * exists) but cannot select `activeTool`. The four measurement subtypes formerly listed
 * here as gaps (`MeasurementPerimeter`, `MeasurementVolume`, `MeasurementAngle`,
 * `MeasurementRadius`) now have real ToolPalette entries - see Viewport.svelte.
 */
const DRAWABLE_MARKUP_TYPES: ReadonlySet<string> = new Set<ToolKind>([
  "Rectangle", "Ellipse", "Line", "Arrow", "Highlight", "Polyline", "Polygon", "Cloud",
  "Ink", "Text", "Callout", "MeasurementLength", "MeasurementArea", "MeasurementCount",
  "MeasurementPerimeter", "MeasurementVolume", "MeasurementAngle", "MeasurementRadius",
  "Underline", "StrikeOut", "Squiggly", "Note",
]);

/**
 * Activate `tool` against `store` (spec "Tools & Tool Sets" - two placement modes):
 *  - Properties mode: apply the tool's saved appearance to newly drawn geometry - sets
 *    `draftAppearance` and, when the tool's type has a matching draw tool, `activeTool`.
 *  - Drawing mode: arm `pendingPlacementTool` and switch to the `placeTool` tool kind, so
 *    the next click in the viewport drops an exact (translated) copy of the tool's fixed
 *    geometry (see `translateToolGeometry` in markup-tools.ts and Viewport's
 *    onOverlayClick).
 */
export function activateTool(tool: Tool, store: MarkupStore): void {
  if (tool.placement_mode === "Drawing") {
    store.pendingPlacementTool = tool;
    store.activeTool = "placeTool";
    return;
  }

  // Properties mode.
  store.draftAppearance = {
    ...tool.appearance,
    font: tool.appearance.font ? { ...tool.appearance.font } : tool.appearance.font,
  };
  if (DRAWABLE_MARKUP_TYPES.has(tool.markup_type)) {
    store.pendingPlacementTool = null;
    store.activeTool = tool.markup_type as ToolKind;
  }
}

/**
 * The tools that answer to the digit keys 1 to 9: the first nine tools of the first Tool Set
 * (Revu assigns "My Tools" hot keys by position; Redline uses the first set so the keys are
 * predictable and the panel can number them).
 */
export function numberedTools(sets: readonly ToolSet[]): Tool[] {
  return (sets[0]?.tools ?? []).slice(0, NUMBERED_TOOL_COUNT);
}

/**
 * Activate `tool` and record it as the real, named Recent entry. The change of active tool
 * must not also add a plain duplicate entry (see `MarkupStore.skipRecentKind`), and the skip
 * is set only when the tool kind actually changes: otherwise no watcher run would consume it
 * and it would swallow the user's next real pick.
 */
export function activateAndRecord(
  tool: Tool,
  store: MarkupStore,
  toolChest: { recordRecent(tool: Tool): Promise<void> },
): void {
  const before = store.activeTool;
  activateTool(tool, store);
  store.skipRecentKind = store.activeTool !== before ? store.activeTool : null;
  void toolChest.recordRecent(tool).catch(() => {});
}
