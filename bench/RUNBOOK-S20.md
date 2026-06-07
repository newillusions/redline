# §20 Floor-Machine Runbook — Definitive Go/No-Go

This is the step-by-step to run the **definitive** §20 performance acceptance on the
**floor machine** (16 GB RAM, 4-core SSD laptop, integrated GPU), on **both Windows 11
and macOS (Apple Silicon)**. The indicative pass (Apple Silicon dev Mac, headless) is in
`bench/results/headless-bench-20260607.md` — this runbook produces the real verdict.

Someone with the hardware should be able to follow this top-to-bottom without reading
the source. Record results in a new `bench/results/floor-<os>-<date>.md`.

---

## 0. What the headless harness does vs. what needs the GUI

| Metric (§20) | Captured by | How |
|---|---|---|
| Cold open → first tile | headless bench | `bench` binary, `Open ms` + `1st tile ms` |
| Single tile rasterize | headless bench | `Tile p50/p95/max` |
| Page-jump → sharp | headless bench | `Jump p50/max` |
| Peak / steady RSS, leak check | headless bench | `RSS peak` / `RSS post-churn`, churn Δ |
| C2-vs-C3 sub-linear RSS | headless bench | compare the two `RSS post-churn` |
| **Pan frame-time (≤33 ms / 30 fps)** | **GUI overlay** | in-app overlay, "pan frame" / "pan worst" |
| **Zoom placeholder ≤16 ms / settle ≤250 ms** | **GUI overlay** | in-app overlay, "zoom settle" |

Run BOTH the headless bench (§3) and the GUI procedure (§4) on each OS.

---

## 1. One-time setup (per machine)

### 1.1 Toolchain
- Rust stable (`rustup`), Node LTS + npm.
- macOS: Xcode CLT. Windows: MSVC Build Tools + WebView2 (ships with Win 11).

### 1.2 Clone + frontend deps
```bash
git clone git@ssh.forge.mms.name:emittiv/redline.git
cd redline
git checkout m1-shell        # or the merged branch once §20 passes
npm install
```

### 1.3 Fetch the PDFium binary for this platform
```bash
# macOS Apple Silicon:
scripts/fetch-pdfium.sh mac-arm64
# macOS Intel:
scripts/fetch-pdfium.sh mac-x64
# Windows x64 (Git Bash / WSL shell):
scripts/fetch-pdfium.sh win-x64
```
This drops the right library into `src-tauri/resources/` (gitignored). The bundled app
resolves it automatically; the headless bench needs `PDFIUM_DYNAMIC_LIB_PATH` (see §3).

> Windows note: the script needs a bash shell (Git Bash or WSL). If unavailable, manually
> download `pdfium-win-x64.tgz` from
> <https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium/7869>, extract
> `bin/pdfium.dll`, and place it at `src-tauri\resources\pdfium.dll`.

### 1.4 Copy the corpus
Copy the five real Emittiv plan sets into `bench/corpus/` (gitignored — never commit):
```
bench/corpus/c1-typical/   <C1 pdf>   # ~80–150 MB, ~100+ sheets, vector
bench/corpus/c2-large/     <C2 pdf>   # ~300 MB, 200+ sheets, vector-heavy (headline)
bench/corpus/c3-stress/    <C3 pdf>   # 500 MB+
bench/corpus/c4-dense/     <C4 pdf>   # single dense large-format (A0) sheet
bench/corpus/c5-scanned/   <C5 pdf>   # raster-heavy scanned set, may exceed 2 GB
```
The bench picks the first `*.pdf` in each tier dir. The exact files used for the indicative
run are listed in `bench/results/headless-bench-20260607.md` ("Corpus files"); use the same
files if available so numbers compare directly.

---

## 2. Build (release — REQUIRED for real numbers)

```bash
# Headless bench binary:
cargo build --release --bin bench
# GUI app (release bundle for the most representative numbers):
npm run build && cargo tauri build      # or: cargo tauri dev (faster, debug — note it in results)
```
Use **release** for the recorded verdict. `cargo tauri dev` (debug) is acceptable for a
quick GUI check but mark it as debug in the results.

---

## 3. Headless bench (per OS)

```bash
# macOS:
export PDFIUM_DYNAMIC_LIB_PATH="$(pwd)/src-tauri/resources/libpdfium.dylib"
# Windows (PowerShell): $env:PDFIUM_DYNAMIC_LIB_PATH = "$PWD\src-tauri\resources\pdfium.dll"

./target/release/bench "$(pwd)/bench/corpus" "bench/results/floor-macos-$(date +%Y%m%d).md"
```
- Each tier runs in its own subprocess → isolated RSS (no cross-tier carryover).
- The report table maps 1:1 to the §20 thresholds. Columns: Open ms, 1st tile ms,
  Tile p50/p95/max ms, Jump p50/max ms, RSS peak MB, RSS post-churn MB, Cache MB.
- **Tile-cache / page-LRU tuning** (optional, if RSS is tight on 16 GB):
  - `REDLINE_CACHE_MB=256 ./target/release/bench ...` — tile-cache byte budget (default 512).
  - Page-LRU cap is `DEFAULT_MAX_LOADED_PAGES` (24) in `src-tauri/src/render/mod.rs`; lower
    and rebuild if needed. The bench prints `tile cache: N MB (M tiles) of K MB steady RSS`
    so you can see how much is cache vs. held page state.

### Map results to §20 (Pass column)
| §20 metric | Pass | Where in report |
|---|---|---|
| Cold open → first sheet | ≤ 3 s | `Open ms` + `1st tile ms` (sum) |
| Single tile rasterize | ≤ 60 ms | `Tile p50` (steady); note p95/max incl. first-tile parse |
| Jump to sheet → sharp | ≤ 600 ms | `Jump max` (+ one-time page parse for dense C4) |
| Peak RSS active (C2) | ≤ 2.0 GB | C2 `RSS post-churn` |
| Peak RSS (C3 500MB+) | ≤ 2.5 GB | C3 `RSS post-churn` |
| Memory after churn | +15 %, no leak | per-tier `2nd-pass RSS Δ` in Notes |
| Sub-linear w/ file size | C2≈C3, not 1.6× | compare C1 vs C3 `RSS post-churn` (same geometry) |
| Crash / OOM (C2 & C3) | none | bench completes all tiers, exit 0 |

C4 (dense A0) and C5 (scanned) are **required** tiers — see their per-tier targets in
spec §20. C4's first-tile-of-page ~1 s parse and C5's one-time normalise (~8 s, large
transient RSS) are ingest costs, not steady-state; record them but judge steady tile/RSS.

---

## 4. GUI procedure — pan-FPS + zoom-settle (per OS)

These are the §20 metrics the headless bench cannot measure. Use the in-app overlay.

1. Launch the app pointed at the C2 headline set (vector-heavy, 200+ sheets):
   ```bash
   # macOS:
   export PDFIUM_DYNAMIC_LIB_PATH="$(pwd)/src-tauri/resources/libpdfium.dylib"   # dev only; bundle resolves automatically
   export REDLINE_OPEN_PDF="$(pwd)/bench/corpus/c2-large/<C2 pdf>"
   cargo tauri dev          # or launch the built app and Open the C2 file manually
   ```
   (Windows PowerShell: `$env:REDLINE_OPEN_PDF = "$PWD\bench\corpus\c2-large\<C2 pdf>"`.)
2. Press **B** to toggle the live §20 metrics overlay (top-left). It shows, colour-coded
   against the §20 thresholds:
   - **pan frame** — last pan frame interval (ms) + smoothed FPS. Target ≤ 33 ms (30 fps).
   - **pan worst** — worst frame in the current drag gesture. Target ≤ 33 ms.
   - **zoom settle** — time from a wheel-zoom to all visible tiles sharp. Target ≤ 250 ms.
   - **last tile** — most recent single-tile rasterize. Target ≤ 60 ms.
   - **RSS** — live process resident set. Target ≤ 2048 MB.
3. **Pan test:** drag-pan continuously across the sheet for ~10 s. Read "pan worst" — it
   must stay ≤ 33 ms. Repeat at 100 %, 200 %, and a high zoom. Record worst per zoom.
4. **Zoom-settle test:** scroll-zoom in one notch, wait for sharp tiles. Read "zoom settle".
   Repeat ~5×; record the max. Must be ≤ 250 ms (placeholder should appear ≤ 16 ms — visually
   confirm the grey placeholder shows immediately on zoom).
5. **Page-jump (visual):** use the page nav ‹ › to jump across the document; confirm sharp
   render is prompt (≤ 600 ms) and RSS stays bounded as you roam (the page-LRU cap holds it).
6. **Soak (leak):** pan/zoom/page-jump across 100+ sheets for a few minutes; watch RSS in the
   overlay — it must plateau, not climb monotonically.
7. Record the overlay readings in the same `bench/results/floor-<os>-<date>.md` under a
   "GUI metrics" section, with the machine spec and whether it was a release or dev build.

---

## 5. Go / No-Go

**GO → proceed to M2** if, on BOTH Windows 11 and macOS floor machines:
- Headless: C1 + C2 (+ C3, C4, C5 per-tier targets) all Pass; no crash/OOM; no leak;
  sub-linear RSS holds.
- GUI: pan worst ≤ 33 ms at all tested zooms; zoom-settle ≤ 250 ms; RSS ≤ 2 GB during soak.

**NO-GO → stop and escalate** (do not build markup/takeoff on top) with mitigation options:
LOD / vector-tile rendering, lower the page-LRU cap / tile budget, alternate render lib,
raise the hardware floor, or scope cut. C2 is the headline gate; C4 or C5 failing their
per-tier targets is also blocking (both are required reviewer workflows).

---

## 6. Notes / gotchas carried from development
- PDFium has process-global C state: the app serialises all PDFium work onto one render
  thread (`RenderHandle`). Don't add a second `Pdfium` instance / second render thread.
- PDFium can't page-load a single PDF whose internal object offsets exceed ~2 GiB; redline
  auto-normalises such files via lopdf on open (transparent; original untouched). C5 hits this.
- The tile cache is byte-budgeted (512 MiB default); the dominant RSS lever is the page-LRU
  cap (loaded pages hold PDFium's parsed state), not the tile cache.
- Pin: PDFium release `chromium/7869` (see `scripts/fetch-pdfium.sh`). Bumping it can change
  render perf/behaviour — re-run this runbook after any bump.
