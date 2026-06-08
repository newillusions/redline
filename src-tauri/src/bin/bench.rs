//! Headless §20 performance benchmark harness for the redline render core.
//!
//! Drives `RenderEngine` directly (no Tauri / no webview) over the bench corpus
//! and measures the §20 metrics that are observable headlessly:
//!   - cold open → first tile
//!   - single-tile rasterize latency (p50 / p95 / max)
//!   - page-jump latency (open a fresh page, render its first tile)
//!   - peak RSS during active render
//!   - RSS after a churn loop across 100+ pages (leak / monotonic-growth check)
//!   - C2-vs-C3 peak RSS (the key sub-linear-with-file-size invariant)
//!
//! NOT measurable headlessly (needs the running GUI): interactive pan FPS,
//! zoom placeholder/settle timing, GPU compositing. Those are reported as
//! "GUI-required" in the verdict.
//!
//! Usage:
//!   PDFIUM_DYNAMIC_LIB_PATH=/abs/path/libpdfium.dylib \
//!     cargo run --release --bin bench -- /abs/path/bench/corpus [out.md]
//!
//! RSS is read via `ps -o rss= -p <pid>` (KB on macOS/Linux). Dependency-free
//! and accurate enough for a multi-hundred-MB budget check.

use std::path::{Path, PathBuf};
use std::time::Instant;

use redline_lib::render::{RenderEngine, TileRequest};

const TILE_CSS: u32 = 512;
const ZOOM: f32 = 1.0;
const DPR: f32 = 2.0; // Retina worst case — tiles at 2× pixels

/// Read current process RSS in megabytes via `ps`. Returns 0.0 on failure.
fn rss_mb() -> f64 {
    let pid = std::process::id();
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.trim().parse::<f64>().map(|kb| kb / 1024.0).unwrap_or(0.0)
        }
        Err(_) => 0.0,
    }
}

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let idx = ((sorted_ms.len() as f64 - 1.0) * p).round() as usize;
    sorted_ms[idx]
}

struct TierResult {
    tier: String,
    file: String,
    file_mb: f64,
    pages: u32,
    open_ms: f64,
    first_tile_ms: f64,
    tile_p50_ms: f64,
    tile_p95_ms: f64,
    tile_max_ms: f64,
    tile_count: usize,
    page_jump_p50_ms: f64,
    page_jump_max_ms: f64,
    rss_before_mb: f64,
    rss_peak_mb: f64,
    rss_after_churn_mb: f64,
    cache_mb: f64,
    churn_pages: u32,
    notes: String,
}

/// How many tiles cover a page at the given scale.
fn tile_grid(engine: &mut RenderEngine, doc_id: &str, page_index: u32, scale: f32) -> (u32, u32) {
    if let Ok(size) = engine.page_size(doc_id, page_index) {
        let tile_px = (TILE_CSS as f32 * scale).max(1.0);
        let cols = ((size.width_pts as f32 * scale) / tile_px).ceil() as u32;
        let rows = ((size.height_pts as f32 * scale) / tile_px).ceil() as u32;
        (cols.max(1), rows.max(1))
    } else {
        (1, 1)
    }
}

fn bench_tier(engine: &mut RenderEngine, tier: &str, path: &Path) -> Option<TierResult> {
    let file = path.file_name()?.to_string_lossy().to_string();
    let file_mb = std::fs::metadata(path).ok()?.len() as f64 / (1024.0 * 1024.0);
    let scale = ZOOM * DPR;

    eprintln!("\n=== {tier}: {file} ({file_mb:.0} MB) ===");

    let rss_before = rss_mb();
    let doc_id = format!("{tier}-doc");

    // --- Cold open ---
    let t = Instant::now();
    let pages = match engine.open_document(path.to_path_buf(), doc_id.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("  OPEN FAILED: {e:#}");
            return None;
        }
    };
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!("  opened: {pages} pages in {open_ms:.0} ms");

    // --- First tile (cold open → first paint) ---
    let first_req = TileRequest {
        doc_id: doc_id.clone(),
        page_index: 0,
        tile_size_css: TILE_CSS,
        tile_x: 0,
        tile_y: 0,
        zoom: ZOOM,
        dpr: DPR,
    };
    let t = Instant::now();
    let first_ok = engine.render_tile(&first_req).is_ok();
    let first_tile_ms = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!("  first tile: {first_tile_ms:.0} ms (ok={first_ok})");

    // --- Single-tile latency: render all tiles of the first few pages ---
    // Sample up to 6 pages spread across the doc to capture variety.
    let mut tile_ms: Vec<f64> = Vec::new();
    let sample_pages: Vec<u32> = {
        let n = pages.min(6);
        (0..n)
            .map(|i| (i * pages / n.max(1)).min(pages - 1))
            .collect()
    };
    for &pg in &sample_pages {
        let (cols, rows) = tile_grid(engine, &doc_id, pg, scale);
        // Cap tiles per page so a huge A0 page doesn't dominate.
        let max_tiles = 24u32;
        let mut count = 0;
        'page: for ty in 0..rows {
            for tx in 0..cols {
                if count >= max_tiles {
                    break 'page;
                }
                let req = TileRequest {
                    doc_id: doc_id.clone(),
                    page_index: pg,
                    tile_size_css: TILE_CSS,
                    tile_x: tx,
                    tile_y: ty,
                    zoom: ZOOM,
                    dpr: DPR,
                };
                let t = Instant::now();
                if engine.render_tile(&req).is_ok() {
                    tile_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                count += 1;
            }
        }
    }
    tile_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let tile_p50 = percentile(&tile_ms, 0.50);
    let tile_p95 = percentile(&tile_ms, 0.95);
    let tile_max = tile_ms.last().copied().unwrap_or(0.0);
    eprintln!(
        "  tile latency: p50={tile_p50:.1} p95={tile_p95:.1} max={tile_max:.1} ms (n={})",
        tile_ms.len()
    );

    // --- Page-jump latency: jump to scattered pages, render first tile each ---
    let mut jump_ms: Vec<f64> = Vec::new();
    let jump_targets: Vec<u32> = {
        let n = pages.min(10);
        (0..n)
            .map(|i| (i * pages / n.max(1)).min(pages - 1))
            .collect()
    };
    for &pg in &jump_targets {
        let req = TileRequest {
            doc_id: doc_id.clone(),
            page_index: pg,
            tile_size_css: TILE_CSS,
            tile_x: 0,
            tile_y: 0,
            zoom: ZOOM,
            dpr: DPR,
        };
        let t = Instant::now();
        let _ = engine.render_tile(&req);
        jump_ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    jump_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let page_jump_p50 = percentile(&jump_ms, 0.50);
    let page_jump_max = jump_ms.last().copied().unwrap_or(0.0);
    eprintln!("  page-jump: p50={page_jump_p50:.0} max={page_jump_max:.0} ms");

    // --- Churn loop: render 1 tile from each of up to 120 pages, then again ---
    // This is the leak / monotonic-growth check. We render across many pages so
    // the bounded tile cache churns; RSS must stabilise, not climb monotonically.
    let churn_pages = pages.min(120);
    let mut rss_peak = rss_before.max(rss_mb());
    let mut rss_samples: Vec<f64> = Vec::new();
    for pass in 0..2 {
        for pg in 0..churn_pages {
            let (cols, rows) = tile_grid(engine, &doc_id, pg, scale);
            // Render a diagonal sample of tiles to exercise different regions.
            let picks = [
                (0u32, 0u32),
                (cols / 2, rows / 2),
                (cols.saturating_sub(1), rows.saturating_sub(1)),
            ];
            for (tx, ty) in picks {
                let req = TileRequest {
                    doc_id: doc_id.clone(),
                    page_index: pg,
                    tile_size_css: TILE_CSS,
                    tile_x: tx.min(cols.saturating_sub(1)),
                    tile_y: ty.min(rows.saturating_sub(1)),
                    zoom: ZOOM,
                    dpr: DPR,
                };
                let _ = engine.render_tile(&req);
            }
            if pg % 10 == 0 {
                let r = rss_mb();
                rss_peak = rss_peak.max(r);
                if pass == 1 {
                    rss_samples.push(r);
                }
            }
        }
        eprintln!("  churn pass {pass} done, rss={:.0} MB", rss_mb());
    }
    let rss_after_churn = rss_mb();
    rss_peak = rss_peak.max(rss_after_churn);
    let cache_mb = engine.tile_cache_bytes() as f64 / (1024.0 * 1024.0);
    eprintln!(
        "  tile cache: {:.0} MB ({} tiles) of {:.0} MB steady RSS",
        cache_mb,
        engine.tile_cache_len(),
        rss_after_churn
    );

    let mut notes = String::new();
    if !first_ok {
        notes.push_str("first-tile render failed; ");
    }
    if !rss_samples.is_empty() {
        let first = rss_samples.first().copied().unwrap_or(0.0);
        let last = rss_samples.last().copied().unwrap_or(0.0);
        let growth = if first > 0.0 {
            (last - first) / first * 100.0
        } else {
            0.0
        };
        notes.push_str(&format!("2nd-pass RSS Δ {growth:+.1}%; "));
    }

    // Close to release page handles and tiles before next tier.
    engine.close_document(&doc_id);

    Some(TierResult {
        tier: tier.to_string(),
        file,
        file_mb,
        pages,
        open_ms,
        first_tile_ms,
        tile_p50_ms: tile_p50,
        tile_p95_ms: tile_p95,
        tile_max_ms: tile_max,
        tile_count: tile_ms.len(),
        page_jump_p50_ms: page_jump_p50,
        page_jump_max_ms: page_jump_max,
        rss_before_mb: rss_before,
        rss_peak_mb: rss_peak,
        rss_after_churn_mb: rss_after_churn,
        cache_mb,
        churn_pages,
        notes,
    })
}

/// Find the single PDF inside a tier directory (first match).
fn find_pdf(dir: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension()
            .map(|e| e.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false)
        {
            return Some(p);
        }
    }
    None
}

const TIERS: [(&str, &str); 5] = [
    ("C1", "c1-typical"),
    ("C2", "c2-large"),
    ("C3", "c3-stress"),
    ("C4", "c4-dense"),
    ("C5", "c5-scanned"),
];

/// Serialize a TierResult to one pipe-delimited line (for subprocess → parent IPC).
fn encode_result(r: &TierResult) -> String {
    format!(
        "RESULT|{}|{}|{:.3}|{}|{:.3}|{:.3}|{:.3}|{:.3}|{:.3}|{}|{:.3}|{:.3}|{:.3}|{:.3}|{:.3}|{:.3}|{}|{}",
        r.tier, r.file, r.file_mb, r.pages, r.open_ms, r.first_tile_ms,
        r.tile_p50_ms, r.tile_p95_ms, r.tile_max_ms, r.tile_count,
        r.page_jump_p50_ms, r.page_jump_max_ms,
        r.rss_before_mb, r.rss_peak_mb, r.rss_after_churn_mb, r.cache_mb, r.churn_pages,
        r.notes.replace('|', "/"),
    )
}

fn decode_result(line: &str) -> Option<TierResult> {
    let p: Vec<&str> = line.trim_start_matches("RESULT|").split('|').collect();
    if p.len() < 18 {
        return None;
    }
    Some(TierResult {
        tier: p[0].to_string(),
        file: p[1].to_string(),
        file_mb: p[2].parse().ok()?,
        pages: p[3].parse().ok()?,
        open_ms: p[4].parse().ok()?,
        first_tile_ms: p[5].parse().ok()?,
        tile_p50_ms: p[6].parse().ok()?,
        tile_p95_ms: p[7].parse().ok()?,
        tile_max_ms: p[8].parse().ok()?,
        tile_count: p[9].parse().ok()?,
        page_jump_p50_ms: p[10].parse().ok()?,
        page_jump_max_ms: p[11].parse().ok()?,
        rss_before_mb: p[12].parse().ok()?,
        rss_peak_mb: p[13].parse().ok()?,
        rss_after_churn_mb: p[14].parse().ok()?,
        cache_mb: p[15].parse().ok()?,
        churn_pages: p[16].parse().ok()?,
        notes: p[17].to_string(),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Single-tier subprocess mode: `bench --tier C2 <corpus>`. Runs one tier in a
    // FRESH process so RSS is isolated (no cross-tier allocator high-water carryover),
    // prints one RESULT| line on stdout, exits.
    if args.get(1).map(|s| s == "--tier").unwrap_or(false) {
        let tier_id = args.get(2).expect("tier id");
        let corpus_dir = args.get(3).expect("corpus dir");
        let subdir = TIERS
            .iter()
            .find(|(t, _)| t == tier_id)
            .map(|(_, d)| *d)
            .expect("unknown tier");
        let mut engine = RenderEngine::new().unwrap_or_else(|e| {
            eprintln!("FATAL: PDFium init: {e:#}");
            std::process::exit(1);
        });
        // Optional cache-budget override for tuning experiments: REDLINE_CACHE_MB=256
        if let Ok(mb) = std::env::var("REDLINE_CACHE_MB") {
            if let Ok(mb) = mb.parse::<usize>() {
                engine = engine.with_cache_budget(mb * 1024 * 1024);
                eprintln!("  (cache budget overridden to {mb} MB)");
            }
        }
        // Optional page-LRU-cap override: REDLINE_MAX_PAGES=12. This is the dominant
        // steady-RSS lever (held PDFium page state, not tiles) — see render::mod.rs.
        if let Ok(n) = std::env::var("REDLINE_MAX_PAGES") {
            if let Ok(n) = n.parse::<usize>() {
                engine = engine.with_max_loaded_pages(n);
                eprintln!("  (max loaded pages overridden to {n})");
            }
        }
        let dir = Path::new(corpus_dir).join(subdir);
        if let Some(pdf) = find_pdf(&dir) {
            if let Some(r) = bench_tier(&mut engine, tier_id, &pdf) {
                println!("{}", encode_result(&r));
            }
        } else {
            eprintln!("(no PDF in {})", dir.display());
        }
        return;
    }

    let corpus_dir = args.get(1).cloned().unwrap_or_else(|| {
        eprintln!("usage: bench <corpus-dir> [out.md]");
        std::process::exit(2);
    });
    let out_path = args.get(2).cloned();

    // Parent: spawn one subprocess per tier for clean per-tier RSS isolation.
    let self_exe = std::env::current_exe().expect("current exe");
    let mut results: Vec<TierResult> = Vec::new();
    for (tier, _subdir) in TIERS {
        eprintln!("\n>>> spawning isolated subprocess for tier {tier} …");
        let out = std::process::Command::new(&self_exe)
            .args(["--tier", tier, &corpus_dir])
            .output();
        match out {
            Ok(o) => {
                // Relay child stderr (progress logs) to our stderr.
                eprint!("{}", String::from_utf8_lossy(&o.stderr));
                let stdout = String::from_utf8_lossy(&o.stdout);
                if let Some(line) = stdout.lines().find(|l| l.starts_with("RESULT|")) {
                    if let Some(r) = decode_result(line) {
                        results.push(r);
                    } else {
                        eprintln!("(tier {tier}: could not parse RESULT line)");
                    }
                } else {
                    eprintln!("(tier {tier}: no RESULT — skipped or failed)");
                }
            }
            Err(e) => eprintln!("(tier {tier}: subprocess failed: {e})"),
        }
    }

    let md = render_report(&results);
    println!("{md}");
    if let Some(op) = out_path {
        if let Err(e) = std::fs::write(&op, &md) {
            eprintln!("could not write {op}: {e}");
        } else {
            eprintln!("\nReport written to {op}");
        }
    }
}

fn render_report(results: &[TierResult]) -> String {
    let mut s = String::new();
    let now = chrono::Utc::now().to_rfc3339();
    s.push_str("# Redline §20 Headless Benchmark — Results\n\n");
    s.push_str(&format!("Run: {now}\n\n"));
    s.push_str("Machine: Apple Silicon dev Mac (NOT the §20 floor machine). ");
    s.push_str("Tiles rendered at zoom×dpr = ");
    s.push_str(&format!(
        "{:.0}× ({}px CSS × {:.0} DPR).\n\n",
        ZOOM * DPR,
        TILE_CSS,
        DPR
    ));
    s.push_str("Render strategy: M1.5 true tile-region matrix render (bitmap allocated at tile size, never full page). ");
    s.push_str("Page-handle cache (C4 dense-sheet fix). Auto-normalise on open for >2 GiB files PDFium can't load (C5).\n\n");
    s.push_str("**Each tier runs in its own subprocess** so RSS is isolated (no cross-tier allocator high-water carryover). ");
    s.push_str("`RSS peak` includes any one-time ingest spike (notably C5's lopdf normalise of a 2.1 GB file); ");
    s.push_str("`RSS post-churn` is the steady-state after rendering tiles across 100+ pages twice — the number that matters for sustained use. ");
    s.push_str("`Cache MB` is the byte-budgeted tile cache occupancy at steady state (budget default 512 MiB).\n\n");

    s.push_str("| Tier | File MB | Pages | Open ms | 1st tile ms | Tile p50/p95/max ms | Jump p50/max ms | RSS peak MB | RSS post-churn MB | Cache MB | Churn pgs | Notes |\n");
    s.push_str("|------|--------:|------:|--------:|-----------:|--------------------:|----------------:|-----------:|------------------:|---------:|----------:|-------|\n");
    for r in results {
        s.push_str(&format!(
            "| {} | {:.0} | {} | {:.0} | {:.0} | {:.1}/{:.1}/{:.1} | {:.0}/{:.0} | {:.0} | {:.0} | {:.0} | {} | {} |\n",
            r.tier, r.file_mb, r.pages, r.open_ms, r.first_tile_ms,
            r.tile_p50_ms, r.tile_p95_ms, r.tile_max_ms,
            r.page_jump_p50_ms, r.page_jump_max_ms,
            r.rss_peak_mb, r.rss_after_churn_mb, r.cache_mb, r.churn_pages, r.notes,
        ));
    }
    s.push('\n');

    // Files list
    s.push_str("## Corpus files\n\n");
    for r in results {
        s.push_str(&format!(
            "- **{}**: `{}` — {:.0} MB, {} pages\n",
            r.tier, r.file, r.file_mb, r.pages
        ));
    }
    s.push('\n');

    // Measurement detail (RSS baseline + tile sample sizes)
    s.push_str("## Measurement detail\n\n");
    for r in results {
        s.push_str(&format!(
            "- **{}**: RSS before open {:.0} MB → peak {:.0} MB (Δ {:+.0} MB); tile latency sample n={}\n",
            r.tier, r.rss_before_mb, r.rss_peak_mb, r.rss_peak_mb - r.rss_before_mb, r.tile_count,
        ));
    }
    s.push('\n');

    s
}
