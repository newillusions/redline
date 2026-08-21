#!/usr/bin/env node
// Batch four-engine render matrix for the cross-viewer markup verification pass
// (2026-08-21, owed since obs:nx5nqon8k8xrty2vljsz). Renders every PDF in a corpus
// directory (page 0) through FOUR independent engines:
//   - pdfium   - redline's OWN render path (crates/pdf-diff render_page example;
//                the same PdfDiffEngine::render_page_full the app's M6 Compare uses).
//   - mupdf    - `mutool draw` (Artifex MuPDF), fully independent codebase.
//   - poppler  - `pdftoppm` (Homebrew poppler), fully independent codebase.
//   - chromium - real installed Chrome (channel:"chrome") via Playwright; Chrome's
//                bundled PDF viewer is ALSO PDFium-based internally, so this is a
//                different PDFium build/config rather than a from-first-principles
//                independent renderer - kept as a real-world-consumption check (see
//                tools/render-matrix.mjs's header comment for the full reasoning).
//
// pdfium is the BASELINE for pixel diffs (redline's own render path - the one thing
// every markup MUST look right in, since it's what the app itself shows the user).
// mupdf/poppler/chromium are each diffed against it. This is the SECONDARY check in
// the BB-interop harness family - see src-tauri/tests/bb_interop_conformance.rs for
// the PRIMARY (annotation-dictionary-level) check this complements. A generic viewer
// mostly blits the stored /AP appearance stream, so a data-model bug can look fine
// here while still failing in Bluebeam (which regenerates appearances on edit).
//
// Usage:
//   node tools/crossviewer-render-matrix.mjs <corpus-dir> <out-dir> [dpi=150]
//
// Output per PDF: <out-dir>/<pdf-stem>/{pdfium,mupdf,poppler,chromium}.png plus a
// diff-vs-pdfium.json. A single contact-sheet.html at <out-dir> root links every PDF's
// four renders in a grid for human/visual review (no ImageMagick montage needed - the
// four PNGs are laid out via plain <img> tags).

import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdir, readdir, readFile, writeFile, stat } from "node:fs/promises";
import path from "node:path";
import { chromium as pw } from "playwright";
import { PNG } from "pngjs";
import pixelmatch from "pixelmatch";

const execFileP = promisify(execFile);
const REPO_ROOT = path.resolve(new URL(".", import.meta.url).pathname, "..");
const PDFIUM_LIB = path.join(REPO_ROOT, "src-tauri/resources/libpdfium.dylib");
const RENDER_PAGE_BIN = path.join(REPO_ROOT, "target/debug/examples/render_page");

async function which(bin) {
  try {
    const { stdout } = await execFileP("which", [bin]);
    return stdout.trim() || null;
  } catch {
    return null;
  }
}

async function renderPdfium(pdfPath, outPng, dpi) {
  try {
    await stat(RENDER_PAGE_BIN);
  } catch {
    return { engine: "pdfium", available: false, reason: `render_page example not built at ${RENDER_PAGE_BIN} - run: cargo build -p pdf-diff --example render_page` };
  }
  try {
    await execFileP(RENDER_PAGE_BIN, [pdfPath, outPng, "0", String(dpi)], {
      env: { ...process.env, PDFIUM_DYNAMIC_LIB_PATH: PDFIUM_LIB },
      timeout: 60_000,
    });
    return { engine: "pdfium", available: true, file: outPng };
  } catch (e) {
    return { engine: "pdfium", available: false, reason: `render_page failed: ${e.message}` };
  }
}

async function renderMuPDF(pdfPath, outPng, dpi) {
  const bin = await which("mutool");
  if (!bin) return { engine: "mupdf", available: false, reason: "mutool not on PATH" };
  try {
    await execFileP(bin, ["draw", "-o", outPng, "-r", String(dpi), pdfPath, "1"], { timeout: 60_000 });
    return { engine: "mupdf", available: true, file: outPng };
  } catch (e) {
    return { engine: "mupdf", available: false, reason: `mutool draw failed: ${e.message}` };
  }
}

async function renderPoppler(pdfPath, outPng, dpi) {
  const bin = await which("pdftoppm");
  if (!bin) return { engine: "poppler", available: false, reason: "pdftoppm not on PATH" };
  const outPrefix = outPng.replace(/\.png$/, "");
  try {
    await execFileP(bin, ["-png", "-r", String(dpi), "-f", "1", "-l", "1", pdfPath, outPrefix], { timeout: 60_000 });
  } catch (e) {
    return { engine: "poppler", available: false, reason: `pdftoppm failed: ${e.message}` };
  }
  const candidates = [`${outPrefix}-1.png`, `${outPrefix}.png`, `${outPrefix}-01.png`];
  for (const c of candidates) {
    try {
      await stat(c);
      if (c !== outPng) await execFileP("mv", [c, outPng]);
      return { engine: "poppler", available: true, file: outPng };
    } catch {
      /* try next */
    }
  }
  return { engine: "poppler", available: false, reason: "rendered but output not found under expected naming" };
}

let sharedBrowser = null;
async function getChromeBrowser() {
  if (sharedBrowser) return sharedBrowser;
  try {
    sharedBrowser = await pw.launch({ headless: true, channel: "chrome" });
  } catch (e) {
    throw new Error(`no system Chrome available: ${e.message}`);
  }
  return sharedBrowser;
}

async function renderChromium(pdfPath, outPng) {
  let browser;
  try {
    browser = await getChromeBrowser();
  } catch (e) {
    return { engine: "chromium", available: false, reason: e.message };
  }
  try {
    const page = await browser.newPage({ viewport: { width: 1400, height: 1800 } });
    const abs = path.resolve(pdfPath);
    await page.goto(`file://${abs}`, { waitUntil: "load", timeout: 20000 });
    await page.waitForTimeout(1200);
    await page.screenshot({ path: outPng });
    await page.close();
    return { engine: "chromium", available: true, file: outPng, note: "channel: chrome (bundled PDFium-based viewer)" };
  } catch (e) {
    return { engine: "chromium", available: false, reason: `render failed: ${e.message}` };
  }
}

async function pngDims(file) {
  const buf = await readFile(file);
  const png = PNG.sync.read(buf);
  return { width: png.width, height: png.height, png };
}

async function diffAgainstBaseline(baseline, candidate) {
  if (!baseline.available || !candidate.available) return null;
  const pa = await pngDims(baseline.file);
  const pb = await pngDims(candidate.file);
  if (pa.width !== pb.width || pa.height !== pb.height) {
    return {
      engine: candidate.engine,
      comparable: false,
      reason: `dimension mismatch ${pa.width}x${pa.height} (pdfium) vs ${pb.width}x${pb.height} (${candidate.engine}) - different DPI rounding/viewport, not a content diff`,
    };
  }
  const diffPng = new PNG({ width: pa.width, height: pa.height });
  const changed = pixelmatch(pa.png.data, pb.png.data, diffPng.data, pa.width, pa.height, { threshold: 0.15 });
  const totalPx = pa.width * pa.height;
  const changedPct = Number(((100 * changed) / totalPx).toFixed(3));
  const diffFile = candidate.file.replace(/\.png$/, "-diff-vs-pdfium.png");
  await writeFile(diffFile, PNG.sync.write(diffPng));
  return { engine: candidate.engine, comparable: true, changedPct, changedPx: changed, totalPx, diffFile };
}

async function processPdf(pdfPath, outDir, dpi) {
  const stem = path.basename(pdfPath, ".pdf");
  const pdfOutDir = path.join(outDir, stem);
  await mkdir(pdfOutDir, { recursive: true });

  const results = {};
  results.pdfium = await renderPdfium(pdfPath, path.join(pdfOutDir, "pdfium.png"), dpi);
  results.mupdf = await renderMuPDF(pdfPath, path.join(pdfOutDir, "mupdf.png"), dpi);
  results.poppler = await renderPoppler(pdfPath, path.join(pdfOutDir, "poppler.png"), dpi);
  results.chromium = await renderChromium(pdfPath, path.join(pdfOutDir, "chromium.png"));

  const diffs = {};
  for (const eng of ["mupdf", "poppler", "chromium"]) {
    diffs[eng] = await diffAgainstBaseline(results.pdfium, results[eng]);
  }

  const summary = { pdf: stem, results, diffs };
  await writeFile(path.join(pdfOutDir, "summary.json"), JSON.stringify(summary, null, 2));
  return summary;
}

function statusLine(r) {
  if (!r) return "n/a";
  if (!r.available) return `SKIP (${r.reason})`;
  return "OK";
}

function diffLine(d) {
  if (!d) return "n/a";
  if (!d.comparable) return `NOT COMPARABLE - ${d.reason}`;
  const flag = d.changedPct > 5 ? " *** GROSS DIVERGENCE ***" : "";
  return `${d.changedPct}% px changed${flag}`;
}

function htmlEscape(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
}

async function writeContactSheet(outDir, summaries) {
  const rows = summaries
    .map((s) => {
      const cells = ["pdfium", "mupdf", "poppler", "chromium"]
        .map((eng) => {
          const r = s.results[eng];
          const d = eng === "pdfium" ? null : s.diffs[eng];
          const img = r.available
            ? `<img src="${htmlEscape(path.relative(outDir, r.file))}" loading="lazy">`
            : `<div class="skip">SKIP<br>${htmlEscape(r.reason)}</div>`;
          const diffNote = d ? `<div class="diffnote">${htmlEscape(diffLine(d))}</div>` : "";
          return `<td><div class="eng">${eng}</div>${img}${diffNote}</td>`;
        })
        .join("");
      return `<tr><th>${htmlEscape(s.pdf)}</th>${cells}</tr>`;
    })
    .join("\n");

  const html = `<!doctype html><html><head><meta charset="utf-8">
<title>Redline cross-viewer render matrix</title>
<style>
body{font-family:-apple-system,sans-serif;background:#111;color:#eee;padding:1rem}
table{border-collapse:collapse;width:100%}
th,td{border:1px solid #444;padding:6px;vertical-align:top;text-align:left}
th{width:180px;font-size:12px;word-break:break-all}
img{max-width:280px;max-height:200px;background:#fff;display:block}
.eng{font-size:11px;color:#8cf;margin-bottom:2px}
.diffnote{font-size:11px;color:#fc8}
.skip{font-size:11px;color:#f66;width:280px}
</style></head><body>
<h1>Redline cross-viewer render matrix (page 0, baseline = pdfium)</h1>
<p>Generated ${new Date().toISOString()}</p>
<table><thead><tr><th>PDF</th><th>pdfium (baseline)</th><th>mupdf</th><th>poppler</th><th>chromium</th></tr></thead>
<tbody>${rows}</tbody></table>
</body></html>`;

  await writeFile(path.join(outDir, "contact-sheet.html"), html);
}

async function main() {
  const [, , corpusDirArg, outDirArg, dpiArg] = process.argv;
  if (!corpusDirArg || !outDirArg) {
    console.error("Usage: node tools/crossviewer-render-matrix.mjs <corpus-dir> <out-dir> [dpi=150]");
    process.exit(1);
  }
  const corpusDir = path.resolve(corpusDirArg);
  const outDir = path.resolve(outDirArg);
  const dpi = dpiArg ? Number.parseInt(dpiArg, 10) : 150;
  await mkdir(outDir, { recursive: true });

  const files = (await readdir(corpusDir))
    .filter((f) => f.toLowerCase().endsWith(".pdf") && !f.startsWith("_"))
    .sort();
  console.log(`crossviewer-render-matrix: ${files.length} PDFs in ${corpusDir} -> ${outDir} @ ${dpi} DPI`);

  const summaries = [];
  for (const f of files) {
    const p = path.join(corpusDir, f);
    console.log(`\n--- ${f} ---`);
    const s = await processPdf(p, outDir, dpi);
    for (const eng of ["pdfium", "mupdf", "poppler", "chromium"]) {
      const r = s.results[eng];
      const d = eng === "pdfium" ? "" : `  diff-vs-pdfium: ${diffLine(s.diffs[eng])}`;
      console.log(`  ${eng.padEnd(10)} ${statusLine(r)}${d}`);
    }
    summaries.push(s);
  }

  if (sharedBrowser) await sharedBrowser.close();
  await writeContactSheet(outDir, summaries);
  await writeFile(path.join(outDir, "all-summaries.json"), JSON.stringify(summaries, null, 2));
  console.log(`\nContact sheet: ${path.join(outDir, "contact-sheet.html")}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
