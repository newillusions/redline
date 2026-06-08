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
}
