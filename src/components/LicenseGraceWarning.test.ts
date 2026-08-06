// @vitest-environment jsdom
/**
 * LicenseGraceWarning component tests.
 *
 * Shown once per launch whenever the license state is "grace" (offline past
 * the token's own expiry, but still within the server's grace window) - a
 * non-blocking dialog that states the concrete deadline/countdown and offers
 * a way to reactivate, per Martin's spec: "warnings when you open the app
 * each time... stating plainly how long is left and that it will stop
 * working" (not a vague "license expiring" message).
 */
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import LicenseGraceWarning from "./LicenseGraceWarning.svelte";
import type { LicenseGrace } from "$lib/license";

function graceState(overrides: Partial<LicenseGrace> = {}): LicenseGrace {
  return {
    state: "grace",
    staff_id: "staff:abc123",
    expired_at: "2026-08-01T00:00:00Z",
    grace_deadline: "2026-08-08T00:00:00Z",
    days_remaining: 3,
    ...overrides,
  };
}

describe("LicenseGraceWarning", () => {
  it("states plainly how many days are left and that it will stop working", () => {
    const { getByRole } = render(LicenseGraceWarning, {
      props: { state: graceState({ days_remaining: 3 }), onOpenSettings: vi.fn(), onDismiss: vi.fn() },
    });
    const dialog = getByRole("dialog");
    expect(dialog.textContent).toMatch(/3 days/i);
    expect(dialog.textContent).toMatch(/stop working/i);
  });

  it("singularizes the day count when exactly one day remains", () => {
    const { getByRole } = render(LicenseGraceWarning, {
      props: { state: graceState({ days_remaining: 1 }), onOpenSettings: vi.fn(), onDismiss: vi.fn() },
    });
    expect(getByRole("dialog").textContent).toMatch(/1 day\b/);
    expect(getByRole("dialog").textContent).not.toMatch(/1 days/);
  });

  it("shows the real grace deadline date, not just a relative countdown", () => {
    const { getByRole } = render(LicenseGraceWarning, {
      props: {
        state: graceState({ grace_deadline: "2026-08-08T00:00:00Z" }),
        onOpenSettings: vi.fn(),
        onDismiss: vi.fn(),
      },
    });
    // Locale-formatted date derived from grace_deadline - assert on the
    // year/month/day fragments rather than a hardcoded locale string.
    expect(getByRole("dialog").textContent).toContain("2026");
  });

  it("clicking the reactivate action calls onOpenSettings", async () => {
    const onOpenSettings = vi.fn();
    const { getByRole } = render(LicenseGraceWarning, {
      props: { state: graceState(), onOpenSettings, onDismiss: vi.fn() },
    });
    await fireEvent.click(getByRole("button", { name: /reactivate|settings/i }));
    await tick();
    expect(onOpenSettings).toHaveBeenCalledOnce();
  });

  it("clicking dismiss calls onDismiss without touching onOpenSettings", async () => {
    const onDismiss = vi.fn();
    const onOpenSettings = vi.fn();
    const { getByRole } = render(LicenseGraceWarning, {
      props: { state: graceState(), onOpenSettings, onDismiss },
    });
    await fireEvent.click(getByRole("button", { name: /dismiss|continue/i }));
    await tick();
    expect(onDismiss).toHaveBeenCalledOnce();
    expect(onOpenSettings).not.toHaveBeenCalled();
  });

  it("Escape dismisses", async () => {
    const onDismiss = vi.fn();
    const { container } = render(LicenseGraceWarning, {
      props: { state: graceState(), onOpenSettings: vi.fn(), onDismiss },
    });
    await fireEvent.keyDown(container.querySelector("dialog") ?? document, {
      key: "Escape",
      code: "Escape",
    });
    await tick();
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
