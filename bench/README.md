# M1 Performance Benchmark Harness

This directory holds the headless performance harness and results for the
**redline M1 make-or-break gate** (spec §20, task:dcufqdmr446ek7u9jcnr).

**Status (2026-06-07):** harness implemented (`src-tauri/src/bin/bench.rs`), real
corpus in place (all 5 tiers, gitignored), PDFium arm64 binary installed. An
**indicative** verdict has been produced on Apple Silicon — see `results/`.
The **definitive** §20 verdict still needs the floor machine (16 GB / integrated
GPU) on both Windows 11 and macOS, plus interactive pan-FPS (GUI-only).

## Running the headless bench

```bash
export PDFIUM_DYNAMIC_LIB_PATH="$(pwd)/src-tauri/resources/libpdfium.dylib"
cargo run --release --bin bench -- "$(pwd)/bench/corpus" "bench/results/headless-bench-$(date +%Y%m%d).md"
```

The bench drives the render core directly (no Tauri/webview) and measures
open/first-tile/tile-latency/page-jump/peak-RSS/after-churn-RSS per tier (each tier in
its own subprocess for isolated RSS). It writes a markdown report to `bench/results/`.
Interactive pan-FPS and zoom-settle timing need the running GUI — measured via the in-app
overlay (press **B**), see the runbook.

**Tuning knobs** (for tight 16 GB targets):
- `REDLINE_CACHE_MB=256 ./target/release/bench ...` — tile-cache byte budget (default 512 MiB).
- Page-handle LRU cap = `DEFAULT_MAX_LOADED_PAGES` (24) in `src-tauri/src/render/mod.rs` — the
  dominant steady-RSS lever (loaded pages hold PDFium's parsed state, not the tile cache).
  Also exposed as `RenderEngine::with_max_loaded_pages`.

## Definitive floor-machine run

See **`bench/RUNBOOK-S20.md`** — the exact copy-corpus → press-go steps to run the
definitive §20 on the 16 GB floor machine on both Windows 11 and macOS, including the GUI
pan-FPS / zoom-settle procedure and the result→§20-threshold mapping.

---

## §20 Acceptance Criteria Checklist

**Floor machine** (all targets measured here):
- 16 GB RAM, 4-core SSD laptop, integrated GPU
- Both **Windows 11** and **macOS (Apple Silicon)**
- ≤ 2 GB RSS budget is the binding constraint

### Corpus tiers

All five tiers are **REQUIRED v1 use cases** (spec §20). Corpus in place:

| ID | Profile | Actual file used |
|----|---------|------------------|
| C1 | ~80–150 MB, ~100 sheets, vector | RUA SB5 contract set, 110 MB, **691 pages** A4 (Bluebeam) |
| **C2 (headline gate)** | **~300 MB, 200+ sheets, vector-heavy** | Observatory DBR, 225 MB, **854 pages** large-format |
| C3 | 500 MB+ stress | RUA SB5 signed contract, 955 MB, 691 pages (same geometry as C1 → sub-linear RSS test) |
| C4 | One huge large-format vector sheet | HoH overall plan, 16 MB, **single dense A0** (3370×2384 pt) |
| C5 | Raster-heavy scanned set | HoH product datasheets, **2.28 GB**, 133 pages (full-page raster) |

### Thresholds (floor machine, C2 unless noted; Pass = required to leave M1)

| Metric | Pass | Stretch | Result |
|--------|------|---------|--------|
| Cold open → first sheet visible | ≤ 3 s | ≤ 1.5 s | — |
| Open → fully interactive (pan/zoom) | ≤ 5 s | ≤ 2 s | — |
| Pan frame time, any zoom | ≤ 33 ms (30 fps) sustained | ≤ 16 ms (60 fps) | — |
| Zoom → placeholder shown | ≤ 16 ms (immediate) | — | — |
| Zoom → sharp tiles settled | ≤ 250 ms | ≤ 120 ms | — |
| Jump to arbitrary sheet → sharp | ≤ 600 ms | ≤ 300 ms | — |
| Single tile rasterize | ≤ 60 ms | ≤ 30 ms | — |
| Peak RSS, active use (C2) | ≤ 2.0 GB, **bounded** | ≤ 1.2 GB | — |
| Peak RSS (C3, 500 MB+) | ≤ 2.5 GB, no OOM | — | — |
| Memory after ~1 hr churn (100+ sheets) | within +15% of steady baseline; **no monotonic growth** | flat | — |
| Crash / OOM (C2 & C3) | none | none | — |

### Invariants (must hold beyond timings)

1. **Memory does not scale linearly with file size** — C2-vs-C3 peak RSS *similar* (not ~1.6×). Proof that streaming actually works.
2. **Tiles sharp at every zoom** — rendered at zoom × DPR, never upscaled; visual check at extreme zoom.
3. **Vector geometry extraction for snapping** works on C4 without Form-XObject paths collapsing to ~(0,0) — pdfium-render *transformed* path-segment iteration.

### Go / No-Go

- All **Pass** on C1 + C2 (both OSes) → proceed to M2.
- C2 fails → **STOP and escalate** with mitigation options before layering markup/takeoff:
  - LOD / vector-tile rendering
  - Alternate render library
  - Raise hardware floor
  - Scope cut

---

## PDFium Setup

pdfium-render needs a prebuilt PDFium shared library. **It is not bundled in this repo.**

### macOS (Apple Silicon — development)

```bash
# Download the macOS arm64 PDFium binary from pdfium-binaries releases:
# https://github.com/bblanchon/pdfium-binaries/releases
# Look for: pdfium-mac-arm64.tgz

curl -L -o /tmp/pdfium-mac-arm64.tgz \
  https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-mac-arm64.tgz

mkdir -p src-tauri/resources
tar -xf /tmp/pdfium-mac-arm64.tgz -C /tmp/pdfium-mac-arm64/
cp /tmp/pdfium-mac-arm64/lib/libpdfium.dylib src-tauri/resources/

# Set the env var for cargo build / tests:
export PDFIUM_DYNAMIC_LIB_PATH="$(pwd)/src-tauri/resources/libpdfium.dylib"
```

### macOS (Intel)

Same as above but use `pdfium-mac-x64.tgz`.

### Windows x64

Download `pdfium-win-x64.tgz`, extract `pdfium.dll`, place in `src-tauri/resources/`.
Set `PDFIUM_DYNAMIC_LIB_PATH=<path>\pdfium.dll` in your shell.

### CI / automated builds

Set `PDFIUM_DYNAMIC_LIB_PATH` as a CI environment variable pointing at the
pre-downloaded binary. The binary should NOT be committed to the repo.

---

## Corpus placement

Place the plan sets Martin provides (task:jtqa5129w4nd69s6qfsd) in:

```
bench/corpus/
  c1-typical/   # ~80–150 MB vector sets
  c2-large/     # ~300 MB, 200+ sheet headline gate set(s)
  c3-stress/    # 500 MB+ sets
  c4-dense/     # Single huge large-format vector sheet
  c5-scanned/   # Raster-heavy scanned set
```

The bench picks the first `*.pdf` in each tier directory.

`bench/corpus/` is in `.gitignore` — plan sets are confidential and large.

---

## Harness (implemented)

The harness is `src-tauri/src/bin/bench.rs` — a headless Rust binary that drives
`RenderEngine` directly. Per tier it measures: cold open → first tile, single-tile
rasterize latency (p50/p95/max over a sample of pages/tiles), page-jump latency,
peak RSS during active render, RSS after a two-pass churn loop over up to 120
pages (the no-leak check), and the C2-vs-C3 sub-linear-RSS invariant. RSS is read
via `ps -o rss=` (KB on macOS/Linux). Tiles are rendered at zoom×DPR = 2× (512px
CSS × 2.0 DPR), the Retina worst case.

What the headless harness **cannot** measure (needs the running GUI):
- interactive pan frame time / sustained pan FPS
- zoom placeholder-show and sharp-settle timing
- GPU compositing behaviour

Those remain for a GUI bench on the floor machine.

---

## Results

Committed results go in `bench/results/` as dated markdown. The most recent
indicative run (Apple Silicon dev Mac) and the indicative verdict are there.
The verdict explicitly separates "indicative PASS" (headline multi-page memory +
latency invariants, measured) from "needs floor machine + both OSes + GUI pan-FPS"
(the definitive §20 Go/No-Go).
