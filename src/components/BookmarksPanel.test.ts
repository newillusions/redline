// @vitest-environment jsdom
/**
 * BookmarksPanel tests (docs/navigation.md RB1-RB10): the tree, following, add / rename /
 * delete / move, read-only states, drag and drop, hostile titles. The NavigationStore is
 * real; its backend calls (`edit`, `undo`, `redo`) are spied on.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import BookmarksPanel from "./BookmarksPanel.svelte";
import { NavigationStore } from "$lib/navigation-store.svelte";
import type { BookmarkView, NavTarget, NavigationInfo, TargetInput } from "$lib/navigation-ipc";

const page = (p: number): NavTarget => ({
  kind: "page",
  page: p,
  view: { kind: "xyz", left: null, top: null, zoom: null },
});

function bm(id: string, title: string, children: BookmarkView[] = [], target: NavTarget = page(0), open = false): BookmarkView {
  return { id, title, open, target, color: null, bold: false, italic: false, children };
}

function info(nodes: BookmarkView[], over: Partial<NavigationInfo> = {}): NavigationInfo {
  const count = (ns: BookmarkView[]): number => ns.reduce((n, b) => n + 1 + count(b.children), 0);
  return {
    page_count: 5,
    outline: { nodes, total: count(nodes), broken: 0, read_only: null },
    labels: ["E-101", "E-102", "", "", ""],
    links: [],
    links_truncated: false,
    read_only: null,
    undo_depth: 0,
    redo_depth: 0,
    ...over,
  };
}

const view: TargetInput = { kind: "page", page: 1, view: { kind: "xyz", left: 10, top: 700, zoom: 1 } };

function setup(
  nodes: BookmarkView[],
  over: Partial<NavigationInfo> = {},
  currentPage = 0,
  getSelectionBookmark?: () => { title: string; target: TargetInput } | null,
) {
  const nav = new NavigationStore("d1");
  nav.info = info(nodes, over);
  nav.loaded = true;
  nav.revision = 1;
  const edit = vi.spyOn(nav, "edit").mockResolvedValue(true);
  const undo = vi.spyOn(nav, "undo").mockResolvedValue(true);
  const redo = vi.spyOn(nav, "redo").mockResolvedValue(true);
  const onfollow = vi.fn();
  const getCurrentView = vi.fn(async () => view);
  const r = render(BookmarksPanel, { nav, pageCount: 5, currentPage, onfollow, getCurrentView, getSelectionBookmark });
  return { nav, edit, undo, redo, onfollow, getCurrentView, ...r };
}

const tree = () => [
  bm("0", "Plans", [bm("0.0", "Level 1", [], page(1)), bm("0.1", "Level 2", [], page(2))], page(0), true),
  bm("1", "Details", [], page(3)),
];

describe("BookmarksPanel", () => {
  beforeEach(() => vi.clearAllMocks());

  it("shows the tree with the file's expand state and the page marker", () => {
    setup(tree(), {}, 1);
    expect(screen.getByText("Plans")).toBeTruthy();
    expect(screen.getByText("Level 1")).toBeTruthy(); // Plans is open in the file
    expect(screen.getByText("Details")).toBeTruthy();
    // The bookmark for the page being viewed (page index 1) is marked.
    const row = screen.getByText("Level 1").closest("[role=treeitem]")!;
    expect(row.querySelector("[aria-label='On this page']")).toBeTruthy();
    expect(screen.getByText("4 bookmarks")).toBeTruthy();
  });

  it("collapses and expands a parent", async () => {
    setup(tree());
    await fireEvent.click(screen.getByLabelText("Collapse"));
    expect(screen.queryByText("Level 1")).toBeNull();
    await fireEvent.click(screen.getByLabelText("Expand"));
    expect(screen.getByText("Level 1")).toBeTruthy();
  });

  it("clicking a bookmark selects it and runs its target", async () => {
    const { onfollow } = setup(tree());
    await fireEvent.click(screen.getByText("Details"));
    expect(onfollow).toHaveBeenCalledWith(page(3));
    expect(screen.getByText("Details").closest("[role=treeitem]")!.getAttribute("aria-selected")).toBe("true");
  });

  it("a bookmark with an unsupported action is dimmed, runs nothing and says why", async () => {
    const { onfollow } = setup([bm("0", "Remote", [], { kind: "unsupported", action: "GoToR" })]);
    await fireEvent.click(screen.getByText("Remote"));
    expect(onfollow).not.toHaveBeenCalled();
    expect(screen.getByText(/Not supported \(GoToR\)/)).toBeTruthy();
  });

  it("flags a broken bookmark and counts it", () => {
    setup([bm("0", "Gone", [], { kind: "broken" })], {
      outline: { nodes: [bm("0", "Gone", [], { kind: "broken" })], total: 1, broken: 1, read_only: null },
    });
    expect(screen.getByLabelText("Broken bookmark")).toBeTruthy();
    expect(screen.getByText(/1 broken/)).toBeTruthy();
  });

  it("renders hostile titles as text, never as HTML", () => {
    const evil = '<img src=x onerror="window.__pwned=1"><b>bold</b>';
    const { container } = setup([bm("0", evil)]);
    expect(screen.getByText(evil)).toBeTruthy();
    expect(container.querySelector("img")).toBeNull();
    expect((window as unknown as Record<string, unknown>).__pwned).toBeUndefined();
  });

  it("Add puts a bookmark after the selection, titled with the page's label, then renames it", async () => {
    const { edit, getCurrentView } = setup(tree());
    await fireEvent.click(screen.getByText("Details"));
    await fireEvent.click(screen.getByText("Add"));
    await waitFor(() => expect(edit).toHaveBeenCalled());
    expect(getCurrentView).toHaveBeenCalled();
    expect(edit).toHaveBeenCalledWith({
      op: "bookmark_add",
      anchor: "1",
      anchor_expect: "Details",
      place: "after",
      title: "E-102", // the label of the page being bookmarked (page index 1)
      target: view,
    });
  });

  it("Add with no selection appends at the end", async () => {
    const { edit } = setup(tree());
    await fireEvent.click(screen.getByText("Add"));
    await waitFor(() => expect(edit).toHaveBeenCalled());
    expect(edit.mock.calls[0][0]).toMatchObject({ op: "bookmark_add", anchor: null, place: "after" });
  });

  it("F2 renames, Enter commits, and an unchanged or empty title writes nothing", async () => {
    const { edit } = setup(tree());
    await fireEvent.click(screen.getByText("Details"));
    const treeEl = screen.getByRole("tree");
    await fireEvent.keyDown(treeEl, { key: "F2" });
    const input = (await screen.findByLabelText("Bookmark title")) as HTMLInputElement;
    expect(input.value).toBe("Details");
    await fireEvent.input(input, { target: { value: "Details A" } });
    await fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(edit).toHaveBeenCalled());
    expect(edit).toHaveBeenCalledWith({ op: "bookmark_rename", id: "1", expect: "Details", title: "Details A" });

    edit.mockClear();
    await fireEvent.keyDown(treeEl, { key: "F2" });
    const again = (await screen.findByLabelText("Bookmark title")) as HTMLInputElement;
    await fireEvent.input(again, { target: { value: "   " } });
    await fireEvent.keyDown(again, { key: "Enter" });
    await tick();
    expect(edit).not.toHaveBeenCalled();
  });

  it("Escape cancels a rename", async () => {
    const { edit } = setup(tree());
    await fireEvent.click(screen.getByText("Details"));
    await fireEvent.keyDown(screen.getByRole("tree"), { key: "F2" });
    const input = await screen.findByLabelText("Bookmark title");
    await fireEvent.input(input, { target: { value: "Nope" } });
    await fireEvent.keyDown(input, { key: "Escape" });
    await tick();
    expect(edit).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Bookmark title")).toBeNull();
  });

  it("Delete removes the selected bookmark (and its children, in the backend)", async () => {
    const { edit } = setup(tree());
    await fireEvent.click(screen.getByText("Plans"));
    await fireEvent.keyDown(screen.getByRole("tree"), { key: "Delete" });
    await waitFor(() => expect(edit).toHaveBeenCalled());
    expect(edit).toHaveBeenCalledWith({ op: "bookmark_delete", id: "0", expect: "Plans" });
  });

  it("the More menu moves a bookmark (anchor and place) and disables moves that are impossible", async () => {
    const { edit } = setup(tree());
    // The menu closes on an outside pointer-down (not simulated by fireEvent.click), so open it
    // only when it is not already open.
    const openMore = async () => {
      if (!screen.queryByRole("menu")) await fireEvent.click(screen.getByRole("button", { name: "More" }));
    };
    const item = (re: RegExp) => screen.getByRole("menuitem", { name: re }) as HTMLButtonElement;
    await fireEvent.click(screen.getByText("Level 2"));
    await openMore();
    await fireEvent.click(item(/Move up/));
    await waitFor(() => expect(edit).toHaveBeenCalled());
    expect(edit).toHaveBeenCalledWith({
      op: "bookmark_move", id: "0.1", expect: "Level 2", anchor: "0.0", anchor_expect: "Level 1", place: "before",
    });
    // The first sibling cannot move up; a top-level item cannot outdent or move down past the end.
    await fireEvent.click(screen.getByText("Level 1"));
    await openMore();
    expect(item(/Move up/).disabled).toBe(true);
    expect(item(/Move down/).disabled).toBe(false);
    await fireEvent.click(screen.getByText("Details"));
    await openMore();
    expect(item(/Outdent/).disabled).toBe(true);
    expect(item(/Move down/).disabled).toBe(true);
    expect(item(/Indent/).disabled).toBe(false);
  });

  it("the More menu adds a bookmark for the selected mark-up, and offers it only when one is selected", async () => {
    const target: TargetInput = { kind: "page", page: 2, view: { kind: "fit_r", left: 1, bottom: 2, right: 3, top: 4 } };
    const { edit } = setup(tree(), {}, 0, () => ({ title: "Fixture A", target }));
    await fireEvent.click(screen.getByText("Details"));
    await fireEvent.click(screen.getByRole("button", { name: "More" }));
    await fireEvent.click(screen.getByRole("menuitem", { name: /for the selected mark-up/ }));
    await waitFor(() => expect(edit).toHaveBeenCalled());
    expect(edit).toHaveBeenCalledWith({
      op: "bookmark_add", anchor: "1", anchor_expect: "Details", place: "after", title: "Fixture A", target,
    });
  });

  it("without a selected mark-up that menu item is disabled", async () => {
    setup(tree(), {}, 0, () => null);
    await fireEvent.click(screen.getByRole("button", { name: "More" }));
    expect((screen.getByRole("menuitem", { name: /for the selected mark-up/ }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("Alt+arrows move the selected bookmark", async () => {
    const { edit } = setup(tree());
    await fireEvent.click(screen.getByText("Details"));
    const t = screen.getByRole("tree");
    await fireEvent.keyDown(t, { key: "ArrowUp", altKey: true });
    await waitFor(() => expect(edit).toHaveBeenCalledTimes(1));
    expect(edit.mock.calls[0][0]).toMatchObject({ op: "bookmark_move", id: "1", anchor: "0", place: "before" });
    await fireEvent.click(screen.getByText("Level 2"));
    await fireEvent.keyDown(screen.getByRole("tree"), { key: "ArrowLeft", altKey: true });
    await waitFor(() => expect(edit).toHaveBeenCalledTimes(2));
    expect(edit.mock.calls[1][0]).toMatchObject({ op: "bookmark_move", id: "0.1", anchor: "0", place: "after" });
    // Nothing to do (first top-level item cannot move up): no call.
    await fireEvent.click(screen.getByText("Plans"));
    await fireEvent.keyDown(screen.getByRole("tree"), { key: "ArrowUp", altKey: true });
    await tick();
    expect(edit).toHaveBeenCalledTimes(2);
  });

  it("dragging one bookmark onto another moves it", async () => {
    const { edit } = setup(tree());
    const from = screen.getByText("Details").closest("[role=treeitem]")!;
    const over = screen.getByText("Level 1").closest("[role=treeitem]")!;
    await fireEvent.dragStart(from);
    await fireEvent.dragOver(over);
    await fireEvent.drop(over);
    await waitFor(() => expect(edit).toHaveBeenCalled());
    expect(edit.mock.calls[0][0]).toMatchObject({ op: "bookmark_move", id: "1", anchor: "0.0" });
  });

  it("a drop onto the bookmark's own subtree does nothing", async () => {
    const { edit } = setup(tree());
    const from = screen.getByText("Plans").closest("[role=treeitem]")!;
    const over = screen.getByText("Level 1").closest("[role=treeitem]")!;
    await fireEvent.dragStart(from);
    await fireEvent.dragOver(over);
    await fireEvent.drop(over);
    await tick();
    expect(edit).not.toHaveBeenCalled();
  });

  it("is read-only for a protected file: says why and disables every edit", () => {
    setup(tree(), { read_only: "This PDF is password-protected" });
    expect(screen.getByText(/password-protected/)).toBeTruthy();
    expect((screen.getByText("Add") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByLabelText("Delete bookmark") as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows a damaged outline but refuses to edit it", async () => {
    const { edit } = setup(tree(), {
      outline: { nodes: tree(), total: 4, broken: 0, read_only: "the outline loops back on itself" },
    });
    expect(screen.getByText(/loops back on itself/)).toBeTruthy();
    expect(screen.getByText("Plans")).toBeTruthy();
    expect((screen.getByText("Add") as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(screen.getByText("Details"));
    await fireEvent.keyDown(screen.getByRole("tree"), { key: "Delete" });
    await tick();
    expect(edit).not.toHaveBeenCalled();
  });

  it("Undo and Redo buttons and Ctrl+Z reach the store", async () => {
    const { undo, redo, nav } = setup(tree(), { undo_depth: 2, redo_depth: 1 });
    expect(nav.canUndo).toBe(true);
    await fireEvent.click(screen.getByLabelText("Undo"));
    expect(undo).toHaveBeenCalledTimes(1);
    await fireEvent.click(screen.getByLabelText("Redo"));
    expect(redo).toHaveBeenCalledTimes(1);
    await fireEvent.keyDown(screen.getByRole("tree"), { key: "z", ctrlKey: true });
    expect(undo).toHaveBeenCalledTimes(2);
    await fireEvent.keyDown(screen.getByRole("tree"), { key: "z", ctrlKey: true, shiftKey: true });
    expect(redo).toHaveBeenCalledTimes(2);
  });

  it("explains an empty outline", () => {
    setup([]);
    expect(screen.getByText(/no bookmarks/i)).toBeTruthy();
  });

  it("shows an error from the store and lets it be dismissed", async () => {
    const { nav } = setup(tree());
    nav.error = "The bookmarks changed since the panel was drawn";
    await tick();
    expect(screen.getByRole("alert").textContent).toContain("changed");
    await fireEvent.click(screen.getByLabelText("Dismiss"));
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
