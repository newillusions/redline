# C2 tuning sweep — cache budget + page-LRU cap (2026-06-08)

Apple Silicon dev Mac (NOT the §20 floor machine). Headless bench, C2 headline tier
(`c2-observatory-854pg-largeformat.pdf`, 215 MB, 854 pages). Goal: identify which lever
moves steady/peak RSS before the floor-machine run, and pick safe defaults.

## Cache-budget sweep (page cap fixed at default 24)

| Cache budget | Peak RSS (pass 0) | Steady RSS (pass 1) | Tile cache used |
|---|---|---|---|
| 512 MiB (default) | 899 MB | 431 MB | 133 MB (400 tiles) |
| 256 MiB | 898 MB | 429 MB | 133 MB (400 tiles) |

**Cache budget does not bind for C2.** The pan/zoom working set fills the cache to only
~133 MB — far below even 256 MiB — so halving the budget changes RSS by ~2 MB (noise).
The tile cache is not the C2 RSS driver. Lowering the default to 256 MiB would be free for
C2 (and cheap insurance against a pathological tile-heavy session) but yields no headline win.

## Page-LRU-cap sweep (cache fixed at default 512 MiB)

| max_pages | Peak RSS (pass 0) | Steady RSS (pass 1) | first-tile ms |
|---|---|---|---|
| 8  | 735 MB  | 422 MB | 6.8 |
| 12 | 799 MB  | 425 MB | 4.6 |
| 24 (default) | 898 MB | 429 MB | 4.6 |
| 48 | 1027 MB | 482 MB | 4.6 |

**The page cap is the dominant lever, and it moves PEAK far more than steady.** §20 budgets
*peak* RSS (≤2.0 GB on C2). Steady is nearly flat (C2's on-screen working set is small);
peak scales roughly linearly with the cap. Lowering 24→8 cuts peak ~163 MB at negligible
latency cost (first-tile 4.6→6.8 ms; tile p50/p95 unchanged). C4 (single dense A0) needs
cap ≥ 1 — the floor there is one page.

## Conclusion

At default (24 pages / 512 MiB), C2 on this Mac: **peak ≈0.9 GB, steady ≈0.43 GB** — both
far under the 2.0 GB budget with ~1.1 GB margin. No further blind tuning is warranted; the
defaults are sound and the curve shape is benign. The binding unknown is the floor machine's
*absolute* numbers (16 GB / 4-core / integrated GPU, Windows + macOS), which this hardware
cannot stand in for.

**For the floor run:** both levers are now bench-tunable via env —
`REDLINE_CACHE_MB` and the new `REDLINE_MAX_PAGES`. If the floor machine shows C2 peak
near 2 GB, drop `REDLINE_MAX_PAGES` to ~12 to reclaim ~15% peak before considering any
deeper change. See `bench/RUNBOOK-S20.md`.

## Not addressed here: C5 normalise transient

The ~3.4 GB one-time normalise peak (C5, oversized scanned) is unchanged — it is a toolchain
limit, not a tuning knob. `lopdf::Document::load` builds the whole file in memory and lopdf
has no streaming/incremental save. Reducing it requires swapping the normalise backend
(e.g. a `qpdf` subprocess, or mupdf) — a dependency decision, deferred for explicit sign-off.
