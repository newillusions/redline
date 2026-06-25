// @vitest-environment jsdom
/**
 * VersionPanel tests (M4 S2).
 *
 * - Mounts real VersionPanel.svelte with controlled props.
 * - Mocks $lib/ipc so IPC calls are captured, not executed.
 * - Covers: empty state, version list rendering, restore button triggers IPC,
 *   snapshot button triggers IPC with optional label, label input.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import VersionPanel from "./VersionPanel.svelte";
import type { VersionRecord } from "$lib/ipc";

// ---------------------------------------------------------------------------
// Mock $lib/ipc
// ---------------------------------------------------------------------------

const mockListDocumentVersions = vi.fn(async (_docId: string): Promise<VersionRecord[]> => []);
const mockSnapshotVersion = vi.fn(
  async (_docId: string, _label: string | null): Promise<VersionRecord> => ({
    id: "snap1",
    created_at: "2026-06-25T12:00:00Z",
    label: null,
    filename: "0000001__2026-06-25T12-00-00Z__snap1.pdf",
  })
);
const mockRestoreDocumentVersion = vi.fn(async (_docId: string, _versionId: string): Promise<void> => {});

vi.mock("$lib/ipc", () => ({
  listDocumentVersions: (docId: string) => mockListDocumentVersions(docId),
  snapshotVersion: (docId: string, label: string | null) => mockSnapshotVersion(docId, label),
  restoreDocumentVersion: (docId: string, versionId: string) =>
    mockRestoreDocumentVersion(docId, versionId),
  // other ipc stubs
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
  rotatePage: vi.fn(),
  deletePage: vi.fn(),
  reorderPages: vi.fn(),
  insertBlankPage: vi.fn(),
  addScale: vi.fn(),
  listScales: vi.fn(),
  deleteScale: vi.fn(),
  exportMarkupList: vi.fn(),
  listApplicableScales: vi.fn(),
  writePageMeasure: vi.fn(),
  getPageCount: vi.fn(),
  searchDocument: vi.fn(),
}));

// ---------------------------------------------------------------------------
// Sample version records
// ---------------------------------------------------------------------------

const sampleVersions: VersionRecord[] = [
  {
    id: "ver3",
    created_at: "2026-06-25T14:00:00Z",
    label: "client v2",
    filename: "0000003__2026-06-25T14-00-00Z__ver3.pdf",
  },
  {
    id: "ver2",
    created_at: "2026-06-25T13:00:00Z",
    label: null,
    filename: "0000002__2026-06-25T13-00-00Z__ver2.pdf",
  },
  {
    id: "ver1",
    created_at: "2026-06-25T12:00:00Z",
    label: "pre-issue",
    filename: "0000001__2026-06-25T12-00-00Z__ver1.pdf",
  },
];

beforeEach(() => {
  mockListDocumentVersions.mockReset();
  mockSnapshotVersion.mockReset();
  mockRestoreDocumentVersion.mockReset();
  mockListDocumentVersions.mockResolvedValue([]);
  mockSnapshotVersion.mockResolvedValue({
    id: "snap1",
    created_at: "2026-06-25T12:00:00Z",
    label: null,
    filename: "0000001__2026-06-25T12-00-00Z__snap1.pdf",
  });
  mockRestoreDocumentVersion.mockResolvedValue(undefined);
});

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("VersionPanel", () => {
  it("VP-1: renders empty state when no versions", async () => {
    mockListDocumentVersions.mockResolvedValue([]);
    render(VersionPanel, { props: { docId: "doc1" } });
    await tick();
    await tick();

    // Should show some empty state text (no version entries)
    const items = document.querySelectorAll("[data-testid='version-item']");
    expect(items.length).toBe(0);
  });

  it("VP-2: renders version list after load", async () => {
    mockListDocumentVersions.mockResolvedValue(sampleVersions);
    render(VersionPanel, { props: { docId: "doc1" } });
    await tick();
    await tick();
    await tick();

    const items = document.querySelectorAll("[data-testid='version-item']");
    expect(items.length).toBe(3);
  });

  it("VP-3: version label is displayed when present", async () => {
    mockListDocumentVersions.mockResolvedValue(sampleVersions);
    render(VersionPanel, { props: { docId: "doc1" } });
    await tick();
    await tick();
    await tick();

    expect(screen.getByText("client v2")).toBeTruthy();
    expect(screen.getByText("pre-issue")).toBeTruthy();
  });

  it("VP-4: restore button calls restoreDocumentVersion with correct id", async () => {
    mockListDocumentVersions.mockResolvedValue(sampleVersions);
    const onRestore = vi.fn();
    render(VersionPanel, { props: { docId: "doc1", onRestore } });
    await tick();
    await tick();
    await tick();

    const restoreButtons = document.querySelectorAll("[data-testid='restore-btn']");
    expect(restoreButtons.length).toBeGreaterThan(0);

    // Click the first restore button (corresponds to ver3 = newest first)
    await fireEvent.click(restoreButtons[0]);
    await tick();
    await tick();

    expect(mockRestoreDocumentVersion).toHaveBeenCalledWith("doc1", "ver3");
  });

  it("VP-5: snapshot button calls snapshotVersion with null label by default", async () => {
    mockListDocumentVersions.mockResolvedValue([]);
    render(VersionPanel, { props: { docId: "doc1" } });
    await tick();
    await tick();

    const snapshotBtn = document.querySelector("[data-testid='snapshot-btn']");
    expect(snapshotBtn).not.toBeNull();

    await fireEvent.click(snapshotBtn!);
    await tick();
    await tick();

    expect(mockSnapshotVersion).toHaveBeenCalledWith("doc1", null);
  });

  it("VP-6: snapshot with label passes label to IPC", async () => {
    mockListDocumentVersions.mockResolvedValue([]);
    render(VersionPanel, { props: { docId: "doc1" } });
    await tick();
    await tick();

    // Enter a label in the input
    const labelInput = document.querySelector("[data-testid='label-input']") as HTMLInputElement;
    expect(labelInput).not.toBeNull();
    await fireEvent.input(labelInput, { target: { value: "my snapshot" } });
    await tick();

    const snapshotBtn = document.querySelector("[data-testid='snapshot-btn']");
    await fireEvent.click(snapshotBtn!);
    await tick();
    await tick();

    expect(mockSnapshotVersion).toHaveBeenCalledWith("doc1", "my snapshot");
  });

  it("VP-7: list refreshes after snapshot", async () => {
    mockListDocumentVersions.mockResolvedValue([]);
    const newVer: VersionRecord = {
      id: "ver_new",
      created_at: "2026-06-25T15:00:00Z",
      label: null,
      filename: "0000001__2026-06-25T15-00-00Z__ver_new.pdf",
    };
    mockSnapshotVersion.mockResolvedValue(newVer);

    render(VersionPanel, { props: { docId: "doc1" } });
    await tick();
    await tick();

    // No items initially
    expect(document.querySelectorAll("[data-testid='version-item']").length).toBe(0);

    // After snapshot, the list should reload
    mockListDocumentVersions.mockResolvedValue([newVer]);
    const snapshotBtn = document.querySelector("[data-testid='snapshot-btn']");
    await fireEvent.click(snapshotBtn!);
    await tick();
    await tick();
    await tick();

    expect(document.querySelectorAll("[data-testid='version-item']").length).toBe(1);
  });

  it("VP-8: onRestore callback is invoked after successful restore", async () => {
    mockListDocumentVersions.mockResolvedValue(sampleVersions);
    const onRestore = vi.fn();
    render(VersionPanel, { props: { docId: "doc1", onRestore } });
    await tick();
    await tick();
    await tick();

    const restoreButtons = document.querySelectorAll("[data-testid='restore-btn']");
    await fireEvent.click(restoreButtons[0]);
    await tick();
    await tick();

    expect(onRestore).toHaveBeenCalled();
  });
});
