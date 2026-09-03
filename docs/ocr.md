# Auto-OCR

Status: Phase 2a (2026-09-02) — engine + rotate-4x strategy + benchmark only.
No `-ocr.pdf` writer, no UI, no auto-trigger. Feature-gated (`ocr`, off by
default in `src-tauri/Cargo.toml`).

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
- **Windows:** unverified in this repo so far. No dev or CI leg exercises
  the `ocr` feature on Windows yet — this is open work, see "Phase 2b" below.

### Phase 2b: bundling into the shipped app (not yet built)

For the shipped macOS/Windows app, end users should not need a separate
Tesseract install. The plan (not implemented this phase):

- Ship `eng.traineddata` (~12 MB) as a Tauri bundled resource, the same
  pattern already used for the PDFium dynamic library
  (`tauri.conf.json`'s `bundle.resources`, populated by
  `scripts/fetch-pdfium.sh` at build time). A parallel
  `scripts/fetch-ocr-tessdata.sh` (not yet written) would download the
  official `eng.traineddata` release asset into
  `src-tauri/resources/ocr/tessdata/` and `OcrEngineHandle::load` would be
  called with `Some(&bundled_tessdata_dir)` resolved via Tauri's resource
  path API at runtime, instead of `None`.
- Tesseract + Leptonica themselves are native SHARED libraries the `leptess`
  crate links at BUILD time (not runtime-loaded like PDFium) — so the
  *build machine* needs them (Homebrew on macOS CI runners, apt on Linux),
  but the *shipped binary* embeds/links against them per the platform's
  normal dynamic-linking rules. macOS: verify the built `.app` correctly
  resolves `libtesseract`/`liblept` (likely needs `install_name_tool`
  rpath fixup or static linking, unverified). **Windows is the hard case**
  flagged since Phase 1's original descoping note in this repo's CLAUDE.md
  history: `leptess`/`tesseract-plumbing` on Windows needs a
  vcpkg-provided static or dynamic Tesseract+Leptonica build, with
  `VCPKG_ROOT` wired into `.github/workflows/build-releases.yml`'s
  `build-windows` job — entirely unattempted so far, no vcpkg bootstrap
  exists in this repo yet. Budget this as its own investigation before
  attempting a Windows release with `--features ocr` on.

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
- macOS/Windows tessdata + native-library bundling into the shipped app
  (Phase 2b — see above).
- Non-English language support (only `eng.traineddata` is wired anywhere in
  this repo or its CI).
