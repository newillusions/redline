//! Pure measurement math — f64, PDF user space throughout (spec §7).
//! `raw_measure` is always scale-independent (PDF points or points²).
//! Recomputing: raw × ratio → computed_quantity in `unit`.

use crate::geometry::PdfPoint;
use crate::markup::{Markup, MarkupType, Measurement, RiseDrop};
use crate::takeoff::scale::{ScaleRecord, ScaleTarget};

/// Compute the length between sequential polyline vertices (sum of segment lengths),
/// in PDF user space (points). Returns the raw_measure.
pub fn compute_length(pts: &[PdfPoint]) -> f64 {
    if pts.len() < 2 {
        return 0.0;
    }
    pts.windows(2)
        .map(|w| {
            let dx = w[1].x - w[0].x;
            let dy = w[1].y - w[0].y;
            (dx * dx + dy * dy).sqrt()
        })
        .sum()
}

/// Compute the signed area of a polygon (shoelace), absolute value, in PDF points².
/// Returns raw_measure.
pub fn compute_area(pts: &[PdfPoint]) -> f64 {
    if pts.len() < 3 {
        return 0.0;
    }
    // Shoelace formula — absolute value for CCW or CW winding
    let n = pts.len();
    let sum: f64 = (0..n)
        .map(|i| {
            let j = (i + 1) % n;
            pts[i].x * pts[j].y - pts[j].x * pts[i].y
        })
        .sum();
    sum.abs() / 2.0
}

/// Metres per unit for the length units a scale can carry. `None` = unknown unit.
pub fn metres_per_unit(unit: &str) -> Option<f64> {
    match unit.trim().to_ascii_lowercase().as_str() {
        "mm" => Some(0.001),
        "cm" => Some(0.01),
        "m" => Some(1.0),
        "km" => Some(1000.0),
        "in" | "\"" => Some(0.0254),
        "ft" | "'" => Some(0.3048),
        "yd" => Some(0.9144),
        _ => None,
    }
}

/// Convert `amount` between length units. Same unit passes through unchanged (also for
/// units not in the table); two different units are converted only when both are known.
pub fn convert_length(amount: f64, from: &str, to: &str) -> Option<f64> {
    if from.trim().eq_ignore_ascii_case(to.trim()) {
        return Some(amount);
    }
    Some(amount * metres_per_unit(from)? / metres_per_unit(to)?)
}

/// Total rise/drop contribution in `unit`: every run adds its magnitude (a drop is extra
/// cable, not a subtraction). A run whose unit cannot be converted to `unit` is skipped
/// and counted in the second return value so callers can surface it instead of silently
/// under-reporting.
pub fn rise_drop_total(runs: &[RiseDrop], unit: &str) -> (f64, usize) {
    let mut total = 0.0;
    let mut unconverted = 0;
    for r in runs {
        match convert_length(r.amount.abs(), &r.unit, unit) {
            Some(v) if v.is_finite() => total += v,
            _ => unconverted += 1,
        }
    }
    (total, unconverted)
}

/// Recompute `computed_quantity` on a measurement using the given scale ratio
/// (real-world units per PDF point). For area units (ending in "2", e.g. "m2", "ft2")
/// the ratio is squared before applying. Rise/drop runs (polylength) are added in `unit`.
/// Mutates the measurement in place.
pub fn recompute_measurement(m: &mut Measurement, ratio: f64, unit: &str) {
    let effective_ratio = if unit.ends_with('2') {
        ratio * ratio
    } else {
        ratio
    };
    let (vertical, _) = rise_drop_total(&m.rise_drop, unit);
    m.computed_quantity = m.raw_measure * effective_ratio + vertical;
    m.unit = unit.to_string();
}

/// Recompute one markup's measurement under `scale`, mirroring the creation formulas in
/// the frontend (`recomputeMeasurement` in `measurement-tools.ts`): length / radius /
/// perimeter = raw x ratio (+ rise/drop for length), area = raw x ratio^2, volume =
/// raw x ratio^2 x depth. Angle and count are scale-independent and left untouched.
pub fn recompute_markup(m: &mut Markup, scale: &ScaleRecord) {
    let ty = m.markup_type;
    let Some(meas) = m.measurement.as_mut() else {
        return;
    };
    if !scale.ratio.is_finite() || scale.ratio <= 0.0 {
        return;
    }
    let r = scale.ratio;
    match ty {
        MarkupType::MeasurementLength => {
            let (vertical, _) = rise_drop_total(&meas.rise_drop, &scale.unit);
            meas.computed_quantity = meas.raw_measure * r + vertical;
            meas.unit = scale.unit.clone();
        }
        MarkupType::MeasurementRadius | MarkupType::MeasurementPerimeter => {
            meas.computed_quantity = meas.raw_measure * r;
            meas.unit = scale.unit.clone();
        }
        MarkupType::MeasurementArea => {
            meas.computed_quantity = meas.raw_measure * r * r;
            meas.unit = scale.unit.clone();
        }
        MarkupType::MeasurementVolume => {
            meas.computed_quantity = meas.raw_measure * r * r * meas.depth.unwrap_or(1.0);
            meas.unit = format!("{}\u{b3}", scale.unit);
        }
        _ => return,
    }
    meas.scale_ref = Some(scale.id.clone());
}

/// Recompute every scaled measurement in `markups` under the scale that applies to its
/// page (page-specific first, else document default). Pages with no applicable scale are
/// left as they are. A measurement taken under a DIFFERENT scale that still exists (the
/// user deliberately picked it) keeps its scale; one whose `scale_ref` is empty or points
/// at a scale that was replaced by a recalibration is brought under the page's scale.
/// Returns how many markups were updated.
pub fn recompute_all(markups: &mut [Markup], scales: &[ScaleRecord]) -> usize {
    let mut n = 0;
    for m in markups.iter_mut() {
        if m.measurement.is_none() {
            continue;
        }
        let scale = scales
            .iter()
            .find(|s| matches!(s.applies_to, ScaleTarget::Page { page } if page == m.page))
            .or_else(|| {
                scales
                    .iter()
                    .find(|s| matches!(s.applies_to, ScaleTarget::DocumentDefault))
            });
        if let Some(scale) = scale {
            let pinned_elsewhere = m
                .measurement
                .as_ref()
                .and_then(|x| x.scale_ref.as_deref())
                .is_some_and(|id| id != scale.id && scales.iter().any(|s| s.id == id));
            if pinned_elsewhere {
                continue;
            }
            recompute_markup(m, scale);
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PdfPoint;
    use std::collections::BTreeMap;

    fn pt(x: f64, y: f64) -> PdfPoint {
        PdfPoint { x, y }
    }

    #[test]
    fn length_two_points() {
        // 3-4-5 right triangle in PDF points
        let pts = vec![pt(0.0, 0.0), pt(3.0, 4.0)];
        let raw = compute_length(&pts);
        assert!((raw - 5.0).abs() < 1e-9, "expected 5.0, got {raw}");
    }

    #[test]
    fn length_three_segments() {
        let pts = vec![pt(0.0, 0.0), pt(1.0, 0.0), pt(1.0, 1.0), pt(0.0, 1.0)];
        let raw = compute_length(&pts);
        assert!((raw - 3.0).abs() < 1e-9, "expected 3.0, got {raw}");
    }

    #[test]
    fn length_single_point_is_zero() {
        assert_eq!(compute_length(&[pt(5.0, 5.0)]), 0.0);
    }

    #[test]
    fn area_unit_square() {
        // Unit square: (0,0)-(1,0)-(1,1)-(0,1)
        let pts = vec![pt(0.0, 0.0), pt(1.0, 0.0), pt(1.0, 1.0), pt(0.0, 1.0)];
        let raw = compute_area(&pts);
        assert!((raw - 1.0).abs() < 1e-9, "expected 1.0, got {raw}");
    }

    #[test]
    fn area_right_triangle() {
        // Base 6, height 4 → area = 12
        let pts = vec![pt(0.0, 0.0), pt(6.0, 0.0), pt(0.0, 4.0)];
        let raw = compute_area(&pts);
        assert!((raw - 12.0).abs() < 1e-9, "expected 12.0, got {raw}");
    }

    #[test]
    fn area_fewer_than_3_points_is_zero() {
        assert_eq!(compute_area(&[pt(0.0, 0.0), pt(1.0, 0.0)]), 0.0);
    }

    #[test]
    fn recompute_length_measurement() {
        // ratio = 0.001 means 1 PDF point = 0.001 m → 1000 pt = 1 m
        let mut m = Measurement {
            scale_ref: Some("sc1".into()),
            raw_measure: 1000.0, // 1000 PDF points
            unit: "m".into(),
            computed_quantity: 0.0,
            depth: None,
            count_value: None,
            custom_columns: BTreeMap::new(),
            rise_drop: Vec::new(),
        };
        recompute_measurement(&mut m, 0.001, "m");
        assert!((m.computed_quantity - 1.0).abs() < 1e-9, "expected 1.0 m");
        assert_eq!(m.unit, "m");
    }

    #[test]
    fn recompute_area_measurement() {
        // ratio = 0.001 → area ratio = 0.001² = 1e-6
        let mut m = Measurement {
            scale_ref: Some("sc1".into()),
            raw_measure: 1_000_000.0, // 1e6 points²
            unit: "m2".into(),
            computed_quantity: 0.0,
            depth: None,
            count_value: None,
            custom_columns: BTreeMap::new(),
            rise_drop: Vec::new(),
        };
        recompute_measurement(&mut m, 0.001, "m2");
        assert!((m.computed_quantity - 1.0).abs() < 1e-9, "expected 1.0 m²");
    }

    fn rd(segment: u32, amount: f64, unit: &str) -> RiseDrop {
        RiseDrop {
            segment,
            amount,
            unit: unit.into(),
        }
    }

    fn length_meas(raw: f64, rise: Vec<RiseDrop>) -> Measurement {
        Measurement {
            scale_ref: None,
            raw_measure: raw,
            unit: "pt".into(),
            computed_quantity: 0.0,
            depth: None,
            count_value: None,
            custom_columns: BTreeMap::new(),
            rise_drop: rise,
        }
    }

    #[test]
    fn rise_and_drop_both_add_to_length() {
        // 1000 pt at 0.001 m/pt = 1 m, plus a 2 m rise and a 1.5 m drop = 4.5 m.
        let mut m = length_meas(1000.0, vec![rd(0, 2.0, "m"), rd(1, -1.5, "m")]);
        recompute_measurement(&mut m, 0.001, "m");
        assert!(
            (m.computed_quantity - 4.5).abs() < 1e-9,
            "{}",
            m.computed_quantity
        );
    }

    #[test]
    fn rise_drop_converts_between_known_units_and_flags_unknown_ones() {
        // 2 ft rise under a metre scale = 0.6096 m.
        let mut m = length_meas(0.0, vec![rd(0, 2.0, "ft")]);
        recompute_measurement(&mut m, 0.001, "m");
        assert!((m.computed_quantity - 0.6096).abs() < 1e-9);
        // An unknown unit pair is skipped and counted, never guessed.
        let (t, bad) = rise_drop_total(&[rd(0, 5.0, "bay"), rd(1, 1.0, "m")], "m");
        assert_eq!((t, bad), (1.0, 1));
        // Same unknown unit on both sides passes through.
        assert_eq!(rise_drop_total(&[rd(0, 5.0, "bay")], "bay"), (5.0, 0));
    }

    #[test]
    fn rise_drop_ignores_non_finite_amounts() {
        let (t, bad) = rise_drop_total(&[rd(0, f64::NAN, "m"), rd(1, f64::INFINITY, "m")], "m");
        assert_eq!(t, 0.0);
        assert_eq!(bad, 2);
    }

    fn scale(id: &str, target: ScaleTarget, ratio: f64, unit: &str) -> ScaleRecord {
        let mut s = ScaleRecord::new(
            target,
            crate::takeoff::scale::ScaleMethod::Preset,
            ratio,
            unit.into(),
            "t".into(),
            2,
        );
        s.id = id.into();
        s
    }

    fn length_markup(page: u32, raw: f64, scale_ref: Option<&str>) -> Markup {
        use crate::markup::{Appearance, MarkupGeometry, UserRef};
        let mut m = Markup::new(
            MarkupType::MeasurementLength,
            page,
            MarkupGeometry::Polyline(vec![pt(0.0, 0.0), pt(raw, 0.0)]),
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "T".into(),
            },
        );
        let mut meas = length_meas(raw, vec![rd(0, 1.0, "m")]);
        meas.scale_ref = scale_ref.map(String::from);
        meas.unit = "m".into();
        m.measurement = Some(meas);
        m
    }

    #[test]
    fn recalibration_recomputes_measurements_under_the_new_scale() {
        // Measured at 0.001 m/pt (old scale "a"); recalibrated to 0.002 m/pt (new "b").
        let mut ms = vec![length_markup(0, 1000.0, Some("a"))];
        let scales = vec![scale("b", ScaleTarget::DocumentDefault, 0.002, "m")];
        let n = recompute_all(&mut ms, &scales);
        assert_eq!(n, 1);
        let meas = ms[0].measurement.as_ref().unwrap();
        // 1000 pt x 0.002 + 1 m rise = 3 m.
        assert!(
            (meas.computed_quantity - 3.0).abs() < 1e-9,
            "{}",
            meas.computed_quantity
        );
        assert_eq!(meas.scale_ref.as_deref(), Some("b"));
    }

    #[test]
    fn a_page_scale_beats_the_default_and_a_deliberately_pinned_scale_is_kept() {
        let scales = vec![
            scale("d", ScaleTarget::DocumentDefault, 0.001, "m"),
            scale("p1", ScaleTarget::Page { page: 1 }, 0.01, "m"),
            scale("other", ScaleTarget::Page { page: 5 }, 0.5, "m"),
        ];
        let mut ms = vec![
            length_markup(0, 1000.0, None),          // default
            length_markup(1, 1000.0, Some("zzz")),   // dangling ref -> page scale
            length_markup(0, 1000.0, Some("other")), // pinned to an existing other scale
        ];
        ms[2].measurement.as_mut().unwrap().computed_quantity = 123.0;
        recompute_all(&mut ms, &scales);
        let q = |i: usize| ms[i].measurement.as_ref().unwrap().computed_quantity;
        assert!((q(0) - 2.0).abs() < 1e-9); // 1 + 1 rise
        assert!((q(1) - 11.0).abs() < 1e-9); // 10 + 1
        assert_eq!(q(2), 123.0);
    }

    #[test]
    fn recompute_leaves_count_and_angle_alone_and_rejects_bad_ratios() {
        let mut c = length_markup(0, 5.0, None);
        c.markup_type = MarkupType::MeasurementAngle;
        c.measurement.as_mut().unwrap().computed_quantity = 42.0;
        let mut l = length_markup(0, 5.0, None);
        l.measurement.as_mut().unwrap().computed_quantity = 9.0;
        let bad = scale("x", ScaleTarget::DocumentDefault, f64::NAN, "m");
        let good = scale("y", ScaleTarget::DocumentDefault, 1.0, "m");
        recompute_markup(&mut c, &good);
        recompute_markup(&mut l, &bad);
        assert_eq!(c.measurement.unwrap().computed_quantity, 42.0);
        assert_eq!(l.measurement.unwrap().computed_quantity, 9.0);
    }
}
