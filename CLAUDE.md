# redline

## Purpose
Cross-platform (Windows + macOS) desktop application for AEC PDF **markup, takeoff, and document management** — an internal replacement for Bluebeam Revu seats, optimized for very large construction plan sets. Zero license cost for v1; architecture kept clean and module-bounded so a future commercial release stays possible.

The authoritative v1 technical specification lives at `docs/bluebeam-alternative-v1-spec.md`. Read it first.

## Tech Stack
- **Shell:** Tauri 2.x (Rust core + OS webview)
- **Frontend:** Svelte 5 + Vite (SPA mode) + TypeScript
- **UI docking:** `dockview-core` (MIT); `svelte-splitpanes` as the lighter fallback
- **Render engine:** PDFium via `pdfium-render` 0.8.x (BSD) — display only
- **Low-level PDF ops:** `lopdf`
- **OCR:** Tesseract (Apache-2.0) via `leptess`
- **Full-text search:** Tantivy (MIT) — folder/library index
- **Doc-surgery backend:** trait-based, swappable — free baseline for v1; MuPDF/Apryse pluggable later
- **Targets:** Windows x64, macOS (universal)

## Architecture (Rust core modules)
`render` (tiled rasterization, display only) · `geometry` (vector path extraction + snap-target spatial index, PDF user space) · `document` (parse/model, page manipulation) · `text` (extraction + search) · `ocr` (invisible text layer) · `search` (Tantivy folder index) · `markup` (annotation model → standard PDF annotations) · `takeoff` (calibration, measurement, quantity calc, f64 user space) · `docops` *(swappable trait: flatten/optimize/redact)* · `compare` (Phase 1.1) · `storage` (local-first + version hooks).

**Precision-critical invariant:** display (raster tiles) and geometry (vector snap targets) are two independent layers. Snapping/measurement NEVER read the raster — all math runs in PDF user space at f64. See spec §5.

## Commands
```bash
# Dev
cargo tauri dev

# Build (release bundle)
cargo tauri build

# Rust tests
cargo test

# Lint / format
cargo clippy --all-targets && cargo fmt
# Frontend
npm run dev   # vite (if run standalone)
npm run check # svelte-check
```
*(Filled in concretely once the M1 shell scaffold lands.)*

## Build Order (milestones)
M1 shell + tiled render (large-file perf is the make-or-break test — validate on 300 MB+ sets early) → M2 markup + Tool Sets + `.btx` importer → M3 takeoff → M4 Sets/versioning + page ops + search/OCR → M5 `docops` baseline → M6 (Phase 1.1) compare. Full detail in spec §13.

## Key Decisions
- `.btx` Tool Set + stamp import is a **v1 requirement** (XML/UTF-8; reuses the annotation parser). Spec §6.
- Dynamic stamps: compose the appearance ourselves, NOT via embedded PDF JavaScript. Spec §6.
- Redaction v1 = rasterize-the-region safe floor; true vector redaction only via a mature engine behind the `docops` trait. Spec §8.
- Markup model carries reserved workflow fields (status/assignee/thread) + stable IDs from day one, for the future field-tools mobile app + async sync. Spec §6.
- Field tools deferred post-v1 as a standalone mobile/tablet app sharing the Rust core. Spec §2/§12.
- (Add decisions here as made; log architectural ones via `kb_decision_create`.)

## Session Workflow
1. `/lamp-on` — load KB context
2. Work on current tasks (TDD — failing test first)
3. `/lamp-off` — save context before ending

## Workspace Standards
Follows Emittiv workspace standards. See `/Volumes/base/dev/.claude/WORKSPACE_STANDARDS.md`.
- Conventional commits with `Co-Authored-By`
- **TDD mandatory** for all code changes (failing test → implement → refactor)
- Credentials via `~/.claude/.credentials.env` with `_FROM` mapping — never hardcode
- Research before implementing (Context7 → existing code → official docs → ask)
- **Forge remotes MUST use SSH** (`git@ssh.forge.mms.name:emittiv/redline.git`)
- Styling via CSS custom properties / design tokens — no Tailwind (workspace-wide)
- Svelte 5 runes: `$state`, `$derived`, `$effect`
- Unraid operations (if any) route through the `unraid-ops` instance — not direct

## Precedent Projects
`e-fees` and `cad-export` are the workspace's existing Tauri 2 + Rust + Svelte desktop apps — mine them for IPC patterns, build config, and bundling before inventing new approaches.
