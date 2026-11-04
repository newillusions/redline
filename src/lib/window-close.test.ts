import { describe, it, expect, vi } from "vitest";
import { handleCloseRequested } from "./window-close";

describe("handleCloseRequested", () => {
  it("does not resolve until the layout flush has finished", async () => {
    let finishFlush!: () => void;
    const flush = vi.fn(() => new Promise<void>((r) => (finishFlush = r)));
    const event = { preventDefault: vi.fn() };
    let resolved = false;
    const p = handleCloseRequested(event, { flush, dirtyDocIds: () => [], setQuitQueue: vi.fn() }).then(() => {
      resolved = true;
    });
    await Promise.resolve();
    await Promise.resolve();
    expect(flush).toHaveBeenCalledOnce();
    expect(resolved).toBe(false); // Tauri would destroy the window as soon as this resolves
    finishFlush();
    await p;
    expect(resolved).toBe(true);
    expect(event.preventDefault).not.toHaveBeenCalled();
  });

  it("prevents close and queues the prompt when tabs are dirty, after the flush", async () => {
    const order: string[] = [];
    const event = { preventDefault: vi.fn(() => order.push("prevent")) };
    const setQuitQueue = vi.fn();
    await handleCloseRequested(event, {
      flush: async () => {
        order.push("flush");
      },
      dirtyDocIds: () => ["a", "b"],
      setQuitQueue,
    });
    expect(order).toEqual(["flush", "prevent"]);
    expect(setQuitQueue).toHaveBeenCalledWith(["a", "b"]);
  });

  it("still lets the window close when the flush throws", async () => {
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    const event = { preventDefault: vi.fn() };
    await expect(
      handleCloseRequested(event, { flush: () => Promise.reject(new Error("x")), dirtyDocIds: () => [], setQuitQueue: vi.fn() }),
    ).resolves.toBeUndefined();
    expect(event.preventDefault).not.toHaveBeenCalled();
    err.mockRestore();
  });
});
