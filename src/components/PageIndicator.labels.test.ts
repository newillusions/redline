// @vitest-environment jsdom
/** PageIndicator with page labels (docs/navigation.md RL1): shown beside the number; a label can be typed. */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import PageIndicator from "./PageIndicator.svelte";

const labels = ["", "E-101", "E-102", ""];
const title = "Type a page number and press Enter to jump";

describe("PageIndicator labels", () => {
  it("shows the current page's label beside the number", () => {
    render(PageIndicator, { props: { currentPage: 1, pageCount: 4, labels, onjump: vi.fn() } });
    expect(screen.getByTitle(title)).toHaveTextContent("2");
    expect(screen.getByTestId("page-label")).toHaveTextContent("E-101");
  });

  it("shows no label for an unlabelled page or a document without labels", () => {
    const { unmount } = render(PageIndicator, { props: { currentPage: 0, pageCount: 4, labels, onjump: vi.fn() } });
    expect(screen.queryByTestId("page-label")).toBeNull();
    unmount();
    render(PageIndicator, { props: { currentPage: 1, pageCount: 4, onjump: vi.fn() } });
    expect(screen.queryByTestId("page-label")).toBeNull();
  });

  it("typing a label jumps to that page", async () => {
    const onjump = vi.fn();
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 0, pageCount: 4, labels, onjump } });
    await user.click(screen.getByTitle(title));
    const input = screen.getByLabelText("Jump to page");
    await user.clear(input);
    await user.type(input, "e-102{Enter}");
    expect(onjump).toHaveBeenCalledWith(2);
  });

  it("typing a number still jumps by position", async () => {
    const onjump = vi.fn();
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 0, pageCount: 4, labels, onjump } });
    await user.click(screen.getByTitle(title));
    const input = screen.getByLabelText("Jump to page");
    await user.clear(input);
    await user.type(input, "4{Enter}");
    expect(onjump).toHaveBeenCalledWith(3);
  });

  it("an unknown label is rejected without jumping", async () => {
    const onjump = vi.fn();
    const user = userEvent.setup();
    render(PageIndicator, { props: { currentPage: 0, pageCount: 4, labels, onjump } });
    await user.click(screen.getByTitle(title));
    const input = screen.getByLabelText("Jump to page");
    await user.clear(input);
    await user.type(input, "E-999{Enter}");
    expect(onjump).not.toHaveBeenCalled();
    await fireEvent.keyDown(document.body, { key: "Escape" });
  });
});
