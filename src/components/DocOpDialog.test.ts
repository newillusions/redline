// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import DocOpDialog from "./DocOpDialog.svelte";

describe("DocOpDialog", () => {
  it("names the file and the consequence", () => {
    const { container } = render(DocOpDialog, {
      props: { kind: "flatten", path: "/jobs/plan.pdf", onConfirm: vi.fn(), onCancel: vi.fn() },
    });
    expect(container.textContent).toContain("plan.pdf");
    expect(container.textContent).toMatch(/can no longer be edited/);
  });

  it("defaults to writing a copy (original unchanged)", async () => {
    const onConfirm = vi.fn();
    const { getByRole } = render(DocOpDialog, {
      props: { kind: "flatten", path: "/jobs/plan.pdf", onConfirm, onCancel: vi.fn() },
    });
    expect((getByRole("radio", { name: /write a copy/i }) as HTMLInputElement).checked).toBe(true);
    await fireEvent.click(getByRole("button", { name: "Continue" }));
    expect(onConfirm).toHaveBeenCalledWith("copy");
  });

  it("offers replacing the original, which mentions the automatic snapshot", async () => {
    const onConfirm = vi.fn();
    const { getByRole } = render(DocOpDialog, {
      props: { kind: "redact", path: "/jobs/plan.pdf", onConfirm, onCancel: vi.fn() },
    });
    const replace = getByRole("radio", { name: /replace the original.*snapshot/i });
    await fireEvent.click(replace);
    await fireEvent.click(getByRole("button", { name: "Continue" }));
    expect(onConfirm).toHaveBeenCalledWith("replace");
  });

  it("Cancel and Escape do not confirm", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const { getByRole } = render(DocOpDialog, {
      props: { kind: "optimize", path: "/jobs/plan.pdf", onConfirm, onCancel },
    });
    await fireEvent.click(getByRole("button", { name: "Cancel" }));
    await fireEvent.keyDown(getByRole("dialog"), { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
