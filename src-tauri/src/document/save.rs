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
///
/// If the PDF is encrypted, `password` decrypts it first (lopdf's `Document::load`
/// does NOT auto-decrypt - it leaves streams/strings as ciphertext). Returns an
/// error rather than silently reading garbled ciphertext as annotation content if
/// decryption fails or no password was supplied for an encrypted file.
pub fn load_markups_from(path: &Path, password: Option<&str>) -> Result<Vec<Markup>> {
    let mut doc = Document::load(path).with_context(|| format!("load {}", path.display()))?;
    if doc.is_encrypted() {
        doc.decrypt(password.unwrap_or(""))
            .map_err(|e| anyhow::anyhow!("incorrect password for encrypted PDF: {e}"))?;
    }
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
///
/// # Encrypted PDFs are refused, not decrypted-and-resaved
/// lopdf's `decrypt()` strips the `/Encrypt` entry and has no matching
/// re-encrypt-on-save path (`encrypt()` requires a fresh `EncryptionState` and
/// errors if the document is already encrypted). Silently decrypting on load and
/// saving plain would strip the user's password protection from their file - a
/// worse outcome than refusing outright. v1 scope is open+view of encrypted PDFs
/// (see render::open_document / load_markups_from); editing and saving markups
/// back into an encrypted PDF is a known, named gap, not implemented here.
pub fn save_with_markups(src: &Path, dest: &Path, markups: &[Markup]) -> Result<()> {
    let mut doc = Document::load(src).with_context(|| format!("load {}", src.display()))?;
    if doc.is_encrypted() {
        anyhow::bail!(
            "Saving markups into a password-protected PDF is not supported yet - saving \
             would strip the file's password protection. Your changes were not saved."
        );
    }
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

/// Save an unprotected copy of an encrypted `src` PDF to `dest`, decrypted
/// with `password` and carrying no open password at all.
///
/// This is the backing implementation for the "save unprotected copy"
/// capability: it does NOT touch markups (unlike `save_with_markups`) - it
/// preserves whatever content/annotations already exist in the source file,
/// exactly as `Document::decrypt` leaves them, and writes that out plain.
/// Refuses a non-encrypted source (nothing to strip) and a wrong password
/// (never writes a partial/garbage `dest`).
pub fn save_decrypted_copy(src: &Path, dest: &Path, password: &str) -> Result<()> {
    let mut doc = Document::load(src).with_context(|| format!("load {}", src.display()))?;
    if !doc.is_encrypted() {
        anyhow::bail!("Source document is not password-protected - nothing to save unprotected.");
    }
    doc.decrypt(password)
        .map_err(|e| anyhow::anyhow!("incorrect password for encrypted PDF: {e}"))?;

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
    use crate::document::annots::tests::{encrypted_one_page_doc, one_page_doc, redline_markup};

    #[test]
    fn save_with_markups_writes_dest_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        let dest = dir.path().join("out.pdf");
        let m = redline_markup(0);
        save_with_markups(&src, &dest, std::slice::from_ref(&m)).unwrap();

        let got = load_markups_from(&dest, None).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), m.id());
        // Source untouched.
        assert!(load_markups_from(&src, None).unwrap().is_empty());
    }

    #[test]
    fn save_in_place_via_temp_swap() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("doc.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&p).unwrap();

        save_with_markups(&p, &p, &[redline_markup(0)]).unwrap();
        assert_eq!(load_markups_from(&p, None).unwrap().len(), 1);
        // No stray temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".redline-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file not cleaned: {leftovers:?}");
    }

    // -----------------------------------------------------------------------
    // Password-protected PDFs: view works, save is refused (not half-shipped).
    // -----------------------------------------------------------------------

    #[test]
    fn load_markups_from_decrypts_with_correct_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted.pdf");

        // Build the markup BEFORE encrypting, so it's present as encrypted content
        // when saved - matches how a real password-protected PDF with existing
        // annotations looks on disk.
        let mut doc = encrypted_one_page_doc_with_markup("redline-pw", "owner-pw");
        doc.save(&path).unwrap();

        let got = load_markups_from(&path, Some("redline-pw")).expect("decrypt + read");
        assert_eq!(
            got.len(),
            1,
            "existing markup must survive encrypted round-trip"
        );
    }

    #[test]
    fn load_markups_from_wrong_password_errors() {
        use crate::document::annots::tests::encrypted_one_page_doc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted.pdf");
        let mut doc = encrypted_one_page_doc("redline-pw", "owner-pw");
        doc.save(&path).unwrap();

        let err = load_markups_from(&path, Some("wrong-password"));
        assert!(
            err.is_err(),
            "wrong password must error, not return garbage"
        );
    }

    #[test]
    fn load_markups_from_no_password_on_encrypted_doc_errors() {
        use crate::document::annots::tests::encrypted_one_page_doc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted.pdf");
        let mut doc = encrypted_one_page_doc("redline-pw", "owner-pw");
        doc.save(&path).unwrap();

        let err = load_markups_from(&path, None);
        assert!(
            err.is_err(),
            "no password on an encrypted doc must error, never silently read ciphertext"
        );
    }

    #[test]
    fn save_with_markups_refuses_encrypted_document() {
        use crate::document::annots::tests::encrypted_one_page_doc;

        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("encrypted.pdf");
        let mut doc = encrypted_one_page_doc("redline-pw", "owner-pw");
        doc.save(&src).unwrap();

        let dest = dir.path().join("out.pdf");
        let err = save_with_markups(&src, &dest, &[redline_markup(0)]);

        assert!(
            err.is_err(),
            "save on an encrypted doc must be refused, not silently strip protection"
        );
        assert!(
            !dest.exists(),
            "refused save must not leave a partial/unprotected output file"
        );
    }

    // -----------------------------------------------------------------------
    // save_decrypted_copy: the "save unprotected copy" capability.
    // -----------------------------------------------------------------------

    #[test]
    fn save_decrypted_copy_produces_a_file_openable_with_no_password() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("protected.pdf");
        let mut doc = encrypted_one_page_doc("redline-pw", "owner-pw");
        doc.save(&src).unwrap();

        let dest = dir.path().join("protected_unprotected.pdf");
        save_decrypted_copy(&src, &dest, "redline-pw").expect("decrypt + save copy");

        // The copy opens with lopdf and reports as NOT encrypted - no password needed.
        let reopened = Document::load(&dest).unwrap();
        assert!(
            !reopened.is_encrypted(),
            "saved copy must carry no open password"
        );

        // The original source file is untouched (still encrypted).
        let original = Document::load(&src).unwrap();
        assert!(original.is_encrypted(), "source file must be unmodified");
    }

    #[test]
    fn save_decrypted_copy_wrong_password_errors_and_leaves_no_dest() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("protected.pdf");
        let mut doc = encrypted_one_page_doc("redline-pw", "owner-pw");
        doc.save(&src).unwrap();

        let dest = dir.path().join("out.pdf");
        let err = save_decrypted_copy(&src, &dest, "wrong-password");

        assert!(err.is_err(), "wrong password must error");
        assert!(
            !dest.exists(),
            "a failed decrypt must not leave a dest file"
        );
    }

    #[test]
    fn save_decrypted_copy_refuses_a_non_encrypted_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("plain.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        let dest = dir.path().join("out.pdf");
        let err = save_decrypted_copy(&src, &dest, "irrelevant");
        assert!(
            err.is_err(),
            "a non-encrypted source has nothing to save unprotected"
        );
        assert!(!dest.exists());
    }

    /// Same fixture as `encrypted_one_page_doc`, but with one redline markup written
    /// into the /Annots array BEFORE encrypting - so decrypt-then-read exercises a
    /// realistic "existing annotations in an encrypted file" scenario.
    fn encrypted_one_page_doc_with_markup(user_password: &str, owner_password: &str) -> Document {
        let (mut doc, page_id) = one_page_doc();
        write_markups(&mut doc, &[redline_markup(0)]).unwrap();
        // Re-derive the page id lookup isn't needed further; write_markups already
        // targeted page 0 via the /Kids array set up by one_page_doc().
        let _ = page_id;

        let id = lopdf::Object::string_literal(b"redline-test-fixture-id".to_vec());
        doc.trailer.set("ID", vec![id.clone(), id]);

        use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
        let state = EncryptionState::try_from(EncryptionVersion::V2 {
            document: &doc,
            owner_password,
            user_password,
            key_length: 128,
            permissions: Permissions::all(),
        })
        .expect("build encryption state for test fixture");
        doc.encrypt(&state).expect("encrypt test fixture");
        doc
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
        let got = load_markups_from(&dest, None).unwrap();
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

    /// One markup per G9 Bluebeam-interop defect (2026-07-12 fix), for the human
    /// Acrobat/Bluebeam re-check. Run on demand:
    ///   cargo test g9_emit_interop_sample -- --ignored --nocapture
    /// Writes to `$REDLINE_G9_INTEROP_SAMPLE` (default `/tmp/redline-g9-interop-sample.pdf`).
    #[test]
    #[ignore]
    fn g9_emit_interop_sample_pdf_for_external_viewer_check() {
        use crate::geometry::PdfPoint;
        use crate::markup::{
            Appearance, CountSet, CountSymbol, FontSpec, MarkupGeometry, MarkupType, UserRef,
        };

        let out = std::env::var("REDLINE_G9_INTEROP_SAMPLE")
            .unwrap_or_else(|_| "/tmp/redline-g9-interop-sample.pdf".to_string());
        let user = UserRef {
            user_id: uuid::Uuid::new_v4(),
            display_name: "Alice".into(),
        };
        let mk = |t, g, a: Appearance| Markup::new(t, 0, g, a, user.clone());

        let mut markups: Vec<Markup> = Vec::new();

        // Defect 1: a wide, short resized FreeText box (the padded-BBox rescale case).
        let mut ft = mk(
            MarkupType::Text,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 60.0, y: 690.0 },
                max: PdfPoint { x: 430.0, y: 730.0 },
            },
            Appearance {
                color: "#cc3300".into(),
                fill: Some("#ddeeff".into()),
                outline_color: Some("#0044aa".into()),
                font: Some(FontSpec {
                    family: "Helvetica".into(),
                    size_pt: 22.0,
                }),
                ..Appearance::default()
            },
        );
        ft.contents = Some("Resized FreeText - should stay this size in Bluebeam".into());
        markups.push(ft);

        // Defect 2: a revision cloud (should show scallops, not a zigzag).
        markups.push(mk(
            MarkupType::Cloud,
            MarkupGeometry::Polyline(vec![
                PdfPoint { x: 70.0, y: 560.0 },
                PdfPoint { x: 300.0, y: 600.0 },
                PdfPoint { x: 320.0, y: 470.0 },
                PdfPoint { x: 90.0, y: 450.0 },
            ]),
            Appearance {
                color: "#cc0000".into(),
                line_weight: 2.0,
                ..Appearance::default()
            },
        ));

        // Defect 3: a translucent highlight over count-set text (must stay readable).
        markups.push(mk(
            MarkupType::Highlight,
            MarkupGeometry::Quads(vec![[
                PdfPoint { x: 70.0, y: 410.0 },
                PdfPoint { x: 360.0, y: 410.0 },
                PdfPoint { x: 70.0, y: 392.0 },
                PdfPoint { x: 360.0, y: 392.0 },
            ]]),
            Appearance {
                color: "#ffdd00".into(),
                opacity: 0.4,
                ..Appearance::default()
            },
        ));

        // Defect 4: a plain line + arrow with NO note (must carry no comment in Bluebeam).
        markups.push(mk(
            MarkupType::Line,
            MarkupGeometry::Polyline(vec![
                PdfPoint { x: 70.0, y: 350.0 },
                PdfPoint { x: 300.0, y: 350.0 },
            ]),
            Appearance {
                color: "#008800".into(),
                line_weight: 2.0,
                ..Appearance::default()
            },
        ));
        markups.push(mk(
            MarkupType::Arrow,
            MarkupGeometry::Polyline(vec![
                PdfPoint { x: 70.0, y: 320.0 },
                PdfPoint { x: 300.0, y: 300.0 },
            ]),
            Appearance {
                color: "#008800".into(),
                line_weight: 2.0,
                ..Appearance::default()
            },
        ));

        // Defect 5: count markers (must appear in Bluebeam, not vanish).
        for (i, (sym, color)) in [
            (CountSymbol::Circle, "#ee0000"),
            (CountSymbol::Square, "#0000ee"),
            (CountSymbol::Triangle, "#008800"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut c = mk(
                MarkupType::MeasurementCount,
                MarkupGeometry::Point(PdfPoint {
                    x: 400.0 + (i as f64) * 40.0,
                    y: 300.0,
                }),
                Appearance {
                    color: color.into(),
                    ..Appearance::default()
                },
            );
            c.count_set = Some(CountSet {
                id: uuid::Uuid::new_v4(),
                name: format!("Count-{i}"),
                color: color.into(),
                symbol: sym,
            });
            markups.push(c);
        }

        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();
        save_with_markups(&src, std::path::Path::new(&out), &markups).unwrap();
        eprintln!(
            "G9 interop sample PDF ({} markups) written to: {out}",
            markups.len()
        );
    }

    /// Bluebeam interop ship gate: a saved markup's `/AP /N` appearance stream survives a
    /// REAL file save -> reopen (not just dict<->dict) as a resolvable indirect Form
    /// XObject with a non-empty content stream. This is what makes redline-authored PDFs
    /// render/persist correctly in a strict external viewer that does not synthesize
    /// appearances from geometry - Acrobat/PDFium already tolerate a missing `/AP`,
    /// Bluebeam does not.
    #[test]
    fn saved_markup_ap_stream_survives_real_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        let m = redline_markup(0);
        let dest = dir.path().join("out.pdf");
        save_with_markups(&src, &dest, std::slice::from_ref(&m)).unwrap();

        // Reopen from the real file on disk and walk the page's /Annots -> /AP -> /N chain.
        let reopened = Document::load(&dest).unwrap();
        let page_id = *reopened.get_pages().values().next().unwrap();
        let page = reopened.get_dictionary(page_id).unwrap();
        let annots = match page.get(b"Annots").unwrap() {
            lopdf::Object::Array(a) => a.clone(),
            lopdf::Object::Reference(r) => {
                reopened.get_object(*r).unwrap().as_array().unwrap().clone()
            }
            other => panic!("unexpected /Annots shape: {other:?}"),
        };
        assert_eq!(annots.len(), 1);
        let annot_dict = match &annots[0] {
            lopdf::Object::Reference(r) => reopened.get_dictionary(*r).unwrap(),
            other => panic!("expected an indirect annotation, got {other:?}"),
        };

        let ap = annot_dict
            .get(b"AP")
            .expect("/AP must survive the file round-trip")
            .as_dict()
            .unwrap();
        let n_ref = match ap.get(b"N").expect("/AP /N must be present") {
            lopdf::Object::Reference(r) => *r,
            other => panic!("/AP /N must be an indirect reference, got {other:?}"),
        };
        let stream = match reopened.get_object(n_ref).expect("/AP /N must resolve") {
            lopdf::Object::Stream(s) => s,
            other => panic!("/AP /N must resolve to a Stream, got {other:?}"),
        };
        assert_eq!(
            stream.dict.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Form"
        );
        assert!(
            !stream.content.is_empty(),
            "appearance content must survive the file round-trip"
        );
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
        let got = load_markups_from(&work, None).expect("load_markups_from saved C1");
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
            .open_document(work.clone(), "c1-saved".into(), None)
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

    // --- G9 Bluebeam-interop: /AP /BBox must equal the annotation /Rect ------------------
    //
    // A strict viewer (Bluebeam) maps the appearance Form's /BBox into the annotation /Rect
    // (ISO 32000-1 12.5.5). When they disagree the whole appearance is scaled - the G9
    // "resized FreeText renders tiny" and "count markers vanish" defects. These tests save a
    // real PDF and re-parse it with lopdf (independent of redline's own reader), walking
    // /Annots -> /AP -> /N, and assert /BBox == /Rect for the affected subtypes.

    /// Re-parse `dest` and return every managed annotation's (subtype, /Rect, /AP /N /BBox).
    fn annot_rect_and_bbox(dest: &std::path::Path) -> Vec<(String, [f64; 4], [f64; 4])> {
        let doc = Document::load(dest).unwrap();
        let mut out = Vec::new();
        for page_id in doc.get_pages().values() {
            let page = doc.get_dictionary(*page_id).unwrap();
            let annots = match page.get(b"Annots") {
                Ok(lopdf::Object::Array(a)) => a.clone(),
                Ok(lopdf::Object::Reference(r)) => {
                    doc.get_object(*r).unwrap().as_array().unwrap().clone()
                }
                _ => continue,
            };
            for entry in annots {
                let dict = match &entry {
                    lopdf::Object::Reference(r) => doc.get_dictionary(*r).unwrap(),
                    lopdf::Object::Dictionary(d) => d,
                    _ => continue,
                };
                let Ok(subtype) = dict.get(b"Subtype").and_then(|o| o.as_name()) else {
                    continue;
                };
                let subtype = String::from_utf8_lossy(subtype).into_owned();
                let reals = |o: &lopdf::Object| -> [f64; 4] {
                    let a = o.as_array().unwrap();
                    [
                        a[0].as_float().unwrap() as f64,
                        a[1].as_float().unwrap() as f64,
                        a[2].as_float().unwrap() as f64,
                        a[3].as_float().unwrap() as f64,
                    ]
                };
                let rect = reals(dict.get(b"Rect").unwrap());
                let ap = dict.get(b"AP").unwrap().as_dict().unwrap();
                let n_ref = match ap.get(b"N").unwrap() {
                    lopdf::Object::Reference(r) => *r,
                    _ => panic!("/AP /N must be indirect"),
                };
                let stream = match doc.get_object(n_ref).unwrap() {
                    lopdf::Object::Stream(s) => s,
                    _ => panic!("/AP /N must be a stream"),
                };
                let bbox = reals(stream.dict.get(b"BBox").unwrap());
                out.push((subtype, rect, bbox));
            }
        }
        out
    }

    fn approx_eq(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    /// Defect 1: a resized FreeText's /AP /BBox must equal its /Rect (identity map, no
    /// Bluebeam down-scaling). Uses a wide, short box - the geometry the old padded BBox
    /// distorted most.
    #[test]
    fn g9_freetext_ap_bbox_equals_rect() {
        use crate::geometry::PdfPoint;
        use crate::markup::{Appearance, FontSpec, MarkupGeometry, MarkupType, UserRef};

        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        let mut m = Markup::new(
            MarkupType::Text,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 100.0, y: 500.0 },
                max: PdfPoint { x: 420.0, y: 540.0 },
            },
            Appearance {
                font: Some(FontSpec {
                    family: "Helvetica".into(),
                    size_pt: 24.0,
                }),
                ..Appearance::default()
            },
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        );
        m.contents = Some("Resized callout text".into());

        let dest = dir.path().join("out.pdf");
        save_with_markups(&src, &dest, std::slice::from_ref(&m)).unwrap();

        let found = annot_rect_and_bbox(&dest);
        let ft = found
            .iter()
            .find(|(s, ..)| s == "FreeText")
            .expect("a FreeText annot");
        assert!(
            approx_eq(ft.1, ft.2),
            "FreeText /Rect {:?} must equal /AP /BBox {:?} so Bluebeam does not rescale it",
            ft.1,
            ft.2
        );
    }

    /// Defect 5: a count marker must save with a non-zero /Rect, a Stamp subtype, and an
    /// /AP whose /BBox equals that /Rect - so a strict viewer renders it instead of dropping
    /// the old zero-rect FreeText.
    #[test]
    fn g9_count_marker_saves_with_stamp_ap_and_matching_bbox() {
        use crate::geometry::PdfPoint;
        use crate::markup::{Appearance, MarkupGeometry, MarkupType, UserRef};

        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        let m = Markup::new(
            MarkupType::MeasurementCount,
            0,
            MarkupGeometry::Point(PdfPoint { x: 300.0, y: 400.0 }),
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        );

        let dest = dir.path().join("out.pdf");
        save_with_markups(&src, &dest, std::slice::from_ref(&m)).unwrap();

        let found = annot_rect_and_bbox(&dest);
        let (subtype, rect, bbox) = found
            .iter()
            .find(|(s, ..)| s == "Stamp")
            .expect("a Stamp annot for the count marker");
        assert_eq!(subtype, "Stamp");
        assert!(
            (rect[2] - rect[0]).abs() > 1.0 && (rect[3] - rect[1]).abs() > 1.0,
            "non-zero count /Rect, got {rect:?}"
        );
        assert!(
            approx_eq(*rect, *bbox),
            "count /Rect {rect:?} must equal /AP /BBox {bbox:?}"
        );

        // And it still reloads as a MeasurementCount at the original point.
        let back = load_markups_from(&dest, None).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].markup_type, MarkupType::MeasurementCount);
    }
}
