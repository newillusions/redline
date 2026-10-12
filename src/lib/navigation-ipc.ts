/**
 * IPC bridge for the Navigator: bookmarks, page labels and links.
 * Mirrors `src-tauri/src/document/navigation/` and `src-tauri/src/commands/navigation.rs`;
 * behaviour spec: docs/navigation.md "Document structure".
 *
 * Kept out of `ipc.ts` (a hot file other features edit). Top-level argument keys are
 * camelCase (Tauri maps them to the snake_case Rust parameters); fields inside the `edit`
 * object are serde field names, so they stay snake_case.
 */
import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

/** How a destination positions the page. Coordinates are in the viewer's page space. */
export type PageView =
  | { kind: "xyz"; left: number | null; top: number | null; zoom: number | null }
  | { kind: "fit" }
  | { kind: "fit_h"; top: number | null }
  | { kind: "fit_v"; left: number | null }
  | { kind: "fit_r"; left: number; bottom: number; right: number; top: number }
  | { kind: "fit_b" }
  | { kind: "fit_b_h"; top: number | null }
  | { kind: "fit_b_v"; left: number | null };

/** What a bookmark or link does, as far as redline understands it. */
export type NavTarget =
  | { kind: "none" }
  | { kind: "page"; page: number; view: PageView }
  | { kind: "uri"; uri: string }
  | { kind: "blocked"; reason: string }
  | { kind: "broken" }
  | { kind: "unsupported"; action: string };

/** A target the user chose (what is sent when creating or re-pointing). */
export type TargetInput =
  | { kind: "page"; page: number; view: PageView }
  | { kind: "uri"; uri: string };

// ---------------------------------------------------------------------------
// Read model
// ---------------------------------------------------------------------------

export interface BookmarkView {
  /** Index path (`"2.0.5"`); valid until the next edit. */
  id: string;
  title: string;
  open: boolean;
  target: NavTarget;
  color: [number, number, number] | null;
  bold: boolean;
  italic: boolean;
  children: BookmarkView[];
}

export interface OutlineView {
  nodes: BookmarkView[];
  total: number;
  broken: number;
  /** Why the outline cannot be edited, when it cannot. */
  read_only: string | null;
}

export interface LinkView {
  /** `obj:<n>:<g>` for an editable link; null for one embedded inline in the page. */
  id: string | null;
  page: number;
  /** `[left, bottom, right, top]` in the viewer's page space. */
  rect: [number, number, number, number];
  target: NavTarget;
}

export interface NavigationInfo {
  page_count: number;
  outline: OutlineView;
  /** Label text per page; `""` where a page has none. */
  labels: string[];
  links: LinkView[];
  links_truncated: boolean;
  /** Set when the whole file cannot be edited (password-protected). */
  read_only: string | null;
  /** Set when the page-label tree could not be read completely, so label edits are refused. */
  labels_read_only?: string | null;
  undo_depth: number;
  redo_depth: number;
}

// ---------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------

export type Place = "before" | "after" | "child";
/** D = 1 2 3, R = I II III, r = i ii iii, A = A B C, a = a b c; null = text only. */
export type LabelStyle = "D" | "R" | "r" | "A" | "a";

export type NavEdit =
  | {
      op: "bookmark_add";
      anchor: string | null;
      anchor_expect: string | null;
      place: Place;
      title: string;
      target: TargetInput;
    }
  | { op: "bookmark_rename"; id: string; expect: string; title: string }
  | { op: "bookmark_delete"; id: string; expect: string }
  | {
      op: "bookmark_move";
      id: string;
      expect: string;
      anchor: string;
      anchor_expect: string;
      place: Place;
    }
  | { op: "bookmark_set_target"; id: string; expect: string; target: TargetInput }
  | {
      op: "bookmark_set_style";
      id: string;
      expect: string;
      color: [number, number, number] | null;
      bold: boolean;
      italic: boolean;
    }
  | { op: "bookmark_set_open"; open: string[] }
  | { op: "bookmarks_from_labels"; pages: number[] }
  | {
      op: "labels_number";
      pages: number[];
      style: LabelStyle | null;
      prefix: string;
      start: number;
    }
  | { op: "label_rename"; page: number; text: string }
  | { op: "labels_from_bookmarks"; pages: number[] }
  | { op: "link_create"; page: number; rect: [number, number, number, number]; target: TargetInput }
  | { op: "link_delete"; page: number; id: string }
  | { op: "link_set_target"; page: number; id: string; target: TargetInput };

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

export async function getNavigation(docId: string): Promise<NavigationInfo> {
  return invoke<NavigationInfo>("get_navigation", { docId });
}

/** Apply one edit (writes the PDF) and return the refreshed state. */
export async function navigationEdit(docId: string, edit: NavEdit): Promise<NavigationInfo> {
  return invoke<NavigationInfo>("navigation_edit", { docId, edit });
}

export async function navigationUndo(docId: string): Promise<NavigationInfo> {
  return invoke<NavigationInfo>("navigation_undo", { docId });
}

export async function navigationRedo(docId: string): Promise<NavigationInfo> {
  return invoke<NavigationInfo>("navigation_redo", { docId });
}

/** The question the confirmation dialog asks: the validated, normalised address and the token
 *  that answers it. */
export interface PreparedOpen {
  token: string;
  uri: string;
}

/** Step one of opening a web address (http, https, mailto): the backend validates and
 *  normalises it and remembers it as the one open question. Nothing is opened yet. */
export async function prepareExternalOpen(uri: string): Promise<PreparedOpen> {
  return invoke<PreparedOpen>("prepare_external_open", { uri });
}

/** Step two, after the user said yes: opens the address the backend stored for `token`. */
export async function confirmExternalOpen(token: string): Promise<void> {
  return invoke<void>("confirm_external_open", { token });
}
