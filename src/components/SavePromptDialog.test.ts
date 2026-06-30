// @vitest-environment jsdom
/**
 * SavePromptDialog component tests.
 *
 * Verifies that the dialog renders correctly and fires the right callbacks
 * for each user action (Save, Don't Save, Cancel, Esc key, Enter key).
 */
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import SavePromptDialog from "./SavePromptDialog.svelte";

describe("SavePromptDialog", () => {
  it("renders with the filename in the prompt text", () => {
    const { getByText } = render(SavePromptDialog, {
      props: {
        filename: "myplan.pdf",
        onSave: vi.fn(),
        onDiscard: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    // The dialog should mention the filename somewhere.
    expect(getByText(/myplan\.pdf/)).toBeTruthy();
  });

  it("clicking Save calls onSave", async () => {
    const onSave = vi.fn();
    const { getByRole } = render(SavePromptDialog, {
      props: { filename: "a.pdf", onSave, onDiscard: vi.fn(), onCancel: vi.fn() },
    });
    await fireEvent.click(getByRole("button", { name: /^save$/i }));
    await tick();
    expect(onSave).toHaveBeenCalledOnce();
  });

  it("clicking Don't Save calls onDiscard", async () => {
    const onDiscard = vi.fn();
    const { getByRole } = render(SavePromptDialog, {
      props: { filename: "a.pdf", onSave: vi.fn(), onDiscard, onCancel: vi.fn() },
    });
    await fireEvent.click(getByRole("button", { name: /don.t save/i }));
    await tick();
    expect(onDiscard).toHaveBeenCalledOnce();
  });

  it("clicking Cancel calls onCancel", async () => {
    const onCancel = vi.fn();
    const { getByRole } = render(SavePromptDialog, {
      props: { filename: "a.pdf", onSave: vi.fn(), onDiscard: vi.fn(), onCancel },
    });
    await fireEvent.click(getByRole("button", { name: /^cancel$/i }));
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("Escape key calls onCancel", async () => {
    const onCancel = vi.fn();
    const { container } = render(SavePromptDialog, {
      props: { filename: "a.pdf", onSave: vi.fn(), onDiscard: vi.fn(), onCancel },
    });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Escape",
      code: "Escape",
    });
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("Enter key on the dialog calls onSave", async () => {
    const onSave = vi.fn();
    const { container } = render(SavePromptDialog, {
      props: { filename: "a.pdf", onSave, onDiscard: vi.fn(), onCancel: vi.fn() },
    });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Enter",
      code: "Enter",
    });
    await tick();
    expect(onSave).toHaveBeenCalledOnce();
  });

  it("has a dialog role for accessibility", () => {
    const { getByRole } = render(SavePromptDialog, {
      props: { filename: "b.pdf", onSave: vi.fn(), onDiscard: vi.fn(), onCancel: vi.fn() },
    });
    expect(getByRole("dialog")).toBeTruthy();
  });
});
