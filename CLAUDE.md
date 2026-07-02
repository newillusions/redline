# redline

## Purpose
Cross-platform (Windows + macOS) desktop application for AEC PDF **markup, takeoff, and document management** - an internal replacement for Bluebeam Revu seats, optimized for very large construction plan sets. Zero license cost for v1; architecture kept clean and module-bounded so a future commercial release stays possible.

The authoritative v1 technical specification lives at `docs/bluebeam-alternative-v1-spec.md`. Read it first.

**Instance scope:** This file governs the redline project instance. The workspace orchestrator CLAUDE.md inherited from `/Volumes/base/dev/` describes the orchestrator's review-only role; its boundaries ("never edit project code", "never commit in project repos") apply to that instance, NOT to work in this repo.

## Tech Stack
- **Shell:** Tauri 2.x (Rust core + OS webview)
- **Frontend:** Svelte 5 + Vite (SPA mode) + TypeScript
- **UI docking:** `dockview-core` (MIT); `svelte-splitpanes` as the lighter fallback
- **Render engine:** PDFium via `pdfium-render` 0.8.x (BSD) - display only
- **Low-level PDF ops:** `lopdf`
- **OCR:** Tesseract (Apache-2.0) via `leptess`
- **Full-text search:** Tantivy (MIT) - folder/library index
- **Doc-surgery backend:** trait-based, swappable - free baseline for v1; MuPDF/Apryse pluggable later
- **Targets:** Windows x64, macOS (universal)

## Architecture (Rust core modules)
`render` (tiled rasterization, display only) · `geometry` (vector path extraction + snap-target spatial index, PDF user space) · `document` (parse/model, page manipulation) · `text` (extraction + search) · `ocr` (invisible text layer) · `search` (Tantivy folder index) · `markup` (annotation model → standard PDF annotations) · `takeoff` (calibration, measurement, quantity calc, f64 user space) · `docops` *(swappable trait: flatten/optimize/redact)* · `compare` (Phase 1.1) · `storage` (local-first + version hooks).

**Precision-critical invariant:** display (raster tiles) and geometry (vector snap targets) are two independent layers. Snapping/measurement NEVER read the raster - all math runs in PDF user space at f64. See spec §5.

## Commands
```bash
# Dev
cargo tauri dev      # full app; resolves bundled PDFium, auto-opens $REDLINE_OPEN_PDF if set

# Build (release bundle)
cargo tauri build

# Tests
cargo test           # portable Rust tests (no PDFium/corpus required)
REDLINE_BENCH_TESTS=1 cargo test --release -- --test-threads=1
                     # PDFium + corpus tests - MUST run serial (PDFium global C state)
npm test             # vitest (frontend)
npm run check        # svelte-check

# Lint / format
cargo clippy --all-targets && cargo fmt
npm run lint && npm run format
```
Gotchas: `default-run = "redline"` is set (the headless `bench` bin otherwise breaks bare
`cargo run`). The §20 corpus is machine-local and gitignored (`bench/corpus/`). Benchmark
procedure: `bench/RUNBOOK-S20.md`.

## Build Order (milestones)
M1 shell + tiled render (large-file perf is the make-or-break test - validate on 300 MB+ sets early) → M2 markup + Tool Sets + `.btx` importer → M3 takeoff → M4 Sets/versioning + page ops + search/OCR → M5 `docops` baseline → M6 (Phase 1.1) compare. Full detail in spec §13.

**Current phase (2026-07-02): M1-M6 + Phase 1.1 all shipped to `main`, 0 open PRs.** Work since has been small polish/fix PRs on the takeoff + markup panels (see git log). Two verification gates remain owed, not yet closed:
- **§20 definitive floor-machine run** (16 GB, Windows + macOS) - the formal M1 Go/No-Go. The current §20 verdict is only the *indicative* Apple-Silicon/headless pass; blocked on hardware access. Do not represent M1 as formally gated-through until this runs. `bench/RUNBOOK-S20.md`.
- **G9 human visual check** - open a sample PDF in Acrobat/Bluebeam to confirm font + group rendering interop (owed since M2, 2026-06-16).
Project direction beyond polish (pause / registration fast-follow / next milestone) is an owner-gated decision, not yet made - don't infer one.

## Key Decisions
- `.btx` Tool Set + stamp import is a **v1 requirement** (XML/UTF-8; reuses the annotation parser). Spec §6.
- Dynamic stamps: compose the appearance ourselves, NOT via embedded PDF JavaScript. Spec §6.
- Redaction v1 = rasterize-the-region safe floor; true vector redaction only via a mature engine behind the `docops` trait. Spec §8.
- Markup model carries reserved workflow fields (status/assignee/thread) + stable IDs from day one, for the future field-tools mobile app + async sync. Spec §6.
- Field tools deferred post-v1 as a standalone mobile/tablet app sharing the Rust core. Spec §2/§12.
- M2 proceeded ahead of the definitive §20 floor-machine run (2026-06): the indicative §20 PASS on Apple Silicon stands in; the floor run (16 GB, Windows + macOS, `bench/RUNBOOK-S20.md`) remains the formal Go/No-Go and is still owed.
- Annotations persist as standard PDF annotation objects per the spec §6 persistence map (M2) - interop with Bluebeam/Acrobat, no sidecar format.
- Shipping flow is `/sendit` (background pipeline agent, Forgejo REST, squash-merge). Background pipeline agents need `mode: "bypassPermissions"` or they stall on Bash. See `.claude/skills/sendit/SKILL.md`.
- (Add decisions here as made; log architectural ones via `kb_decision_create`.)

## Session Workflow
1. `/lamp-on` - load KB context
2. Work on current tasks (TDD - failing test first)
3. Ship via `/sendit` (`--dry-run` first if unsure); deep-review risky diffs (render path, markup serde, geometry) with `/code-review` BEFORE shipping
4. `/lamp-off` - save context before ending

## Workspace Standards
Follows Emittiv workspace standards. See `/Volumes/base/dev/.claude/WORKSPACE_STANDARDS.md`.
- Conventional commits with `Co-Authored-By`
- **TDD mandatory** for all code changes (failing test → implement → refactor)
- Credentials via `~/.claude/.credentials.env` with `_FROM` mapping - never hardcode
- Research before implementing (Context7 → existing code → official docs → ask)
- **Forge remotes MUST use SSH** (`git@ssh.forge.mms.name:emittiv/redline.git`)
- Styling via CSS custom properties / design tokens - no Tailwind (workspace-wide)
- Svelte 5 runes: `$state`, `$derived`, `$effect`
- Unraid operations (if any) route through the `unraid-ops` instance - not direct

## Precedent Projects
`e-fees` and `cad-export` are the workspace's existing Tauri 2 + Rust + Svelte desktop apps - mine them for IPC patterns, build config, and bundling before inventing new approaches.
