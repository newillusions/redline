//! Atomic markup save: lopdf load -> write_markups -> save to a sibling temp file ->
//! fsync -> rename over the destination (the workspace sensitive-write pattern).
//! Full-rewrite strategy (v1); incremental PDF update is a later optimization.
//!
//! A crash between staging and rename can orphan a harmless `.pdf.redline-staged`
//! file next to the original (never auto-opened; manual cleanup; an open-time
//! sweep is a future nicety).

use std::path::Path;

use anyhow::{Context, Result};
use lopdf::Document;

use super::annots::{read_markups, write_markups};
use crate::markup::Markup;

/// Read the markup set from a PDF on disk.
pub fn load_markups_from(path: &Path) -> Result<Vec<Markup>> {
    let doc = Document::load(path).with_context(|| format!("load {}", path.display()))?;
    read_markups(&doc)
}

/// Load `src`, replace its managed annotations with `markups`, atomically produce
/// `dest`. `src == dest` is the save-in-place case (temp + rename over).
///
/// # Large-file cost
/// Full rewrite strategy (v1): loads the entire PDF into memory, writes a
/// complete temp copy, then renames - transiently ~2x disk and multi-second
/// wall time on C5-class (2 GB+) sets. Callers MUST run this off the UI
/// thread (spawn_blocking) and should surface progress for large files.
///
/// # Caller contract (Windows)
/// When `src == dest` (save-in-place), the caller MUST ensure no open file
/// handle to `dest` remains (e.g. the render engine's PDFium document) before
/// calling. Windows rename-over-an-open-file fails with ERROR_ACCESS_DENIED;
/// macOS tolerates it. The command layer closes the render doc before the
/// swap and reopens it after.
pub fn save_with_markups(src: &Path, dest: &Path, markups: &[Markup]) -> Result<()> {
    let mut doc = Document::load(src).with_context(|| format!("load {}", src.display()))?;
    write_markups(&mut doc, markups)?;

    let dir = dest.parent().context("dest has no parent dir")?;
    let tmp = dir.join(format!(
        ".redline-tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> Result<()> {
        let f = doc
            .save(&tmp)
            .with_context(|| format!("write {}", tmp.display()))?;
        f.sync_all().context("fsync temp")?;
        // Dir-fsync skipped deliberately: APFS/NTFS journal the rename; desktop-app
        // durability contract doesn't require it (same stance as Acrobat/Bluebeam).
        std::fs::rename(&tmp, dest).context("atomic rename")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::annots::tests::{one_page_doc, redline_markup};

    #[test]
    fn save_with_markups_writes_dest_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        let dest = dir.path().join("out.pdf");
        let m = redline_markup(0);
        save_with_markups(&src, &dest, std::slice::from_ref(&m)).unwrap();

        let got = load_markups_from(&dest).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), m.id());
        // Source untouched.
        assert!(load_markups_from(&src).unwrap().is_empty());
    }

    #[test]
    fn save_in_place_via_temp_swap() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("doc.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&p).unwrap();

        save_with_markups(&p, &p, &[redline_markup(0)]).unwrap();
        assert_eq!(load_markups_from(&p).unwrap().len(), 1);
        // No stray temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".redline-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file not cleaned: {leftovers:?}");
    }

    #[test]
    fn missing_source_errors_and_dest_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.pdf");
        let err = save_with_markups(&dir.path().join("absent.pdf"), &dest, &[]);
        assert!(err.is_err());
        assert!(!dest.exists());
    }

    /// E2E on the real C1 corpus tier: copy to temp -> save with one markup ->
    /// reload markups -> PDFium still opens and renders a tile from the saved file.
    ///
    /// Gated the same way as the render corpus tests: skips (with a printed
    /// message) when `PDFIUM_DYNAMIC_LIB_PATH` is unset or the corpus is absent.
    /// C1 is the light tier (~105 MB) and carries no extra `REDLINE_BENCH_TESTS`
    /// gate - it runs whenever PDFium + the corpus are present.
    ///
    /// Run single-threaded (PDFium global C state):
    ///   cargo test corpus_c1 -- --test-threads=1
    #[test]
    fn corpus_c1_save_roundtrip_and_renders() {
        use crate::render::tests::{corpus, one_tile};
        use crate::render::{RenderEngine, TileRequest};

        let Some(src) = corpus("c1-typical/c1-contract-691pg-A4.pdf") else {
            eprintln!("skip corpus_c1_save_roundtrip_and_renders: no PDFium env or corpus");
            return;
        };

        // 1. Copy to a temp dir — never touch the original.
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("c1-work.pdf");
        std::fs::copy(&src, &work).expect("copy corpus to temp");

        // 2. Save one markup in-place (temp-swap strategy).
        let m = redline_markup(0);
        save_with_markups(&work, &work, std::slice::from_ref(&m))
            .expect("save_with_markups on C1 corpus");

        // 3. Reload markups and verify the annotation survived the round-trip.
        let got = load_markups_from(&work).expect("load_markups_from saved C1");
        assert!(
            got.iter().any(|x| x.id() == m.id()),
            "markup id {} not found after save; got {:?}",
            m.id(),
            got.iter().map(|x| x.id()).collect::<Vec<_>>()
        );

        // 4. PDFium fidelity: the saved file must open and render a tile without error.
        let mut engine =
            RenderEngine::new().expect("PDFium must load (PDFIUM_DYNAMIC_LIB_PATH set)");
        engine
            .open_document(work.clone(), "c1-saved".into())
            .expect("PDFium open saved C1");
        let req = TileRequest {
            doc_id: "c1-saved".into(),
            ..one_tile(0)
        };
        let tile = engine
            .render_tile(&req)
            .expect("PDFium render tile from saved C1");
        assert!(
            !tile.png_base64.is_empty(),
            "rendered tile must be non-empty"
        );
        assert!(
            tile.width_px > 0 && tile.height_px > 0,
            "tile dimensions must be non-zero"
        );
    }
}
