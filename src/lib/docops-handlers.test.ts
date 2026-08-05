// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { runDocOpAndReseed, formatBytes } from "./docops-handlers";
import type { Markup } from "./ipc";

function markup(id: string): Markup {
  return {
    id,
    markup_type: "Rectangle",
    page: 0,
    geometry: { kind: "Rect", min: { x: 0, y: 0 }, max: { x: 1, y: 1 } } as unknown as Markup["geometry"],
    appearance: {
      color: "#e02424",
      line_weight: 2,
      opacity: 1,
      fill: null,
      line_style: "Solid",
      font: null,
      outline_color: null,
      fill_opacity: null,
    },
    subject: null,
    layer: null,
    contents: null,
    group_id: null,
    audit: {
      created_by: { user_id: "u1", display_name: "Alice" },
      created_at: "2026-01-01T00:00:00Z",
      modified_by: { user_id: "u1", display_name: "Alice" },
      modified_at: "2026-01-01T00:00:00Z",
      revision: 0,
      origin: "Desktop",
    },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: null,
    count_set: null,
    stamp_asset: null,
  } as unknown as Markup;
}

describe("runDocOpAndReseed", () => {
  // Regression test for the live report "flatten and optimise don't seem to do
  // anything": flatten_document strips baked annotations from the PDF's /Annots array
  // on the BACKEND, but MarkupStore (the frontend's in-session source of truth) is a
  // plain Svelte $state array that nothing automatically re-syncs after a docops
  // command completes. Without a reseed, the overlay keeps showing the flattened
  // markups as live/selectable, and a later save() would resurrect them as fresh
  // annotations (write_markups always writes whatever the store currently holds).
  it("flushes the store, runs the op, then reseeds the store from the backend's post-op markup list", async () => {
    const calls: string[] = [];
    const flush = vi.fn(async () => {
      calls.push("flush");
    });
    const seed = vi.fn((_m: Markup[]) => {
      calls.push("seed");
    });
    const op = vi.fn(async () => {
      calls.push("op");
    });
    const postOpMarkups = [markup("survivor-1")]; // flatten removed markup "flattened-1"
    const loadMarkups = vi.fn(async (_docId: string) => postOpMarkups);

    await runDocOpAndReseed("doc-1", { flush, seed }, { loadMarkups }, op);

    // Order matters: flush (drain pending edits) -> op (the actual backend mutation) ->
    // reseed from what the backend now reports. Reseeding BEFORE the op would just
    // reload the pre-op state; reseeding never at all is the bug this fixes.
    expect(calls).toEqual(["flush", "op", "seed"]);
    expect(loadMarkups).toHaveBeenCalledWith("doc-1");
    expect(seed).toHaveBeenCalledWith(postOpMarkups);
  });

  it("propagates op's return value to the caller (e.g. flatten's count, optimize's report)", async () => {
    const flush = vi.fn(async () => {});
    const seed = vi.fn();
    const loadMarkups = vi.fn(async () => []);
    const op = vi.fn(async () => 3);

    const result = await runDocOpAndReseed("doc-1", { flush, seed }, { loadMarkups }, op);

    expect(result).toBe(3);
  });

  it("does not call seed if the op rejects (failed ops must not silently reseed as if they succeeded)", async () => {
    const flush = vi.fn(async () => {});
    const seed = vi.fn();
    const op = vi.fn(async () => {
      throw new Error("backend exploded");
    });
    const loadMarkups = vi.fn(async () => [markup("x")]);

    await expect(
      runDocOpAndReseed("doc-1", { flush, seed }, { loadMarkups }, op),
    ).rejects.toThrow("backend exploded");

    expect(seed).not.toHaveBeenCalled();
    expect(loadMarkups).not.toHaveBeenCalled();
  });
});

describe("formatBytes", () => {
  it("renders sub-KiB values in plain bytes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1023)).toBe("1023 B");
  });

  it("renders KiB with one decimal place", () => {
    expect(formatBytes(1024)).toBe("1.0 KiB");
    expect(formatBytes(1536)).toBe("1.5 KiB");
  });

  it("renders MiB for larger values", () => {
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MiB");
  });

  it("renders GiB for very large values", () => {
    expect(formatBytes(2 * 1024 * 1024 * 1024)).toBe("2.0 GiB");
  });
});
