/**
 * Per-document Navigator state: bookmarks, page labels and links, loaded from the backend
 * and replaced by every edit's response. One instance per open tab (`DocTab.navStore`).
 *
 * Edits write the PDF immediately (the backend runs them through the same path as the page
 * operations), so each call is async and `busy` guards against overlapping ones.
 */
import {
  getNavigation,
  navigationEdit,
  navigationRedo,
  navigationUndo,
  type BookmarkView,
  type LinkView,
  type NavEdit,
  type NavigationInfo,
} from "./navigation-ipc";

const NO_LABELS: readonly string[] = [];
const NO_LINKS: readonly LinkView[] = [];

export class NavigationStore {
  readonly docId: string;
  info = $state<NavigationInfo | null>(null);
  /** True once the first read finished (or failed), so callers do not retry in a loop. */
  loaded = $state(false);
  busy = $state(false);
  error = $state<string | null>(null);
  /** The Link tool is active: drag on the page to make a link. */
  linkMode = $state(false);
  /** A link area just drawn, waiting for its target to be chosen. */
  pendingLink = $state<{ page: number; rect: [number, number, number, number] } | null>(null);
  /** Monotonic counter bumped whenever `info` is replaced (lets views reconcile state). */
  revision = $state(0);

  constructor(docId: string) {
    this.docId = docId;
  }

  get bookmarks(): readonly BookmarkView[] {
    return this.info?.outline.nodes ?? [];
  }

  get labels(): readonly string[] {
    return this.info?.labels ?? NO_LABELS;
  }

  get links(): readonly LinkView[] {
    return this.info?.links ?? NO_LINKS;
  }

  /** Why nothing here can be edited, or null. */
  get readOnlyReason(): string | null {
    return this.info?.read_only ?? this.info?.outline.read_only ?? null;
  }

  /** True when bookmarks can be edited (links and labels only need the file writable). */
  get fileEditable(): boolean {
    return this.info !== null && this.info.read_only === null;
  }

  get canUndo(): boolean {
    return (this.info?.undo_depth ?? 0) > 0 && !this.busy;
  }

  get canRedo(): boolean {
    return (this.info?.redo_depth ?? 0) > 0 && !this.busy;
  }

  private replace(info: NavigationInfo) {
    this.info = info;
    this.revision += 1;
  }

  private pending: Promise<void> | null = null;

  /** (Re)read everything from the file. Overlapping calls share one read. */
  refresh(): Promise<void> {
    if (this.pending) return this.pending;
    this.pending = (async () => {
      try {
        this.replace(await getNavigation(this.docId));
        this.error = null;
      } catch (e) {
        this.error = e instanceof Error ? e.message : String(e);
      } finally {
        this.loaded = true;
        this.pending = null;
      }
    })();
    return this.pending;
  }

  private async run(op: () => Promise<NavigationInfo>): Promise<boolean> {
    if (this.busy) return false;
    this.busy = true;
    this.error = null;
    try {
      this.replace(await op());
      return true;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
      // A refused or stale edit means the panel may be showing old state: show the file's.
      void getNavigation(this.docId).then((i) => this.replace(i)).catch(() => {});
      return false;
    } finally {
      this.busy = false;
    }
  }

  edit(e: NavEdit): Promise<boolean> {
    return this.run(() => navigationEdit(this.docId, e));
  }

  undo(): Promise<boolean> {
    return this.run(() => navigationUndo(this.docId));
  }

  redo(): Promise<boolean> {
    return this.run(() => navigationRedo(this.docId));
  }

  clearError() {
    this.error = null;
  }

  beginLinkMode() {
    this.pendingLink = null;
    this.linkMode = true;
  }

  endLinkMode() {
    this.linkMode = false;
  }
}
