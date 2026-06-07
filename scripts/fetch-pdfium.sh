#!/usr/bin/env bash
# Fetch the prebuilt PDFium shared library into src-tauri/resources/ for the
# current platform (or one named via $1). The binary is gitignored and bundled
# into the app by Tauri (tauri.conf.json bundle.resources).
#
# Usage:
#   scripts/fetch-pdfium.sh            # auto-detect host platform
#   scripts/fetch-pdfium.sh mac-arm64  # force a specific target
#   scripts/fetch-pdfium.sh mac-x64
#   scripts/fetch-pdfium.sh win-x64    # downloads pdfium.dll (run on/for Windows builds)
#   scripts/fetch-pdfium.sh linux-x64
#
# Pinned to a known-good release tag (chromium/7869). Bump PDFIUM_TAG to update;
# re-run the §20 bench afterward (a PDFium bump can change render perf/behaviour).
set -euo pipefail

PDFIUM_TAG="chromium/7869"
REPO="bblanchon/pdfium-binaries"
DEST="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/resources"

detect_target() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  case "$os" in
    Darwin) [ "$arch" = "arm64" ] && echo "mac-arm64" || echo "mac-x64" ;;
    Linux)  echo "linux-x64" ;;
    MINGW*|MSYS*|CYGWIN*) echo "win-x64" ;;
    *) echo "unknown" ;;
  esac
}

TARGET="${1:-$(detect_target)}"
case "$TARGET" in
  mac-arm64|mac-x64) LIBNAME="libpdfium.dylib"; LIBSUBPATH="lib/libpdfium.dylib" ;;
  linux-x64)         LIBNAME="libpdfium.so";    LIBSUBPATH="lib/libpdfium.so" ;;
  win-x64)           LIBNAME="pdfium.dll";      LIBSUBPATH="bin/pdfium.dll" ;;
  *) echo "Unknown target: $TARGET (expected mac-arm64|mac-x64|linux-x64|win-x64)"; exit 2 ;;
esac

ASSET="pdfium-${TARGET}.tgz"
# URL-encode the slash in the tag.
TAG_ENC="${PDFIUM_TAG/\//%2F}"
URL="https://github.com/${REPO}/releases/download/${TAG_ENC}/${ASSET}"

echo "Target:   $TARGET"
echo "Asset:    $ASSET ($PDFIUM_TAG)"
echo "Dest:     $DEST/$LIBNAME"

mkdir -p "$DEST"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading $URL …"
curl -fsSL "$URL" -o "$TMP/pdfium.tgz"
tar -xzf "$TMP/pdfium.tgz" -C "$TMP"

if [ ! -f "$TMP/$LIBSUBPATH" ]; then
  echo "ERROR: expected $LIBSUBPATH not found in archive. Contents:"
  find "$TMP" -name "*.dylib" -o -name "*.so" -o -name "*.dll" | sed 's/^/  /'
  exit 1
fi

cp "$TMP/$LIBSUBPATH" "$DEST/$LIBNAME"

# Strip macOS quarantine so the dylib loads without Gatekeeper prompts.
if [ "$TARGET" = "mac-arm64" ] || [ "$TARGET" = "mac-x64" ]; then
  xattr -d com.apple.quarantine "$DEST/$LIBNAME" 2>/dev/null || true
fi

echo "Installed: $DEST/$LIBNAME ($(du -h "$DEST/$LIBNAME" | cut -f1))"
echo "Done. The bundled app resolves this automatically; for the headless bench set:"
echo "  export PDFIUM_DYNAMIC_LIB_PATH=\"$DEST/$LIBNAME\""
