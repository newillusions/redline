// Two-pass Acrobat orchestration (pure logic, no I/O of its own).
//
// WHY TWO PASSES: on mr-desktop, Acrobat IAC (AcroExch.App, annotation counts) and a
// command-line launch (the only route that produces a real document window to photograph)
// are mutually exclusive - once Acrobat is started normally, AcroExch.App fails 0x80080005,
// most likely because of Protected Mode. Protected Mode stays ON (owner decision
// 2026-11-11); instead the harness runs the two halves as separate Acrobat lifetimes:
//
//   phase 1  IAC pass        counts only (AcrobatLeg.ps1 -CountsOnly)
//   close    Acrobat fully closed and VERIFIED gone (by exact image name)
//   phase 2  command-line    one render run per file (AcrobatLeg.ps1 -LaunchViaCommandLine),
//                            Acrobat closed and verified gone after each file
//   merge    one acrobat-results.json in the shape the report already consumes
//
// Every I/O action is injected via `steps`, so the ordering, the close-and-verify gates and
// the partial-failure reporting can be unit-tested with Acrobat mocked.
//
// SAFETY (owner workstation): the harness never closes an Acrobat it did not launch.
//   - Before phase 1 it only LISTS Acrobat.exe. If any is running (the owner's session), the
//     whole Acrobat leg is skipped and reported; closeAcrobat() is never called.
//   - Every close is scoped to the pids the legs themselves reported launching
//     (`launched_pids` in their result JSON). A running Acrobat.exe outside that set is
//     never touched: the close is refused and the run stops and reports it.
//
// steps = {
//   listAcrobatProcesses():   Promise<Proc[]>     Acrobat.exe running (exact image name)
//   closeAcrobat(procs):      Promise<void>       gracefully close EXACTLY these processes (never a kill)
//   countsPass(files):        Promise<{results: Entry[], launched_pids?: Proc[]}>   phase 1; may throw
//   renderPass(file):         Promise<{results: Entry[], launched_pids?: Proc[]}>   phase 2, one file; may throw
// Proc = { pid: string, start: string }  start = StartTime in UTC ticks, as a STRING (ticks
// exceed 2^53). A process is identified by BOTH: a reused pid with a different start is a
// different process and is never ours.
// }

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Identity of a process: pid AND start time. An unknown start never matches anything. */
export const procKey = (p) => `${String(p.pid)}@${p.start == null ? "?" : String(p.start)}`;
const fmt = (ps) => ps.map((p) => `${p.pid}`).join(", ");

/**
 * Close the Acrobat the harness launched and prove it is gone. `launched` is the set of pids
 * the legs reported launching (pid + start time); nothing outside it is ever passed to closeAcrobat(). The
 * verification is an independent process listing, retried because Acrobat takes a moment to
 * exit. A running Acrobat.exe that is not in `launched` is someone else's: refuse, do not
 * touch it. Returns { ok, remaining } and never throws.
 */
export async function closeAndVerify(steps, launched = [], { attempts = 3, settleMs = 2000 } = {}) {
  const mine = new Set(launched.filter((p) => p.start != null).map(procKey));
  let remaining = [];
  let closeError = null;
  for (let i = 0; i < attempts; i++) {
    try {
      remaining = await steps.listAcrobatProcesses();
    } catch (e) {
      // An unverifiable close is a failed close.
      return { ok: false, remaining: [], error: `cannot verify Acrobat is closed: ${e.message}` };
    }
    if (remaining.length === 0) return { ok: true, remaining: [] };
    const foreign = remaining.filter((p) => !mine.has(procKey(p)));
    if (foreign.length > 0) {
      return {
        ok: false,
        remaining,
        error: `Acrobat.exe pid(s) ${fmt(foreign)} were not launched by the harness (owner session?, or a reused pid) - not touching them`,
      };
    }
    try {
      await steps.closeAcrobat(remaining);
      closeError = null;
    } catch (e) {
      closeError = e.message;
    }
    if (settleMs) await sleep(settleMs);
  }
  try {
    remaining = await steps.listAcrobatProcesses();
  } catch (e) {
    return { ok: false, remaining: [], error: `cannot verify Acrobat is closed: ${e.message}` };
  }
  if (remaining.length === 0) return { ok: true, remaining: [] };
  return {
    ok: false,
    remaining,
    error: `Acrobat.exe still running after close (pid(s): ${fmt(remaining)})${closeError ? `; close step said: ${closeError}` : ""}`,
  };
}

function blankEntry(file) {
  const stem = file.replace(/\.pdf$/i, "");
  return {
    file,
    stem,
    opened: false,
    open_error: null,
    pages: null,
    annots_total: null,
    annots_by_page: [],
    renders: [],
    render_detail: [],
    render_error: null,
    phases: { counts: { ok: false, error: null }, render: { ok: false, error: null } },
  };
}

const errText = (e) => (e && e.message) || String(e);

/**
 * Run both phases and return { payload, ok, errors }.
 * payload.results is one merged entry per file; a failed phase is recorded on the entry
 * (`open_error` for counts, `render_error` for render, plus phases.*) and in `errors` -
 * never swallowed, never allowed to hide the other phase's data.
 */
export async function runTwoPass(files, steps, log = () => {}, { settleMs = 2000 } = {}) {
  const errors = [];
  const entries = new Map(files.map((f) => [f, blankEntry(f)]));
  const fail = (file, phase, msg) => {
    const e = entries.get(file);
    e.phases[phase] = { ok: false, error: msg };
    if (phase === "counts") e.open_error = e.open_error || msg;
    else e.render_error = e.render_error || msg;
    errors.push(`${file} [${phase}]: ${msg}`);
  };
  const failAll = (phase, msg) => {
    for (const f of files) fail(f, phase, msg);
  };

  // Preflight: LIST only. A running Acrobat.exe is the owner's session (the harness has not
  // launched anything yet): skip the whole Acrobat leg and never call closeAcrobat().
  let existing;
  try {
    existing = await steps.listAcrobatProcesses();
  } catch (e) {
    const msg = `skipped: cannot check for a running Acrobat (${errText(e)})`;
    failAll("counts", msg);
    failAll("render", msg);
    return finish(entries, errors, files, true);
  }
  if (existing.length > 0) {
    const msg = "skipped: Acrobat already running (owner session)";
    failAll("counts", msg);
    failAll("render", msg);
    return finish(entries, errors, files, true);
  }
  const launched = new Map();
  const note = (r) => { for (const p of (r && r.launched_pids) || []) if (p && p.start != null) launched.set(procKey(p), p); };

  // ---- phase 1: IAC counts -------------------------------------------------------------
  log("phase 1: IAC counts");
  try {
    const c = await steps.countsPass(files);
    note(c);
    const byFile = new Map((c.results || []).map((r) => [r.file, r]));
    for (const f of files) {
      const r = byFile.get(f);
      const e = entries.get(f);
      if (!r) {
        fail(f, "counts", "no result for this file from the counts pass");
        continue;
      }
      e.opened = !!r.opened;
      e.pages = r.pages ?? null;
      e.annots_total = r.annots_total ?? null;
      e.annots_by_page = r.annots_by_page || [];
      if (r.open_error) fail(f, "counts", r.open_error);
      else e.phases.counts = { ok: true, error: null };
    }
  } catch (e) {
    failAll("counts", `counts pass failed: ${errText(e)}`);
  }

  // ---- close fully and verify, whether or not phase 1 succeeded -------------------------
  const mid = await closeAndVerify(steps, [...launched.values()], { settleMs });
  if (mid.ok) launched.clear(); // verified gone: those processes are not ours to close again
  if (!mid.ok) {
    // Phase 2 launching into a live IAC Acrobat (or alongside someone else's) is exactly the
    // failure this design avoids.
    failAll("render", `render pass skipped: Acrobat not closed after phase 1: ${mid.error}`);
    return finish(entries, errors, files);
  }

  // ---- phase 2: command-line render, one file at a time ---------------------------------
  log("phase 2: command-line render");
  for (let i = 0; i < files.length; i++) {
    const f = files[i];
    const e = entries.get(f);
    try {
      const r = await steps.renderPass(f);
      note(r);
      const row = (r.results || []).find((x) => x.file === f) || (r.results || [])[0];
      if (!row) {
        fail(f, "render", "no result row from the render pass");
      } else {
        e.renders = row.renders || [];
        e.render_detail = row.render_detail || [];
        e.display = row.display ?? null;
        if (row.render_error || row.open_error) {
          fail(f, "render", row.render_error || row.open_error);
        } else if (e.renders.length === 0) {
          fail(f, "render", "render pass produced no render");
        } else if (e.pages != null && e.renders.length < e.pages) {
          // Command-line mode has no COM, so only the open page can be photographed.
          // Say so rather than present 1 render as a full-document pass.
          fail(f, "render", `captured ${e.renders.length} of ${e.pages} page(s) (no COM page navigation in command-line mode)`);
        } else {
          e.phases.render = { ok: true, error: null };
        }
      }
    } catch (err) {
      fail(f, "render", `render pass failed: ${errText(err)}`);
    }
    // Close and verify after EVERY file, including a failed one: the next launch must not
    // attach to this one's process.
    const v = await closeAndVerify(steps, [...launched.values()], { settleMs });
    if (v.ok) launched.clear();
    if (!v.ok) {
      for (const g of files.slice(i + 1)) fail(g, "render", `skipped: Acrobat not closed after ${f}: ${v.error}`);
      errors.push(`${f} [close]: ${v.error}`);
      break;
    }
  }

  return finish(entries, errors, files);
}

function finish(entries, errors, files, skipped = false) {
  const results = files.map((f) => entries.get(f));
  const ok = errors.length === 0;
  return {
    ok,
    errors,
    payload: {
      engine: "acrobat",
      pass: "two-pass",
      render_via: "two-pass: IAC counts, Acrobat closed, then command-line launch + window capture per file",
      protected_mode: "unchanged",
      partial: !ok,
      skipped,
      errors,
      results,
    },
  };
}

/**
 * Parse the output of
 *   Get-Process -Name Acrobat | % { "$($_.Id)|$($_.StartTime.ToUniversalTime().Ticks)" }
 * into Procs. A line whose start time could not be read ("pid|") yields start: null, which
 * matches nothing (treated as not ours).
 */
export function parseProcessList(out) {
  const procs = [];
  for (const line of String(out).split(/\r?\n/)) {
    const m = line.trim().match(/^(\d+)\|(\d*)$/);
    if (m) procs.push({ pid: m[1], start: m[2] === "" ? null : m[2] });
  }
  return procs;
}
