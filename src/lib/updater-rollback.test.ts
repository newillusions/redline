/**
 * Unit tests for the release-history/rollback IPC wrappers - invoke
 * command/argument-key correctness (Tauri v2 maps JS camelCase keys to Rust
 * snake_case params - see ipc.test.ts's guard comment for the 2026-06-15
 * incident this class of test protects against).
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listAvailableReleases, rollbackToVersion } from "./updater-rollback";

// @tauri-apps/api/core is globally mocked in src/tests/setup.ts.
const mockInvoke = vi.mocked(invoke);

describe("updater-rollback invoke argument keys", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("list_available_releases -> no args", async () => {
    mockInvoke.mockResolvedValue([]);
    await listAvailableReleases();
    expect(mockInvoke).toHaveBeenCalledWith("list_available_releases");
  });

  it("rollback_to_version -> manifestUrl / targetVersion (camelCase keys)", async () => {
    mockInvoke.mockResolvedValue(undefined as never);
    await rollbackToVersion("https://raw.githubusercontent.com/newillusions/redline/abc123/update.json", "0.3.10");
    expect(mockInvoke).toHaveBeenCalledWith("rollback_to_version", {
      manifestUrl: "https://raw.githubusercontent.com/newillusions/redline/abc123/update.json",
      targetVersion: "0.3.10",
    });
  });
});
