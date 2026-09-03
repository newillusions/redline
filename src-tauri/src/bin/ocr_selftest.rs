//! OCR bundling smoke test (Phase 2b) — proves the actual built app can
//! locate its bundled Tesseract engine + tessdata and recognize real text,
//! run against the CI-built release bundle (or a local dev build) rather
//! than only compiling against dev-machine Homebrew/apt Tesseract. See
//! docs/ocr.md "Phase 2b: bundling into the shipped app" and
//! `.github/workflows/build-releases.yml`'s `build-ocr-check` steps, which
//! invoke this binary with `--tessdata-dir` (or `TESSDATA_PREFIX`) pointed
//! at the just-built bundle's extracted `resources/ocr/tessdata` directory.
//!
//! Feature-gated (`required-features = ["ocr"]` in Cargo.toml) — this binary
//! does not exist in a default (`ocr` off) build, so `cargo build`/`cargo
//! tauri build` without `--features ocr` never touches it.
//!
//! Usage: `ocr-selftest [--tessdata-dir <path>]`
//!   - `--tessdata-dir` is optional; when absent, `OcrEngineHandle::load(None)`
//!     falls through to Tesseract's own lookup (`TESSDATA_PREFIX` env var or
//!     compiled-in default — the same resolution `lib.rs::resolve_tessdata_dir`
//!     performs at app startup). Passing `--tessdata-dir` explicitly (as the
//!     CI proof step does) tests the bundle path directly, independent of
//!     whether the app-startup env-var wiring also works.
//!
//! The fixture is `tools/fixtures/ocr/selftest.png` (420x90 PNG, black text
//! "REDLINE OCR SELFTEST" on white, generated once via Pillow — not part of
//! the accuracy-benchmark corpus, which stays scanned-CAD-realistic; this is
//! deliberately trivial so a failure means "the engine/tessdata didn't load
//! at all", not "recognition accuracy regressed"). Embedded via
//! `include_bytes!` so the binary is self-contained and runs from any CWD —
//! load-bearing for CI, where the working directory during a bundle-proof
//! step is wherever the extracted app bundle lives, not the repo checkout.
//! Only 2 of the 3 words are asserted (see `EXPECT_SUBSTRINGS`): a local
//! smoke run found Tesseract misreads the trailing "T" as "1" at this small
//! a rendered font size, which is exactly the kind of low-stakes noise this
//! test is NOT meant to gate on.

use std::path::PathBuf;
use std::process::ExitCode;

use redline_lib::ocr::OcrEngineHandle;
use redline_lib::render::PageRaster;

const FIXTURE_PNG: &[u8] = include_bytes!("../../../tools/fixtures/ocr/selftest.png");
const EXPECT_SUBSTRINGS: [&str; 2] = ["REDLINE", "OCR"];

fn main() -> ExitCode {
    let mut tessdata_dir: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tessdata-dir" => {
                tessdata_dir = args.next().map(PathBuf::from);
            }
            other => {
                eprintln!("ocr-selftest: unknown argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    println!(
        "ocr-selftest: tessdata_dir={:?} TESSDATA_PREFIX={:?}",
        tessdata_dir,
        std::env::var("TESSDATA_PREFIX").ok()
    );

    let mut engine = match OcrEngineHandle::load(tessdata_dir.as_deref()) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("ocr-selftest: FAIL — OcrEngineHandle::load: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    let img = match image::load_from_memory(FIXTURE_PNG) {
        Ok(i) => i.to_rgb8(),
        Err(err) => {
            eprintln!("ocr-selftest: FAIL — decoding embedded fixture PNG: {err}");
            return ExitCode::FAILURE;
        }
    };
    let (width_px, height_px) = (img.width(), img.height());
    // dpi/scale are arbitrary here (only recognition text matters, not
    // bbox-to-PDF mapping) but kept internally consistent with the
    // documented `scale = dpi / 72.0` / `pts = px / scale` relationship
    // (see `render::PageRaster`'s doc comment) rather than left at 0, so
    // downstream bbox math never divides by zero.
    let dpi = 300.0_f32;
    let scale = dpi / 72.0;
    let raster = PageRaster {
        doc_id: "ocr-selftest".to_string(),
        page_index: 0,
        rgb: img.into_raw(),
        width_px,
        height_px,
        dpi,
        scale,
        page_width_pts: width_px as f64 / scale as f64,
        page_height_pts: height_px as f64 / scale as f64,
    };

    let lines = match engine.recognize_page(&raster) {
        Ok(l) => l,
        Err(err) => {
            eprintln!("ocr-selftest: FAIL — recognize_page: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    let joined = lines
        .iter()
        .map(|l| l.text.to_uppercase())
        .collect::<Vec<_>>()
        .join(" ");
    println!(
        "ocr-selftest: recognized {} line(s): {:?}",
        lines.len(),
        joined
    );

    let missing: Vec<&str> = EXPECT_SUBSTRINGS
        .iter()
        .filter(|s| !joined.contains(*s))
        .copied()
        .collect();
    if missing.is_empty() {
        println!(
            "ocr-selftest: PASS — found {:?} in recognized text",
            EXPECT_SUBSTRINGS
        );
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "ocr-selftest: FAIL — missing {:?} in recognized text {:?}",
            missing, joined
        );
        ExitCode::FAILURE
    }
}
