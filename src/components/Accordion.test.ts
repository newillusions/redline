// @vitest-environment jsdom
/**
 * Accordion tests (owner defect, 2026-09-07: "the recent documents panel
 * needs to be able to accordion - actually, all subpanels in the side
 * panels" - shared collapsible-section component used across App.svelte,
 * ToolChestPanel, and SearchPanel).
 *
 * Covers: uncontrolled mode (self-managed state + sessionStorage
 * persistence), controlled mode (caller-owned state via collapsed/ontoggle),
 * click + explicit Enter/Space keyboard toggling, aria-expanded correctness,
 * and that headerExtra content stays a SIBLING of the toggle button (no
 * nested interactive elements).
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import AccordionHarness from "./__test-fixtures__/AccordionHarness.svelte";

beforeEach(() => {
  try {
    sessionStorage.clear();
  } catch {
    // ignore
  }
});

describe("Accordion — uncontrolled mode", () => {
  it("defaults to expanded (defaultCollapsed omitted)", async () => {
    const { getByTestId, getByText } = render(AccordionHarness, {
      props: { title: "Recent Documents", testId: "acc" },
    });
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("true");
    expect(getByText("body content")).toBeTruthy();
  });

  it("respects defaultCollapsed", async () => {
    const { getByTestId, queryByText } = render(AccordionHarness, {
      props: { title: "Recent Documents", testId: "acc", defaultCollapsed: true },
    });
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("false");
    expect(queryByText("body content")).toBeNull();
  });

  it("clicking the header toggles collapsed state and hides/shows the body", async () => {
    const { getByTestId, queryByText } = render(AccordionHarness, {
      props: { title: "Recent Documents", testId: "acc" },
    });
    await tick();
    await fireEvent.click(getByTestId("acc"));
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("false");
    expect(queryByText("body content")).toBeNull();

    await fireEvent.click(getByTestId("acc"));
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("true");
    expect(queryByText("body content")).toBeTruthy();
  });

  it("Enter key toggles the header (explicit keyboard handler, not relying on jsdom's native button behaviour)", async () => {
    const { getByTestId } = render(AccordionHarness, {
      props: { title: "Recent Documents", testId: "acc" },
    });
    await tick();
    await fireEvent.keyDown(getByTestId("acc"), { key: "Enter" });
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("false");
  });

  it("Space key toggles the header", async () => {
    const { getByTestId } = render(AccordionHarness, {
      props: { title: "Recent Documents", testId: "acc" },
    });
    await tick();
    await fireEvent.keyDown(getByTestId("acc"), { key: " " });
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("false");
  });

  it("an unrelated key (e.g. Tab) does not toggle", async () => {
    const { getByTestId } = render(AccordionHarness, {
      props: { title: "Recent Documents", testId: "acc" },
    });
    await tick();
    await fireEvent.keyDown(getByTestId("acc"), { key: "Tab" });
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("true");
  });

  it("with a storageKey, remembers collapsed state across a remount (sessionStorage) within the same session", async () => {
    const props = { title: "Recent Documents", testId: "acc", storageKey: "test-recent-docs" };
    const first = render(AccordionHarness, { props });
    await tick();
    await fireEvent.click(first.getByTestId("acc"));
    await tick();
    expect(first.getByTestId("acc").getAttribute("aria-expanded")).toBe("false");
    first.unmount();

    // Simulates a real app restart (App.svelte's panels stay mounted across a
    // Search toggle as of the 2026-09-08 panel-stacking fix, so this no longer
    // happens mid-session) - a fresh mount of the same storageKey must still
    // pick the persisted state back up.
    const second = render(AccordionHarness, { props });
    await tick();
    expect(second.getByTestId("acc").getAttribute("aria-expanded")).toBe("false");
  });

  it("without a storageKey, state does NOT survive a remount (in-memory only)", async () => {
    const props = { title: "Recent Documents", testId: "acc" };
    const first = render(AccordionHarness, { props });
    await tick();
    await fireEvent.click(first.getByTestId("acc"));
    await tick();
    first.unmount();

    const second = render(AccordionHarness, { props });
    await tick();
    expect(second.getByTestId("acc").getAttribute("aria-expanded")).toBe("true");
  });

  it("two different storageKeys persist independently", async () => {
    const a = render(AccordionHarness, { props: { title: "A", testId: "acc-a", storageKey: "test-key-a" } });
    const b = render(AccordionHarness, { props: { title: "B", testId: "acc-b", storageKey: "test-key-b" } });
    await tick();
    await fireEvent.click(a.getByTestId("acc-a"));
    await tick();
    expect(a.getByTestId("acc-a").getAttribute("aria-expanded")).toBe("false");
    expect(b.getByTestId("acc-b").getAttribute("aria-expanded")).toBe("true");
  });

  it("a blocked/unavailable sessionStorage does not break rendering or toggling", async () => {
    const original = globalThis.sessionStorage;
    // Simulate a private-mode/blocked sessionStorage throwing on access.
    Object.defineProperty(globalThis, "sessionStorage", {
      configurable: true,
      get() {
        throw new Error("blocked");
      },
    });
    try {
      const { getByTestId } = render(AccordionHarness, {
        props: { title: "Recent Documents", testId: "acc", storageKey: "test-blocked" },
      });
      await tick();
      expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("true");
      await fireEvent.click(getByTestId("acc"));
      await tick();
      expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("false");
    } finally {
      Object.defineProperty(globalThis, "sessionStorage", { configurable: true, value: original });
    }
  });
});

describe("Accordion — controlled mode", () => {
  it("reflects the caller's collapsed prop and calls ontoggle instead of managing its own state", async () => {
    const ontoggle = vi.fn();
    const { getByTestId, rerender } = render(AccordionHarness, {
      props: { title: "Group A", testId: "acc", collapsed: false, ontoggle },
    });
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("true");

    await fireEvent.click(getByTestId("acc"));
    expect(ontoggle).toHaveBeenCalledWith(true);
    // Controlled: the caller hasn't updated the prop yet, so it stays expanded
    // until the caller re-renders with collapsed=true.
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("true");

    await rerender({ title: "Group A", testId: "acc", collapsed: true, ontoggle });
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("false");
  });

  it("controlled mode ignores storageKey - the caller owns the state, not sessionStorage", async () => {
    const ontoggle = vi.fn();
    const { getByTestId } = render(AccordionHarness, {
      props: { title: "Group A", testId: "acc", collapsed: false, ontoggle, storageKey: "should-be-unused" },
    });
    await tick();
    await fireEvent.click(getByTestId("acc"));
    // Still expanded (controlled - the prop didn't change), proving the click
    // did not fall through to uncontrolled self-management despite storageKey
    // being present.
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("true");
    expect(ontoggle).toHaveBeenCalledWith(true);
  });
});

describe("Accordion — headerExtra stays outside the toggle button", () => {
  it("does not nest an interactive headerExtra element inside the header button", async () => {
    const { getByTestId, container } = render(AccordionHarness, {
      props: { title: "Structural Notes", testId: "acc", withHeaderExtra: true },
    });
    await tick();
    const header = getByTestId("acc");
    expect(header.tagName).toBe("BUTTON");
    // The delete button must not be a descendant of the toggle button (HTML
    // forbids nested interactive controls, and clicking delete would also
    // fire the toggle if it were nested).
    expect(header.querySelector('[data-testid="header-extra-btn"]')).toBeNull();
    const extraBtn = container.querySelector('[data-testid="header-extra-btn"]');
    expect(extraBtn).toBeTruthy();
    expect(header.contains(extraBtn)).toBe(false);
  });

  it("clicking headerExtra content does not toggle the accordion", async () => {
    const { getByTestId, container } = render(AccordionHarness, {
      props: { title: "Structural Notes", testId: "acc", withHeaderExtra: true },
    });
    await tick();
    await fireEvent.click(container.querySelector('[data-testid="header-extra-btn"]')!);
    await tick();
    expect(getByTestId("acc").getAttribute("aria-expanded")).toBe("true");
  });
});
