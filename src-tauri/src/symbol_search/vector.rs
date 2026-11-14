// Adapted from redcolumn (Apache-2.0), packages/measure/src/visualSearch.ts, modified:
// ported from TypeScript to Rust (flat f64 segment arrays, integer cell keys), segment
// storage is index-based, and the page-wide match runs off the UI thread.
//! Symbol Search on vector line work.
//!
//! Every place a luminaire symbol (the line work inside the user's box) appears on a page,
//! the original included. The template's longest line anchors the search: each page line of
//! the same length is tried as its image (both ways round), and the match is kept when
//! enough of the template's other lines are present after the same move (and turn, with
//! `rotations`; and resize, with `scales`).
//!
//! Input is a flat `[x1, y1, x2, y2, ...]` segment array in PDF page space, as returned by
//! `RenderEngine::page_segments`. Pure math: no PDFium dependency.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Axis-aligned box in page space (`x`, `y` is the lower-left corner in PDFium page space).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SearchRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl SearchRect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

/// One found copy of the symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolMatch {
    /// Centre of the matched symbol.
    pub center: [f64; 2],
    /// Axis-aligned bounds of the matched symbol's lines.
    pub rect: SearchRect,
    /// Rotation from the template, in degrees (0 when rotations are off).
    pub angle: f64,
    /// Share of the template's lines found (0-1). Raster matches report the correlation.
    pub score: f64,
    /// Size relative to the template (1 unless scaled copies are searched for).
    pub scale: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualSearchOptions {
    /// Find the symbol at any rotation, not just as drawn.
    pub rotations: bool,
    /// How far matching endpoints may be apart, in points.
    pub tolerance: f64,
    /// Share of the template's lines a match must have.
    pub min_score: f64,
    /// Also find copies drawn larger or smaller (half to twice the size).
    pub scales: bool,
}

impl Default for VisualSearchOptions {
    fn default() -> Self {
        VisualSearchOptions {
            rotations: false,
            tolerance: 0.75,
            min_score: 0.9,
            scales: false,
        }
    }
}

/// Work bounds for one page's match. `Default` is unbounded (what the plain [`visual_search`] uses).
#[derive(Debug, Clone, Default)]
pub struct SearchLimits {
    /// Stop after this many matches (counted after de-duplication).
    pub max_matches: Option<usize>,
    /// Stop at this time and return what was found.
    pub deadline: Option<Instant>,
    /// Stop as soon as this is set (the caller dropped or finished with the search).
    pub cancel: Option<Arc<AtomicBool>>,
}

impl SearchLimits {
    fn stopped(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
            || self.deadline.is_some_and(|d| Instant::now() >= d)
    }
}

/// A page's matches, and whether the search stopped early (match cap, deadline or cancel) so
/// the list may be incomplete.
#[derive(Debug, Clone, Default)]
pub struct SearchOutcome {
    pub matches: Vec<SymbolMatch>,
    pub truncated: bool,
}

pub type Seg = [f64; 4];

#[inline]
fn seg_len(s: &Seg) -> f64 {
    (s[2] - s[0]).hypot(s[3] - s[1])
}

/// The line work fully inside a box: the symbol to look for.
pub fn template_segments(segments: &[f64], b: &SearchRect, margin: f64) -> Vec<Seg> {
    let inside = |x: f64, y: f64| {
        x >= b.x - margin && x <= b.x + b.w + margin && y >= b.y - margin && y <= b.y + b.h + margin
    };
    segments
        .chunks_exact(4)
        .filter_map(|c| {
            let s: Seg = [c[0], c[1], c[2], c[3]];
            (inside(s[0], s[1]) && inside(s[2], s[3]) && seg_len(&s) > 1e-3).then_some(s)
        })
        .collect()
}

/// Page segments hashed by midpoint, for "is there a segment with these endpoints?" lookups.
struct SegmentHash<'a> {
    cells: HashMap<(i64, i64), Vec<u32>>,
    segs: &'a [f64],
    cell: f64,
}

impl<'a> SegmentHash<'a> {
    fn new(segs: &'a [f64], cell: f64) -> Self {
        let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        for (i, c) in segs.chunks_exact(4).enumerate() {
            let k = (
                (((c[0] + c[2]) / 2.0) / cell).floor() as i64,
                (((c[1] + c[3]) / 2.0) / cell).floor() as i64,
            );
            cells.entry(k).or_default().push(i as u32);
        }
        SegmentHash { cells, segs, cell }
    }

    /// Whether a segment joins (ax, ay) and (bx, by), either way round, within `tol`.
    fn has(&self, ax: f64, ay: f64, bx: f64, by: f64, tol: f64) -> bool {
        let cx = (((ax + bx) / 2.0) / self.cell).floor() as i64;
        let cy = (((ay + by) / 2.0) / self.cell).floor() as i64;
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(list) = self.cells.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &i in list {
                    let o = i as usize * 4;
                    let (x1, y1, x2, y2) = (
                        self.segs[o],
                        self.segs[o + 1],
                        self.segs[o + 2],
                        self.segs[o + 3],
                    );
                    if ((x1 - ax).hypot(y1 - ay) <= tol && (x2 - bx).hypot(y2 - by) <= tol)
                        || ((x1 - bx).hypot(y1 - by) <= tol && (x2 - ax).hypot(y2 - ay) <= tol)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Every place the symbol (`template`, see [`template_segments`]) appears in `segments`, the
/// original included, one match per place, ordered top to bottom then left to right (page
/// space y ascending).
pub fn visual_search(
    segments: &[f64],
    template: &[Seg],
    options: &VisualSearchOptions,
) -> Vec<SymbolMatch> {
    visual_search_limited(segments, template, options, &SearchLimits::default()).matches
}

/// [`visual_search`] with work bounds. On a hatch-filled sheet every hatch line can be a genuine
/// copy of a boxed hatch fragment (hundreds of thousands of matches), so the raw match list,
/// the de-duplication and the time spent are all bounded by `limits`.
pub fn visual_search_limited(
    segments: &[f64],
    template: &[Seg],
    options: &VisualSearchOptions,
    limits: &SearchLimits,
) -> SearchOutcome {
    if template.len() < 2 {
        return SearchOutcome::default();
    }
    // Raw (pre-dedup) matches kept: symmetric symbols match several ways, so allow some slack
    // over the cap, but never an unbounded list.
    let raw_cap = limits.max_matches.map(|m| m.saturating_mul(8).max(64));
    let mut truncated = false;
    let tol = options.tolerance;
    let min_score = options.min_score;
    let (anchor_idx, anchor) =
        template
            .iter()
            .enumerate()
            .fold((0, &template[0]), |(bi, b), (i, s)| {
                if seg_len(s) > seg_len(b) {
                    (i, s)
                } else {
                    (bi, b)
                }
            });
    let l = seg_len(anchor);
    let a_angle = (anchor[3] - anchor[1]).atan2(anchor[2] - anchor[0]);
    let amx = (anchor[0] + anchor[2]) / 2.0;
    let amy = (anchor[1] + anchor[3]) / 2.0;
    // Template geometry relative to the anchor's midpoint.
    let rel: Vec<Seg> = template
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != anchor_idx)
        .map(|(_, s)| [s[0] - amx, s[1] - amy, s[2] - amx, s[3] - amy])
        .collect();
    // The symbol's own extent (its lines), not the looser box drawn around it.
    let mut minx = f64::INFINITY;
    let mut miny = f64::INFINITY;
    let mut maxx = f64::NEG_INFINITY;
    let mut maxy = f64::NEG_INFINITY;
    for t in template {
        for (x, y) in [(t[0], t[1]), (t[2], t[3])] {
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
    }
    let (tbx, tby, tbw, tbh) = (minx, miny, maxx - minx, maxy - miny);
    let corners = [
        (tbx - amx, tby - amy),
        (tbx + tbw - amx, tby - amy),
        (tbx + tbw - amx, tby + tbh - amy),
        (tbx - amx, tby + tbh - amy),
    ];
    let center_rel = (tbx + tbw / 2.0 - amx, tby + tbh / 2.0 - amy);
    let hash = SegmentHash::new(segments, (4.0 * tol).max(l / 2.0).max(2.0));
    let angle_tol = (2.0 * tol / l.max(1.0)).max(0.002);
    let allowed = ((1.0 - min_score) * template.len() as f64).floor() as usize;
    let mut matches: Vec<SymbolMatch> = Vec::new();

    for c in segments.chunks_exact(4) {
        if limits.stopped() || raw_cap.is_some_and(|cap| matches.len() >= cap) {
            truncated = true;
            break;
        }
        let (x1, y1, x2, y2) = (c[0], c[1], c[2], c[3]);
        let seg_l = (x2 - x1).hypot(y2 - y1);
        // The page line's length gives the copy's size.
        let mut k = 1.0;
        if options.scales {
            k = seg_l / l;
            if !(0.5..=2.0).contains(&k) {
                continue;
            }
        } else if (seg_l - l).abs() > tol * 2.0 {
            continue;
        }
        let ktol = tol * k.max(1.0);
        let s_angle = (y2 - y1).atan2(x2 - x1);
        let mx = (x1 + x2) / 2.0;
        let my = (y1 + y2) / 2.0;
        // The page line may be the anchor either way round: a turn of phi or phi + pi.
        for flip in [0.0, std::f64::consts::PI] {
            let mut phi = s_angle - a_angle + flip;
            phi = phi.sin().atan2(phi.cos());
            if !options.rotations && phi.abs() > angle_tol {
                continue;
            }
            if !options.rotations {
                phi = 0.0;
            }
            let (cs, sn) = (phi.cos(), phi.sin());
            let map = |x: f64, y: f64| (mx + k * (x * cs - y * sn), my + k * (x * sn + y * cs));
            let mut found = 1usize; // the anchor itself
            let mut missing = 0usize;
            for r in &rel {
                let (ax, ay) = map(r[0], r[1]);
                let (bx, by) = map(r[2], r[3]);
                if hash.has(ax, ay, bx, by, ktol) {
                    found += 1;
                } else {
                    missing += 1;
                    if missing > allowed {
                        break;
                    }
                }
            }
            if missing > allowed {
                continue;
            }
            let pts: Vec<(f64, f64)> = corners.iter().map(|&(x, y)| map(x, y)).collect();
            let (mut rx0, mut ry0, mut rx1, mut ry1) = (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            );
            for (x, y) in &pts {
                rx0 = rx0.min(*x);
                rx1 = rx1.max(*x);
                ry0 = ry0.min(*y);
                ry1 = ry1.max(*y);
            }
            let center = map(center_rel.0, center_rel.1);
            matches.push(SymbolMatch {
                center: [center.0, center.1],
                rect: SearchRect {
                    x: rx0,
                    y: ry0,
                    w: rx1 - rx0,
                    h: ry1 - ry0,
                },
                angle: phi.to_degrees(),
                score: found as f64 / template.len() as f64,
                scale: k,
            });
        }
    }

    // One match per place: symmetric symbols match several ways, and repeated lines several times.
    // Kept matches go in a grid so each candidate only meets its neighbours (not every kept match).
    matches.sort_by(|a, b| b.score.total_cmp(&a.score));
    let near = (tbw.min(tbh) / 2.0).max(tol * 4.0);
    // A kept match closer than near * min(scale) (at most 2 * near) suppresses a candidate.
    let cell = near * 2.0;
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut out: Vec<SymbolMatch> = Vec::new();
    for m in matches {
        let (gx, gy) = (
            (m.center[0] / cell).floor() as i64,
            (m.center[1] / cell).floor() as i64,
        );
        let mut dup = false;
        'near: for dx in -1..=1 {
            for dy in -1..=1 {
                for &k in grid.get(&(gx + dx, gy + dy)).into_iter().flatten() {
                    let o = &out[k];
                    if (o.center[0] - m.center[0]).hypot(o.center[1] - m.center[1])
                        < near * o.scale.min(m.scale)
                    {
                        dup = true;
                        break 'near;
                    }
                }
            }
        }
        if dup {
            continue;
        }
        if limits.max_matches.is_some_and(|cap| out.len() >= cap) {
            truncated = true;
            break;
        }
        grid.entry((gx, gy)).or_default().push(out.len());
        out.push(m);
    }
    out.sort_by(|a, b| {
        a.center[1]
            .total_cmp(&b.center[1])
            .then(a.center[0].total_cmp(&b.center[0]))
    });
    SearchOutcome {
        matches: out,
        truncated,
    }
}
