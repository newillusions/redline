# /sendit --release — cross-platform build, prerequisites

The release flag is intentionally inert. It produces the shippable artifacts — **macOS
universal + Windows x64 installers** — and that requires infrastructure redline does not have
yet. This file is the checklist for wiring it in. Update it as items land; flip the flag on
in `agent-prompt.md` Step 0 only when all of 1–5 are true.

Model the release pipeline on **e-fees** (`/Volumes/base/dev/claude/e-fees/.claude/skills/
sendit/references/agent-prompt.md`, Steps 8–11): version bump → tag → CI build matrix → poll
→ verify release assets. e-fees builds the binaries in CI from a pushed tag — do the same.
Do NOT try to cross-compile Windows bundles on the Mac mini.

## Prerequisites (all required)

1. **Forgejo Actions runners for macOS AND Windows.**
   redline has no `.forgejo/workflows/` yet. Need a build workflow triggered on `v*` tags,
   with a job per target OS, on runners that can build Tauri. (`/ci-status` checks runner health.)

2. **Per-OS PDFium bundling in CI.**
   The workflow must run `scripts/fetch-pdfium.sh <target>` for each OS (pinned chromium/7869)
   and place the binary where `resolve_pdfium_path` finds it (resource dir → exe dir). The
   `tauri.conf.json` `bundle.resources` mapping is already wired. Verify the resource lands in
   the bundle on each OS.

3. **Code signing.**
   - macOS: Developer ID cert + notarization (`APPLE_SIGNING_IDENTITY`, notarytool creds).
   - Windows: Authenticode cert. Unsigned Windows builds get SmartScreen-blocked.
   Decide cert storage (CI secrets) before enabling.

4. **Windows bundle verified on a real Windows box.**
   The Windows path (`resolve_pdfium_path` + `bundle.resources` + `fetch-pdfium.sh win`) is
   wired but UNTESTED — there is no Windows machine here. A green local/CI Windows build that
   opens a PDF must be confirmed once. (Open M1 task.)

5. **Definitive §20 verdict passed.**
   Cutting installers before the M1 performance gate is proven on the 16 GB floor machine
   (Windows + macOS, per `bench/RUNBOOK-S20.md`) ships an app that may fail its own acceptance
   criteria. §20 Go is the gate for M2; it is also the gate for releases.

## Release pipeline shape (once enabled)

```
version bump (scripts/sync-version.cjs across the 3 files) → commit
  → git tag v<X.Y.Z> → push tag to Forgejo
  → Forgejo Actions: build macOS universal + Windows x64, bundle PDFium, sign
  → poll run via Forgejo API (/ci-status pattern) until success/failure
  → verify release assets present (macOS .dmg/.app.tar.gz + sig, Windows .msi/.exe + sig)
  → KB observation → report
```

## Version files (source of truth = package.json)

`scripts/sync-version.cjs [version]` syncs `package.json`, `src-tauri/Cargo.toml`,
`src-tauri/tauri.conf.json`. Run `npm install` after a bump so `package-lock.json` follows,
and commit `Cargo.lock` too.
