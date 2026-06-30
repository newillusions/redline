// @vitest-environment jsdom
/**
 * PropertiesPanel G7.3 tests.
 *
 * Strategy:
 * - Mock $lib/ipc so getUserIdentity resolves without Tauri.
 * - Mount REAL PropertiesPanel.svelte with a REAL MarkupStore (fake ipc injected).
 * - No canvas / ResizeObserver shims needed (no Viewport here).
 * - Drive controls via @testing-library fireEvent / userEvent.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor } from "@testing-library/svelte";
import { fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import PropertiesPanel from "./PropertiesPanel.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { buildMarkup } from "$lib/markup-tools";

// ---------------------------------------------------------------------------
// Mock $lib/ipc (hoisted by Vitest).
// ---------------------------------------------------------------------------
vi.mock("$lib/ipc", () => ({
  getPageSize: vi.fn(),
  renderTile: vi.fn(),
  processRssMb: vi.fn(),
  getUserIdentity: vi.fn(),
  openDocument: vi.fn(),
  closeDocument: vi.fn(),
  addMarkup: vi.fn(),
  listMarkups: vi.fn(),
  loadMarkups: vi.fn(),
  saveDocument: vi.fn(),
  saveDocumentAs: vi.fn(),
  updateMarkup: vi.fn(),
  deleteMarkup: vi.fn(),
}));

import * as ipcMocks from "$lib/ipc";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const FAKE_IDENTITY = { user_id: "aaaaaaaa-0000-0000-0000-000000000001", display_name: "Tester" };

const BASE_APPEARANCE = {
  color: "#ff0000",
  line_weight: 2,
  opacity: 1,
  fill: null,
  line_style: "Solid" as const,
  font: { family: "Helvetica", size_pt: 12 },
};

const BASE_IDENTITY = { user_id: "bbbbbbbb-0000-0000-0000-000000000001", display_name: "Author" };

function fakeIpc() {
  return {
    add: vi.fn(async () => {}),
    update: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
  };
}

function makeStore() {
  return new MarkupStore("doc1", fakeIpc());
}

function makeMarkup(id: string, overrides: Partial<Parameters<typeof buildMarkup>[0]> = {}) {
  return buildMarkup({
    markupType: "Rectangle",
    page: 0,
    geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 100, y: 100 } } },
    appearance: { ...BASE_APPEARANCE },
    identity: BASE_IDENTITY,
    now: "2026-01-01T00:00:00.000Z",
    id,
    ...overrides,
  });
}

/** Mount the panel and wait for getUserIdentity to be called (identity load on mount). */
async function mountPanel(store: MarkupStore) {
  const result = render(PropertiesPanel, { props: { store } });
  await waitFor(() => {
    expect(vi.mocked(ipcMocks.getUserIdentity)).toHaveBeenCalled();
  });
  await tick();
  return result;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

beforeEach(() => {
  vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);
});

// ---------------------------------------------------------------------------
describe("draft mode (no selection)", () => {
  it("shows 'Defaults for new markups' header", async () => {
    const store = makeStore();
    const { getByText } = await mountPanel(store);
    expect(getByText(/defaults for new markups/i)).toBeTruthy();
  });

  it("does not show contents / subject / layer fields", async () => {
    const store = makeStore();
    const { container } = await mountPanel(store);
    const labels = Array.from(container.querySelectorAll("label")).map((l) =>
      l.textContent?.toLowerCase() ?? ""
    );
    expect(labels.some((l) => l.includes("contents"))).toBe(false);
    expect(labels.some((l) => l.includes("subject"))).toBe(false);
    expect(labels.some((l) => l.includes("layer"))).toBe(false);
  });

  it("changing color updates draftAppearance without creating an undo entry", async () => {
    const store = makeStore();
    const { container } = await mountPanel(store);

    const colorInput = container.querySelector(
      "input[type='color'][data-field='color']"
    ) as HTMLInputElement;
    expect(colorInput).toBeTruthy();

    fireEvent.input(colorInput, { target: { value: "#00ff00" } });
    await tick();

    expect(store.draftAppearance.color).toBe("#00ff00");
    expect(store.markups.length).toBe(0);
    expect(store.canUndo).toBe(false);
  });

  it("changing line weight updates draftAppearance", async () => {
    const store = makeStore();
    const { container } = await mountPanel(store);

    const weightInput = container.querySelector(
      "input[type='number'][data-field='line_weight']"
    ) as HTMLInputElement;
    expect(weightInput).toBeTruthy();

    fireEvent.input(weightInput, { target: { value: "5" } });
    await tick();

    expect(store.draftAppearance.line_weight).toBe(5);
    expect(store.canUndo).toBe(false);
  });

  it("changing box outline colour updates draftAppearance.outline_color (distinct from color)", async () => {
    const store = makeStore();
    const { container } = await mountPanel(store);

    const outlineInput = container.querySelector(
      "input[type='color'][data-field='outline_color']"
    ) as HTMLInputElement;
    expect(outlineInput).toBeTruthy();

    fireEvent.input(outlineInput, { target: { value: "#0000ff" } });
    await tick();

    expect(store.draftAppearance.outline_color).toBe("#0000ff");
    // The glyph colour is untouched — outline is a separate field.
    expect(store.draftAppearance.color).not.toBe("#0000ff");
    expect(store.canUndo).toBe(false);
  });

  it("changing fill opacity updates draftAppearance.fill_opacity independently of opacity", async () => {
    const store = makeStore();
    const { container } = await mountPanel(store);

    const fillOpacityInput = container.querySelector(
      "input[type='range'][data-field='fill_opacity']"
    ) as HTMLInputElement;
    expect(fillOpacityInput).toBeTruthy();

    fireEvent.input(fillOpacityInput, { target: { value: "0.3" } });
    await tick();

    expect(store.draftAppearance.fill_opacity).toBeCloseTo(0.3);
    // Overall opacity is a different control — unchanged.
    expect(store.draftAppearance.opacity).toBe(1);
    expect(store.canUndo).toBe(false);
  });
});

// ---------------------------------------------------------------------------
describe("single selection mode", () => {
  it("shows 'markup(s) selected' header", async () => {
    const store = makeStore();
    const m = makeMarkup("m1");
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { getByText } = await mountPanel(store);
    expect(getByText(/1 markup\(s\) selected/i)).toBeTruthy();
  });

  it("color input reflects the selected markup's color", async () => {
    const store = makeStore();
    const m = makeMarkup("m1");
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    const colorInput = container.querySelector(
      "input[type='color'][data-field='color']"
    ) as HTMLInputElement;
    expect(colorInput.value).toBe("#ff0000");
  });

  it("changing color updates the markup and creates ONE undo frame", async () => {
    const ipc = fakeIpc();
    const store = new MarkupStore("doc1", ipc);
    const m = makeMarkup("m1");
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    const colorInput = container.querySelector(
      "input[type='color'][data-field='color']"
    ) as HTMLInputElement;

    fireEvent.input(colorInput, { target: { value: "#0000ff" } });
    await tick();

    expect(store.markups[0].appearance.color).toBe("#0000ff");
    expect(store.canUndo).toBe(true);

    store.undo();
    await tick();
    expect(store.markups[0].appearance.color).toBe("#ff0000");
    expect(store.canUndo).toBe(false);

    // Drain mirror queue
    await waitFor(() => expect(ipc.update).toHaveBeenCalled());
  });

  it("changing line weight commits and is undoable", async () => {
    const store = makeStore();
    const m = makeMarkup("m1");
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    const weightInput = container.querySelector(
      "input[type='number'][data-field='line_weight']"
    ) as HTMLInputElement;

    fireEvent.input(weightInput, { target: { value: "8" } });
    await tick();

    expect(store.markups[0].appearance.line_weight).toBe(8);
    expect(store.canUndo).toBe(true);

    store.undo();
    await tick();
    expect(store.markups[0].appearance.line_weight).toBe(2);
  });

  it("'No fill' checkbox sets fill to null", async () => {
    const store = makeStore();
    // markup with a fill color
    const m = makeMarkup("m1", {
      appearance: { ...BASE_APPEARANCE, fill: "#aabbcc" },
    });
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    const noFillCheckbox = container.querySelector(
      "input[type='checkbox'][data-field='no_fill']"
    ) as HTMLInputElement;
    expect(noFillCheckbox).toBeTruthy();
    // currently has fill, so checkbox should NOT be checked
    expect(noFillCheckbox.checked).toBe(false);

    fireEvent.click(noFillCheckbox);
    await tick();

    expect(store.markups[0].appearance.fill).toBeNull();
  });

  it("line style select commits and is undoable", async () => {
    const store = makeStore();
    const m = makeMarkup("m1");
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    const lineStyleSelect = container.querySelector(
      "select[data-field='line_style']"
    ) as HTMLSelectElement;
    expect(lineStyleSelect).toBeTruthy();

    fireEvent.change(lineStyleSelect, { target: { value: "Dashed" } });
    await tick();

    expect(store.markups[0].appearance.line_style).toBe("Dashed");
    expect(store.canUndo).toBe(true);
  });

  it("font family select updates font.family", async () => {
    const store = makeStore();
    const m = makeMarkup("m1");
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    const fontSelect = container.querySelector(
      "select[data-field='font_family']"
    ) as HTMLSelectElement;
    expect(fontSelect).toBeTruthy();

    fireEvent.change(fontSelect, { target: { value: "Courier" } });
    await tick();

    expect(store.markups[0].appearance.font?.family).toBe("Courier");
    expect(store.canUndo).toBe(true);
  });

  it("contents textarea updates contents field", async () => {
    const store = makeStore();
    const m = makeMarkup("m1", { markupType: "Text" });
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    const contentsArea = container.querySelector(
      "textarea[data-field='contents']"
    ) as HTMLTextAreaElement;
    expect(contentsArea).toBeTruthy();

    fireEvent.input(contentsArea, { target: { value: "Hello PDF" } });
    await tick();

    expect(store.markups[0].contents).toBe("Hello PDF");
    expect(store.canUndo).toBe(true);

    store.undo();
    await tick();
    expect(store.markups[0].contents).toBeNull();
  });

  it("subject input updates subject field", async () => {
    const store = makeStore();
    const m = makeMarkup("m1");
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    const subjectInput = container.querySelector(
      "input[type='text'][data-field='subject']"
    ) as HTMLInputElement;
    expect(subjectInput).toBeTruthy();

    fireEvent.input(subjectInput, { target: { value: "Review comment" } });
    await tick();

    expect(store.markups[0].subject).toBe("Review comment");
  });

  it("layer input updates layer field", async () => {
    const store = makeStore();
    const m = makeMarkup("m1");
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    const layerInput = container.querySelector(
      "input[type='text'][data-field='layer']"
    ) as HTMLInputElement;
    expect(layerInput).toBeTruthy();

    fireEvent.input(layerInput, { target: { value: "Architectural" } });
    await tick();

    expect(store.markups[0].layer).toBe("Architectural");
  });
});

// ---------------------------------------------------------------------------
describe("page number display (selection mode only)", () => {
  it("single selected markup shows 1-based page number", async () => {
    const store = makeStore();
    // page: 2 (0-based) → displayed as 3
    const m = makeMarkup("m1", { page: 2 });
    store.seed([m]);
    store.selectedIds = new Set(["m1"]);

    const { container } = await mountPanel(store);

    // "Page" label text must be present in the Content section (selection mode only).
    expect(container.textContent?.toLowerCase()).toContain("page");
    // The 1-based value "3" must appear.
    expect(container.textContent).toContain("3");
  });

  it("multi-selection same page shows that 1-based page", async () => {
    const store = makeStore();
    const m1 = makeMarkup("m1", { page: 1 });
    const m2 = makeMarkup("m2", { page: 1 });
    store.seed([m1, m2]);
    store.selectedIds = new Set(["m1", "m2"]);

    const { container } = await mountPanel(store);
    // page 1 → displayed as 2
    expect(container.textContent).toContain("2");
  });

  it("multi-selection spanning different pages shows 'Multiple'", async () => {
    const store = makeStore();
    const m1 = makeMarkup("m1", { page: 0 });
    const m2 = makeMarkup("m2", { page: 1 });
    store.seed([m1, m2]);
    store.selectedIds = new Set(["m1", "m2"]);

    const { getByText } = await mountPanel(store);
    expect(getByText("Multiple")).toBeTruthy();
  });

  it("draft mode does not show a page row", async () => {
    const store = makeStore();
    const { container } = await mountPanel(store);
    // No markup selected → draft mode → Content section hidden → no "Page" label.
    expect(container.textContent?.toLowerCase()).not.toContain("page");
  });
});

// ---------------------------------------------------------------------------
describe("multi-selection (2 markups)", () => {
  it("color input is blank/indeterminate when colors differ", async () => {
    const store = makeStore();
    const m1 = makeMarkup("m1", { appearance: { ...BASE_APPEARANCE, color: "#ff0000" } });
    const m2 = makeMarkup("m2", { appearance: { ...BASE_APPEARANCE, color: "#0000ff" } });
    store.seed([m1, m2]);
    store.selectedIds = new Set(["m1", "m2"]);

    const { container } = await mountPanel(store);

    const colorInput = container.querySelector(
      "input[type='color'][data-field='color']"
    ) as HTMLInputElement;
    // indeterminate state is signalled via data-indeterminate attribute
    // (jsdom normalises empty string to #000000 for color inputs, so we use an attribute flag)
    expect(colorInput.getAttribute("data-indeterminate")).toBe("true");
  });

  it("setting a color applies to ALL selected as ONE undo frame", async () => {
    const ipc = fakeIpc();
    const store = new MarkupStore("doc1", ipc);
    const m1 = makeMarkup("m1", { appearance: { ...BASE_APPEARANCE, color: "#ff0000" } });
    const m2 = makeMarkup("m2", { appearance: { ...BASE_APPEARANCE, color: "#0000ff" } });
    store.seed([m1, m2]);
    store.selectedIds = new Set(["m1", "m2"]);

    const { container } = await mountPanel(store);

    const colorInput = container.querySelector(
      "input[type='color'][data-field='color']"
    ) as HTMLInputElement;

    fireEvent.input(colorInput, { target: { value: "#00ff00" } });
    await tick();

    expect(store.markups[0].appearance.color).toBe("#00ff00");
    expect(store.markups[1].appearance.color).toBe("#00ff00");

    // ipc.update should have been called twice (one per markup) for the initial apply
    await waitFor(() => expect(ipc.update).toHaveBeenCalledTimes(2));

    // ONE undo frame reverts BOTH
    expect(store.canUndo).toBe(true);
    store.undo();
    await tick();
    expect(store.markups[0].appearance.color).toBe("#ff0000");
    expect(store.markups[1].appearance.color).toBe("#0000ff");
    expect(store.canUndo).toBe(false);
  });

  it("shows '2 markup(s) selected' header", async () => {
    const store = makeStore();
    const m1 = makeMarkup("m1");
    const m2 = makeMarkup("m2");
    store.seed([m1, m2]);
    store.selectedIds = new Set(["m1", "m2"]);

    const { getByText } = await mountPanel(store);
    expect(getByText(/2 markup\(s\) selected/i)).toBeTruthy();
  });
});
