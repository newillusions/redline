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
- **OCR:** deferred, not integrated (decided-but-unbuilt Tesseract via `leptess` - see "Deferred: OCR" under Key Decisions)
- **Full-text search:** Tantivy (MIT) - folder/library index
- **Doc-surgery backend:** trait-based, swappable - free baseline for v1; MuPDF/Apryse pluggable later
- **Targets:** Windows x64, macOS Apple Silicon only (Intel dropped per decision:boujy4d42i8w7zovifts, 2026-06-29)

## Architecture (Rust core modules)
`render` (tiled rasterization, display only) · `geometry` (vector path extraction + snap-target spatial index, PDF user space) · `document` (parse/model, page manipulation) · `text` (extraction + search) · `search` (Tantivy folder index) · `markup` (annotation model → standard PDF annotations) · `takeoff` (calibration, measurement, quantity calc, f64 user space) · `docops` *(swappable trait: flatten/optimize/redact)* · `compare` (Phase 1.1) · `storage` (local-first + version hooks).

No `ocr` module - deferred (see "Deferred: OCR" under Key Decisions); `pub mod ocr` and its stub file were removed 2026-08-04 rather than kept as inert scaffolding.

**Precision-critical invariant:** display (raster tiles) and geometry (vector snap targets) are two independent layers. Snapping/measurement NEVER read the raster - all math runs in PDF user space at f64. See spec §5.

## MCP Server

Full design: `docs/superpowers/specs/2026-09-01-mcp-server-design.md` - read it before touching `src-tauri/src/rpc/` or `src-tauri/src/bin/redline_mcp.rs`. Summary:

- **Architecture:** an embedded companion process, not a standalone file-mutating binary. The `redline` GUI app runs a loopback-only local RPC bridge (`src-tauri/src/rpc/`, a Unix domain socket on macOS/Linux at `$TMPDIR/redline-mcp/mcp.sock`, `0700`/`0600`-permissioned; a named pipe on Windows, unverified). `redline-mcp` (`src-tauri/src/bin/redline_mcp.rs`, a separate `[[bin]]` target) is the actual MCP stdio server Claude Code launches - a pure protocol translator with no PDF/Tauri logic of its own. Every tool call resolves to the exact same `commands::*`/`MarkupStore` functions the Svelte frontend already calls via `invoke` - single-writer correctness, no parallel mutation path.
- **30 tools defined, 29 advertised by default** (`redline_mcp.rs::tool_defs`/`tool_defs_for`): document lifecycle (`list_open_documents`, `open_document`, `close_document`, `get_active_document` - Phase 2a, 2026-09-03), read-only markup (`list_markups`, `read_markup`, `search_markups`, `export_markup_schedule`), mutating markup behind the lock guard (`create_markup`, `update_markup`, `delete_markup`, `save_document`), docops (`flatten_document`, `reduce_file_size`), and sixteen Phase 2b app-surface tools (2026-09-03, "start 2b"): search (`search_document`, `open_folder_index`, `search_folder`, `folder_index_status`), takeoff (`list_scales`, `add_scale`, `delete_scale`, `write_page_measure`, `export_markup_list`), page operations (`rotate_page`, `delete_page`, `reorder_pages`, `insert_blank_page`), compare (`compare_pages` - **opt-in only**, see below), docops (`redact_document`), and `save_document_as`.
- **`compare_pages` is disabled by default** (PR #99 review fix, 2026-09-03): it hangs indefinitely today due to a pre-existing double-PDFium-binding conflict between the render engine's binding and `pdf-diff`'s own separate one (see the design doc's Phase 2b "Fix round" section). Not listed in `tools/list`, and a direct call by name is refused with `experimental_tool_disabled` unless `REDLINE_MCP_EXPERIMENTAL=1` is set. Even when enabled, a hang now recovers client-side via the socket timeout below rather than wedging the process forever - but may still leak a stuck thread app-side, so treat it as genuinely experimental, not merely gated.
- **The MCP socket round trip has a read/write timeout** (`redline_mcp.rs::socket_timeout`, default 120s, override `REDLINE_MCP_TIMEOUT_SECS`) - `redline-mcp`'s own loop is single-threaded/synchronous, so before this fix one stuck server-side call wedged the entire client process silently for every later call too. A timeout now surfaces as a structured `redline_timeout` tool error.
- **Every tool but document-discovery/folder-search/compare requires `doc_id`** - obtain one via `list_open_documents` or `get_active_document` first, or via `open_document`'s own return value. The doc_id-free exceptions and why: `list_open_documents`/`get_active_document`/`open_document` (Phase 2a, discovery itself), `open_folder_index`/`search_folder`/`folder_index_status` (Phase 2b, keyed by a folder `root` instead - redline holds exactly one active folder index at a time, `AppState.folder_index: Mutex<Option<FolderIndex>>`), and `compare_pages` (Phase 2b, keyed by raw `path_a`/`path_b` - it wraps a Tauri command that never touches `AppState`/`MarkupStore` and doesn't require either document open).
- **Most Phase 2b writes are NOT staged in memory and are NOT gated behind `save_document`**, unlike markup create/update/delete. `add_scale`/`delete_scale` write immediately to the sidecar metadata file; `write_page_measure`/`rotate_page`/`delete_page`/`reorder_pages`/`insert_blank_page`/`redact_document` all share `commands::document::apply_page_edit` and write the PDF file on disk immediately (atomic temp+rename), reloading the render engine before the command returns. No document-level lock concept exists anywhere in this codebase to check before allowing one of these - only the per-markup `Locked`/`LockedContents` flags from the lock guard above, which apply to individual annotations, not page structure. Each tool's own MCP `description` states this plainly. Full mutates/persists table: design doc's "Implementation notes (Phase 2b...)" section.
- **Lock guard:** `Markup::is_locked`/`is_contents_locked` (ISO 32000-1 `/F` bits 8/10) are enforced once, inside `document::store::MarkupStore::update`/`delete` - the single choke point both the GUI's own `commands::document::update_markup`/`delete_markup` and the MCP bridge's `update_markup`/`delete_markup` tools call through.
- **Active-tab/dirty tracking has no other source of truth than this MCP work.** `AppState.active_doc` is pushed from a single `$effect` in `App.svelte` watching `tabStore.activeDocId` - if you touch tab-switching logic, verify that effect still fires (see its comment). `dirty` is derived server-side from `MarkupStore::add`/`update`/`delete` and cleared by `save_inner`/`apply_page_edit` - no frontend plumbing needed for it, don't add any.
- **Testing an MCP change live:** never point a dev build at the default `$TMPDIR` while the installed app (or another dev session) might be running - it shares the same fixed socket path (single-instance assumption, named in the design doc). Run with an isolated, SHORT `TMPDIR` instead (macOS Unix-socket paths are capped at ~104 bytes - `SUN_LEN` - so a deeply-nested scratch path will fail to bind with a cryptic error), e.g. `TMPDIR=/tmp/rlmcp-scratch/ REDLINE_OPEN_PDF=/path/to/scratch.pdf npm run tauri:dev -- --no-watch`, then run `TMPDIR=/tmp/rlmcp-scratch/ target/debug/redline-mcp` with JSON-RPC lines on stdin.

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

## Releasing (desktop, auto-updater)

Windows + macOS installers build on **GitHub-hosted runners** (`.github/workflows/build-releases.yml`); the Forgejo self-hosted runner is Linux-only and cannot produce them. Two git remotes, both configured: `origin` (Forgejo, source of truth + container/CI) and `github` (`newillusions/redline`, the desktop build runner). A release fires on a `v*` tag pushed to **GitHub** (the tag drives the manifest version and artifact filenames; the `update-manifest` job commits `update.json` back to GitHub `main`).

**Bump the source version BEFORE tagging - this is load-bearing.** The workflow derives `update.json`'s advertised version and the artifact filenames from the git tag, but the built app reports its *own* version from the source fields. If they disagree, the in-app updater offers the "new" version on every launch forever (installed `<` advertised, but installing never changes the installed number). Incident 2026-07-11: v0.3.2 was tagged on a docs commit with the source still at 0.3.1 → infinite update loop; fixed by shipping 0.3.3 with the bump. (obs:uxwsluoras8c1hir86wx)

Release steps:
1. Bump **all** version carriers to the target: `src-tauri/tauri.conf.json` (`version`), `src-tauri/Cargo.toml` (`[package] version`), `package.json` (`version`), then `cargo update -p redline --precise <ver>` to sync `Cargo.lock`. Commit (`chore(release): bump version to <ver>`).
2. Tag `v<ver>` on the bump commit and push the tag to **both** remotes (`git push origin v<ver>` and `git push github v<ver>`). The `github` push triggers the build.
3. After the build: verify not just `update.json`'s `version` field but the **built artifact** - decode a platform signature's trusted-comment (`file:<name>`); it embeds the real built filename and reveals a version mismatch the manifest field hides.
4. Re-releasing under a fresh number (e.g. burn a bad tag, ship the next patch) is cleaner than force-rewriting a tag when an auto-updater is in play.

## Build Order (milestones)
M1 shell + tiled render (large-file perf is the make-or-break test - validate on 300 MB+ sets early) → M2 markup + Tool Sets + `.btx` importer → M3 takeoff → M4 Sets/versioning + page ops + search (OCR deferred, see below) → M5 `docops` baseline → M6 (Phase 1.1) compare. Full detail in spec §13.

**Current phase (2026-07-02): M1-M6 + Phase 1.1 all shipped to `main`, 0 open PRs.** Work since has been small polish/fix PRs on the takeoff + markup panels (see git log). Two verification gates remain owed, not yet closed:
- **§20 definitive floor-machine run** (16 GB, Windows + macOS) - the formal M1 Go/No-Go. The current §20 verdict is only the *indicative* Apple-Silicon/headless pass; blocked on hardware access. Do not represent M1 as formally gated-through until this runs. `bench/RUNBOOK-S20.md`.
- **G9 human visual check** - open a sample PDF in Acrobat/Bluebeam to confirm font + group rendering interop (owed since M2, 2026-06-16).

One M4 scope item was NOT actually shipped despite the above: **OCR** was decided (leptess/Tesseract) but never built - see "Deferred: OCR" under Key Decisions. Everything else M4 named (Sets/versioning, page ops, in-doc/Set text search, folder full-text index) is real and tested.

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
- **Deferred: OCR** (investigated + descoped 2026-08-04, wave-2 portfolio review). Tesseract via `leptess` was decided for M4 (decision:tntyyjau94smf6r6jitq, 2026-06-25) but never built - `ocr/mod.rs` had sat as a 9-line stub with the dependency and Cargo `ocr` feature both commented out since M1, and the M4/"shipped" claims above never flagged the gap. Removed the stub module rather than leave it as dead scaffolding (`pub mod ocr` deleted from `lib.rs`, `src/ocr/` deleted). NOT a "not needed" call - it's real spec-mandated v1 scope (spec §14, §219) with no engineering blocker on the code itself, but three real release-pipeline blockers found this session, none yet addressed:
  1. Forgejo CI (`.forgejo/workflows/ci.yml`, `ubuntu-latest`) has no `tesseract-ocr`/`libtesseract-dev`/`libleptonica-dev` apt step - `--features ocr` wouldn't compile in CI today (cheapest to fix - plain apt packages on Linux).
  2. `.github/workflows/build-releases.yml`'s `build-macos` job has no `brew install tesseract` (or vendored equivalent) step.
  3. `build-windows` has no vcpkg/tesseract bootstrap at all - `leptess` on Windows is the hard case (static-vs-dynamic linking, `VCPKG_ROOT`/`TESSDATA_PREFIX` env wiring), unverified in this repo.
  Also unstaged: `eng.traineddata` (~12MB tessdata) isn't in `tauri.bundle.resources` anywhere. Re-enabling needs all four addressed IN ORDER (Linux CI first, cheapest signal) - each verified by an actual green CI/release run, not a local compile, since the whole point of the gap is that a local Linux build can look fine while both release legs are broken. See the `Cargo.toml` comment above the commented-out `leptess` line for the same detail inline with the code.
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
