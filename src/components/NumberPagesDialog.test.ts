// @vitest-environment jsdom
/** NumberPagesDialog tests (docs/navigation.md L3, L4, RL2). */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import NumberPagesDialog from "./NumberPagesDialog.svelte";

function setup(over: Record<string, unknown> = {}) {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  render(NumberPagesDialog, { pageCount: 10, currentPage: 2, selectedPages: [], onConfirm, onCancel, ...over });
  return { onConfirm, onCancel };
}

describe("NumberPagesDialog", () => {
  it("numbers all pages 1, 2, 3 by default", async () => {
    const { onConfirm } = setup();
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm).toHaveBeenCalledWith({
      pages: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9], style: "D", prefix: "", start: 1,
    });
  });

  it("applies a style, prefix and start to the current page only", async () => {
    const { onConfirm } = setup();
    await fireEvent.change(screen.getByLabelText("Style"), { target: { value: "r" } });
    await fireEvent.input(screen.getByLabelText("Prefix"), { target: { value: "F-" } });
    await fireEvent.input(screen.getByLabelText("Start"), { target: { value: "4" } });
    await fireEvent.click(screen.getByLabelText(/Current/));
    expect(screen.getByText(/F-iv/)).toBeTruthy(); // live preview
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm).toHaveBeenCalledWith({ pages: [2], style: "r", prefix: "F-", start: 4 });
  });

  it("defaults to the selected thumbnails and accepts a custom list", async () => {
    const { onConfirm } = setup({ selectedPages: [4, 5] });
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm.mock.calls[0][0].pages).toEqual([4, 5]);
    await fireEvent.input(screen.getByLabelText("Custom page range"), { target: { value: "1-3, 5, 9" } });
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm.mock.calls[1][0].pages).toEqual([0, 1, 2, 4, 8]);
  });

  it("refuses a custom range outside the document", async () => {
    const { onConfirm } = setup();
    await fireEvent.input(screen.getByLabelText("Custom page range"), { target: { value: "1-99" } });
    expect(screen.getByRole("alert").textContent).toContain("10");
    expect((screen.getByText("OK") as HTMLButtonElement).disabled).toBe(true);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("style none with a blank prefix removes labels; with a prefix it is text only", async () => {
    const { onConfirm } = setup();
    await fireEvent.change(screen.getByLabelText("Style"), { target: { value: "none" } });
    expect(screen.getByText("Remove labels")).toBeTruthy();
    await fireEvent.click(screen.getByText("Remove labels"));
    expect(onConfirm.mock.calls[0][0]).toMatchObject({ style: null, prefix: "" });
    await fireEvent.input(screen.getByLabelText("Prefix"), { target: { value: "Cover" } });
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm.mock.calls[1][0]).toMatchObject({ style: null, prefix: "Cover" });
  });

  it("refuses a start below 1", async () => {
    setup();
    await fireEvent.input(screen.getByLabelText("Start"), { target: { value: "0" } });
    expect((screen.getByText("OK") as HTMLButtonElement).disabled).toBe(true);
  });
});
