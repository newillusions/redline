# Redline - Handover Notes

*Rewritten 2026-06-12 (Fable 5 config-review session). The previous version predated the
M1 merge and all M2 work.*

## Current Status

**M2 (markup) in flight - S1 (document save pipeline) code-complete on `feat/m2-annotation-serde`.**
`main` carries the M1 squash (b050f47) and the M2 markup envelope (f6300fe, PR #1). The
branch is 13 commits ahead of origin (ae4ea2c..90f0a8c): markup store, lopdf annotation
read/write, atomic save, Tauri command layer (close-swap-reopen + save-in-flight guard +
load-before-save guard), ipc.ts types/wrappers, Save/Save-As UI + Cmd/Ctrl+S, corpus C1
e2e fidelity test. Each task two-stage reviewed (spec + quality) with fix loops; final
whole-slice review verdict SHIP. 44 Rust tests pass (`--test-threads=1`), clippy 0,
`npm run check` 0 errors. NOT yet pushed/merged - awaiting GUI smoke + `/sendit`.

S1 plan: `docs/superpowers/plans/2026-06-12-s1-document-save-pipeline.md`.
Master roadmap: `docs/superpowers/plans/2026-06-12-v1-completion-roadmap.md`.

### S1 follow-ups deferred to later slices (from final review, non-blocking)
- S2 contract: after a save, the frontend MUST re-pull `listMarkups` to reconcile the
  bottom markup-list panel with the store (no markup UI exists yet, so nothing stale now).
- C5 perf cliff: an oversized (>1.9 GB) file reopens via the lopdf-normalize path after
  every in-place save (re-normalizes the original-path file). Correctness fine; cache the
  normalized working copy in a later perf pass.

§20 status: indicative PASS on Apple Silicon (headless), with large memory margin after
the page-handle LRU fix (C2 headline 1546→431 MB). The definitive Go/No-Go (16 GB floor
machine, Windows + macOS, GUI pan-FPS) is STILL OWED - procedure in `bench/RUNBOOK-S20.md`.
M2 proceeded ahead of it by decision (logged in CLAUDE.md Key Decisions).

2026-06-12 session: Fable 5 first run; full config review + cleanup (CLAUDE.md commands/
decisions filled in, permission allowlist pruned, MEMORY.md deduped, hub recommendation to
dev re: orchestrator CLAUDE.md scope line + opus-4-7-calibration retirement).

## Next Steps

1. GUI smoke S1 (`cargo tauri dev`: open a small PDF → Cmd+S → reopen, confirm no error;
   external-viewer check that the saved file opens), then ship `feat/m2-annotation-serde`
   via `/sendit` (run `/code-review` first - render/save path is risky-diff)
2. S2 (next slice): markup overlay + authoring UI + undo/redo - the big one
3. S3-S6 finish M2: review workflow + list panel, sidecar, Tool Sets/stamps, .btx importer
4. Definitive §20 floor-machine run, Windows + macOS (blocked on hardware - Martin)
5. Verify Windows build/bundle on a real Windows box (wired, untested)

## Key Gotchas (carry forward)

- PDFium 2 GiB internal object-offset limit: >2 GiB files need lopdf normalise before page-load
- PDFium global C state: tests MUST be serial (`--test-threads=1`); production serialises via RenderHandle
- `RenderEngine` field drop order: `documents` before `pdfium` (dylib owner) or SIGSEGV at teardown
- Page-handle LRU cap (24 pages) is the real RSS control - NOT the tile cache (byte-budgeted 512 MiB)
- `default-run = "redline"` is set (the `bench` bin otherwise breaks bare `cargo run`)
- Corpus-dependent tests are NOT run by the /sendit gate - run manually before shipping render-path changes

## Key References

| Item | Value |
|------|-------|
| Spec | `docs/bluebeam-alternative-v1-spec.md` (§5 precision, §6 markup, §20 perf) |
| §20 runbook | `bench/RUNBOOK-S20.md` |
| Bench results | `bench/results/headless-bench-20260607.md` |
| Ship pipeline | `.claude/skills/sendit/SKILL.md` |
| Remote | `git@ssh.forge.mms.name:emittiv/redline.git` |
| pdfium-render API corrections | obs:8pkkeu6qnpznjcmnzzud |
| rstar PointDistance pattern | obs:cw6prk33xgrjgtcldu53 |
