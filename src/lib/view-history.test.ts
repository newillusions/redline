import { describe, it, expect } from "vitest";
import { ViewHistory, isLargeZoomChange, sameView, MAX_VIEW_HISTORY, type ViewEntry } from "./view-history";

const v = (pageIndex: number, zoom = 1, scrollX = 0, scrollY = 0): ViewEntry => ({ pageIndex, zoom, scrollX, scrollY });

describe("ViewHistory", () => {
  it("starts empty and returns null", () => {
    const h = new ViewHistory();
    expect(h.canBack).toBe(false);
    expect(h.canForward).toBe(false);
    expect(h.back(v(0))).toBeNull();
    expect(h.forward(v(0))).toBeNull();
  });

  it("goes back to the recorded view and forward again", () => {
    const h = new ViewHistory();
    h.record(v(0, 1, 10, 20)); // leaving page 0 for page 5
    const prev = h.back(v(5, 2, 0, 0));
    expect(prev).toEqual(v(0, 1, 10, 20));
    expect(h.canForward).toBe(true);
    const next = h.forward(v(0, 1, 10, 20));
    expect(next).toEqual(v(5, 2, 0, 0));
    expect(h.canBack).toBe(true);
  });

  it("walks several steps in order", () => {
    const h = new ViewHistory();
    h.record(v(0));
    h.record(v(3));
    h.record(v(7));
    expect(h.back(v(9))?.pageIndex).toBe(7);
    expect(h.back(v(7))?.pageIndex).toBe(3);
    expect(h.back(v(3))?.pageIndex).toBe(0);
    expect(h.back(v(0))).toBeNull();
  });

  it("a new jump after going back clears the forward stack", () => {
    const h = new ViewHistory();
    h.record(v(0));
    h.back(v(4));
    expect(h.canForward).toBe(true);
    h.record(v(0));
    expect(h.canForward).toBe(false);
  });

  it("does not record the same view twice in a row", () => {
    const h = new ViewHistory();
    h.record(v(2, 1, 100, 100));
    h.record(v(2, 1.01, 105, 110));
    expect(h.backCount).toBe(1);
    h.record(v(2, 1, 100, 400));
    expect(h.backCount).toBe(2);
  });

  it("caps the stack and drops the oldest", () => {
    const h = new ViewHistory(3);
    for (let i = 0; i < 6; i++) h.record(v(i));
    expect(h.backCount).toBe(3);
    expect(h.back(v(99))?.pageIndex).toBe(5);
    expect(h.back(v(5))?.pageIndex).toBe(4);
    expect(h.back(v(4))?.pageIndex).toBe(3);
    expect(h.back(v(3))).toBeNull();
  });

  it("defaults to a 50-entry cap", () => {
    const h = new ViewHistory();
    for (let i = 0; i < MAX_VIEW_HISTORY + 10; i++) h.record(v(i));
    expect(h.backCount).toBe(MAX_VIEW_HISTORY);
  });

  it("clear empties both stacks", () => {
    const h = new ViewHistory();
    h.record(v(0));
    h.back(v(1));
    h.clear();
    expect(h.canBack || h.canForward).toBe(false);
  });

  it("does not alias the recorded object", () => {
    const h = new ViewHistory();
    const cur = v(1, 1, 5, 5);
    h.record(cur);
    cur.scrollX = 999;
    expect(h.back(v(2))?.scrollX).toBe(5);
  });
});

describe("isLargeZoomChange / sameView", () => {
  it("flags only zoom changes of 1.5x or more", () => {
    expect(isLargeZoomChange(1, 1.4)).toBe(false);
    expect(isLargeZoomChange(1, 1.5)).toBe(true);
    expect(isLargeZoomChange(1, 0.7)).toBe(false);
    expect(isLargeZoomChange(1, 0.6)).toBe(true);
    expect(isLargeZoomChange(1, 0.5)).toBe(true);
    expect(isLargeZoomChange(0, 1)).toBe(false);
  });
  it("treats different pages as different views", () => {
    expect(sameView(v(0), v(1))).toBe(false);
  });
});
