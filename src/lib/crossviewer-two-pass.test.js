import { describe, it, expect } from "vitest";
import { runTwoPass, closeAndVerify, parseTasklistPids } from "../../tools/crossviewer/two-pass.mjs";

const FAST = { settleMs: 0 };

// Fake Acrobat host. `running` is a Set of pids. A leg that launches Acrobat adds a pid
// and reports it in launched_pids (as the real legs do). closeAcrobat(pids) removes ONLY
// the pids it is given, so a test can prove nothing else was touched.
function makeSteps(opts = {}) {
  const calls = [];
  const closeArgs = [];
  const running = new Set(opts.startRunning || []);
  let nextPid = 1000;
  const launch = () => { const p = String(nextPid++); running.add(p); return p; };
  const steps = {
    calls,
    closeArgs,
    running,
    async listAcrobatProcesses() {
      calls.push("list");
      if (opts.listThrows) throw new Error("tasklist boom");
      return [...running];
    },
    async closeAcrobat(pids) {
      calls.push("close");
      closeArgs.push(pids);
      if (opts.closeThrows) throw new Error("close boom");
      if (!opts.neverCloses) for (const p of pids) running.delete(p);
    },
    // Models the real legs: launched_pids is the launch-time snapshot INTERSECTED with what is
    // still alive at the end, never "whatever is alive". opts.ownerDuringLeg(running) lets a
    // test start an owner Acrobat while the leg runs; opts.ownerAfterExit does so in the gap
    // after the counts leg's App.Exit().
    async countsPass(files) {
      calls.push("counts");
      const snapshot = [launch()];
      if (opts.ownerDuringLeg) opts.ownerDuringLeg(running);
      if (opts.countsThrows) throw new Error("iac boom");
      if (!opts.appExitFails) for (const p of snapshot) running.delete(p); // $app.Exit()
      if (opts.ownerAfterExit) opts.ownerAfterExit(running);
      return {
        launched_pids: snapshot.filter((p) => running.has(p)),
        results: files.map((f) => ({
          file: f, opened: true, pages: 2, annots_total: 5,
          annots_by_page: [{ page: 0, annots: 3 }, { page: 1, annots: 2 }],
          open_error: opts.openError === f ? "AVDoc.Open returned false" : null,
        })),
      };
    },
    async renderPass(file) {
      calls.push(`render:${file}`);
      if (running.size) throw new Error("render launched while an Acrobat was still running");
      const snapshot = [launch()];
      if (opts.renderThrows === file) throw new Error("capture boom");
      const pages = opts.renderPages ?? 2;
      return {
        launched_pids: snapshot.filter((p) => running.has(p)),
        results: [{ file, renders: Array.from({ length: pages }, (_, i) => `${file}-${i}.png`), render_detail: [], render_error: null }],
      };
    },
  };
  return steps;
}
const strip = (c) => c.filter((x) => x !== "list");

describe("owner-session safety", () => {
  it("pre-existing Acrobat: counts and render skipped AND close never called", async () => {
    const s = makeSteps({ startRunning: ["4242"] });
    const r = await runTwoPass(["a.pdf", "b.pdf"], s, undefined, FAST);
    expect(s.calls).not.toContain("close");
    expect(s.calls).not.toContain("counts");
    expect(s.calls.some((c) => c.startsWith("render:"))).toBe(false);
    expect(s.running.has("4242")).toBe(true); // owner's Acrobat untouched
    expect(r.ok).toBe(false);
    expect(r.payload.skipped).toBe(true);
    for (const e of r.payload.results) {
      expect(e.open_error).toBe("skipped: Acrobat already running (owner session)");
      expect(e.render_error).toBe("skipped: Acrobat already running (owner session)");
    }
  });

  it("an unverifiable preflight skips the leg without closing anything", async () => {
    const s = makeSteps({ listThrows: true });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    expect(s.calls).not.toContain("close");
    expect(s.calls).not.toContain("counts");
    expect(r.payload.results[0].open_error).toMatch(/cannot check for a running Acrobat/);
  });

  it("close only targets harness-launched PIDs", async () => {
    const s = makeSteps();
    const r = await runTwoPass(["a.pdf", "b.pdf"], s, undefined, FAST);
    expect(r.ok).toBe(true);
    // every close was handed exactly the pid the previous leg launched
    // counts leg exits its own Acrobat (nothing left to close); each render leg leaves one
    expect(s.closeArgs).toEqual([["1001"], ["1002"]]);
  });

  it("an owner Acrobat that appears DURING the leg is not reported as launched and is never closed", async () => {
    const s = makeSteps({ appExitFails: true, ownerDuringLeg: (r) => r.add("7777") });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    expect(s.closeArgs.flat()).not.toContain("7777");
    expect(s.closeArgs).toEqual([]); // refuse the whole close: a foreign pid is present
    expect(s.running.has("7777")).toBe(true);
    expect(s.calls.some((c) => c.startsWith("render:"))).toBe(false);
    expect(r.payload.results[0].render_error).toMatch(/7777 were not launched by the harness/);
  });

  it("post-Exit gap: an owner Acrobat started after App.Exit() is never reported or closed", async () => {
    const s = makeSteps({ ownerAfterExit: (r) => r.add("8888") });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    expect(s.closeArgs.flat()).not.toContain("8888");
    expect(s.closeArgs).toEqual([]);
    expect(s.running.has("8888")).toBe(true);
    expect(r.ok).toBe(false);
    expect(r.payload.results[0].render_error).toMatch(/8888 were not launched by the harness/);
    expect(s.calls.some((c) => c.startsWith("render:"))).toBe(false);
  });

  it("a failed leg that reported no pids never causes a close of what is running", async () => {
    const s = makeSteps({ countsThrows: true });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    expect(s.closeArgs).toEqual([]);
    expect(r.payload.results[0].render_error).toMatch(/not launched by the harness/);
  });
});

describe("two-pass orchestration", () => {
  it("runs counts (leg exits its own Acrobat), then render per file with a verified close after each", async () => {
    const s = makeSteps();
    const r = await runTwoPass(["a.pdf", "b.pdf"], s, undefined, FAST);
    expect(strip(s.calls)).toEqual(["counts", "render:a.pdf", "close", "render:b.pdf", "close"]);
    expect(r.ok).toBe(true);
    expect(r.errors).toEqual([]);
  });

  it("merges counts from phase 1 with renders from phase 2", async () => {
    const r = await runTwoPass(["a.pdf"], makeSteps(), undefined, FAST);
    const e = r.payload.results[0];
    expect(e.pages).toBe(2);
    expect(e.annots_total).toBe(5);
    expect(e.renders).toHaveLength(2);
    expect(e.phases).toEqual({ counts: { ok: true, error: null }, render: { ok: true, error: null } });
  });

  it("skips phase 2 and reports it if the harness Acrobat survives phase 1", async () => {
    const s = makeSteps({ appExitFails: true, neverCloses: true });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    const e = r.payload.results[0];
    expect(e.annots_total).toBe(5); // phase 1 data kept
    expect(e.render_error).toMatch(/Acrobat not closed after phase 1/);
    expect(s.calls.some((c) => c.startsWith("render:"))).toBe(false);
    expect(r.ok).toBe(false);
  });

  it("reports a per-file open error from phase 1", async () => {
    const r = await runTwoPass(["a.pdf", "b.pdf"], makeSteps({ openError: "a.pdf" }), undefined, FAST);
    const [a, b] = r.payload.results;
    expect(a.open_error).toMatch(/returned false/);
    expect(b.phases.counts.ok).toBe(true);
  });

  it("reports a render failure, keeps the other phase's data, and stops rather than close a pid it was not told about", async () => {
    const s = makeSteps({ renderThrows: "a.pdf" });
    const r = await runTwoPass(["a.pdf", "b.pdf"], s, undefined, FAST);
    const [a, b] = r.payload.results;
    expect(a.render_error).toMatch(/capture boom/);
    expect(a.annots_total).toBe(5);
    expect(b.annots_total).toBe(5);
    expect(b.render_error).toMatch(/skipped: Acrobat not closed after a.pdf/);
    // the failed leg's launched pid was not reported, so it is not ours to close: stop safely
    // rather than kill it, and say so.
    expect(strip(s.calls)).toEqual(["counts", "render:a.pdf"]);
    expect(r.errors.some((x) => x.startsWith("a.pdf [render]"))).toBe(true);
  });

  it("flags a single-page capture of a multi-page document as partial", async () => {
    const r = await runTwoPass(["a.pdf"], makeSteps({ renderPages: 1 }), undefined, FAST);
    expect(r.payload.results[0].render_error).toMatch(/captured 1 of 2 page/);
    expect(r.ok).toBe(false);
  });

  it("stops launching further renders if Acrobat will not close after one", async () => {
    const s = makeSteps();
    s.closeAcrobat = async () => {}; // the close never takes effect
    const r = await runTwoPass(["a.pdf", "b.pdf", "c.pdf"], s, undefined, FAST);
    expect(strip(s.calls).filter((c) => c.startsWith("render:"))).toEqual(["render:a.pdf"]);
    expect(r.payload.results[1].render_error).toMatch(/skipped: Acrobat not closed after a.pdf/);
    expect(r.payload.results[2].render_error).toMatch(/skipped/);
  });
});

describe("closeAndVerify", () => {
  it("treats an unverifiable listing as not closed", async () => {
    const r = await closeAndVerify(makeSteps({ listThrows: true }), [], { settleMs: 0 });
    expect(r.ok).toBe(false);
    expect(r.error).toMatch(/cannot verify/);
  });
  it("includes the close error when the launched Acrobat survives", async () => {
    const r = await closeAndVerify(makeSteps({ startRunning: ["5"], closeThrows: true }), ["5"], { settleMs: 0 });
    expect(r.ok).toBe(false);
    expect(r.error).toMatch(/close boom/);
  });
  it("closes nothing and succeeds when nothing is running", async () => {
    const s = makeSteps();
    expect((await closeAndVerify(s, [], { settleMs: 0 })).ok).toBe(true);
    expect(s.closeArgs).toEqual([]);
  });
  it("refuses when a running pid is not in the launched set", async () => {
    const s = makeSteps({ startRunning: ["5", "6"] });
    const r = await closeAndVerify(s, ["5"], { settleMs: 0 });
    expect(r.ok).toBe(false);
    expect(r.error).toMatch(/6 were not launched by the harness/);
    expect(s.closeArgs).toEqual([]);
  });
});

describe("parseTasklistPids", () => {
  it("matches only the exact image name", () => {
    const out = '"Acrobat.exe","4120","Console","1","210,000 K"\r\n"AcroCEF.exe","4200","Console","1","1 K"\r\n';
    expect(parseTasklistPids(out)).toEqual(["4120"]);
  });
  it("returns none for the no-tasks banner", () => {
    expect(parseTasklistPids("INFO: No tasks are running which match the specified criteria.")).toEqual([]);
  });
});
