import type { Markup } from "./ipc";

/** Test factory for list-model tests (not shipped: only imported from *.test.ts). */
export function mk(id: string, over: Partial<Markup> & { author?: string; created?: string } = {}): Markup {
  const { author = "Ann", created = "2026-01-01T00:00:00Z", ...rest } = over;
  const u = { user_id: "u-" + author, display_name: author };
  return {
    id,
    page: 0,
    markup_type: "Rectangle",
    geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } },
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null },
    subject: null,
    layer: null,
    contents: null,
    group_id: null,
    measurement: null,
    workflow: { status: "None", assignee: null, thread: [] },
    audit: { created_by: u, created_at: created, modified_by: u, modified_at: created, revision: 0, origin: "Desktop" },
    ...rest,
  } as Markup;
}
