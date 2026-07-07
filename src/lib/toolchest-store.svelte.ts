/**
 * Reactive Tool Chest state (spec "Tools & Tool Sets"): the list of Tool Sets + the
 * Recent Tools MRU, mirrored from the Rust-side `ToolChestStore` (persisted to disk) via
 * IPC. One instance lives for the app's lifetime (not per-document - tool sets are a
 * workspace-level resource, not tied to any single open PDF).
 */
import type { Tool, ToolSet, PlacementMode, ImportReport } from "./ipc";
import * as ipc from "./ipc";

export class ToolChestStore {
  sets = $state<ToolSet[]>([]);
  recent = $state<Tool[]>([]);
  loading = $state(false);
  error = $state<string | null>(null);

  /** Load (or reload) both Tool Sets and Recent Tools from the backend. */
  async load(): Promise<void> {
    this.loading = true;
    this.error = null;
    try {
      const [sets, recent] = await Promise.all([ipc.listToolSets(), ipc.recentTools()]);
      this.sets = sets;
      this.recent = recent;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    } finally {
      this.loading = false;
    }
  }

  async createSet(name: string): Promise<ToolSet> {
    const set = await ipc.createToolSet(name);
    this.sets = [...this.sets, set];
    return set;
  }

  async renameSet(setId: string, name: string): Promise<void> {
    await ipc.renameToolSet(setId, name);
    this.sets = this.sets.map((s) => (s.id === setId ? { ...s, name } : s));
  }

  async deleteSet(setId: string): Promise<void> {
    await ipc.deleteToolSet(setId);
    this.sets = this.sets.filter((s) => s.id !== setId);
  }

  /** "Save current markup as tool" (spec "Tools & Tool Sets"). */
  async addToolFromMarkup(
    setId: string,
    markup: Parameters<typeof ipc.addToolFromMarkup>[1],
    name: string,
    placementMode: PlacementMode,
  ): Promise<Tool> {
    const tool = await ipc.addToolFromMarkup(setId, markup, name, placementMode);
    this.sets = this.sets.map((s) => (s.id === setId ? { ...s, tools: [...s.tools, tool] } : s));
    return tool;
  }

  async deleteTool(setId: string, toolId: string): Promise<void> {
    await ipc.deleteTool(setId, toolId);
    this.sets = this.sets.map((s) =>
      s.id === setId ? { ...s, tools: s.tools.filter((t) => t.id !== toolId) } : s,
    );
  }

  /** Reorder a set's tools to match `toolIds` (front to back). */
  async reorderTools(setId: string, toolIds: string[]): Promise<void> {
    await ipc.reorderTools(setId, toolIds);
    this.sets = this.sets.map((s) => {
      if (s.id !== setId) return s;
      const byId = new Map(s.tools.map((t) => [t.id, t] as const));
      const reordered = toolIds.map((id) => byId.get(id)).filter((t): t is Tool => t !== undefined);
      const namedIds = new Set(toolIds);
      const remaining = s.tools.filter((t) => !namedIds.has(t.id));
      return { ...s, tools: [...reordered, ...remaining] };
    });
  }

  /** Record a tool as recently used (move-to-front, de-duplicated, capped at 20). */
  async recordRecent(tool: Tool): Promise<void> {
    await ipc.recordRecentTool(tool);
    this.recent = [tool, ...this.recent.filter((t) => t.id !== tool.id)].slice(0, 20);
  }

  /**
   * Import a `.btx` (or `.zip`-wrapped `.btx`) file as a new Tool Set. On success (at
   * least one tool imported) the set list is reloaded from the backend - the simplest
   * correct way to pick up the new set with its backend-assigned id.
   */
  async importBtx(path: string): Promise<ImportReport> {
    const report = await ipc.importBtx(path);
    if (report.tools.length > 0) {
      await this.load();
    }
    return report;
  }
}
