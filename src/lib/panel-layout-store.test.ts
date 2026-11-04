import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { PanelLayoutStore } from "./panel-layout-store.svelte";
import { defaultLayout, movePanel, setSidebarWidth } from "./panel-layout";

describe("PanelLayoutStore", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("starts at the default layout and adopts a persisted value via load()", () => {
    const persist = vi.fn().mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 100);
    expect(store.layout).toEqual(defaultLayout());
    const saved = movePanel(defaultLayout(), "toolchest", "right", 0);
    store.load(JSON.parse(JSON.stringify(saved)));
    expect(store.layout.right.panels).toEqual(["toolchest", "properties"]);
    vi.advanceTimersByTime(1000);
    expect(persist).not.toHaveBeenCalled();
  });

  it("debounces a burst of updates into one save of the final layout", () => {
    const persist = vi.fn().mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 100);
    store.load(null);
    for (let w = 200; w <= 300; w += 10) store.update((l) => setSidebarWidth(l, "left", w));
    vi.advanceTimersByTime(99);
    expect(persist).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(persist).toHaveBeenCalledTimes(1);
    expect(persist.mock.calls[0][0].left.width).toBe(300);
  });

  it("does not save when an update returns the same layout", () => {
    const persist = vi.fn().mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 100);
    store.load(null);
    store.update((l) => l);
    vi.advanceTimersByTime(1000);
    expect(persist).not.toHaveBeenCalled();
  });

  it("flush() writes a pending change immediately", async () => {
    const persist = vi.fn().mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 10_000);
    store.load(null);
    store.update((l) => setSidebarWidth(l, "right", 320));
    await store.flush();
    expect(persist).toHaveBeenCalledTimes(1);
    expect(persist.mock.calls[0][0].right.width).toBe(320);
  });

  it("reset() returns to the default layout and saves it", async () => {
    const persist = vi.fn().mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 10);
    store.load(null);
    store.update((l) => movePanel(l, "layers", "right", 0));
    store.reset();
    expect(store.layout).toEqual(defaultLayout());
    await store.flush();
    expect(persist.mock.calls.at(-1)?.[0]).toEqual(defaultLayout());
  });

  it("a failing save is reported and not thrown", async () => {
    const onError = vi.fn();
    const store = new PanelLayoutStore(() => Promise.reject(new Error("disk full")), 10, onError);
    store.load(null);
    store.update((l) => setSidebarWidth(l, "left", 300));
    await expect(store.flush()).resolves.toBeUndefined();
    expect(onError).toHaveBeenCalledOnce();
    expect(onError.mock.calls[0][0]).toContain("disk full");
  });

  it("a failed save stays dirty: the next flush retries it and clears the error state", async () => {
    const onError = vi.fn();
    const persist = vi.fn().mockRejectedValueOnce(new Error("locked")).mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 10_000, onError);
    store.load(null);
    store.update((l) => setSidebarWidth(l, "left", 300));
    await store.flush();
    expect(persist).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledOnce();
    await store.flush(); // no new change, but still dirty
    expect(persist).toHaveBeenCalledTimes(2);
    expect(persist.mock.calls[1][0].left.width).toBe(300);
    await store.flush(); // clean now
    expect(persist).toHaveBeenCalledTimes(2);
  });

  it("a failed save is retried with the next change too", async () => {
    const persist = vi.fn().mockRejectedValueOnce(new Error("locked")).mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 100, () => {});
    store.load(null);
    store.update((l) => setSidebarWidth(l, "left", 300));
    await vi.advanceTimersByTimeAsync(100);
    store.update((l) => setSidebarWidth(l, "left", 310));
    await vi.advanceTimersByTimeAsync(100);
    expect(persist).toHaveBeenCalledTimes(2);
    expect(persist.mock.calls[1][0].left.width).toBe(310);
  });

  it("never saves before a load succeeded, so a failed load cannot overwrite the stored layout", async () => {
    const onError = vi.fn();
    const persist = vi.fn().mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 10, onError);
    store.loadFailed(new Error("settings unreadable"));
    expect(onError.mock.calls[0][0]).toContain("settings unreadable");
    store.update((l) => setSidebarWidth(l, "left", 300));
    await vi.advanceTimersByTimeAsync(1000);
    await store.flush();
    expect(persist).not.toHaveBeenCalled();
    expect(store.ready).toBe(false);
  });

  it("an unrecognised stored version is not adopted, reported, and never overwritten", async () => {
    const onError = vi.fn();
    const persist = vi.fn().mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 10, onError);
    store.load({ version: 2, left: { panels: ["layers"], width: 400 } });
    expect(onError).toHaveBeenCalledOnce();
    expect(store.layout).toEqual(defaultLayout());
    store.update((l) => setSidebarWidth(l, "left", 300));
    await store.flush();
    expect(persist).not.toHaveBeenCalled();
  });

  it("a missing stored layout (null) is a successful load and saving then works", async () => {
    const persist = vi.fn().mockResolvedValue(undefined);
    const store = new PanelLayoutStore(persist, 10);
    store.load(null);
    expect(store.ready).toBe(true);
    store.update((l) => setSidebarWidth(l, "left", 300));
    await store.flush();
    expect(persist).toHaveBeenCalledOnce();
  });
});
