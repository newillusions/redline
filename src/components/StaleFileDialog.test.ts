// @vitest-environment jsdom
/**
 * StaleFileDialog component tests: the changed-on-disk recovery prompt offers both ways to
 * keep the user's mark-ups, and Escape cancels.
 */
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import StaleFileDialog from "./StaleFileDialog.svelte";

function props(over: Record<string, unknown> = {}) {
  return {
    filename: "plan.pdf",
    onRebuild: vi.fn(),
    onReload: vi.fn(),
    onCancel: vi.fn(),
    ...over,
  };
}

describe("StaleFileDialog", () => {
  it("names the file and says nothing was lost", () => {
    const { getByText, getByRole } = render(StaleFileDialog, { props: props() });
    expect(getByRole("dialog")).toBeTruthy();
    expect(getByText(/plan\.pdf was changed by another program/)).toBeTruthy();
    expect(getByText(/not lost/)).toBeTruthy();
  });

  it("Save As (rebuild) and Reload and keep my mark-ups call their handlers", async () => {
    const p = props();
    const { getByRole } = render(StaleFileDialog, { props: p });
    await fireEvent.click(getByRole("button", { name: /save as \(rebuild\)/i }));
    expect(p.onRebuild).toHaveBeenCalledOnce();
    await fireEvent.click(getByRole("button", { name: /reload and keep my mark-ups/i }));
    expect(p.onReload).toHaveBeenCalledOnce();
    expect(p.onCancel).not.toHaveBeenCalled();
  });

  it("disables Save As (rebuild) when redline holds no copy of the loaded file", () => {
    const { getByRole } = render(StaleFileDialog, { props: props({ canRebuild: false }) });
    expect((getByRole("button", { name: /save as \(rebuild\)/i }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("Escape and Cancel cancel", async () => {
    const p = props();
    const { getByRole } = render(StaleFileDialog, { props: p });
    await fireEvent.keyDown(getByRole("dialog"), { key: "Escape" });
    await fireEvent.click(getByRole("button", { name: /^cancel$/i }));
    expect(p.onCancel).toHaveBeenCalledTimes(2);
  });
});
