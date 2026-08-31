#!/usr/bin/env node
// Answers, mechanically, the question this harness exists to answer for AllTypes.pdf: is
// the lower-left markup cluster (Rectangle/Arrow/Text/StampDynamic) that Acrobat renders
// also present in Revu's render, once both are cropped to just their page rectangle
// (crop-to-page.mjs)?
//
// WHY A DEDICATED CHECK AND NOT JUST THE VISION REVIEW: the vision review is a screening
// layer over a small local model - useful breadth, not a verdict on its own (see its own
// header). This is a direct pixel measurement against the two real renders this harness
// already produces, requires no model call, and is what actually resolved the 2026-08-30
// open question - see the module doc below for the finding.
//
// FINDING THIS CHECK IS BUILT FROM (2026-08-30/31, measured against this harness's own
// prior real captures, not assumed): the "Revu is missing the cluster" gap recorded in
// HANDOVER.md was a VIEWPORT ARTIFACT, not a Bluebeam interop defect. Revu's own page
// thumbnail (visible in revu-AllTypes.png's own left rail) shows the cluster; the main
// viewport did not because Revu opened at a remembered zoom where the page ran well past
// the bottom of the window (a small scrollbar thumb spanning ~30% of the track proves it),
// and BluebeamGuiLeg.ps1 never sent a Fit Page command before capturing. Measuring the same
// lower-left region on both crops with pngjs: Acrobat 25-41% non-white depending on exact
// sample box (the cluster genuinely fills a large chunk of it), Revu EXACTLY 0% across
// every box tried - not "small but present", a hard zero, consistent with the region being
// scrolled fully off screen rather than merely small or misrendered.
//
// This module's win/BluebeamGuiLeg.ps1 companion fix sends Fit Page (Ctrl+9, verified
// against Bluebeam's own Keyboard Shortcuts Guide - Ctrl+0 is Fit WIDTH, a different
// command) before every capture, so a live re-run should show the cluster in both legs.
// This comparator stays in the harness permanently as the mechanical proof of that, not a
// one-off diagnostic - a future regression (real or viewport) reproduces the same 0%.
//
// Usage (library): const v = await compareAllTypes(croppedRenderDir);
// Usage (CLI):      node tools/crossviewer/compare-alltypes.mjs <cropped-render-dir>

import { PNG } from "pngjs";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Bottom-left box as a fraction of the cropped page's own width/height. Calibrated against
// the real corpus fixture (bench/corpus's AllTypes page places the cluster in roughly the
// bottom-left third) with margin; not sensitive to the exact value - 0.35-0.45 all measured
// within a few points of each other on the real captures above, and Revu measured exactly 0
// at every size tried.
const REGION_FRACTION_W = 0.45;
const REGION_FRACTION_H = 0.45;
// A pixel counts as "ink" (not blank page) below this brightness on every channel. Slightly
// stricter than crop-to-page.mjs's page-vs-chrome threshold (200) since this is checking
// for the PRESENCE of markup content on an already-known-page pixel, not disambiguating
// page from chrome - a near-white anti-aliased edge should not by itself count as content.
const INK_MAX = 245;
// Sampling stride - every 3rd pixel in each axis. The real measurement above used the same
// stride on 1400-3300px-wide crops and gave a clean 25-41% vs 0% split; a full-resolution
// scan would not change which side of that gap either verdict falls on.
const STEP = 3;
// Non-white fraction above this in the sampled region counts as "the cluster is present".
// Set an order of magnitude below the smallest measured Acrobat value (0.25) and two orders
// above sampling noise, so it is not a close call in practice on this corpus.
const PRESENT_THRESHOLD = 0.01;

function measureRegion(png) {
  const x0 = 0;
  const x1 = Math.floor(png.width * REGION_FRACTION_W);
  const y0 = Math.floor(png.height * (1 - REGION_FRACTION_H));
  const y1 = png.height;
  let nonwhite = 0, total = 0;
  for (let y = y0; y < y1; y += STEP) {
    for (let x = x0; x < x1; x += STEP) {
      const idx = (png.width * y + x) << 2;
      const r = png.data[idx], g = png.data[idx + 1], b = png.data[idx + 2];
      total++;
      if (!(r >= INK_MAX && g >= INK_MAX && b >= INK_MAX)) nonwhite++;
    }
  }
  const fraction = total > 0 ? nonwhite / total : 0;
  return { fraction, present: fraction >= PRESENT_THRESHOLD, sampled: total, nonwhite };
}

async function findAllTypesRender(legDir) {
  let entries;
  try { entries = await readdir(legDir); } catch { return null; }
  // Prefer the single-page name; fall back to the first page of a multi-page render (some
  // corpus revisions emit AllTypes as >1 page - see AcrobatLeg.ps1's naming rule).
  const exact = entries.find((f) => /^AllTypes\.png$/i.test(f));
  if (exact) return path.join(legDir, exact);
  const paged = entries.filter((f) => /^AllTypes_Page_\d+\.png$/i.test(f)).sort();
  return paged.length ? path.join(legDir, paged[0]) : null;
}

export async function compareAllTypes(croppedRenderDir) {
  const acrobatPath = await findAllTypesRender(path.join(croppedRenderDir, "acrobat"));
  const revuPath = await findAllTypesRender(path.join(croppedRenderDir, "bluebeam-gui"));
  if (!acrobatPath || !revuPath) {
    return {
      error: `AllTypes render missing - acrobat=${acrobatPath ?? "NOT FOUND"} revu=${revuPath ?? "NOT FOUND"}`,
    };
  }
  const acrobatPng = PNG.sync.read(await readFile(acrobatPath));
  const revuPng = PNG.sync.read(await readFile(revuPath));
  const acrobat = measureRegion(acrobatPng);
  const revu = measureRegion(revuPng);

  let verdict;
  if (acrobat.present && revu.present) {
    verdict = "MATCH - lower-left cluster present in both viewers";
  } else if (acrobat.present && !revu.present) {
    verdict = "MISMATCH - Revu is missing the lower-left cluster Acrobat renders";
  } else if (!acrobat.present && revu.present) {
    verdict = "MISMATCH - Acrobat is missing the lower-left cluster Revu renders (unexpected direction - re-check the fixture)";
  } else {
    verdict = "MISMATCH - NEITHER viewer shows the lower-left cluster - check the corpus fixture and both crops before trusting either render";
  }

  return {
    verdict,
    acrobatPath,
    revuPath,
    acrobat,
    revu,
    regionFractionW: REGION_FRACTION_W,
    regionFractionH: REGION_FRACTION_H,
  };
}

async function main() {
  const dir = process.argv[2];
  if (!dir) {
    console.error("usage: compare-alltypes.mjs <cropped-render-dir>");
    process.exit(2);
  }
  const v = await compareAllTypes(dir);
  console.log(JSON.stringify(v, null, 2));
  if (v.error) process.exit(1);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((e) => { console.error(e); process.exit(1); });
}
