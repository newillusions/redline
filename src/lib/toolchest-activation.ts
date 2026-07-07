/**
 * Activating a Tool Chest tool (spec "Tools & Tool Sets" - "the active tool drives markup
 * creation"). Pure w.r.t. IPC: mutates the given `MarkupStore`'s reactive fields directly,
 * no network calls - callers (ToolChestPanel.svelte) separately fire-and-forget the
 * `recordRecent` IPC call, which does not need to block activation.
 */
import type { Tool } from "./ipc";
import type { MarkupStore, ToolKind } from "./markup-store.svelte";

/**
 * `MarkupType` values that already have a drawable `ToolKind` counterpart (i.e. the
 * existing tool palette can create geometry for them). Properties-mode activation maps a
 * tool's `markup_type` onto its matching `ToolKind` 1:1 when the name matches one of these.
 *
 * SIMPLIFICATION (named): `Stamp`, `StampDynamic`, `MeasurementPerimeter`,
 * `MeasurementVolume`, and `MeasurementAngle`/`MeasurementRadius` measurement subtypes have
 * no drawing UI in the tool palette yet (a pre-existing gap, not introduced here) - a
 * Properties-mode tool of one of those types still updates `draftAppearance` (so the
 * appearance is ready the moment a compatible tool exists) but cannot select `activeTool`.
 */
const DRAWABLE_MARKUP_TYPES: ReadonlySet<string> = new Set<ToolKind>([
  "Rectangle", "Ellipse", "Line", "Arrow", "Highlight", "Polyline", "Polygon", "Cloud",
  "Ink", "Text", "Callout", "MeasurementLength", "MeasurementArea", "MeasurementCount",
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
