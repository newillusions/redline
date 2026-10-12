// @vitest-environment jsdom
/** NavActionDialog tests: Jump to page (number or label, view) and Open web address. */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import NavActionDialog from "./NavActionDialog.svelte";
import type { NavTarget } from "$lib/navigation-ipc";

function setup(initial: NavTarget | null = null, currentPage: number | null = 1) {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  render(NavActionDialog, {
    title: "New link", pageCount: 5, labels: ["", "E-101", "E-102", "", ""], currentPage, initial, onConfirm, onCancel,
  });
  return { onConfirm, onCancel };
}

describe("NavActionDialog", () => {
  it("starts on the current page (by label) and confirms a page target", async () => {
    const { onConfirm } = setup();
    expect((screen.getByLabelText("Page") as HTMLInputElement).value).toBe("E-101");
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm).toHaveBeenCalledWith({
      kind: "page", page: 1, view: { kind: "xyz", left: null, top: null, zoom: null },
    });
  });

  it("accepts a page number or a label, and a view choice", async () => {
    const { onConfirm } = setup();
    await fireEvent.input(screen.getByLabelText("Page"), { target: { value: "5" } });
    await fireEvent.change(screen.getByLabelText("View"), { target: { value: "fit_width" } });
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm).toHaveBeenLastCalledWith({ kind: "page", page: 4, view: { kind: "fit_h", top: null } });
    await fireEvent.input(screen.getByLabelText("Page"), { target: { value: "e-102" } });
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm.mock.calls.at(-1)![0].page).toBe(2);
  });

  it("refuses a page outside the document", async () => {
    const { onConfirm } = setup();
    await fireEvent.input(screen.getByLabelText("Page"), { target: { value: "9" } });
    expect(screen.getByRole("alert").textContent).toContain("1 to 5");
    expect((screen.getByText("OK") as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("opens a web address only when it is http, https or mailto", async () => {
    const { onConfirm } = setup();
    await fireEvent.click(screen.getByLabelText("Open web address"));
    const url = screen.getByLabelText("Address");
    for (const bad of [
      "javascript:alert(1)", "file:///etc/passwd", "ftp://example.com", "https://", "https://a b", "example.com", "",
      'https://example.com/"x', "https://example.com/<b>", "https://example.com/a|b", "https://exämple.com", "mailto:",
    ]) {
      await fireEvent.input(url, { target: { value: bad } });
      expect((screen.getByText("OK") as HTMLButtonElement).disabled, bad).toBe(true);
    }
    await fireEvent.input(url, { target: { value: " https://example.com/x " } });
    await fireEvent.click(screen.getByText("OK"));
    expect(onConfirm).toHaveBeenCalledWith({ kind: "uri", uri: "https://example.com/x" });
    await fireEvent.input(url, { target: { value: "mailto:me@example.com" } });
    expect((screen.getByText("OK") as HTMLButtonElement).disabled).toBe(false);
  });

  it("pre-fills from the existing target", () => {
    setup({ kind: "uri", uri: "https://example.com/a" });
    expect((screen.getByLabelText("Address") as HTMLInputElement).value).toBe("https://example.com/a");
  });

  it("Escape cancels", async () => {
    const { onCancel } = setup();
    await fireEvent.keyDown(screen.getByRole("dialog", { hidden: true }), { key: "Escape" });
    expect(onCancel).toHaveBeenCalled();
  });
});
