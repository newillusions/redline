// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { isEditableTarget, resolveUndoRedoShortcut, resolveSearchShortcut } from "./keyboard-shortcuts";

function key(opts: Partial<{ key: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; target: EventTarget | null }>) {
  return { key: "z", metaKey: false, ctrlKey: false, shiftKey: false, target: null, ...opts };
}

describe("isEditableTarget", () => {
  it("is false for null", () => {
    expect(isEditableTarget(null)).toBe(false);
  });

  it("is false for a plain div", () => {
    expect(isEditableTarget(document.createElement("div"))).toBe(false);
  });

  it("is true for an INPUT", () => {
    expect(isEditableTarget(document.createElement("input"))).toBe(true);
  });

  it("is true for a TEXTAREA (the Text/Callout inline editor)", () => {
    expect(isEditableTarget(document.createElement("textarea"))).toBe(true);
  });

  it("is true for a contentEditable element", () => {
    // jsdom doesn't wire the `.contentEditable` IDL property to the attribute (it has no
    // contentEditable implementation at all) - set the attribute directly, which is what
    // isEditableTarget actually checks (see its doc comment for why).
    const el = document.createElement("div");
    el.setAttribute("contenteditable", "true");
    document.body.appendChild(el);
    expect(isEditableTarget(el)).toBe(true);
    el.remove();
  });
});

describe("resolveUndoRedoShortcut", () => {
  it("Ctrl+Z resolves to undo", () => {
    expect(resolveUndoRedoShortcut(key({ key: "z", ctrlKey: true }))).toBe("undo");
  });

  it("Cmd+Z resolves to undo", () => {
    expect(resolveUndoRedoShortcut(key({ key: "z", metaKey: true }))).toBe("undo");
  });

  it("Ctrl+Shift+Z resolves to redo", () => {
    expect(resolveUndoRedoShortcut(key({ key: "z", ctrlKey: true, shiftKey: true }))).toBe("redo");
  });

  it("Cmd+Shift+Z resolves to redo", () => {
    expect(resolveUndoRedoShortcut(key({ key: "z", metaKey: true, shiftKey: true }))).toBe("redo");
  });

  it("Ctrl+Y resolves to redo", () => {
    expect(resolveUndoRedoShortcut(key({ key: "y", ctrlKey: true }))).toBe("redo");
  });

  it("Cmd+Y resolves to redo", () => {
    expect(resolveUndoRedoShortcut(key({ key: "y", metaKey: true }))).toBe("redo");
  });

  it("uppercase Z (Shift held on a US layout) still resolves via the key check, not case-sensitive", () => {
    expect(resolveUndoRedoShortcut(key({ key: "Z", ctrlKey: true }))).toBe("undo");
  });

  it("no modifier held resolves to null", () => {
    expect(resolveUndoRedoShortcut(key({ key: "z" }))).toBe(null);
  });

  it("unrelated key with a modifier resolves to null", () => {
    expect(resolveUndoRedoShortcut(key({ key: "a", ctrlKey: true }))).toBe(null);
  });

  it("returns null when the event target is an editable element (native undo must win)", () => {
    const input = document.createElement("input");
    expect(resolveUndoRedoShortcut(key({ key: "z", ctrlKey: true, target: input }))).toBe(null);
  });

  it("returns null for Ctrl+Shift+Z when the target is a textarea", () => {
    const textarea = document.createElement("textarea");
    expect(resolveUndoRedoShortcut(key({ key: "z", ctrlKey: true, shiftKey: true, target: textarea }))).toBe(null);
  });
});

function skey(opts: Partial<{ key: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }>) {
  return { key: "", metaKey: false, ctrlKey: false, shiftKey: false, ...opts };
}

describe("resolveSearchShortcut", () => {
  it("Cmd+F resolves to open", () => {
    expect(resolveSearchShortcut(skey({ key: "f", metaKey: true }))).toBe("open");
  });

  it("Ctrl+F resolves to open", () => {
    expect(resolveSearchShortcut(skey({ key: "F", ctrlKey: true }))).toBe("open");
  });

  it("Cmd+Shift+F does NOT resolve to open (reserved, not a search shortcut here)", () => {
    expect(resolveSearchShortcut(skey({ key: "f", metaKey: true, shiftKey: true }))).toBe(null);
  });

  it("F3 resolves to next", () => {
    expect(resolveSearchShortcut(skey({ key: "F3" }))).toBe("next");
  });

  it("Shift+F3 resolves to prev", () => {
    expect(resolveSearchShortcut(skey({ key: "F3", shiftKey: true }))).toBe("prev");
  });

  it("Cmd/Ctrl+G does NOT resolve to a search shortcut — regression guard, PR #86 review 2026-08-31: Viewport.svelte binds this to group/ungroup markups via its own separate window listener that never checks defaultPrevented, so both would fire on one keystroke if search claimed it too", () => {
    expect(resolveSearchShortcut(skey({ key: "g", metaKey: true }))).toBe(null);
    expect(resolveSearchShortcut(skey({ key: "g", ctrlKey: true }))).toBe(null);
    expect(resolveSearchShortcut(skey({ key: "g", metaKey: true, shiftKey: true }))).toBe(null);
    expect(resolveSearchShortcut(skey({ key: "g", ctrlKey: true, shiftKey: true }))).toBe(null);
  });

  it("unrelated keys resolve to null", () => {
    expect(resolveSearchShortcut(skey({ key: "a" }))).toBe(null);
    expect(resolveSearchShortcut(skey({ key: "Enter" }))).toBe(null);
    expect(resolveSearchShortcut(skey({ key: "f" }))).toBe(null); // bare "f", no modifier
  });
});
