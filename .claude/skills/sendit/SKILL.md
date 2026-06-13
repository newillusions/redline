---
name: sendit
description: Use when ready to ship a redline change — runs quality-gate → commit → push → PR → review → squash-merge autonomously in the background, Forgejo-only. The core flow lands a feature branch on main. The cross-platform release build (macOS universal + Windows x64) is a separate --release extension that is NOT active yet (no CI runners). Run /sendit --dry-run first to preview.
---

# /sendit — Redline Ship It Pipeline

Lands the current feature branch on `main` via Forgejo: quality gate → commit → push →
PR → review → squash-merge → cleanup. Runs autonomously in a background agent so the main
instance stays free. A final report is delivered on completion.

**Forgejo-only.** Redline has a single remote (`origin` → `forge.mms.name/emittiv/redline`).
There is no GitHub mirror and no CI yet. PRs and merges go through the Forgejo REST API
using `$FORGEJO_ADMIN_TOKEN` from `~/.claude/.credentials.env` — never the `gh` CLI.

## Scope: what this does NOT do yet

The **cross-platform release build** (macOS universal + Windows x64 bundles, signing,
release assets) is modeled on e-fees's tag→CI pipeline but is **not wired in**. It activates
under a future `--release` flag once ALL of these exist (see `references/release-todo.md`):

1. Forgejo Actions runners for macOS **and** Windows
2. A build workflow that runs `scripts/fetch-pdfium.sh <target>` per-OS and bundles `pdfium`
3. Code signing (macOS notarization, Windows Authenticode)
4. The Windows bundle path verified on a real Windows box (currently wired-but-untested)
5. Definitive §20 floor-machine verdict passed (the M1 gate)

Until then `/sendit` stops after squash-merge. It will print a reminder that release builds
are pending. Do not fake a release.

## Arguments

```
/sendit                  — Land current branch: quality gate → commit → push → PR → merge
/sendit fix|feat|docs|chore|refactor|test
                         — Override the conventional-commit type for any staged commit
/sendit --dry-run        — Print every step with [DRY-RUN], change nothing (safe anytime)
/sendit --skip-review    — Skip the diff review pass (trivial/docs changes only)
/sendit --no-merge       — Stop after creating the PR (leave it open for human merge)
/sendit --release        — RESERVED. Errors out until the release prerequisites above exist.
```

Arguments combine: `/sendit fix --no-merge`.

## Pre-conditions

- Be on a **feature branch**, not `main`. If on `main` with changes, the pipeline stops and
  tells you to branch first (it will not push commits straight to `main`).
- Corpus-dependent PDFium tests (`REDLINE_BENCH_TESTS=1`, `--test-threads=1`) need the
  machine-local gitignored corpus and are **not** run by the gate — run those manually
  before shipping render-path changes. The gate runs the portable tests only.
- The pipeline's internal review is a **single haiku smoke pass**, diff-only. For risky
  diffs (render path, markup model/serde, geometry, takeoff math) run `/code-review`
  (high effort) on the branch BEFORE invoking /sendit — treat the pipeline review as a
  final gate, not the review.

## How It Works

When `/sendit` (or `/sendit <args>`) is invoked:

1. Read `.claude/skills/sendit/references/agent-prompt.md` for the full pipeline prompt.
2. Extract args from the invocation.
3. Spawn a **background Task agent**:

```
Agent tool:
  description: "Redline sendit pipeline"
  subagent_type: "general-purpose"
  model: "sonnet"
  run_in_background: true
  mode: "bypassPermissions"
  prompt: [agent-prompt.md content] + "\n\n## Args\n" + [parsed args or "none"]
```

> `mode: "bypassPermissions"` is REQUIRED — the pipeline is entirely git/cargo/npm/curl
> shell work, and a background agent spawned with `mode: "auto"` is NOT granted Bash and
> stalls at pre-flight (observed on the first dogfood, 2026-06-08). The agent still has the
> pipeline's own guards (gate must pass, stop-before-push on anything wrong), so the merge
> is gated on real green checks, not on permission prompts.

4. Tell the user: "Pipeline running in background — I'll report when it completes."
5. Continue with other work.

## Pipeline Flow (core, active now)

```
Pre-flight (branch != main, remote reachable, Forgejo token, node/cargo)
  → Commit staged changes (conventional commit, if any)
  → Review (single haiku pass, diff-only)
      → BLOCK: auto-fix → test gate → pass? commit fix : revert + STOP
      → WARN/PASS: continue
  → Test Gate: cargo test + cargo clippy --all-targets (0 warnings)
               + cargo fmt --check + npm run check
  → Push feature branch to Forgejo (origin)
  → Create PR via Forgejo REST API (base: main)
  → Squash-merge via Forgejo REST API   [skipped if --no-merge]
  → Delete remote branch + local sync to main
  → KB observation
  → Final report
```

## Key Paths (Hardcoded)

| Item | Value |
|------|-------|
| Project root | `/Volumes/base/dev/claude/redline` |
| Remote | `origin` → `git@ssh.forge.mms.name:emittiv/redline.git` |
| Forgejo repo (API) | `emittiv/redline` |
| Forgejo API base | `https://forge.mms.name/api/v1` |
| Auth | `$FORGEJO_ADMIN_TOKEN` from `~/.claude/.credentials.env` |
| Base branch | `main` |
| Version sync script | `scripts/sync-version.cjs` (version-only, 3 files) |
| Version source of truth | `package.json` |
| Test gate | `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check`, `npm run check` |
| Corpus tests (manual) | `REDLINE_BENCH_TESTS=1 cargo test --release -- --test-threads=1` |
| Release TODO | `.claude/skills/sendit/references/release-todo.md` |
