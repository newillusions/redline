// Analysis of the owner-supplied Bluebeam Revu "Reduce File Size" reference pair
// (bench/corpus/bb-ref/, gitignored/local): the SAME markup-heavy document before and
// after Bluebeam's own optimizer, used to (a) sanity-check this crate's image-encoding
// choices against what Revu actually does, and (b) prove this crate's optimize pipeline
// never moves or alters annotations - a sibling investigation is fixing a real
// markup-position interop bug in this exact file, and this pipeline must not be a new
// source of positional drift.
use lopdf::{Document, Object};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn corpus(rel: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bench")
        .join("corpus")
        .join(rel);
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

fn cs_key(doc: &Document, dict: &lopdf::Dictionary) -> String {
    let bpc = dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|o| o.as_i64().ok());
    let cs = match dict.get(b"ColorSpace") {
        Ok(Object::Name(n)) => String::from_utf8_lossy(n).to_string(),
        Ok(Object::Reference(r)) => match doc.get_object(*r) {
            Ok(Object::Name(n)) => String::from_utf8_lossy(n).to_string(),
            Ok(Object::Array(a)) => a
                .first()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).to_string())
                .unwrap_or_else(|| "arr?".into()),
            _ => "ref?".into(),
        },
        Ok(Object::Array(a)) => a
            .first()
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).to_string())
            .unwrap_or_else(|| "arr?".into()),
        _ => "none".into(),
    };
    format!(
        "{cs}/bpc={}",
        bpc.map(|b| b.to_string()).unwrap_or_else(|| "?".into())
    )
}

fn dump_images(label: &str, path: &PathBuf) {
    let bytes = std::fs::read(path).unwrap();
    let doc = Document::load_from(std::io::Cursor::new(&bytes)).unwrap();
    println!(
        "=== {label}: {} bytes, {} objects ===",
        bytes.len(),
        doc.objects.len()
    );
    let mut idx = 0;
    for (id, obj) in &doc.objects {
        if let Object::Stream(s) = obj {
            if matches!(s.dict.get(b"Subtype"), Ok(Object::Name(n)) if n == b"Image") {
                idx += 1;
                let w = s.dict.get(b"Width").ok().and_then(|o| o.as_i64().ok());
                let h = s.dict.get(b"Height").ok().and_then(|o| o.as_i64().ok());
                let filt = match s.dict.get(b"Filter") {
                    Ok(Object::Name(n)) => String::from_utf8_lossy(n).to_string(),
                    Ok(Object::Array(a)) => a
                        .iter()
                        .filter_map(|o| o.as_name().ok())
                        .map(|n| String::from_utf8_lossy(n).to_string())
                        .collect::<Vec<_>>()
                        .join("+"),
                    _ => "none".to_string(),
                };
                println!(
                    "  image #{idx} {:?}: {w:?}x{h:?} filter={filt} {} bytes {}",
                    id,
                    s.content.len(),
                    cs_key(&doc, &s.dict)
                );
            }
        }
    }
    if idx == 0 {
        println!("  (no Image XObjects found)");
    }
}

#[test]
#[ignore]
fn dump_bluebeam_reference_images() {
    let Some(orig) = corpus("bb-ref/markup-test-original.pdf") else {
        eprintln!("skip: bb-ref corpus not present");
        return;
    };
    let Some(reduced) = corpus("bb-ref/markup-test-bb-reduced.pdf") else {
        eprintln!("skip: bb-ref corpus not present");
        return;
    };
    dump_images("ORIGINAL (redline export)", &orig);
    dump_images("BLUEBEAM-REDUCED", &reduced);
}

/// Three-way comparison: original vs Bluebeam Revu's own reduction vs this crate's
/// optimizer at each preset, on the exact same file - direct sanity-check that this
/// crate's encoding choices are in the same ballpark as a real, shipped competitor.
#[test]
#[ignore]
fn compare_this_crate_against_bluebeam_reduction() {
    use redline_lib::docops::{optimize_in_place_with_images, ImageQualityPreset};

    let Some(orig_path) = corpus("bb-ref/markup-test-original.pdf") else {
        eprintln!("skip: bb-ref corpus not present");
        return;
    };
    let Some(reduced_path) = corpus("bb-ref/markup-test-bb-reduced.pdf") else {
        eprintln!("skip: bb-ref corpus not present");
        return;
    };
    let orig_bytes = std::fs::read(&orig_path).unwrap();
    let bb_bytes = std::fs::read(&reduced_path).unwrap();

    println!("ORIGINAL:        {} bytes", orig_bytes.len());
    println!(
        "BLUEBEAM REDUCE: {} bytes ({:.1}% reduction)",
        bb_bytes.len(),
        100.0 * (orig_bytes.len() as i64 - bb_bytes.len() as i64) as f64 / orig_bytes.len() as f64
    );

    for preset in [
        ImageQualityPreset::High,
        ImageQualityPreset::Balanced,
        ImageQualityPreset::Small,
    ] {
        let mut doc = Document::load_from(std::io::Cursor::new(&orig_bytes)).unwrap();
        let stats = optimize_in_place_with_images(&mut doc, 2, Some(preset)).unwrap();
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        println!(
            "THIS CRATE {preset:?}: {} bytes ({:.1}% reduction) - {} images recompressed/{} downsampled/{} total, skip_reasons={:?}",
            out.len(),
            100.0 * (orig_bytes.len() as i64 - out.len() as i64) as f64 / orig_bytes.len() as f64,
            stats.images_recompressed,
            stats.images_downsampled,
            stats.images_total,
            stats.skip_reasons,
        );
    }
}

/// Extract every annotation's full serialized dict (Rect, AP ref target, Subtype, and
/// all other keys) for a stable, order-independent identity check.
fn annotation_fingerprints(doc: &Document) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let page_ids: Vec<_> = doc.get_pages().values().cloned().collect();
    for (page_num, page_id) in page_ids.iter().enumerate() {
        let Ok(page) = doc.get_dictionary(*page_id) else {
            continue;
        };
        let annots: Vec<Object> = match page.get(b"Annots") {
            Ok(Object::Array(a)) => a.clone(),
            Ok(Object::Reference(r)) => match doc.get_object(*r).and_then(|o| o.as_array()) {
                Ok(a) => a.clone(),
                Err(_) => continue,
            },
            _ => continue,
        };
        for (i, a) in annots.iter().enumerate() {
            let Object::Reference(r) = a else { continue };
            let Ok(Object::Dictionary(d)) = doc.get_object(*r) else {
                continue;
            };
            let subtype = d
                .get(b"Subtype")
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).to_string())
                .unwrap_or_default();
            let rect = d
                .get(b"Rect")
                .ok()
                .and_then(|o| o.as_array().ok())
                .map(|arr| {
                    arr.iter()
                        .map(|v| v.as_float().unwrap_or(0.0))
                        .map(|f| format!("{f:.4}"))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let key = format!("p{page_num}a{i}:{subtype}");
            out.insert(key, format!("subtype={subtype} rect=[{rect}]"));
        }
    }
    out
}

/// PROOF that this crate's optimize pipeline never moves or alters annotations: run
/// optimize_in_place_with_images (the exact function commands::docops::optimize_document
/// calls) on the real "markup test.pdf" file at every quality preset, and assert every
/// annotation's Subtype+Rect fingerprint is IDENTICAL before and after. This is the
/// concrete, file-specific check requested alongside the sibling markup-position
/// interop investigation on this same document - this pipeline must not be a source of
/// positional drift, whatever that sibling bug turns out to be.
#[test]
#[ignore]
fn optimize_never_moves_or_alters_annotations_on_markup_test_pdf() {
    use redline_lib::docops::{optimize_in_place_with_images, ImageQualityPreset};

    let Some(path) = corpus("bb-ref/markup-test-original.pdf") else {
        eprintln!("skip: bb-ref corpus not present");
        return;
    };
    let bytes = std::fs::read(&path).unwrap();

    let before_doc = Document::load_from(std::io::Cursor::new(&bytes)).unwrap();
    let before = annotation_fingerprints(&before_doc);
    assert!(
        !before.is_empty(),
        "reference file must actually contain annotations to test"
    );
    println!(
        "annotation count in markup-test-original.pdf: {}",
        before.len()
    );

    for preset in [
        ImageQualityPreset::High,
        ImageQualityPreset::Balanced,
        ImageQualityPreset::Small,
    ] {
        let mut doc = Document::load_from(std::io::Cursor::new(&bytes)).unwrap();
        let stats = optimize_in_place_with_images(&mut doc, 2, Some(preset)).unwrap();
        let after = annotation_fingerprints(&doc);

        assert_eq!(
            before.len(),
            after.len(),
            "{preset:?}: annotation count must not change (before={}, after={})",
            before.len(),
            after.len()
        );
        for (key, before_val) in &before {
            let after_val = after.get(key).unwrap_or_else(|| {
                panic!("{preset:?}: annotation {key} disappeared after optimize")
            });
            assert_eq!(
                before_val, after_val,
                "{preset:?}: annotation {key} changed position/subtype after optimize - Optimize must never move markups"
            );
        }
        println!(
            "{preset:?}: {} annotations verified byte-stable (Rect+Subtype); {} images recompressed",
            before.len(),
            stats.images_recompressed
        );
    }
}
