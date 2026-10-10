import { describe, it, expect, vi } from "vitest";
import {
  copyPathFor,
  baseName,
  docOpDialogCopy,
  exportFormatFromPath,
  exportDoneMessage,
  snapshotLabelFor,
  prepareDocOp,
  samePath,
  type PrepareDocOpDeps,
} from "./doc-op-flow";
import { dirtyDocIds, advanceQuit } from "./close-guard";
import { reasonText, gateHint, normaliseCode } from "./licence-copy";

describe("copyPathFor", () => {
  it("inserts the suffix before .pdf in the same folder (POSIX)", () => {
    expect(copyPathFor("/jobs/a/plan.pdf", "flatten")).toBe("/jobs/a/plan-flattened.pdf");
    expect(copyPathFor("/jobs/a/plan.pdf", "redact")).toBe("/jobs/a/plan-redacted.pdf");
    expect(copyPathFor("/jobs/a/plan.pdf", "optimize")).toBe("/jobs/a/plan-optimised.pdf");
  });
  it("handles Windows separators and an upper-case extension", () => {
    expect(copyPathFor("C:\\jobs\\Plan.PDF", "flatten")).toBe("C:\\jobs\\Plan-flattened.pdf");
  });
  it("handles a bare file name and a name with no extension", () => {
    expect(copyPathFor("plan.pdf", "flatten")).toBe("plan-flattened.pdf");
    expect(copyPathFor("/x/plan", "flatten")).toBe("/x/plan-flattened.pdf");
  });
});

describe("docOpDialogCopy", () => {
  it("names the file and the consequence for every action", () => {
    for (const kind of ["flatten", "redact", "optimize"] as const) {
      const c = docOpDialogCopy(kind, "plan.pdf");
      expect(c.summary).toContain("plan.pdf");
      expect(c.copyLabel).toContain("leave the original unchanged");
      expect(c.replaceLabel).toContain("snapshot");
    }
    expect(docOpDialogCopy("flatten", "p.pdf").summary).toMatch(/can no longer be edited/);
    expect(docOpDialogCopy("redact", "p.pdf").summary).toMatch(/cannot be recovered/);
  });
  it("uses UK spelling in user-facing copy", () => {
    const text = JSON.stringify(docOpDialogCopy("optimize", "p.pdf"));
    expect(text).toMatch(/Optimise/);
    expect(text).not.toMatch(/Optimiz/);
  });
});

describe("misc doc-op helpers", () => {
  it("baseName handles both separators", () => {
    expect(baseName("C:\\a\\b.pdf")).toBe("b.pdf");
    expect(baseName("/a/b.pdf")).toBe("b.pdf");
  });
  it("snapshot labels name the action", () => {
    expect(snapshotLabelFor("flatten")).toBe("Before Flatten");
  });
  it("sniffs the export format from the chosen extension", () => {
    expect(exportFormatFromPath("/x/list.csv")).toBe("Csv");
    expect(exportFormatFromPath("/x/list.CSV")).toBe("Csv");
    expect(exportFormatFromPath("/x/list.xlsx")).toBe("Xlsx");
    expect(exportFormatFromPath("/x/list")).toBe("Xlsx");
  });
  it("reports the export with the path and a pluralised count", () => {
    expect(exportDoneMessage("/x/l.xlsx", 1)).toBe("Exported 1 mark-up to /x/l.xlsx.");
    expect(exportDoneMessage("/x/l.xlsx", 3)).toBe("Exported 3 mark-ups to /x/l.xlsx.");
  });
});

describe("close guard", () => {
  const tabs = [
    { docId: "a", store: { dirty: true } },
    { docId: "b", store: { dirty: false } },
    { docId: "c", store: { dirty: true } },
  ];
  it("lists only dirty tabs in tab order", () => {
    expect(dirtyDocIds(tabs)).toEqual(["a", "c"]);
    expect(dirtyDocIds([])).toEqual([]);
  });
  it("walks the queue and quits after the last answer", () => {
    expect(advanceQuit(["a", "c"], "save")).toEqual({ action: "prompt", queue: ["c"] });
    expect(advanceQuit(["c"], "discard")).toEqual({ action: "quit" });
  });
  it("cancel aborts the whole quit", () => {
    expect(advanceQuit(["a", "c"], "cancel")).toEqual({ action: "abort" });
  });
});

describe("licence copy", () => {
  it("maps known codes and hides unknown ones", () => {
    expect(reasonText("staff_not_active")).toMatch(/no longer active/);
    expect(reasonText("zzz")).toBe("The activation could not be verified.");
  });
  it("tells a mismatched device to quote the Device ID", () => {
    expect(gateHint({ state: "invalid", reason: "device_mismatch" })).toMatch(/Device ID/);
  });
  it("normalises pasted codes without changing case", () => {
    expect(normaliseCode(" ab 12\n")).toBe("ab12");
  });
});

describe("prepareDocOp (UX audit F1)", () => {
  function deps(over: Partial<PrepareDocOpDeps> = {}) {
    const calls: string[] = [];
    const d: PrepareDocOpDeps = {
      currentPath: "/jobs/plan.pdf",
      isOpenInAnotherTab: vi.fn(() => false),
      flush: vi.fn(async () => void calls.push("flush")),
      pickCopyDest: vi.fn(async (def: string) => (calls.push(`pick:${def}`), "/jobs/out.pdf")),
      saveAs: vi.fn(async (dest: string) => void calls.push(`saveAs:${dest}`)),
      save: vi.fn(async () => void calls.push("save")),
      snapshot: vi.fn(async (label: string) => void calls.push(`snapshot:${label}`)),
      adoptPath: vi.fn((dest: string) => void calls.push(`adopt:${dest}`)),
      clearDirty: vi.fn(() => void calls.push("clean")),
      ...over,
    };
    return { d, calls };
  }

  it("copy mode: writes the copy and re-homes the tab, never saves or snapshots the original", async () => {
    const { d, calls } = deps();
    const r = await prepareDocOp("flatten", "copy", d);
    expect(r).toEqual({ proceed: true, note: "Original left unchanged; working on out.pdf. " });
    expect(calls).toEqual([
      "flush",
      "pick:/jobs/plan-flattened.pdf",
      "saveAs:/jobs/out.pdf",
      "clean",
      "adopt:/jobs/out.pdf",
    ]);
    expect(d.save).not.toHaveBeenCalled();
    expect(d.snapshot).not.toHaveBeenCalled();
  });

  it("copy mode: backing out of the file dialog changes nothing", async () => {
    const { d } = deps({ pickCopyDest: vi.fn(async () => null) });
    expect(await prepareDocOp("redact", "copy", d)).toEqual({ proceed: false });
    expect(d.saveAs).not.toHaveBeenCalled();
    expect(d.save).not.toHaveBeenCalled();
    expect(d.snapshot).not.toHaveBeenCalled();
  });

  it("copy mode: picking the original's own path still takes a snapshot first", async () => {
    const { d, calls } = deps({ pickCopyDest: vi.fn(async () => "/jobs/plan.pdf") });
    const r = await prepareDocOp("flatten", "copy", d);
    expect(r.proceed).toBe(true);
    expect(d.saveAs).not.toHaveBeenCalled();
    expect(calls).toEqual(["flush", "save", "clean", "snapshot:Before Flatten"]);
  });

  it("replace mode: saves, then snapshots, in that order", async () => {
    const { d, calls } = deps();
    const r = await prepareDocOp("optimize", "replace", d);
    expect(r).toEqual({ proceed: true, note: "Snapshot saved to History first. " });
    expect(calls).toEqual(["flush", "save", "clean", "snapshot:Before Optimise"]);
    expect(d.pickCopyDest).not.toHaveBeenCalled();
  });

  it("replace mode: a failing snapshot throws so the caller never rewrites the file", async () => {
    const { d } = deps({ snapshot: vi.fn(async () => { throw new Error("disk full"); }) });
    await expect(prepareDocOp("flatten", "replace", d)).rejects.toThrow("disk full");
  });
});

describe("samePath and copy-mode edge cases", () => {
  it("is case- and separator-insensitive for Windows-style paths only", () => {
    expect(samePath("C:\\Jobs\\Plan.pdf", "c:/jobs/plan.PDF")).toBe(true);
    expect(samePath("C:\\Jobs\\Plan.pdf", "C:\\Jobs\\Other.pdf")).toBe(false);
    expect(samePath("/jobs/Plan.pdf", "/jobs/plan.pdf")).toBe(false); // POSIX is case-sensitive
    expect(samePath("/jobs/plan.pdf", "/jobs/plan.pdf")).toBe(true);
  });

  it("copy mode: a differently-cased path to the original degrades to replace with a snapshot", async () => {
    const calls: string[] = [];
    const d: PrepareDocOpDeps = {
      currentPath: "C:\\Jobs\\Plan.pdf",
      isOpenInAnotherTab: () => false,
      flush: async () => void calls.push("flush"),
      pickCopyDest: async () => "c:/jobs/plan.pdf",
      saveAs: async () => void calls.push("saveAs"),
      save: async () => void calls.push("save"),
      snapshot: async (l) => void calls.push(`snapshot:${l}`),
      adoptPath: () => void calls.push("adopt"),
      clearDirty: () => void calls.push("clean"),
    };
    await prepareDocOp("flatten", "copy", d);
    expect(calls).toEqual(["flush", "save", "clean", "snapshot:Before Flatten"]);
  });

  it("copy mode: refuses a destination that is another open tab's file", async () => {
    const saveAs = vi.fn();
    const d: PrepareDocOpDeps = {
      currentPath: "/jobs/plan.pdf",
      isOpenInAnotherTab: (dest) => dest === "/jobs/other.pdf",
      flush: async () => {},
      pickCopyDest: async () => "/jobs/other.pdf",
      saveAs,
      save: async () => {},
      snapshot: async () => {},
      adoptPath: () => {},
      clearDirty: () => {},
    };
    await expect(prepareDocOp("flatten", "copy", d)).rejects.toThrow(/open in another tab/);
    expect(saveAs).not.toHaveBeenCalled();
  });
});
