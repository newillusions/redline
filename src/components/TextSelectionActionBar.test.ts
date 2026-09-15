// @vitest-environment jsdom
/**
 * TextSelectionActionBar component tests (redline#text-select-ux).
 *
 * Explicit-outcome bar for the I-beam text-selection tool - owner feedback,
 * v0.3.22: a completed selection previously left "nothing happening", so this
 * bar's whole job is to make the result visible and actionable. Covers both
 * modes (a real selection vs. a miss) and the three action callbacks.
 */
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import TextSelectionActionBar from "./TextSelectionActionBar.svelte";

describe("TextSelectionActionBar", () => {
  it("selection mode: shows the character count and all three actions", () => {
    const { getByText, getByRole } = render(TextSelectionActionBar, {
      props: {
        x: 10,
        y: 20,
        mode: "selection",
        charCount: 42,
        onCopy: vi.fn(),
        onHighlight: vi.fn(),
        onSearchText: vi.fn(),
      },
    });
    expect(getByText("42 characters selected")).toBeTruthy();
    expect(getByRole("button", { name: /^copy$/i })).toBeTruthy();
    expect(getByRole("button", { name: /^highlight$/i })).toBeTruthy();
    expect(getByRole("button", { name: /^search$/i })).toBeTruthy();
  });

  it("singular character count reads '1 character selected', not '1 characters'", () => {
    const { getByText } = render(TextSelectionActionBar, {
      props: {
        x: 0,
        y: 0,
        mode: "selection",
        charCount: 1,
        onCopy: vi.fn(),
        onHighlight: vi.fn(),
        onSearchText: vi.fn(),
      },
    });
    expect(getByText("1 character selected")).toBeTruthy();
  });

  it("empty mode: shows the no-text message and no action buttons", () => {
    const { getByText, queryByRole } = render(TextSelectionActionBar, {
      props: {
        x: 0,
        y: 0,
        mode: "empty",
        charCount: 0,
        onCopy: vi.fn(),
        onHighlight: vi.fn(),
        onSearchText: vi.fn(),
      },
    });
    expect(getByText(/no text selected/i)).toBeTruthy();
    expect(getByText(/auto-ocr can add one/i)).toBeTruthy();
    expect(queryByRole("button")).toBeNull();
  });

  it("clicking Copy/Highlight/Search calls exactly the matching callback", async () => {
    const onCopy = vi.fn();
    const onHighlight = vi.fn();
    const onSearchText = vi.fn();
    const { getByRole } = render(TextSelectionActionBar, {
      props: { x: 0, y: 0, mode: "selection", charCount: 5, onCopy, onHighlight, onSearchText },
    });

    await fireEvent.click(getByRole("button", { name: /^copy$/i }));
    expect(onCopy).toHaveBeenCalledOnce();
    expect(onHighlight).not.toHaveBeenCalled();
    expect(onSearchText).not.toHaveBeenCalled();

    await fireEvent.click(getByRole("button", { name: /^highlight$/i }));
    expect(onHighlight).toHaveBeenCalledOnce();

    await fireEvent.click(getByRole("button", { name: /^search$/i }));
    expect(onSearchText).toHaveBeenCalledOnce();
  });

  it("positions itself at the given x/y anchor", () => {
    const { container } = render(TextSelectionActionBar, {
      props: { x: 123, y: 45, mode: "selection", charCount: 1, onCopy: vi.fn(), onHighlight: vi.fn(), onSearchText: vi.fn() },
    });
    const bar = container.querySelector(".text-sel-bar") as HTMLElement;
    expect(bar.style.left).toBe("123px");
    expect(bar.style.top).toBe("45px");
  });
});
