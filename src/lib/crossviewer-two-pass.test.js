import { describe, it, expect } from "vitest";
import { spawn, spawnSync, execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { writeFileSync } from "node:fs";
import process from "node:process";
import { runTwoPass, closeAndVerify, parseProcessList, procKey } from "../../tools/crossviewer/two-pass.mjs";

const FAST = { settleMs: 0 };
const P = (pid, start = "100") => ({ pid: String(pid), start: String(start) });

// Fake Acrobat host. `running` maps pid -> start time. A leg that launches Acrobat adds a
// process (fresh start time) and reports {pid,start} in launched_pids, as the real legs do.
// closeAcrobat(procs) removes ONLY a process whose pid AND start both match, so a test can
// prove nothing else was touched.
function makeSteps(opts = {}) {
  const calls = [];
  const closeArgs = [];
  const running = new Map((opts.startRunning || []).map((p) => [String(p.pid ?? p), String(p.start ?? "1")]));
  let nextPid = 1000;
  let clock = 5000;
  const launch = () => { const pid = String(nextPid++); const start = String(clock++); running.set(pid, start); return { pid, start }; };
  const alive = (procs) => procs.filter((q) => running.get(q.pid) === q.start);
  const steps = {
    calls,
    closeArgs,
    running,
    launch,
    async listAcrobatProcesses() {
      calls.push("list");
      if (opts.listThrows) throw new Error("tasklist boom");
      return [...running].map(([pid, start]) => ({ pid, start }));
    },
    async closeAcrobat(procs) {
      calls.push("close");
      closeArgs.push(procs);
      if (opts.closeThrows) throw new Error("close boom");
      if (!opts.neverCloses) for (const q of procs) if (running.get(q.pid) === q.start) running.delete(q.pid);
    },
    // Models the real legs: launched_pids is the launch-time snapshot INTERSECTED with what is
    // still alive (same pid AND start) at the end, never "whatever is alive".
    // opts.ownerDuringLeg(running) starts an owner Acrobat while the leg runs; opts.ownerAfterExit
    // does so in the gap after the counts leg's App.Exit(); opts.reusePidAfterExit(running, snap)
    // reuses the leg's pid number for a different process.
    async countsPass(files) {
      calls.push("counts");
      const snapshot = [launch()];
      if (opts.ownerDuringLeg) opts.ownerDuringLeg(running);
      if (opts.countsThrows) throw new Error("iac boom");
      if (!opts.appExitFails) for (const q of snapshot) running.delete(q.pid); // $app.Exit()
      if (opts.ownerAfterExit) opts.ownerAfterExit(running);
      if (opts.reusePidAfterExit) opts.reusePidAfterExit(running, snapshot);
      return {
        launched_pids: opts.reportStale ? snapshot : alive(snapshot),
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
      if (opts.renderRootReused) { // our Acrobat exits; a later-started owner Acrobat gets the same pid
        running.delete(snapshot[0].pid);
        running.set(snapshot[0].pid, "88888");
      }
      if (opts.renderThrows === file) throw new Error("capture boom");
      const pages = opts.renderPages ?? 2;
      return {
        launched_pids: alive(snapshot),
        results: [{ file, renders: Array.from({ length: pages }, (_, i) => `${file}-${i}.png`), render_detail: [], render_error: null }],
      };
    },
  };
  return steps;
}
const strip = (c) => c.filter((x) => x !== "list");

describe("owner-session safety", () => {
  it("pre-existing Acrobat: counts and render skipped AND close never called", async () => {
    const s = makeSteps({ startRunning: [P(4242)] });
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

  it("close only targets harness-launched PIDs (pid + start)", async () => {
    const s = makeSteps();
    const r = await runTwoPass(["a.pdf", "b.pdf"], s, undefined, FAST);
    expect(r.ok).toBe(true);
    // counts leg exits its own Acrobat (nothing to close); each render leg leaves one
    expect(s.closeArgs).toEqual([[P(1001, 5001)], [P(1002, 5002)]]);
  });

  it("an owner Acrobat that appears DURING the leg is not reported as launched and is never closed", async () => {
    const s = makeSteps({ appExitFails: true, ownerDuringLeg: (r) => r.set("7777", "900") });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    expect(s.closeArgs.flat().map((q) => q.pid)).not.toContain("7777");
    expect(s.closeArgs).toEqual([]); // refuse the whole close: a foreign process is present
    expect(s.running.has("7777")).toBe(true);
    expect(s.calls.some((c) => c.startsWith("render:"))).toBe(false);
    expect(r.payload.results[0].render_error).toMatch(/7777 were not launched by the harness/);
  });

  it("post-Exit gap: an owner Acrobat started after App.Exit() is never reported or closed", async () => {
    const s = makeSteps({ ownerAfterExit: (r) => r.set("8888", "901") });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    expect(s.closeArgs).toEqual([]);
    expect(s.running.has("8888")).toBe(true);
    expect(r.ok).toBe(false);
    expect(r.payload.results[0].render_error).toMatch(/8888 were not launched by the harness/);
    expect(s.calls.some((c) => c.startsWith("render:"))).toBe(false);
  });

  it("pid reuse: the same pid number with a different start time is not ours and is never closed", async () => {
    // The counts leg's own Acrobat exits; the OS reuses its pid for an owner Acrobat (new start).
    // A stale report (pid only matched) would send WM_CLOSE to the owner's window.
    const s = makeSteps({
      reportStale: true,
      reusePidAfterExit: (r, snap) => r.set(snap[0].pid, "99999"),
    });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    expect(s.closeArgs).toEqual([]);
    expect(s.running.get("1000")).toBe("99999"); // owner process still there
    expect(r.payload.results[0].render_error).toMatch(/not launched by the harness/);
    expect(s.calls.some((c) => c.startsWith("render:"))).toBe(false);
  });

  it("render root pid reused after launch by a later-started Acrobat: not reported, never closed", async () => {
    const s = makeSteps({ renderRootReused: true });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    expect(s.closeArgs).toEqual([]); // nothing was reported as ours, nothing closed
    expect(s.running.get("1001")).toBe("88888"); // the owner's process is still there
    expect(r.errors.some((x) => x.startsWith("a.pdf [close]") && /not launched by the harness/.test(x))).toBe(true);
  });

  it("a failed leg that reported no pids never causes a close of what is running", async () => {
    const s = makeSteps({ countsThrows: true });
    const r = await runTwoPass(["a.pdf"], s, undefined, FAST);
    expect(s.closeArgs).toEqual([]);
    expect(r.payload.results[0].render_error).toMatch(/not launched by the harness/);
  });

  it("clears launched after each verified close: an earlier leg's identity is no longer closable", async () => {
    const s = makeSteps();
    const base = s.renderPass;
    let first = null;
    s.renderPass = async (f) => {
      const r = await base(f);
      if (!first) first = r.launched_pids[0];
      else s.running.set(first.pid, first.start); // file b's leg: first leg's exact identity shows up again
      return r;
    };
    const r = await runTwoPass(["a.pdf", "b.pdf"], s, undefined, FAST);
    // had `launched` not been cleared after a's verified close, closing b would also have been
    // handed a's identity; instead the whole close is refused.
    expect(s.closeArgs).toHaveLength(1); // only a's own close; b's was refused
    expect(r.errors.some((x) => x.startsWith("b.pdf [close]") && /not launched by the harness/.test(x))).toBe(true);
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

  it("reports a render failure, keeps the other phase's data, and stops rather than close a process it was not told about", async () => {
    const s = makeSteps({ renderThrows: "a.pdf" });
    const r = await runTwoPass(["a.pdf", "b.pdf"], s, undefined, FAST);
    const [a, b] = r.payload.results;
    expect(a.render_error).toMatch(/capture boom/);
    expect(a.annots_total).toBe(5);
    expect(b.annots_total).toBe(5);
    expect(b.render_error).toMatch(/skipped: Acrobat not closed after a.pdf/);
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
    const r = await closeAndVerify(makeSteps({ startRunning: [P(5, 7)], closeThrows: true }), [P(5, 7)], { settleMs: 0 });
    expect(r.ok).toBe(false);
    expect(r.error).toMatch(/close boom/);
  });
  it("closes nothing and succeeds when nothing is running", async () => {
    const s = makeSteps();
    expect((await closeAndVerify(s, [], { settleMs: 0 })).ok).toBe(true);
    expect(s.closeArgs).toEqual([]);
  });
  it("refuses when a running process is not in the launched set", async () => {
    const s = makeSteps({ startRunning: [P(5, 7), P(6, 8)] });
    const r = await closeAndVerify(s, [P(5, 7)], { settleMs: 0 });
    expect(r.ok).toBe(false);
    expect(r.error).toMatch(/6 were not launched by the harness/);
    expect(s.closeArgs).toEqual([]);
  });
  it("refuses a same-pid, different-start process", async () => {
    const s = makeSteps({ startRunning: [P(5, 8)] });
    const r = await closeAndVerify(s, [P(5, 7)], { settleMs: 0 });
    expect(r.ok).toBe(false);
    expect(s.closeArgs).toEqual([]);
  });
  it("never treats an unknown start time as ours", async () => {
    const s = makeSteps({ startRunning: [P(5, 7)] });
    const r = await closeAndVerify(s, [{ pid: "5", start: null }], { settleMs: 0 });
    expect(r.ok).toBe(false);
    expect(s.closeArgs).toEqual([]);
  });
});

describe("process identity helpers", () => {
  it("procKey needs pid and start", () => {
    expect(procKey(P(1, 2))).not.toBe(procKey(P(1, 3)));
    expect(procKey({ pid: "1", start: null })).toBe("1@?");
  });
  it("parseProcessList reads pid|ticks lines and keeps an unreadable start as null", () => {
    const out = "4120|639299669307893140\r\n77|\r\njunk\r\n";
    expect(parseProcessList(out)).toEqual([
      { pid: "4120", start: "639299669307893140" },
      { pid: "77", start: null },
    ]);
  });
  it("parseProcessList returns none for empty output", () => {
    expect(parseProcessList("")).toEqual([]);
  });
});

// ---- the PowerShell matching logic, exercised for real when pwsh is available ----------------
const WIN = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../tools/crossviewer/win");
const hasPwsh = spawnSync("pwsh", ["-NoProfile", "-Command", "$PSVersionTable.PSVersion.Major"]).status === 0;
const pw = (script) => spawnSync("pwsh", ["-NoProfile", "-Command", script], { encoding: "utf8" });

describe.skipIf(!hasPwsh)("PowerShell process matching (pwsh)", () => {
  const load = `. '${path.join(WIN, "AcrobatProcs.ps1")}'; $t0 = [datetime]'2026-11-11T10:00:00Z';
    function P($id,$par,$sec,$n='Acrobat.exe'){ [pscustomobject]@{ProcessId=$id;ParentProcessId=$par;CreationDate=$t0.AddSeconds($sec);Name=$n} };`;

  it("reports descendants of the launch process and not the root", () => {
    const r = pw(`${load} (Select-OwnedDescendants -Processes @((P 10 1 0),(P 11 10 1),(P 12 11 2),(P 99 1 5)) -RootPid 10 -RootStart $t0 -LiveStart $t0) -join ','`);
    expect(r.stdout.trim()).toBe("11,12");
  });

  it("root pid reused by a later-started process: nothing is walked or reported", () => {
    const r = pw(`${load} @(Select-OwnedDescendants -Processes @((P 10 1 600),(P 11 10 601)) -RootPid 10 -RootStart $t0 -LiveStart $t0.AddSeconds(600)).Count`);
    expect(r.stdout.trim()).toBe("0");
  });

  it("a child that started before its parent (reused parent pid) is rejected, with its subtree", () => {
    const r = pw(`${load} (Select-OwnedDescendants -Processes @((P 10 5 0),(P 11 10 4),(P 12 11 6)) -RootPid 10 -RootStart $t0.AddSeconds(5) -LiveStart $t0.AddSeconds(5)) -join ','`);
    expect(r.stdout.trim()).toBe("");
  });

  it("walks through a non-Acrobat intermediate but reports only Acrobat.exe", () => {
    const r = pw(`${load} (Select-OwnedDescendants -Processes @((P 10 1 0),(P 11 10 1 'helper.exe'),(P 12 11 2)) -RootPid 10 -RootStart $t0 -LiveStart $t0) -join ','`);
    expect(r.stdout.trim()).toBe("12");
  });

  it("counts independent new Acrobat trees (one instance + helpers = 1, plus an owner's = 2)", () => {
    expect(pw(`${load} Get-NewAcrobatRootCount -Processes @((P 10 1 0),(P 11 10 1))`).stdout.trim()).toBe("1");
    expect(pw(`${load} Get-NewAcrobatRootCount -Processes @((P 10 1 0),(P 11 10 1),(P 50 2 1))`).stdout.trim()).toBe("2");
  });

  it("CloseAcrobat.ps1 matches on name + pid + start: wrong start is untouched, right start is a target", async () => {
    const child = spawn("sleep", ["30"], { stdio: "ignore" });
    try {
      const execP = promisify(execFile);
      const run = async (start) => {
        const f = path.join(process.env.TMPDIR || "/tmp", `cp-${child.pid}-${start.length}.json`);
        writeFileSync(f, `[{"pid":"${child.pid}","start":"${start}"}]`); // fs, not a pwsh cold start per case
        try {
          const r = await execP("pwsh", ["-NoProfile", "-File", path.join(WIN, "CloseAcrobat.ps1"), "-PidFile", f, "-ImageName", "sleep", "-GracefulWaitSec", "1"], { encoding: "utf8" });
          return { status: 0, stdout: r.stdout };
        } catch (e) { return { status: e.code, stdout: e.stdout }; }
      };
      // The wrong-start case needs no real start time, so it runs concurrently with the ticks lookup:
      // each pwsh cold start is seconds on a loaded runner, and they were serial (past the 5 s limit).
      const [wrong, ticksOut] = await Promise.all([
        run("1"),
        execP("pwsh", ["-NoProfile", "-Command", `(Get-Process -Id ${child.pid}).StartTime.ToUniversalTime().Ticks`], { encoding: "utf8" }),
      ]);
      const ticks = ticksOut.stdout.trim();
      expect(ticks).toMatch(/^\d+$/);
      expect(wrong.status).toBe(0); // not matched: nothing to close
      const right = await run(ticks);
      expect(right.status).toBe(3); // matched (and, being `sleep`, did not close)
      expect(right.stdout).toContain(String(child.pid));
      expect(process.kill(child.pid, 0)).toBe(true); // never killed
    } finally {
      child.kill();
    }
    // Real processes: 3 pwsh spawns plus CloseAcrobat's deliberate 1 s graceful wait. Measured 2.9-3.9 s
    // idle (vitest default is 5 s), well over it on a loaded CI runner. Timeout scoped to this test only.
  }, 30_000);
});
