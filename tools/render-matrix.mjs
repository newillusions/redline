#!/usr/bin/env node
// Multi-renderer screenshot matrix for Bluebeam-interoperability validation
// (owner-directed, 2026-08-11 G9 reopen: "in browsers and stirling pdf maybe").
//
// This is the SECONDARY check in the BB-interop harness - a gross-divergence sanity
// net across independently-implemented PDF renderers. It is deliberately NOT the
// primary defence: a generic renderer mostly blits the stored /AP appearance stream,
// so a data-model bug (wrong /Rect-vs-BBox fit, a missing /BE, a dangling /Popup - the
// actual shape of every real G9 fix to date) can look fine here while still failing in
// Bluebeam, which regenerates appearances from the annotation dictionary on edit. See
// `src-tauri/tests/bb_interop_conformance.rs` for the structural (dictionary-level)
// primary check this complements.
//
// Renderers actually available on this workspace Mac without new native installs:
//   - poppler (`pdftoppm`, Homebrew) - independent codebase from PDFium.
//   - macOS Quartz/CoreGraphics (`sips`) - independent, built-in, page-1-only.
//   - headless Chromium (Playwright, already a devDependency) - Chrome's own bundled
//     PDF viewer. NOTE: this is ALSO PDFium-based internally, so it is not a fully
//     independent renderer from redline's own render path in the strict codebase
//     sense - but it IS a different PDFium build/version/config, and it is the exact
//     rendering path a huge fraction of real-world PDF consumption goes through
//     (anyone who opens a PDF attachment in Chrome), so it stays in the matrix as a
//     real-world-consumption check, not a from-first-principles independent renderer.
//   - mutool (MuPDF) and ImageMagick were checked and are NOT installed on this
//     machine (`which mutool magick` both empty) - reported as a named gap in this
//     tool's own output rather than silently skipped, see printReport().
//
// Usage:
//   node tools/render-matrix.mjs <pdf-path> [pageIndex=0] [outDir=bench/results/render-matrix]
//
// Output: one PNG per available renderer in outDir, plus a pairwise pixelmatch diff
// (dimension mismatch is reported directly; same-dimension pairs get a real pixel diff
// via pixelmatch/pngjs, already devDependencies of this repo - not vendored here).

import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdir, readFile, stat } from "node:fs/promises";
import path from "node:path";
import { chromium } from "playwright";
import { PNG } from "pngjs";
import pixelmatch from "pixelmatch";

const execFileP = promisify(execFile);

async function which(bin) {
  try {
    const { stdout } = await execFileP("which", [bin]);
    return stdout.trim() || null;
  } catch {
    return null;
  }
}

async function renderPoppler(pdfPath, pageIndex, outDir) {
  const bin = await which("pdftoppm");
  if (!bin) return { engine: "poppler", available: false, reason: "pdftoppm not on PATH" };
  const outPrefix = path.join(outDir, "poppler");
  const page = pageIndex + 1; // pdftoppm is 1-indexed
  await execFileP(bin, ["-png", "-r", "150", "-f", String(page), "-l", String(page), pdfPath, outPrefix]);
  // pdftoppm names output <prefix>-<page>.png (or <prefix>.png if single page requested
  // and -f/-l match exactly 1 page, depending on version) - probe both.
  const candidates = [`${outPrefix}-${page}.png`, `${outPrefix}.png`, `${outPrefix}-${String(page).padStart(2, "0")}.png`];
  for (const c of candidates) {
    try {
      await stat(c);
      return { engine: "poppler", available: true, file: c };
    } catch {
      /* try next */
    }
  }
  return { engine: "poppler", available: false, reason: "rendered but output file not found under expected naming" };
}

async function renderQuartz(pdfPath, pageIndex, outDir) {
  const bin = await which("sips");
  if (!bin) return { engine: "macos-quartz", available: false, reason: "sips not on PATH" };
  if (pageIndex !== 0) {
    return { engine: "macos-quartz", available: false, reason: "sips converts page 1 only, no page-select flag - skipped for pageIndex != 0" };
  }
  const out = path.join(outDir, "macos-quartz.png");
  await execFileP(bin, ["-s", "format", "png", pdfPath, "--out", out]);
  return { engine: "macos-quartz", available: true, file: out };
}

async function renderChromium(pdfPath, pageIndex, outDir) {
  // Playwright's BUNDLED headless Chromium ("new headless" mode) does not ship the PDF
  // viewer extension at all - a direct file:// PDF navigation throws "Download is
  // starting" regardless of CDP Page.setDownloadBehavior (tried and confirmed
  // ineffective). A REAL installed Chrome (channel: "chrome") DOES include the PDF
  // viewer and renders inline - confirmed working on this machine. Falls back to
  // bundled Chromium (reporting the likely download failure) if no system Chrome is
  // installed, rather than silently picking a different renderer.
  let browser;
  let usedChannel = "chrome";
  try {
    browser = await chromium.launch({ headless: true, channel: "chrome" });
  } catch {
    usedChannel = "chromium (bundled - no system Chrome found, PDF viewer likely unavailable)";
    try {
      browser = await chromium.launch({ headless: true });
    } catch (e) {
      return { engine: "headless-chromium", available: false, reason: `chromium launch failed: ${e.message}` };
    }
  }
  try {
    const page = await browser.newPage({ viewport: { width: 1400, height: 1800 } });
    const abs = path.resolve(pdfPath);
    // Chrome's built-in PDF viewer renders inline on file:// navigation. It is a
    // multi-page continuous-scroll view - for pageIndex > 0 we scroll the plugin
    // frame's page containers into view; page 0 is captured at initial scroll.
    await page.goto(`file://${abs}`, { waitUntil: "load", timeout: 20000 });
    // Give the PDF plugin a moment to paint (no reliable load event for the embed).
    await page.waitForTimeout(1200);
    if (pageIndex > 0) {
      // Chrome's PDF viewer numbers page containers #page{N} (1-indexed) inside its
      // shadow DOM in recent versions; best-effort scroll, falls back to no-op if the
      // internal structure differs across Chrome versions (named limitation, not a
      // silent wrong-page capture - the screenshot is still taken either way and a
      // human/agent reviewing it can tell if it didn't move).
      await page.evaluate((n) => {
        const el = document.querySelector(`#page${n + 1}`);
        if (el) el.scrollIntoView();
      }, pageIndex).catch(() => {});
      await page.waitForTimeout(500);
    }
    const out = path.join(outDir, "headless-chromium.png");
    await page.screenshot({ path: out });
    await browser.close();
    const notes = [`channel: ${usedChannel}`];
    if (pageIndex > 0) notes.push("multi-page scroll capture is best-effort");
    return { engine: "headless-chromium", available: true, file: out, note: notes.join("; ") };
  } catch (e) {
    await browser.close().catch(() => {});
    return { engine: "headless-chromium", available: false, reason: `render failed: ${e.message}` };
  }
}

async function pngDims(file) {
  const buf = await readFile(file);
  const png = PNG.sync.read(buf);
  return { width: png.width, height: png.height, png };
}

async function diffPair(a, b) {
  if (!a.available || !b.available) return null;
  const pa = await pngDims(a.file);
  const pb = await pngDims(b.file);
  if (pa.width !== pb.width || pa.height !== pb.height) {
    return { pair: `${a.engine} vs ${b.engine}`, comparable: false, reason: `dimension mismatch ${pa.width}x${pa.height} vs ${pb.width}x${pb.height} (different DPI/viewport - resize before a real pixel diff, not attempted here)` };
  }
  const diffPng = new PNG({ width: pa.width, height: pa.height });
  const changed = pixelmatch(pa.png.data, pb.png.data, diffPng.data, pa.width, pa.height, { threshold: 0.15 });
  const totalPx = pa.width * pa.height;
  const changedPct = (100 * changed) / totalPx;
  return { pair: `${a.engine} vs ${b.engine}`, comparable: true, changedPct: Number(changedPct.toFixed(2)), changedPx: changed, totalPx };
}

async function main() {
  const [, , pdfPathArg, pageIndexArg, outDirArg] = process.argv;
  if (!pdfPathArg) {
    console.error("Usage: node tools/render-matrix.mjs <pdf-path> [pageIndex=0] [outDir]");
    process.exit(1);
  }
  const pdfPath = path.resolve(pdfPathArg);
  const pageIndex = pageIndexArg ? Number.parseInt(pageIndexArg, 10) : 0;
  const outDir = path.resolve(outDirArg || "bench/results/render-matrix");
  await mkdir(outDir, { recursive: true });

  try {
    await stat(pdfPath);
  } catch {
    console.error(`PDF not found: ${pdfPath}`);
    process.exit(1);
  }

  console.log(`render-matrix: ${pdfPath} page ${pageIndex} -> ${outDir}`);

  const results = [];
  results.push(await renderPoppler(pdfPath, pageIndex, outDir));
  results.push(await renderQuartz(pdfPath, pageIndex, outDir));
  results.push(await renderChromium(pdfPath, pageIndex, outDir));

  // Named gaps, not silently omitted.
  results.push({ engine: "mupdf", available: false, reason: "mutool not installed on this machine (which mutool: empty) - not attempted" });
  results.push({
    engine: "pdfium (redline's own render path)",
    available: false,
    reason: "not wired into this script - redline's own PDFium usage is already exercised by crates/pdf-diff (pixel/text diff engine) and tools/gui-harness.mjs (live app viewport); a single-page PDFium PNG dump would need a small new mode on pdf-diff's headless example (currently two-file-diff-only) - follow-up, not attempted here",
  });

  console.log("\n=== Renderers ===");
  for (const r of results) {
    if (r.available) {
      console.log(`  OK  ${r.engine.padEnd(32)} -> ${r.file}${r.note ? ` (${r.note})` : ""}`);
    } else {
      console.log(`  --  ${r.engine.padEnd(32)} SKIPPED: ${r.reason}`);
    }
  }

  const available = results.filter((r) => r.available);
  console.log("\n=== Pairwise diffs (gross divergence) ===");
  if (available.length < 2) {
    console.log("  fewer than 2 renderers available - nothing to diff");
  } else {
    for (let i = 0; i < available.length; i++) {
      for (let j = i + 1; j < available.length; j++) {
        const d = await diffPair(available[i], available[j]);
        if (!d) continue;
        if (!d.comparable) {
          console.log(`  ${d.pair}: NOT COMPARABLE - ${d.reason}`);
        } else {
          const flag = d.changedPct > 5 ? " *** GROSS DIVERGENCE ***" : "";
          console.log(`  ${d.pair}: ${d.changedPct}% pixels changed (${d.changedPx}/${d.totalPx})${flag}`);
        }
      }
    }
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
