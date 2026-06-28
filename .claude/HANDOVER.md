# Redline — Handover Notes

## Current Status

**M5 optimize shipped (feat/m5-docops-optimize, PR open).**

- **M4 all complete**: S1-S4 + fmt/clippy cleanup (PR #16, commit `05b149b`)
- **M5 flatten baseline**: PR #17, squash-merged `ba87ed5` (2026-06-27)
- **M5 optimize**: PR #18 open on `feat/m5-docops-optimize` (2026-06-27) — prune unused objects (level 1) + Deflate-compress streams (level 2) via lopdf's `prune_objects()` + `compress()`

All tests: 193 Rust / 340 frontend green. Clippy 0. npm run check 0 errors.

## Last Session

**Date**: 2026-06-27
**Summary**: Implemented M5 optimize. Added `optimize_in_place(doc, level)` to `docops/mod.rs` (level 0 = noop, level 1 = prune, level 2 = prune + compress). Replaced passthrough stub in `LopdfDocOps::optimize()`. Added `optimize_document` Tauri command in `commands/docops.rs`, registered in `lib.rs`, wired `optimizeDocument()` IPC wrapper in `ipc.ts`, added Optimize toolbar button to `App.svelte`. 9 new Rust tests, all green. PR #18 open.

## Untracked Items (left as-is)

- `docs/superpowers/plans/2026-06-13-s2a-markup-overlay-display.md` — old plan doc, not part of any current workstream. Leave untracked unless needed for M6 compare work.

## Next Steps

1. **§20 definitive floor-machine run** (16 GB, Windows + macOS) — OWED, formal M1 Go/No-Go (blocked on hardware)
2. **G9 human step** — open `/tmp/redline-g9-sample.pdf` in Acrobat/Bluebeam to verify Times-fonted text + group rendering
3. **M5 redact** — rasterize-region safe floor: render each redact region via PDFium at high DPI → Image XObject in lopdf → remove annotation; true vector redact deferred to MuPDF/Apryse (spec §8)
4. **M6 compare** — page-pair diff rendering (Phase 1.1): manual two-point registration, color-channel overlay, change-highlight; `compare` module stub exists in `src-tauri/src/compare/mod.rs`

## Key Gotchas (carry forward)

- **`FolderIndex::alive()`** = `Arc::strong_count(&self.inner) > 1` — background watcher thread exits within ~1s of AppState replacing the index
- **Background indexer uses `std::thread::spawn`** (not tokio) — watcher loop is indefinitely blocking, must NOT consume tokio's blocking thread pool
- **Tantivy `Document` trait must be imported** for `to_json()` to be in scope: `use tantivy::{Document, ...};`
- **Svelte store is in-session SoT**; Rust store is a mirror + save buffer. `flush()` throws on undrained mirror queue.
- **lopdf reals: read with `as_float()`, NEVER `as_f32()`** — integer-valued reals serialise without decimal and reload as `Object::Integer`
- **lopdf borrow checker pattern**: immutable read phase (collect owned structs) then mutable write phase — avoids aliasing on `&mut Document`
- **lopdf `Document::load_from(Cursor::new(bytes))`** and **`doc.save_to(&mut out)`** work correctly in 0.36
- **lopdf `Stream::compress()` threshold**: only applies Deflate when `compressed.len() + 19 < original.len()` — streams shorter than ~50 bytes typically don't compress. Tests must use `compressible_stream_content()` (20x repeated PDF ops, ~1 KB) to reliably exercise compression.
- **Resources mutation**: clone /XObject dict as owned, modify, write back as direct dict (acceptable — semantically equivalent to indirect)
- **M5 flatten v1 limitation**: only handles indirect /AP /N appearance streams; inline /AP /N streams (rare) are preserved as-is
- **M5 optimize v1 limitation**: no deep image downsampling (spec §8 — deferred to pluggable engine)
- Overlay `pointer-events` toggles via `isCreateTool()`; Hand tool pans, creation tools capture on SVG overlay.
- Tests: `npm run test` (vitest, mixed node+jsdom). Rust: `cargo test` from `src-tauri/` (not project root)
- §5 precision invariant: overlay maps PDF user space → screen every render (never reads raster)
- **`next_version_seq`** in `SidecarMeta` is monotonic — increment BEFORE deriving seq; don't revert to `versions.len()+1` (breaks after prune)
- PDFium 2 GiB limit, global C state, `RenderEngine` drop order — unchanged from M1

## Key References

| Item | Value |
|------|-------|
| Remote | `git@ssh.forge.mms.name:emittiv/redline.git` |
| Main branch | `main` (M4 + M5 flatten merged) |
| Active branch | `feat/m5-docops-optimize` (PR #18 open) |
| M4 spec | `.claude/specs/2026-06-25-redline-m4-plan.md` |
| Ship pipeline | `.claude/skills/sendit/SKILL.md` |
| KB obs M5 optimize | `obs:TBD` (recorded this session) |
| KB obs M5 flatten | `obs:b0gvmk85m4wpalj5g4i1` |
| KB obs S4 | `obs:v42tjtmz4tmicd2jn0n2` |
| KB obs S2 | `obs:p5uy0je5vj15n72jezaa` |
| KB obs fmt/PR#16 | `obs:o1biypj4taff1irczgcc` |

## M5 Progress

- **flatten baseline**: DONE - PR #17, merged `ba87ed5`
- **optimize**: DONE - PR #18 open on `feat/m5-docops-optimize`
- **redact**: stub only — rasterize-region floor; vector redact needs MuPDF/Apryse (deferred, spec §8)

---
*Updated: 2026-06-27*
