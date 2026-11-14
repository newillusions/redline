// Adapted from redcolumn (Apache-2.0), packages/measure/src/rasterSearch.ts, modified:
// ported from TypeScript to Rust, RGB input helper added, and the coarse pass runs in
// parallel across page rows with std scoped threads.
//! Symbol Search on scanned sheets: the boxed symbol's pixels matched across a page by
//! normalised cross-correlation (bright paper and dark ink alike, whatever the scan's
//! contrast), coarse first on shrunken images, then refined at full size around each
//! promising spot. Used when the boxed area has too little vector line work to match.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// One byte per pixel, 0 black to 255 white, row-major.
#[derive(Debug, Clone)]
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RasterMatch {
    /// Pixel position of the match's top-left corner, in the page image.
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    /// Quarter turns of the template that matched (0-3).
    pub turns: u8,
    /// Correlation, -1 to 1.
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct RasterSearchOptions {
    /// Correlation a match needs.
    pub threshold: f64,
    /// Also look for the symbol turned by 90, 180 and 270 degrees.
    pub rotations: bool,
    /// Stop at this time and return what was found (checked per row and per candidate).
    pub deadline: Option<Instant>,
    /// Stop as soon as this is set.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for RasterSearchOptions {
    fn default() -> Self {
        RasterSearchOptions {
            threshold: 0.75,
            rotations: false,
            deadline: None,
            cancel: None,
        }
    }
}

impl RasterSearchOptions {
    fn stopped(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
            || self.deadline.is_some_and(|d| Instant::now() >= d)
    }
}

/// Luminance from tightly packed RGB8 pixels.
pub fn gray_from_rgb(rgb: &[u8], width: usize, height: usize) -> GrayImage {
    let mut data = Vec::with_capacity(width * height);
    for p in rgb.chunks_exact(3).take(width * height) {
        data.push(((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8);
    }
    GrayImage {
        width,
        height,
        data,
    }
}

/// Part of an image; pixels outside it read as white paper.
pub fn crop(img: &GrayImage, x: usize, y: usize, w: usize, h: usize) -> GrayImage {
    let mut data = vec![255u8; w * h];
    for r in 0..h {
        for c in 0..w {
            if x + c < img.width && y + r < img.height {
                data[r * w + c] = img.data[(y + r) * img.width + x + c];
            }
        }
    }
    GrayImage {
        width: w,
        height: h,
        data,
    }
}

/// Shrinks by a whole factor, averaging each block.
pub fn shrink(img: &GrayImage, f: usize) -> GrayImage {
    if f <= 1 {
        return img.clone();
    }
    let w = (img.width / f).max(1);
    let h = (img.height / f).max(1);
    let mut data = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0u32;
            for dy in 0..f {
                for dx in 0..f {
                    let sy = (y * f + dy).min(img.height - 1);
                    let sx = (x * f + dx).min(img.width - 1);
                    sum += img.data[sy * img.width + sx] as u32;
                }
            }
            data[y * w + x] = (sum / (f * f) as u32) as u8;
        }
    }
    GrayImage {
        width: w,
        height: h,
        data,
    }
}

/// 3 x 3 box blur (edges clamp). Thin anti-aliased lines shift by a fraction of a pixel from one
/// copy to the next (the copies sit at different sub-pixel offsets on the page grid), which on
/// its own drops the correlation of identical symbols well below the threshold; a light blur
/// makes the correlation tolerant of that without blurring the symbol's shape away.
pub fn blur3(img: &GrayImage) -> GrayImage {
    let (w, h) = (img.width, img.height);
    if w < 3 || h < 3 {
        return img.clone();
    }
    let mut tmp = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let l = img.data[y * w + x.saturating_sub(1)] as u16;
            let c = img.data[y * w + x] as u16;
            let r = img.data[y * w + (x + 1).min(w - 1)] as u16;
            tmp[y * w + x] = l + c + r;
        }
    }
    let mut data = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let u = tmp[y.saturating_sub(1) * w + x] as u32;
            let c = tmp[y * w + x] as u32;
            let d = tmp[(y + 1).min(h - 1) * w + x] as u32;
            data[y * w + x] = ((u + c + d) / 9) as u8;
        }
    }
    GrayImage {
        width: w,
        height: h,
        data,
    }
}

/// The image turned a quarter turn clockwise.
pub fn turn(img: &GrayImage) -> GrayImage {
    let (w, h) = (img.width, img.height);
    let mut data = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            data[x * h + (h - 1 - y)] = img.data[y * w + x];
        }
    }
    GrayImage {
        width: h,
        height: w,
        data,
    }
}

/// Most coarse-pass candidates refined at full size. A flat or noisy template correlates
/// "somewhat" almost everywhere; without a cap the refinement would run per candidate.
const MAX_CANDIDATES: usize = 20_000;
/// Smallest grey-level standard deviation a template needs to count as having a symbol in it
/// (a thin dark line drawing on white paper is about 50; blank paper and scanner dust are < 10).
pub const MIN_TEMPLATE_STDDEV: f64 = 12.0;

/// Grey-level standard deviation of an image.
pub fn stddev(t: &GrayImage) -> f64 {
    if t.data.is_empty() {
        return 0.0;
    }
    let st = stats(t);
    st.norm / (t.data.len() as f64).sqrt()
}

struct Stats {
    mean: f64,
    norm: f64,
}

fn stats(t: &GrayImage) -> Stats {
    let sum: f64 = t.data.iter().map(|&v| v as f64).sum();
    let mean = sum / t.data.len() as f64;
    let norm: f64 = t.data.iter().map(|&v| (v as f64 - mean).powi(2)).sum();
    Stats {
        mean,
        norm: norm.sqrt(),
    }
}

/// Correlation of the template with the page at (x, y); None over flat page areas.
fn ncc(page: &GrayImage, t: &GrayImage, st: &Stats, x: usize, y: usize) -> Option<f64> {
    let (mut sum, mut sum2, mut cross) = (0.0f64, 0.0f64, 0.0f64);
    let n = (t.width * t.height) as f64;
    for r in 0..t.height {
        let row = &page.data[(y + r) * page.width + x..(y + r) * page.width + x + t.width];
        let trow = &t.data[r * t.width..(r + 1) * t.width];
        for (p, tv) in row.iter().zip(trow) {
            let p = *p as f64;
            sum += p;
            sum2 += p * p;
            cross += p * (*tv as f64 - st.mean);
        }
    }
    let variance = sum2 - sum * sum / n;
    if variance < n * 4.0 {
        return None; // blank paper
    }
    Some(cross / (variance.sqrt() * st.norm))
}

/// Every place the template appears on the page (the original included), best first per spot.
/// Results are ordered top to bottom then left to right.
pub fn raster_search(
    page: &GrayImage,
    template: &GrayImage,
    options: &RasterSearchOptions,
) -> Vec<RasterMatch> {
    raster_search_limited(page, template, options).0
}

/// [`raster_search`] that also reports whether it stopped early (deadline or cancel), so the
/// list may be incomplete.
pub fn raster_search_limited(
    page: &GrayImage,
    template: &GrayImage,
    options: &RasterSearchOptions,
) -> (Vec<RasterMatch>, bool) {
    let threshold = options.threshold;
    let mut truncated = false;
    if stddev(template) < MIN_TEMPLATE_STDDEV {
        return (Vec::new(), false);
    }
    let blurred_page = blur3(page);
    let page = &blurred_page;
    let mut templates = vec![blur3(template)];
    if options.rotations {
        for k in 1..4 {
            let next = turn(&templates[k - 1]);
            templates.push(next);
        }
    }
    // Shrink so the template is about 12 pixels across for the coarse pass.
    let f = (template.width.min(template.height) / 12).max(1);
    let small = shrink(page, f);
    let mut found: Vec<RasterMatch> = Vec::new();
    for (turns, t) in templates.iter().enumerate() {
        let st = stats(t);
        if st.norm < 1.0 || t.width > page.width || t.height > page.height {
            continue;
        }
        let ts = shrink(t, f);
        let sts = stats(&ts);
        if sts.norm < 1.0 || ts.width > small.width || ts.height > small.height {
            continue;
        }
        let rows = small.height - ts.height + 1;
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(rows)
            .max(1);
        let chunk = rows.div_ceil(workers);
        let mut coarse: Vec<(f64, usize, usize)> = Vec::new();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|w| {
                    let (small, ts, sts) = (&small, &ts, &sts);
                    scope.spawn(move || {
                        let mut local = Vec::new();
                        for y in (w * chunk)..((w + 1) * chunk).min(rows) {
                            if options.stopped() {
                                break;
                            }
                            for x in 0..=(small.width - ts.width) {
                                if let Some(s) = ncc(small, ts, sts, x, y) {
                                    if s >= threshold - 0.15 {
                                        local.push((s, x, y));
                                    }
                                }
                            }
                        }
                        local
                    })
                })
                .collect();
            for h in handles {
                coarse.extend(h.join().expect("raster coarse-pass worker panicked"));
            }
        });
        // Best candidates first; refine each at full size within one coarse pixel.
        coarse.sort_by(|a, b| b.0.total_cmp(&a.0));
        coarse.truncate(MAX_CANDIDATES);
        for (_, cx, cy) in coarse {
            if options.stopped() {
                truncated = true;
                break;
            }
            let mut best: Option<RasterMatch> = None;
            let y0 = (cy * f).saturating_sub(f);
            let y1 = (cy * f + f).min(page.height - t.height);
            let x0 = (cx * f).saturating_sub(f);
            let x1 = (cx * f + f).min(page.width - t.width);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if let Some(s) = ncc(page, t, &st, x, y) {
                        if s >= threshold
                            && best.as_ref().map_or(true, |b: &RasterMatch| s > b.score)
                        {
                            best = Some(RasterMatch {
                                x,
                                y,
                                w: t.width,
                                h: t.height,
                                turns: turns as u8,
                                score: s,
                            });
                        }
                    }
                }
            }
            if let Some(b) = best {
                found.push(b);
            }
        }
    }
    // One match per place.
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out: Vec<RasterMatch> = Vec::new();
    for m in found {
        let dup = out.iter().any(|o| {
            ((o.x + o.w / 2) as f64 - (m.x + m.w / 2) as f64).abs() < o.w.min(m.w) as f64 / 2.0
                && ((o.y + o.h / 2) as f64 - (m.y + m.h / 2) as f64).abs()
                    < o.h.min(m.h) as f64 / 2.0
        });
        if !dup {
            out.push(m);
        }
    }
    out.sort_by(|a, b| a.y.cmp(&b.y).then(a.x.cmp(&b.x)));
    (out, truncated || options.stopped())
}
