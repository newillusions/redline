# Auto-OCR

Status: Phase 2b (2026-09-03) — macOS/Windows release-bundle wiring + a
bundling smoke test, implemented and verified locally (macOS: compiles,
`cargo test`/`--features ocr` both green, the `ocr-selftest` binary PASSes
against the fetched bundled tessdata). The GitHub Actions proof leg
(`build-ocr-check`, see below) that exercises the actual macOS/Windows
GitHub-hosted runners has not landed a result as of this writing — see the
PR for the run outcome. Still no `-ocr.pdf` writer, no UI, no auto-trigger.
Feature stays OFF by default (`src-tauri/Cargo.toml`'s `ocr` feature); the
tag-triggered release path is unaffected by this phase — see "Phase 2b:
bundling into the shipped app" below for exactly what changed and what's
still owed.

## Engine

Tesseract 5 via the [`leptess`](https://docs.rs/leptess/0.14.0/leptess/)
crate (`src-tauri/src/ocr/mod.rs`). This supersedes Phase 1's `ocrs` engine
(pure-Rust, ONNX via `rten`): a 2026-09-02 bake-off
(`/Users/martin/dev-reports/2026-09-02-redline-ocr-bakeoff.md`,
observation:lr0cwsixkpbzei7vthon) measured `ocrs` topping out at 30% overall
recall on this crate's own scanned-CAD fixture corpus even with a rotation
fix, against Tesseract's 93%/98% (baseline/rotate-4x) on the same corpus.
Martin's decision (2026-09-02, decision:h6k4psk6n7xegql9ya9g): switch to
Tesseract and pay the native-dependency bring-up cost.

`leptess` links system Tesseract + Leptonica via `tesseract-plumbing`'s
pkg-config-driven build — no vendored/bundled build, no Cargo feature flags
of its own.

## Language data (tessdata)

Tesseract needs `eng.traineddata` at runtime, found either at an explicit
path passed to `OcrEngineHandle::load(Some(path))`, or via Tesseract's own
standard lookup (`TESSDATA_PREFIX` env var, or its compiled-in default) when
`load(None)` is used.

Per platform, today (Phase 2a, dev/CI only — no shipped-app plan executed
yet):

- **macOS (dev machines):** Homebrew's `tesseract` + `leptonica` formulae
  install `eng.traineddata` under
  `/opt/homebrew/share/tessdata/` (Apple Silicon) or the Intel-prefix
  equivalent. `OcrEngineHandle::load(None)` finds it via Tesseract's
  compiled-in default search relative to the linked library.
- **Linux CI** (`.forgejo/Dockerfile.test-rust`, the `ocr` build-arg leg):
  the `tesseract-ocr-eng` apt package installs it under
  `/usr/share/tesseract-ocr/<major>/tessdata/` (the exact subpath varies by
  Tesseract's packaged major version — the Dockerfile resolves it at build
  time via `dpkg -L tesseract-ocr-eng` rather than hardcoding a path, and
  exports `TESSDATA_PREFIX` explicitly before running the ocr tests).
- **Windows:** static-linked via vcpkg (`tesseract:x64-windows-static-md`)
  in the GitHub Actions OCR-proof leg as of Phase 2b — no dev machine or
  Forgejo CI leg exercises the `ocr` feature on Windows; see "Phase 2b"
  below for the design and its unverified parts.

### Phase 2b: bundling into the shipped app

For the shipped macOS/Windows app, end users should not need a separate
Tesseract install. What Phase 2b actually built:

#### tessdata (both platforms)

`scripts/fetch-ocr-tessdata.sh` downloads `eng.traineddata` into
`src-tauri/resources/ocr/tessdata/eng.traineddata` (gitignored, fetched at
build time — same pattern as `scripts/fetch-pdfium.sh`). `tauri.conf.json`'s
existing `bundle.resources: {"resources/": "resources/"}` mapping already
wholesale-copies that directory into the bundle with no config change
needed. At runtime, `lib.rs::resolve_tessdata_dir` (new, mirrors
`resolve_pdfium_path`) checks, in order: an existing `TESSDATA_PREFIX` (dev
override, never overwritten), the Tauri resource dir's
`resources/ocr/tessdata`, then next-to-the-executable `resources/ocr/tessdata`
(portable layout) — and sets `TESSDATA_PREFIX` so
`OcrEngineHandle::load(None)` finds it via Tesseract's own standard lookup,
same as `leptess`'s existing `None`-path already documented above. If
nothing resolves, this logs loudly (`log::error!`) but does NOT panic app
startup — OCR is off by default with no auto-trigger yet, so a missing
tessdata directory must not block the rest of the app; the actual fail-loud
behavior for a user attempting OCR happens at the point of use, inside
`OcrEngineHandle::load`'s own error (unchanged from Phase 2a).

**Source pinning, not "biggest/most accurate":**
`tesseract-ocr/tessdata_fast`'s `eng.traineddata` (pinned to commit
`923915d4ced2a7235221788285785a29c4a42d4a`, sha256-verified in the fetch
script) was chosen over the standard `tessdata`/`tessdata_best` repos for a
specific reason, verified this session: it is BYTE-IDENTICAL (same sha256,
`7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2`, 3.9 MB —
not the ~12 MB this doc previously guessed) to the `eng.traineddata`
Homebrew's `tesseract` formula installs, which is the exact file the Phase
2a benchmark (93%/98% recall) was measured against. Bundling a different
model would ship unbenchmarked accuracy.

#### macOS: dylib bundling via `dylibbundler`

`leptess`/`tesseract-plumbing` links Tesseract + Leptonica as DYNAMIC
libraries via pkg-config at build time, at their absolute Homebrew install
paths — a released `.app` can't assume Homebrew exists on the end user's
Mac. `brew install tesseract` alone pulls a large transitive dependency tree
(cairo, fontconfig, glib, harfbuzz, icu4c, libarchive, pango, freetype,
gettext — **37 recursive runtime deps**, per `brew info tesseract`, verified
2026-09-03), far too many to hand-list in `tauri.conf.json`'s
`bundle.macOS.frameworks` (built for a small fixed set with stable names,
not a sprawling Homebrew tree with version-suffixed filenames that drift on
every Homebrew bump). The `.github/workflows/build-releases.yml`
`build-macos` job's new `Bundle Tesseract + Leptonica dylibs into the .app`
step instead runs [`dylibbundler`](https://github.com/SCG82/macdylibbundler)
(verified via its README this session) after `tauri:build`, before the
existing code-signing step:

```bash
dylibbundler -od -b -x "$APP_PATH/Contents/MacOS/<exe>" \
  -d "$APP_PATH/Contents/Frameworks/" \
  -p "@executable_path/../Frameworks/"
```

`-b` walks `otool -L` recursively, copies every non-system dylib into `-d`,
and rewrites both the executable's and each copied dylib's load commands to
the `-p` path — so this bypasses `tauri.conf.json`'s frameworks config
entirely rather than fighting it. Runs BEFORE the existing `Fix code
signature` step (dylibbundler rewrites load commands, which invalidates any
prior signature, matching the existing re-sign-after-build flow already in
place for the CSResourcesFileMapped fix).

#### Windows: static linking via vcpkg

`tesseract-sys`'s `build.rs` (`ccouzens/tesseract-sys`, verified against its
own source this session) calls `vcpkg::Config::new().find_package("tesseract")`
on Windows — no manual `TESSERACT_INCLUDE_PATHS`/`TESSERACT_LINK_PATHS`/
`TESSERACT_LINK_LIBS` env vars needed as long as vcpkg finds the port.
`windows-latest` ships vcpkg preinstalled at `C:\vcpkg` with
`VCPKG_INSTALLATION_ROOT` already set (verified against
`actions/runner-images`'s `Windows2022-Readme.md`); the workflow aliases
that to `VCPKG_ROOT`, which is what the `vcpkg` Rust crate actually reads
(verified via its docs.rs page: `VCPKG_ROOT` first, then user-wide `vcpkg
integrate install`, then a cargo-vcpkg tree).

**Static linking, deliberately, not leptess's own README example.** The
`leptess` README shows `vcpkg install tesseract:x64-windows` (the dynamic
triplet) plus `VCPKGRS_DYNAMIC=true` for its own test suite. This workflow
instead runs `vcpkg install tesseract:x64-windows-static-md` — the `vcpkg`
crate's own DEFAULT triplet on 64-bit Windows when `VCPKGRS_DYNAMIC` is
*unset* (verified via the crate's docs.rs page) — so no triplet/dynamic
override is needed at all. Static linking bakes Tesseract + Leptonica (and
their own C dependency tree) directly into `redline.exe`, needing ZERO
DLL-bundling work — the exact problem `dylibbundler` solves on macOS has no
equivalent tool in this pipeline, so avoiding it entirely by going static is
the simpler, more robust choice for a first bring-up. **This triplet choice
is unverified against a real vcpkg build of the `tesseract` port until the
GitHub Actions proof leg actually runs** — some vcpkg ports don't build
cleanly under every triplet — which is exactly what that leg exists to
prove or disprove; see "What's NOT built yet" below if it needs revisiting.

#### Bundling smoke test: `ocr-selftest`

A new feature-gated binary (`src-tauri/src/bin/ocr_selftest.rs`,
`required-features = ["ocr"]` so it doesn't exist in a default build) loads
`OcrEngineHandle` against a caller-supplied `--tessdata-dir` (or
`TESSDATA_PREFIX`), decodes an embedded 420×90 PNG fixture
(`tools/fixtures/ocr/selftest.png`, "REDLINE OCR SELFTEST" in black
Helvetica-36 on white, generated via Pillow — deliberately NOT part of the
scanned-CAD accuracy corpus, since this gates "did the engine/tessdata load
and run at all", not recognition accuracy), wraps it as a `PageRaster`
directly (no PDFium needed — `PageRaster`'s fields are all public), and
asserts the recognized text contains both `"REDLINE"` and `"OCR"` (not the
full string — a local run found Tesseract misreads the trailing "T" as "1"
at this font size, exactly the low-stakes noise this test should not gate
on). Verified locally (macOS, Apple Silicon, Homebrew Tesseract 5.5.3):
`cargo run --release --features ocr --bin ocr-selftest -- --tessdata-dir
src-tauri/resources/ocr/tessdata` → `PASS`.

**Platform asymmetry in what the CI proof leg actually tests**, stated
plainly rather than glossed over: on macOS the `.app` bundle IS the final
build artifact (no separate install step), so the workflow's smoke-test step
runs `ocr-selftest` against the ACTUAL packaged
`Contents/Resources/resources/ocr/tessdata` inside the just-built,
just-dylib-bundled `.app`. On Windows, the bundled resource layout only
materializes after the NSIS installer runs, and silently installing it on a
CI runner (`installMode: perMachine` typically wants elevation) is its own
source of flakiness. To keep that leg's failure signal narrowly about "does
the vcpkg-linked Tesseract binary work on Windows at all" — the genuinely
unproven, risky part — the Windows smoke test instead copies
`src-tauri/resources/ocr/tessdata` next to the raw `target/release/redline.exe`
(exercising `resolve_tessdata_dir`'s 3rd, "portable layout" candidate) rather
than installing the real NSIS package. **Proving the actual NSIS-installed
layout end-to-end is a residual gap**, not silently assumed to work — see
"What's NOT built yet" below.

#### Proving both legs without cutting a release

`.github/workflows/build-releases.yml` gained a `push: branches: [feat/**]`
trigger and a `workflow_dispatch` boolean input `build_ocr`; either sets a
per-job `Determine OCR proof mode` step's `enabled` output, which:
adds `--features ocr` (macOS also adds `--bundles app`, skipping the DMG —
nothing in this leg needs one) to the `Build Tauri app` step, runs the new
Tesseract-install/tessdata-fetch/dylib-bundle/smoke-test steps above, and
gates OFF every publish-oriented step (code signing, DMG recreation,
Authenticode signing, Gitea/GitHub asset upload, the `update-manifest`
job) via `if: steps.ocr_mode.outputs.enabled != 'true'` — no signing, no
upload, no release cut. The tag-triggered release path's steps are
functionally unchanged: `ocr_mode.enabled` evaluates `false` there, so every
existing step's condition is unaffected and the feature stays off, exactly
as before this phase.

## Rotate-4x strategy

Tesseract's page segmentation reads text at one fixed page orientation and
has no per-region rotation detection (unlike Phase 1's `ocrs`, which
returned a per-line `RotatedRect`). Rotated/vertical CAD dimension strings —
Martin's hard requirement — are therefore recovered by rotating the WHOLE
PAGE, not by asking Tesseract to find rotated regions on an upright page:

1. For each of `0°, 90°, 180°, 270°` (`image::imageops::rotate{90,180,270}`,
   all clockwise), rotate the page raster's RGB buffer, PNG-encode it in
   memory, and hand it to Tesseract (`leptess::LepTess::set_image_from_mem`
   — the crate has no raw-pixel-buffer input, only file/encoded-bytes).
2. Run recognition (`get_tsv_text`), which yields per-WORD bounding boxes
   (pixel space of the ROTATED raster) and confidences (0-100) in one pass.
   Words are grouped into lines by Tesseract's own
   `(block_num, par_num, line_num)` structural grouping, text joined with
   spaces, bounding box unioned, confidence averaged.
3. Each line's bounding box is mapped back from the rotated raster's pixel
   space to the ORIGINAL (unrotated) raster's pixel space via the algebraic
   inverse of that pass's rotation, then to PDF user-space via the existing
   `pixel_to_pdf` conversion (unchanged from Phase 1). A vertical dimension
   string therefore comes out with a correctly tall, narrow bounding box in
   PDF space, even though Tesseract itself only ever read upright text.
4. The four passes' results are merged: sort all candidate lines by
   confidence descending, then greedily keep each one unless it overlaps
   (intersection-over-union > 0.3) an already-kept, higher-confidence
   candidate — classic non-max suppression. In practice the SAME real text
   line is detected at 1-4 of the passes (correctly, at the pass whose
   rotation makes it upright; as noise or missed entirely at the others),
   and only the best reading survives per region.

See `src-tauri/src/ocr/mod.rs`'s module doc comment and unit tests
(`Rotation::inverse_map`, `iou`, `merge_rotate4x_candidates`,
`parse_tsv_words`/`group_words_into_lines`) for the exact geometry and
parsing logic, all covered by pure-Rust tests needing no Tesseract binary.

## Measured numbers

Full corpus benchmark (`src-tauri/tests/ocr_benchmark.rs`,
`cargo test --features ocr --test ocr_benchmark -- --ignored --nocapture`),
run this session on macOS (Apple Silicon, Tesseract 5.5.3 / Leptonica 1.87.0
via Homebrew, PDFium `chromium/7869`):

| fixture | ms/page | recall | precision | mean conf | vert. recall |
|---|---|---|---|---|---|
| a-plain-horizontal | 4789 | 100% | 24% | 63% | n/a |
| b-rotated-dimension | 1440 | 100% | 30% | 82% | 100% |
| c-mixed-horizontal-vertical | 3457 | 100% | 23% | 77% | 100% |
| d-dense-a0-latency (150 DPI, 40 labels) | 19113 | 100% | 9% | 77% | n/a |
| e-small-font-title-block (6pt) | 457 | 80% | 28% | 69% | n/a |

**OVERALL recall: 98% (56/57 lines); vertical-text recall: 100% (3/3).**
Matches the bake-off report's rotate-4x numbers. The benchmark test asserts
a hard floor of **≥90% overall recall** — below that the build fails, not
just reports a worse number.

Precision is low (9-30%) because the merge keeps every non-overlapping
candidate across all 4 rotation passes, and the 3 "wrong orientation" passes
routinely produce extra low-confidence noise fragments alongside the correct
reading (see the bake-off report's raw-dump section for concrete examples
like `"fo)"`, `"(=)"` from reading vertical text sideways). This does not
affect recall (the matching logic only checks whether each EXPECTED line
was found among the recognized ones) and is expected, not a defect — a
future `-ocr.pdf` writer (Phase 2c) should filter low-confidence/degenerate
recognized text before embedding it as invisible searchable text, rather
than embedding every raw candidate.

Latency is real-page latency for the FULL rotate-4x pass (all 4 rotations
run unconditionally); the dense A0 fixture at 19.1s reflects that cost on a
40-label sheet at 150 DPI on this Mac. No latency ceiling is asserted by the
benchmark.

## What's NOT built yet

- `-ocr.pdf` invisible-text-layer writer (Phase 2c).
- Auto-trigger on document open / manual "Run OCR" UI action (Phase 2c).
- **The GitHub Actions proof leg's actual result.** Phase 2b's macOS/Windows
  bundling design (above) is implemented and locally verified on macOS
  (compiles, tests green, `ocr-selftest` PASSes against fetched tessdata) —
  but has not yet been confirmed green on real GitHub-hosted macOS/Windows
  runners. Windows in particular carries real unverified risk: whether the
  vcpkg `tesseract` port actually builds cleanly under the
  `x64-windows-static-md` triplet has never been exercised in this repo.
  See the PR this phase shipped in for the actual run outcome.
- **Windows NSIS-installed-layout verification.** The Windows smoke test
  (see "Bundling smoke test" above) proves the vcpkg-linked Tesseract binary
  itself works and that `resolve_tessdata_dir`'s portable-layout candidate
  resolves — it does NOT install the real NSIS package and verify resources
  land where a genuinely installed app would look for them. A follow-up
  should either silently-install the NSIS output on the CI runner and repeat
  the smoke test against that layout, or accept the current portable-layout
  proof as sufficient and say so explicitly (owner call).
- Non-English language support (only `eng.traineddata` is wired anywhere in
  this repo or its CI).
