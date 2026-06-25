//! Pure measurement math — f64, PDF user space throughout (spec §7).
//! `raw_measure` is always scale-independent (PDF points or points²).
//! Recomputing: raw × ratio → computed_quantity in `unit`.

use crate::geometry::PdfPoint;
use crate::markup::Measurement;

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

/// Recompute `computed_quantity` on a measurement using the given scale ratio
/// (real-world units per PDF point). For area units (ending in "2", e.g. "m2", "ft2")
/// the ratio is squared before applying. Mutates the measurement in place.
pub fn recompute_measurement(m: &mut Measurement, ratio: f64, unit: &str) {
    let effective_ratio = if unit.ends_with('2') {
        ratio * ratio
    } else {
        ratio
    };
    m.computed_quantity = m.raw_measure * effective_ratio;
    m.unit = unit.to_string();
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
        };
        recompute_measurement(&mut m, 0.001, "m2");
        assert!((m.computed_quantity - 1.0).abs() < 1e-9, "expected 1.0 m²");
    }
}
