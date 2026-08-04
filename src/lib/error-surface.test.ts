// @vitest-environment jsdom
/**
 * Global uncaught-error / unhandled-rejection surface. Pure formatting logic +
 * the window-listener wiring, kept separate so the formatting can be unit-tested
 * without needing a real ErrorEvent/PromiseRejectionEvent from a browser engine.
 */
import { describe, it, expect, vi } from "vitest";
import { formatUncaughtError, installGlobalErrorHandlers } from "./error-surface";

describe("formatUncaughtError", () => {
  it("formats a window 'error' event with message + source location", () => {
    const event = new ErrorEvent("error", {
      message: "Cannot read properties of null",
      filename: "Viewport.svelte",
      lineno: 42,
      colno: 7,
      error: new Error("Cannot read properties of null"),
    });
    expect(formatUncaughtError(event)).toBe(
      "Uncaught error: Cannot read properties of null (Viewport.svelte:42:7)",
    );
  });

  it("falls back gracefully when a window 'error' event has no source location", () => {
    const event = new ErrorEvent("error", { message: "boom" });
    expect(formatUncaughtError(event)).toBe("Uncaught error: boom");
  });

  it("formats an unhandledrejection event with an Error reason", () => {
    const reason = new Error("fetch failed");
    const event = { reason, promise: Promise.reject(reason).catch(() => {}) } as PromiseRejectionEvent;
    expect(formatUncaughtError(event)).toBe("Unhandled promise rejection: fetch failed");
  });

  it("formats an unhandledrejection event with a non-Error reason", () => {
    const event = { reason: "plain string reason", promise: Promise.resolve() } as unknown as PromiseRejectionEvent;
    expect(formatUncaughtError(event)).toBe("Unhandled promise rejection: plain string reason");
  });
});

describe("installGlobalErrorHandlers", () => {
  it("invokes the callback on a window 'error' event", () => {
    const onError = vi.fn();
    const uninstall = installGlobalErrorHandlers(onError);

    window.dispatchEvent(new ErrorEvent("error", { message: "kaboom" }));

    expect(onError).toHaveBeenCalledWith("Uncaught error: kaboom");
    uninstall();
  });

  it("invokes the callback on an 'unhandledrejection' event", async () => {
    const onError = vi.fn();
    const uninstall = installGlobalErrorHandlers(onError);

    const rejected = Promise.reject(new Error("async boom"));
    rejected.catch(() => {});
    const event = new Event("unhandledrejection") as PromiseRejectionEvent;
    Object.defineProperty(event, "reason", { value: new Error("async boom") });
    Object.defineProperty(event, "promise", { value: rejected });
    window.dispatchEvent(event);

    expect(onError).toHaveBeenCalledWith("Unhandled promise rejection: async boom");
    uninstall();
  });

  it("stops invoking the callback after uninstall", () => {
    const onError = vi.fn();
    const uninstall = installGlobalErrorHandlers(onError);
    uninstall();

    window.dispatchEvent(new ErrorEvent("error", { message: "after uninstall" }));

    expect(onError).not.toHaveBeenCalled();
  });
});
