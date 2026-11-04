#!/usr/bin/env bash
# Run every tools/gui-harness*.mjs headless against a Vite dev server on :1421 (mock Tauri
# IPC, no native app). Needs Chromium for Playwright: `npx playwright install chromium`.
# The server is stopped by PID on exit. Exits non-zero if any harness fails.
set -u
cd "$(dirname "$0")/.."
LOG="${TMPDIR:-/tmp}/redline-harness-vite.log"
npx vite --port 1421 --strictPort >"$LOG" 2>&1 &
VITE_PID=$!
trap 'kill "$VITE_PID" 2>/dev/null' EXIT
for _ in $(seq 1 60); do
  curl -fs --max-time 2 http://localhost:1421/ >/dev/null 2>&1 && break
  kill -0 "$VITE_PID" 2>/dev/null || { echo "vite exited early:"; cat "$LOG"; exit 1; }
  sleep 1
done
export OUT="${OUT:-${TMPDIR:-/tmp}}"
failed=0
for f in tools/gui-harness*.mjs; do
  echo "=== $f"
  if ! timeout 240 node "$f"; then echo "HARNESS FAILED: $f"; failed=1; fi
done
exit "$failed"
