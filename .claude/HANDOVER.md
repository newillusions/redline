# Redline — Handover Notes

## Current Status

**M1.5 done + floor-machine run prepped ("copy corpus → press go").** Branch `m1-shell`
(commit 7404aff, pushed). Indicative §20 passed on Apple Silicon; memory now has huge
margin after the page-LRU fix. Definitive §20 still needs the 16 GB floor machine on
Windows + macOS (headless bench + GUI pan-FPS overlay) — runbook written.

Last session: 2026-06-07 (cache byte-budget, page-LRU RSS fix, GUI smoke + bench overlay,
Windows PDFium bundling, C5 prune, §20 runbook).

### Memory fix (the headline win this session)
Instrumentation disproved the "tile cache drives RSS" assumption — tiles were only
97–133 MB of ~1.5 GB. The driver was **held PDFium page state**. Added a **page-handle
LRU cap (24 pages)**. Steady RSS: C1 984→474, **C2 (headline) 1546→431** (−72%),
C3 1268→694, C5 907→588 MB. C4 single-dense-A0 stays 1305 MB (LRU floor = 1 page).
All tiers now 0.7–1.6 GB under the 2 GB floor budget. Tile cache also made byte-budgeted
(512 MiB default). See bench/results/headless-bench-20260607.md.

### GUI proven
`cargo tauri dev` launches a window, resolves+loads bundled PDFium, auto-opens a real PDF
(verified: C1 691-page + C4 dense A0 open & render live). In-app §20 overlay (press **B**):
pan frame-time, zoom-settle, last-tile, RSS — colour-coded vs thresholds. `process_rss_mb`
+ `auto_open_path` (REDLINE_OPEN_PDF) commands added.

### Windows bundling
`resolve_pdfium_path` in lib.rs `.setup()` finds the bundled lib (resource dir → exe dir),
sets PDFIUM_DYNAMIC_LIB_PATH before the render thread spawns. `tauri.conf.json`
bundle.resources maps `resources/`. `scripts/fetch-pdfium.sh <target>` fetches the pinned
(chromium/7869) per-OS binary. `default-run="redline"` (bench bin broke bare `cargo run`).
Windows path is wired + documented but UNVERIFIED (no Windows box here).

### C5 ingest
prune_objects() + compress() cut normalise from ~7.9s/~4.6GB → ~6.1s/~3.4GB transient.
Still over §20 open ≤5s; one-time ingest cost. Full streaming rewrite deferred (lopdf is
in-memory only). Steady render after normalise: 1.8 ms/tile, 585 MB.

## What Was Built

### M1 shell (commit 13a7c65)
- Tauri 2 + Svelte 5 runes + Vite + TS; full spec §4 module layout
- pdfium-render 0.8.37, rstar 0.12, lopdf, memmap2, tokio, uuid, image
- PDFium thread-isolation: `RenderHandle` (Send+Sync mpsc) → dedicated `redline-render`
  OS thread owning `RenderEngine`+`Pdfium` (Pdfium is !Send+!Sync)
- Frontend: ipc.ts, viewport.ts, 3-column layout, Viewport.svelte, CSS tokens

### M1.5 + fixes (this session)
- **True tile-region matrix render** (`FPDF_RenderPageBitmapWithMatrix`): allocates only
  tile_w×tile_h, never the full page. Replaced the M1 full-page-render+crop.
- **Page-handle cache** per (doc,page): fixed C4 dense A0 from **1100 ms/tile → 35 ms/tile**.
- **Size-based load**: streaming <1.9 GiB; mmap + `FPDF_LoadMemDocument64` ≥1.9 GiB.
- **Auto-normalise-on-open** (lopdf compress+re-serialise) for files PDFium can't
  page-load (>2 GiB internal offset limit): fixed C5 (2.1 GB scanned). Original never modified.
- **Drop-order fix**: `documents` drop before `pdfium` (dylib owner) — killed a teardown SIGSEGV.
- **Headless bench** (`src-tauri/src/bin/bench.rs`): per-tier subprocess isolation for clean RSS.

## Benchmark Results (indicative, Apple Silicon — see bench/results/headless-bench-20260607.md)

| Tier | File | Pages | Open | 1st tile | Tile p50/p95/max | RSS steady |
|------|------|-------|------|----------|------------------|-----------|
| C1 | 105 MB | 691 | 3 ms | 3 ms | 2.3/18.6/56.2 ms | 986 MB |
| C2 (headline) | 215 MB | 854 | 67 ms | 5 ms | 1.3/21.8/49.5 ms | 1579 MB |
| C3 | 911 MB | 691 | 14 ms | 6 ms | 2.8/40.6/51.8 ms | 1267 MB |
| C4 dense A0 | 15 MB | 1 | 1291 ms | 1080 ms | 34.7/316.8/1730.9 ms | 1305 MB |
| C5 scanned | 2175 MB | 133 | 7910 ms | 15 ms | 1.6/14.8/17.2 ms | 907 MB |

- **Sub-linear RSS invariant PROVEN**: C1 986 MB vs C3 1267 MB (same 691 pp, 8.7× bytes) = 1.28×.
- No leak (churn Δ +0.0–6.5%). All steady RSS < 2 GB on this machine.
- C4 p95/max high = first-tile-of-page parse (one-time, ~1 s); steady p50 35 ms.
- C5 normalise = one-time ~8 s / ~4.6 GB transient RSS (2.28 GB → 54.8 MB; 97% bloat).

## Build Results (this session)

- `cargo build --all-targets`: PASS
- `cargo test` (default, no PDFium): 9 passed, 0 failed
- `cargo test --release -- --test-threads=1` (PDFium + corpus, REDLINE_BENCH_TESTS=1): all pass
  (PDFium needs serial tests — global C state; production serialises via RenderHandle)
- `cargo clippy --all-targets`: 0 warnings

## Next Steps

### Definitive §20 verdict (still needed — NOT done)

The indicative pass is on Apple Silicon + headless only. Definitive Go/No-Go needs:
1. **Floor machine** (16 GB RAM, 4-core, integrated GPU) — the binding budget.
2. **Both Windows 11 (pdfium.dll) and macOS.**
3. **Interactive pan-FPS + zoom placeholder/settle** — GUI-only, run via `cargo tauri dev`
   with an instrumented frame loop (headless bench cannot measure these).
4. **Tune the tile cache before the floor run** — 256 tiles × ~4 MB (1024² RGBA at 2× DPR)
   ≈ up to 1 GB; it's the bulk of the ~1–1.6 GB steady RSS. Byte-budget or lower the cap
   (`RenderEngine.cache_cap`) so the 16 GB floor keeps headroom under 2 GB.
5. **Reduce the C5 normalise transient** (~4.6 GB) — stream-rewrite instead of whole-file
   lopdf load — so the 16 GB floor isn't stressed during ingest.

### After §20 Go verdict

- M2: markup annotation model + PDF serialization + BTX importer

## Open Tasks

- [x] Corpus sourced from server (all 5 tiers, gitignored)
- [x] PDFium arm64 binary installed + scripts/fetch-pdfium.sh for all targets
- [x] M1.5 render + page cache + mmap + auto-normalize + drop-order fix
- [x] Tile-cache byte-budget + **page-handle LRU cap (the real RSS fix)**
- [x] Headless benchmark + indicative verdict
- [x] GUI smoke (window + auto-open render verified) + in-app §20 bench overlay
- [x] Windows PDFium bundling wired (resolve_pdfium_path + bundle.resources + fetch script)
- [x] C5 normalise prune improvement (measurable progress; streaming rewrite deferred)
- [x] bench/RUNBOOK-S20.md (copy-corpus → press-go for the floor machine)
- [ ] **Definitive §20 on the 16 GB floor machine, Windows + macOS** (follow RUNBOOK-S20.md)
- [ ] Verify the Windows build/bundle on an actual Windows box (path wired, untested here)
- [ ] Merge `m1-shell` → `main` after definitive §20 Go verdict

## Key API Corrections / Gotchas (KB)

obs:8pkkeu6qnpznjcmnzzud, obs:cw6prk33xgrjgtcldu53 (M1) plus this session:
- `pdfium_render::prelude::*` for `PdfDocument`; no `clip_page_to_bounding_box`; use matrix
- `PdfRenderConfig::apply_matrix(PdfMatrix)` returns Result; matrix maps PDF→device (Y flip)
- PDFium **2 GiB internal object-offset limit**: both `FPDF_LoadCustomDocument` AND
  `FPDF_LoadMemDocument64` fail page-load on >2 GiB files — normalise via lopdf
- PDFium global C state: tests MUST run `--test-threads=1`; production serialises via RenderHandle
- `RenderEngine` field drop order: `documents` before `pdfium` (dylib owner) or SIGSEGV at teardown
- Page-handle cache is essential for dense pages (1100 ms→35 ms/tile)

## Key References

| Item | Value |
|------|-------|
| Branch | `m1-shell` (pushed) |
| Commit | 13a7c65 |
| Forge PR | https://forge.mms.name/emittiv/redline/compare/main...m1-shell |
| M1 spike task | task:dcufqdmr446ek7u9jcnr (completed) |
| Corpus task | task:jtqa5129w4nd69s6qfsd (pending Martin) |
| pdfium-render API obs | obs:8pkkeu6qnpznjcmnzzud |
| rstar pattern obs | obs:cw6prk33xgrjgtcldu53 |
| PDFium binary source | https://github.com/bblanchon/pdfium-binaries/releases |
| Spec | `docs/bluebeam-alternative-v1-spec.md` |
| §20 acceptance | `bench/README.md` |
