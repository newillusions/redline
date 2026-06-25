//! Geometry module — vector path extraction + spatial snap-target index (spec §4, §5).
//!
//! # Precision-critical invariant (spec §5)
//! Snapping and measurement NEVER read raster tiles. All snap-target math runs in
//! PDF user space at f64, independent of zoom or tile resolution.
//!
//! # PDFium path iteration
//! Use pdfium-render's *transformed* path-segment iteration so coordinates compose
//! parent matrices correctly. Paths nested in Form XObjects otherwise return coordinates
//! near (0,0) — the transformed variant accounts for the full CTM stack. (spec §5, §20 invariant 3)
//!
//! # M1 scope
//! Stub — structure and types only. Full extraction + rstar spatial index lands in M2
//! alongside the markup overlay (snap is needed for accurate markup placement).

use rstar::{PointDistance, RTree, RTreeObject, AABB};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Snap target types
// ---------------------------------------------------------------------------

/// A 2D point in PDF user space (origin bottom-left, units = points).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PdfPoint {
    pub x: f64,
    pub y: f64,
}

/// Category of snap target — mirrors Bluebeam's snap modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapKind {
    /// Path endpoint or polygon vertex.
    Endpoint,
    /// Midpoint of a straight segment.
    Midpoint,
    /// Intersection of two paths.
    Intersection,
    /// Centre of a circular arc or circle.
    ArcCenter,
}

/// A single snap target: a point + its kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapTarget {
    pub point: PdfPoint,
    pub kind: SnapKind,
}

// Implement RTreeObject so SnapTargets can live in an rstar RTree.
impl RTreeObject for SnapTarget {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_point([self.point.x, self.point.y])
    }
}

// Implement PointDistance so nearest_neighbor() works on RTree<SnapTarget>.
// distance_2 returns the squared Euclidean distance to the query point.
impl PointDistance for SnapTarget {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let dx = self.point.x - point[0];
        let dy = self.point.y - point[1];
        dx * dx + dy * dy
    }
}

// ---------------------------------------------------------------------------
// Page geometry index
// ---------------------------------------------------------------------------

/// Spatial index of snap targets for a single page.
/// Built once per page on first snapping interaction; cached until the document changes.
pub struct PageGeometry {
    pub page_index: u32,
    /// r-tree of snap targets, queryable by proximity.
    pub snap_index: RTree<SnapTarget>,
}

impl PageGeometry {
    /// Query snap targets within `tolerance_pts` of `cursor` (PDF user space).
    /// Returns the closest target within tolerance, or None.
    pub fn nearest_snap(&self, cursor: PdfPoint, tolerance_pts: f64) -> Option<&SnapTarget> {
        let tol2 = tolerance_pts * tolerance_pts;
        self.snap_index
            .nearest_neighbor(&[cursor.x, cursor.y])
            .filter(|t| t.distance_2(&[cursor.x, cursor.y]) <= tol2)
    }
}

// ---------------------------------------------------------------------------
// Stub: extraction (M2)
// ---------------------------------------------------------------------------

/// Extract snap targets from a PDF page via PDFium transformed path iteration.
///
/// TODO (M2): implement using `page.objects().iter()` + transformed segment extraction.
/// Key: use the *transformed* variant of path segment iteration to correctly compose
/// Form XObject CTM stacks — bare path coordinates collapse to ~(0,0) otherwise.
/// See spec §5 and pdfium-render docs for `PdfPagePathObject::transform()`.
#[allow(dead_code)]
pub fn extract_page_geometry(_page_index: u32) -> PageGeometry {
    PageGeometry {
        page_index: _page_index,
        snap_index: RTree::new(),
    }
}

// ---------------------------------------------------------------------------
// Takeoff geometry tools (M4 S1)
//
// Pure math functions used by the takeoff measurement engine.
// All inputs/outputs are in PDF user space (f64 points or squares of points).
// ---------------------------------------------------------------------------

/// Sum of segment lengths for a closed polyline (last point connects back to first).
/// Returns 0.0 for fewer than 2 points.
pub fn perimeter(points: &[PdfPoint]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut total = 0.0_f64;
    for w in points.windows(2) {
        let dx = w[1].x - w[0].x;
        let dy = w[1].y - w[0].y;
        total += (dx * dx + dy * dy).sqrt();
    }
    // Close the polygon: last point back to first.
    let last = &points[points.len() - 1];
    let first = &points[0];
    let dx = first.x - last.x;
    let dy = first.y - last.y;
    total += (dx * dx + dy * dy).sqrt();
    total
}

/// Simple volume estimate: `area * depth` (2D area multiplied by user-supplied depth).
/// Both inputs and the output are in consistent units (PDF user space or converted SI).
pub fn volume(area: f64, depth: f64) -> f64 {
    area * depth
}

/// Angle in degrees between two line segments, measured as the acute/obtuse bearing
/// at their conceptual intersection. Each segment is defined by two endpoints.
///
/// Returns a value in [0, 180]. Returns 0.0 if either segment has zero length.
pub fn angle_between_lines(a1: PdfPoint, a2: PdfPoint, b1: PdfPoint, b2: PdfPoint) -> f64 {
    let ux = a2.x - a1.x;
    let uy = a2.y - a1.y;
    let vx = b2.x - b1.x;
    let vy = b2.y - b1.y;

    let len_u = (ux * ux + uy * uy).sqrt();
    let len_v = (vx * vx + vy * vy).sqrt();
    if len_u == 0.0 || len_v == 0.0 {
        return 0.0;
    }

    let dot = (ux * vx + uy * vy) / (len_u * len_v);
    // Clamp to [-1, 1] to guard against floating-point noise outside acos domain.
    dot.clamp(-1.0, 1.0).acos().to_degrees()
}

/// Euclidean distance from `center` to `edge` (circle radius in PDF user space).
pub fn circle_radius(center: PdfPoint, edge: PdfPoint) -> f64 {
    let dx = edge.x - center.x;
    let dy = edge.y - center.y;
    (dx * dx + dy * dy).sqrt()
}

/// Shoelace formula for a simple polygon (outer area minus hole areas).
///
/// `outer` is the outer polygon (any winding). `holes` is a slice of inner
/// polygons (cutouts); their signed area is subtracted from the outer area.
///
/// Returns the net absolute area in PDF user space square units (points^2).
pub fn area_with_cutouts(outer: &[PdfPoint], holes: &[Vec<PdfPoint>]) -> f64 {
    fn signed_area(pts: &[PdfPoint]) -> f64 {
        if pts.len() < 3 {
            return 0.0;
        }
        let mut acc = 0.0_f64;
        let n = pts.len();
        for i in 0..n {
            let j = (i + 1) % n;
            acc += pts[i].x * pts[j].y;
            acc -= pts[j].x * pts[i].y;
        }
        acc / 2.0
    }

    let outer_area = signed_area(outer).abs();
    let holes_area: f64 = holes.iter().map(|h| signed_area(h).abs()).sum();
    (outer_area - holes_area).max(0.0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rstar::RTree;

    #[test]
    fn nearest_snap_within_tolerance() {
        let targets = vec![
            SnapTarget {
                point: PdfPoint { x: 10.0, y: 20.0 },
                kind: SnapKind::Endpoint,
            },
            SnapTarget {
                point: PdfPoint { x: 50.0, y: 60.0 },
                kind: SnapKind::Midpoint,
            },
        ];
        let geom = PageGeometry {
            page_index: 0,
            snap_index: RTree::bulk_load(targets),
        };
        // Cursor within 5pt tolerance of first point.
        let result = geom.nearest_snap(PdfPoint { x: 11.0, y: 21.0 }, 5.0);
        assert!(result.is_some());
        assert_eq!(result.unwrap().kind, SnapKind::Endpoint);
    }

    #[test]
    fn nearest_snap_outside_tolerance() {
        let targets = vec![SnapTarget {
            point: PdfPoint { x: 10.0, y: 20.0 },
            kind: SnapKind::Endpoint,
        }];
        let geom = PageGeometry {
            page_index: 0,
            snap_index: RTree::bulk_load(targets),
        };
        // Cursor 100pt away — outside any tolerance.
        let result = geom.nearest_snap(PdfPoint { x: 110.0, y: 120.0 }, 5.0);
        assert!(result.is_none());
    }

    #[test]
    fn empty_index_returns_none() {
        let geom = PageGeometry {
            page_index: 0,
            snap_index: RTree::new(),
        };
        let result = geom.nearest_snap(PdfPoint { x: 0.0, y: 0.0 }, 10.0);
        assert!(result.is_none());
    }

    // ---------------------------------------------------------------------------
    // Takeoff geometry tool tests (M4 S1)
    // ---------------------------------------------------------------------------

    fn pt(x: f64, y: f64) -> PdfPoint {
        PdfPoint { x, y }
    }

    // --- perimeter ---

    #[test]
    fn perimeter_equilateral_triangle_3_4_5() {
        // Right triangle with legs 3 and 4, hypotenuse 5.
        let pts = vec![pt(0.0, 0.0), pt(3.0, 0.0), pt(0.0, 4.0)];
        let p = perimeter(&pts);
        assert!((p - 12.0).abs() < 1e-10, "3+4+5 = 12, got {p}");
    }

    #[test]
    fn perimeter_unit_square() {
        let pts = vec![pt(0.0, 0.0), pt(1.0, 0.0), pt(1.0, 1.0), pt(0.0, 1.0)];
        let p = perimeter(&pts);
        assert!(
            (p - 4.0).abs() < 1e-10,
            "unit square perimeter = 4, got {p}"
        );
    }

    #[test]
    fn perimeter_single_point_zero() {
        assert_eq!(perimeter(&[pt(0.0, 0.0)]), 0.0);
    }

    #[test]
    fn perimeter_empty_zero() {
        assert_eq!(perimeter(&[]), 0.0);
    }

    // --- volume ---

    #[test]
    fn volume_simple_product() {
        assert_eq!(volume(100.0, 2.5), 250.0);
    }

    #[test]
    fn volume_zero_depth() {
        assert_eq!(volume(500.0, 0.0), 0.0);
    }

    // --- angle_between_lines ---

    #[test]
    fn angle_perpendicular_lines_90_degrees() {
        // Horizontal and vertical lines - 90 degrees.
        let a = angle_between_lines(pt(0.0, 0.0), pt(1.0, 0.0), pt(0.0, 0.0), pt(0.0, 1.0));
        assert!((a - 90.0).abs() < 1e-10, "perpendicular = 90, got {a}");
    }

    #[test]
    fn angle_parallel_lines_0_degrees() {
        // Two horizontal lines - 0 degrees.
        let a = angle_between_lines(pt(0.0, 0.0), pt(1.0, 0.0), pt(2.0, 0.0), pt(3.0, 0.0));
        assert!((a - 0.0).abs() < 1e-10, "parallel = 0, got {a}");
    }

    #[test]
    fn angle_antiparallel_lines_180_degrees() {
        // One horizontal right, one horizontal left - 180 degrees.
        let a = angle_between_lines(pt(0.0, 0.0), pt(1.0, 0.0), pt(1.0, 0.0), pt(0.0, 0.0));
        assert!((a - 180.0).abs() < 1e-10, "antiparallel = 180, got {a}");
    }

    #[test]
    fn angle_zero_length_segment_returns_zero() {
        let a = angle_between_lines(pt(0.0, 0.0), pt(0.0, 0.0), pt(1.0, 0.0), pt(2.0, 0.0));
        assert_eq!(a, 0.0);
    }

    #[test]
    fn angle_45_degrees() {
        // Horizontal vs 45-degree diagonal.
        let a = angle_between_lines(pt(0.0, 0.0), pt(1.0, 0.0), pt(0.0, 0.0), pt(1.0, 1.0));
        assert!((a - 45.0).abs() < 1e-10, "45 degrees, got {a}");
    }

    // --- circle_radius ---

    #[test]
    fn circle_radius_known_values() {
        // Radius of a unit circle.
        let r = circle_radius(pt(0.0, 0.0), pt(1.0, 0.0));
        assert!((r - 1.0).abs() < 1e-10);
    }

    #[test]
    fn circle_radius_pythagorean_triple() {
        // 3-4-5 triangle: radius = 5.
        let r = circle_radius(pt(0.0, 0.0), pt(3.0, 4.0));
        assert!((r - 5.0).abs() < 1e-10, "radius = 5, got {r}");
    }

    #[test]
    fn circle_radius_same_point_zero() {
        let r = circle_radius(pt(5.0, 5.0), pt(5.0, 5.0));
        assert_eq!(r, 0.0);
    }

    // --- area_with_cutouts ---

    #[test]
    fn area_unit_square_no_holes() {
        let outer = vec![pt(0.0, 0.0), pt(1.0, 0.0), pt(1.0, 1.0), pt(0.0, 1.0)];
        assert!((area_with_cutouts(&outer, &[]) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn area_rectangle_with_square_hole() {
        // 4x4 outer rectangle minus a 2x2 inner square = 16 - 4 = 12.
        let outer = vec![pt(0.0, 0.0), pt(4.0, 0.0), pt(4.0, 4.0), pt(0.0, 4.0)];
        let hole = vec![pt(1.0, 1.0), pt(3.0, 1.0), pt(3.0, 3.0), pt(1.0, 3.0)];
        let a = area_with_cutouts(&outer, &[hole]);
        assert!((a - 12.0).abs() < 1e-10, "4x4 - 2x2 = 12, got {a}");
    }

    #[test]
    fn area_with_two_holes() {
        // 10x10 outer (100) minus two 2x2 holes (4 each) = 92.
        let outer = vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0)];
        let h1 = vec![pt(1.0, 1.0), pt(3.0, 1.0), pt(3.0, 3.0), pt(1.0, 3.0)];
        let h2 = vec![pt(5.0, 5.0), pt(7.0, 5.0), pt(7.0, 7.0), pt(5.0, 7.0)];
        let a = area_with_cutouts(&outer, &[h1, h2]);
        assert!((a - 92.0).abs() < 1e-10, "100 - 4 - 4 = 92, got {a}");
    }

    #[test]
    fn area_hole_larger_than_outer_clamped_to_zero() {
        // Degenerate: hole larger than outer -> result clamped to 0.
        let outer = vec![pt(0.0, 0.0), pt(1.0, 0.0), pt(1.0, 1.0), pt(0.0, 1.0)];
        let big_hole = vec![pt(-1.0, -1.0), pt(5.0, -1.0), pt(5.0, 5.0), pt(-1.0, 5.0)];
        let a = area_with_cutouts(&outer, &[big_hole]);
        assert_eq!(a, 0.0, "negative net area clamped to 0");
    }

    #[test]
    fn area_empty_outer_zero() {
        assert_eq!(area_with_cutouts(&[], &[]), 0.0);
    }
}
