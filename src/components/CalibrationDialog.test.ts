// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import CalibrationDialog from "./CalibrationDialog.svelte";

describe("CalibrationDialog", () => {
  it("renders distance input and unit selector", () => {
    const { getByLabelText } = render(CalibrationDialog, {
      props: { pixelDist: 283.0, onConfirm: vi.fn(), onCancel: vi.fn() },
    });
    expect(getByLabelText(/known distance/i)).toBeTruthy();
  });

  it("calls onCancel when Cancel is clicked", async () => {
    const onCancel = vi.fn();
    const { getByRole } = render(CalibrationDialog, {
      props: { pixelDist: 283.0, onConfirm: vi.fn(), onCancel },
    });
    await fireEvent.click(getByRole("button", { name: /cancel/i }));
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("calls onConfirm with ratio when Set Scale is clicked", async () => {
    const onConfirm = vi.fn();
    const { getByLabelText, getByRole } = render(CalibrationDialog, {
      props: { pixelDist: 283.0, onConfirm, onCancel: vi.fn() },
    });
    // pixelDist = 283 pts, knownDist = 283 m → ratio = 1.0 m/pt
    await fireEvent.input(getByLabelText(/known distance/i), { target: { value: "283" } });
    await fireEvent.click(getByRole("button", { name: /set scale/i }));
    expect(onConfirm).toHaveBeenCalledWith(
      expect.objectContaining({ ratio: expect.closeTo(1.0, 5), unit: expect.any(String) })
    );
  });

  it("disables Set Scale when distance is 0 or empty", async () => {
    const { getByRole, getByLabelText } = render(CalibrationDialog, {
      props: { pixelDist: 283.0, onConfirm: vi.fn(), onCancel: vi.fn() },
    });
    await fireEvent.input(getByLabelText(/known distance/i), { target: { value: "0" } });
    expect((getByRole("button", { name: /set scale/i }) as HTMLButtonElement).disabled).toBe(true);
  });
});
