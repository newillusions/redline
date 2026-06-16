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

    /// Build a grouped pair (shared `group_id`) + one fonted Text markup on page 0.
    /// Shared by the G9 round-trip test and the external-viewer sample emitter.
    fn g9_markups() -> (uuid::Uuid, [Markup; 3]) {
        use crate::geometry::PdfPoint;
        use crate::markup::{Appearance, FontSpec, MarkupGeometry, MarkupType, UserRef};

        let user = UserRef {
            user_id: uuid::Uuid::new_v4(),
            display_name: "Alice".into(),
        };
        let gid = uuid::Uuid::new_v4();

        let mut g1 = Markup::new(
            MarkupType::Rectangle,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 60.0, y: 600.0 },
                max: PdfPoint { x: 200.0, y: 680.0 },
            },
            // Integer-valued reals on purpose: line_weight 3 and colour channels
            // 0/0/1 are exactly the values lopdf serialises without a decimal point,
            // so they catch the as_f32-vs-as_float file-round-trip regression.
            Appearance {
                color: "#0000ff".into(),
                line_weight: 3.0,
                ..Appearance::default()
            },
            user.clone(),
        );
        g1.group_id = Some(gid);

        let mut g2 = Markup::new(
            MarkupType::Ellipse,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 240.0, y: 600.0 },
                max: PdfPoint { x: 360.0, y: 680.0 },
            },
            Appearance::default(),
            user.clone(),
        );
        g2.group_id = Some(gid);

        let mut txt = Markup::new(
            MarkupType::Text,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 60.0, y: 540.0 },
                max: PdfPoint { x: 360.0, y: 564.0 },
            },
            Appearance {
                font: Some(FontSpec {
                    family: "Times New Roman".into(),
                    size_pt: 12.0,
                }),
                ..Appearance::default()
            },
            user,
        );
        txt.contents = Some("G9 sample: grouped pair above + Times /DA text".into());

        (gid, [g1, g2, txt])
    }

    /// G9 ship gate: a grouped + fonted markup set survives a REAL PDF file
    /// save -> reopen (not just dict<->dict), and the saved bytes carry the
    /// private `/RLGroup` key plus the standard base-14 `/DA` (`/TiRo ... Tf`) so
    /// foreign viewers (Acrobat/Bluebeam) render the font and ignore /RLGroup.
    #[test]
    fn g9_grouped_and_fonted_markups_survive_file_roundtrip_with_standard_keys() {
        use crate::markup::MarkupGeometry;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        let (gid, markups) = g9_markups();
        let dest = dir.path().join("out.pdf");
        save_with_markups(&src, &dest, &markups).unwrap();

        // Reopen from the real file on disk.
        let got = load_markups_from(&dest).unwrap();
        assert_eq!(got.len(), 3, "all three markups survive");
        let find = |id: uuid::Uuid| got.iter().find(|m| m.id() == id).unwrap();

        // Grouping survives and stays one group.
        assert_eq!(find(markups[0].id()).group_id, Some(gid));
        assert_eq!(find(markups[1].id()).group_id, Some(gid));
        assert_eq!(
            find(markups[0].id()).group_id,
            find(markups[1].id()).group_id,
            "both members still share the group id"
        );
        // The ungrouped text carries no group.
        assert_eq!(find(markups[2].id()).group_id, None);

        // Integer-valued reals survive the lopdf file round-trip (as_float, not as_f32):
        // geometry coordinates (§5 precision), line weight, and colour channels.
        let g1_back = find(markups[0].id());
        match &g1_back.geometry {
            MarkupGeometry::Rect { min, max } => {
                assert_eq!((min.x, min.y), (60.0, 600.0), "g1 geometry min survives");
                assert_eq!((max.x, max.y), (200.0, 680.0), "g1 geometry max survives");
            }
            other => panic!("g1 geometry changed shape: {other:?}"),
        }
        assert_eq!(g1_back.appearance.line_weight, 3.0, "line weight survives");
        assert_eq!(g1_back.appearance.color, "#0000ff", "colour survives");

        // Font survives (G7).
        let font = find(markups[2].id())
            .appearance
            .font
            .as_ref()
            .expect("font survives file round-trip");
        assert_eq!(font.family, "Times New Roman");
        assert_eq!(font.size_pt, 12.0);

        // Foreign-viewer evidence: re-parse the saved file and confirm the standard
        // base-14 `/DA` (/TiRo ... Tf) and the private `/RLGroup` are physically present
        // as PDF objects. Re-parsing (not a raw byte grep) is robust to lopdf object-stream
        // compression, and proves what a foreign viewer actually parses.
        let reparsed = Document::load(&dest).unwrap();
        let mut saw_da_tiro = false;
        let mut saw_rlgroup = false;
        for obj in reparsed.objects.values() {
            let Ok(d) = obj.as_dict() else { continue };
            if let Ok(da) = d.get(b"DA").and_then(|o| o.as_str()) {
                let s = String::from_utf8_lossy(da);
                if s.contains("/TiRo") && s.contains("Tf") {
                    saw_da_tiro = true;
                }
            }
            if d.has(b"RLGroup") {
                saw_rlgroup = true;
            }
        }
        assert!(
            saw_da_tiro,
            "saved PDF must carry a /DA with the base-14 /TiRo Tf operator"
        );
        assert!(saw_rlgroup, "saved PDF must carry the private /RLGroup key");
    }

    /// Emits a small sample PDF with a grouped pair + a Times-fonted text note to
    /// `$REDLINE_G9_SAMPLE` (default `/tmp/redline-g9-sample.pdf`) so it can be
    /// opened in Acrobat/Bluebeam to confirm /DA fonts render and /RLGroup is
    /// ignored gracefully. Not a gate — run on demand:
    ///   cargo test g9_emit_sample -- --ignored --nocapture
    #[test]
    #[ignore]
    fn g9_emit_sample_pdf_for_external_viewer_check() {
        let out = std::env::var("REDLINE_G9_SAMPLE")
            .unwrap_or_else(|_| "/tmp/redline-g9-sample.pdf".to_string());
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        let (_, markups) = g9_markups();
        save_with_markups(&src, std::path::Path::new(&out), &markups).unwrap();
        eprintln!("G9 sample PDF written to: {out}");
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
