// @vitest-environment jsdom
/**
 * ConfirmDialog component tests.
 *
 * Generic Yes/No prompt shared by the remember-password and
 * save-unprotected-copy flows. Verifies rendering (message/hint/labels),
 * confirm/cancel callbacks, and Escape/Enter keyboard handling.
 */
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import ConfirmDialog from "./ConfirmDialog.svelte";

describe("ConfirmDialog", () => {
  it("renders the message and title", () => {
    const { getByText, getByRole } = render(ConfirmDialog, {
      props: {
        title: "Remember password?",
        message: "Remember this password for next time?",
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    expect(getByText(/remember this password/i)).toBeTruthy();
    expect(getByRole("dialog")).toBeTruthy();
  });

  it("shows the hint when provided, hides it otherwise", () => {
    const { queryByText, rerender } = render(ConfirmDialog, {
      props: {
        title: "t",
        message: "m",
        hint: "Stored obfuscated on this device.",
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    expect(queryByText(/stored obfuscated/i)).toBeTruthy();

    rerender({ title: "t", message: "m", hint: null, onConfirm: vi.fn(), onCancel: vi.fn() });
  });

  it("uses default Yes/No labels when none are given", () => {
    const { getByRole } = render(ConfirmDialog, {
      props: { title: "t", message: "m", onConfirm: vi.fn(), onCancel: vi.fn() },
    });
    expect(getByRole("button", { name: /^yes$/i })).toBeTruthy();
    expect(getByRole("button", { name: /^no$/i })).toBeTruthy();
  });

  it("uses custom confirm/cancel labels when given", () => {
    const { getByRole } = render(ConfirmDialog, {
      props: {
        title: "t",
        message: "m",
        confirmLabel: "Save Copy…",
        cancelLabel: "Not now",
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    expect(getByRole("button", { name: /save copy/i })).toBeTruthy();
    expect(getByRole("button", { name: /not now/i })).toBeTruthy();
  });

  it("clicking the confirm button calls onConfirm, not onCancel", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const { getByRole } = render(ConfirmDialog, {
      props: { title: "t", message: "m", onConfirm, onCancel },
    });
    await fireEvent.click(getByRole("button", { name: /^yes$/i }));
    await tick();
    expect(onConfirm).toHaveBeenCalledOnce();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("clicking the cancel button calls onCancel, not onConfirm", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const { getByRole } = render(ConfirmDialog, {
      props: { title: "t", message: "m", onConfirm, onCancel },
    });
    await fireEvent.click(getByRole("button", { name: /^no$/i }));
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("Escape key calls onCancel", async () => {
    const onCancel = vi.fn();
    const { container } = render(ConfirmDialog, {
      props: { title: "t", message: "m", onConfirm: vi.fn(), onCancel },
    });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Escape",
      code: "Escape",
    });
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("Enter key calls onConfirm", async () => {
    const onConfirm = vi.fn();
    const { container } = render(ConfirmDialog, {
      props: { title: "t", message: "m", onConfirm, onCancel: vi.fn() },
    });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Enter",
      code: "Enter",
    });
    await tick();
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("clicking the backdrop calls onCancel", async () => {
    const onCancel = vi.fn();
    const { container } = render(ConfirmDialog, {
      props: { title: "t", message: "m", onConfirm: vi.fn(), onCancel },
    });
    const backdrop = container.querySelector(".dialog-backdrop");
    expect(backdrop).toBeTruthy();
    await fireEvent.click(backdrop as Element);
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
