#!/usr/bin/env bash
# Generate a SELF-SIGNED Authenticode code-signing certificate for Redline
# Windows releases (macOS/openssl). This is the "internal tooling" tier of
# code signing: it stops AV heuristics (Norton/AVG IDP.HELU etc.) that flag
# *unsigned* binaries, and gives tamper-evidence, but does NOT get Redline a
# publicly-trusted publisher chain - that requires a paid OV/EV certificate
# from a CA (DigiCert, Sectigo, ...), out of scope here. See
# docs/windows-code-signing.md for the OV-cert upgrade path if redline is
# ever distributed externally.
#
# Usage:
#   scripts/gen-signing-cert.sh
#   scripts/gen-signing-cert.sh --cn "Emittiv L.L.C-FZ" --days 3650 --out ~/.redline-signing
#
# Output (written OUTSIDE the repo by default - never commit these files):
#   <out>/redline-codesign.key   - private key (PEM, keep offline/secret)
#   <out>/redline-codesign.pem   - self-signed cert (PEM)
#   <out>/redline-codesign.cer   - self-signed cert (DER, for Windows import)
#   <out>/redline-codesign.pfx   - PKCS#12 bundle (key+cert) for CI signing
#
# This script does NOT write secrets into the repo or into creds.env. It is
# the owner's responsibility (owner-gated materialization) to:
#   1. Add the printed base64 .pfx + the chosen password as CI secrets
#      (WINDOWS_SIGN_CERT_B64, WINDOWS_SIGN_CERT_PASSWORD) on the
#      newillusions/redline GitHub repo.
#   2. Import <out>/redline-codesign.cer into "Trusted Root Certification
#      Authorities" AND "Trusted Publishers" (Local Machine) on every Windows
#      machine that should trust Redline installers without a SmartScreen/AV
#      warning.
set -euo pipefail

CN="Emittiv L.L.C-FZ"
DAYS=3650
OUT_DIR="${HOME}/.redline-signing"
PASSWORD=""

while [ $# -gt 0 ]; do
  case "$1" in
    --cn) CN="$2"; shift 2 ;;
    --days) DAYS="$2"; shift 2 ;;
    --out) OUT_DIR="$2"; shift 2 ;;
    --password) PASSWORD="$2"; shift 2 ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

if ! command -v openssl >/dev/null 2>&1; then
  echo "ERROR: openssl not found on PATH." >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
chmod 700 "$OUT_DIR"

KEY_PATH="$OUT_DIR/redline-codesign.key"
PEM_PATH="$OUT_DIR/redline-codesign.pem"
CER_PATH="$OUT_DIR/redline-codesign.cer"
PFX_PATH="$OUT_DIR/redline-codesign.pfx"

if [ -f "$PFX_PATH" ]; then
  echo "ERROR: $PFX_PATH already exists. Remove it first if you intend to regenerate" >&2
  echo "(regenerating rotates the signing identity - every machine's Trusted Root" >&2
  echo "import + the CI secrets must be updated together, or old + new installers" >&2
  echo "will show mismatched publishers)." >&2
  exit 1
fi

if [ -z "$PASSWORD" ]; then
  # Random 32-char password; printed once below, never written to a file.
  PASSWORD="$(openssl rand -base64 24 | tr -d '=+/\n' | cut -c1-32)"
  GENERATED_PASSWORD=1
else
  GENERATED_PASSWORD=0
fi

echo "Generating self-signed code-signing certificate..."
echo "  CN:        $CN"
echo "  Validity:  $DAYS days"
echo "  Output:    $OUT_DIR"
echo

# Single-command self-signed leaf cert with the codeSigning EKU (openssl 1.1.1+/3.x
# supports -addext on `req -x509` directly - no separate CSR step needed).
openssl req -x509 -newkey rsa:4096 -sha256 -nodes \
  -keyout "$KEY_PATH" \
  -out "$PEM_PATH" \
  -days "$DAYS" \
  -subj "/CN=${CN}/O=${CN}" \
  -addext "extendedKeyUsage=critical,codeSigning" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "basicConstraints=critical,CA:FALSE"

chmod 600 "$KEY_PATH"

# DER cert for Windows certmgr / double-click import.
openssl x509 -in "$PEM_PATH" -outform DER -out "$CER_PATH"

# PKCS#12 bundle for CI (osslsigncode -pkcs12).
openssl pkcs12 -export \
  -inkey "$KEY_PATH" \
  -in "$PEM_PATH" \
  -out "$PFX_PATH" \
  -passout "pass:${PASSWORD}"
chmod 600 "$PFX_PATH"

echo "Certificate generated. Verifying codeSigning EKU:"
echo "----------------------------------------------------------------"
openssl x509 -in "$PEM_PATH" -noout -text | grep -A1 "Extended Key Usage"
echo "----------------------------------------------------------------"
echo

PFX_B64="$(base64 < "$PFX_PATH" | tr -d '\n')"

echo "======================================================================"
echo " PFX (base64) - copy this into the GitHub secret WINDOWS_SIGN_CERT_B64"
echo "======================================================================"
echo "$PFX_B64"
echo
echo "======================================================================"
if [ "$GENERATED_PASSWORD" = "1" ]; then
  echo " Generated PFX password (SAVE NOW - not stored anywhere by this script):"
  echo "   $PASSWORD"
else
  echo " PFX password: the one you passed via --password."
fi
echo "======================================================================"
echo
cat <<'EOF'
OWNER ACTION REQUIRED (this script does not touch CI or Windows machines):

  1. GitHub repo secrets - newillusions/redline > Settings > Secrets and
     variables > Actions:
       WINDOWS_SIGN_CERT_B64      = the base64 blob printed above
       WINDOWS_SIGN_CERT_PASSWORD = the PFX password printed above

  2. Import the certificate on every Windows machine that should trust
     Redline installers without a SmartScreen/AV warning:
       - Double-click the .cer file (path printed below), or run:
           certutil -addstore -f "Root" <path-to>.cer
           certutil -addstore -f "TrustedPublisher" <path-to>.cer
       - Import into BOTH "Trusted Root Certification Authorities" and
         "Trusted Publishers" (Local Machine store) - a self-signed leaf
         needs to be in Trusted Root because it is its own issuer.

  3. Keep the .pfx and its password OFFLINE once the CI secret is set
     (e.g. delete OUT_DIR, or move it to a password manager). This script
     never wrote them into the repo or into creds.env - that boundary is
     yours to keep from here.
EOF
echo
echo "Files written to $OUT_DIR (NOT committed, NOT in the repo tree):"
echo "  $KEY_PATH"
echo "  $PEM_PATH"
echo "  $CER_PATH   <- import this one on Windows"
echo "  $PFX_PATH   <- base64 of this one is the CI secret"
