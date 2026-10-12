import { beforeEach, describe, expect, it, vi } from "vitest";
import type { NavigationInfo } from "./navigation-ipc";

const getNavigation = vi.fn();
const navigationEdit = vi.fn();
const navigationUndo = vi.fn();
const navigationRedo = vi.fn();

vi.mock("./navigation-ipc", () => ({
  getNavigation: (...a: unknown[]) => getNavigation(...a),
  navigationEdit: (...a: unknown[]) => navigationEdit(...a),
  navigationUndo: (...a: unknown[]) => navigationUndo(...a),
  navigationRedo: (...a: unknown[]) => navigationRedo(...a),
}));

import { NavigationStore } from "./navigation-store.svelte";

function info(over: Partial<NavigationInfo> = {}): NavigationInfo {
  return {
    page_count: 3,
    outline: { nodes: [], total: 0, broken: 0, read_only: null },
    labels: ["", "", ""],
    links: [],
    links_truncated: false,
    read_only: null,
    undo_depth: 0,
    redo_depth: 0,
    ...over,
  };
}

describe("NavigationStore", () => {
  beforeEach(() => {
    getNavigation.mockReset();
    navigationEdit.mockReset();
    navigationUndo.mockReset();
    navigationRedo.mockReset();
  });

  it("refresh loads the state and marks it loaded", async () => {
    getNavigation.mockResolvedValue(info({ labels: ["E-1", "E-2", "E-3"] }));
    const s = new NavigationStore("d1");
    expect(s.loaded).toBe(false);
    await s.refresh();
    expect(getNavigation).toHaveBeenCalledWith("d1");
    expect(s.loaded).toBe(true);
    expect(s.labels).toEqual(["E-1", "E-2", "E-3"]);
    expect(s.error).toBeNull();
  });

  it("a failed read is reported and does not retry forever", async () => {
    getNavigation.mockRejectedValue(new Error("boom"));
    const s = new NavigationStore("d1");
    await s.refresh();
    expect(s.loaded).toBe(true);
    expect(s.error).toBe("boom");
    expect(s.info).toBeNull();
  });

  it("overlapping refreshes share one read", async () => {
    let resolve!: (i: NavigationInfo) => void;
    getNavigation.mockReturnValue(new Promise<NavigationInfo>((r) => (resolve = r)));
    const s = new NavigationStore("d1");
    const a = s.refresh();
    const b = s.refresh();
    resolve(info());
    await Promise.all([a, b]);
    expect(getNavigation).toHaveBeenCalledTimes(1);
  });

  it("an edit replaces the state and bumps the revision", async () => {
    getNavigation.mockResolvedValue(info());
    const s = new NavigationStore("d1");
    await s.refresh();
    const rev = s.revision;
    navigationEdit.mockResolvedValue(info({ undo_depth: 1, labels: ["A", "B", "C"] }));
    const ok = await s.edit({ op: "label_rename", page: 0, text: "A" });
    expect(ok).toBe(true);
    expect(navigationEdit).toHaveBeenCalledWith("d1", { op: "label_rename", page: 0, text: "A" });
    expect(s.labels).toEqual(["A", "B", "C"]);
    expect(s.revision).toBeGreaterThan(rev);
    expect(s.canUndo).toBe(true);
    expect(s.canRedo).toBe(false);
  });

  it("a refused edit shows the reason and re-reads the file", async () => {
    getNavigation.mockResolvedValue(info({ labels: ["x", "", ""] }));
    const s = new NavigationStore("d1");
    await s.refresh();
    navigationEdit.mockRejectedValue("The bookmarks changed since the panel was drawn");
    const ok = await s.edit({ op: "bookmark_delete", id: "0", expect: "A" });
    expect(ok).toBe(false);
    expect(s.error).toContain("changed");
    await vi.waitFor(() => expect(getNavigation).toHaveBeenCalledTimes(2));
    expect(s.busy).toBe(false);
  });

  it("does not start a second edit while one is running", async () => {
    getNavigation.mockResolvedValue(info());
    const s = new NavigationStore("d1");
    await s.refresh();
    let resolve!: (i: NavigationInfo) => void;
    navigationEdit.mockReturnValue(new Promise<NavigationInfo>((r) => (resolve = r)));
    const first = s.edit({ op: "label_rename", page: 0, text: "A" });
    expect(s.busy).toBe(true);
    expect(await s.edit({ op: "label_rename", page: 1, text: "B" })).toBe(false);
    expect(navigationEdit).toHaveBeenCalledTimes(1);
    resolve(info());
    await first;
    expect(s.busy).toBe(false);
  });

  it("undo and redo call the backend and take the returned state", async () => {
    getNavigation.mockResolvedValue(info({ undo_depth: 2 }));
    const s = new NavigationStore("d1");
    await s.refresh();
    navigationUndo.mockResolvedValue(info({ undo_depth: 1, redo_depth: 1 }));
    await s.undo();
    expect(navigationUndo).toHaveBeenCalledWith("d1");
    expect(s.canRedo).toBe(true);
    navigationRedo.mockResolvedValue(info({ undo_depth: 2, redo_depth: 0 }));
    await s.redo();
    expect(navigationRedo).toHaveBeenCalledWith("d1");
    expect(s.canRedo).toBe(false);
  });

  it("reports why editing is off", async () => {
    getNavigation.mockResolvedValue(info({ read_only: "password-protected" }));
    const s = new NavigationStore("d1");
    await s.refresh();
    expect(s.fileEditable).toBe(false);
    expect(s.readOnlyReason).toBe("password-protected");
    getNavigation.mockResolvedValue(
      info({ outline: { nodes: [], total: 0, broken: 0, read_only: "loops" } }),
    );
    await new NavigationStore("d2").refresh();
    const t = new NavigationStore("d2");
    await t.refresh();
    expect(t.readOnlyReason).toBe("loops");
    expect(t.fileEditable).toBe(true);
  });

  it("link mode starts clean and ends cleanly", () => {
    const s = new NavigationStore("d1");
    s.pendingLink = { page: 1, rect: [0, 0, 5, 5] };
    s.beginLinkMode();
    expect(s.linkMode).toBe(true);
    expect(s.pendingLink).toBeNull();
    s.endLinkMode();
    expect(s.linkMode).toBe(false);
  });
});
