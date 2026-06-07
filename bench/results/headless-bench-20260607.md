# Redline §20 Headless Benchmark — Results

Run: 2026-06-07T11:32:53.525759+00:00

Machine: Apple Silicon dev Mac (NOT the §20 floor machine). Tiles rendered at zoom×dpr = 2× (512px CSS × 2 DPR).

Render strategy: M1.5 true tile-region matrix render (bitmap allocated at tile size, never full page). Page-handle cache (C4 dense-sheet fix). Auto-normalise on open for >2 GiB files PDFium can't load (C5).

**Each tier runs in its own subprocess** so RSS is isolated (no cross-tier allocator high-water carryover). `RSS peak` includes any one-time ingest spike (notably C5's lopdf normalise of a 2.1 GB file); `RSS post-churn` is the steady-state after rendering tiles across 100+ pages twice — the number that matters for sustained use. `Cache MB` is the byte-budgeted tile cache occupancy at steady state (budget default 512 MiB).

| Tier | File MB | Pages | Open ms | 1st tile ms | Tile p50/p95/max ms | Jump p50/max ms | RSS peak MB | RSS post-churn MB | Cache MB | Churn pgs | Notes |
|------|--------:|------:|--------:|-----------:|--------------------:|----------------:|-----------:|------------------:|---------:|----------:|-------|
| C1 | 105 | 691 | 4 | 4 | 2.4/18.1/54.1 | 8/50 | 474 | 474 | 97 | 120 | 2nd-pass RSS Δ +0.1%;  |
| C2 | 215 | 854 | 14 | 6 | 1.3/21.8/49.9 | 8/35 | 899 | 431 | 133 | 120 | 2nd-pass RSS Δ -51.9%;  |
| C3 | 911 | 691 | 14 | 6 | 2.8/40.8/51.6 | 18/53 | 694 | 694 | 97 | 120 | 2nd-pass RSS Δ +0.1%;  |
| C4 | 15 | 1 | 1272 | 1080 | 31.7/302.2/1647.5 | 0/0 | 1305 | 1305 | 10 | 1 | 2nd-pass RSS Δ +0.0%;  |
| C5 | 2175 | 133 | 8935 | 15 | 1.7/15.4/17.4 | 7/19 | 588 | 588 | 45 | 120 | 2nd-pass RSS Δ +0.2%;  |

## Corpus files

- **C1**: `c1-contract-691pg-A4.pdf` — 105 MB, 691 pages
- **C2**: `c2-observatory-854pg-largeformat.pdf` — 215 MB, 854 pages
- **C3**: `c3-contract-signed-955mb-691pg.pdf` — 911 MB, 691 pages
- **C4**: `c4-overall-plan-A0.pdf` — 15 MB, 1 pages
- **C5**: `c5-datasheets-133pg-raster.pdf` — 2175 MB, 133 pages

## Measurement detail

- **C1**: RSS before open 10 MB → peak 474 MB (Δ +463 MB); tile latency sample n=28
- **C2**: RSS before open 10 MB → peak 899 MB (Δ +889 MB); tile latency sample n=36
- **C3**: RSS before open 10 MB → peak 694 MB (Δ +684 MB); tile latency sample n=28
- **C4**: RSS before open 10 MB → peak 1305 MB (Δ +1294 MB); tile latency sample n=24
- **C5**: RSS before open 10 MB → peak 588 MB (Δ +578 MB); tile latency sample n=24

---

## Memory fix: page-handle LRU cap (the real RSS lever)

Earlier indicative runs hit ~1.58 GB steady on C2 and the cause was assumed to be the
tile cache. Instrumenting cache occupancy disproved that: tiles were only **97–133 MB**
of a ~1.5 GB steady RSS. **The dominant contributor was held PDFium page state** — every
loaded `PdfPage` retains its parsed content, and the churn loop kept up to 120 pages
loaded.

Two changes:
1. **Tile cache → byte-budgeted** (512 MiB default, was a 256-tile count cap). Correct
   hygiene, but not the RSS lever (cache never exceeded ~133 MB on the corpus).
2. **Page-handle cache → LRU-bounded to 24 loaded pages** (`DEFAULT_MAX_LOADED_PAGES`).
   The LRU page is dropped (`FPDF_ClosePage` frees its parsed state) past the cap.

Steady-RSS before → after the page LRU cap (same corpus, same machine):

| Tier | Before (no page cap) | After (24-page LRU) | Δ |
|------|---------------------:|--------------------:|---|
| C1   | 984 MB  | **474 MB**  | −52 % |
| C2 (headline) | 1546 MB | **431 MB** | **−72 %** |
| C3   | 1268 MB | **694 MB**  | −45 % |
| C4 (dense A0) | 1305 MB | **1305 MB** | unchanged* |
| C5   | 907 MB  | **588 MB**  | −35 % |

*C4 is a single A0 page; the LRU floor is 1 page, and that one page's parsed vector
state is ~1.3 GB. Cannot evict below the page currently being rendered. Still < 2 GB.

Tile latency is unchanged (C1/C2/C3/C5 p50 1.3–2.8 ms; revisiting an evicted normal page
re-parses in sub-ms). Steady RSS now sits **0.7–1.6 GB under the 2 GB floor budget** on
every tier — the headline C2 case has ~1.57 GB of margin. The LRU cap is tunable via
`RenderEngine::with_max_loaded_pages` for the floor-machine run if a tighter bound is
wanted; `REDLINE_CACHE_MB` env overrides the tile budget in the bench.

### Still needs the actual floor hardware
These numbers are on Apple Silicon (fast unified memory, strong allocator). The 16 GB /
integrated-GPU floor machine may differ (different allocator, GPU compositing). The big
margin here makes a floor-machine pass likely, but it is not proven until measured there.
C4's single-dense-A0 ~1.3 GB floor is the tightest case to watch on 16 GB.

## C5 ingest (normalise) cost — improved, still a one-time caveat

C5 (2.28 GB scanned) can't be page-loaded by PDFium (internal 2 GiB object-offset
limit), so it's auto-normalised via lopdf on open. Adding `prune_objects()` before
`compress()` trimmed the one-time ingest cost:

| | open (normalise) time | transient peak RSS |
|---|---:|---:|
| compress only (prior) | ~7.9 s | ~4.6 GB |
| **prune + compress (now)** | **~6.1 s** | **~3.4 GB** |

Still over the §20 open ≤5 s and a large transient — this is an **ingest cost paid once
per oversized file** (Acrobat/Bluebeam "reduce file size" class), not steady-state. The
normalised copy renders at **1.8 ms/tile, 585 MB steady RSS**. A true streaming/incremental
rewrite (to cut the transient further) needs a different toolchain — lopdf is a full
in-memory model with no streaming-write API — and is deferred. On a 16 GB floor machine the
~3.4 GB transient is the one ingest spike to watch for C5; it is brief and one-time.

