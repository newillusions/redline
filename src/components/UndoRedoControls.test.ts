// @vitest-environment jsdom
/**
 * UndoRedoControls tests - covers the 2026-08-05 GUI validation pass finding
 * (obs:us5j4ne1r5byjzle8u23): MarkupStore.undo()/redo() had zero UI surface. These assert
 * the buttons actually call the store and that disabled state tracks the history stack.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import UndoRedoControls from "./UndoRedoControls.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import type { Markup } from "$lib/ipc";

function fakeIpc() {
  return {
    add: vi.fn(async () => {}),
    update: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
  };
}

function fakeMarkup(id: string): Markup {
  return {
    id, markup_type: "Rectangle", page: 0,
    geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } },
    appearance: { color: "#e02424", line_weight: 1, opacity: 1, fill: null, line_style: "Solid", font: null },
    subject: null, layer: null, contents: null, group_id: null,
    audit: { created_by: { user_id: "u", display_name: "U" }, created_at: "", modified_by: { user_id: "u", display_name: "U" }, modified_at: "", revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] }, measurement: null,
  };
}

describe("UndoRedoControls", () => {
  it("both buttons are disabled when store is null (no document open)", () => {
    render(UndoRedoControls, { props: { store: null } });
    expect(screen.getByTitle("Undo (Cmd/Ctrl+Z)")).toBeDisabled();
    expect(screen.getByTitle("Redo (Cmd/Ctrl+Shift+Z)")).toBeDisabled();
  });

  it("both buttons are disabled on an empty (freshly created) store", () => {
    const store = new MarkupStore("doc1", fakeIpc());
    render(UndoRedoControls, { props: { store } });
    expect(screen.getByTitle("Undo (Cmd/Ctrl+Z)")).toBeDisabled();
    expect(screen.getByTitle("Redo (Cmd/Ctrl+Shift+Z)")).toBeDisabled();
  });

  it("Undo becomes enabled after a create, and clicking it calls store.undo()", async () => {
    const user = userEvent.setup();
    const store = new MarkupStore("doc1", fakeIpc());
    render(UndoRedoControls, { props: { store } });

    store.create(fakeMarkup("m1"));
    await Promise.resolve();

    const undoBtn = screen.getByTitle("Undo (Cmd/Ctrl+Z)");
    expect(undoBtn).not.toBeDisabled();
    expect(store.markups).toHaveLength(1);

    await user.click(undoBtn);

    expect(store.markups).toHaveLength(0);
  });

  it("Redo becomes enabled after an undo, and clicking it calls store.redo()", async () => {
    const user = userEvent.setup();
    const store = new MarkupStore("doc1", fakeIpc());
    render(UndoRedoControls, { props: { store } });

    store.create(fakeMarkup("m1"));
    store.undo();
    await Promise.resolve();

    const redoBtn = screen.getByTitle("Redo (Cmd/Ctrl+Shift+Z)");
    expect(redoBtn).not.toBeDisabled();

    await user.click(redoBtn);

    expect(store.markups).toHaveLength(1);
  });

  it("Redo is disabled again after a new create clears the redo stack", async () => {
    const store = new MarkupStore("doc1", fakeIpc());
    render(UndoRedoControls, { props: { store } });

    store.create(fakeMarkup("m1"));
    store.undo();
    await Promise.resolve();
    expect(screen.getByTitle("Redo (Cmd/Ctrl+Shift+Z)")).not.toBeDisabled();

    store.create(fakeMarkup("m2"));
    await Promise.resolve();

    expect(screen.getByTitle("Redo (Cmd/Ctrl+Shift+Z)")).toBeDisabled();
  });
});
