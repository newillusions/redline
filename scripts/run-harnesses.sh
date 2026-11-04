#!/usr/bin/env bash
# Run every tools/gui-harness*.mjs headless against a Vite dev server on :1421 (mock Tauri
# IPC, no native app). Needs Chromium for Playwright: `npx playwright install chromium`.
# The server runs in its own process group (job control) and the whole group is stopped on
# exit - killing only the `npx` PID left the node/vite child holding :1421. Exits non-zero
# if any harness fails.
set -u
set -m
cd "$(dirname "$0")/.."
LOG="${TMPDIR:-/tmp}/redline-harness-vite.log"
npx vite --port 1421 --strictPort >"$LOG" 2>&1 &
VITE_PID=$!
# With job control the background job is its own group leader: PGID == VITE_PID.
stop_vite() { kill -- "-$VITE_PID" 2>/dev/null; wait "$VITE_PID" 2>/dev/null; }
trap stop_vite EXIT
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
