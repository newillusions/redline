//! Symbol Search tests: precision and recall against fixture drawings with a known number of
//! symbols. The drawing builder emits PDF content-stream operators once; the pure tests read
//! them back with a tiny parser (no PDFium needed, so they run in every CI job), and the
//! PDFium tests write the same stream into a real PDF and go through the real extraction,
//! the real renderer and the async `search` entry point (they self-skip, like the other
//! render tests, when PDFIUM_DYNAMIC_LIB_PATH is unset).

use super::raster::{raster_search, GrayImage, RasterSearchOptions};
use super::vector::*;
use super::*;
use crate::geometry::{PdfPoint, Subpath};

// ---------------------------------------------------------------------------
// Fixture drawing
// ---------------------------------------------------------------------------

/// xorshift64: deterministic noise without a dev-dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Octagon, two crossing lines and two ticks of different length at 45 degrees apart: no
    /// turn or mirror of it maps onto itself, so a turned copy is distinguishable.
    Luminaire,
    /// The luminaire without its second tick (11 of 12 lines).
    LuminaireOneTick,
    /// Octagon plus ONE cross line: shares 9 of the luminaire's 12 lines (75%, below 90%).
    DecoyOneArm,
    /// Octagon only: 8 of 12 lines.
    DecoyRing,
    /// Square with a diagonal: shares nothing.
    DecoySquare,
}

struct Drawing {
    ops: String,
    /// Centres of every symbol that is a true match for the template (original included).
    truth: Vec<(f64, f64)>,
}

fn pt(cx: f64, cy: f64, r: f64, a: f64, rot: f64) -> (f64, f64) {
    (cx + r * (a + rot).cos(), cy + r * (a + rot).sin())
}

/// Draw one symbol centred at (cx, cy), radius `r`, turned `rot` radians. All numbers go through
/// `{:.3}` so the PDF content stream and the test parser agree on every coordinate.
fn symbol(ops: &mut String, kind: Kind, cx: f64, cy: f64, r: f64, rot: f64) {
    let m = |ops: &mut String, (x, y): (f64, f64)| ops.push_str(&format!("{x:.3} {y:.3} m "));
    let l = |ops: &mut String, (x, y): (f64, f64)| ops.push_str(&format!("{x:.3} {y:.3} l "));
    use std::f64::consts::PI;
    match kind {
        Kind::DecoySquare => {
            let c: Vec<_> = (0..4)
                .map(|i| pt(cx, cy, r, PI / 4.0 + i as f64 * PI / 2.0, rot))
                .collect();
            m(ops, c[0]);
            for p in &c[1..] {
                l(ops, *p);
            }
            ops.push_str("h S ");
            m(ops, c[0]);
            l(ops, c[2]);
            ops.push_str("S ");
        }
        _ => {
            let v: Vec<_> = (0..8)
                .map(|i| pt(cx, cy, r, i as f64 * PI / 4.0, rot))
                .collect();
            m(ops, v[0]);
            for p in &v[1..] {
                l(ops, *p);
            }
            ops.push_str("h S ");
            if kind != Kind::DecoyRing {
                m(ops, pt(cx, cy, r * 0.7, PI / 2.0, rot));
                l(ops, pt(cx, cy, r * 0.7, -PI / 2.0, rot));
                ops.push_str("S ");
            }
            if kind == Kind::Luminaire || kind == Kind::LuminaireOneTick {
                m(ops, pt(cx, cy, r * 0.7, 0.0, rot));
                l(ops, pt(cx, cy, r * 0.7, PI, rot));
                ops.push_str("S ");
                m(ops, pt(cx, cy, r, PI / 8.0, rot));
                l(ops, pt(cx, cy, r * 1.5, PI / 8.0, rot));
                ops.push_str("S ");
                if kind == Kind::Luminaire {
                    m(ops, pt(cx, cy, r, 3.0 * PI / 8.0, rot));
                    l(ops, pt(cx, cy, r * 1.3, 3.0 * PI / 8.0, rot));
                    ops.push_str("S ");
                }
            }
        }
    }
}

const R: f64 = 14.0;
const PAGE_W: f64 = 1200.0;
const PAGE_H: f64 = 800.0;

/// A sheet: grid lines, walls running through some symbols, `n` luminaires in a grid (the first
/// is the template at (60, 60)), decoys between them. Extra turned/scaled copies per flags.
fn drawing(
    n_cols: usize,
    n_rows: usize,
    with_turned: bool,
    with_scaled: bool,
    jitter: f64,
) -> Drawing {
    drawing_bg(n_cols, n_rows, with_turned, with_scaled, jitter, true)
}

/// `background: false` leaves out walls, grid and hatch: the raster path correlates the whole box,
/// background included, so its fixture needs a clean paper behind every symbol.
fn drawing_bg(
    n_cols: usize,
    n_rows: usize,
    with_turned: bool,
    with_scaled: bool,
    jitter: f64,
    background: bool,
) -> Drawing {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut ops = String::new();
    let mut truth = Vec::new();
    // Walls and a drafting grid; two walls pass straight through symbol rows.
    for i in 0..(if background { 10 } else { 0 }) {
        let y = 40.0 + i as f64 * 70.0;
        ops.push_str(&format!("20 {y:.3} m {:.3} {y:.3} l S ", PAGE_W - 20.0));
    }
    for i in 0..(if background { 16 } else { 0 }) {
        let x = 30.0 + i as f64 * 75.0;
        ops.push_str(&format!("{x:.3} 20 m {x:.3} {:.3} l S ", PAGE_H - 20.0));
    }
    // Hatch noise: short random lines.
    for _ in 0..(if background { 600 } else { 0 }) {
        let (x, y) = (
            rng.range(10.0, PAGE_W - 10.0),
            rng.range(10.0, PAGE_H - 10.0),
        );
        let (dx, dy) = (rng.range(-6.0, 6.0), rng.range(-6.0, 6.0));
        if near_template(x, y) {
            continue; // the user boxes one clean symbol; stray line work inside the box is not the template
        }
        ops.push_str(&format!("{x:.3} {y:.3} m {:.3} {:.3} l S ", x + dx, y + dy));
    }
    for r in 0..n_rows {
        for c in 0..n_cols {
            let cx = 60.0 + c as f64 * 95.0;
            let cy = 60.0 + r as f64 * 95.0;
            let (jx, jy) = (rng.range(-jitter, jitter), rng.range(-jitter, jitter));
            symbol(&mut ops, Kind::Luminaire, cx + jx, cy + jy, R, 0.0);
            truth.push((cx, cy));
            // Decoys one third of the way to the next column.
            let dxp = cx + 45.0;
            match if background { (r + c) % 3 } else { 2 } {
                0 => symbol(&mut ops, Kind::DecoyOneArm, dxp, cy, R, 0.0),
                1 => symbol(&mut ops, Kind::DecoyRing, dxp, cy, R, 0.0),
                _ => symbol(&mut ops, Kind::DecoySquare, dxp, cy, R, 0.0),
            }
        }
    }
    let base_y = 60.0 + n_rows as f64 * 95.0 + 40.0;
    if with_turned {
        for (i, deg) in [90.0f64, 180.0, 270.0, 37.0].iter().enumerate() {
            let (cx, cy) = (60.0 + i as f64 * 95.0, base_y);
            symbol(&mut ops, Kind::Luminaire, cx, cy, R, deg.to_radians());
            truth.push((cx, cy));
        }
    }
    if with_scaled {
        for (i, k) in [0.6f64, 1.5, 1.9].iter().enumerate() {
            let (cx, cy) = (560.0 + i as f64 * 110.0, base_y);
            symbol(&mut ops, Kind::Luminaire, cx, cy, R * k, 0.0);
            truth.push((cx, cy));
        }
    }
    Drawing { ops, truth }
}

/// Content stream -> flat segments through the same helper the PDFium path uses.
fn parse_segments(ops: &str) -> Vec<f64> {
    let mut subs: Vec<Subpath> = Vec::new();
    let mut cur = Subpath::default();
    let mut nums: Vec<f64> = Vec::new();
    for tok in ops.split_whitespace() {
        match tok {
            "m" => {
                if !cur.points.is_empty() {
                    subs.push(std::mem::take(&mut cur));
                }
                cur.points.push(PdfPoint {
                    x: nums[0],
                    y: nums[1],
                });
                nums.clear();
            }
            "l" => {
                cur.points.push(PdfPoint {
                    x: nums[0],
                    y: nums[1],
                });
                nums.clear();
            }
            "h" => cur.closed = true,
            "S" => subs.push(std::mem::take(&mut cur)),
            n => nums.push(n.parse().unwrap()),
        }
    }
    subpaths_to_segments(&subs)
}

/// Template box around the first luminaire at (60, 60): the octagon, cross and tick fit inside
/// R * 1.5 + a margin.
fn template_box() -> SearchRect {
    SearchRect {
        x: 60.0 - R * 1.6,
        y: 60.0 - R * 1.6,
        w: R * 3.2,
        h: R * 3.2,
    }
}

fn near_template(x: f64, y: f64) -> bool {
    (x - 60.0).abs() < R * 2.5 && (y - 60.0).abs() < R * 2.5
}

struct Score {
    tp: usize,
    fp: usize,
    fn_: usize,
}
impl Score {
    fn precision(&self) -> f64 {
        if self.tp + self.fp == 0 {
            1.0
        } else {
            self.tp as f64 / (self.tp + self.fp) as f64
        }
    }
    fn recall(&self) -> f64 {
        if self.tp + self.fn_ == 0 {
            1.0
        } else {
            self.tp as f64 / (self.tp + self.fn_) as f64
        }
    }
}

/// Greedy one-to-one match of found symbols to truth points: a hit is right when it covers a
/// truth point (its box, grown by `tol` points, contains it). A symbol's own centre is not the
/// truth point (the tick shifts the lines' extent), so the box is the fair test.
fn score(found: &[SymbolMatch], truth: &[(f64, f64)], tol: f64) -> Score {
    let mut used = vec![false; found.len()];
    let mut tp = 0;
    for &(tx, ty) in truth {
        if let Some(i) = (0..found.len()).find(|&i| {
            let r = &found[i].rect;
            !used[i]
                && SearchRect {
                    x: r.x - tol,
                    y: r.y - tol,
                    w: r.w + 2.0 * tol,
                    h: r.h + 2.0 * tol,
                }
                .contains(tx, ty)
        }) {
            used[i] = true;
            tp += 1;
        }
    }
    Score {
        tp,
        fp: used.iter().filter(|u| !**u).count(),
        fn_: truth.len() - tp,
    }
}

fn run_vector(d: &Drawing, opts: &VisualSearchOptions) -> Vec<SymbolMatch> {
    let segs = parse_segments(&d.ops);
    let tpl = template_segments(&segs, &template_box(), 0.5);
    visual_search(&segs, &tpl, opts)
}

// ---------------------------------------------------------------------------
// Vector: pure tests
// ---------------------------------------------------------------------------

#[test]
fn subpaths_flatten_with_closing_edge() {
    let tri = Subpath {
        points: vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 10.0, y: 0.0 },
            PdfPoint { x: 0.0, y: 10.0 },
        ],
        closed: true,
        ..Default::default()
    };
    let open = Subpath {
        points: vec![PdfPoint { x: 1.0, y: 1.0 }, PdfPoint { x: 2.0, y: 2.0 }],
        closed: false,
        ..Default::default()
    };
    let segs = subpaths_to_segments(&[tri, open]);
    assert_eq!(
        segs.len(),
        4 * 4,
        "triangle has 3 edges incl. closing, open line has 1"
    );
    assert_eq!(
        &segs[8..12],
        &[0.0, 10.0, 0.0, 0.0],
        "closing edge goes last point -> first point"
    );
}

#[test]
fn non_finite_and_absurd_coordinates_are_dropped() {
    let sub = Subpath {
        points: vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint {
                x: f64::NAN,
                y: 1.0,
            },
            PdfPoint { x: 5.0, y: 5.0 },
            PdfPoint { x: 1.0e12, y: 5.0 },
        ],
        ..Default::default()
    };
    assert!(
        subpaths_to_segments(&[sub]).is_empty(),
        "every edge touches a bad point"
    );
}

#[test]
fn vector_precision_and_recall_on_clean_sheet() {
    // 7 x 5 grid = 35 luminaires (the template among them) + 35 decoys of three kinds.
    let d = drawing(7, 5, false, false, 0.0);
    assert_eq!(d.truth.len(), 35);
    let found = run_vector(&d, &VisualSearchOptions::default());
    let s = score(&(found), &d.truth, 2.0);
    assert_eq!(
        found.len(),
        35,
        "one hit per luminaire, no hits on the 35 decoys"
    );
    assert_eq!(
        (s.precision(), s.recall()),
        (1.0, 1.0),
        "tp={} fp={} fn={}",
        s.tp,
        s.fp,
        s.fn_
    );
}

#[test]
fn vector_decoys_alone_do_not_match() {
    let d = drawing(7, 5, false, false, 0.0);
    let found = run_vector(&d, &VisualSearchOptions::default());
    // No hit may sit where a decoy was drawn (45 pt right of a luminaire).
    let decoys: Vec<(f64, f64)> = d.truth.iter().map(|&(x, y)| (x + 45.0, y)).collect();
    let on_decoy = score(&(found), &decoys, 6.0).tp;
    assert_eq!(on_decoy, 0, "decoys matched as luminaires");
}

#[test]
fn vector_tolerates_endpoint_jitter() {
    // 0.2 pt of draughting noise on every symbol position stays inside the 0.75 pt tolerance.
    let d = drawing(5, 3, false, false, 0.2);
    let found = run_vector(&d, &VisualSearchOptions::default());
    let s = score(&(found), &d.truth, 2.0);
    assert_eq!(
        (s.precision(), s.recall()),
        (1.0, 1.0),
        "tp={} fp={} fn={}",
        s.tp,
        s.fp,
        s.fn_
    );
}

#[test]
fn vector_rotations_are_opt_in() {
    let d = drawing(4, 3, true, false, 0.0);
    let upright: Vec<_> = d.truth[..12].to_vec();
    let turned: Vec<_> = d.truth[12..].to_vec();
    assert_eq!(turned.len(), 4);

    let off = run_vector(&d, &VisualSearchOptions::default());
    let s_off = score(&(off), &d.truth, 2.0);
    assert_eq!(s_off.precision(), 1.0, "rotations off must not invent hits");
    assert_eq!(score(&(off), &upright, 2.0).recall(), 1.0);
    assert_eq!(
        score(&(off), &turned, 2.0).tp,
        0,
        "turned copies are not matched with rotations off"
    );

    let on = run_vector(
        &d,
        &VisualSearchOptions {
            rotations: true,
            ..Default::default()
        },
    );
    let s_on = score(&(on), &d.truth, 2.0);
    assert_eq!(
        (s_on.precision(), s_on.recall()),
        (1.0, 1.0),
        "tp={} fp={} fn={}",
        s_on.tp,
        s_on.fp,
        s_on.fn_
    );
    // The tick makes the turn unambiguous: 90 degrees counter-clockwise copy reports ~90.
    let m90 = on
        .iter()
        .find(|m| m.rect.contains(60.0, 60.0 + 3.0 * 95.0 + 40.0))
        .unwrap();
    assert!((m90.angle - 90.0).abs() < 0.5, "angle was {}", m90.angle);
}

#[test]
fn vector_scales_are_opt_in() {
    let d = drawing(4, 3, false, true, 0.0);
    assert_eq!(d.truth.len(), 15);
    let off = run_vector(&d, &VisualSearchOptions::default());
    let s_off = score(&(off), &d.truth, 2.0);
    assert_eq!(
        (s_off.precision(), s_off.tp),
        (1.0, 12),
        "only the as-drawn size without scales"
    );
    let on = run_vector(
        &d,
        &VisualSearchOptions {
            scales: true,
            ..Default::default()
        },
    );
    let s_on = score(&(on), &d.truth, 2.0);
    assert_eq!(
        (s_on.precision(), s_on.recall()),
        (1.0, 1.0),
        "tp={} fp={} fn={}",
        s_on.tp,
        s_on.fp,
        s_on.fn_
    );
}

#[test]
fn vector_min_score_admits_a_symbol_missing_one_line() {
    // The template has 12 lines. A copy missing one tick has 11 (92%): found at the default 0.9,
    // not at 1.0. A copy with one cross arm only has 9 (75%): never found.
    let mut ops = String::new();
    symbol(&mut ops, Kind::Luminaire, 60.0, 60.0, R, 0.0);
    symbol(&mut ops, Kind::Luminaire, 160.0, 60.0, R, 0.0);
    symbol(&mut ops, Kind::LuminaireOneTick, 260.0, 60.0, R, 0.0);
    symbol(&mut ops, Kind::DecoyOneArm, 360.0, 60.0, R, 0.0);
    let segs = parse_segments(&ops);
    let tpl = template_segments(&segs, &template_box(), 0.5);
    assert_eq!(tpl.len(), 12);
    let col = |ms: &[SymbolMatch]| -> Vec<i64> {
        ms.iter()
            .map(|m| ((m.center[0] - 60.0) / 100.0).round() as i64)
            .collect()
    };
    let default = visual_search(&segs, &tpl, &VisualSearchOptions::default());
    assert_eq!(
        col(&default),
        vec![0, 1, 2],
        "copy missing one line admitted, one-arm decoy rejected"
    );
    let strict = visual_search(
        &segs,
        &tpl,
        &VisualSearchOptions {
            min_score: 1.0,
            ..Default::default()
        },
    );
    assert_eq!(
        col(&strict),
        vec![0, 1],
        "min_score 1.0 rejects the copy missing a line"
    );
}

#[test]
fn vector_degenerate_templates_find_nothing() {
    let segs = parse_segments("0 0 m 10 0 l S ");
    assert!(visual_search(&segs, &[], &VisualSearchOptions::default()).is_empty());
    assert!(visual_search(
        &segs,
        &[[0.0, 0.0, 10.0, 0.0]],
        &VisualSearchOptions::default()
    )
    .is_empty());
    assert!(template_segments(
        &segs,
        &SearchRect {
            x: 50.0,
            y: 50.0,
            w: 5.0,
            h: 5.0
        },
        0.5
    )
    .is_empty());
}

#[test]
fn vector_large_page_still_exact() {
    // ~250k segments of hatch noise plus 300 symbols: correctness at size (timing is in the
    // ignored `timing_large_sheet`).
    let mut rng = Rng(42);
    let mut ops = String::new();
    for _ in 0..250_000 {
        let (x, y) = (rng.range(0.0, 3300.0), rng.range(0.0, 2300.0));
        let (dx, dy) = (rng.range(-5.0, 5.0), rng.range(-5.0, 5.0));
        if (x - 100.0).abs() < R * 2.5 && (y - 100.0).abs() < R * 2.5 {
            continue;
        }
        ops.push_str(&format!("{x:.3} {y:.3} m {:.3} {:.3} l S ", x + dx, y + dy));
    }
    let mut truth = Vec::new();
    for i in 0..300 {
        let (cx, cy) = (
            100.0 + (i % 30) as f64 * 100.0,
            100.0 + (i / 30) as f64 * 200.0,
        );
        symbol(&mut ops, Kind::Luminaire, cx, cy, R, 0.0);
        truth.push((cx, cy));
    }
    let segs = parse_segments(&ops);
    let tpl = template_segments(
        &segs,
        &SearchRect {
            x: 100.0 - R * 1.6,
            y: 100.0 - R * 1.6,
            w: R * 3.2,
            h: R * 3.2,
        },
        0.5,
    );
    let found = visual_search(&segs, &tpl, &VisualSearchOptions::default());
    let s = score(&(found), &truth, 2.0);
    assert_eq!(
        (s.precision(), s.recall()),
        (1.0, 1.0),
        "tp={} fp={} fn={}",
        s.tp,
        s.fp,
        s.fn_
    );
}

// ---------------------------------------------------------------------------
// Raster: pure tests on a synthetic scan
// ---------------------------------------------------------------------------

const PX_PER_PT: f64 = 3.0;

/// Draw the content stream's lines into a gray page (white paper, dark 2 px ink), y flipped.
fn rasterise(ops: &str, w_pts: f64, h_pts: f64) -> GrayImage {
    let (w, h) = ((w_pts * PX_PER_PT) as usize, (h_pts * PX_PER_PT) as usize);
    let mut data = vec![255u8; w * h];
    let segs = parse_segments(ops);
    for c in segs.chunks_exact(4) {
        let (x1, y1, x2, y2) = (
            c[0] * PX_PER_PT,
            (h_pts - c[1]) * PX_PER_PT,
            c[2] * PX_PER_PT,
            (h_pts - c[3]) * PX_PER_PT,
        );
        let n = ((x2 - x1).hypot(y2 - y1) * 2.0).ceil().max(1.0) as usize;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let (px, py) = ((x1 + (x2 - x1) * t) as i64, (y1 + (y2 - y1) * t) as i64);
            for dy in 0..2 {
                for dx in 0..2 {
                    let (xx, yy) = (px + dx, py + dy);
                    if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                        data[yy as usize * w + xx as usize] = 20;
                    }
                }
            }
        }
    }
    GrayImage {
        width: w,
        height: h,
        data,
    }
}

fn raster_fixture() -> (String, Vec<(f64, f64)>) {
    let mut ops = String::new();
    let mut truth = Vec::new();
    for r in 0..3 {
        for c in 0..4 {
            let (cx, cy) = (60.0 + c as f64 * 90.0, 60.0 + r as f64 * 90.0);
            symbol(&mut ops, Kind::Luminaire, cx, cy, R, 0.0);
            truth.push((cx, cy));
            symbol(&mut ops, Kind::DecoySquare, cx + 45.0, cy, R, 0.0);
        }
    }
    (ops, truth)
}

fn raster_hits(ms: &[raster::RasterMatch], page_h_pts: f64) -> Vec<SymbolMatch> {
    ms.iter()
        .map(|m| raster_to_symbol_match(m, PX_PER_PT, page_h_pts))
        .collect()
}

#[test]
fn raster_precision_and_recall_on_synthetic_scan() {
    let (ops, truth) = raster_fixture();
    let (pw, ph) = (440.0, 300.0);
    let page = rasterise(&ops, pw, ph);
    let tpl = raster_template(&page, &template_box(), PX_PER_PT, ph);
    let found = raster_search(&page, &tpl, &RasterSearchOptions::default());
    let s = score(&raster_hits(&found, ph), &truth, 3.0);
    assert_eq!(
        (s.precision(), s.recall()),
        (1.0, 1.0),
        "tp={} fp={} fn={}",
        s.tp,
        s.fp,
        s.fn_
    );
}

/// An "F": stem, long top arm, short middle arm. No symmetry, so a turned copy correlates poorly.
fn glyph_f(ops: &mut String, x: f64, y: f64, turns: u32) {
    let rot = |px: f64, py: f64| -> (f64, f64) {
        let (mut dx, mut dy) = (px - x, py - y);
        for _ in 0..turns {
            (dx, dy) = (-dy, dx);
        }
        (x + dx, y + dy)
    };
    for ((ax, ay), (bx, by)) in [
        ((0.0, -12.0), (0.0, 12.0)),
        ((0.0, 12.0), (12.0, 12.0)),
        ((0.0, 2.0), (7.0, 2.0)),
    ] {
        let (p, q) = (rot(x + ax, y + ay), rot(x + bx, y + by));
        ops.push_str(&format!(
            "{:.3} {:.3} m {:.3} {:.3} l S ",
            p.0, p.1, q.0, q.1
        ));
    }
}

#[test]
fn raster_rotations_find_quarter_turns() {
    let mut ops = String::new();
    let mut upright = Vec::new();
    let mut turned = Vec::new();
    for i in 0..6 {
        let (cx, cy) = (60.0 + (i % 3) as f64 * 80.0, 60.0 + (i / 3) as f64 * 80.0);
        glyph_f(&mut ops, cx, cy, 0);
        upright.push((cx, cy));
    }
    for (i, t) in [1u32, 2, 3].iter().enumerate() {
        let (cx, cy) = (60.0 + i as f64 * 80.0, 220.0);
        glyph_f(&mut ops, cx, cy, *t);
        turned.push((cx, cy));
    }
    let all: Vec<_> = upright.iter().chain(&turned).copied().collect();
    let (pw, ph) = (300.0, 280.0);
    let page = rasterise(&ops, pw, ph);
    let tbox = SearchRect {
        x: 60.0 - 6.0,
        y: 60.0 - 16.0,
        w: 24.0,
        h: 32.0,
    };
    let tpl = raster_template(&page, &tbox, PX_PER_PT, ph);
    let off = raster_search(&page, &tpl, &RasterSearchOptions::default());
    let s = score(&raster_hits(&off, ph), &upright, 4.0);
    assert_eq!(
        (off.len(), s.recall()),
        (6, 1.0),
        "rotations off: only the upright copies"
    );
    let on = raster_search(
        &page,
        &tpl,
        &RasterSearchOptions {
            rotations: true,
            ..Default::default()
        },
    );
    let s = score(&raster_hits(&on, ph), &all, 4.0);
    assert_eq!(
        (s.precision(), s.recall()),
        (1.0, 1.0),
        "tp={} fp={} fn={}",
        s.tp,
        s.fp,
        s.fn_
    );
}

#[test]
fn raster_blank_template_finds_nothing() {
    let page = GrayImage {
        width: 200,
        height: 200,
        data: vec![255; 200 * 200],
    };
    let tpl = GrayImage {
        width: 30,
        height: 30,
        data: vec![255; 900],
    };
    assert!(raster_search(&page, &tpl, &RasterSearchOptions::default()).is_empty());
}

#[test]
fn raster_match_maps_back_to_page_space() {
    let m = raster::RasterMatch {
        x: 30,
        y: 60,
        w: 90,
        h: 60,
        turns: 1,
        score: 0.9,
    };
    let sm = raster_to_symbol_match(&m, 3.0, 100.0);
    assert_eq!(
        sm.rect,
        SearchRect {
            x: 10.0,
            y: 100.0 - 40.0,
            w: 30.0,
            h: 20.0
        }
    );
    assert_eq!(sm.center, [25.0, 100.0 - 30.0]);
    assert_eq!(sm.angle, 90.0);
}

// ---------------------------------------------------------------------------
// PDFium: real extraction, real renderer, async entry point
// ---------------------------------------------------------------------------

fn pdfium_available() -> bool {
    std::env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_some()
}

/// One page per entry of `page_ops`, all `PAGE_W` x `PAGE_H`.
fn write_pdf(name: &str, page_ops: &[String], w: f64, h: f64) -> std::path::PathBuf {
    let sizes = vec![(w, h); page_ops.len()];
    write_pdf_sized(name, page_ops, &sizes)
}

/// Like `write_pdf` with a size per page.
fn write_pdf_sized(name: &str, page_ops: &[String], sizes: &[(f64, f64)]) -> std::path::PathBuf {
    let geoms: Vec<PageGeom> = sizes
        .iter()
        .map(|&(w, h)| PageGeom {
            w,
            h,
            ox: 0.0,
            oy: 0.0,
            rotate: 0,
        })
        .collect();
    write_pdf_geom(name, page_ops, &geoms)
}

/// A page's true MediaBox (`w` x `h` at lower-left `ox`, `oy`) and `/Rotate`.
#[derive(Clone, Copy)]
struct PageGeom {
    w: f64,
    h: f64,
    ox: f64,
    oy: f64,
    rotate: i64,
}

fn write_pdf_geom(name: &str, page_ops: &[String], geoms: &[PageGeom]) -> std::path::PathBuf {
    use lopdf::{dictionary, Document, Object, Stream};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids: Vec<Object> = Vec::new();
    for (ops, g) in page_ops.iter().zip(geoms) {
        let content_id = doc.add_object(Stream::new(dictionary! {}, ops.as_bytes().to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Real(g.ox as f32),
                Object::Real(g.oy as f32),
                Object::Real((g.ox + g.w) as f32),
                Object::Real((g.oy + g.h) as f32),
            ],
            "Rotate" => g.rotate,
            "Contents" => content_id,
        });
        kids.push(page_id.into());
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.compress();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    doc.save(&path).expect("save fixture");
    std::mem::forget(dir);
    path
}

#[test]
fn pdfium_extraction_matches_the_content_stream_and_finds_every_symbol() {
    if !pdfium_available() {
        eprintln!("skip symbol_search pdfium: no PDFIUM_DYNAMIC_LIB_PATH");
        return;
    }
    use crate::render::{OpenOutcome, RenderEngine};
    let d = drawing(7, 5, true, true, 0.0);
    let path = write_pdf("symbols.pdf", std::slice::from_ref(&d.ops), PAGE_W, PAGE_H);
    let mut e = RenderEngine::new().expect("pdfium");
    e.open_document(path, "t".into(), None)
        .and_then(OpenOutcome::into_page_count)
        .expect("open");
    let segs = e.page_segments("t", 0).expect("page_segments");
    // PDFium reports the same line work the content stream drew (f32 rounding aside).
    assert_eq!(
        segs.len(),
        parse_segments(&d.ops).len(),
        "segment count differs from the content stream"
    );
    let tpl = template_segments(&segs, &template_box(), 0.5);
    assert_eq!(tpl.len(), 12, "octagon 8 + cross 2 + ticks 2");
    let opts = VisualSearchOptions {
        rotations: true,
        scales: true,
        ..Default::default()
    };
    let found = visual_search(&segs, &tpl, &opts);
    let s = score(&(found), &d.truth, 2.0);
    assert_eq!(d.truth.len(), 35 + 4 + 3);
    assert_eq!(
        (s.precision(), s.recall()),
        (1.0, 1.0),
        "tp={} fp={} fn={}",
        s.tp,
        s.fp,
        s.fn_
    );
}

#[tokio::test]
async fn pdfium_search_entry_point_vector_over_a_sheet_set_and_raster_fallback() {
    if !pdfium_available() {
        eprintln!("skip symbol_search entry point: no PDFIUM_DYNAMIC_LIB_PATH");
        return;
    }
    let sheet1 = drawing_bg(6, 4, false, false, 0.0, false); // 24
    let sheet2 = drawing_bg(3, 2, false, false, 0.0, false); // 6
    let path = write_pdf(
        "set.pdf",
        &[sheet1.ops.clone(), sheet2.ops.clone()],
        PAGE_W,
        PAGE_H,
    );
    let render = RenderHandle::spawn().expect("render thread");
    render
        .open_document(path, "set".into(), None)
        .await
        .expect("open");
    let req = |mode, pages: Option<Vec<u32>>| SymbolSearchRequest {
        doc_id: "set".into(),
        template_page: 0,
        rect: template_box(),
        pages,
        mode,
        rotations: false,
        scales: false,
        tolerance: None,
        min_score: None,
        raster_threshold: None,
        raster_dpi: Some(150.0),
    };

    let one = search(&render, req(SearchMode::Auto, None))
        .await
        .expect("page search");
    assert_eq!(one.method, "vector");
    assert_eq!(one.pages.len(), 1);
    assert_eq!(one.pages[0].matches.len(), 24);

    let set = search(&render, req(SearchMode::Auto, Some(vec![0, 1])))
        .await
        .expect("set search");
    let counts: Vec<usize> = set.pages.iter().map(|p| p.matches.len()).collect();
    assert_eq!(counts, vec![24, 6], "per-sheet counts");
    for (page, truth) in [(0usize, &sheet1.truth), (1, &sheet2.truth)] {
        let s = score(&(set.pages[page].matches), truth, 2.0);
        assert_eq!(
            (s.precision(), s.recall()),
            (1.0, 1.0),
            "page {page}: tp={} fp={} fn={}",
            s.tp,
            s.fp,
            s.fn_
        );
    }

    // The raster path on the same PDF: a real PDFium render, correlated against the template.
    // Default correlation threshold. Look-alike decoys (an octagon with one cross arm shares most of
    // the luminaire's ink) are NOT separated by correlation; the raster fixture therefore uses
    // clearly different decoys. Vector search is the primary path and tells such symbols apart.
    let raster_req = req(SearchMode::Raster, Some(vec![0, 1]));
    let ras = search(&render, raster_req).await.expect("raster search");
    assert_eq!(ras.method, "raster");
    for (page, truth) in [(0usize, &sheet1.truth), (1, &sheet2.truth)] {
        let s = score(&(ras.pages[page].matches), truth, 6.0);
        assert_eq!(
            (s.precision(), s.recall()),
            (1.0, 1.0),
            "raster page {page}: tp={} fp={} fn={}",
            s.tp,
            s.fp,
            s.fn_
        );
    }
}

/// A scanned sheet: the raster fixture embedded as a single grayscale image (no vector paths).
fn write_scan_pdf(img: &GrayImage, w_pts: f64, h_pts: f64) -> std::path::PathBuf {
    use lopdf::{dictionary, Document, Object, Stream};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let image_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image",
            "Width" => img.width as i64, "Height" => img.height as i64,
            "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8,
        },
        img.data.clone(),
    ));
    let content = format!("q {w_pts} 0 0 {h_pts} 0 0 cm /Im0 Do Q");
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), Object::Real(w_pts as f32), Object::Real(h_pts as f32)],
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 },
        ),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scan.pdf");
    doc.save(&path).expect("save scan");
    std::mem::forget(dir);
    path
}

fn request(doc: &str, mode: SearchMode, rect: SearchRect) -> SymbolSearchRequest {
    SymbolSearchRequest {
        doc_id: doc.into(),
        template_page: 0,
        rect,
        pages: None,
        mode,
        rotations: false,
        scales: false,
        tolerance: None,
        min_score: None,
        raster_threshold: None,
        raster_dpi: None,
    }
}

#[tokio::test]
async fn auto_mode_uses_the_raster_path_on_a_scanned_sheet() {
    if !pdfium_available() {
        eprintln!("skip symbol_search scan: no PDFIUM_DYNAMIC_LIB_PATH");
        return;
    }
    let (ops, truth) = raster_fixture();
    let (pw, ph) = (440.0, 300.0);
    let path = write_scan_pdf(&rasterise(&ops, pw, ph), pw, ph);
    let render = RenderHandle::spawn().expect("render thread");
    render
        .open_document(path, "scan".into(), None)
        .await
        .expect("open");
    // The page has no vector paths at all.
    assert!(render
        .page_segments("scan".into(), 0)
        .await
        .unwrap()
        .is_empty());
    let r = search(&render, request("scan", SearchMode::Auto, template_box()))
        .await
        .expect("search");
    assert_eq!((r.method, r.template_segments), ("raster", 0));
    let s = score(&r.pages[0].matches, &truth, 6.0);
    assert_eq!(
        (s.precision(), s.recall()),
        (1.0, 1.0),
        "scan: tp={} fp={} fn={}",
        s.tp,
        s.fp,
        s.fn_
    );
}

#[tokio::test]
async fn a_blank_box_is_refused_instead_of_searched() {
    if !pdfium_available() {
        eprintln!("skip symbol_search blank box: no PDFIUM_DYNAMIC_LIB_PATH");
        return;
    }
    let (ops, _) = raster_fixture();
    let (pw, ph) = (440.0, 300.0);
    let path = write_scan_pdf(&rasterise(&ops, pw, ph), pw, ph);
    let render = RenderHandle::spawn().expect("render thread");
    render
        .open_document(path, "blank".into(), None)
        .await
        .expect("open");
    let blank = SearchRect {
        x: 380.0,
        y: 250.0,
        w: 40.0,
        h: 40.0,
    };
    let err = search(&render, request("blank", SearchMode::Auto, blank))
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("blank paper"), "{err:#}");
}

#[test]
fn raster_blank_and_flat_templates_do_not_flood_candidates() {
    // A near-flat template with sparse speckle: refused up front, instantly.
    let mut data = vec![255u8; 60 * 60];
    data[100] = 200;
    let tpl = GrayImage {
        width: 60,
        height: 60,
        data,
    };
    let page = GrayImage {
        width: 800,
        height: 800,
        data: vec![250; 800 * 800],
    };
    let t = std::time::Instant::now();
    assert!(raster_search(&page, &tpl, &RasterSearchOptions::default()).is_empty());
    assert!(t.elapsed().as_millis() < 500);
}

/// Timing on a real-size sheet. `cargo test --release -- --ignored --nocapture timing_large_sheet`.
/// An A0 drawing (2384 x 3370 pt) with ~600k line segments of dense CAD-style linework, 600
/// luminaires among them, a 1-page and a 10-sheet run.
#[tokio::test]
#[ignore]
async fn timing_large_sheet() {
    if !pdfium_available() {
        eprintln!("skip timing: no PDFIUM_DYNAMIC_LIB_PATH");
        return;
    }
    let (w, h) = (3370.0, 2384.0);
    let mut rng = Rng(7);
    let mut ops = String::new();
    for _ in 0..600_000 {
        let (x, y) = (rng.range(0.0, w), rng.range(0.0, h));
        let (dx, dy) = (rng.range(-8.0, 8.0), rng.range(-8.0, 8.0));
        if (x - 150.0).abs() < R * 2.5 && (y - 150.0).abs() < R * 2.5 {
            continue;
        }
        ops.push_str(&format!("{x:.3} {y:.3} m {:.3} {:.3} l S ", x + dx, y + dy));
    }
    let mut truth = Vec::new();
    for i in 0..600 {
        let (cx, cy) = (
            150.0 + (i % 30) as f64 * 105.0,
            150.0 + (i / 30) as f64 * 105.0,
        );
        symbol(&mut ops, Kind::Luminaire, cx, cy, R, 0.0);
        truth.push((cx, cy));
    }
    let sheet = ops;
    let t0 = std::time::Instant::now();
    let path = write_pdf("a0.pdf", &vec![sheet; 10], w, h);
    eprintln!(
        "fixture: 10 x A0 sheets, {:.1} MB pdf, built in {:?}",
        std::fs::metadata(&path).unwrap().len() as f64 / 1e6,
        t0.elapsed()
    );
    let render = RenderHandle::spawn().expect("render thread");
    render
        .open_document(path, "a0".into(), None)
        .await
        .expect("open");
    let rect = SearchRect {
        x: 150.0 - R * 1.6,
        y: 150.0 - R * 1.6,
        w: R * 3.2,
        h: R * 3.2,
    };
    let mk = |pages| SymbolSearchRequest {
        doc_id: "a0".into(),
        template_page: 0,
        rect,
        pages,
        mode: SearchMode::Vector,
        rotations: false,
        scales: false,
        tolerance: None,
        min_score: None,
        raster_threshold: None,
        raster_dpi: None,
    };
    for (label, pages) in [
        ("1 A0 page", None),
        ("10 A0 pages", Some((0..10).collect::<Vec<u32>>())),
    ] {
        let t = std::time::Instant::now();
        let r = search(&render, mk(pages)).await.expect("search");
        let total: usize = r.pages.iter().map(|p| p.matches.len()).sum();
        let s = score(&(r.pages[0].matches), &truth, 2.0);
        eprintln!(
            "TIMING vector {label}: wall {:?} (reported {} ms), {} hits, p={} r={}",
            t.elapsed(),
            r.elapsed_ms,
            total,
            s.precision(),
            s.recall()
        );
        assert_eq!((s.precision(), s.recall()), (1.0, 1.0));
    }
    let t = std::time::Instant::now();
    let mut r = mk(None);
    r.rotations = true;
    let rr = search(&render, r).await.expect("search");
    eprintln!(
        "TIMING vector 1 A0 page with rotations: wall {:?}, {} hits",
        t.elapsed(),
        rr.pages[0].matches.len()
    );
    let t = std::time::Instant::now();
    let mut r = mk(None);
    r.mode = SearchMode::Raster;
    let rr = search(&render, r).await.expect("raster");
    eprintln!(
        "TIMING raster 1 A0 page @150dpi: wall {:?}, {} hits",
        t.elapsed(),
        rr.pages[0].matches.len()
    );
}

#[test]
fn raster_cap_is_per_page_size() {
    assert!(!raster_over_cap(1200.0, 800.0, 150.0));
    assert!(
        !raster_over_cap(2384.0, 3370.0, 150.0),
        "A0 at 150 dpi is allowed"
    );
    assert!(
        raster_over_cap(14000.0, 14000.0, 150.0),
        "a 14000 pt square page is not"
    );
    assert!(raster_over_cap(f64::NAN, 100.0, 150.0));
}

#[tokio::test]
async fn raster_skips_only_the_oversized_page_in_a_mixed_size_set() {
    if !pdfium_available() {
        eprintln!("skip symbol_search mixed sizes: no PDFIUM_DYNAMIC_LIB_PATH");
        return;
    }
    let d = drawing_bg(3, 2, false, false, 0.0, false);
    let pages = vec![d.ops.clone(), String::new(), d.ops.clone()];
    // Small, hostile-huge, small: the cap must be checked against each page, not the first.
    let sizes = [(PAGE_W, PAGE_H), (14000.0, 14000.0), (PAGE_W, PAGE_H)];
    let path = write_pdf_sized("mixed.pdf", &pages, &sizes);
    let render = RenderHandle::spawn().expect("render thread");
    render
        .open_document(path, "mixed".into(), None)
        .await
        .expect("open");
    let mut req = request("mixed", SearchMode::Raster, template_box());
    req.pages = Some(vec![0, 1, 2]);
    let r = search(&render, req).await.expect("search");
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.skipped[0].page, 1);
    assert!(r.skipped[0].reason.contains("too large"));
    let got: Vec<(u32, usize)> = r.pages.iter().map(|p| (p.page, p.matches.len())).collect();
    assert_eq!(
        got,
        vec![(0, 6), (2, 6)],
        "the small pages are still searched"
    );
}

/// Where local (unrotated, origin-relative) point (x, y) lands in DISPLAY space, written out by
/// hand from the PDF spec (ISO 32000-1 8.4.1 / 12.? /Rotate is clockwise) rather than through
/// the crate's own `true_to_display`, so the test is not the implementation checking itself.
fn display_of(rotate: i64, w: f64, h: f64, x: f64, y: f64) -> (f64, f64) {
    match rotate {
        90 => (y, w - x),
        180 => (w - x, h - y),
        270 => (h - y, x),
        _ => (x, y),
    }
}

#[tokio::test]
async fn vector_and_raster_agree_on_rotated_and_offset_sheets() {
    if !pdfium_available() {
        eprintln!("skip symbol_search geometry: no PDFIUM_DYNAMIC_LIB_PATH");
        return;
    }
    let local = drawing_bg(4, 3, false, false, 0.0, false); // 12 luminaires + 12 square decoys
    let render = RenderHandle::spawn().expect("render thread");
    // (rotate, origin): unrotated at the origin, offset origin only, each rotation with and
    // without an offset origin.
    let cases: [(i64, f64, f64); 6] = [
        (0, 0.0, 0.0),
        (0, 100.0, 50.0),
        (90, 0.0, 0.0),
        (90, 100.0, 50.0),
        (180, 30.0, 20.0),
        (270, 30.0, 20.0),
    ];
    for (i, &(rotate, ox, oy)) in cases.iter().enumerate() {
        // Content is drawn in the local frame, shifted into true space by a translation.
        let ops = format!("1 0 0 1 {ox} {oy} cm {}", local.ops);
        let g = PageGeom {
            w: PAGE_W,
            h: PAGE_H,
            ox,
            oy,
            rotate,
        };
        let path = write_pdf_geom(&format!("geom{i}.pdf"), &[ops], &[g]);
        let doc = format!("geom{i}");
        render
            .open_document(path, doc.clone(), None)
            .await
            .expect("open");

        let truth: Vec<(f64, f64)> = local
            .truth
            .iter()
            .map(|&(x, y)| display_of(rotate, PAGE_W, PAGE_H, x, y))
            .collect();
        let (cx, cy) = truth[0];
        let half = R * 1.6;
        let rect = SearchRect {
            x: cx - half,
            y: cy - half,
            w: 2.0 * half,
            h: 2.0 * half,
        };

        let mut hits = Vec::new();
        for mode in [SearchMode::Vector, SearchMode::Raster] {
            let r = search(&render, request(&doc, mode, rect))
                .await
                .unwrap_or_else(|e| panic!("rotate={rotate} origin=({ox},{oy}) {mode:?}: {e:#}"));
            let s = score(&r.pages[0].matches, &truth, 6.0);
            assert_eq!(
                (s.precision(), s.recall()),
                (1.0, 1.0),
                "rotate={rotate} origin=({ox},{oy}) {mode:?}: tp={} fp={} fn={}",
                s.tp,
                s.fp,
                s.fn_
            );
            hits.push(r.pages[0].matches.clone());
        }
        // Both paths report the same places, in the same (display) space: every vector hit has a
        // raster hit within 8 pt (the two order and centre their boxes differently).
        for v in &hits[0] {
            assert!(
                hits[1]
                    .iter()
                    .any(|r| (v.center[0] - r.center[0]).abs() < 8.0
                        && (v.center[1] - r.center[1]).abs() < 8.0),
                "rotate={rotate} origin=({ox},{oy}): vector hit {:?} has no raster hit",
                v.center
            );
        }
        assert_eq!(hits[0].len(), hits[1].len());
    }
}

fn outcome(n: usize) -> SearchOutcome {
    SearchOutcome {
        matches: (0..n)
            .map(|k| SymbolMatch {
                center: [k as f64, 0.0],
                rect: SearchRect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
                angle: 0.0,
                score: 1.0,
                scale: 1.0,
            })
            .collect(),
        truncated: false,
    }
}

fn job(o: SearchOutcome) -> MatchJob {
    tokio::task::spawn_blocking(move || o)
}

#[tokio::test]
async fn a_failed_page_task_skips_that_page_and_keeps_the_rest() {
    let mut col = Collector::new(1000, Arc::new(AtomicBool::new(false)));
    let bad: MatchJob = tokio::task::spawn_blocking(|| panic!("boom"));
    col.settle(0, job(outcome(0))).await;
    col.settle(1, bad).await;
    col.settle(2, job(outcome(0))).await;
    assert_eq!(
        col.out.iter().map(|p| p.page).collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert_eq!(col.skipped.len(), 1);
    assert_eq!(col.skipped[0].page, 1);
    assert!(
        col.skipped[0].reason.starts_with("matching failed"),
        "{}",
        col.skipped[0].reason
    );
}

#[tokio::test]
async fn the_total_cap_truncates_cancels_the_rest_and_skips_later_pages() {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut col = Collector::new(5, Arc::clone(&cancel));
    col.settle(0, job(outcome(3))).await;
    assert!(!col.full() && !cancel.load(Ordering::SeqCst));
    col.settle(1, job(outcome(3))).await;
    assert_eq!(col.total, 5, "second page cut to the 2 that fit");
    assert_eq!(col.out[1].matches.len(), 2);
    assert!(col.out[1].truncated && col.truncated);
    assert!(
        col.full() && cancel.load(Ordering::SeqCst),
        "pages still matching are told to stop"
    );
    col.skip_for_cap(2);
    assert_eq!(col.skipped[0].reason, "result limit reached");
}

/// A hatch-filled sheet: `n x n` parallel equal-length lines, 3 pt apart. Boxing four of them as the
/// "symbol" makes every shifted group of four a genuine copy (~n*n matches).
fn hatch_sheet(n: usize) -> (Vec<f64>, Vec<Seg>) {
    let mut segs = Vec::with_capacity(n * n * 4);
    for col in 0..n {
        for row in 0..n {
            let (x, y) = (col as f64 * 3.0, row as f64 * 14.0);
            segs.extend_from_slice(&[x, y, x, y + 10.0]);
        }
    }
    let tpl = template_segments(
        &segs,
        &SearchRect {
            x: -1.0,
            y: -1.0,
            w: 11.0,
            h: 12.0,
        },
        0.5,
    );
    (segs, tpl)
}

#[test]
fn hatch_fill_returns_a_truncated_result_within_a_time_bound() {
    let (segs, tpl) = hatch_sheet(400); // 160 000 hatch lines
    assert_eq!(tpl.len(), 4);
    let limits = SearchLimits {
        max_matches: Some(2_000),
        deadline: Some(Instant::now() + Duration::from_secs(20)),
        cancel: None,
    };
    let t = Instant::now();
    let o = visual_search_limited(&segs, &tpl, &VisualSearchOptions::default(), &limits);
    let took = t.elapsed();
    assert!(o.truncated, "a hatch sheet has far more than 2000 copies");
    assert_eq!(o.matches.len(), 2_000);
    assert!(took < Duration::from_secs(10), "took {took:?}");
}

#[test]
fn an_expired_deadline_or_a_cancel_stops_the_matcher_at_once() {
    let (segs, tpl) = hatch_sheet(300);
    let past = SearchLimits {
        max_matches: None,
        deadline: Some(Instant::now()),
        cancel: None,
    };
    let o = visual_search_limited(&segs, &tpl, &VisualSearchOptions::default(), &past);
    assert!(o.truncated && o.matches.is_empty());
    let flag = Arc::new(AtomicBool::new(true));
    let cancelled = SearchLimits {
        max_matches: None,
        deadline: None,
        cancel: Some(flag),
    };
    let o = visual_search_limited(&segs, &tpl, &VisualSearchOptions::default(), &cancelled);
    assert!(o.truncated && o.matches.is_empty());
}

#[test]
fn the_raster_matcher_honours_the_deadline() {
    let (ops, _) = raster_fixture();
    let page = rasterise(&ops, 440.0, 300.0);
    let tpl = raster_template(&page, &template_box(), PX_PER_PT, 300.0);
    let opts = RasterSearchOptions {
        deadline: Some(Instant::now()),
        ..Default::default()
    };
    let (found, truncated) = raster::raster_search_limited(&page, &tpl, &opts);
    assert!(truncated && found.is_empty());
}

#[tokio::test]
async fn matching_concurrency_is_bounded_and_every_page_is_still_searched() {
    if !pdfium_available() {
        eprintln!("skip symbol_search concurrency: no PDFIUM_DYNAMIC_LIB_PATH");
        return;
    }
    let d = drawing_bg(3, 2, false, false, 0.0, false);
    let pages: Vec<String> = (0..12).map(|_| d.ops.clone()).collect();
    let path = write_pdf("many.pdf", &pages, PAGE_W, PAGE_H);
    let render = RenderHandle::spawn().expect("render thread");
    render
        .open_document(path, "many".into(), None)
        .await
        .expect("open");
    let probe = Arc::new(Probe {
        delay_ms: 60,
        ..Default::default()
    });
    let limits = Limits {
        max_in_flight: 2,
        probe: Some(Arc::clone(&probe)),
        ..Default::default()
    };
    let mut req = request("many", SearchMode::Vector, template_box());
    req.pages = Some((0..12).collect());
    let r = search_with_limits(&render, req, limits)
        .await
        .expect("search");
    assert_eq!(r.pages.len(), 12);
    assert!(r.pages.iter().all(|p| p.matches.len() == 6));
    assert_eq!(probe.calls.load(Ordering::SeqCst), 12);
    assert_eq!(
        probe.peak.load(Ordering::SeqCst),
        2,
        "12 pages with max_in_flight 2: never more than 2 matching at once, and 2 do overlap"
    );
}

#[tokio::test]
async fn limits_skip_dense_pages_and_stop_at_the_total_cap() {
    if !pdfium_available() {
        eprintln!("skip symbol_search limits: no PDFIUM_DYNAMIC_LIB_PATH");
        return;
    }
    let d = drawing_bg(3, 2, false, false, 0.0, false); // 6 per page
    let pages: Vec<String> = (0..4).map(|_| d.ops.clone()).collect();
    let path = write_pdf("cap.pdf", &pages, PAGE_W, PAGE_H);
    let render = RenderHandle::spawn().expect("render thread");
    render
        .open_document(path, "cap".into(), None)
        .await
        .expect("open");
    let mut req = request("cap", SearchMode::Vector, template_box());
    req.pages = Some(vec![0, 1, 2, 3]);

    // Total cap 8: page 0 gives 6, page 1 is cut to 2, pages 2 and 3 are not searched.
    let limits = Limits {
        max_matches_total: 8,
        max_in_flight: 1,
        ..Default::default()
    };
    let r = search_with_limits(&render, req.clone(), limits)
        .await
        .expect("search");
    assert!(r.truncated);
    let counts: Vec<usize> = r.pages.iter().map(|p| p.matches.len()).collect();
    assert_eq!(counts, vec![6, 2]);
    assert_eq!(
        r.skipped.iter().map(|s| s.page).collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert!(r.skipped.iter().all(|s| s.reason == "result limit reached"));

    // Every page is over a 10-segment cap: all are skipped, each with its reason.
    let limits = Limits {
        max_segments_per_page: 10,
        ..Default::default()
    };
    let r = search_with_limits(&render, req.clone(), limits)
        .await
        .expect("search");
    assert_eq!(r.skipped.len(), 4);
    assert!(
        r.skipped
            .iter()
            .all(|s| s.reason.starts_with("too many line segments")),
        "{:?}",
        r.skipped
    );

    // A box outside the page, or not finite, is refused before any work.
    for rect in [
        SearchRect {
            x: 5000.0,
            y: 5000.0,
            w: 20.0,
            h: 20.0,
        },
        SearchRect {
            x: -100.0,
            y: -100.0,
            w: 50.0,
            h: 50.0,
        },
        SearchRect {
            x: f64::NAN,
            y: 0.0,
            w: 20.0,
            h: 20.0,
        },
    ] {
        let mut bad = req.clone();
        bad.rect = rect;
        assert!(
            search(&render, bad).await.is_err(),
            "{rect:?} should be refused"
        );
    }
}
