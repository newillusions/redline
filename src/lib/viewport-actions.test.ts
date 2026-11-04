import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("./ipc", () => ({ takeScaleNotices: vi.fn() }));
vi.mock("./error-surface", () => ({ surfaceError: vi.fn() }));

import { takeScaleNotices, type ScaleRecord } from "./ipc";
import { surfaceError } from "./error-surface";
import { drainScaleNotices, reportRotateError, viewportDragOutcome } from "./viewport-actions";

function vp(id: string, retired = false): ScaleRecord {
  return {
    id,
    applies_to: { kind: "Viewport", page: 0, viewport_id: id, name: id, bbox: [0, 0, 10, 10] },
    method: "Preset",
    ratio: 0.02,
    unit: "m",
    label: "1:20",
    precision: 2,
    retired,
  } as ScaleRecord;
}

describe("viewportDragOutcome", () => {
  it("a plain viewport drag makes a new viewport", () => {
    expect(viewportDragOutcome("viewport", null, [])).toEqual({ kind: "new" });
  });
  it("a redraw of a live viewport replaces it", () => {
    const a = vp("a");
    expect(viewportDragOutcome("viewportRedraw", "a", [a])).toEqual({ kind: "redraw", old: a });
  });
  it("a redraw whose viewport vanished or was deleted never falls through to New Viewport", () => {
    expect(viewportDragOutcome("viewportRedraw", "gone", [vp("a")])).toEqual({ kind: "vanished" });
    expect(viewportDragOutcome("viewportRedraw", "a", [vp("a", true)])).toEqual({ kind: "vanished" });
    expect(viewportDragOutcome("viewportRedraw", null, [vp("a")])).toEqual({ kind: "vanished" });
  });
});

describe("drainScaleNotices", () => {
  beforeEach(() => vi.clearAllMocks());

  it("shows every queued notice", async () => {
    vi.mocked(takeScaleNotices).mockResolvedValue(["one", "two"]);
    await drainScaleNotices("d1");
    expect(takeScaleNotices).toHaveBeenCalledWith("d1");
    expect(surfaceError).toHaveBeenCalledTimes(2);
    expect(surfaceError).toHaveBeenCalledWith("one");
  });

  it("a failed read of the notices is swallowed", async () => {
    vi.mocked(takeScaleNotices).mockRejectedValue(new Error("boom"));
    await expect(drainScaleNotices("d1")).resolves.toBeUndefined();
    expect(surfaceError).not.toHaveBeenCalled();
  });
});

describe("reportRotateError", () => {
  it("puts a failed rotation in the error banner with the 1-based page", () => {
    vi.mocked(surfaceError).mockClear();
    reportRotateError(2, new Error("file is locked"));
    expect(surfaceError).toHaveBeenCalledWith("Could not rotate page 3: file is locked");
  });
});
