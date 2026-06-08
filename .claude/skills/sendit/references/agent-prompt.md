# Redline Sendit — Autonomous Background Pipeline

You are running the redline ship-it pipeline autonomously. No user interaction is available.
Execute every step. Log progress throughout. Deliver a complete report at the end.

**Project root:** `/Volumes/base/dev/claude/redline`
**All commands run from the project root unless otherwise stated.**
**Remote is Forgejo-only.** Use the Forgejo REST API with `$FORGEJO_ADMIN_TOKEN` for PR and
merge operations. Do NOT use the `gh` CLI (there is no GitHub remote).

**Token discipline:** Review from the DIFF only — never read whole source files. If you need
function context, read the cited lines (offset + limit). Budget: review ≤ 8K tokens, whole
pipeline ≤ 40K tokens.

---

## Step 0: Parse Args

Read the `## Args` section at the bottom. Extract:

- **COMMIT_TYPE_OVERRIDE**: `fix|feat|docs|chore|refactor|test` — or empty (auto-detect)
- **DRY_RUN**: true if `--dry-run` present
- **SKIP_REVIEW**: true if `--skip-review` present
- **NO_MERGE**: true if `--no-merge` present (stop after PR creation)
- **RELEASE**: true if `--release` present

In DRY_RUN: print every command with `[DRY-RUN]` prefix instead of executing. Read files normally.

**If RELEASE is true:** stop immediately with this message and exit 0:
```
/sendit --release is not active yet. The cross-platform build pipeline (macOS universal +
Windows x64) requires Forgejo Actions runners, per-OS PDFium bundling, code signing, a
Windows-verified bundle, and the definitive §20 verdict. See
.claude/skills/sendit/references/release-todo.md. Run /sendit without --release to land a branch.
```

---

## Step 1: Pre-Flight Checks

```bash
cd /Volumes/base/dev/claude/redline
echo "=== Pre-Flight ==="

BRANCH=$(git branch --show-current)
echo "Branch: $BRANCH"
if [ "$BRANCH" = "main" ]; then
  echo "PIPELINE STOPPED: on 'main'. /sendit lands a FEATURE BRANCH into main."
  echo "Create a branch first:  git switch -c feat/<name>"
  exit 1
fi

# Remote reachable
git ls-remote origin HEAD > /dev/null 2>&1 && echo "✓ origin (Forgejo) reachable" \
  || { echo "FAIL: origin not reachable"; exit 1; }

# Forgejo token present
source ~/.claude/.credentials.env 2>/dev/null
[ -n "$FORGEJO_ADMIN_TOKEN" ] && echo "✓ Forgejo token loaded" \
  || { echo "FAIL: FORGEJO_ADMIN_TOKEN missing from ~/.claude/.credentials.env"; exit 1; }
FORGEJO_API="${FORGEJO_HOST:-https://forge.mms.name}/api/v1"

# Toolchain
cargo --version > /dev/null 2>&1 && echo "✓ cargo available" || { echo "FAIL: cargo not found"; exit 1; }
node --version  > /dev/null 2>&1 && echo "✓ node available"  || { echo "FAIL: node not found"; exit 1; }

# Staged files
STAGED=$(git diff --cached --name-only)
echo ""; echo "Staged files:"
[ -z "$STAGED" ] && echo "  (nothing staged)" || echo "$STAGED" | sed 's/^/  /'
echo "=== Pre-Flight OK ==="
```

---

## Step 2: Commit Staged Changes

**Skip if nothing is staged.**

```bash
if [ -n "$STAGED" ]; then
  if [ -n "$COMMIT_TYPE_OVERRIDE" ]; then
    COMMIT_TYPE="$COMMIT_TYPE_OVERRIDE"
  else
    DF=$(git diff --cached --name-only)
    if   echo "$DF" | grep -qE '\.(test|spec)\.(ts|rs|js)$|tests?/'; then COMMIT_TYPE="test"
    elif echo "$DF" | grep -qE '(^docs?/|README|CHANGELOG|\.md$)'; then COMMIT_TYPE="docs"
    elif echo "$DF" | grep -qE '(Cargo\.toml|package\.json|tauri\.conf\.json|vite\.config|tsconfig|\.forgejo/)'; then COMMIT_TYPE="chore"
    elif echo "$DF" | grep -qE '\.(rs|svelte|ts|js)$'; then COMMIT_TYPE="feat"
    else COMMIT_TYPE="chore"; fi
  fi

  # Scope from most-changed area
  SCOPE="redline"
  DF=$(git diff --cached --name-only)
  echo "$DF" | grep -q "^src-tauri/src/render"   && SCOPE="render"
  echo "$DF" | grep -q "^src-tauri/src/geometry" && SCOPE="geometry"
  echo "$DF" | grep -q "^src-tauri/src/document" && SCOPE="document"
  echo "$DF" | grep -q "^src-tauri/src/markup"   && SCOPE="markup"
  echo "$DF" | grep -q "^src-tauri/src/takeoff"  && SCOPE="takeoff"
  echo "$DF" | grep -qE "^src/"                  && SCOPE="ui"

  N=$(echo "$DF" | wc -l | tr -d ' ')
  FIRST=$(echo "$DF" | head -1 | xargs basename 2>/dev/null || echo various)
  DESC="update $FIRST"; [ "$N" -gt 3 ] && DESC="update $SCOPE ($N files)"

  COMMIT_MSG="${COMMIT_TYPE}(${SCOPE}): ${DESC}

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
  echo "=== Commit ==="; echo "$COMMIT_MSG"
  if [ "$DRY_RUN" = "true" ]; then echo "[DRY-RUN] git commit"; else git commit -m "$COMMIT_MSG"; fi
fi
```

---

## Step 3: Review + Auto-Fix (single haiku pass, diff-only)

**Skip if `--skip-review`. In DRY_RUN print `[DRY-RUN]` and skip.**

```bash
echo ""; echo "=== Code Review ==="
if [ "$SKIP_REVIEW" = "true" ]; then REVIEW_VERDICT="skipped"; echo "(skipped)"
elif [ "$DRY_RUN" = "true" ]; then REVIEW_VERDICT="dry-run"; echo "[DRY-RUN] review HEAD~1..HEAD"
else
  DIFF=$(git diff origin/main...HEAD 2>/dev/null); [ -z "$DIFF" ] && DIFF=$(git diff HEAD~1 HEAD 2>/dev/null)
  if [ -z "$DIFF" ]; then REVIEW_VERDICT="skipped"; echo "(no diff)"; else
    DIFF_FILE=$(mktemp /tmp/redline-review-XXXXXX.diff); echo "$DIFF" > "$DIFF_FILE"
    echo "Spawning reviewer over $DIFF_FILE..."
  fi
fi
```

**Spawn the reviewer** (only when not DRY_RUN/skipped and a diff exists). Use the Agent tool
with the real `$DIFF_FILE` substituted:

```
Agent tool:
  subagent_type: "general-purpose"
  model: "haiku"
  run_in_background: false
  description: "Redline sendit review"
  prompt: |
    Fast code reviewer for redline (Tauri 2 + Svelte 5 runes + Rust core + PDFium/pdfium-render).
    Catch real bugs and security issues. Diff only. Under 8K output tokens. Do NOT read full files.

    ## Tech facts (do not research — use these)
    - PDFium global C state: tests serialise via --test-threads=1; production via RenderHandle (mpsc).
      Pdfium is !Send + !Sync. Flag any code that shares a Pdfium/PdfDocument across threads.
    - RenderEngine field drop order MUST be `documents` before `pdfium` (dylib owner) or SIGSEGV.
    - PDFium 2 GiB internal offset limit: >2 GiB files must be normalised (lopdf) before page-load.
    - Snap/measure math runs in PDF user space at f64 — NEVER read the raster for geometry (spec §5).
    - Svelte 5 runes: $state/$derived/$effect; $props() not export let; mount() not new App().
    - Tauri 2: snake_case Rust command args map to camelCase JS. invoke() for IPC.
    - CSS: no Tailwind — CSS custom properties / design tokens only.

    ## Check from diff ONLY
    CRITICAL (blocks): runtime-crash/data-loss logic errors; hardcoded secrets/tokens;
      cross-thread Pdfium sharing; broken render-engine drop order; reading raster for geometry math;
      breaking changes to a Tauri IPC command signature.
    WARNING (logged): missing error handling at boundaries; dbg!/println!/console.log in prod paths;
      TODO/FIXME in new code; unwrap() on fallible PDFium/IO paths.
    INFO: style notes.

    Read the diff from: <DIFF_FILE>

    ## Output (pipeline parses this)
    List findings as `CRITICAL: file:line desc` / `WARNING: ...` / `INFO: ...`.
    Last line MUST be exactly one of: VERDICT: PASS | VERDICT: WARN | VERDICT: BLOCK
```

Capture the result as `REVIEW_RESULT`, then:

```bash
echo "$REVIEW_RESULT"
REVIEW_VERDICT=$(echo "$REVIEW_RESULT" | grep '^VERDICT:' | tail -1 | awk '{print $2}')
rm -f "$DIFF_FILE"
case "$REVIEW_VERDICT" in
  PASS) echo "✓ review passed";;
  WARN) echo "⚠ review warnings — continuing";;
  BLOCK) echo "review BLOCK — attempting auto-fix (Step 3.5)";;
  *) echo "WARNING: unparsed verdict — continuing"; REVIEW_VERDICT="unknown";;
esac
```

### Step 3.5: Auto-Fix Loop (only if BLOCK)

Fix each CRITICAL finding yourself (read cited lines → minimal edit). Do NOT spawn another
agent. Then run the test gate (Step 4 commands). Tests pass → `git add -A && git commit -m
"fix: address review findings\n\nCo-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"`
and set `REVIEW_VERDICT=PASS`. Tests fail → `git checkout -- .`, print the CRITICAL findings,
`exit 1`.

---

## Step 4: Test Gate

Runs regardless of whether review was skipped. The corpus/PDFium tests are NOT run here
(machine-local gitignored corpus); the portable gate is:

```bash
echo ""; echo "=== Test Gate ==="
if [ "$DRY_RUN" = "true" ]; then echo "[DRY-RUN] cargo test / clippy / fmt --check / npm run check"; else
  cd /Volumes/base/dev/claude/redline

  echo "cargo test..."
  CARGO=$( (cd src-tauri && cargo test 2>&1) ); echo "$CARGO" | grep "test result:" | tail -3
  echo "$CARGO" | grep -q "test result: FAILED" && { echo "STOPPED: cargo test failed"; echo "$CARGO" | grep -E "FAILED|panicked|^error" | head -20; exit 1; }
  echo "✓ cargo test"

  echo "cargo clippy --all-targets..."
  # Filter the benign workspace "profiles for the non root package" notice — it is
  # not a code warning. Real code warnings/errors look like `warning: ...` / `error[...]`.
  CLIPPY=$( (cd src-tauri && cargo clippy --all-targets 2>&1) )
  CLIPPY_HITS=$(echo "$CLIPPY" | grep -E "^(warning|error)(\[|:)" | grep -v "profiles for the non root")
  [ -n "$CLIPPY_HITS" ] && { echo "STOPPED: clippy not clean"; echo "$CLIPPY_HITS" | head -20; exit 1; }
  echo "✓ clippy clean"

  echo "cargo fmt --check..."
  (cd src-tauri && cargo fmt --check) || { echo "STOPPED: cargo fmt --check failed. Run cargo fmt."; exit 1; }
  echo "✓ fmt clean"

  echo "npm run check..."
  CHECK=$(npm run check 2>&1); echo "$CHECK" | tail -6
  # svelte-check emits "<N> ERRORS <M> WARNINGS"; also catch raw "error TS" lines.
  ERRS=$(echo "$CHECK" | grep -oE '[0-9]+ ERRORS' | grep -oE '[0-9]+' | tail -1)
  if [ "${ERRS:-0}" -gt 0 ] || echo "$CHECK" | grep -q "error TS"; then
    echo "STOPPED: svelte-check reported ${ERRS:-?} errors"; exit 1
  fi
  echo "✓ svelte-check (${ERRS:-0} errors)"
  echo "=== Test Gate OK ==="
fi
```

---

## Step 5: Push Feature Branch

```bash
echo ""; echo "=== Push ==="
if [ "$DRY_RUN" = "true" ]; then echo "[DRY-RUN] git push -u origin $BRANCH"; else
  git push -u origin "$BRANCH" && echo "✓ pushed $BRANCH to Forgejo" || { echo "FAIL: push rejected"; exit 1; }
fi
```

---

## Step 6: Create PR (Forgejo REST API)

```bash
echo ""; echo "=== Create PR ==="
PR_TITLE="${COMMIT_TYPE:-chore}: $BRANCH"
PR_BODY="Automated /sendit pipeline.

- Review verdict: ${REVIEW_VERDICT:-skipped}
- Test gate: cargo test + clippy + fmt --check + svelte-check passed
- Branch: $BRANCH -> main

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"

if [ "$DRY_RUN" = "true" ]; then echo "[DRY-RUN] POST $FORGEJO_API/repos/emittiv/redline/pulls"; PR_NUMBER="DRY"; else
  PR_JSON=$(curl -sf -X POST "$FORGEJO_API/repos/emittiv/redline/pulls" \
    -H "Authorization: token $FORGEJO_ADMIN_TOKEN" -H "Content-Type: application/json" \
    -d "$(python3 -c 'import json,sys,os;print(json.dumps({"title":sys.argv[1],"body":sys.argv[2],"head":sys.argv[3],"base":"main"}))' "$PR_TITLE" "$PR_BODY" "$BRANCH")")
  PR_NUMBER=$(echo "$PR_JSON" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("number","ERR"))' 2>/dev/null)
  [ "$PR_NUMBER" = "ERR" ] || [ -z "$PR_NUMBER" ] && { echo "FAIL: PR creation"; echo "$PR_JSON" | head -c 500; exit 1; }
  echo "✓ PR #$PR_NUMBER  https://forge.mms.name/emittiv/redline/pulls/$PR_NUMBER"
fi
```

**Stop here if `--no-merge`.** Report the open PR URL and exit 0.

---

## Step 7: Squash-Merge + Cleanup

```bash
echo ""; echo "=== Merge ==="
if [ "$NO_MERGE" = "true" ]; then echo "(--no-merge: leaving PR #$PR_NUMBER open)"; 
elif [ "$DRY_RUN" = "true" ]; then echo "[DRY-RUN] merge PR squash + delete branch"; else
  MERGE=$(curl -sf -X POST "$FORGEJO_API/repos/emittiv/redline/pulls/$PR_NUMBER/merge" \
    -H "Authorization: token $FORGEJO_ADMIN_TOKEN" -H "Content-Type: application/json" \
    -d '{"Do":"squash"}' -w "%{http_code}" -o /tmp/redline-merge.out)
  if echo "$MERGE" | grep -qE "^(200|201)$"; then
    echo "✓ PR #$PR_NUMBER squash-merged"
    git push origin --delete "$BRANCH" 2>/dev/null && echo "✓ remote branch deleted"
    git checkout main && git pull origin main && echo "✓ local main synced"
    git branch -D "$BRANCH" 2>/dev/null && echo "✓ local branch removed"
  else
    echo "FAIL: merge HTTP $MERGE"; cat /tmp/redline-merge.out | head -c 500; exit 1
  fi
fi
```

---

## Step 8: KB Observation

```bash
if [ "$DRY_RUN" != "true" ] && [ "$NO_MERGE" != "true" ]; then
  source ~/.kb-agent.env 2>/dev/null || true
  if [ -n "$SURREALDB_URL" ] && [ -n "$SURREALDB_PASS" ]; then
    TS=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    curl -s -X POST "${SURREALDB_URL}/sql" -u "${SURREALDB_USER:-martin}:${SURREALDB_PASS}" \
      -H "surreal-ns: ${SURREALDB_NS:-kb}" -H "surreal-db: ${SURREALDB_DB:-knowledge}" \
      -H "Accept: application/json" -H "Content-Type: text/plain" \
      -d "INSERT INTO observation (entity_name, content, confidence, status, created_at) VALUES (
        'Redline Ship', 'Landed branch ${BRANCH} via /sendit (PR #${PR_NUMBER}). Review: ${REVIEW_VERDICT}. Gate: passed.',
        0.9, 'active', '${TS}');" > /dev/null 2>&1 && echo "✓ KB observation saved" || echo "(KB save skipped)"
  fi
fi
```

---

## Step 9: Final Report

```bash
echo ""; echo "================================================"
echo "  /sendit Complete — redline"
echo "================================================"
[ -n "$STAGED" ] && echo "  ✓ Committed: ${COMMIT_TYPE}(${SCOPE}) — $DESC" || echo "  - No staged changes"
echo "  ✓ Review: ${REVIEW_VERDICT:-skipped}"
echo "  ✓ Gate:   cargo test + clippy + fmt + svelte-check"
if [ "$NO_MERGE" = "true" ]; then
  echo "  ◦ PR #${PR_NUMBER} left OPEN (--no-merge): https://forge.mms.name/emittiv/redline/pulls/${PR_NUMBER}"
else
  echo "  ✓ Merged: PR #${PR_NUMBER} squashed into main, branches cleaned"
fi
echo ""
echo "  Release builds (macOS universal + Windows x64) are NOT part of this run."
echo "  They activate under /sendit --release once CI + signing exist — see"
echo "  .claude/skills/sendit/references/release-todo.md"
[ "$DRY_RUN" = "true" ] && echo "" && echo "  ↑ DRY-RUN — nothing changed."
echo "================================================"
```

---

## Args

(Populated by SKILL.md when spawning this agent — args follow below)
