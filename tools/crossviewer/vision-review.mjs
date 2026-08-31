#!/usr/bin/env node
// Vision-review leg of the cross-viewer harness.
//
// Feeds each rendered page image to a local vision model behind llm-gate and asks it the
// checklist questions a human answered by eye during the manual cross-viewer pass. Emits
// one PASS/FAIL verdict per rendered page plus the model's own notes, so a regression that
// only shows up visually (a stamp that renders as an empty box, a callout whose text is
// clipped, markups shifted off-page) is caught without anyone opening 24 files by hand.
//
// This is a SCREENING layer, not an oracle. A vision model can miss a subtle defect and can
// occasionally invent one, so a FAIL is a prompt to look at the named page - never grounds
// on its own for asserting the app is broken. The authoritative checks remain
// src-tauri/tests/bb_interop_conformance.rs (annotation-dictionary structure) and the
// Acrobat leg's own open/page/annotation-count report.
//
// Usage:
//   node tools/crossviewer/vision-review.mjs <render-dir> <out-json> [--model qwen3.8:27b]
//
// <render-dir> is scanned recursively for .png files.

import { readdir, readFile, writeFile, stat, mkdtemp } from "node:fs/promises";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { tmpdir } from "node:os";
import path from "node:path";

const execFileP = promisify(execFile);

const GATE = process.env.LLM_GATE_URL || "http://10.0.21.19:11435";
const DEFAULT_MODEL = "qwen3.8:27b";
const REQUEST_TIMEOUT_MS = 180_000;
// llm-gate rejects oversized request bodies with HTTP 413, and a full-resolution 150dpi
// render of an A1 sheet is several megabytes before base64 inflates it by a third. 1568px
// on the long edge is the size the workspace's own vision probes settled on and is ample
// for the checklist's questions (obs:ai2wr23wu0gzka7d5qp4).
const MAX_EDGE_PX = 1568;
const DOWNSCALE_ABOVE_BYTES = 1_500_000;

function parseArgs(argv) {
  const positional = [];
  let model = DEFAULT_MODEL;
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--model") { model = argv[++i]; continue; }
    positional.push(argv[i]);
  }
  const [renderDir, outJson] = positional;
  if (!renderDir || !outJson) {
    console.error("usage: vision-review.mjs <render-dir> <out-json> [--model NAME]");
    process.exit(2);
  }
  return { renderDir, outJson, model };
}

async function findPngs(dir) {
  const out = [];
  async function walk(d) {
    for (const e of await readdir(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) await walk(p);
      else if (e.name.toLowerCase().endsWith(".png")) out.push(p);
    }
  }
  await walk(dir);
  return out.sort();
}

function buildPrompt(checklist) {
  const questions = checklist.items
    .map((it, i) => `${i + 1}. [${it.id}] ${it.question}`)
    .join("\n");
  return `You are reviewing a single rendered page of a PDF that contains engineering drawing markups (annotations) written by a PDF markup application. Answer strictly from what you can see in the image.

Answer these questions:
${questions}

Then give an overall verdict.

Reply with ONLY a JSON object, no prose outside it, in exactly this shape:
{
  "answers": [ { "id": "<the id in brackets>", "verdict": "pass" | "fail" | "unclear", "note": "<one short sentence>" } ],
  "overall": "pass" | "fail" | "unclear",
  "summary": "<one sentence describing what the page shows and anything wrong with it>"
}`;
}

// The model is asked for bare JSON, but small models habitually wrap it in prose or a code
// fence. Recover the outermost JSON object rather than failing the whole page on formatting.
function extractJson(text) {
  const fenced = text.match(/```(?:json)?\s*([\s\S]*?)```/);
  const candidate = fenced ? fenced[1] : text;
  const start = candidate.indexOf("{");
  const end = candidate.lastIndexOf("}");
  if (start === -1 || end === -1 || end <= start) return null;
  try { return JSON.parse(candidate.slice(start, end + 1)); } catch { return null; }
}

// macOS `sips` is used rather than an image library so the harness pulls in no extra
// dependency; the runner is macOS-side by design. If sips is unavailable the original is
// sent and an oversized page surfaces as a named 413 rather than a silent skip.
async function downscaleIfNeeded(pngPath, workDir) {
  const { size } = await stat(pngPath);
  if (size <= DOWNSCALE_ABOVE_BYTES) return pngPath;
  const dest = path.join(workDir, path.basename(pngPath));
  try {
    await execFileP("sips", ["-Z", String(MAX_EDGE_PX), pngPath, "--out", dest], { timeout: 60_000 });
    return dest;
  } catch (e) {
    console.warn(`    (downscale failed, sending original: ${e.message})`);
    return pngPath;
  }
}

async function reviewOne(pngPath, prompt, model, workDir) {
  const sendPath = await downscaleIfNeeded(pngPath, workDir);
  const b64 = (await readFile(sendPath)).toString("base64");
  const ac = new AbortController();
  const timer = setTimeout(() => ac.abort(), REQUEST_TIMEOUT_MS);
  try {
    const res = await fetch(`${GATE}/api/chat`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      signal: ac.signal,
      body: JSON.stringify({
        model,
        stream: false,
        think: false,           // reasoning mode exhausts the budget before emitting JSON (obs:ai2wr23wu0gzka7d5qp4)
        options: { temperature: 0 },
        messages: [{ role: "user", content: prompt, images: [b64] }],
      }),
    });
    if (!res.ok) {
      return { error: `llm-gate HTTP ${res.status}: ${(await res.text()).slice(0, 300)}` };
    }
    const body = await res.json();
    const content = body?.message?.content ?? "";
    const parsed = extractJson(content);
    if (!parsed) return { error: "model did not return parseable JSON", raw: content.slice(0, 500) };
    return { review: parsed };
  } catch (e) {
    return { error: e.name === "AbortError" ? `timed out after ${REQUEST_TIMEOUT_MS}ms` : e.message };
  } finally {
    clearTimeout(timer);
  }
}

async function main() {
  const { renderDir, outJson, model } = parseArgs(process.argv.slice(2));
  await stat(renderDir);
  const checklist = JSON.parse(await readFile(new URL("./checklist.json", import.meta.url), "utf8"));
  const prompt = buildPrompt(checklist);
  const pngs = await findPngs(renderDir);
  const workDir = await mkdtemp(path.join(tmpdir(), "crossviewer-vision-"));
  console.log(`vision-review: ${pngs.length} render(s) under ${renderDir}, model=${model}`);

  const results = [];
  for (const png of pngs) {
    const rel = path.relative(renderDir, png);
    process.stdout.write(`  ${rel} ... `);
    const t0 = Date.now();
    const r = await reviewOne(png, prompt, model, workDir);
    const ms = Date.now() - t0;
    if (r.error) {
      console.log(`ERROR (${ms}ms): ${r.error}`);
      results.push({ render: rel, ok: false, error: r.error, raw: r.raw, duration_ms: ms });
    } else {
      console.log(`${r.review.overall} (${ms}ms)`);
      results.push({ render: rel, ok: true, ...r.review, duration_ms: ms });
    }
  }

  const summary = {
    generated_at: new Date().toISOString(),
    model,
    gate: GATE,
    render_dir: renderDir,
    checklist_version: checklist.version,
    counts: {
      total: results.length,
      pass: results.filter((r) => r.overall === "pass").length,
      fail: results.filter((r) => r.overall === "fail").length,
      unclear: results.filter((r) => r.overall === "unclear").length,
      error: results.filter((r) => !r.ok).length,
    },
    results,
  };
  await writeFile(outJson, JSON.stringify(summary, null, 2));
  console.log(`\nwrote ${outJson}`);
  console.log(`pass=${summary.counts.pass} fail=${summary.counts.fail} unclear=${summary.counts.unclear} error=${summary.counts.error}`);
}

main().catch((e) => { console.error(e); process.exit(1); });
