//! PDF /Measure viewport dictionary write (spec §12.7) and preset-scale picker
//! helpers (M4 S1, M3-deferred takeoff items).
//!
//! The /Measure dict embeds the calibration scale into the PDF in a standard,
//! interoperable form so Acrobat and Bluebeam can read the scale and display
//! measurements. Operates on the M3 `ScaleRecord` (ratio = real-world units per
//! PDF point).
//!
//! Precision guardrail: never call `as_f32()` on PDF Number objects when reading
//! back - always `as_float()` (f64). lopdf serialises integer-valued reals without
//! a decimal point, so they reload as Object::Integer and as_f32() would drop them.

use anyhow::{bail, Context, Result};

use super::scale::{ScaleRecord, ScaleTarget};

// ---------------------------------------------------------------------------
// Preset calibration picker (M3 deferred)
// ---------------------------------------------------------------------------

/// Return the scales applicable to a given page for the preset picker: a
/// page-specific scale plus the document default, so the user can pick a saved
/// scale without re-drawing a calibration line.
///
/// Page-specific scales for OTHER pages are excluded. Order: page-specific first,
/// then document-default.
pub fn applicable_scales(scales: &[ScaleRecord], page_idx: u32) -> Vec<&ScaleRecord> {
    let mut out: Vec<&ScaleRecord> = scales
        .iter()
        .filter(|s| matches!(s.applies_to, ScaleTarget::Page { page } if page == page_idx))
        .collect();
    out.extend(
        scales
            .iter()
            .filter(|s| matches!(s.applies_to, ScaleTarget::DocumentDefault)),
    );
    out
}

/// Find a saved scale by id (for "apply this preset" without recalibrating).
pub fn find_scale<'a>(scales: &'a [ScaleRecord], scale_id: &str) -> Option<&'a ScaleRecord> {
    scales.iter().find(|s| s.id == scale_id)
}

// ---------------------------------------------------------------------------
// PDF /Measure viewport dictionary write (spec §12.7)
// ---------------------------------------------------------------------------
//
//   /VP [<<
//     /Type /Viewport
//     /BBox [0 0 width height]
//     /Measure <<
//       /Type /Measure
//       /Subtype /RL              % rectilinear
//       /O [<< /Type /NumberFormat /U /pt /C 1 /D 1 >>]    % origin
//       /X [<< /Type /NumberFormat /U (unit) /C (ratio) /D (precision) >>]
//       /D [<< /Type /NumberFormat /U (unit) /C (ratio) /D (precision) >>]
//       /CYX 1
//     >>
//   >>]

/// Write a PDF /Measure viewport dictionary to a page, embedding the calibration
/// scale so Acrobat/Bluebeam can read it. Appends to an existing /VP array if present.
///
/// `page_idx` is 0-based. `scale.ratio` is real-world units per PDF point.
pub fn write_measure_dict(
    doc: &mut lopdf::Document,
    page_idx: u32,
    scale: &ScaleRecord,
) -> Result<()> {
    use lopdf::{dictionary, Object};

    if scale.ratio == 0.0 {
        bail!("scale {}: ratio is 0", scale.id);
    }

    let pages = doc.get_pages();
    let page_no = page_idx + 1;
    let page_id = *pages
        .get(&page_no)
        .with_context(|| format!("page_idx {page_idx} out of range"))?;

    // Page MediaBox for the viewport BBox (as_float handles Integer + Real variants).
    let (w, h) = {
        let page = doc.get_dictionary(page_id).context("page dict")?;
        match page.get(b"MediaBox") {
            Ok(obj) => {
                let arr = obj.as_array().context("/MediaBox is not an array")?;
                let w = arr.get(2).and_then(|o| o.as_float().ok()).unwrap_or(612.0) as f64;
                let h = arr.get(3).and_then(|o| o.as_float().ok()).unwrap_or(792.0) as f64;
                (w, h)
            }
            Err(_) => (612.0, 792.0),
        }
    };

    let unit_bytes = scale.unit.as_bytes().to_vec();
    let precision = scale.precision as i64;

    let nf_origin = dictionary! {
        "Type" => "NumberFormat",
        "U" => "pt",
        "C" => Object::Real(1.0_f32),
        "D" => Object::Integer(1),
    };
    let nf_scale = dictionary! {
        "Type" => "NumberFormat",
        "U" => Object::String(unit_bytes, lopdf::StringFormat::Literal),
        "C" => Object::Real(scale.ratio as f32),
        "D" => Object::Integer(precision),
    };

    let measure_dict = dictionary! {
        "Type" => "Measure",
        "Subtype" => "RL",
        "O" => Object::Array(vec![Object::Dictionary(nf_origin)]),
        "X" => Object::Array(vec![Object::Dictionary(nf_scale.clone())]),
        "D" => Object::Array(vec![Object::Dictionary(nf_scale)]),
        "CYX" => Object::Real(1.0_f32),
    };

    let viewport_dict = dictionary! {
        "Type" => "Viewport",
        "BBox" => vec![
            Object::Real(0.0_f32),
            Object::Real(0.0_f32),
            Object::Real(w as f32),
            Object::Real(h as f32),
        ],
        "Measure" => Object::Dictionary(measure_dict),
    };

    let new_vp = Object::Dictionary(viewport_dict);
    let page = doc.get_dictionary_mut(page_id).context("page dict")?;
    match page.get_mut(b"VP") {
        Ok(vp_obj) => {
            if let Ok(arr) = vp_obj.as_array_mut() {
                arr.push(new_vp);
            }
        }
        Err(_) => {
            page.set("VP", Object::Array(vec![new_vp]));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::annots::tests::one_page_doc;
    use crate::takeoff::scale::{ScaleMethod, ScaleRecord, ScaleTarget};

    fn scale(applies_to: ScaleTarget, ratio: f64, unit: &str, precision: u8) -> ScaleRecord {
        ScaleRecord::new(
            applies_to,
            ScaleMethod::Preset,
            ratio,
            unit.into(),
            "1:100".into(),
            precision,
        )
    }

    // --- preset picker ---

    #[test]
    fn applicable_scales_page_specific_plus_default() {
        let s_default = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let s_p3 = scale(ScaleTarget::Page { page: 3 }, 0.002, "m", 2);
        let s_p5 = scale(ScaleTarget::Page { page: 5 }, 0.003, "m", 2);
        let scales = vec![s_default, s_p3, s_p5];

        let got = applicable_scales(&scales, 3);
        assert_eq!(got.len(), 2, "page-3 scale + document default");
        // Page-specific listed first.
        assert!(matches!(got[0].applies_to, ScaleTarget::Page { page: 3 }));
        assert!(matches!(got[1].applies_to, ScaleTarget::DocumentDefault));
    }

    #[test]
    fn applicable_scales_only_default_for_uncalibrated_page() {
        let s_default = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let s_p3 = scale(ScaleTarget::Page { page: 3 }, 0.002, "m", 2);
        let scales = vec![s_default, s_p3];

        let got = applicable_scales(&scales, 9);
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].applies_to, ScaleTarget::DocumentDefault));
    }

    #[test]
    fn find_scale_by_id() {
        let s = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let id = s.id.clone();
        let scales = vec![s];
        assert!(find_scale(&scales, &id).is_some());
        assert!(find_scale(&scales, "nonexistent").is_none());
    }

    // --- /Measure dict write ---

    #[test]
    fn write_measure_dict_creates_vp_array() {
        let (mut doc, page_id) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.001, "mm", 4);
        write_measure_dict(&mut doc, 0, &s).unwrap();

        let page = doc.get_dictionary(page_id).unwrap();
        assert!(page.has(b"VP"), "/VP must be added to page");
        let vp_arr = page.get(b"VP").unwrap().as_array().unwrap();
        assert_eq!(vp_arr.len(), 1);
    }

    #[test]
    fn write_measure_dict_vp_has_measure_subdict_rl() {
        let (mut doc, page_id) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.005, "mm", 2);
        write_measure_dict(&mut doc, 0, &s).unwrap();

        let page = doc.get_dictionary(page_id).unwrap();
        let vp = page.get(b"VP").unwrap().as_array().unwrap()[0]
            .as_dict()
            .unwrap();
        assert!(vp.has(b"Measure"));
        let m = vp.get(b"Measure").unwrap().as_dict().unwrap();
        assert_eq!(m.get(b"Subtype").unwrap().as_name().unwrap(), b"RL");
    }

    #[test]
    fn write_measure_dict_appends_to_existing_vp() {
        let (mut doc, _) = one_page_doc();
        let s1 = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let s2 = scale(ScaleTarget::Page { page: 0 }, 0.002, "m", 2);
        write_measure_dict(&mut doc, 0, &s1).unwrap();
        write_measure_dict(&mut doc, 0, &s2).unwrap();

        let pages = doc.get_pages();
        let page = doc.get_dictionary(*pages.get(&1).unwrap()).unwrap();
        let vp_arr = page.get(b"VP").unwrap().as_array().unwrap();
        assert_eq!(vp_arr.len(), 2, "second write appends, not replaces");
    }

    #[test]
    fn write_measure_dict_out_of_range_errors() {
        let (mut doc, _) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        assert!(write_measure_dict(&mut doc, 5, &s).is_err());
    }

    #[test]
    fn write_measure_dict_zero_ratio_errors() {
        let (mut doc, _) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.0, "m", 2);
        assert!(write_measure_dict(&mut doc, 0, &s).is_err());
    }

    /// Save doc with /Measure dict, reload from disk, assert /VP + /Measure survive.
    #[test]
    fn measure_dict_survives_file_roundtrip() {
        let (mut doc, _) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.001, "mm", 4);
        write_measure_dict(&mut doc, 0, &s).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("measure.pdf");
        doc.save(&path).unwrap();

        let reloaded = lopdf::Document::load(&path).unwrap();
        let pages = reloaded.get_pages();
        let page = reloaded.get_dictionary(*pages.get(&1).unwrap()).unwrap();
        assert!(page.has(b"VP"), "/VP survives round-trip");
        let vp = page.get(b"VP").unwrap().as_array().unwrap()[0]
            .as_dict()
            .unwrap();
        assert!(vp.has(b"Measure"), "/Measure survives round-trip");
        // Scale C value readable via as_float (Integer-or-Real safe).
        let m = vp.get(b"Measure").unwrap().as_dict().unwrap();
        let x = m.get(b"X").unwrap().as_array().unwrap()[0]
            .as_dict()
            .unwrap();
        let c = x.get(b"C").unwrap().as_float().unwrap();
        assert!((c - 0.001).abs() < 1e-6, "scale C survives via as_float");
    }
}
