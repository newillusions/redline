// @vitest-environment jsdom
/** LinksPanel tests (docs/navigation.md RH4): list, filter, select-to-jump, Go, Edit, Delete. */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import LinksPanel from "./LinksPanel.svelte";
import { NavigationStore } from "$lib/navigation-store.svelte";
import type { LinkView, NavigationInfo, NavTarget } from "$lib/navigation-ipc";

const page = (p: number): NavTarget => ({ kind: "page", page: p, view: { kind: "fit" } });

const links: LinkView[] = [
  { id: "obj:10:0", page: 0, rect: [10, 10, 60, 40], target: page(2) },
  { id: "obj:11:0", page: 1, rect: [5, 5, 50, 30], target: { kind: "uri", uri: "https://example.com/spec" } },
  { id: "obj:12:0", page: 1, rect: [5, 50, 50, 70], target: { kind: "unsupported", action: "Launch" } },
  { id: null, page: 2, rect: [1, 1, 20, 20], target: page(0) },
];

function setup(over: Partial<NavigationInfo> = {}) {
  const nav = new NavigationStore("d1");
  nav.info = {
    page_count: 3,
    outline: { nodes: [], total: 0, broken: 0, read_only: null },
    labels: ["E-1", "E-2", "E-3"],
    links,
    links_truncated: false,
    read_only: null,
    undo_depth: 0,
    redo_depth: 0,
    ...over,
  };
  nav.loaded = true;
  const edit = vi.spyOn(nav, "edit").mockResolvedValue(true);
  const onjump = vi.fn();
  const onfollow = vi.fn();
  const onedit = vi.fn();
  render(LinksPanel, { nav, onjump, onfollow, onedit });
  return { nav, edit, onjump, onfollow, onedit };
}

describe("LinksPanel", () => {
  beforeEach(() => vi.clearAllMocks());

  it("lists every link with its page label and target", () => {
    setup();
    expect(screen.getByText("Page E-3")).toBeTruthy(); // the first link points at page index 2
    expect(screen.getByText("https://example.com/spec")).toBeTruthy();
    expect(screen.getByText(/Not supported \(Launch\)/)).toBeTruthy();
    expect(screen.getByText("4 links")).toBeTruthy();
  });

  it("filters by page label and target text", async () => {
    setup();
    await fireEvent.input(screen.getByLabelText("Filter links"), { target: { value: "example.com" } });
    expect(screen.getAllByRole("option")).toHaveLength(1);
    await fireEvent.input(screen.getByLabelText("Filter links"), { target: { value: "nothing like this" } });
    expect(screen.getByText("No links match.")).toBeTruthy();
  });

  it("selecting a link jumps to its rectangle", async () => {
    const { onjump } = setup();
    await fireEvent.click(screen.getByText("https://example.com/spec"));
    expect(onjump).toHaveBeenCalledWith({ page: 1, rect: [5, 5, 50, 30] });
  });

  it("Go runs a followable target and is off for an unsupported one", async () => {
    const { onfollow } = setup();
    await fireEvent.click(screen.getByText("https://example.com/spec"));
    await fireEvent.click(screen.getByText("Go"));
    expect(onfollow).toHaveBeenCalledWith({ kind: "uri", uri: "https://example.com/spec" });
    await fireEvent.click(screen.getByText(/Not supported \(Launch\)/));
    expect((screen.getByText("Go") as HTMLButtonElement).disabled).toBe(true);
  });

  it("Delete removes an editable link and Edit action asks the host", async () => {
    const { edit, onedit } = setup();
    await fireEvent.click(screen.getByText("https://example.com/spec"));
    await fireEvent.click(screen.getByText("Edit action..."));
    expect(onedit).toHaveBeenCalledWith(links[1]);
    await fireEvent.click(screen.getByText("Delete"));
    await waitFor(() => expect(edit).toHaveBeenCalled());
    expect(edit).toHaveBeenCalledWith({ op: "link_delete", page: 1, id: "obj:11:0" });
  });

  it("a link embedded in the page cannot be deleted or edited", async () => {
    const { edit } = setup();
    // The inline link (no id) is the only one pointing at page 1 from page 3.
    await fireEvent.click(screen.getAllByRole("option")[3].querySelector("button")!);
    expect((screen.getByText("Delete") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByText("Edit action...") as HTMLButtonElement).disabled).toBe(true);
    expect(edit).not.toHaveBeenCalled();
  });

  it("Undo and Redo reach the store", async () => {
    const nav = new NavigationStore("d1");
    nav.info = { page_count: 1, outline: { nodes: [], total: 0, broken: 0, read_only: null }, labels: [""], links, links_truncated: false, read_only: null, undo_depth: 1, redo_depth: 1 };
    nav.loaded = true;
    const undo = vi.spyOn(nav, "undo").mockResolvedValue(true);
    const redo = vi.spyOn(nav, "redo").mockResolvedValue(true);
    render(LinksPanel, { nav, onjump: vi.fn(), onfollow: vi.fn(), onedit: vi.fn() });
    await fireEvent.click(screen.getByLabelText("Undo"));
    await fireEvent.click(screen.getByLabelText("Redo"));
    expect(undo).toHaveBeenCalledTimes(1);
    expect(redo).toHaveBeenCalledTimes(1);
  });

  it("is read-only for a protected file", () => {
    setup({ read_only: "This PDF is password-protected" });
    expect(screen.getByText(/password-protected/)).toBeTruthy();
    expect((screen.getByText("Add link") as HTMLButtonElement).disabled).toBe(true);
  });

  it("Add link toggles link mode", async () => {
    const { nav } = setup();
    await fireEvent.click(screen.getByText("Add link"));
    expect(nav.linkMode).toBe(true);
    expect(screen.getByText(/Drag on the page/)).toBeTruthy();
    await fireEvent.click(screen.getByText("Add link"));
    expect(nav.linkMode).toBe(false);
  });

  it("renders hostile targets as text", () => {
    const evil = '<img src=x onerror="window.__pwned=1">';
    const nav = new NavigationStore("d1");
    nav.info = {
      page_count: 1, outline: { nodes: [], total: 0, broken: 0, read_only: null }, labels: [""],
      links: [{ id: "obj:1:0", page: 0, rect: [0, 0, 5, 5], target: { kind: "unsupported", action: evil } }],
      links_truncated: false, read_only: null, undo_depth: 0, redo_depth: 0,
    };
    nav.loaded = true;
    const { container } = render(LinksPanel, { nav, onjump: vi.fn(), onfollow: vi.fn(), onedit: vi.fn() });
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain(evil);
  });
});
