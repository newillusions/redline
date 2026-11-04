/**
 * Recent Tools from every entry point (palette click, tool key, Tool Chest): the active
 * draw tool is mirrored into the Tool Chest's Recent list as a Properties-mode Tool carrying
 * the current draft appearance, so re-clicking it in Recent restores both the tool and its
 * look (Revu "automatically remembers recently used markups").
 */
import type { Appearance, MarkupType, Tool } from "./ipc";
import type { ToolKind } from "./markup-store.svelte";

/** Draw tools worth remembering: ToolKind -> the MarkupType a Properties-mode Tool needs,
 *  its Recent label, and a fixed UUID (Rust `Tool.id` is a Uuid). Hand/select/calibrate and
 *  the multi-step measure helpers are navigation or setup, not markup styles. */
const RECENT_KINDS: Partial<Record<ToolKind, { type: MarkupType; name: string; n: number }>> = {
  Rectangle: { type: "Rectangle", name: "Rectangle", n: 1 },
  Ellipse: { type: "Ellipse", name: "Ellipse", n: 2 },
  Line: { type: "Line", name: "Line", n: 3 },
  Arrow: { type: "Arrow", name: "Arrow", n: 4 },
  Highlight: { type: "Highlight", name: "Highlight", n: 5 },
  Polyline: { type: "Polyline", name: "Polyline", n: 6 },
  Polygon: { type: "Polygon", name: "Polygon", n: 7 },
  Cloud: { type: "Cloud", name: "Cloud", n: 8 },
  Ink: { type: "Ink", name: "Pen", n: 9 },
  Text: { type: "Text", name: "Text Box", n: 10 },
  Callout: { type: "Callout", name: "Callout", n: 11 },
  Note: { type: "Note", name: "Sticky Note", n: 12 },
  Underline: { type: "Underline", name: "Underline", n: 13 },
  StrikeOut: { type: "StrikeOut", name: "Strikethrough", n: 14 },
  Squiggly: { type: "Squiggly", name: "Squiggly", n: 15 },
  MeasurementLength: { type: "MeasurementLength", name: "Measure Length", n: 16 },
  MeasurementArea: { type: "MeasurementArea", name: "Measure Area", n: 17 },
  MeasurementCount: { type: "MeasurementCount", name: "Count", n: 18 },
  MeasurementPerimeter: { type: "MeasurementPerimeter", name: "Measure Perimeter", n: 19 },
  MeasurementVolume: { type: "MeasurementVolume", name: "Measure Volume", n: 20 },
  MeasurementAngle: { type: "MeasurementAngle", name: "Measure Angle", n: 21 },
  MeasurementRadius: { type: "MeasurementRadius", name: "Measure Radius", n: 22 },
};

/** The Recent entry for a palette/key tool, or null when the tool is not a markup style. */
export function recentToolFor(kind: ToolKind, appearance: Appearance): Tool | null {
  const r = RECENT_KINDS[kind];
  if (!r) return null;
  return {
    id: `71000000-0000-4000-8000-${r.n.toString(16).padStart(12, "0")}`,
    name: r.name,
    markup_type: r.type,
    appearance: { ...appearance, font: appearance.font ? { ...appearance.font } : null },
    subject: null,
    placement_mode: "Properties",
    geometry: null,
    stamp: null,
    children: [],
  };
}
