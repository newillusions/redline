/**
 * Release-history + rollback IPC wrappers for the About page. Lives in its
 * own file (not `ipc.ts`) per the conflict-avoidance pattern in
 * `.claude/rules/judgment.md` - the About page/updater surface has no
 * business sharing a hot file with the render/document IPC.
 *
 * Mirrors the Rust `ReleaseInfo` struct in
 * `src-tauri/src/updater_rollback.rs` - see that module's doc comment for
 * why a commit-pinned `update.json` snapshot is what makes `rollbackToVersion`
 * a REAL downgrade rather than a relabeled reinstall of latest.
 */
import { invoke } from "@tauri-apps/api/core";

export interface ReleaseInfo {
  version: string;
  pub_date: string;
  notes: string;
  /** Commit-pinned raw manifest URL - the actual rollback install target. */
  manifest_url: string;
}

/** List past releases (newest first) discovered from this repo's
 * `update.json` commit history on GitHub. Network call - can fail (offline,
 * GitHub unreachable); callers should show a retry affordance, not crash. */
export async function listAvailableReleases(): Promise<ReleaseInfo[]> {
  return invoke<ReleaseInfo[]>("list_available_releases");
}

/** Download and install a specific past release, then the caller is
 * responsible for relaunching (mirrors the normal update flow's
 * `installAndRestart` pattern in `UpdateNotification.svelte`). `manifestUrl`
 * must be one returned by `listAvailableReleases` - it carries the real,
 * commit-pinned, CI-signed manifest for that exact release. */
export async function rollbackToVersion(manifestUrl: string, targetVersion: string): Promise<void> {
  return invoke<void>("rollback_to_version", { manifestUrl, targetVersion });
}
