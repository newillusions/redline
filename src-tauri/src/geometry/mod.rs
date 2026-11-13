// Adapted from redcolumn (Apache-2.0), packages/measure/src/snap.ts, modified
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
//! # Extraction split: pure math here, PDFium I/O on the render thread
//! This module builds a page's snap index from already-extracted [`Subpath`] point
//! data ([`build_snap_index`]) — pure math, no PDFium dependency, unit-testable
//! without a PDFium binary or corpus (same split as the text-selection quad helpers
//! below: `rect_to_quad`/`rects_to_quads`). The PDFium-touching half — walking
//! `page.objects().iter()`, filtering path objects, and reading their *transformed*
//! segments — lives in `render::RenderEngine::page_snap_index`, because PDFium access
//! must happen on the dedicated render thread (the `RenderCmd`/`RenderHandle`
//! architecture; PDFium keeps process-global C state and isn't safe to touch from
//! any other thread). A free `fn extract_page_geometry(page_index) -> PageGeometry`
//! can never work as a standalone function in this module for that reason — it
//! needs an open `PdfPage` handle, which only the render thread may hold.
//!
//! # Snap kinds populated by `build_snap_index`
//! - [`SnapKind::Endpoint`] (every distinct vertex) and [`SnapKind::Midpoint`] (every
//!   segment midpoint, including a sub-path's closing segment).
//! - [`SnapKind::Intersection`]: proper crossings between straight segments (interior to
//!   both; segments that merely meet at an end already snap as endpoints). Candidate pairs
//!   come from an rstar tree of segment bounding boxes rather than a naive O(n^2) pass, and
//!   the pass is capped (`MAX_PAIR_TESTS`, `MAX_INTERSECTIONS`) so a pathological page
//!   degrades to fewer intersection snaps instead of an interaction-latency regression.
//! - [`SnapKind::ArcCenter`] and [`SnapKind::Quadrant`]: PDFium reports circles/arcs as
//!   `BezierTo` segments, and pdfium-render exposes only each segment's destination point
//!   (no Bezier control points). A run of at least three consecutive curve segments whose
//!   destination points all lie on one circle (tolerance `max(0.25pt, 2% of r)`) is treated
//!   as an arc: its centre snaps as `ArcCenter`, and the compass points the arc reaches
//!   snap as `Quadrant`. A single quarter-arc (a rounded corner) has too few points to
//!   fit and is deliberately ignored.
//!
//! `Perpendicular` and nearest-on-segment depend on the cursor and the point being drawn
//! from, so they are computed client-side (`src/lib/snap.ts`) from the straight segments
//! this index also exposes (`PageGeometry::segments`).
//!
//! Intersection/arc fitting is adapted in approach from redcolumn
//! (`packages/measure/src/snap.ts`, Apache-2.0); see NOTICE.

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

// ---------------------------------------------------------------------------
// Rotation- and MediaBox-origin-aware coordinate conversion.
//
// Canonical home for the true<->display transform (moved here 2026-09-07 from
// `document::annots`, its original site - the 2026-08-06 markup interop fix - so a
// second consumer, `render::search_page`'s rotation fix, doesn't have to duplicate it a
// third time). See `document::annots`'s module doc comment (still there, unmodified) for
// the full derivation and empirical verification against real PDFium rendering; the
// short version: PDF content-stream/annotation coordinates ("true" space) are always in
// the page's absolute default user space, unaffected by `/Rotate` or a non-origin
// `/MediaBox` (ISO 32000-1 §14.4/§8.4.1) - but PDFium's OWN page-space APIs
// (`get_page_size`, `render_tile`'s custom matrix, and per the 2026-09-07 rotation-space
// probe in `render::tests::search_rotation_space`, also the raw `/MediaBox` origin but
// NOT `FPDFText_*` search results) report a "display" space that bakes `/Rotate` in
// (width/height swap on 90/270) and treats the MediaBox's own lower-left corner as
// local (0,0). `w0`/`h0` are the page's TRUE (unrotated) MediaBox width/height; `ox`/`oy`
// are its lower-left corner's absolute coordinates (0,0 for the common case).
// ---------------------------------------------------------------------------

/// Rotation-only step (MediaBox assumed to start at its own local (0,0)) - PDF true
/// default user space -> PDFium "display" (rotated) page space.
pub fn rotate_local_true_to_display(p: PdfPoint, rotation: i32, w0: f64, h0: f64) -> PdfPoint {
    match rotation.rem_euclid(360) {
        90 => PdfPoint {
            x: p.y,
            y: w0 - p.x,
        },
        180 => PdfPoint {
            x: w0 - p.x,
            y: h0 - p.y,
        },
        270 => PdfPoint {
            x: h0 - p.y,
            y: p.x,
        },
        _ => p, // 0 (or a non-multiple-of-90 value some other tool wrote) - identity.
    }
}

/// Exact inverse of [`rotate_local_true_to_display`].
pub fn rotate_local_display_to_true(p: PdfPoint, rotation: i32, w0: f64, h0: f64) -> PdfPoint {
    match rotation.rem_euclid(360) {
        90 => PdfPoint {
            x: w0 - p.y,
            y: p.x,
        },
        180 => PdfPoint {
            x: w0 - p.x,
            y: h0 - p.y,
        },
        270 => PdfPoint {
            x: p.y,
            y: h0 - p.x,
        },
        _ => p,
    }
}

/// PDF true (absolute) default user space -> PDFium "display" page space: shift into the
/// MediaBox's own local frame, then rotate.
pub fn true_to_display(p: PdfPoint, rotation: i32, w0: f64, h0: f64, ox: f64, oy: f64) -> PdfPoint {
    let local = PdfPoint {
        x: p.x - ox,
        y: p.y - oy,
    };
    rotate_local_true_to_display(local, rotation, w0, h0)
}

/// PDFium "display" page space -> PDF true (absolute) default user space: un-rotate, then
/// shift out of the MediaBox's local frame. Exact inverse of [`true_to_display`].
pub fn display_to_true(p: PdfPoint, rotation: i32, w0: f64, h0: f64, ox: f64, oy: f64) -> PdfPoint {
    let local = rotate_local_display_to_true(p, rotation, w0, h0);
    PdfPoint {
        x: local.x + ox,
        y: local.y + oy,
    }
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
    /// Compass point (top/bottom/left/right) of a circle or the part of an arc that reaches it.
    Quadrant,
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
    /// Straight segments `[x1, y1, x2, y2]` (PDF user space) for client-side
    /// perpendicular / nearest-on-segment snapping. Curve pieces are excluded.
    pub segments: Vec<[f64; 4]>,
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
// Snap-index construction (pure math half — see module doc comment)
// ---------------------------------------------------------------------------

/// One sub-path (contiguous polyline) extracted from a PDFium path page object's
/// *transformed* segments — one entry per `MoveTo`-delimited run. `closed` is set
/// when PDFium reported an explicit path-close (`FPDFPathSegment_GetClose`) on the
/// sub-path's final segment. `points` holds each segment's *destination* point only
/// (Bézier control points aren't exposed by pdfium-render's segment API), so a
/// curved sub-path is approximated here by its Bézier endpoints, not its true curve.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Subpath {
    pub points: Vec<PdfPoint>,
    pub closed: bool,
    /// Parallel to `points` when non-empty: `curve[i]` is true when point `i` was reached by
    /// a `BezierTo` (so segment `i-1 -> i` is a curve piece). Empty means all straight.
    pub curve: Vec<bool>,
}

impl Subpath {
    fn is_curve(&self, i: usize) -> bool {
        self.curve.get(i).copied().unwrap_or(false)
    }
}

/// One segment of a flattened sub-path: a straight line, or a cubic Bezier with its two
/// control points (the on-curve end points live in [`Flat::pts`]).
#[derive(Debug, Clone, Copy)]
enum Seg {
    Line,
    Cubic(PdfPoint, PdfPoint),
}

/// A sub-path reduced to on-curve points only. PDFium reports each cubic Bezier as three
/// consecutive `BezierTo` points (control 1, control 2, end), so control points are folded
/// into [`Seg::Cubic`] instead of being treated as vertices. `segs.len() == pts.len() - 1`.
struct Flat {
    pts: Vec<PdfPoint>,
    segs: Vec<Seg>,
}

fn flatten(sub: &Subpath) -> Flat {
    let p = &sub.points;
    let mut pts = vec![p[0]];
    let mut segs = Vec::new();
    let mut i = 1;
    while i < p.len() {
        if sub.is_curve(i) && i + 2 < p.len() && sub.is_curve(i + 1) && sub.is_curve(i + 2) {
            segs.push(Seg::Cubic(p[i], p[i + 1]));
            pts.push(p[i + 2]);
            i += 3;
        } else {
            // A straight segment, or a malformed curve run (fewer than three points):
            // keep the destination point as a vertex rather than dropping it.
            segs.push(Seg::Line);
            pts.push(p[i]);
            i += 1;
        }
    }
    Flat { pts, segs }
}

/// Point at parameter `t` on the cubic `a, c1, c2, b`.
fn cubic_at(a: PdfPoint, c1: PdfPoint, c2: PdfPoint, b: PdfPoint, t: f64) -> PdfPoint {
    let u = 1.0 - t;
    let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    PdfPoint {
        x: w0 * a.x + w1 * c1.x + w2 * c2.x + w3 * b.x,
        y: w0 * a.y + w1 * c1.y + w2 * c2.y + w3 * b.y,
    }
}

/// Midpoint of segment `i` of `flat` (t = 0.5 for a cubic).
fn seg_midpoint(flat: &Flat, i: usize) -> PdfPoint {
    let (a, b) = (flat.pts[i], flat.pts[i + 1]);
    match flat.segs[i] {
        Seg::Line => PdfPoint {
            x: (a.x + b.x) / 2.0,
            y: (a.y + b.y) / 2.0,
        },
        Seg::Cubic(c1, c2) => cubic_at(a, c1, c2, b, 0.5),
    }
}

/// Axis-aligned region (PDF user space) outside which snap geometry is dropped or clipped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapBounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl SnapBounds {
    fn contains(&self, p: PdfPoint) -> bool {
        p.x >= self.min_x && p.x <= self.max_x && p.y >= self.min_y && p.y <= self.max_y
    }
}

/// Coordinates beyond this magnitude are treated as corrupt, whatever the page box says.
const MAX_SANE_COORD: f64 = 1.0e9;

fn sane(p: &PdfPoint) -> bool {
    p.x.is_finite() && p.y.is_finite() && p.x.abs() < MAX_SANE_COORD && p.y.abs() < MAX_SANE_COORD
}

/// Liang-Barsky: `seg` clipped to `b`, or `None` when no part lies inside.
fn clip_segment(seg: [f64; 4], b: &SnapBounds) -> Option<[f64; 4]> {
    let (x0, y0, dx, dy) = (seg[0], seg[1], seg[2] - seg[0], seg[3] - seg[1]);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, x0 - b.min_x),
        (dx, b.max_x - x0),
        (-dy, y0 - b.min_y),
        (dy, b.max_y - y0),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some([x0 + t0 * dx, y0 + t0 * dy, x0 + t1 * dx, y0 + t1 * dy])
}

/// Build a page's snap-target spatial index from its extracted vector sub-paths
/// (spec §5), with no page bounds. See [`build_snap_index_bounded`].
pub fn build_snap_index(page_index: u32, subpaths: &[Subpath]) -> PageGeometry {
    build_snap_index_bounded(page_index, subpaths, None)
}

/// Build a page's snap-target spatial index. See the module doc comment for which
/// `SnapKind`s this populates. Hostile or broken content is contained here, before it reaches
/// the r-trees or the client: a sub-path with a non-finite or absurdly large coordinate is
/// skipped whole, and with `bounds` set, targets outside it are dropped and segments are
/// clipped to it (so a segment with coordinates around 1e8 cannot make the client rasterise
/// millions of grid cells).
pub fn build_snap_index_bounded(
    page_index: u32,
    subpaths: &[Subpath],
    bounds: Option<SnapBounds>,
) -> PageGeometry {
    let mut targets = Vec::new();
    let mut segments: Vec<[f64; 4]> = Vec::new();

    for sub in subpaths {
        if sub.points.is_empty() || !sub.points.iter().all(sane) {
            continue;
        }
        let flat = flatten(sub);
        let pts = &flat.pts;

        for p in pts {
            targets.push(SnapTarget {
                point: *p,
                kind: SnapKind::Endpoint,
            });
        }

        for (i, seg) in flat.segs.iter().enumerate() {
            targets.push(SnapTarget {
                point: seg_midpoint(&flat, i),
                kind: SnapKind::Midpoint,
            });
            if matches!(seg, Seg::Line) {
                segments.push([pts[i].x, pts[i].y, pts[i + 1].x, pts[i + 1].y]);
            }
        }

        // Closing segment (last point back to first), only when PDFium marked the
        // sub-path closed and it isn't already degenerate (single point, or the
        // last point already coincides with the first — no distinct closing edge).
        let first = pts[0];
        let last = *pts.last().unwrap();
        if sub.closed && pts.len() > 1 && first != last {
            targets.push(midpoint_target(last, first));
            segments.push([last.x, last.y, first.x, first.y]);
        }

        fit_arcs(&flat, &mut targets);
    }

    if let Some(b) = bounds {
        targets.retain(|t| b.contains(t.point));
        segments = segments
            .into_iter()
            .filter_map(|s| clip_segment(s, &b))
            .collect();
    }
    add_intersections(&segments, &mut targets);

    PageGeometry {
        page_index,
        snap_index: RTree::bulk_load(targets),
        segments,
    }
}

/// Cap on candidate segment pairs tested per page.
const MAX_PAIR_TESTS: usize = 4_000_000;
/// Cap on intersection targets emitted per page.
const MAX_INTERSECTIONS: usize = 200_000;

/// Segment with its bounding box, for the candidate-pair r-tree.
struct SegEnv {
    idx: usize,
    s: [f64; 4],
}

impl RTreeObject for SegEnv {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.s[0], self.s[1]], [self.s[2], self.s[3]])
    }
}

/// Proper intersection of two segments: `Some` only when the crossing lies strictly
/// inside both (segments meeting at an end are endpoints, not intersections).
fn proper_intersection(a: &[f64; 4], b: &[f64; 4]) -> Option<PdfPoint> {
    let (rx, ry) = (a[2] - a[0], a[3] - a[1]);
    let (qx, qy) = (b[2] - b[0], b[3] - b[1]);
    let den = rx * qy - ry * qx;
    let scale = (rx.hypot(ry) * qx.hypot(qy)).max(1e-9);
    if den.abs() < 1e-9 * scale {
        return None; // parallel or degenerate
    }
    let t = ((b[0] - a[0]) * qy - (b[1] - a[1]) * qx) / den;
    let u = ((b[0] - a[0]) * ry - (b[1] - a[1]) * rx) / den;
    let eps = 1e-6;
    if t > eps && t < 1.0 - eps && u > eps && u < 1.0 - eps {
        Some(PdfPoint {
            x: a[0] + t * rx,
            y: a[1] + t * ry,
        })
    } else {
        None
    }
}

fn add_intersections(segments: &[[f64; 4]], targets: &mut Vec<SnapTarget>) {
    if segments.len() < 2 {
        return;
    }
    let tree = RTree::bulk_load(
        segments
            .iter()
            .enumerate()
            .map(|(idx, s)| SegEnv { idx, s: *s })
            .collect::<Vec<_>>(),
    );
    let mut tests = 0usize;
    let mut found = 0usize;
    for (i, s) in segments.iter().enumerate() {
        let env = AABB::from_corners([s[0], s[1]], [s[2], s[3]]);
        for other in tree.locate_in_envelope_intersecting(&env) {
            if other.idx <= i {
                continue;
            }
            tests += 1;
            if tests > MAX_PAIR_TESTS {
                return;
            }
            if let Some(point) = proper_intersection(s, &other.s) {
                targets.push(SnapTarget {
                    point,
                    kind: SnapKind::Intersection,
                });
                found += 1;
                if found >= MAX_INTERSECTIONS {
                    return;
                }
            }
        }
    }
}

/// Circle through three points, or `None` when they are (nearly) collinear.
fn circle_through(a: PdfPoint, b: PdfPoint, c: PdfPoint) -> Option<(f64, f64, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-9 {
        return None;
    }
    let (a2, b2, c2) = (
        a.x * a.x + a.y * a.y,
        b.x * b.x + b.y * b.y,
        c.x * c.x + c.y * c.y,
    );
    let cx = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let cy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let r = (a.x - cx).hypot(a.y - cy);
    (r.is_finite() && r > 0.0 && r < 1.0e6).then_some((cx, cy, r))
}

/// Minimum consecutive cubic segments to attempt a circle fit. One cubic (a rounded
/// corner) is too little evidence; two (a semicircle) are verified against their
/// midpoints as well as their end points.
const MIN_ARC_CUBICS: usize = 2;

/// Fit circles to runs of consecutive cubic segments and emit `ArcCenter` + `Quadrant`.
fn fit_arcs(flat: &Flat, targets: &mut Vec<SnapTarget>) {
    let n = flat.segs.len();
    let mut i = 0;
    while i < n {
        if !matches!(flat.segs[i], Seg::Cubic(..)) {
            i += 1;
            continue;
        }
        let mut end = i;
        while end + 1 < n && matches!(flat.segs[end + 1], Seg::Cubic(..)) {
            end += 1;
        }
        if end - i + 1 >= MIN_ARC_CUBICS {
            fit_run(flat, i, end, targets);
        }
        i = end + 1;
    }
}

/// `first..=last` are consecutive cubic segment indices of `flat`.
fn fit_run(flat: &Flat, first: usize, last: usize, targets: &mut Vec<SnapTarget>) {
    // On-curve evidence: every end point of the run plus each cubic's t = 0.5 point.
    let pts = &flat.pts[first..=last + 1];
    let mut evidence: Vec<PdfPoint> = pts.to_vec();
    for k in first..=last {
        evidence.push(seg_midpoint(flat, k));
    }
    let n = pts.len();
    let Some((cx, cy, r)) = circle_through(pts[0], pts[n / 3], pts[(2 * n) / 3]) else {
        return;
    };
    let tol = (r * 0.02).max(0.25);
    if !evidence
        .iter()
        .all(|p| ((p.x - cx).hypot(p.y - cy) - r).abs() <= tol)
    {
        return;
    }
    targets.push(SnapTarget {
        point: PdfPoint { x: cx, y: cy },
        kind: SnapKind::ArcCenter,
    });

    // Angular span covered: sum of signed steps between consecutive end points.
    let ang = |p: &PdfPoint| (p.y - cy).atan2(p.x - cx);
    let tau = std::f64::consts::TAU;
    let norm = |mut d: f64| {
        while d > std::f64::consts::PI {
            d -= tau;
        }
        while d <= -std::f64::consts::PI {
            d += tau;
        }
        d
    };
    let a0 = ang(&pts[0]);
    let mut total = 0.0;
    for w in pts.windows(2) {
        total += norm(ang(&w[1]) - ang(&w[0]));
    }
    let closed = (pts[0].x - pts[n - 1].x).hypot(pts[0].y - pts[n - 1].y) < 1e-2;
    let compass = [
        (0.0, PdfPoint { x: cx + r, y: cy }),
        (std::f64::consts::FRAC_PI_2, PdfPoint { x: cx, y: cy + r }),
        (std::f64::consts::PI, PdfPoint { x: cx - r, y: cy }),
        (-std::f64::consts::FRAC_PI_2, PdfPoint { x: cx, y: cy - r }),
    ];
    for (a, point) in compass {
        // Offset of the compass angle from the arc start, in the arc's sweep direction.
        let mut off = norm(a - a0);
        if total < 0.0 && off > 0.0 {
            off -= tau;
        } else if total >= 0.0 && off < 0.0 {
            off += tau;
        }
        if closed || off.abs() <= total.abs() + 1e-6 {
            targets.push(SnapTarget {
                point,
                kind: SnapKind::Quadrant,
            });
        }
    }
}

fn midpoint_target(a: PdfPoint, b: PdfPoint) -> SnapTarget {
    SnapTarget {
        point: PdfPoint {
            x: (a.x + b.x) / 2.0,
            y: (a.y + b.y) / 2.0,
        },
        kind: SnapKind::Midpoint,
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
// Text-selection quads (redline text selection + text-anchored highlight feature)
//
// Converts PDFium line-segment rects (one per visual text line, from
// `PdfPageText::segments_subset`) into PDF `/QuadPoints` quadrilaterals for a
// Highlight annotation (spec section 6 addendum). Pure PDF-user-space math - no
// PDFium dependency, so this is unit-testable against manufactured rect data
// without a PDFium binary or corpus (CI trap obs:he8y7010dlo82k88rocu: anything
// that actually loads PDFium must self-skip in CI; this code never touches
// PDFium).
// ---------------------------------------------------------------------------

/// A single quadrilateral in PDF user space, 4 points ordered
/// **top-left, top-right, bottom-left, bottom-right**.
///
/// This is the de-facto Acrobat/Bluebeam `/QuadPoints` vertex order - NOT the PDF
/// spec's literal "counterclockwise" wording (ISO 32000-1 section 12.5.6.10,
/// Figure 8.9). Every mainstream viewer (Acrobat, Bluebeam, pdf.js) implements
/// the TL/TR/BL/BR "Z" order in practice; a spec-literal CCW ordering renders
/// highlights mirrored or rotated in Acrobat. Confirmed via an Adobe forum
/// thread on QuadPoints ordering (Acrobat-authored Highlight annotations use
/// x1y1 x2y2 x4y4 x3y3 = TL,TR,BL,BR against the spec's own vertex numbering).
pub type Quad = [PdfPoint; 4];

/// Convert one axis-aligned line-segment rect `[left, bottom, right, top]` (PDF
/// user space, y-up - the same `[f64; 4]` shape `SearchHit::rect` and PDFium's
/// `FPDFText_GetRect` use) into a `Quad` in the TL/TR/BL/BR order documented on
/// [`Quad`].
pub fn rect_to_quad(rect: [f64; 4]) -> Quad {
    let [left, bottom, right, top] = rect;
    [
        PdfPoint { x: left, y: top },    // top-left
        PdfPoint { x: right, y: top },   // top-right
        PdfPoint { x: left, y: bottom }, // bottom-left
        PdfPoint {
            x: right,
            y: bottom,
        }, // bottom-right
    ]
}

/// Convert a list of line-segment rects (as returned by PDFium's
/// `segments_subset` for a character range - one rect per visual text line) into
/// `Quad`s, preserving order and count. Unlike text-search highlighting (which
/// merges all segments of a hit into one bounding rect), a text-selection
/// highlight must carry ONE quad per line so multi-line selections render as
/// separate translucent bands hugging each line, not one box spanning the gap
/// between lines.
pub fn rects_to_quads(rects: &[[f64; 4]]) -> Vec<Quad> {
    rects.iter().copied().map(rect_to_quad).collect()
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
            segments: Vec::new(),
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
            segments: Vec::new(),
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
            segments: Vec::new(),
        };
        let result = geom.nearest_snap(PdfPoint { x: 0.0, y: 0.0 }, 10.0);
        assert!(result.is_none());
    }

    // ---------------------------------------------------------------------------
    // build_snap_index (v1 snap-target extraction — pure half)
    // ---------------------------------------------------------------------------

    #[test]
    fn build_snap_index_open_polyline_endpoints_and_midpoints() {
        // A single open 3-point polyline: (0,0) -> (10,0) -> (10,10).
        let subpaths = vec![Subpath {
            points: vec![
                PdfPoint { x: 0.0, y: 0.0 },
                PdfPoint { x: 10.0, y: 0.0 },
                PdfPoint { x: 10.0, y: 10.0 },
            ],
            closed: false,
            ..Default::default()
        }];
        let geom = build_snap_index(0, &subpaths);

        let endpoints: Vec<_> = geom
            .snap_index
            .iter()
            .filter(|t| t.kind == SnapKind::Endpoint)
            .collect();
        assert_eq!(endpoints.len(), 3, "one Endpoint per vertex");

        let midpoints: Vec<_> = geom
            .snap_index
            .iter()
            .filter(|t| t.kind == SnapKind::Midpoint)
            .collect();
        // Open polyline: 2 segments, no closing edge -> exactly 2 midpoints.
        assert_eq!(
            midpoints.len(),
            2,
            "no closing-segment midpoint on an open sub-path"
        );
        assert!(midpoints
            .iter()
            .any(|t| (t.point.x - 5.0).abs() < 1e-9 && (t.point.y - 0.0).abs() < 1e-9));
        assert!(midpoints
            .iter()
            .any(|t| (t.point.x - 10.0).abs() < 1e-9 && (t.point.y - 5.0).abs() < 1e-9));
    }

    #[test]
    fn build_snap_index_closed_subpath_adds_closing_midpoint() {
        // A closed triangle: (0,0) -> (10,0) -> (0,10), closed back to (0,0).
        let subpaths = vec![Subpath {
            points: vec![
                PdfPoint { x: 0.0, y: 0.0 },
                PdfPoint { x: 10.0, y: 0.0 },
                PdfPoint { x: 0.0, y: 10.0 },
            ],
            closed: true,
            ..Default::default()
        }];
        let geom = build_snap_index(0, &subpaths);
        let midpoints = geom
            .snap_index
            .iter()
            .filter(|t| t.kind == SnapKind::Midpoint)
            .count();
        // 2 interior segments + 1 closing segment = 3 midpoints.
        assert_eq!(
            midpoints, 3,
            "closed sub-path adds a midpoint for the closing edge"
        );
    }

    #[test]
    fn build_snap_index_closed_but_degenerate_last_equals_first_no_extra_midpoint() {
        // PDFium reported `closed`, but the point list already round-trips back to
        // its start (no distinct closing edge to add).
        let subpaths = vec![Subpath {
            points: vec![
                PdfPoint { x: 0.0, y: 0.0 },
                PdfPoint { x: 10.0, y: 0.0 },
                PdfPoint { x: 0.0, y: 0.0 },
            ],
            closed: true,
            ..Default::default()
        }];
        let geom = build_snap_index(0, &subpaths);
        let midpoints = geom
            .snap_index
            .iter()
            .filter(|t| t.kind == SnapKind::Midpoint)
            .count();
        assert_eq!(
            midpoints, 2,
            "no extra closing midpoint when last already equals first"
        );
    }

    #[test]
    fn build_snap_index_multiple_subpaths_are_independent() {
        let subpaths = vec![
            Subpath {
                points: vec![PdfPoint { x: 0.0, y: 0.0 }, PdfPoint { x: 1.0, y: 0.0 }],
                closed: false,
                ..Default::default()
            },
            Subpath {
                points: vec![
                    PdfPoint { x: 100.0, y: 100.0 },
                    PdfPoint { x: 101.0, y: 100.0 },
                ],
                closed: false,
                ..Default::default()
            },
        ];
        let geom = build_snap_index(0, &subpaths);
        assert_eq!(
            geom.snap_index
                .iter()
                .filter(|t| t.kind == SnapKind::Endpoint)
                .count(),
            4
        );
        assert_eq!(
            geom.snap_index
                .iter()
                .filter(|t| t.kind == SnapKind::Midpoint)
                .count(),
            2
        );
    }

    #[test]
    fn build_snap_index_empty_and_single_point_subpaths_produce_no_midpoints() {
        let subpaths = vec![
            Subpath {
                points: vec![],
                closed: false,
                ..Default::default()
            },
            Subpath {
                points: vec![PdfPoint { x: 5.0, y: 5.0 }],
                closed: true,
                ..Default::default()
            },
        ];
        let geom = build_snap_index(0, &subpaths);
        assert_eq!(
            geom.snap_index
                .iter()
                .filter(|t| t.kind == SnapKind::Endpoint)
                .count(),
            1,
            "the single-point sub-path still yields one Endpoint"
        );
        assert_eq!(
            geom.snap_index
                .iter()
                .filter(|t| t.kind == SnapKind::Midpoint)
                .count(),
            0
        );
    }

    #[test]
    fn build_snap_index_nearest_snap_finds_extracted_endpoint() {
        // Round-trip through PageGeometry::nearest_snap to prove the built index is
        // actually queryable, not just structurally correct.
        let subpaths = vec![Subpath {
            points: vec![
                PdfPoint { x: 50.0, y: 50.0 },
                PdfPoint { x: 150.0, y: 50.0 },
            ],
            closed: false,
            ..Default::default()
        }];
        let geom = build_snap_index(3, &subpaths);
        assert_eq!(geom.page_index, 3);
        let hit = geom.nearest_snap(PdfPoint { x: 51.0, y: 50.0 }, 5.0);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().kind, SnapKind::Endpoint);
    }

    // ---------------------------------------------------------------------------
    // Intersection / arc-centre / quadrant snaps
    // ---------------------------------------------------------------------------

    fn line(a: (f64, f64), b: (f64, f64)) -> Subpath {
        Subpath {
            points: vec![pt(a.0, a.1), pt(b.0, b.1)],
            ..Default::default()
        }
    }

    fn of_kind(g: &PageGeometry, kind: SnapKind) -> Vec<PdfPoint> {
        g.snap_index
            .iter()
            .filter(|t| t.kind == kind)
            .map(|t| t.point)
            .collect()
    }

    fn has_near(pts: &[PdfPoint], x: f64, y: f64) -> bool {
        pts.iter()
            .any(|p| (p.x - x).abs() < 1e-6 && (p.y - y).abs() < 1e-6)
    }

    /// PDFium-style points for a circular arc drawn as cubic Beziers, one per quarter
    /// turn from `start_deg`, `quarters` of them, counter-clockwise (`dir` = 1) or
    /// clockwise (`dir` = -1): [start, c1, c2, end, c1, c2, end, ...].
    fn arc_points(cx: f64, cy: f64, r: f64, start_deg: f64, quarters: usize, dir: f64) -> Subpath {
        let k = 0.552_284_749_8;
        let at = |deg: f64| {
            let a = deg.to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        };
        let tangent = |deg: f64| {
            let a = deg.to_radians();
            (-a.sin() * dir, a.cos() * dir)
        };
        let mut points = vec![];
        let mut curve = vec![false];
        let s = at(start_deg);
        points.push(pt(s.0, s.1));
        for q in 0..quarters {
            let a0 = start_deg + dir * 90.0 * q as f64;
            let a1 = a0 + dir * 90.0;
            let (p0, p1) = (at(a0), at(a1));
            let (t0, t1) = (tangent(a0), tangent(a1));
            points.push(pt(p0.0 + t0.0 * k * r, p0.1 + t0.1 * k * r));
            points.push(pt(p1.0 - t1.0 * k * r, p1.1 - t1.1 * k * r));
            points.push(pt(p1.0, p1.1));
            curve.extend([true, true, true]);
        }
        Subpath {
            points,
            closed: false,
            curve,
        }
    }

    #[test]
    fn crossing_segments_yield_one_intersection() {
        let g = build_snap_index(
            0,
            &[
                line((0.0, 0.0), (10.0, 10.0)),
                line((0.0, 10.0), (10.0, 0.0)),
            ],
        );
        let hits = of_kind(&g, SnapKind::Intersection);
        assert_eq!(hits.len(), 1);
        assert!(has_near(&hits, 5.0, 5.0));
    }

    #[test]
    fn touching_parallel_and_disjoint_segments_yield_no_intersection() {
        let g = build_snap_index(
            0,
            &[
                line((0.0, 0.0), (10.0, 0.0)),
                line((10.0, 0.0), (10.0, 10.0)), // meets at an end: an endpoint, not a crossing
                line((0.0, 5.0), (10.0, 5.0)),   // parallel to the first
                line((20.0, 20.0), (30.0, 30.0)), // far away
                line((5.0, 0.0), (5.0, 10.0)), // T onto the first: touches its interior at the end
            ],
        );
        // (5,0)-(5,10) ends on the first segment's interior (u at an end), and crosses the
        // parallel one at (5,5) properly.
        let hits = of_kind(&g, SnapKind::Intersection);
        assert_eq!(hits.len(), 1, "only the proper (5,5) crossing: {hits:?}");
        assert!(has_near(&hits, 5.0, 5.0));
    }

    #[test]
    fn grid_of_lines_yields_every_interior_crossing() {
        let mut subs = vec![];
        for i in 0..=5 {
            let v = i as f64 * 2.0;
            subs.push(line((0.0, v), (10.0, v)));
            subs.push(line((v, 0.0), (v, 10.0)));
        }
        let g = build_snap_index(0, &subs);
        // Interior lattice is 4 x 4; the border lines only touch at ends/along edges.
        assert_eq!(of_kind(&g, SnapKind::Intersection).len(), 16);
    }

    #[test]
    fn full_circle_yields_centre_and_four_quadrants_and_no_control_point_endpoints() {
        let mut c = arc_points(300.0, 400.0, 50.0, 0.0, 4, 1.0);
        c.closed = true;
        let g = build_snap_index(0, &[c]);
        let centres = of_kind(&g, SnapKind::ArcCenter);
        assert_eq!(centres.len(), 1);
        assert!(has_near(&centres, 300.0, 400.0));
        let quads = of_kind(&g, SnapKind::Quadrant);
        assert_eq!(quads.len(), 4);
        for (x, y) in [
            (350.0, 400.0),
            (300.0, 450.0),
            (250.0, 400.0),
            (300.0, 350.0),
        ] {
            assert!(has_near(&quads, x, y), "missing quadrant ({x},{y})");
        }
        // 4 arc ends + the start point repeated at the close: control points are not vertices.
        assert_eq!(of_kind(&g, SnapKind::Endpoint).len(), 5);
        // Midpoint of the first quarter is on the circle at 45 degrees.
        let mids = of_kind(&g, SnapKind::Midpoint);
        let d = 50.0 / std::f64::consts::SQRT_2;
        assert!(mids
            .iter()
            .any(|m| (m.x - (300.0 + d)).abs() < 0.2 && (m.y - (400.0 + d)).abs() < 0.2));
    }

    #[test]
    fn clockwise_circle_also_fits() {
        let mut c = arc_points(10.0, 10.0, 8.0, 90.0, 4, -1.0);
        c.closed = true;
        let g = build_snap_index(0, &[c]);
        assert!(has_near(&of_kind(&g, SnapKind::ArcCenter), 10.0, 10.0));
        assert_eq!(of_kind(&g, SnapKind::Quadrant).len(), 4);
    }

    #[test]
    fn semicircle_snaps_only_the_compass_points_it_reaches() {
        // Counter-clockwise from 0 to 180 degrees: reaches east, north, west, not south.
        let g = build_snap_index(0, &[arc_points(0.0, 0.0, 20.0, 0.0, 2, 1.0)]);
        assert!(has_near(&of_kind(&g, SnapKind::ArcCenter), 0.0, 0.0));
        let quads = of_kind(&g, SnapKind::Quadrant);
        assert_eq!(quads.len(), 3, "{quads:?}");
        assert!(has_near(&quads, 20.0, 0.0));
        assert!(has_near(&quads, 0.0, 20.0));
        assert!(has_near(&quads, -20.0, 0.0));
        assert!(!has_near(&quads, 0.0, -20.0));
    }

    #[test]
    fn single_quarter_arc_is_not_fitted() {
        // One cubic is a rounded corner as often as an arc: too little evidence.
        let g = build_snap_index(0, &[arc_points(0.0, 0.0, 20.0, 0.0, 1, 1.0)]);
        assert!(of_kind(&g, SnapKind::ArcCenter).is_empty());
        assert!(of_kind(&g, SnapKind::Quadrant).is_empty());
    }

    #[test]
    fn s_curve_is_not_mistaken_for_an_arc() {
        // Two cubics bending opposite ways.
        let s = Subpath {
            points: vec![
                pt(0.0, 0.0),
                pt(10.0, 0.0),
                pt(10.0, 10.0),
                pt(20.0, 10.0),
                pt(30.0, 10.0),
                pt(30.0, 20.0),
                pt(40.0, 20.0),
            ],
            closed: false,
            curve: vec![false, true, true, true, true, true, true],
        };
        let g = build_snap_index(0, &[s]);
        assert!(of_kind(&g, SnapKind::ArcCenter).is_empty());
    }

    #[test]
    fn segments_exclude_curve_pieces_and_include_the_closing_edge() {
        let tri = Subpath {
            points: vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(0.0, 10.0)],
            closed: true,
            ..Default::default()
        };
        let g = build_snap_index(0, &[tri, arc_points(0.0, 0.0, 5.0, 0.0, 4, 1.0)]);
        assert_eq!(g.segments.len(), 3, "triangle only: {:?}", g.segments);
        assert!(g.segments.contains(&[0.0, 10.0, 0.0, 0.0]));
    }

    #[test]
    fn many_overlapping_segments_stay_within_the_pair_cap() {
        // 3000 near-coincident long segments: ~4.5M candidate pairs, above MAX_PAIR_TESTS.
        // Must terminate, and must not emit more than the intersection cap.
        let subs: Vec<Subpath> = (0..3000)
            .map(|i| {
                line(
                    (0.0, i as f64 * 1e-3),
                    (1000.0, 1.0 + i as f64 * 1e-3 + 0.5),
                )
            })
            .collect();
        let g = build_snap_index(0, &subs);
        assert!(of_kind(&g, SnapKind::Intersection).len() <= MAX_INTERSECTIONS);
    }

    // --- hostile / broken content ---

    #[test]
    fn non_finite_and_absurd_coordinates_skip_the_subpath() {
        let g = build_snap_index(
            0,
            &[
                line((0.0, 0.0), (f64::NAN, 5.0)),
                line((0.0, 0.0), (f64::INFINITY, 5.0)),
                line((0.0, 0.0), (1.0e12, 5.0)),
                line((0.0, 0.0), (10.0, 0.0)),
            ],
        );
        assert_eq!(g.segments, vec![[0.0, 0.0, 10.0, 0.0]]);
        assert_eq!(of_kind(&g, SnapKind::Endpoint).len(), 2);
    }

    #[test]
    fn bounds_clip_segments_and_drop_outside_targets() {
        let b = SnapBounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 100.0,
            max_y: 100.0,
        };
        let g = build_snap_index_bounded(
            0,
            &[
                line((-1.0e8, 50.0), (1.0e8, 50.0)),  // spans the page
                line((200.0, 200.0), (300.0, 300.0)), // wholly outside
                line((10.0, 10.0), (20.0, 10.0)),
            ],
            Some(b),
        );
        assert_eq!(g.segments.len(), 2);
        assert!(
            g.segments.iter().any(|s| s[0].abs() < 1e-3
                && (s[2] - 100.0).abs() < 1e-3
                && (s[1] - 50.0).abs() < 1e-6
                && (s[3] - 50.0).abs() < 1e-6),
            "{:?}",
            g.segments
        );
        for t in g.snap_index.iter() {
            assert!(b.contains(t.point), "{t:?} is outside the bounds");
        }
    }

    #[test]
    fn clip_segment_cases() {
        let b = SnapBounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 10.0,
            max_y: 10.0,
        };
        assert_eq!(
            clip_segment([2.0, 2.0, 5.0, 5.0], &b),
            Some([2.0, 2.0, 5.0, 5.0])
        );
        assert_eq!(
            clip_segment([-5.0, 5.0, 15.0, 5.0], &b),
            Some([0.0, 5.0, 10.0, 5.0])
        );
        assert_eq!(clip_segment([-5.0, -5.0, -1.0, 20.0], &b), None);
        assert_eq!(clip_segment([20.0, 0.0, 20.0, 10.0], &b), None); // parallel, outside
        assert_eq!(
            clip_segment([5.0, 5.0, 5.0, 5.0], &b),
            Some([5.0, 5.0, 5.0, 5.0])
        );
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

    // ---------------------------------------------------------------------------
    // Text-selection quads (rect_to_quad / rects_to_quads)
    // ---------------------------------------------------------------------------

    #[test]
    fn rect_to_quad_orders_top_left_top_right_bottom_left_bottom_right() {
        // rect = [left, bottom, right, top]
        let q = rect_to_quad([10.0, 20.0, 110.0, 35.0]);
        assert_eq!(q[0], pt(10.0, 35.0), "index 0 must be top-left");
        assert_eq!(q[1], pt(110.0, 35.0), "index 1 must be top-right");
        assert_eq!(q[2], pt(10.0, 20.0), "index 2 must be bottom-left");
        assert_eq!(q[3], pt(110.0, 20.0), "index 3 must be bottom-right");
    }

    #[test]
    fn rects_to_quads_returns_one_quad_per_rect_for_multi_line_selection() {
        // Three line-segment rects, as PDFium's segments_subset would return for a
        // selection spanning 3 visual lines (top line widest, middle line narrower,
        // bottom line partial - the classic "drag across 3 lines" shape).
        let rects = [
            [72.0, 700.0, 500.0, 712.0], // line 1: full width
            [72.0, 686.0, 500.0, 698.0], // line 2: full width
            [72.0, 672.0, 220.0, 684.0], // line 3: partial (selection ends mid-line)
        ];
        let quads = rects_to_quads(&rects);
        assert_eq!(
            quads.len(),
            3,
            "must return exactly one quad per input rect"
        );
        // Never merged into a single bounding quad - each retains its own rect.
        assert_eq!(quads[0][0], pt(72.0, 712.0));
        assert_eq!(quads[0][1], pt(500.0, 712.0));
        assert_eq!(
            quads[2][1],
            pt(220.0, 684.0),
            "partial last-line width preserved"
        );
    }

    #[test]
    fn rects_to_quads_empty_input_returns_empty() {
        assert!(rects_to_quads(&[]).is_empty());
    }

    #[test]
    fn rect_to_quad_degenerate_zero_height_rect_still_orders_correctly() {
        // A zero-height rect (e.g. a single-point selection) must not panic and
        // must still preserve the TL/TR/BL/BR positional contract.
        let q = rect_to_quad([50.0, 100.0, 50.0, 100.0]);
        assert_eq!(q[0], pt(50.0, 100.0));
        assert_eq!(q[3], pt(50.0, 100.0));
    }
}
