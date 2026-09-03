#!/usr/bin/env bash
# Fetch eng.traineddata (Tesseract English language data) into
# src-tauri/resources/ocr/tessdata/ for bundling into the app (Tauri's
# `bundle.resources` config wholesale-maps `resources/` -> `$RESOURCE/resources/`,
# so nothing else needs to change once this file exists at build time - mirrors
# scripts/fetch-pdfium.sh's pattern). The file is gitignored.
#
# Usage:
#   scripts/fetch-ocr-tessdata.sh
#
# SOURCE CHOICE (deliberate, not the obvious "biggest/most accurate" pick):
# tesseract-ocr/tessdata_fast's eng.traineddata is BYTE-IDENTICAL (verified
# 2026-09-03 via sha256, see docs/ocr.md "Phase 2b") to the eng.traineddata
# Homebrew's `tesseract` formula installs on macOS dev machines - which is the
# exact file the Phase 2a accuracy benchmark (93%/98% baseline/rotate-4x
# recall, docs/ocr.md) was measured against. Bundling the standard `tessdata`
# or `tessdata_best` repo's eng.traineddata instead would ship a DIFFERENT,
# unbenchmarked model - so this pins to tessdata_fast specifically, not "the
# newest/largest available", to keep the shipped app's behavior consistent
# with what was actually measured.
#
# Pinned to a specific commit (not `main`, which can move) + sha256-verified.
# Bump TESSDATA_COMMIT + TESSDATA_SHA256 together to update; re-run the Phase
# 2a benchmark (src-tauri/tests/ocr_benchmark.rs) afterward, since a model
# swap can change recognition accuracy.
set -euo pipefail

TESSDATA_COMMIT="923915d4ced2a7235221788285785a29c4a42d4a"
TESSDATA_SHA256="7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2"
URL="https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/${TESSDATA_COMMIT}/eng.traineddata"

DEST_DIR="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/resources/ocr/tessdata"
DEST="${DEST_DIR}/eng.traineddata"

echo "Source: $URL"
echo "Dest:   $DEST"

mkdir -p "$DEST_DIR"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

echo "Downloading eng.traineddata …"
curl -fsSL --max-time 60 "$URL" -o "$TMP"

# macOS ships `shasum`, not `sha256sum`; Linux and Windows Git-Bash ship
# `sha256sum`, not `shasum` - this workflow runs on both GitHub-hosted macOS
# and Windows runners plus local macOS dev machines, so pick whichever exists.
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA256="$(sha256sum "$TMP" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL_SHA256="$(shasum -a 256 "$TMP" | cut -d' ' -f1)"
else
  echo "ERROR: neither sha256sum nor shasum found on PATH"
  exit 1
fi
if [ "$ACTUAL_SHA256" != "$TESSDATA_SHA256" ]; then
  echo "ERROR: sha256 mismatch."
  echo "  expected: $TESSDATA_SHA256"
  echo "  actual:   $ACTUAL_SHA256"
  exit 1
fi

cp "$TMP" "$DEST"
SIZE_DESC="$(du -h "$DEST" 2>/dev/null | cut -f1)"
echo "Installed: $DEST (${SIZE_DESC:-unknown size}, sha256 verified)"
