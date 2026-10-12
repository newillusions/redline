import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import * as nav from "./navigation-ipc";

/**
 * Tauri v2 maps camelCase JS argument keys to the Rust command's snake_case parameters; a
 * snake_case key is rejected at runtime ("command X missing required key"), which the rest of
 * the suite cannot see because it mocks `invoke`. Fields INSIDE the `edit` object are serde
 * names and stay snake_case (they match `NavEdit` in document/navigation/edit.rs).
 */
const mockInvoke = vi.mocked(invoke);

describe("navigation IPC argument keys", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined as never);
  });

  it("get_navigation / navigation_undo / navigation_redo -> docId", async () => {
    await nav.getNavigation("d1");
    await nav.navigationUndo("d1");
    await nav.navigationRedo("d1");
    expect(mockInvoke).toHaveBeenNthCalledWith(1, "get_navigation", { docId: "d1" });
    expect(mockInvoke).toHaveBeenNthCalledWith(2, "navigation_undo", { docId: "d1" });
    expect(mockInvoke).toHaveBeenNthCalledWith(3, "navigation_redo", { docId: "d1" });
  });

  it("navigation_edit -> docId + the edit object untouched (serde snake_case inside)", async () => {
    const edit: nav.NavEdit = {
      op: "bookmark_add",
      anchor: "0",
      anchor_expect: "Plans",
      place: "child",
      title: "Level 1",
      target: { kind: "page", page: 1, view: { kind: "xyz", left: null, top: null, zoom: null } },
    };
    await nav.navigationEdit("d1", edit);
    expect(mockInvoke).toHaveBeenCalledWith("navigation_edit", { docId: "d1", edit });
  });

  it("prepare_external_open -> uri, confirm_external_open -> token only", async () => {
    mockInvoke.mockResolvedValue({ token: "t", uri: "https://example.com/" } as never);
    await expect(nav.prepareExternalOpen("HTTPS://Example.com")).resolves.toEqual({ token: "t", uri: "https://example.com/" });
    expect(mockInvoke).toHaveBeenCalledWith("prepare_external_open", { uri: "HTTPS://Example.com" });
    await nav.confirmExternalOpen("t");
    // The yes carries the token, never an address: what opens is what the dialog showed.
    expect(mockInvoke).toHaveBeenLastCalledWith("confirm_external_open", { token: "t" });
  });
});
