// @vitest-environment jsdom
/**
 * PageIndicator tests — "current / total" display, click-to-edit jump field, Enter
 * commits, Escape discards without committing, invalid input is rejected.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import PageIndicator from "./PageIndicator.svelte";

describe("PageIndicator", () => {
  it("shows the 1-based current page and total", () => {
    render(PageIndicator, { props: { currentPage: 2, pageCount: 10, onjump: vi.fn() } });
    expect(screen.getByTitle("Type a page number and press Enter to jump")).toHaveTextContent("3");
    expect(screen.getByText("/ 10")).toBeTruthy();
  });

  it("shows a dash when no document is open", () => {
    render(PageIndicator, { props: { currentPage: null, pageCount: 0, onjump: vi.fn() } });
    expect(screen.getByTitle("Type a page number and press Enter to jump")).toHaveTextContent("—");
  });

  it("the display button is disabled when there are no pages", () => {
    render(PageIndicator, { props: { currentPage: null, pageCount: 0, onjump: vi.fn() } });
    expect(screen.getByTitle("Type a page number and press Enter to jump")).toBeDisabled();
  });

  it("clicking the display switches to an editable input pre-filled with the current page", async () => {
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 4, pageCount: 10, onjump: vi.fn() } });
    await user.click(screen.getByTitle("Type a page number and press Enter to jump"));
    const input = screen.getByLabelText("Jump to page") as HTMLInputElement;
    expect(input.value).toBe("5");
  });

  it("Enter with a valid page number calls onjump with a 0-based index", async () => {
    const onjump = vi.fn();
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 0, pageCount: 10, onjump } });
    await user.click(screen.getByTitle("Type a page number and press Enter to jump"));
    const input = screen.getByLabelText("Jump to page") as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "7{enter}");
    expect(onjump).toHaveBeenCalledWith(6);
  });

  it("returns to the display (non-editing) state after committing", async () => {
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 0, pageCount: 10, onjump: vi.fn() } });
    await user.click(screen.getByTitle("Type a page number and press Enter to jump"));
    const input = screen.getByLabelText("Jump to page") as HTMLInputElement;
    await user.type(input, "{enter}");
    expect(screen.queryByLabelText("Jump to page")).toBeNull();
  });

  it("invalid input (out of range) does not call onjump", async () => {
    const onjump = vi.fn();
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 0, pageCount: 10, onjump } });
    await user.click(screen.getByTitle("Type a page number and press Enter to jump"));
    const input = screen.getByLabelText("Jump to page") as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "999{enter}");
    expect(onjump).not.toHaveBeenCalled();
  });

  it("invalid input (non-numeric) does not call onjump", async () => {
    const onjump = vi.fn();
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 0, pageCount: 10, onjump } });
    await user.click(screen.getByTitle("Type a page number and press Enter to jump"));
    const input = screen.getByLabelText("Jump to page") as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "abc{enter}");
    expect(onjump).not.toHaveBeenCalled();
  });

  it("Escape discards the edit without calling onjump, even with a valid typed value", async () => {
    const onjump = vi.fn();
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 0, pageCount: 10, onjump } });
    await user.click(screen.getByTitle("Type a page number and press Enter to jump"));
    const input = screen.getByLabelText("Jump to page") as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "5");
    await fireEvent.keyDown(input, { key: "Escape" });
    expect(onjump).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Jump to page")).toBeNull();
  });

  it("blur without pressing Enter still commits a valid value (Viewport zoom-input parity)", async () => {
    const onjump = vi.fn();
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 0, pageCount: 10, onjump } });
    await user.click(screen.getByTitle("Type a page number and press Enter to jump"));
    const input = screen.getByLabelText("Jump to page") as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "3");
    await fireEvent.blur(input);
    expect(onjump).toHaveBeenCalledWith(2);
  });
});
