//! DocOps Tauri commands — M5 baseline (spec §4, §8).
//!
//! Exposes `flatten_document`, `optimize_document`, and `redact_document`:
//! lopdf-backed PDF surgery operations that reload the render engine and write
//! the result atomically.
//!
//! All three commands reuse `apply_page_edit` from `commands::document` (the
//! same load-op-save-reload pipeline used by all page ops) to keep the save
//! contract consistent: markups are preserved, the file is written atomically,
//! the render engine is reopened, and the markup cache is invalidated.

use serde::Serialize;
use tauri::State;

use crate::commands::document::{apply_page_edit, apply_page_edit_opts};
use crate::docops::{
    flatten_annotations, optimize_in_place_with_images, redact_annotations, redact_regions,
    ImageOptimizeStats, ImageQualityPreset, RedactRegion,
};
use crate::AppState;

/// File-size before/after an `optimize_document` call, so the frontend can report what
/// actually happened (pruned/compressed vs. genuinely nothing to save) instead of giving
/// identical silent-success feedback either way - the same "success that changed nothing
/// must say so" gap `flatten_document`'s returned count closes for Flatten.
///
/// `image_stats` is all-zeros (via `ImageOptimizeStats::default()`) when `image_preset`
/// was `None` — additive to the original two-field shape, so existing frontend code
/// reading only `bytes_before`/`bytes_after` is unaffected.
#[derive(Debug, Clone, Serialize)]
pub struct OptimizeReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub image_stats: ImageOptimizeStats,
    /// `"reduced"`: the file was rewritten and is smaller. `"no_gain"`: the rewrite was not
    /// smaller than the original, so the original file was kept untouched
    /// (`bytes_after == bytes_before`). Never "optimised" without a real byte reduction.
    pub outcome: &'static str,
}

/// Flatten all annotation appearance streams in the open document into page content.
///
/// After completion:
/// - Annotations that had a Normal appearance stream (`/AP /N`) are baked into
///   the page content and removed from the PDF `/Annots` array.
/// - Annotations without an appearance stream (e.g. popup notes) are untouched.
/// - The render engine is reloaded so the viewport reflects the updated file.
///
/// Returns the number of annotations actually flattened (`0` is a real, meaningful
/// result - nothing on the document had a bakeable `/AP /N` - distinct from an error).
/// The frontend uses this to tell a genuine no-op apart from a real flatten, rather than
/// showing identical "success" feedback either way (see `obs` on the live report "flatten
/// doesn't seem to do anything" - an unqualified `Ok(())` can't distinguish the two).
///
/// # Errors
///
/// Returns an error string if the document is not open, the lopdf parse fails,
/// the lopdf parse fails, a page's content cannot be decoded (nothing is written), or the
/// atomic save/rename fails.
#[tauri::command]
pub async fn flatten_document(state: State<'_, AppState>, doc_id: String) -> Result<usize, String> {
    // `apply_page_edit`'s `op` closure is `FnOnce(&mut Document) -> anyhow::Result<()>` -
    // shared by every page op (rotate/delete/reorder/insert/redact/optimize), so it isn't
    // generic over a return value. Captured via `Arc<Mutex<_>>` instead of widening that
    // shared signature - keeps the blast radius to this one command.
    let count = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let count_write = std::sync::Arc::clone(&count);
    apply_page_edit(&state, &doc_id, move |doc| {
        let n = flatten_annotations(doc)?;
        *count_write.lock().expect("count mutex poisoned") = n;
        Ok(())
    })
    .await?;
    let n = *count.lock().expect("count mutex poisoned");
    Ok(n)
}

/// Apply redactions to the open document.
///
/// Two modes, used together in the toolbar "Apply Redactions" flow:
///
/// 1. **`regions`** (explicit coordinates): the content under each `RedactRegion` is deleted
///    from the page and a solid-black box is drawn. Pass an empty slice when not needed.
/// 2. **`apply_annots`**: when `true`, scan every page for `/Subtype /Redact` annotations,
///    redact each one's rect the same way, and remove the consumed annotations from `/Annots`.
///
/// Both modes run in a single `apply_page_edit` pass so the file is written once.
///
/// After completion the render engine is reloaded so the viewport reflects the updated file.
///
/// # Errors
///
/// Returns an error string if the document is not open, the lopdf parse fails,
/// or the atomic save/rename fails.
#[tauri::command]
pub async fn redact_document(
    state: State<'_, AppState>,
    doc_id: String,
    regions: Vec<RedactRegion>,
    apply_annots: bool,
) -> Result<(), String> {
    apply_page_edit(&state, &doc_id, move |doc| {
        if !regions.is_empty() {
            redact_regions(doc, &regions)?;
        }
        if apply_annots {
            redact_annotations(doc)?;
        }
        Ok(())
    })
    .await?;
    // Redaction deletes annotations from the file. The in-memory markup store still holds
    // them, and the next save would write them back (re-leaking their text): drop every
    // store markup that no longer exists in the redacted file.
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let password = state.markups.password(&doc_id);
    let on_disk = tokio::task::spawn_blocking(move || {
        crate::document::save::load_markups_from(&path, password.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        format!("redaction was written, but the markup list could not be refreshed ({e:#}); reopen the document")
    })?;
    let keep: std::collections::HashSet<uuid::Uuid> = on_disk.iter().map(|m| m.id()).collect();
    state.markups.purge_missing(&doc_id, &keep);
    Ok(())
}

/// Optimize the open document (prune unused objects, compress streams, and — when
/// `image_preset` is given — recompress raster images).
///
/// `level` controls object/stream optimization depth (see `docops::optimize_in_place`):
/// - 0: no-op.
/// - 1: prune unreferenced objects only (lossless, fast).
/// - 2: prune + Deflate-compress all compressable streams (default for UI button).
///
/// `image_preset` is a separate, orthogonal axis controlling the raster image
/// pipeline (see `docops::image_ops`): downsamples oversized images to the preset's
/// target DPI and re-encodes as JPEG at the preset's quality factor. `None` skips
/// image work entirely, preserving the original (pre-image-pipeline) behavior exactly.
/// This is additive rather than a change to `level`'s existing semantics, so the
/// pre-existing IPC casing test for `optimizeDocument`'s default `level` argument is
/// unaffected.
///
/// After completion the render engine is reloaded so the viewport reflects the
/// updated file. Returns the file size before and after plus a per-category image
/// breakdown (see `OptimizeReport`) - optimize has no other user-visible effect (no
/// annotations change, no viewport difference beyond raster fidelity if images were
/// recompressed), so without this the toolbar action would look identical whether it
/// saved a meaningful amount of space or nothing at all.
///
/// # Errors
///
/// Returns an error string if the document is not open, the lopdf parse fails,
/// or the atomic save/rename fails.
#[tauri::command]
pub async fn optimize_document(
    state: State<'_, AppState>,
    doc_id: String,
    level: u8,
    image_preset: Option<ImageQualityPreset>,
) -> Result<OptimizeReport, String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let bytes_before = std::fs::metadata(&path)
        .map(|m| m.len())
        .map_err(|e| format!("stat {}: {e}", path.display()))?;

    let image_stats = std::sync::Arc::new(std::sync::Mutex::new(ImageOptimizeStats::default()));
    let image_stats_write = std::sync::Arc::clone(&image_stats);
    let written = apply_page_edit_opts(
        &state,
        &doc_id,
        move |doc| {
            let stats = optimize_in_place_with_images(doc, level, image_preset)?;
            *image_stats_write
                .lock()
                .expect("image_stats mutex poisoned") = stats;
            Ok(())
        },
        false,
        true,
    )
    .await?;
    let image_stats = image_stats
        .lock()
        .expect("image_stats mutex poisoned")
        .clone();

    build_report(&path, bytes_before, image_stats, written)
}

/// Assemble the report from the measured file sizes: `bytes_after` is re-read from disk and
/// `outcome` says "reduced" only when the file was rewritten (which requires it to be
/// smaller), so the label always agrees with the byte delta.
fn build_report(
    path: &std::path::Path,
    bytes_before: u64,
    image_stats: ImageOptimizeStats,
    written: bool,
) -> Result<OptimizeReport, String> {
    let bytes_after = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat {}: {e}", path.display()))?;
    Ok(OptimizeReport {
        bytes_before,
        bytes_after,
        image_stats,
        outcome: if written { "reduced" } else { "no_gain" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::document::apply_edit_and_save_inner;
    use lopdf::{dictionary, Document, Object, Stream};

    /// Run the exact optimize pipeline (+ never-grow guard) on a file and report.
    fn run(path: &std::path::Path, preset: Option<ImageQualityPreset>) -> OptimizeReport {
        let before = std::fs::metadata(path).unwrap().len();
        let stats = std::sync::Arc::new(std::sync::Mutex::new(ImageOptimizeStats::default()));
        let sw = std::sync::Arc::clone(&stats);
        let written = apply_edit_and_save_inner(
            path,
            &[],
            &Default::default(),
            &[],
            move |doc| {
                *sw.lock().unwrap() = optimize_in_place_with_images(doc, 2, preset)?;
                Ok(())
            },
            true,
        )
        .unwrap();
        let stats = stats.lock().unwrap().clone();
        build_report(path, before, stats, written).unwrap()
    }

    /// The reported outcome must always agree with the measured byte delta.
    fn assert_honest(r: &OptimizeReport) {
        match r.outcome {
            "reduced" => assert!(r.bytes_after < r.bytes_before, "{r:?}"),
            "no_gain" => assert_eq!(r.bytes_after, r.bytes_before, "{r:?}"),
            other => panic!("unknown outcome {other}"),
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("redline-reduce-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    fn save_doc(name: &str, content: Vec<u8>, images: Vec<Stream>) -> std::path::PathBuf {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let mut xo = lopdf::Dictionary::new();
        for (i, img) in images.into_iter().enumerate() {
            let id = doc.add_object(img);
            xo.set(format!("Im{i}"), id);
        }
        let content_id = doc.add_object(Stream::new(dictionary! {}, content));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => xo },
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 },
            ),
        );
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", cat);
        let p = scratch(name);
        doc.save(&p).unwrap();
        p
    }

    fn rgb_image(w: u32, h: u32, jpeg_q: Option<u8>) -> Stream {
        let mut px = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    ((x ^ y) & 0xff) as u8,
                ]);
            }
        }
        let mut dict = dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
        };
        let data = match jpeg_q {
            Some(q) => {
                let mut out = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
                    .encode(&px, w, h, image::ExtendedColorType::Rgb8)
                    .unwrap();
                dict.set("Filter", "DCTDecode");
                out
            }
            None => px,
        };
        Stream::new(dict, data)
    }

    /// Drawing-style content: many short vector ops, stored Flate at the fastest level so
    /// there is something for the max-level re-deflate to win back (as for plotter output).
    fn vector_content() -> Vec<u8> {
        let mut s = String::new();
        for i in 0..40_000u32 {
            let x = (i * 37 % 600) as f32 + (i % 7) as f32 * 0.25;
            let y = (i * 91 % 780) as f32 + (i % 5) as f32 * 0.5;
            s.push_str(&format!(
                "{x:.2} {y:.2} m {:.2} {:.2} l S\n",
                x + 3.5,
                y + (i % 11) as f32
            ));
        }
        s.into_bytes()
    }

    fn fast_flate(data: &[u8]) -> Stream {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        let mut e = ZlibEncoder::new(Vec::new(), Compression::fast());
        e.write_all(data).unwrap();
        Stream::new(
            dictionary! { "Filter" => "FlateDecode" },
            e.finish().unwrap(),
        )
    }

    #[test]
    fn image_heavy_pdf_reports_reduced_and_matches_bytes() {
        let p = save_doc(
            "heavy.pdf",
            b"q 500 0 0 500 20 20 cm /Im0 Do Q".to_vec(),
            vec![rgb_image(1800, 1800, None)],
        );
        let r = run(&p, Some(ImageQualityPreset::Balanced));
        println!(
            "image-heavy: {} -> {} ({})",
            r.bytes_before, r.bytes_after, r.outcome
        );
        assert_eq!(r.outcome, "reduced");
        assert!(r.bytes_after * 2 < r.bytes_before, "{r:?}");
        assert!(r.image_stats.images_recompressed >= 1);
        assert_honest(&r);
    }

    #[test]
    fn vector_plot_reports_only_what_the_bytes_show() {
        // Content stored as fast-Flate (already compressed, not touched by lopdf compress()).
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let content_id = doc.add_object(fast_flate(&vector_content()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 },
            ),
        );
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", cat);
        let p = scratch("vector.pdf");
        doc.save(&p).unwrap();
        let r = run(&p, Some(ImageQualityPreset::Balanced));
        println!(
            "vector: {} -> {} ({}) streams_recompressed={}",
            r.bytes_before, r.bytes_after, r.outcome, r.image_stats.streams_recompressed
        );
        assert_eq!(
            r.outcome, "reduced",
            "max-level re-deflate must win bytes back"
        );
        assert!(r.image_stats.streams_recompressed >= 1);
        assert_honest(&r);
        // Running again has nothing left to win: must say so and leave the file alone.
        let again = run(&p, Some(ImageQualityPreset::Balanced));
        assert_eq!(again.outcome, "no_gain", "{again:?}");
        assert_honest(&again);
    }

    #[test]
    fn already_jpeg_file_never_grows_and_says_no_gain() {
        let p = save_doc(
            "jpeg.pdf",
            b"q 200 0 0 200 20 20 cm /Im0 Do Q".to_vec(),
            vec![rgb_image(300, 300, Some(30))],
        );
        let r = run(&p, Some(ImageQualityPreset::Balanced));
        println!(
            "already-jpeg: {} -> {} ({})",
            r.bytes_before, r.bytes_after, r.outcome
        );
        assert!(
            r.bytes_after <= r.bytes_before,
            "file must never grow: {r:?}"
        );
        assert_honest(&r);
    }

    /// Zlib stream of `mib` MiB of zeros, built without holding the plaintext.
    fn zero_bomb(mib: usize) -> Vec<u8> {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        let chunk = vec![0u8; 1024 * 1024];
        let mut e = ZlibEncoder::new(Vec::new(), Compression::fast());
        for _ in 0..mib {
            e.write_all(&chunk).unwrap();
        }
        e.finish().unwrap()
    }

    #[test]
    fn decompression_bomb_stream_is_skipped_not_inflated() {
        // 300 MiB of zeros in ~300 KB: over the 256 MiB per-stream cap.
        let bomb = zero_bomb(300);
        assert!(bomb.len() < 20_000_000);
        let mut doc = Document::with_version("1.7");
        let id = doc.add_object(Stream::new(
            dictionary! { "Filter" => "FlateDecode" },
            bomb.clone(),
        ));
        let started = std::time::Instant::now();
        let (n, saved) = crate::docops::recompress_flate_streams(&mut doc);
        assert_eq!((n, saved), (0, 0), "bomb must be left alone");
        assert_eq!(
            doc.get_object(id).unwrap().as_stream().unwrap().content,
            bomb
        );
        assert!(started.elapsed().as_secs() < 30, "bounded work");
    }

    #[test]
    fn oversized_and_bomb_images_are_skipped_with_a_reason() {
        let mut doc = Document::with_version("1.7");
        // Declared 50000 x 50000 = 2.5 Gpx with a few bytes of data.
        let huge = doc.add_object(Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 50000, "Height" => 50000,
                "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
            vec![0u8; 16],
        ));
        // Plausible dimensions but a 300 MiB inflate behind them.
        let bomb = doc.add_object(Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 4000, "Height" => 4000,
                "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "FlateDecode" },
            zero_bomb(300),
        ));
        let stats = crate::docops::image_ops::optimize_images_in_place(
            &mut doc,
            ImageQualityPreset::Balanced,
        )
        .unwrap();
        assert_eq!(stats.images_recompressed, 0);
        assert_eq!(stats.skip_reasons.get("too-large"), Some(&1), "{stats:?}");
        assert_eq!(
            stats.skip_reasons.get("decode-failed"),
            Some(&1),
            "{stats:?}"
        );
        assert_eq!(
            doc.get_object(huge)
                .unwrap()
                .as_stream()
                .unwrap()
                .content
                .len(),
            16
        );
        assert!(
            doc.get_object(bomb)
                .unwrap()
                .as_stream()
                .unwrap()
                .content
                .len()
                < 20_000_000
        );
    }

    /// The exact write path `redact_document` uses: a redaction that cannot be fully
    /// applied must return Err and leave the file byte-for-byte unchanged.
    #[test]
    fn redaction_with_a_residual_errors_and_leaves_the_file_unchanged() {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        );
        let form = doc.add_object(Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } } },
            b"BT /F1 12 Tf 20 150 Td (PINNED) Tj ET".to_vec(),
        ));
        let content = doc.add_object(Stream::new(dictionary! {}, b"/Fm Do".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
            "Contents" => content,
            "Resources" => dictionary! { "XObject" => dictionary! { "Fm" => form } },
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
            ),
        );
        // The same form is also reachable from a catalog key the redactor cannot rewrite.
        let cat = doc
            .add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id, "Extra" => form });
        doc.trailer.set("Root", cat);
        let p = scratch("residual.pdf");
        doc.save(&p).unwrap();
        let before = std::fs::read(&p).unwrap();
        let r = apply_edit_and_save_inner(
            &p,
            &[],
            &Default::default(),
            &[],
            |d| {
                crate::docops::redact_regions(
                    d,
                    &[crate::docops::RedactRegion {
                        page_index: 0,
                        x: 10.0,
                        y: 140.0,
                        width: 280.0,
                        height: 30.0,
                    }],
                )
            },
            false,
        );
        let e = r.expect_err("must be Err, never Ok");
        assert!(format!("{e:#}").contains("redaction refused"), "{e:#}");
        assert_eq!(std::fs::read(&p).unwrap(), before, "file must be unchanged");
    }

    /// Real-PDF measurement: `REDLINE_REDUCE_PDFS="a.pdf|b.pdf" cargo test --lib real_pdfs -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_pdfs_outcome_matches_bytes() {
        let Ok(list) = std::env::var("REDLINE_REDUCE_PDFS") else {
            return;
        };
        for src in list.split('|') {
            let p = scratch("real.pdf");
            std::fs::copy(src, &p).unwrap();
            let r = run(&p, Some(ImageQualityPreset::Balanced));
            let pct =
                (r.bytes_after as f64 - r.bytes_before as f64) / r.bytes_before as f64 * 100.0;
            println!(
                "REAL {src}: {} -> {} ({:+.2}%) outcome={} streams={} images {}/{}",
                r.bytes_before,
                r.bytes_after,
                pct,
                r.outcome,
                r.image_stats.streams_recompressed,
                r.image_stats.images_recompressed,
                r.image_stats.images_total
            );
            assert_honest(&r);
            assert!(r.bytes_after <= r.bytes_before);
        }
    }
}
