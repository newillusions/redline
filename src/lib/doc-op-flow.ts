/**
 * Pure helpers for the document-rewriting toolbar actions (Flatten / Optimise / Apply
 * Redactions) and the mark-up list export. Kept out of App.svelte so the wording, the
 * copy-path rule and the format sniffing are unit-testable (UX audit F1/F3, 2026-10-10).
 */
import type { ExportFormat } from "./ipc";

export type DocOpKind = "flatten" | "optimize" | "redact";

/** How a document-rewriting action is applied. "copy" writes a new file and leaves the
 *  original untouched; "replace" rewrites the original (after an automatic snapshot). */
export type DocOpMode = "copy" | "replace";

const SUFFIX: Record<DocOpKind, string> = {
  flatten: "-flattened",
  optimize: "-optimised",
  redact: "-redacted",
};

const SNAPSHOT_LABEL: Record<DocOpKind, string> = {
  flatten: "Before Flatten",
  optimize: "Before Optimise",
  redact: "Before Apply Redactions",
};

/** Label stored with the automatic version snapshot taken before an in-place rewrite. */
export function snapshotLabelFor(kind: DocOpKind): string {
  return SNAPSHOT_LABEL[kind];
}

/**
 * True when two paths name the same file as far as string comparison can tell. Separators
 * are normalised, and the comparison ignores case when either path looks like a Windows
 * path (drive letter, UNC, or backslashes), where the file system is case-insensitive.
 */
export function samePath(a: string, b: string): boolean {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const windowsLike = (p: string) => /^[A-Za-z]:|^\\\\|\\/.test(p);
  const na = norm(a);
  const nb = norm(b);
  return windowsLike(a) || windowsLike(b) ? na.toLowerCase() === nb.toLowerCase() : na === nb;
}

/** Final path segment of a Windows or POSIX path. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).at(-1) ?? path;
}

/**
 * Default destination for the "write a copy" mode: same folder, `-flattened` /
 * `-optimised` / `-redacted` inserted before the extension. A name without a `.pdf`
 * extension just gets the suffix and `.pdf` appended.
 */
export function copyPathFor(path: string, kind: DocOpKind): string {
  const sepIdx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const dir = sepIdx >= 0 ? path.slice(0, sepIdx + 1) : "";
  const name = sepIdx >= 0 ? path.slice(sepIdx + 1) : path;
  const stem = name.replace(/\.pdf$/i, "");
  return `${dir}${stem}${SUFFIX[kind]}.pdf`;
}

export interface DocOpDialogCopy {
  title: string;
  /** What the action does, in one plain sentence. */
  summary: string;
  copyLabel: string;
  replaceLabel: string;
  confirmLabel: string;
}

/** Dialog wording per action. `file` is the open document's file name. */
export function docOpDialogCopy(kind: DocOpKind, file: string): DocOpDialogCopy {
  const common = { confirmLabel: "Continue" };
  switch (kind) {
    case "flatten":
      return {
        ...common,
        title: "Flatten annotations",
        summary: `Flatten bakes every annotation in ${file} into the page content. Flattened mark-ups can no longer be edited or removed.`,
        copyLabel: `Write a copy (${copyPathFor(file, "flatten")}) and leave the original unchanged`,
        replaceLabel: "Replace the original file (a History snapshot is taken first)",
      };
    case "redact":
      return {
        ...common,
        title: "Apply redactions",
        summary: `Apply Redactions permanently covers every Redact-marked region in ${file}. The covered content cannot be recovered from the result.`,
        copyLabel: `Write a copy (${copyPathFor(file, "redact")}) and leave the original unchanged`,
        replaceLabel: "Replace the original file (a History snapshot is taken first)",
      };
    case "optimize":
      return {
        ...common,
        title: "Optimise file size",
        summary: `Optimise removes unused objects and recompresses ${file}. Images may be recompressed at the chosen quality.`,
        copyLabel: `Write a copy (${copyPathFor(file, "optimize")}) and leave the original unchanged`,
        replaceLabel: "Replace the original file (a History snapshot is taken first)",
      };
  }
}

/** Export format implied by the file extension the user picked in the save dialog. */
export function exportFormatFromPath(path: string): ExportFormat {
  return /\.csv$/i.test(path) ? "Csv" : "Xlsx";
}

/** Success message for a mark-up list export. */
export function exportDoneMessage(path: string, count: number): string {
  return `Exported ${count} mark-up${count === 1 ? "" : "s"} to ${path}.`;
}

/** Side effects `prepareDocOp` needs; injected so the ordering is unit-testable. */
export interface PrepareDocOpDeps {
  /** Current path of the document the action will run on. */
  currentPath: string;
  /** True when `dest` is the path of a DIFFERENT open tab (writing there would clobber it). */
  isOpenInAnotherTab(dest: string): boolean;
  /** Drain pending mark-up edits to the backend. */
  flush(): Promise<void>;
  /** Ask the user where the copy goes; null when they back out. */
  pickCopyDest(defaultPath: string): Promise<string | null>;
  /** Write the document (with its mark-ups) to a new path and re-home the open document there. */
  saveAs(dest: string): Promise<void>;
  /** Save the document in place. */
  save(): Promise<void>;
  /** Take a History snapshot of the saved file. */
  snapshot(label: string): Promise<void>;
  /** The tab now points at `dest`. */
  adoptPath(dest: string): void;
  /** Mark-ups are on disk now. */
  clearDirty(): void;
}

export type PrepareDocOpResult = { proceed: false } | { proceed: true; note: string };

/**
 * Make a document-rewriting action safe before it runs (UX audit F1):
 * - "copy": write the document to a new file and re-home the tab there, so the action runs
 *   on the copy and the original is never touched. Backing out of the file dialog cancels.
 *   Choosing the original's own path degrades to "replace" (so it still gets a snapshot).
 * - "replace": save in place, then take a History snapshot. A failing snapshot throws, so
 *   the caller never rewrites a file it could not snapshot.
 */
export async function prepareDocOp(
  kind: DocOpKind,
  requested: DocOpMode,
  deps: PrepareDocOpDeps,
): Promise<PrepareDocOpResult> {
  await deps.flush();
  let mode = requested;
  let note = "";
  if (mode === "copy") {
    const dest = await deps.pickCopyDest(copyPathFor(deps.currentPath, kind));
    if (!dest) return { proceed: false };
    if (samePath(dest, deps.currentPath)) {
      mode = "replace";
    } else if (deps.isOpenInAnotherTab(dest)) {
      throw new Error(`${baseName(dest)} is open in another tab. Close it or choose a different name.`);
    } else {
      await deps.saveAs(dest);
      deps.clearDirty();
      deps.adoptPath(dest);
      note = `Original left unchanged; working on ${baseName(dest)}. `;
    }
  }
  if (mode === "replace") {
    await deps.save();
    deps.clearDirty();
    await deps.snapshot(snapshotLabelFor(kind));
    note = "Snapshot saved to History first. ";
  }
  return { proceed: true, note };
}
