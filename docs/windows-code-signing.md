# Windows code signing

Redline's Windows NSIS installer is Authenticode-signed in CI with a
**self-signed code-signing certificate**. This is the internal-tooling tier
of signing: it stops AV heuristics (Norton/AVG `IDP.HELU` and similar) that
flag *unsigned* binaries, and gives tamper-evidence on the installer, but it
does **not** get Redline a publicly-trusted publisher chain - a machine that
has never imported the certificate will still show an "Unknown Publisher"
SmartScreen prompt. See [Future: a real OV/EV certificate](#future-a-real-ovev-certificate)
if redline is ever distributed outside machines we control.

## How it works

1. `scripts/gen-signing-cert.sh` generates the self-signed certificate
   locally (macOS/openssl) and prints its base64-encoded `.pfx`.
2. The owner adds that base64 blob + its password as two GitHub Actions
   secrets on `newillusions/redline`.
3. The `Build Windows` job in
   [`.github/workflows/build-releases.yml`](../.github/workflows/build-releases.yml)
   signs the NSIS installer with `osslsigncode` right after
   `npm run tauri:build` produces it, using those secrets.
4. Because Tauri's updater plugin also computes a minisign signature over
   the installer as part of that same `tauri:build` step (before
   Authenticode signing touches the file), the workflow regenerates that
   minisign signature immediately after Authenticode-signing, via
   `tauri signer sign`. This keeps the two signatures in the correct order:
   **Authenticode signing happens first, minisign signing happens on the
   final (already-Authenticode-signed) bytes.** If it were the other way
   around, Authenticode signing would change the file's bytes after
   minisign had already hashed it, and the auto-updater would reject every
   signed release as corrupt.
5. Both new steps are guarded on the two CI secrets being present
   (`Check Windows signing secrets` step) and no-op cleanly when they are
   not - a build without the cert configured still produces a working,
   just-unsigned installer.

## Owner steps (one-time)

### 1. Generate the certificate

```
scripts/gen-signing-cert.sh
```

Run this on a Mac with `openssl` available. It writes the key/cert/pfx to
`~/.redline-signing/` (outside the repo, never committed) and prints:

- the base64 of the `.pfx` file
- the PFX password (random, unless you pass `--password`)

### 2. Add the two CI secrets

On `newillusions/redline` -> **Settings -> Secrets and variables -> Actions**,
add:

| Secret name | Value |
|---|---|
| `WINDOWS_SIGN_CERT_B64` | the base64 blob the script printed |
| `WINDOWS_SIGN_CERT_PASSWORD` | the PFX password the script printed |

### 3. Import the certificate on Windows machines

On every Windows machine that should trust Redline installers without a
SmartScreen/AV warning (both dev/test boxes), import
`~/.redline-signing/redline-codesign.cer`:

```powershell
certutil -addstore -f "Root" redline-codesign.cer
certutil -addstore -f "TrustedPublisher" redline-codesign.cer
```

Or double-click the `.cer` file and use the Certificate Import Wizard,
placing it in both **Trusted Root Certification Authorities** and
**Trusted Publishers** (Local Machine store). A self-signed leaf certificate
needs to be in Trusted Root because it is its own issuer - there is no
separate CA to anchor trust to.

## Verifying a signed installer

On Windows, after a release build:

```powershell
Get-AuthenticodeSignature .\Redline_<version>_x64-setup.exe | Format-List

# or, using the classic tool:
signtool verify /pa /v .\Redline_<version>_x64-setup.exe
```

`Get-AuthenticodeSignature` should report `Status: Valid` once the
certificate is imported into Trusted Root + Trusted Publishers on that
machine (it will show `UnknownError` / untrusted on a machine that hasn't
imported it - expected for a self-signed cert).

To confirm the auto-updater still accepts the signed installer, check that
`update.json` (committed to `main` by the `update-manifest` job) contains a
non-empty `signature` field for `windows-x86_64` and that an existing
Redline install picks up the new version via the in-app updater.

## Scope note: the installer is signed, not the inner app binary

This pass signs the outer NSIS installer executable (`*-setup.exe`) - the
file that gets downloaded and that AV/SmartScreen scans on first run, and
the file the Tauri auto-updater fetches and verifies. It does **not**
separately re-sign the `redline.exe` binary that the installer extracts into
`Program Files` at install time, because that binary is built and bundled
into the NSIS payload inside the single `tauri:build` step, before this
workflow gets a chance to intervene. Signing it too would require moving to
Tauri's native `bundle.windows.signCommand` hook (which runs signing inside
the bundler pipeline, before both NSIS packaging and minisign) - a larger
change than this pass, left as a follow-up if AV/SmartScreen still flags the
*installed* application after this change ships.

## Future: a real OV/EV certificate

If Redline is ever distributed to people outside machines we control
(public releases, non-Emittiv users), replace the self-signed certificate
with a paid Organization Validation (OV) or Extended Validation (EV)
certificate from a CA such as DigiCert or Sectigo. That gets a chain any
Windows machine trusts out of the box (no per-machine import step) and, for
EV certs, builds Microsoft SmartScreen reputation faster. The CI mechanics
(`osslsigncode`, the two secrets, the signing step in
`build-releases.yml`) stay the same - only the certificate source changes.
Out of scope for this pass.
