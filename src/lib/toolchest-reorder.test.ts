import { describe, it, expect } from "vitest";
import { reorderAfterDrag } from "./toolchest-reorder";

describe("reorderAfterDrag", () => {
  it("moves the dragged id to sit immediately before the target id", () => {
    expect(reorderAfterDrag(["a", "b", "c"], "c", "a")).toEqual(["c", "a", "b"]);
  });

  it("moving an item later in the list drops it before the target, closing the gap", () => {
    expect(reorderAfterDrag(["a", "b", "c", "d"], "a", "c")).toEqual(["b", "a", "c", "d"]);
  });

  it("dragging onto itself is a no-op (same order returned)", () => {
    expect(reorderAfterDrag(["a", "b", "c"], "b", "b")).toEqual(["a", "b", "c"]);
  });

  it("unknown draggedId returns the original order unchanged", () => {
    expect(reorderAfterDrag(["a", "b", "c"], "zzz", "a")).toEqual(["a", "b", "c"]);
  });

  it("unknown targetId returns the original order unchanged", () => {
    expect(reorderAfterDrag(["a", "b", "c"], "a", "zzz")).toEqual(["a", "b", "c"]);
  });

  it("does not mutate the input array", () => {
    const ids = ["a", "b", "c"];
    reorderAfterDrag(ids, "c", "a");
    expect(ids).toEqual(["a", "b", "c"]);
  });

  it("single-item list is unchanged", () => {
    expect(reorderAfterDrag(["a"], "a", "a")).toEqual(["a"]);
  });
});
