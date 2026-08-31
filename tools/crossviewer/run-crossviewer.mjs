#!/usr/bin/env node
// Mac-side orchestrator for the cross-viewer harness.
//
// Replaces the manual pass in which the owner opened every staged PDF in Acrobat and
// Bluebeam on mr-desktop and judged it by eye. End to end:
//
//   1. regenerate the corpus from the repo's own emitters (cargo test, --ignored)
//   2. stage it onto mr-desktop over scp
//   3. run the viewer legs there as Session-1 scheduled tasks (GUI apps cannot run in the
//      Session 0 context an SSH command lands in - see win/Register-CrossviewerTask.ps1)
//   4. pull the per-viewer JSON, logs and renders back
//   4.5. crop every render to just its page rectangle (crop-to-page.mjs) - both legs
//        photograph the whole application window, and a fair comparison needs the same
//        content on both sides, not one viewer's chrome vs the other's
//   5. screen every CROPPED render through a local vision model against
//      tools/crossviewer/checklist.json
//   5.5. compare Acrobat's and Revu's cropped AllTypes.pdf renders directly
//        (compare-alltypes.mjs) - a mechanical, no-model-call answer to the specific
//        lower-left-cluster question this harness's first live run raised
//   6. emit a per-file PASS/FAIL report
//
// Usage:
//   node tools/crossviewer/run-crossviewer.mjs [--skip-corpus] [--skip-remote] [--skip-vision]
//                                              [--skip-crop] [--out <dir>] [--model qwen3.8:27b]

import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { mkdir, readFile, writeFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { cropOrCopy } from "./crop-to-page.mjs";
import { compareAllTypes } from "./compare-alltypes.mjs";

const execFileP = promisify(execFile);
const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, "../..");

const HOST = process.env.CROSSVIEWER_HOST || "mr-desktop";
const STAGING = process.env.CROSSVIEWER_STAGING || "H:\\redline-crossviewer";
const STAGING_POSIX = STAGING.replace(/\\/g, "/");
const CORPUS_DIR = process.env.REDLINE_CROSSVIEWER_OUT || "/tmp/redline-crossviewer";
const SSH = ["-o", "ConnectTimeout=8", "-o", "BatchMode=yes"];
const TASK_PREFIX = "redline-crossviewer";

function arg(name, fallback = null) {
  const i = process.argv.indexOf(name);
  return i === -1 ? fallback : process.argv[i + 1];
}
const has = (f) => process.argv.includes(f);

// Windows PowerShell mangles quoting through ssh -> cmd.exe; -EncodedCommand takes a
// base64 UTF-16LE payload and sidesteps the whole problem (obs:2ezmbx03b09m9kttrn81).
async function ps(script, timeoutMs = 120_000) {
  const b64 = Buffer.from(script, "utf16le").toString("base64");
  const { stdout } = await execFileP("ssh", [...SSH, HOST, `pwsh -NoProfile -EncodedCommand ${b64}`], {
    timeout: timeoutMs,
    maxBuffer: 16 * 1024 * 1024,
  });
  // PowerShell emits CLIXML progress records on stderr-as-stdout; drop them.
  return stdout.split("\n").filter((l) => !l.startsWith("#< CLIXML") && !l.startsWith("<Objs")).join("\n");
}

async function run(cmd, args, opts = {}) {
  return new Promise((resolve, reject) => {
    const p = spawn(cmd, args, { stdio: "inherit", ...opts });
    p.on("error", reject);
    p.on("close", (code) => (code === 0 ? resolve() : reject(new Error(`${cmd} exited ${code}`))));
  });
}

async function generateCorpus() {
  console.log("== 1. regenerating corpus from the repo's own emitters");
  // These two #[ignore]d tests are the single source of truth for what gets checked; the
  // harness deliberately does not keep its own copy of the corpus that could drift.
  await run("cargo", [
    "test", "--lib", "--",
    "--ignored", "--nocapture",
    "emit_crossviewer_corpus", "emit_bb_corpus_roundtrip",
  ], { cwd: REPO_ROOT, env: { ...process.env, REDLINE_CROSSVIEWER_OUT: CORPUS_DIR } });
  const pdfs = (await readdir(CORPUS_DIR)).filter((f) => f.endsWith(".pdf"));
  console.log(`   ${pdfs.length} PDF(s) in ${CORPUS_DIR}`);
  if (pdfs.length === 0) throw new Error("corpus generation produced no PDFs");
  return pdfs;
}

async function stage() {
  console.log(`== 2. staging corpus + scripts to ${HOST}:${STAGING}`);
  await ps(`
    foreach($d in @('${STAGING}','${STAGING}\\in','${STAGING}\\out','${STAGING}\\scripts','${STAGING}\\logs')){
      New-Item -ItemType Directory -Force -Path $d | Out-Null
    }
    Remove-Item '${STAGING}\\in\\*' -Force -EA SilentlyContinue
    Remove-Item '${STAGING}\\out\\*' -Force -Recurse -EA SilentlyContinue
  `);
  for (const f of ["Displays.ps1", "Capture.ps1", "ProbeDisplays.ps1", "AcrobatLeg.ps1", "BluebeamLeg.ps1", "BluebeamGuiLeg.ps1", "CloseAcrobat.ps1", "Register-CrossviewerTask.ps1"]) {
    await run("scp", [...SSH, path.join(HERE, "win", f), `${HOST}:${STAGING_POSIX}/scripts/${f}`]);
  }
  const pdfs = (await readdir(CORPUS_DIR)).filter((f) => f.endsWith(".pdf"));
  for (const f of pdfs) {
    await run("scp", [...SSH, path.join(CORPUS_DIR, f), `${HOST}:${STAGING_POSIX}/in/${f}`]);
  }
  console.log(`   staged ${pdfs.length} PDF(s)`);
}

async function runLeg(leg, timeoutMs) {
  const task = `${TASK_PREFIX}-${leg}`;
  console.log(`== 3. running ${task} in Session 1`);
  await ps(`Start-ScheduledTask -TaskName '${task}'`);
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    await new Promise((r) => setTimeout(r, 10_000));
    const out = await ps(`Write-Output "STATE=$((Get-ScheduledTask -TaskName '${task}').State)"`);
    const state = (out.match(/STATE=(\w+)/) || [])[1];
    process.stdout.write(`   ${leg}: ${state}\n`);
    if (state === "Ready") return true;
    if (Date.now() > deadline) {
      console.error(`   ${leg}: TIMED OUT after ${timeoutMs}ms - stopping task`);
      await ps(`Stop-ScheduledTask -TaskName '${task}' -EA SilentlyContinue`);
      return false;
    }
  }
}

// Register-CrossviewerTask.ps1 registers (and thereby enables) all 5 standard tasks and
// never disables them again; runLeg() leaves whichever task it drove sitting enabled too
// (its own polling loop only cares about State === "Ready", which an enabled-but-idle task
// also reports). Left uncorrected, every run-crossviewer.mjs invocation ends with every
// harness task enabled on the owner's machine - found live 2026-08-31, had to be disabled
// by hand. Call this once after the remote block, success or failure, so a task never
// outlives this script's own run.
async function disableAllTasks() {
  await ps(`Get-ScheduledTask -TaskName '${TASK_PREFIX}-*' | Disable-ScheduledTask | Out-Null`)
    .catch((e) => console.warn(`   (could not disable tasks: ${e.message})`));
}

async function collect(outDir) {
  console.log("== 4. pulling results back");
  await mkdir(outDir, { recursive: true });
  // -r so the per-leg subdirectories (renders + JSON) come across in one go.
  await run("scp", [...SSH, "-r", `${HOST}:${STAGING_POSIX}/out/.`, outDir]).catch((e) =>
    console.warn(`   (no results to pull: ${e.message})`));
  await run("scp", [...SSH, "-r", `${HOST}:${STAGING_POSIX}/logs/.`, path.join(outDir, "logs")]).catch(() => {});
}

// Crops every collected render down to just its PDF page rectangle before anything
// compares the two legs against each other. Without this, a vision-model or pixel diff is
// comparing Acrobat's chrome-heavy dark UI against Revu's own different chrome, not the
// pages - see crop-to-page.mjs's header for why a plain brightness-majority scan is not
// used (Acrobat's own Comments panel is bright too) and why the ray-cast debounce is tuned
// the way it is (measured against this harness's own 2026-08-30 captures).
async function cropRenders(outDir) {
  console.log("== 4.5 cropping renders to their page rectangle");
  const croppedDir = path.join(outDir, "cropped");
  let cropped = 0, fellBack = 0, total = 0;
  async function walk(dir) {
    let entries;
    try { entries = await readdir(dir, { withFileTypes: true }); } catch { return; }
    for (const e of entries) {
      const p = path.join(dir, e.name);
      // Never recurse into our own output - a re-run would otherwise crop the crops.
      if (e.isDirectory()) { if (p !== croppedDir) await walk(p); continue; }
      if (!e.name.toLowerCase().endsWith(".png")) continue;
      total++;
      const rel = path.relative(outDir, p);
      const dest = path.join(croppedDir, rel);
      const r = await cropOrCopy(p, dest);
      if (r.cropped) cropped++; else fellBack++;
      if (!r.cropped) console.log(`   NOT CROPPED (${r.bbox.reason}): ${rel}`);
    }
  }
  await walk(outDir);
  console.log(`   cropped ${cropped}/${total} render(s), ${fellBack} fell back to the original`);
  return croppedDir;
}

async function report(outDir, visionPath, allTypesVerdict) {
  const lines = ["# Cross-viewer harness report", "", `Generated: ${new Date().toISOString()}`, `Host: ${HOST}`, ""];

  for (const leg of ["acrobat", "bluebeam", "bluebeam-gui"]) {
    const p = path.join(outDir, leg, `${leg}-results.json`);
    let data = null;
    try { data = JSON.parse(await readFile(p, "utf8")); } catch { }
    lines.push(`## ${leg}`, "");
    if (!data) { lines.push("_no results file - leg did not run_", ""); continue; }
    if (data.blocked) { lines.push(`**BLOCKED** (exit ${data.exit_code}): ${data.reason}`, ""); continue; }
    if (data.display) {
      lines.push(`Captured on **${data.display.device}** ${data.display.width}x${data.display.height} (${data.display.orientation}) — ${data.display.reason}`, "");
      if (data.displays) {
        lines.push("<details><summary>all displays</summary>", "");
        for (const d of data.displays) lines.push(`- ${d.device} ${d.width}x${d.height} at (${d.x},${d.y}) ${d.orientation}${d.primary ? " primary" : ""}`);
        lines.push("", "</details>", "");
      }
    }
    if (data.reason) lines.push(`**Leg note:** ${data.reason}`, "");
    lines.push("| file | opened | pages | annots | renders | error |", "|---|---|---|---|---|---|");
    for (const r of data.results || []) {
      // The Acrobat leg reports `renders` (an array, one per page) and per-page annotation
      // counts; the Revu GUI leg reports a single `render` and cannot enumerate annotations
      // at all (no scripting licence). One table, both shapes.
      const nRenders = (r.renders || []).length || (r.render ? 1 : 0);
      const err = r.open_error || r.render_error || r.error || "";
      lines.push(`| ${r.file} | ${r.opened ? "yes" : "**NO**"} | ${r.pages ?? "-"} | ${r.annots_total ?? "-"} | ${nRenders} | ${err.replace(/\|/g, "\\|")} |`);
    }
    lines.push("");
  }

  try {
    const v = JSON.parse(await readFile(visionPath, "utf8"));
    lines.push("## vision review", "", `Model: ${v.model} — pass ${v.counts.pass} / fail ${v.counts.fail} / unclear ${v.counts.unclear} / error ${v.counts.error}`, "");
    lines.push("| render | verdict | summary |", "|---|---|---|");
    for (const r of v.results) {
      lines.push(`| ${r.render} | ${r.overall || "ERROR"} | ${(r.summary || r.error || "").replace(/\|/g, "\\|")} |`);
    }
    lines.push("");
  } catch {
    lines.push("## vision review", "", "_not run_", "");
  }

  lines.push("## AllTypes.pdf: Acrobat vs Revu lower-left cluster", "");
  if (!allTypesVerdict) {
    lines.push("_not run_", "");
  } else if (allTypesVerdict.error) {
    lines.push(`**could not compare:** ${allTypesVerdict.error}`, "");
  } else {
    const v = allTypesVerdict;
    lines.push(
      `**Verdict: ${v.verdict}**`, "",
      `- Acrobat lower-left region non-white fraction: ${v.acrobat.fraction.toFixed(4)} (${v.acrobat.present ? "present" : "ABSENT"})`,
      `- Revu lower-left region non-white fraction: ${v.revu.fraction.toFixed(4)} (${v.revu.present ? "present" : "ABSENT"})`,
      `- Region sampled: bottom ${(v.regionFractionH * 100).toFixed(0)}% x left ${(v.regionFractionW * 100).toFixed(0)}% of each cropped page render`,
      ""
    );
  }

  const out = path.join(outDir, "report.md");
  await writeFile(out, lines.join("\n"));
  console.log(`\nwrote ${out}`);
  return out;
}

async function main() {
  const outDir = path.resolve(arg("--out", path.join(CORPUS_DIR, "results")));
  const model = arg("--model", "qwen3.8:27b");

  if (!has("--skip-corpus")) await generateCorpus();
  if (!has("--skip-remote")) {
    try {
      await stage();
      // One-shot (idempotent) task registration, so a fresh machine needs no manual step.
      await ps(`& '${STAGING}\\scripts\\Register-CrossviewerTask.ps1' -StagingRoot '${STAGING}'`);
      // Legs run one at a time on purpose: both drive GUI windows on the same physical
      // display and screen-capture them, so overlapping them would photograph each other.
      await runLeg("bluebeam", 2 * 60_000);          // licence probe only, seconds
      await runLeg("acrobat", 45 * 60_000);
      await runLeg("bluebeam-gui", 60 * 60_000);
      await collect(outDir);
    } finally {
      // Runs on success AND on any thrown error above - a task must never outlive this
      // script's own run, matching Invoke-CrossviewerTask.ps1's own enable/disable
      // discipline (see the comment on disableAllTasks() for why this was missing).
      await disableAllTasks();
    }
  }

  const croppedDir = has("--skip-crop") ? outDir : await cropRenders(outDir);

  const visionPath = path.join(outDir, "vision-review.json");
  if (!has("--skip-vision")) {
    // Both legs' renders, not just Acrobat's - the Revu captures are the ones that prove
    // the data model, since Revu regenerates appearances rather than blitting our /AP.
    // Reviews the CROPPED renders - a vision model asked "is this markup on the page" should
    // not have to first figure out which pixels are page and which are app chrome.
    await run("node", [path.join(HERE, "vision-review.mjs"), croppedDir, visionPath, "--model", model])
      .catch((e) => console.warn(`vision review failed: ${e.message}`));
  }

  let allTypesVerdict = null;
  try {
    allTypesVerdict = await compareAllTypes(croppedDir);
  } catch (e) {
    allTypesVerdict = { error: e.message };
  }
  if (allTypesVerdict) console.log(`\nAllTypes lower-left cluster verdict: ${allTypesVerdict.verdict || allTypesVerdict.error}`);

  await report(outDir, visionPath, allTypesVerdict);
}

main().catch((e) => { console.error(e); process.exit(1); });
