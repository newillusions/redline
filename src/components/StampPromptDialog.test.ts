// @vitest-environment jsdom
/**
 * StampPromptDialog component tests.
 *
 * Verifies rendering (one input per label), submit/cancel callbacks (values returned in
 * label order), and Escape/Enter keyboard handling.
 */
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import StampPromptDialog from "./StampPromptDialog.svelte";

describe("StampPromptDialog", () => {
  it("renders one labeled input per PromptedText label", () => {
    const { getByLabelText } = render(StampPromptDialog, {
      props: { labels: ["Reason", "Ref #"], onSubmit: vi.fn(), onCancel: vi.fn() },
    });
    expect(getByLabelText("Reason")).toBeTruthy();
    expect(getByLabelText("Ref #")).toBeTruthy();
  });

  it("submits the entered values in label order", async () => {
    const onSubmit = vi.fn();
    const { getByLabelText, getByRole } = render(StampPromptDialog, {
      props: { labels: ["Reason", "Ref #"], onSubmit, onCancel: vi.fn() },
    });
    await fireEvent.input(getByLabelText("Reason"), { target: { value: "fire rating" } });
    await fireEvent.input(getByLabelText("Ref #"), { target: { value: "RFI-12" } });
    await fireEvent.click(getByRole("button", { name: /place stamp/i }));
    await tick();
    expect(onSubmit).toHaveBeenCalledWith(["fire rating", "RFI-12"]);
  });

  it("submits empty strings for fields left blank (no required-field gate)", async () => {
    const onSubmit = vi.fn();
    const { getByRole } = render(StampPromptDialog, {
      props: { labels: ["Reason"], onSubmit, onCancel: vi.fn() },
    });
    await fireEvent.click(getByRole("button", { name: /place stamp/i }));
    await tick();
    expect(onSubmit).toHaveBeenCalledWith([""]);
  });

  it("clicking Cancel calls onCancel without calling onSubmit", async () => {
    const onCancel = vi.fn();
    const onSubmit = vi.fn();
    const { getByRole } = render(StampPromptDialog, {
      props: { labels: ["Reason"], onSubmit, onCancel },
    });
    await fireEvent.click(getByRole("button", { name: /^cancel$/i }));
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("Escape key calls onCancel", async () => {
    const onCancel = vi.fn();
    const { container } = render(StampPromptDialog, {
      props: { labels: ["Reason"], onSubmit: vi.fn(), onCancel },
    });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Escape",
      code: "Escape",
    });
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("Enter key submits the entered values", async () => {
    const onSubmit = vi.fn();
    const { container, getByLabelText } = render(StampPromptDialog, {
      props: { labels: ["Reason"], onSubmit, onCancel: vi.fn() },
    });
    await fireEvent.input(getByLabelText("Reason"), { target: { value: "urgent" } });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Enter",
      code: "Enter",
    });
    await tick();
    expect(onSubmit).toHaveBeenCalledWith(["urgent"]);
  });

  it("has a dialog role for accessibility", () => {
    const { getByRole } = render(StampPromptDialog, {
      props: { labels: ["Reason"], onSubmit: vi.fn(), onCancel: vi.fn() },
    });
    expect(getByRole("dialog")).toBeTruthy();
  });
});
