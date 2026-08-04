// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, fireEvent, waitFor } from "@testing-library/svelte";
import ErrorBanner from "./ErrorBanner.svelte";

vi.mock("@tauri-apps/plugin-log", () => ({
  error: vi.fn(async () => {}),
}));

describe("ErrorBanner", () => {
  it("renders nothing when no error has occurred", () => {
    const { container } = render(ErrorBanner);
    expect(container.querySelector(".error-banner")).toBeNull();
  });

  it("shows a banner when an uncaught window error fires", async () => {
    const { container } = render(ErrorBanner);

    window.dispatchEvent(new ErrorEvent("error", { message: "boom" }));

    await waitFor(() => {
      expect(container.textContent).toContain("Uncaught error: boom");
    });
  });

  it("dismisses a banner on click", async () => {
    const { container } = render(ErrorBanner);

    window.dispatchEvent(new ErrorEvent("error", { message: "dismiss me" }));
    await waitFor(() => expect(container.querySelector(".error-banner")).toBeTruthy());

    const dismissBtn = container.querySelector(".error-banner-dismiss") as HTMLButtonElement;
    await fireEvent.click(dismissBtn);

    await waitFor(() => expect(container.querySelector(".error-banner")).toBeNull());
  });

  it("caps the visible banner list at 5", async () => {
    const { container } = render(ErrorBanner);

    for (let i = 0; i < 8; i++) {
      window.dispatchEvent(new ErrorEvent("error", { message: `err ${i}` }));
    }

    await waitFor(() => {
      expect(container.querySelectorAll(".error-banner").length).toBe(5);
    });
  });
});
