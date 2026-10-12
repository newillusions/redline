// @vitest-environment jsdom
import { describe, it, expect, afterEach } from "vitest";
import { nativeEditingOwnsClipboardKeys } from "./keyboard-shortcuts";

afterEach(() => { document.body.innerHTML = ""; window.getSelection()?.removeAllRanges(); });

function select(node: Node, text: string) {
  const r = document.createRange();
  r.selectNodeContents(node);
  const s = window.getSelection()!;
  s.removeAllRanges();
  s.addRange(r);
  expect(s.toString()).toBe(text);
  return s;
}

describe("nativeEditingOwnsClipboardKeys", () => {
  it("leaves the keys to a focused input, textarea or contenteditable", () => {
    const input = document.createElement("input");
    const ce = document.createElement("div");
    ce.setAttribute("contenteditable", "true");
    expect(nativeEditingOwnsClipboardKeys(input, null, null)).toBe(true);
    expect(nativeEditingOwnsClipboardKeys(ce, null, null)).toBe(true);
    expect(nativeEditingOwnsClipboardKeys(document.createElement("button"), null, null)).toBe(false);
  });
  it("leaves the keys to text selected outside the viewport, not inside it", () => {
    const panel = document.createElement("p");
    panel.textContent = "side panel text";
    const vp = document.createElement("div");
    vp.textContent = "viewport text";
    document.body.append(panel, vp);
    expect(nativeEditingOwnsClipboardKeys(null, select(panel, "side panel text"), vp)).toBe(true);
    expect(nativeEditingOwnsClipboardKeys(null, select(vp, "viewport text"), vp)).toBe(false);
  });
  it("no selection or a collapsed one does not own the keys", () => {
    expect(nativeEditingOwnsClipboardKeys(null, null, null)).toBe(false);
    expect(nativeEditingOwnsClipboardKeys(null, window.getSelection(), null)).toBe(false);
  });
});
