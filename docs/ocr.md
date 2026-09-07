# Auto-OCR

Status: Phase 2c-i (2026-09-04) — the invisible searchable text-layer writer
(`ocr::writer::write_ocr_pdf`), proven end-to-end against a real fixture:
real Tesseract OCR output written by the real writer, then found by both
PDFium in-document search AND the Tantivy folder-indexer's `lopdf`
extraction path (see "Phase 2c: text-layer writer" below and
`tests/ocr_writer_e2e.rs`). Phase 2a (engine) and Phase 2b (macOS/Windows
bundling — the GitHub Actions proof leg landed GREEN on both platforms
before its PR merged, correcting this doc's earlier "has not landed a
result" note) are both merged to `main`. Still no UI, no "Run OCR" command
wiring, no auto-trigger-on-open — explicitly Phase 2c-ii, split off by owner
decision 2026-09-04 so 2c-i (writer + search pickup) could ship on its own.
Feature stays OFF by default (`src-tauri/Cargo.toml`'s `ocr` feature); no
code path in this phase runs OCR automatically or changes anything about a
normal document-open/save flow.

**Release pipeline (fixed 2026-09-07):** tagged desktop releases now build
with `--features ocr` and bundle it, closing a gap where v0.3.17/v0.3.18
shipped without OCR compiled in despite the `workflow_dispatch` proof leg
passing green - see "Shipping OCR in every tagged release" under Phase 2b
below.

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

#### Shipping OCR in every tagged release (fixed 2026-09-07)

**Tagged releases now build and bundle OCR.** Through v0.3.17/v0.3.18 the
tag-triggered release path never set `--features ocr` at all - the
`workflow_dispatch` proof leg (below) built and smoke-tested OCR
successfully, but that never touched a real `v*`-tag release, so shipped
installers had no OCR compiled in despite the proof leg passing green. The
owner hit "OCR is not compiled into this build" on the shipped app and that
gap is what this fix closes.

`.github/workflows/build-releases.yml`'s per-job `Determine OCR proof mode`
step now sets two separate flags instead of one:

- **`enabled`** - whether THIS build compiles with `--features ocr` and runs
  the Tesseract-install/tessdata-fetch/dylib-bundle(macOS)/vcpkg-static-link
  (Windows)/smoke-test steps. `true` for a real `v*` tag push AND for the
  `workflow_dispatch` proof leg (`build_ocr=true`) - OCR now ships in every
  tagged release, not just the rehearsal.
- **`proof_leg`** - `true` only for the `workflow_dispatch` rehearsal.
  Publish-oriented steps (code signing, DMG recreation, Authenticode
  signing, Gitea/GitHub asset upload, the `update-manifest` job) are gated
  `if: steps.ocr_mode.outputs.proof_leg != 'true'`, so a real tag release
  still signs and ships; only the rehearsal skips signing/upload/manifest.

**The OCR bundling smoke test is a hard release gate**, not just a proof-leg
check: it runs (gated on `enabled`) before the code-signing step in the same
job, with no `continue-on-error`, so a failure fails the job and every
downstream signing/upload step (gated on `proof_leg`) is skipped by GitHub
Actions' default stop-on-failure behavior. The `update-manifest` job's own
condition additionally requires both platform jobs' `result` to be
`success` (not merely "not cancelled") before publishing `update.json` -
closing a separate gap where a same-job smoke-test failure could otherwise
still let a stale/empty-signature manifest ship to the auto-updater, since a
job's declared `outputs:` survive a later step's failure.

Manually triggering the `workflow_dispatch` rehearsal (`build_ocr=true`)
remains the way to prove the OCR build/bundle/smoke-test steps on a branch
without cutting a release - unchanged from before this fix, still the only
way to exercise this on a non-tag ref since GitHub Actions workflow changes
can't be run on Forgejo's self-hosted runner.

An earlier version of this leg used a `push: branches: [feat/**]` trigger
instead — removed on review feedback the same day it landed, because it
fired a full macOS+Windows GitHub Actions build (the expensive, slow leg)
on every push to ANY `feat/**` branch workspace-wide, whether or not that
push had anything to do with OCR. `workflow_dispatch` is opt-in per run.

**Triggering the OCR proof leg by hand:**

```bash
gh workflow run build-releases.yml \
  --repo newillusions/redline \
  --ref <your-branch> \
  -f version=0.0.0-ocr-proof \
  -f build_ocr=true
```

`version` is a required input even though the proof leg never uses it
(no release is cut, so no version string is embedded anywhere) — any
placeholder value works. Or via the GitHub Actions UI: Actions tab →
"Build macOS and Windows Releases" → "Run workflow" → pick the branch →
toggle "OCR proof leg" on → Run workflow. Poll with `gh run list --repo
newillusions/redline --workflow build-releases.yml` /
`gh run watch <run-id> --repo newillusions/redline`, or the GitHub API
(`GET /repos/newillusions/redline/actions/runs`).

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

### Why no `osd.traineddata`

Tesseract ships a separate orientation/script-detection model
(`osd.traineddata`) specifically for the case this module's rotate-4x
strategy already solves a different way. Only `eng.traineddata` is fetched
(`scripts/fetch-ocr-tessdata.sh`) and bundled (Phase 2b) — `osd.traineddata`
is deliberately NOT included. Rationale: Tesseract's OSD would tell us "this
page looks rotated 90°" for a whole page or region, then Tesseract would
still need a *second* recognition pass at the corrected orientation — which
is exactly what rotate-4x already does for every page unconditionally (all
4 orientations, always), except rotate-4x also handles the CAD-specific case
OSD is not designed for: a SINGLE page with text at MULTIPLE simultaneous
orientations (e.g. `b-rotated-dimension.pdf`'s horizontal label plus
90°-rotated dimension string on the same sheet) — OSD picks one dominant
orientation per page/region, not a per-line mix. Adding OSD would mean
running it, branching on its verdict, and STILL needing the rotate-4x
fallback for mixed-orientation sheets — extra tessdata (~10MB), extra
runtime cost, and extra code path for no coverage rotate-4x doesn't already
provide. Revisit only if a future profiling pass shows rotate-4x's
always-run-all-4-passes cost (`docs/ocr.md`'s "Measured numbers" below —
19.1s on the dense A0 fixture) is a real problem OSD-first branching would
measurably fix.

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
was found among the recognized ones) and is expected, not a defect — the
Phase 2c text-layer writer (below) filters low-confidence/degenerate
recognized text before embedding it as invisible searchable text, rather
than embedding every raw candidate.

Latency is real-page latency for the FULL rotate-4x pass (all 4 rotations
run unconditionally); the dense A0 fixture at 19.1s reflects that cost on a
40-label sheet at 150 DPI on this Mac. No latency ceiling is asserted by the
benchmark.

## Phase 2c: text-layer writer

`src-tauri/src/ocr/writer.rs` (`write_ocr_pdf`) embeds recognized `OcrLine`s
as an invisible, searchable text layer.

**In-place, not a sidecar file.** The scoping doc's working name (`-ocr.pdf`)
implied a separate output file; the shipped design writes the text layer
into the SAME PDF bytes instead. Every consumer that needs to "see" OCR
text already reads a document's own content stream directly — Bluebeam and
Acrobat search the file a user opens, `search::indexer::extract_pdf_text`
(the Tantivy folder-index source) walks whatever file it finds in a folder
via `lopdf::Document::extract_text`, and `render::RenderEngine::search_page`
(in-app search) opens the `doc_id` the app already loaded. None of them
know a sidecar convention; a sidecar would need matching logic added in at
least three independent places for no real benefit, since embedding an
invisible layer never touches existing visible content anyway (see
`writer.rs`'s module doc comment for the full reasoning).

**Mechanism** (PDF spec, not vendor-specific): each kept line becomes one
`BT...ET` text object appended to the page's content stream (reusing
`docops::append_to_page_contents`/`docops::add_named_objects_to_page_resources`
— generalized this phase from the redact-region helpers `docops::mod.rs`
already had, rather than duplicating that finicky `lopdf` resource-merging
logic), drawn in **text-rendering mode 3** (`3 Tr` — "neither fill nor
stroke", PDF 32000-1:2008 §9.3.3) using a standard, non-embedded Helvetica
font (`/Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding` — built into
every PDF-1.x viewer, no font file to bundle). Font size matches the
recognized line's box height; horizontal scaling (`Tz`) is chosen so the
string's nominal Helvetica width matches the line's measured baseline
length, using a single flat average-glyph-width constant rather than a
full per-glyph AFM metrics table (documented at `HELVETICA_AVG_WIDTH_EM`'s
definition — the layer is invisible, so width accuracy only affects the
approximate size of a search-hit/selection box, never text-extraction
correctness or PDF validity; `Tz` is clamped to a safe range regardless).
Rotation follows the line's own baseline vector (`corners_pdf`'s
top-left→top-right edge, which rotate-4x already maps into the ORIGINAL
page's coordinate space — see the rotate-4x section above), so a
90°-rotated CAD dimension string gets an invisible text run rotated to
match, not a wide horizontal box stamped over a tall narrow region.

**Confidence filtering** (named as owed above): `write_ocr_pdf` takes a
`min_confidence` parameter (`DEFAULT_MIN_CONFIDENCE = 0.5`) and drops, before
embedding, any line below that threshold, with `None` confidence, empty/
whitespace-only text, or a degenerate (near-zero-area) box. Filtering
happens BEFORE embedding, never after — an already-embedded noise fragment
is exactly as searchable as a correct line, so there is no cheaper place to
filter than the moment the invisible text is chosen. 0.5 is chosen from the
measured per-fixture mean confidences above (correct lines: 63-82%; the
"wrong orientation" noise the precision numbers are attributed to reads as
qualitatively low-confidence) rather than a corpus sweep tuned to an exact
number this session — a future UI setting (Phase 2c-ii) can expose a
different threshold without changing the writer's contract.

**Proof, not just compilation.** `ocr::writer`'s own unit tests (pure
`lopdf`, no Tesseract/PDFium needed — same reason the ocr feature gate
already requires `libtesseract-dev` at compile time regardless) assert the
`3 Tr` operator, the `Tj` string, font-resource registration, and every
filter case (low confidence, `None` confidence, empty text, degenerate box,
out-of-range page index) using synthetic `OcrLine`s. `tests/ocr_writer_e2e.rs`
(`#[ignore]`d, real PDFium + real Tesseract, matching `ocr_benchmark.rs`'s
own convention) proves the FULL real pipeline on `a-plain-horizontal.pdf`
(the same image-only fixture the benchmark scores): confirms the source
fixture has ZERO extractable text, runs real OCR, writes the real layer,
then confirms BOTH `render::RenderEngine::search_page` (PDFium — the
in-app/Bluebeam-equivalent search path) AND `lopdf::Document::extract_text`
(the exact call the Tantivy folder indexer makes) find the recognized word
— and that the text layer survives an unrelated `docops::optimize` pass.
This is the "does a scanned fixture become searchable" proof, exercised
against real engines rather than mocked. **Caveat:** like `ocr_benchmark.rs`,
this test is `#[ignore]`d and needs a real PDFium dylib + Tesseract install
neither Forgejo CI's `test-rust` job nor its `ocr` job's `RUN_OCR_TESTS`
build-arg leg provisions for the WRITER path specifically (the `ocr` job's
`RUN_OCR_TESTS` block does run `ocr_benchmark.rs`, but `ocr_writer_e2e.rs`
was added after that Dockerfile step was written and is not yet wired into
it) — the "proven end-to-end" claim above rests on a real local run (quoted
in the shipping PR's test plan), not on CI evidence. Wiring it into the
Linux CI `ocr` job (same Tesseract/PDFium setup that leg already has) is a
cheap, real follow-up, not yet done.

**Not done this phase, explicitly Phase 2c-ii** (owner-approved split,
2026-09-04): no Tauri command/MCP tool wiring a "Run OCR" action to
`write_ocr_pdf`, no UI (toolbar action, progress, per-page status), no
auto-trigger heuristic on document open, no human visual/search
confirmation in real Bluebeam/Acrobat (same "owed, not silently assumed"
posture `.claude/rules/judgment.md` already applies to G9).

## What's NOT built yet

- **UI, command/MCP wiring, and auto-trigger (Phase 2c-ii).** See "Phase 2c:
  text-layer writer" above for exactly what IS built (the writer + its
  real-pipeline search-pickup proof) and what this split deliberately left
  out: an OCR action in the app (toolbar/menu), progress + per-page status,
  a setting-gated offer-or-run heuristic when a document opens with no
  extractable text on N sampled pages (default: offer, not silently run —
  matching this repo's general posture of never taking a write action a
  user didn't ask for), and a human visual/search confirmation in real
  Bluebeam/Acrobat.
- **Windows NSIS-installed-layout verification.** The Windows smoke test
  (see "Bundling smoke test" above) proves the vcpkg-linked Tesseract binary
  itself works and that `resolve_tessdata_dir`'s portable-layout candidate
  resolves — it does NOT install the real NSIS package and verify resources
  land where a genuinely installed app would look for them. A follow-up
  should either silently-install the NSIS output on the CI runner and repeat
  the smoke test against that layout, or accept the current portable-layout
  proof as sufficient and say so explicitly (owner call).
- ~~Codesigning + dylib bundling are mutually exclusive on macOS~~ **FIXED
  2026-09-07** (see "Shipping OCR in every tagged release" above): the
  dylib-bundling and smoke-test steps now run on the real tag-release path
  too, gated on `enabled` rather than the old proof-leg-only flag, and still
  placed BEFORE the `Fix code signature` step so the codesign happens after
  dylibbundler rewrites load commands, matching the pre-existing
  re-sign-after-build pattern. **Still owed:** the combined result
  (OCR-enabled + dylib-bundled + code-signed + smoke-tested, in one real
  build) has not yet been verified end-to-end against an actual signed
  release - that verification is the `workflow_dispatch` proof run this fix
  shipped with, and finally the first real tagged release built on top of
  it (v0.3.19).
- **Installer size deltas are unmeasured.** The macOS proof leg bundles 15
  dylibs into `Contents/Frameworks` (measured locally 2026-09-03: libarchive,
  libb2, libgif, libjpeg, libleptonica, liblz4, liblzma, libopenjp2, libpng16,
  libsharpyuv, libtesseract, libtiff, libwebp, libwebpmux, libzstd — NOT the
  ~37 recursive Homebrew *runtime* dependencies `brew info tesseract` reports,
  which include cairo/pango/glib/harfbuzz/icu4c/etc. that turned out to be
  needed by Tesseract's optional training-tool binaries, not the
  `libtesseract.dylib` this build actually links against). No DMG or NSIS
  installer has been built WITH the `ocr` feature (the proof leg uses
  `--bundles app` on macOS specifically to skip DMG creation, and the
  Windows leg never reaches the "Find installers"/size-reporting step,
  which is gated off). The actual installed-size and download-size impact
  is real but not yet measured — do that as part of wiring signing above.
- Non-English language support (only `eng.traineddata` is wired anywhere in
  this repo or its CI).
