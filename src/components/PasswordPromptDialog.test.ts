// @vitest-environment jsdom
/**
 * PasswordPromptDialog component tests.
 *
 * Verifies rendering (filename, error hint), submit/cancel callbacks, the
 * empty-password guard, and Escape/Enter keyboard handling.
 */
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import PasswordPromptDialog from "./PasswordPromptDialog.svelte";

describe("PasswordPromptDialog", () => {
  it("renders with the filename in the prompt text", () => {
    const { getByText } = render(PasswordPromptDialog, {
      props: {
        filename: "plans.pdf",
        errorHint: null,
        onSubmit: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    expect(getByText(/plans\.pdf/)).toBeTruthy();
  });

  it("does not show an error hint on the first prompt", () => {
    const { queryByText } = render(PasswordPromptDialog, {
      props: { filename: "a.pdf", errorHint: null, onSubmit: vi.fn(), onCancel: vi.fn() },
    });
    expect(queryByText(/incorrect/i)).toBeNull();
  });

  it("shows the error hint on a wrong-password retry", () => {
    const { getByText } = render(PasswordPromptDialog, {
      props: {
        filename: "a.pdf",
        errorHint: "Incorrect password. Try again.",
        onSubmit: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    expect(getByText(/incorrect password/i)).toBeTruthy();
  });

  it("typing a password then clicking Open calls onSubmit with it", async () => {
    const onSubmit = vi.fn();
    const { getByRole, getByLabelText } = render(PasswordPromptDialog, {
      props: { filename: "a.pdf", errorHint: null, onSubmit, onCancel: vi.fn() },
    });
    await fireEvent.input(getByLabelText(/pdf password/i), { target: { value: "secret" } });
    await fireEvent.click(getByRole("button", { name: /^open$/i }));
    await tick();
    expect(onSubmit).toHaveBeenCalledWith("secret");
  });

  it("the Open button is disabled while the password field is empty", () => {
    const { getByRole } = render(PasswordPromptDialog, {
      props: { filename: "a.pdf", errorHint: null, onSubmit: vi.fn(), onCancel: vi.fn() },
    });
    expect(getByRole("button", { name: /^open$/i })).toBeDisabled();
  });

  it("clicking Cancel calls onCancel without calling onSubmit", async () => {
    const onCancel = vi.fn();
    const onSubmit = vi.fn();
    const { getByRole } = render(PasswordPromptDialog, {
      props: { filename: "a.pdf", errorHint: null, onSubmit, onCancel },
    });
    await fireEvent.click(getByRole("button", { name: /^cancel$/i }));
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("Escape key calls onCancel", async () => {
    const onCancel = vi.fn();
    const { container } = render(PasswordPromptDialog, {
      props: { filename: "a.pdf", errorHint: null, onSubmit: vi.fn(), onCancel },
    });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Escape",
      code: "Escape",
    });
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("Enter key submits the entered password", async () => {
    const onSubmit = vi.fn();
    const { container, getByLabelText } = render(PasswordPromptDialog, {
      props: { filename: "a.pdf", errorHint: null, onSubmit, onCancel: vi.fn() },
    });
    await fireEvent.input(getByLabelText(/pdf password/i), { target: { value: "secret" } });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Enter",
      code: "Enter",
    });
    await tick();
    expect(onSubmit).toHaveBeenCalledWith("secret");
  });

  it("Enter key with an empty password does not call onSubmit", async () => {
    const onSubmit = vi.fn();
    const { container } = render(PasswordPromptDialog, {
      props: { filename: "a.pdf", errorHint: null, onSubmit, onCancel: vi.fn() },
    });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Enter",
      code: "Enter",
    });
    await tick();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("has a dialog role for accessibility", () => {
    const { getByRole } = render(PasswordPromptDialog, {
      props: { filename: "b.pdf", errorHint: null, onSubmit: vi.fn(), onCancel: vi.fn() },
    });
    expect(getByRole("dialog")).toBeTruthy();
  });
});
