# Redline — Handover Notes

## Current Status

**M1-M6 + Phase 1.1 (compare) all complete and merged to `main`. 0 open PRs as of 2026-07-02.**

Latest work: PR #32 (count-marker selection hit-region fix + per-page count breakdown +
page number in Properties panel), squash-merged to `main` as `fff93c8` (2026-06-30). Main
is the working branch — there is no active feature branch right now.

## Last Session

**Date**: 2026-06-30 (PR #32)
**Summary**: Fixed count-marker selection — non-circular symbols (Cross/Square/Diamond)
were unselectable because the hit-test used Euclidean distance against a tolerance smaller
than the symbol's corner radius; switched Point-geometry hit-testing to a Chebyshev
(bounding-box) test. Added a per-page count breakdown under each count set's subtotal in
the Quantities panel. Added a read-only "Page: N" row to the Properties panel (selection
mode only). 14 new tests; 496 FE tests reported green in the PR (not re-run this pass —
this is a documentation-only session, see below).

## Next Steps

No active development branch. Two verification items remain owed; everything else is
owner-gated:

1. **§20 definitive floor-machine run** (16 GB RAM, Windows + macOS) — the formal M1
   Go/No-Go. Only an *indicative* pass exists today (Apple Silicon, headless). Blocked on
   hardware access. Procedure: `bench/RUNBOOK-S20.md`.
2. **G9 human visual check** — regenerate the sample via `cd src-tauri && cargo test
   g9_emit_sample -- --ignored --nocapture` (writes `/tmp/redline-g9-sample.pdf`), then open
   it in Acrobat AND Bluebeam to confirm font + group rendering. Owed since M2 (2026-06-16).
3. **Project direction beyond polish** (pause / registration fast-follow / next milestone)
   is an owner-gated decision on Martin's business backlog — not yet made, don't infer one.

## Housekeeping flagged for the orchestrator

- This session found the repo checked out on a stray local branch `verify/redline-count`
  (3 commits identical in content to what PR #32 squash-merged, never pushed/PR'd) with an
  uncommitted, stale edit to this file. Stashed that edit (`git stash list` — message
  "stray HANDOVER.md edit + worktrees on verify/redline-count") and switched to `main`,
  which was 1 commit behind `origin/main` and has been fast-forwarded. The stray branch
  itself was left alone (not deleted — out of scope for a docs-only pass).
- `.claude/worktrees/` has 9 untracked, uncommitted agent-worktree directories from
  2026-06-30 (`agent-<hash>/`, one per dispatched sub-agent). Not part of this task; flagging
  for cleanup since it's untracked disk clutter in the repo root.

## Key Context

| Item | Value |
|------|-------|
| Remote | `git@ssh.forge.mms.name:emittiv/redline.git` |
| Main branch | `main` @ `fff93c8` (M1-M6 + Phase 1.1 all merged, 0 open PRs) |
| Active branch | none — work happens directly off `main` until the next feature starts |
| KB mission record | `project:q8gm8dv3k7smld12rm25` (stage: stabilizing, health: on_track) |
| Ship pipeline | `.claude/skills/sendit/SKILL.md` |
| Judgment rules | `.claude/rules/judgment.md` (new, 2026-07-02 — incident/decision distillation) |

## Key Gotchas (carry forward)

- **`FolderIndex::alive()`** = `Arc::strong_count(&self.inner) > 1` — background watcher thread exits within ~1s of AppState replacing the index
- **Background indexer uses `std::thread::spawn`** (not tokio) — watcher loop is indefinitely blocking, must NOT consume tokio's blocking thread pool
- **Tantivy `Document` trait must be imported** for `to_json()` to be in scope: `use tantivy::{Document, ...};`
- **Svelte store is in-session SoT**; Rust store is a mirror + save buffer. `flush()` throws on undrained mirror queue.
- **lopdf reals: read with `as_float()`, NEVER `as_f32()`** — see `.claude/rules/judgment.md` for the full incident (integer-valued reals silently corrupt on save→reopen)
- **lopdf borrow checker pattern**: immutable read phase (collect owned structs) then mutable write phase — avoids aliasing on `&mut Document`
- **lopdf `Stream::compress()` threshold**: only applies Deflate when `compressed.len() + 19 < original.len()` — streams shorter than ~50 bytes typically don't compress
- **M5 flatten v1 limitation**: only handles indirect /AP /N appearance streams; inline /AP /N streams (rare) are preserved as-is
- **M5 optimize v1 limitation**: no deep image downsampling (spec §8 — deferred to pluggable engine)
- **Recent docs IPC**: lives in `src/lib/recent-docs.ts` (NOT `ipc.ts`) — intentional conflict-avoidance pattern, see judgment.md
- Overlay `pointer-events` toggles via `isCreateTool()`; Hand tool pans, creation tools capture on SVG overlay
- §5 precision invariant: overlay maps PDF user space → screen every render (never reads raster)
- **`next_version_seq`** in `SidecarMeta` is monotonic — increment BEFORE deriving seq; don't revert to `versions.len()+1` (breaks after prune)
- PDFium 2 GiB limit, global C state, `RenderEngine` drop order — unchanged from M1
- Tests: `npm run test` (vitest, mixed node+jsdom). Rust: `cargo test` from `src-tauri/` (not project root)

---
*Updated: 2026-07-02 (departing-architect distillation pass)*
