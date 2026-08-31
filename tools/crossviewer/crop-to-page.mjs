#!/usr/bin/env node
// Crops a viewer-window capture down to just the PDF page rectangle.
//
// WHY THIS EXISTS: both win/AcrobatLeg.ps1 and win/BluebeamGuiLeg.ps1 photograph the whole
// application window (CopyFromScreen against the window's screen rectangle), not just the
// page - Acrobat's dark chrome, its Comments panel, Revu's toolbars and thumbnail rail all
// ride along in every capture. A vision-model comparison or a pixel diff between the two
// legs is only meaningful once both images show the SAME thing: the page and nothing else.
// Cropping to the page rectangle is what makes the comparison symmetric.
//
// METHOD: cast rays outward from the image's own centre (already known to land on the page
// - both legs verify a page is on screen before ever writing a file, see Test-PageVisible /
// the render-settle loops) toward each of the four edges, along several parallel scan lines,
// and stop each ray once a run of consistently non-bright pixels confirms the page has ended
// - background/chrome in both apps is a dark theme, so "the ray left the page" reads as "the
// ray left a large bright region". Several rays and a per-edge median (not min/max) means one
// ray crossing a coloured markup near the edge does not skew the whole edge, and a debounce
// run-length means a single anti-aliased dark pixel inside the page does not either.
//
// This deliberately does NOT try to find "the largest white rectangle" via a global
// threshold scan: Acrobat's own Comments panel is ALSO a bright, near-white region (see
// tools/crossviewer/README.md's captured evidence) and sits to the right of the page
// separated only by a dark gap - a global brightness-majority scan over full rows/columns
// would happily include it. Ray-casting from a point already proven to be ON the page
// naturally stops at that gap and never reaches the panel.
//
// Usage (library):
//   import { findPageBBox, cropToPageBBox } from "./crop-to-page.mjs";
//   const bbox = findPageBBox(png);              // { left, top, width, height, ok, reason }
//   await cropToPageBBox(srcPath, destPath);      // writes the cropped PNG, returns the bbox
//
// Usage (CLI): crop every .png under a directory tree into a mirrored output tree.
//   node tools/crossviewer/crop-to-page.mjs <in-dir> <out-dir>

import { PNG } from "pngjs";
import { readFile, writeFile, mkdir, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

// A pixel counts as "page background" at this brightness - near-white, matching the
// threshold win/AcrobatLeg.ps1's Test-PageVisible already uses (200) so a ray direct from
// this script and a debug capture opened by a human read as consistent.
const BRIGHT_MIN = 200;
const SCAN_STEP_PX = 2;
// Consecutive non-bright samples along a ray before the edge is declared - absorbs a solid
// block of markup (a highlight bar, a filled rectangle) crossing the ray without mistaking
// it for the page boundary.
//
// MEASURED, NOT GUESSED (2026-08-30/31, real captures from this harness's own prior run,
// redline-crossviewer-2026-08-30/revu-AllTypes.png): an earlier 20-sample (40px) debounce
// stopped a Revu up-ray dead at the SECOND of two Highlight-annotation bars crossing dead
// centre of the image - measured directly by sampling the PNG, that bar is 60px/30 samples
// thick (y=624-684 at the image's own centre column). A 20-sample debounce is barely half
// that, so it read "left the page" 426px before the real chrome boundary at y~198 and
// cropped away everything above the bar, INCLUDING part of the page. This is exactly the
// failure this whole tool exists to avoid producing silently - it emitted a plausible,
// well-formed, wrong crop, no different in shape from the 2026-08-29 Teams-photograph
// near-miss in Capture.ps1's own header.
//
// 60 samples (120px) clears that measured 30-sample bar with 2x margin while staying far
// below the measured true chrome runs in the same image (Revu's own top toolbar: ~425
// samples of continuous dark from centre; Acrobat's side chrome: over 1000px). Scales with
// image size (0.02 of the ray's own axis) as a second line of defence for captures taken
// at higher resolution (this harness's displays span 2560x1440 to 5120x1440), with 60 as
// the floor since that is what a REAL measured case needed, not a guess this would.
function debounceSamplesFor(axisLengthPx) {
  return Math.max(60, Math.round((axisLengthPx * 0.02) / SCAN_STEP_PX));
}
// Rays spread across the middle band of the orthogonal dimension, avoiding the extreme
// corners where a rounded window edge or drop shadow can read as non-bright even over the
// page (mirrors Capture.ps1's Test-WindowUnobstructed corner-inset reasoning).
const RAY_FRACTIONS = [0.3, 0.4, 0.5, 0.6, 0.7];
// A detected page smaller than this fraction of the source image's area is treated as a
// detection failure (almost certainly the rays never left the app's own chrome pattern)
// rather than trusted and cropped to.
const MIN_AREA_FRACTION = 0.05;

function isBright(png, x, y) {
  if (x < 0 || y < 0 || x >= png.width || y >= png.height) return false;
  const idx = (png.width * y + x) << 2;
  const r = png.data[idx], g = png.data[idx + 1], b = png.data[idx + 2];
  return r >= BRIGHT_MIN && g >= BRIGHT_MIN && b >= BRIGHT_MIN;
}

// A ray that hits the image boundary mid-dark-run is ambiguous: is the page genuinely
// full-bleed to that edge (a stray shadow/AA pixel or two), or did the window simply not
// leave much chrome margin before its own edge (MEASURED, 2026-08-30/31: a real Acrobat
// capture has only 85px/42 samples of status-bar chrome between the page's true bottom and
// the window's bottom edge - well short of the 60-sample mid-image debounce, so treating
// "hit the edge" as always meaning "page goes all the way there" cropped in 93px of pure
// chrome). This threshold is deliberately far BELOW the mid-image debounce and only needs
// to clear a couple of anti-aliased edge pixels, not a whole markup - the real disambiguator
// is which side of "edge" happens: a full-bleed page has ~0 dark samples right at the
// boundary, a chrome margin (however short) reliably has many more than a handful.
const EDGE_CONFIRM_SAMPLES = 10;

// Walks one ray from (x0,y0) stepping (dx,dy) per sample. Returns the last coordinate that
// was still confidently inside the page. Three ways a ray ends: (1) a full debounce run of
// non-bright samples mid-image - definitely left the page; (2) the image boundary reached
// with a non-trivial dark run already under way - also left the page, just ran out of room
// to prove it with the full debounce (see EDGE_CONFIRM_SAMPLES above); (3) the image
// boundary reached with ~0 dark run - the page is genuinely full-bleed to that edge.
function castRay(png, x0, y0, dx, dy, debounceSamples) {
  let x = x0, y = y0;
  let lastBright = { x: x0, y: y0 };
  let darkRun = 0;
  for (;;) {
    x += dx; y += dy;
    if (x < 0 || y < 0 || x >= png.width || y >= png.height) {
      if (darkRun >= EDGE_CONFIRM_SAMPLES) return { ...lastBright, hitEdge: false };
      return { x: Math.max(0, Math.min(png.width - 1, x)), y: Math.max(0, Math.min(png.height - 1, y)), hitEdge: true };
    }
    if (isBright(png, x, y)) {
      lastBright = { x, y };
      darkRun = 0;
    } else {
      darkRun++;
      if (darkRun >= debounceSamples) return { ...lastBright, hitEdge: false };
    }
  }
}

function median(nums) {
  const s = [...nums].sort((a, b) => a - b);
  const mid = Math.floor(s.length / 2);
  return s.length % 2 ? s[mid] : (s[mid - 1] + s[mid]) / 2;
}

// Finds the page rectangle in a decoded PNG. Centre-out, per the module header. Returns
// { ok, left, top, width, height, reason, rays } - rays is the raw per-direction samples,
// kept so a caller can log exactly what was measured rather than trusting a bbox blind.
export function findPageBBox(png) {
  const cx = Math.floor(png.width / 2);
  const cy = Math.floor(png.height / 2);
  if (!isBright(png, cx, cy)) {
    return { ok: false, reason: `image centre (${cx},${cy}) is not bright - not starting from a known page pixel`, rays: null };
  }

  const vDebounce = debounceSamplesFor(png.height);
  const hDebounce = debounceSamplesFor(png.width);
  const up = [], down = [], left = [], right = [];
  for (const f of RAY_FRACTIONS) {
    const rx = Math.floor(png.width * f);
    const ry = Math.floor(png.height * f);
    up.push(castRay(png, rx, cy, 0, -SCAN_STEP_PX, vDebounce).y);
    down.push(castRay(png, rx, cy, 0, SCAN_STEP_PX, vDebounce).y);
    left.push(castRay(png, cx, ry, -SCAN_STEP_PX, 0, hDebounce).x);
    right.push(castRay(png, cx, ry, SCAN_STEP_PX, 0, hDebounce).x);
  }

  const top = Math.round(median(up));
  const bottom = Math.round(median(down));
  const l = Math.round(median(left));
  const r = Math.round(median(right));
  const width = r - l;
  const height = bottom - top;
  const area = width * height;
  const sourceArea = png.width * png.height;

  const rays = { up, down, left, right };
  if (width <= 0 || height <= 0) {
    return { ok: false, reason: `degenerate bbox ${width}x${height}`, rays };
  }
  if (area / sourceArea < MIN_AREA_FRACTION) {
    return { ok: false, reason: `bbox area ${(100 * area / sourceArea).toFixed(1)}% of source, below ${100 * MIN_AREA_FRACTION}% floor - detection likely failed`, rays };
  }
  return { ok: true, left: l, top, width, height, reason: null, rays };
}

export async function cropToPageBBox(srcPath, destPath) {
  const png = PNG.sync.read(await readFile(srcPath));
  const bbox = findPageBBox(png);
  if (!bbox.ok) {
    return { cropped: false, bbox, source: { width: png.width, height: png.height } };
  }
  const out = new PNG({ width: bbox.width, height: bbox.height });
  PNG.bitblt(png, out, bbox.left, bbox.top, bbox.width, bbox.height, 0, 0);
  await mkdir(path.dirname(destPath), { recursive: true });
  await writeFile(destPath, PNG.sync.write(out));
  return { cropped: true, bbox, source: { width: png.width, height: png.height } };
}

// Fell back to the ORIGINAL image (copied verbatim) when detection fails, rather than
// dropping the file - a caller downstream (vision-review) should still see something, and
// the returned result makes the fallback visible in the log/report rather than silent.
export async function cropOrCopy(srcPath, destPath) {
  const result = await cropToPageBBox(srcPath, destPath);
  if (!result.cropped) {
    await mkdir(path.dirname(destPath), { recursive: true });
    await writeFile(destPath, await readFile(srcPath));
  }
  return result;
}

async function findPngs(dir) {
  const out = [];
  async function walk(d) {
    let entries;
    try { entries = await readdir(d, { withFileTypes: true }); } catch { return; }
    for (const e of entries) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) await walk(p);
      else if (e.name.toLowerCase().endsWith(".png")) out.push(p);
    }
  }
  await walk(dir);
  return out.sort();
}

async function main() {
  const [inDir, outDir] = process.argv.slice(2);
  if (!inDir || !outDir) {
    console.error("usage: crop-to-page.mjs <in-dir> <out-dir>");
    process.exit(2);
  }
  const pngs = await findPngs(inDir);
  console.log(`crop-to-page: ${pngs.length} PNG(s) under ${inDir}`);
  let cropped = 0, fellBack = 0;
  for (const src of pngs) {
    const rel = path.relative(inDir, src);
    const dest = path.join(outDir, rel);
    const r = await cropOrCopy(src, dest);
    if (r.cropped) {
      cropped++;
      const pctW = (100 * r.bbox.width / r.source.width).toFixed(0);
      const pctH = (100 * r.bbox.height / r.source.height).toFixed(0);
      console.log(`  ${rel}: ${r.source.width}x${r.source.height} -> ${r.bbox.width}x${r.bbox.height} at (${r.bbox.left},${r.bbox.top}) [${pctW}%x${pctH}% of source]`);
    } else {
      fellBack++;
      console.log(`  ${rel}: NOT CROPPED (${r.bbox.reason}) - copied verbatim`);
    }
  }
  console.log(`cropped ${cropped}/${pngs.length}, fell back to original for ${fellBack}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((e) => { console.error(e); process.exit(1); });
}
