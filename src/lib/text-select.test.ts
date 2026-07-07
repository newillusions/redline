import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { charIndexAtPoint, getTextSelection, selectionRange } from "./text-select";

/**
 * Casing-assertion guard for the two new text-selection IPC wrappers (Tauri v2
 * maps JS camelCase invoke keys to Rust snake_case command params - a raw
 * snake_case key makes Tauri reject the call at runtime). Mirrors the pattern
 * established in ipc.test.ts (regression guard for the 2026-06-15 GUI incident).
 */
const mockInvoke = vi.mocked(invoke);

describe("text-select ipc wrappers (Tauri v2 camelCase keys)", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined as never);
  });

  it("char_index_at_point -> docId / pageIndex / x / y / tolerance", async () => {
    mockInvoke.mockResolvedValue(42 as never);
    const result = await charIndexAtPoint("d1", 3, 100.5, 200.25, 4);
    expect(mockInvoke).toHaveBeenCalledWith("char_index_at_point", {
      docId: "d1",
      pageIndex: 3,
      x: 100.5,
      y: 200.25,
      tolerance: 4,
    });
    expect(result).toBe(42);
  });

  it("char_index_at_point -> returns null when no character is within tolerance", async () => {
    mockInvoke.mockResolvedValue(null as never);
    const result = await charIndexAtPoint("d1", 0, 0, 0, 2);
    expect(result).toBeNull();
  });

  it("get_text_selection -> docId / pageIndex / start / end", async () => {
    const sel = { quads: [], text: "hello" };
    mockInvoke.mockResolvedValue(sel as never);
    const result = await getTextSelection("d1", 2, 5, 12);
    expect(mockInvoke).toHaveBeenCalledWith("get_text_selection", {
      docId: "d1",
      pageIndex: 2,
      start: 5,
      end: 12,
    });
    expect(result).toEqual(sel);
  });
});

// ---------------------------------------------------------------------------
// selectionRange (pure gesture math - no DOM, no Tauri)
// ---------------------------------------------------------------------------

describe("selectionRange", () => {
  it("forward drag: anchor before focus", () => {
    expect(selectionRange(5, 12)).toEqual({ start: 5, end: 13 });
  });

  it("backward drag: focus before anchor is normalized (swapped)", () => {
    expect(selectionRange(12, 5)).toEqual({ start: 5, end: 13 });
  });

  it("single-character selection: anchor === focus yields a 1-char range", () => {
    expect(selectionRange(7, 7)).toEqual({ start: 7, end: 8 });
  });

  it("range width equals the number of characters covered", () => {
    const { start, end } = selectionRange(0, 9);
    expect(end - start).toBe(10);
  });
});
