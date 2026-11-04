/**
 * Built-in review stamps for the palette's Stamp tool. Each is a Drawing-mode `StampDynamic`
 * Tool with no artwork and no fields: placement composes `base_text` (unchanged) and the
 * backend draws its bordered-box + label appearance, the same path as a saved dynamic stamp
 * (Viewport.placeToolCopy -> completeStampPlacement). They are not persisted in the Tool
 * Chest; saved stamp tools are listed beside them by `savedStampTools`.
 */
import type { Appearance, Tool, ToolSet } from "./ipc";

/** Tool ids are UUIDs on the Rust side (Recent Tools persists them), so built-in ids are
 *  fixed, valid UUIDs: stable across runs so a re-used stamp de-duplicates in Recent. */
const stampId = (n: number) => `5a3b0000-0000-4000-8000-${String(n).padStart(12, "0")}`;

const STAMPS: readonly { label: string; color: string }[] = [
  { label: "Approved", color: "#15803d" },
  { label: "Rejected", color: "#b91c1c" },
  { label: "Revise and Resubmit", color: "#c2410c" },
  { label: "Reviewed", color: "#1d4ed8" },
  { label: "For Information", color: "#475569" },
];

/** Stamp box height in PDF points. */
const STAMP_HEIGHT = 36;

function appearance(color: string): Appearance {
  return {
    color, line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null,
    outline_color: null, fill_opacity: null,
  };
}

export function builtinStamps(): Tool[] {
  return STAMPS.map(({ label, color }, i) => {
    const width = Math.max(110, label.length * 11 + 24);
    return {
      id: stampId(i + 1),
      name: label,
      markup_type: "StampDynamic",
      appearance: appearance(color),
      subject: label,
      placement_mode: "Drawing",
      geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: width, y: STAMP_HEIGHT } } },
      stamp: { Dynamic: { asset: null, fields: [], base_text: label.toUpperCase() } },
      children: [],
    } satisfies Tool;
  });
}

/** Stamp tools (Static or Dynamic) saved in the user's Tool Chest sets. */
export function savedStampTools(sets: readonly ToolSet[]): Tool[] {
  return sets.flatMap((s) =>
    s.tools.filter((t) => (t.markup_type === "Stamp" || t.markup_type === "StampDynamic") && t.geometry !== null),
  );
}
