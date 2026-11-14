// Adapted in part from redcolumn (Apache-2.0), packages/measure/src/visualSearch.ts and
// rasterSearch.ts (see vector.rs and raster.rs), modified. This file is Redline's own glue.
//! Symbol Search (Bluebeam "VisualSearch" equivalent): the user boxes one luminaire symbol on
//! a drawing and Redline finds every matching instance on a page or across a sheet set.
//!
//! - [`vector`]: matches line work. Used when the box holds vector paths (CAD exports).
//! - [`raster`]: normalised cross-correlation on a rendered page. Used when the box holds
//!   (almost) no vector line work, i.e. a scanned sheet.
//!
//! # Threading
//! Only the segment extraction (and, for raster, the page render) touches PDFium, and that
//! runs on the render thread via `RenderHandle`. All matching runs on tokio blocking
//! threads, so a large sheet set never stalls tile rendering or the UI.

pub mod raster;
pub mod vector;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::geometry::Subpath;
use crate::render::RenderHandle;

pub use raster::{GrayImage, RasterSearchOptions};
pub use vector::{
    template_segments, visual_search, visual_search_limited, SearchLimits, SearchOutcome,
    SearchRect, Seg, SymbolMatch, VisualSearchOptions,
};

/// Fewer template segments than this and the box is treated as image content.
pub const MIN_VECTOR_SEGMENTS: usize = 4;
/// Default raster resolution, and the cap on a page's long side in pixels.
const RASTER_DPI: f32 = 150.0;
const RASTER_MAX_SIDE_PX: f64 = 10_000.0;
const RASTER_MAX_PIXELS: f64 = 100_000_000.0;
/// Most pixels in the template crop.
const RASTER_MAX_TEMPLATE_PIXELS: usize = 4_000_000;
/// Most pages one request may cover.
pub const MAX_PAGES: usize = 5_000;

/// Whether rasterising a page of this size at `dpi` would exceed the pixel or side-length cap.
pub fn raster_over_cap(width_pts: f64, height_pts: f64, dpi: f32) -> bool {
    let scale = dpi as f64 / 72.0;
    let (w, h) = (width_pts * scale, height_pts * scale);
    !(w.is_finite() && h.is_finite())
        || w * h > RASTER_MAX_PIXELS
        || w.max(h) > RASTER_MAX_SIDE_PX * 1.5
}

/// Flatten sub-paths into `[x1, y1, x2, y2, ...]` (closing edges included).
pub fn subpaths_to_segments(subpaths: &[Subpath]) -> Vec<f64> {
    // Corrupt content can carry NaN or absurd coordinates; drop those segments (the snap index
    // does the same) so they cannot poison the match hash.
    let ok = |v: f64| v.is_finite() && v.abs() < 1.0e7;
    let mut out = Vec::new();
    let mut push = |a: &crate::geometry::PdfPoint, b: &crate::geometry::PdfPoint| {
        if ok(a.x) && ok(a.y) && ok(b.x) && ok(b.y) {
            out.extend_from_slice(&[a.x, a.y, b.x, b.y]);
        }
    };
    for sub in subpaths {
        let pts = &sub.points;
        for w in pts.windows(2) {
            push(&w[0], &w[1]);
        }
        if sub.closed && pts.len() > 1 && pts[0] != pts[pts.len() - 1] {
            push(&pts[pts.len() - 1], &pts[0]);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Vector when the box holds enough line work, raster otherwise.
    #[default]
    Auto,
    Vector,
    Raster,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SymbolSearchRequest {
    pub doc_id: String,
    /// The page the symbol was boxed on.
    pub template_page: u32,
    /// The box, in page space (same space as markup geometry and snap targets).
    pub rect: SearchRect,
    /// Pages to search. `None` is the template page only.
    #[serde(default)]
    pub pages: Option<Vec<u32>>,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub rotations: bool,
    #[serde(default)]
    pub scales: bool,
    /// Vector endpoint tolerance in points.
    #[serde(default)]
    pub tolerance: Option<f64>,
    /// Share of the template's lines a vector match needs (0-1).
    #[serde(default)]
    pub min_score: Option<f64>,
    /// Correlation a raster match needs (0-1).
    #[serde(default)]
    pub raster_threshold: Option<f64>,
    #[serde(default)]
    pub raster_dpi: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageHits {
    pub page: u32,
    pub matches: Vec<SymbolMatch>,
    /// The page's match stopped early (match cap, time limit or cancel): the list may be incomplete.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedPage {
    pub page: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolSearchResponse {
    /// `"vector"` or `"raster"`: what was actually used.
    pub method: &'static str,
    pub template_segments: usize,
    pub pages: Vec<PageHits>,
    pub skipped: Vec<SkippedPage>,
    /// Some page was cut short or not searched because a result, time or size limit was reached.
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// Raster page-pixel match to the common match shape, in page space (y up).
pub fn raster_to_symbol_match(m: &raster::RasterMatch, scale: f64, page_h_pts: f64) -> SymbolMatch {
    let x = m.x as f64 / scale;
    let w = m.w as f64 / scale;
    let h = m.h as f64 / scale;
    let y = page_h_pts - (m.y as f64 + m.h as f64) / scale;
    SymbolMatch {
        center: [x + w / 2.0, y + h / 2.0],
        rect: SearchRect { x, y, w, h },
        angle: m.turns as f64 * 90.0,
        score: m.score,
        scale: 1.0,
    }
}

/// Template image cut from a rendered template page.
pub fn raster_template(
    page: &GrayImage,
    rect: &SearchRect,
    scale: f64,
    page_h_pts: f64,
) -> GrayImage {
    let x0 = (rect.x * scale).floor().max(0.0) as usize;
    let y0 = ((page_h_pts - (rect.y + rect.h)) * scale).floor().max(0.0) as usize;
    let w = ((rect.w * scale).ceil() as usize).max(1);
    let h = ((rect.h * scale).ceil() as usize).max(1);
    raster::crop(page, x0, y0, w, h)
}

fn vector_options(req: &SymbolSearchRequest) -> VisualSearchOptions {
    let d = VisualSearchOptions::default();
    VisualSearchOptions {
        rotations: req.rotations,
        scales: req.scales,
        tolerance: req.tolerance.unwrap_or(d.tolerance).clamp(0.05, 10.0),
        min_score: req.min_score.unwrap_or(d.min_score).clamp(0.3, 1.0),
    }
}

type MatchJob = tokio::task::JoinHandle<SearchOutcome>;

/// Work bounds for one search. A hostile or merely dense set (hundreds of A0 sheets, a hatch-filled
/// sheet where every hatch line is a genuine copy) must cost bounded memory, CPU and IPC payload.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Pages whose segments wait for or undergo matching at once.
    pub max_in_flight: usize,
    pub max_matches_per_page: usize,
    pub max_matches_total: usize,
    /// Wall time one page's match may run before it returns what it has.
    pub page_timeout: Duration,
    /// A page with more line segments than this is skipped (reason shown).
    pub max_segments_per_page: usize,
    /// Observes concurrency; tests only.
    pub probe: Option<Arc<Probe>>,
}

impl Default for Limits {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
        Limits {
            max_in_flight: (cpus / 2).clamp(1, 8),
            max_matches_per_page: 2_000,
            max_matches_total: 20_000,
            page_timeout: Duration::from_secs(20),
            max_segments_per_page: 5_000_000,
            probe: None,
        }
    }
}

/// Counts concurrent matching tasks (and can slow each, so a test can see the bound hold).
#[derive(Debug, Default)]
pub struct Probe {
    pub active: AtomicUsize,
    pub peak: AtomicUsize,
    pub calls: AtomicUsize,
    pub delay_ms: u64,
}

impl Probe {
    fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let n = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(n, Ordering::SeqCst);
        if self.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.delay_ms));
        }
        let out = f();
        self.active.fetch_sub(1, Ordering::SeqCst);
        out
    }
}

/// Sets the flag when dropped: if the command future is dropped (window closed, request
/// superseded) the blocking match tasks see it and stop instead of running to completion.
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Gathers per-page results under the total cap.
struct Collector {
    out: Vec<PageHits>,
    skipped: Vec<SkippedPage>,
    total: usize,
    max_total: usize,
    truncated: bool,
    cancel: Arc<AtomicBool>,
}

impl Collector {
    fn new(max_total: usize, cancel: Arc<AtomicBool>) -> Self {
        Collector {
            out: Vec::new(),
            skipped: Vec::new(),
            total: 0,
            max_total,
            truncated: false,
            cancel,
        }
    }

    fn full(&self) -> bool {
        self.total >= self.max_total
    }

    fn skip(&mut self, page: u32, reason: impl Into<String>) {
        self.skipped.push(SkippedPage {
            page,
            reason: reason.into(),
        });
    }

    /// Collect one page's match task. A task that panicked or was cancelled skips that page
    /// (with the reason) instead of failing the whole search and discarding other pages' hits.
    async fn settle(&mut self, page: u32, job: MatchJob) {
        match job.await {
            Ok(mut o) => {
                let room = self.max_total - self.total;
                if o.matches.len() > room {
                    o.matches.truncate(room);
                    o.truncated = true;
                }
                self.total += o.matches.len();
                self.truncated |= o.truncated;
                if self.full() {
                    // Enough: stop the pages still matching.
                    self.cancel.store(true, Ordering::SeqCst);
                }
                self.out.push(PageHits {
                    page,
                    matches: o.matches,
                    truncated: o.truncated,
                });
            }
            Err(e) => self.skip(page, format!("matching failed: {e}")),
        }
    }

    /// A page not searched because the total cap was already reached.
    fn skip_for_cap(&mut self, page: u32) {
        self.truncated = true;
        self.skip(page, "result limit reached");
    }

    fn finish(
        self,
        method: &'static str,
        template_segments: usize,
        started: Instant,
    ) -> SymbolSearchResponse {
        SymbolSearchResponse {
            method,
            template_segments,
            pages: self.out,
            skipped: self.skipped,
            truncated: self.truncated,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    }
}

/// Run a Symbol Search with the default [`Limits`].
pub async fn search(
    render: &RenderHandle,
    req: SymbolSearchRequest,
) -> Result<SymbolSearchResponse> {
    search_with_limits(render, req, Limits::default()).await
}

/// Run a Symbol Search. Extraction and rendering go through the render thread; matching runs on
/// blocking threads, at most `limits.max_in_flight` pages at a time.
pub async fn search_with_limits(
    render: &RenderHandle,
    req: SymbolSearchRequest,
    limits: Limits,
) -> Result<SymbolSearchResponse> {
    let started = Instant::now();
    let r = &req.rect;
    let usable = [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) && r.w > 0.0 && r.h > 0.0;
    if !usable {
        bail!("the symbol box is empty");
    }
    let mut pages = req.pages.clone().unwrap_or_else(|| vec![req.template_page]);
    pages.sort_unstable();
    pages.dedup();
    if pages.is_empty() {
        bail!("no pages to search");
    }
    if pages.len() > MAX_PAGES {
        bail!("too many pages ({}; most is {MAX_PAGES})", pages.len());
    }

    // The box must lie on the template page (display space: origin at the page's lower-left).
    let size = render
        .page_size(req.doc_id.clone(), req.template_page)
        .await
        .context("reading the template page size")?;
    if r.x >= size.width_pts || r.y >= size.height_pts || r.x + r.w <= 0.0 || r.y + r.h <= 0.0 {
        bail!("the symbol box is outside the page");
    }
    if r.w * r.h > size.width_pts * size.height_pts * 4.0 {
        bail!("the symbol box is far larger than the page");
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(Arc::clone(&cancel));
    let mut col = Collector::new(limits.max_matches_total, Arc::clone(&cancel));

    let template_segs = render
        .page_segments(req.doc_id.clone(), req.template_page)
        .await
        .context("reading the template page's line work")?;
    let template = template_segments(&template_segs, &req.rect, 0.5);
    let use_vector = match req.mode {
        SearchMode::Vector => true,
        SearchMode::Raster => false,
        SearchMode::Auto => template.len() >= MIN_VECTOR_SEGMENTS,
    };
    let template_len = template.len();
    if use_vector && template.len() < 2 {
        bail!(
            "the box holds too little line work to search for; draw it tighter around one symbol"
        );
    }

    if use_vector {
        let opts = vector_options(&req);
        let template = Arc::new(template);
        let template_segs = Arc::new(template_segs);
        let mut jobs: VecDeque<(u32, MatchJob)> = VecDeque::new();
        for &p in &pages {
            // Bound the pages held in memory and matching at once: extraction on the render
            // thread is serial and fast, matching is slower, so without a bound a long set would
            // queue every page's segments (and match hash) before the first match finished.
            while jobs.len() >= limits.max_in_flight.max(1) {
                if let Some((page, job)) = jobs.pop_front() {
                    col.settle(page, job).await;
                }
            }
            if col.full() {
                col.skip_for_cap(p);
                continue;
            }
            let segs = if p == req.template_page {
                Arc::clone(&template_segs)
            } else {
                match render.page_segments(req.doc_id.clone(), p).await {
                    Ok(s) => Arc::new(s),
                    Err(e) => {
                        col.skip(p, format!("{e:#}"));
                        continue;
                    }
                }
            };
            if segs.len() / 4 > limits.max_segments_per_page {
                col.skip(
                    p,
                    format!("too many line segments ({}) to search", segs.len() / 4),
                );
                continue;
            }
            let t = Arc::clone(&template);
            let (cancel, probe) = (Arc::clone(&cancel), limits.probe.clone());
            let (cap, timeout) = (limits.max_matches_per_page, limits.page_timeout);
            // Matching starts now and overlaps the next page's extraction on the render thread.
            jobs.push_back((
                p,
                tokio::task::spawn_blocking(move || {
                    let lim = SearchLimits {
                        max_matches: Some(cap),
                        deadline: Some(Instant::now() + timeout),
                        cancel: Some(cancel),
                    };
                    let run = || visual_search_limited(&segs, &t, &opts, &lim);
                    match probe {
                        Some(pr) => pr.run(run),
                        None => run(),
                    }
                }),
            ));
        }
        while let Some((page, job)) = jobs.pop_front() {
            col.settle(page, job).await;
        }
        return Ok(col.finish("vector", template_len, started));
    }

    // Raster fallback: one dpi for every page so the template's pixel size carries over.
    let long_side_pts = size.width_pts.max(size.height_pts);
    let dpi = req
        .raster_dpi
        .unwrap_or(RASTER_DPI)
        .clamp(36.0, 600.0)
        .min((RASTER_MAX_SIDE_PX / long_side_pts * 72.0) as f32);
    if raster_over_cap(size.width_pts, size.height_pts, dpi) {
        bail!("the template page is too large to rasterise for an image search");
    }
    let tpage = render
        .render_page_full(req.doc_id.clone(), req.template_page, dpi)
        .await
        .context("rendering the template page")?;
    let tgray = raster::gray_from_rgb(
        &tpage.rgb,
        tpage.width_px as usize,
        tpage.height_px as usize,
    );
    drop(tpage.rgb);
    let timg = Arc::new(raster_template(
        &tgray,
        &req.rect,
        tpage.scale as f64,
        tpage.page_height_pts,
    ));
    drop(tgray);
    if raster::stddev(&timg) < raster::MIN_TEMPLATE_STDDEV {
        bail!("the box holds no symbol (blank paper); draw it around one symbol");
    }
    if timg.width < 8 || timg.height < 8 {
        bail!("the box is too small to search for at {dpi:.0} dpi; draw a larger box or raise the resolution");
    }
    if timg.width * timg.height > RASTER_MAX_TEMPLATE_PIXELS {
        bail!("the box is too large for an image search; draw it around one symbol");
    }
    let (threshold, rotations) = (
        req.raster_threshold.unwrap_or(0.75).clamp(0.3, 0.99),
        req.rotations,
    );
    // Pages are rendered one at a time (a full-page raster is large); the correlation of the
    // previous page overlaps the next render.
    let mut pending: Option<(u32, MatchJob)> = None;
    for &p in &pages {
        if col.full() {
            col.skip_for_cap(p);
            continue;
        }
        // Each page is sized on its own: a set can mix a small first sheet with a huge later one.
        let psize = match render.page_size(req.doc_id.clone(), p).await {
            Ok(sz) => sz,
            Err(e) => {
                col.skip(p, format!("{e:#}"));
                continue;
            }
        };
        if raster_over_cap(psize.width_pts, psize.height_pts, dpi) {
            col.skip(p, "page too large to rasterise at this resolution");
            continue;
        }
        let raster = match render.render_page_full(req.doc_id.clone(), p, dpi).await {
            Ok(r) => r,
            Err(e) => {
                col.skip(p, format!("{e:#}"));
                continue;
            }
        };
        let t = Arc::clone(&timg);
        let (cancel, probe) = (Arc::clone(&cancel), limits.probe.clone());
        let (cap, timeout) = (limits.max_matches_per_page, limits.page_timeout);
        let job = tokio::task::spawn_blocking(move || {
            let run = || {
                let gray = raster::gray_from_rgb(
                    &raster.rgb,
                    raster.width_px as usize,
                    raster.height_px as usize,
                );
                drop(raster.rgb);
                let opts = RasterSearchOptions {
                    threshold,
                    rotations,
                    deadline: Some(Instant::now() + timeout),
                    cancel: Some(cancel),
                };
                let (found, mut truncated) = raster::raster_search_limited(&gray, &t, &opts);
                let mut matches: Vec<SymbolMatch> = found
                    .iter()
                    .map(|m| raster_to_symbol_match(m, raster.scale as f64, raster.page_height_pts))
                    .collect();
                if matches.len() > cap {
                    matches.truncate(cap);
                    truncated = true;
                }
                SearchOutcome { matches, truncated }
            };
            match probe {
                Some(pr) => pr.run(run),
                None => run(),
            }
        });
        if let Some((pp, prev)) = pending.replace((p, job)) {
            col.settle(pp, prev).await;
        }
    }
    if let Some((pp, prev)) = pending {
        col.settle(pp, prev).await;
    }
    Ok(col.finish("raster", template_len, started))
}

#[cfg(test)]
mod tests;
