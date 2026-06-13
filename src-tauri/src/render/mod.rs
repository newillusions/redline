//! Render module — PDFium tiled rasterization (spec §4, §5).
//!
//! Design principles (spec §5):
//! - Render happens in Rust, never in the webview.
//! - Tiles are rasterized at exactly zoom × DPR resolution — NEVER upscaled.
//! - On zoom change, tiles are re-rendered (no upscale of stale tiles).
//! - A bounded LRU tile cache keeps memory stable under 100+ sheet churn.
//! - Memory does NOT scale linearly with file size (streaming, not full load).
//! - Raster tiles are display-only; geometry/snapping never reads them (§5 invariant).
//!
//! # PDFium binary
//!
//! pdfium-render needs a prebuilt PDFium shared library. Two paths:
//! - Set `PDFIUM_DYNAMIC_LIB_PATH` env var pointing at `libpdfium.so` / `.dylib` / `.dll`.
//! - Or use `PdfiumLibraryBindings::try_from_pdfium_source` auto-download (requires network
//!   at first build). The `pdfium-auto` crate can also be used as a build-time downloader.
//!
//! For M1 development: download the macOS arm64 PDFium binary from
//! <https://github.com/bblanchon/pdfium-binaries/releases> and place it at a path set in
//! `PDFIUM_DYNAMIC_LIB_PATH`, OR ship it inside `src-tauri/resources/` and point to it
//! at runtime. The bench/ README documents the exact steps.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use log::{debug, info, warn};
use memmap2::Mmap;
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Files at or above this size use the memory-mapped `FPDF_LoadMemDocument64`
/// (64-bit-clean) load path instead of the streaming file-access path, which has
/// a signed 32-bit (2 GiB) internal offset limit in PDFium. Set a little below
/// 2 GiB for headroom.
const MMAP_LOAD_THRESHOLD: u64 = 1_900_000_000;

/// Default tile-cache byte budget: 512 MiB.
///
/// Reasoning against the §20 ≤ 2.0 GB binding budget on the 16 GB floor machine:
/// the non-cache steady working set (PDFium engine + one open document + its
/// page-handle cache, measured isolated on the corpus) sits around 0.7–1.0 GB.
/// Tiles are PNG-encoded then base64'd (~50–250 KB each typically, not the 4 MB a
/// raw 1024² RGBA tile would be), so the *count* cap was a poor proxy. Capping the
/// cache at 512 MiB of tiles keeps the worst-case total (≈ 1.0 GB engine + 0.5 GB
/// tiles ≈ 1.5 GB) clearly under 2 GB with ~0.5 GB margin for fragmentation and the
/// webview. Tunable via `with_cache_budget`; the floor-machine run can dial it down
/// further if the engine working set is larger there.
const DEFAULT_TILE_CACHE_BUDGET_BYTES: usize = 512 * 1024 * 1024;

/// Default max number of PDFium pages kept loaded per document (LRU cap).
///
/// A loaded `PdfPage` holds PDFium's parsed page state, which the bench showed to be
/// the dominant steady-RSS contributor (not the tile cache). Capping the number of
/// simultaneously-loaded pages bounds RSS independent of document length. 24 covers a
/// generous on-screen working set (a viewport rarely spans >24 sheets at once) while
/// keeping held page state small; revisiting an evicted page re-parses it (ms for
/// normal pages, the known ~1s one-time cost for a dense A0). Tunable via
/// `with_max_loaded_pages` for the floor-machine run.
const DEFAULT_MAX_LOADED_PAGES: usize = 24;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A rendered tile returned to the frontend: raw RGBA bytes (or PNG-encoded).
/// The webview draws this onto an `<img>` / canvas element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedTile {
    /// Which document (opaque handle from `open_document`).
    pub doc_id: String,
    /// Zero-based page index.
    pub page_index: u32,
    /// Tile position in tile-grid coordinates.
    pub tile_x: u32,
    pub tile_y: u32,
    /// Pixel dimensions of this tile (may be smaller at page edges).
    pub width_px: u32,
    pub height_px: u32,
    /// Zoom level (device-independent; 1.0 = 100%).
    pub zoom: f32,
    /// Device pixel ratio (1.0 on non-HiDPI, 2.0 on Retina).
    pub dpr: f32,
    /// PNG-encoded RGBA pixel data, base64-encoded for IPC transport.
    pub png_base64: String,
    /// Rasterization latency for this tile (ms) — for bench harness.
    pub render_ms: u64,
}

/// Page size in PDF user-space points.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSize {
    pub doc_id: String,
    pub page_index: u32,
    /// Width in PDF points (1 pt = 1/72 inch).
    pub width_pts: f64,
    /// Height in PDF points.
    pub height_pts: f64,
}

/// Request for a single tile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TileRequest {
    pub doc_id: String,
    pub page_index: u32,
    /// Tile size in CSS pixels (before DPR). Typical: 512.
    pub tile_size_css: u32,
    pub tile_x: u32,
    pub tile_y: u32,
    pub zoom: f32,
    pub dpr: f32,
}

// ---------------------------------------------------------------------------
// Tile cache key
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TileCacheKey {
    doc_id: String,
    page_index: u32,
    tile_x: u32,
    tile_y: u32,
    /// Effective render scale × 1000 (avoids f32 in hash key).
    scale_millis: u32,
}

impl TileCacheKey {
    fn new(req: &TileRequest) -> Self {
        let scale = req.zoom * req.dpr;
        Self {
            doc_id: req.doc_id.clone(),
            page_index: req.page_index,
            tile_x: req.tile_x,
            tile_y: req.tile_y,
            scale_millis: (scale * 1000.0) as u32,
        }
    }
}

// ---------------------------------------------------------------------------
// Open document handle
// ---------------------------------------------------------------------------

/// An open PDF document with its PDFium handle.
/// Kept alive for the document's session; dropped on `close_document`.
///
/// # Page-handle cache (the C4 dense-sheet fix) + LRU cap (the C2 memory fix)
/// Loading a page in PDFium parses its full content stream. For a dense A0 sheet
/// (thousands of vector objects) this costs ~1s. The naive per-tile path called
/// `pages().get()` on every tile, paying that 1s repeatedly — measured 1.1s/tile.
/// Caching the loaded `PdfPage` amortises page-load to once-per-page; per-tile
/// matrix render then drops to ~10ms (measured).
///
/// BUT a loaded `PdfPage` retains PDFium's parsed page state, which is the dominant
/// contributor to steady RSS (bench: tiles were only ~100 MB of a ~1.5 GB C2 RSS;
/// the rest was held page state). So the cache is an **LRU bounded to `max_pages`**:
/// only the most-recently-used pages stay loaded; the LRU page is dropped
/// (`FPDF_ClosePage` frees its state) when the cap is exceeded. Revisiting an evicted
/// page re-parses it — cheap for normal pages (ms), the known ~1s one-time cost for a
/// dense A0. This keeps steady RSS bounded regardless of document page count.
///
/// # Drop order (SAFETY-critical)
/// PDFium requires every `PdfPage` to be dropped BEFORE the owning `PdfDocument`,
/// and (for mmap-loaded docs) the `PdfDocument` borrows the mmap so the mmap must
/// drop LAST. Struct fields drop in declaration order, so the order MUST be:
/// `page_cache` → `document` → `_backing`. Do not reorder these fields.
struct OpenDoc {
    /// Cache of loaded page handles, keyed by page index. Lifetime-transmuted to
    /// `'static`; logically borrows from `document` and is dropped before it.
    page_cache: HashMap<u32, PdfPage<'static>>,
    /// Per-page last-access tick for LRU eviction (monotonic counter).
    page_access: HashMap<u32, u64>,
    /// Monotonic access counter; bumped on every `page()` call.
    access_tick: u64,
    /// Max number of pages kept loaded at once (LRU cap). Bounds steady RSS.
    max_pages: usize,
    /// The PDFium document handle. MUST drop after `page_cache`, before `_backing`.
    document: PdfDocument<'static>,
    /// Backing store the document borrows from. `None` for the streaming
    /// (file-access) load path; `Some(mmap)` for the >2 GiB memory-mapped path
    /// (FPDF_LoadMemDocument64). MUST be the last field so it outlives `document`.
    _backing: Option<Mmap>,
    #[allow(dead_code)] // retained for future: path display, hot-reload on file change
    path: PathBuf,
    page_count: u32,
}

impl OpenDoc {
    /// Build an OpenDoc with empty page caches and the given LRU cap.
    fn new(
        document: PdfDocument<'static>,
        backing: Option<Mmap>,
        path: PathBuf,
        page_count: u32,
        max_pages: usize,
    ) -> Self {
        OpenDoc {
            page_cache: HashMap::new(),
            page_access: HashMap::new(),
            access_tick: 0,
            max_pages,
            document,
            _backing: backing,
            path,
            page_count,
        }
    }

    /// Get a loaded page, loading + caching it on first access. Maintains an LRU cap
    /// (`max_pages`): when exceeded, the least-recently-used page is dropped so
    /// PDFium frees its parsed state (the C2 steady-RSS fix).
    ///
    /// SAFETY: the returned `PdfPage<'static>` borrows from `self.document`. The
    /// `'static` is a lie tied to the document's actual lifetime; callers must not
    /// hold the reference past `close_document`. Within `render_tile` the page is
    /// used and dropped synchronously, so this is sound.
    fn page(&mut self, page_index: u32) -> Result<&PdfPage<'static>> {
        self.access_tick += 1;
        let tick = self.access_tick;

        if !self.page_cache.contains_key(&page_index) {
            let page = self
                .document
                .pages()
                .get(page_index as u16)
                .with_context(|| format!("Failed to load page {page_index}"))?;
            // SAFETY: page borrows from self.document (both effectively 'static for
            // the document's lifetime). page_cache drops before document (field order).
            let page: PdfPage<'static> = unsafe { std::mem::transmute(page) };
            self.page_cache.insert(page_index, page);
            self.page_access.insert(page_index, tick);
            self.evict_lru_pages();
        } else {
            self.page_access.insert(page_index, tick);
        }
        Ok(self.page_cache.get(&page_index).unwrap())
    }

    /// Drop least-recently-used pages until at most `max_pages` remain loaded.
    fn evict_lru_pages(&mut self) {
        while self.page_cache.len() > self.max_pages.max(1) {
            // Find the page with the smallest access tick.
            let lru = self
                .page_access
                .iter()
                .min_by_key(|(_, &t)| t)
                .map(|(&idx, _)| idx);
            match lru {
                Some(idx) => {
                    // Dropping the PdfPage calls FPDF_ClosePage, freeing parsed state.
                    self.page_cache.remove(&idx);
                    self.page_access.remove(&idx);
                }
                None => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RenderEngine
// ---------------------------------------------------------------------------

/// Central render state: PDFium bindings handle + open documents + tile cache.
///
/// # Drop order (SAFETY-critical)
/// `Pdfium` owns the dynamically-loaded PDFium library; dropping it UNLOADS the
/// dylib. Every `OpenDoc` (and its cached `PdfPage`s) calls back into the library
/// on drop (`FPDF_ClosePage`/`FPDF_CloseDocument`). So `documents` MUST drop
/// BEFORE `pdfium`. Struct fields drop in declaration order, therefore `documents`
/// is declared before `pdfium`. Do NOT reorder. (Reordering caused a SIGSEGV at
/// teardown — the dylib unloaded while open pages still referenced it.)
pub struct RenderEngine {
    /// Open documents keyed by opaque ID (UUID string).
    /// MUST drop before `pdfium` (see struct doc).
    documents: HashMap<String, OpenDoc>,
    /// Byte-budgeted tile cache. Key → (base64 PNG, last-access Instant).
    /// Eviction is by TOTAL BYTES, not tile count (a tile-count cap is meaningless
    /// when tiles range from a few KB to ~MB depending on content). Oldest tiles
    /// are evicted until the cache is back under `cache_budget_bytes`.
    tile_cache: HashMap<TileCacheKey, (String, Instant)>,
    /// Running total of cached tile bytes (sum of base64 string lengths).
    /// Maintained incrementally on insert/evict so eviction is O(evicted), not O(n).
    tile_cache_bytes: usize,
    /// Tile-cache byte budget. See `DEFAULT_TILE_CACHE_BUDGET_BYTES` for the
    /// reasoning behind the default; override via `with_cache_budget` for the
    /// floor-machine run or tighter targets.
    cache_budget_bytes: usize,
    /// Per-document LRU cap on simultaneously-loaded PDFium pages. The dominant
    /// steady-RSS lever (loaded pages hold PDFium's parsed state). Applied to each
    /// `OpenDoc` at open time. See `DEFAULT_MAX_LOADED_PAGES`.
    max_loaded_pages: usize,
    /// PDFium bindings — owns the dylib; MUST be the LAST field so it drops AFTER
    /// `documents` (whose pages/docs call into the library on drop).
    pdfium: Pdfium,
}

impl RenderEngine {
    /// Create the render engine and load PDFium bindings.
    ///
    /// PDFium is loaded dynamically from:
    ///   1. `PDFIUM_DYNAMIC_LIB_PATH` environment variable (recommended for dev).
    ///   2. `PDFIUM_DYNAMIC_LIB_PATH` set via `tauri::Builder` env injection (release bundle).
    ///
    /// Returns an error if PDFium cannot be loaded — this is a hard startup failure.
    pub fn new() -> Result<Self> {
        let bindings = if let Ok(p) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
            Pdfium::bind_to_library(p)
                .context("PDFium dynamic library not found at PDFIUM_DYNAMIC_LIB_PATH. See bench/README.md for setup steps.")?
        } else {
            Pdfium::bind_to_system_library()
                .context("PDFium dynamic library not found. Set PDFIUM_DYNAMIC_LIB_PATH or place libpdfium alongside the binary. See bench/README.md for setup steps.")?
        };
        let pdfium = Pdfium::new(bindings);

        info!("PDFium loaded successfully");

        Ok(Self {
            pdfium,
            documents: HashMap::new(),
            tile_cache: HashMap::new(),
            tile_cache_bytes: 0,
            cache_budget_bytes: DEFAULT_TILE_CACHE_BUDGET_BYTES,
            max_loaded_pages: DEFAULT_MAX_LOADED_PAGES,
        })
    }

    /// Override the tile-cache byte budget (e.g. for the floor-machine §20 run or a
    /// tighter memory target). Takes effect for subsequent inserts; if the new budget
    /// is below the current cache size, excess tiles are evicted immediately.
    pub fn with_cache_budget(mut self, budget_bytes: usize) -> Self {
        self.cache_budget_bytes = budget_bytes;
        self.evict_until_under_budget();
        self
    }

    /// Override the per-document loaded-page LRU cap (the dominant steady-RSS lever).
    /// Applies to documents opened AFTER this call.
    pub fn with_max_loaded_pages(mut self, max_pages: usize) -> Self {
        self.max_loaded_pages = max_pages.max(1);
        self
    }

    /// Total loaded PDFium pages across all open documents (diagnostic).
    pub fn loaded_pages(&self) -> usize {
        self.documents.values().map(|d| d.page_cache.len()).sum()
    }

    /// Current total bytes held in the tile cache (sum of base64 tile lengths).
    pub fn tile_cache_bytes(&self) -> usize {
        self.tile_cache_bytes
    }

    /// Number of tiles currently cached.
    pub fn tile_cache_len(&self) -> usize {
        self.tile_cache.len()
    }

    /// Open a PDF file and register it under a new doc_id.
    ///
    /// Three-stage strategy for robustness across the §20 corpus:
    ///   1. **< ~1.9 GiB:** streaming `FPDF_LoadCustomDocument` (file-access reader).
    ///      Lowest memory — PDFium pulls bytes on demand via a callback.
    ///   2. **>= ~1.9 GiB:** memory-mapped `FPDF_LoadMemDocument64` (64-bit clean),
    ///      no full read into RSS (pages fault in lazily, OS-managed).
    ///   3. **Fallback for pathological large files:** if a page fails to load after
    ///      (1)/(2) — the signature of PDFium's internal 2 GiB object-offset limit,
    ///      observed on the 2.1 GB C5 scanned set where `open` + page-count succeed
    ///      but every page load returns `PdfiumLibraryInternalError(Unknown)` — we
    ///      transparently NORMALISE the file with `lopdf` (load → compress object
    ///      streams → re-serialise) into a working copy in the OS temp dir, which
    ///      packs offsets back under 2 GiB, and reload from that. The user's original
    ///      file is never modified. (C5: 2.28 GB → 54.8 MB normalised copy; renders
    ///      at 22 ms/tile. The one-time normalise step peaks ~4.6 GB RSS — an ingest
    ///      cost, not steady-state — acceptable like Acrobat/Bluebeam "reduce size".)
    pub fn open_document(&mut self, path: PathBuf, doc_id: String) -> Result<u32> {
        info!("Opening document: {:?} as {}", path, doc_id);

        let file_len = std::fs::metadata(&path)
            .with_context(|| format!("Failed to stat PDF: {:?}", path))?
            .len();

        // Stage 1/2: size-based load.
        let (doc, backing) = Self::load_doc(&self.pdfium, &path, file_len)?;
        let page_count = doc.pages().len() as u32;

        // Probe page 0. If it loads, the doc is healthy — register and return.
        let probe_ok = doc.pages().get(0).is_ok();

        if probe_ok || file_len < MMAP_LOAD_THRESHOLD {
            // Healthy, or a small file where normalise wouldn't help (a genuinely
            // broken small file should surface its real error on first render).
            self.documents.insert(
                doc_id.clone(),
                OpenDoc::new(doc, backing, path, page_count, self.max_loaded_pages),
            );
            info!("Opened document {} — {} pages", doc_id, page_count);
            return Ok(page_count);
        }

        // Stage 3: large file whose pages won't load (PDFium >2 GiB offset limit).
        // Drop the failed doc first (releases the mmap), then normalise + reload.
        warn!(
            "Large PDF page-load failed (PDFium 2 GiB offset limit); normalising {:?} via lopdf",
            path
        );
        drop(doc);
        drop(backing);

        let normalized = Self::normalize_large_pdf(&path)
            .with_context(|| format!("Failed to normalise oversized PDF: {:?}", path))?;
        let norm_len = std::fs::metadata(&normalized).map(|m| m.len()).unwrap_or(0);
        info!(
            "Normalised {:?} → {:?} ({:.1} MB)",
            path,
            normalized,
            norm_len as f64 / (1u64 << 20) as f64
        );

        let (doc2, backing2) = Self::load_doc(&self.pdfium, &normalized, norm_len)?;
        let page_count2 = doc2.pages().len() as u32;
        doc2.pages()
            .get(0)
            .context("Normalised PDF still fails page load — file may be corrupt")?;

        self.documents.insert(
            doc_id.clone(),
            // keep the ORIGINAL path for display/identity
            OpenDoc::new(doc2, backing2, path, page_count2, self.max_loaded_pages),
        );
        info!(
            "Opened (normalised) document {} — {} pages",
            doc_id, page_count2
        );
        Ok(page_count2)
    }

    /// Load a PDF, choosing streaming vs mmap by size. Returns the `'static`-cast
    /// document and an optional backing mmap that must outlive it.
    fn load_doc(
        pdfium: &Pdfium,
        path: &std::path::Path,
        file_len: u64,
    ) -> Result<(PdfDocument<'static>, Option<Mmap>)> {
        if file_len >= MMAP_LOAD_THRESHOLD {
            info!(
                "Large PDF ({:.2} GiB ≥ {:.2} GiB): mmap + FPDF_LoadMemDocument64",
                file_len as f64 / (1u64 << 30) as f64,
                MMAP_LOAD_THRESHOLD as f64 / (1u64 << 30) as f64,
            );
            let file = std::fs::File::open(path)
                .with_context(|| format!("Failed to open PDF for mmap: {:?}", path))?;
            // SAFETY: file is read-only; the Mmap is owned by OpenDoc for the doc's life.
            let mmap = unsafe { Mmap::map(&file) }
                .with_context(|| format!("Failed to mmap PDF: {:?}", path))?;
            // SAFETY: returned doc borrows `mmap`, which OpenDoc holds in `_backing`
            // (declared after `document`), so the mmap outlives the document.
            let doc = pdfium
                .load_pdf_from_byte_slice(
                    unsafe { std::mem::transmute::<&[u8], &'static [u8]>(&mmap[..]) },
                    None,
                )
                .with_context(|| format!("Failed to load large PDF via mmap: {:?}", path))?;
            let doc: PdfDocument<'static> = unsafe { std::mem::transmute(doc) };
            Ok((doc, Some(mmap)))
        } else {
            let doc = pdfium
                .load_pdf_from_file(path, None)
                .with_context(|| format!("Failed to open PDF: {:?}", path))?;
            // SAFETY: doc borrows the Pdfium bindings (live as long as the engine);
            // streaming reader is owned internally. 'static tied to OpenDoc's life.
            let doc: PdfDocument<'static> = unsafe { std::mem::transmute(doc) };
            Ok((doc, None))
        }
    }

    /// Normalise an oversized PDF with lopdf (prune unreferenced objects + compress
    /// object streams + re-serialise) into a working copy in the OS temp dir, packing
    /// object offsets back under PDFium's 2 GiB limit. Returns the path to the
    /// normalised copy. The original is never modified.
    ///
    /// # One-time cost / known caveat
    /// lopdf is an in-memory model: `Document::load` parses the WHOLE file into a
    /// `BTreeMap` of objects, so the transient peak RSS during normalise is large
    /// (~4.6 GB observed on the 2.28 GB C5 set) and the wall time (~8 s) exceeds the
    /// §20 open ≤5 s target. This is an INGEST cost (paid once per oversized file,
    /// comparable to Acrobat/Bluebeam "reduce file size"), not steady-state. A true
    /// streaming/incremental rewrite would need a different toolchain (lopdf has no
    /// streaming-write API — `save_to` serialises the full in-memory doc); deferred.
    /// `prune_objects()` here trims unreferenced objects to shrink the serialise side.
    fn normalize_large_pdf(path: &std::path::Path) -> Result<PathBuf> {
        let mut doc = lopdf::Document::load(path)
            .with_context(|| format!("lopdf failed to load {:?}", path))?;
        // Drop unreferenced/orphan objects first (the source of much of the bloat),
        // then compress content/object streams before re-serialising.
        let pruned = doc.prune_objects();
        if !pruned.is_empty() {
            info!("normalize: pruned {} unreferenced objects", pruned.len());
        }
        doc.compress();

        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "doc".into());
        let mut out = std::env::temp_dir();
        out.push(format!(
            "redline-normalized-{}-{}.pdf",
            stem,
            std::process::id()
        ));
        doc.save(&out)
            .with_context(|| format!("lopdf failed to save normalised copy to {:?}", out))?;
        Ok(out)
    }

    /// Close a document and evict its tiles from cache.
    pub fn close_document(&mut self, doc_id: &str) {
        if let Some(mut doc) = self.documents.remove(doc_id) {
            // Explicitly drop cached pages before the document drops. Field order
            // already guarantees this, but we make it explicit for the SAFETY
            // contract (PDFium requires pages dropped before their document).
            doc.page_cache.clear();
            drop(doc);
            // Evict this doc's tiles and decrement the byte total accordingly.
            let mut freed = 0usize;
            self.tile_cache.retain(|k, v| {
                if k.doc_id == doc_id {
                    freed += v.0.len();
                    false
                } else {
                    true
                }
            });
            self.tile_cache_bytes = self.tile_cache_bytes.saturating_sub(freed);
            info!("Closed document {}", doc_id);
        } else {
            warn!("close_document: unknown doc_id {}", doc_id);
        }
    }

    /// Return the number of pages for an open document.
    pub fn page_count(&self, doc_id: &str) -> Option<u32> {
        self.documents.get(doc_id).map(|d| d.page_count)
    }

    /// Return the size of a page in PDF user-space points.
    /// Uses the page-handle cache (loads + caches the page on first access).
    pub fn page_size(&mut self, doc_id: &str, page_index: u32) -> Result<PageSize> {
        let doc = self
            .documents
            .get_mut(doc_id)
            .with_context(|| format!("Unknown doc_id: {}", doc_id))?;

        let page = doc.page(page_index)?;
        let size = page.page_size();
        Ok(PageSize {
            doc_id: doc_id.to_string(),
            page_index,
            width_pts: size.width().value as f64,
            height_pts: size.height().value as f64,
        })
    }

    /// Rasterize a single tile.
    ///
    /// Tiles are rendered at exactly (tile_size_css × zoom × dpr) pixels — never upscaled.
    /// Result is PNG-encoded and base64'd for IPC transport.
    ///
    /// Cache hit: returns immediately without touching PDFium.
    /// Cache miss: rasterizes, stores in cache (with FIFO eviction if at cap).
    pub fn render_tile(&mut self, req: &TileRequest) -> Result<RenderedTile> {
        let key = TileCacheKey::new(req);

        // Cache hit — clone the bytes and release the immutable tile_cache borrow
        // BEFORE calling page_size (which now needs &mut self for the page cache).
        let cached_png: Option<String> = self.tile_cache.get(&key).map(|(b, _)| b.clone());
        if let Some(png_b64) = cached_png {
            debug!(
                "Cache hit: doc={} page={} tile=({},{}) zoom={} dpr={}",
                req.doc_id, req.page_index, req.tile_x, req.tile_y, req.zoom, req.dpr
            );
            // We need width/height — recompute from page size for the response.
            // For cache hits this is fast (page handle is already cached too).
            let page_size = self.page_size(&req.doc_id, req.page_index)?;
            let (tile_w, tile_h) = compute_tile_pixel_dims(req, &page_size);
            return Ok(RenderedTile {
                doc_id: req.doc_id.clone(),
                page_index: req.page_index,
                tile_x: req.tile_x,
                tile_y: req.tile_y,
                width_px: tile_w,
                height_px: tile_h,
                zoom: req.zoom,
                dpr: req.dpr,
                png_base64: png_b64,
                render_ms: 0,
            });
        }

        let t0 = Instant::now();

        let scale = req.zoom * req.dpr;
        let tile_px = (req.tile_size_css as f32 * scale) as u32;

        // The whole PDFium-borrowing render runs inside this block so the mutable
        // borrow of `self.documents` (via `doc`/`page`) ends before we touch
        // `self.tile_cache` below. Returns owned tile bytes + dimensions.
        let (png_b64, tile_w, tile_h) = {
            let doc = self
                .documents
                .get_mut(&req.doc_id)
                .with_context(|| format!("Unknown doc_id: {}", req.doc_id))?;

            // Cached page handle — loads + caches on first access. This is the C4
            // dense-sheet fix: page-load (≈1s for a dense A0) is paid once per page,
            // not once per tile.
            let page = doc.page(req.page_index)?;

            let page_size = page.page_size();
            let page_w_pts = page_size.width().value;
            let page_h_pts = page_size.height().value;

            // Full-page pixel dimensions at this scale.
            let full_w_px = (page_w_pts * scale) as u32;
            let full_h_px = (page_h_pts * scale) as u32;

            // Tile origin in pixel space (top-left).
            let tile_origin_x = req.tile_x * tile_px;
            let tile_origin_y = req.tile_y * tile_px;

            // Clamp tile to page bounds (edge tiles are smaller).
            let tile_w = (tile_px).min(full_w_px.saturating_sub(tile_origin_x));
            let tile_h = (tile_px).min(full_h_px.saturating_sub(tile_origin_y));

            if tile_w == 0 || tile_h == 0 {
                anyhow::bail!(
                    "Tile ({},{}) at zoom={} dpr={} is outside page bounds ({}×{} px)",
                    req.tile_x,
                    req.tile_y,
                    req.zoom,
                    req.dpr,
                    full_w_px,
                    full_h_px
                );
            }

            // M1.5: TRUE tile-region render via transformation matrix.
            //
            // We allocate a bitmap of exactly tile_w × tile_h pixels (NEVER the full page)
            // and use a custom PDF→device matrix so only the tile's slice of the page is
            // rasterised into it. This keeps peak RSS bounded by tile size, independent of
            // page dimensions — the §20 memory invariant. (Replaces the M1 full-page-render
            // + crop_imm strategy, which allocated the entire page bitmap per tile.)
            //
            // How it composes with pdfium-render's PdfRenderConfig (verified against 0.8.37
            // render_config.rs apply_to_page):
            //   - set_fixed_size(tile_w, tile_h) ⇒ use_auto_scaling = false ⇒ the output
            //     bitmap is allocated at exactly (tile_w, tile_h), and width/height scale
            //     factors default to 1.0 (scale_width_factor/scale_height_factor = None).
            //   - set_matrix(M) sets transformation_matrix; with form data disabled the final
            //     device matrix = transformation_matrix.scale(1.0, 1.0) = M exactly.
            //   - clip_rect defaults to (0,0,tile_w,tile_h) = the whole output bitmap.
            //   - render is dispatched via FPDF_RenderPageBitmapWithMatrix.
            //
            // The matrix maps PDF user space (origin bottom-left, +y up) to device pixels
            // (origin top-left, +y down) for THIS tile:
            //   x_dev = s·x_pdf            − tile_origin_x      (a = s,  e = −tile_origin_x)
            //   y_dev = −s·y_pdf + full_h_px − tile_origin_y    (d = −s, f = full_h_px − tile_origin_y)
            // i.e. the full page is placed at device scale s, top-left at (0,0), then shifted
            // up-left by the tile's pixel origin so the tile's slice lands in the bitmap.
            let matrix = PdfMatrix::new(
                scale,                                   // a: x scale
                0.0,                                     // b
                0.0,                                     // c
                -scale,                                  // d: y scale (flip)
                -(tile_origin_x as f32),                 // e: x translate
                full_h_px as f32 - tile_origin_y as f32, // f: y translate (flip origin)
            );

            let render_config = PdfRenderConfig::new()
                .set_fixed_size(tile_w as i32, tile_h as i32)
                .render_form_data(false) // required: matrix path only applies when form data is off
                .apply_matrix(matrix) // config starts at IDENTITY, so apply == set here
                .context("failed to set tile transformation matrix")?;

            let bitmap = page
                .render_with_config(&render_config)
                .context("PDFium render_with_config (tile matrix) failed")?;

            let img = bitmap.as_image();
            let mut png_bytes: Vec<u8> = Vec::new();
            img.write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .context("PNG encode failed")?;

            let png_b64 = base64_encode(&png_bytes);
            (png_b64, tile_w, tile_h)
        }; // end PDFium-borrowing block — `doc`/`page` borrow of self ends here

        let render_ms = t0.elapsed().as_millis() as u64;

        debug!(
            "Rendered tile doc={} page={} ({},{}) {}×{}px zoom={} dpr={} in {}ms",
            req.doc_id,
            req.page_index,
            req.tile_x,
            req.tile_y,
            tile_w,
            tile_h,
            req.zoom,
            req.dpr,
            render_ms
        );

        // Insert into the byte-budgeted cache, then evict oldest tiles until the
        // total is back under budget. (Insert first so a single tile larger than the
        // budget is still served once; the next insert evicts it.)
        let new_bytes = png_b64.len();
        if let Some((old, _)) = self
            .tile_cache
            .insert(key, (png_b64.clone(), Instant::now()))
        {
            // Replaced an existing tile at the same key: adjust by the delta.
            self.tile_cache_bytes = self.tile_cache_bytes.saturating_sub(old.len());
        }
        self.tile_cache_bytes += new_bytes;
        self.evict_until_under_budget();

        Ok(RenderedTile {
            doc_id: req.doc_id.clone(),
            page_index: req.page_index,
            tile_x: req.tile_x,
            tile_y: req.tile_y,
            width_px: tile_w,
            height_px: tile_h,
            zoom: req.zoom,
            dpr: req.dpr,
            png_base64: png_b64,
            render_ms,
        })
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Evict the oldest tiles (by last-access Instant) until the cache total is at
    /// or under `cache_budget_bytes`. O(evicted × n) worst case; fine because the
    /// steady state evicts at most a handful per insert.
    fn evict_until_under_budget(&mut self) {
        while self.tile_cache_bytes > self.cache_budget_bytes && !self.tile_cache.is_empty() {
            if let Some(oldest_key) = self
                .tile_cache
                .iter()
                .min_by_key(|(_, (_, ts))| *ts)
                .map(|(k, _)| k.clone())
            {
                if let Some((bytes, _)) = self.tile_cache.remove(&oldest_key) {
                    self.tile_cache_bytes = self.tile_cache_bytes.saturating_sub(bytes.len());
                }
            } else {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Render thread + channel API
// ---------------------------------------------------------------------------
//
// `Pdfium` (and thus `RenderEngine`) is !Send + !Sync because the underlying
// PDFium C library uses thread-local state. The correct pattern for Tauri
// (which requires `AppState: Send + Sync`) is to pin `RenderEngine` to a
// single dedicated OS thread and communicate with it via channels.
//
// `RenderHandle` is Send + Sync — it holds only `mpsc::Sender` — and is what
// lives in `AppState`. Commands are dispatched as enum variants; results come
// back through per-call `oneshot` channels so callers can await them cleanly.

/// Commands sent to the render thread.
pub enum RenderCmd {
    OpenDocument {
        path: PathBuf,
        doc_id: String,
        reply: oneshot::Sender<Result<u32>>,
    },
    CloseDocument {
        doc_id: String,
        reply: oneshot::Sender<()>,
    },
    PageCount {
        doc_id: String,
        reply: oneshot::Sender<Option<u32>>,
    },
    PageSize {
        doc_id: String,
        page_index: u32,
        reply: oneshot::Sender<Result<PageSize>>,
    },
    RenderTile {
        req: TileRequest,
        reply: oneshot::Sender<Result<RenderedTile>>,
    },
}

/// A `Send + Sync` handle to the render thread.
/// Holds a `std::sync::mpsc::SyncSender` (bounded, blocking) wrapped in an
/// `Arc` so it can be cloned into `AppState`.
///
/// We use `std::sync::mpsc` (not tokio) for the sender because the render
/// thread runs a plain blocking loop, not an async runtime.
#[derive(Clone)]
pub struct RenderHandle {
    tx: std::sync::Arc<std::sync::mpsc::SyncSender<RenderCmd>>,
}

// SAFETY: SyncSender is Send + Sync when the message type is Send.
// RenderCmd is Send because all fields in it are Send (PathBuf, String,
// primitive types, and oneshot::Sender<T> where T: Send).
unsafe impl Send for RenderHandle {}
unsafe impl Sync for RenderHandle {}

impl RenderHandle {
    /// Spawn the render thread and return a handle to it.
    ///
    /// Returns `Err` if PDFium cannot be loaded (hard startup failure).
    pub fn spawn() -> Result<Self> {
        // Buffer up to 64 pending commands before the sender blocks.
        let (tx, rx) = std::sync::mpsc::sync_channel::<RenderCmd>(64);

        // Spawn a dedicated OS thread. RenderEngine lives here forever.
        std::thread::Builder::new()
            .name("redline-render".to_string())
            .spawn(move || {
                // Initialise engine on the render thread.
                let mut engine = match RenderEngine::new() {
                    Ok(e) => e,
                    Err(err) => {
                        log::error!("Render thread: PDFium init failed: {:#}", err);
                        // Drain the channel, sending errors back so callers don't hang.
                        while let Ok(cmd) = rx.recv() {
                            send_init_error(cmd, &err);
                        }
                        return;
                    }
                };

                log::info!("Render thread started");

                // Command loop — runs until the sender is dropped (app shutdown).
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        RenderCmd::OpenDocument {
                            path,
                            doc_id,
                            reply,
                        } => {
                            let _ = reply.send(engine.open_document(path, doc_id));
                        }
                        RenderCmd::CloseDocument { doc_id, reply } => {
                            engine.close_document(&doc_id);
                            let _ = reply.send(());
                        }
                        RenderCmd::PageCount { doc_id, reply } => {
                            let _ = reply.send(engine.page_count(&doc_id));
                        }
                        RenderCmd::PageSize {
                            doc_id,
                            page_index,
                            reply,
                        } => {
                            let _ = reply.send(engine.page_size(&doc_id, page_index));
                        }
                        RenderCmd::RenderTile { req, reply } => {
                            let _ = reply.send(engine.render_tile(&req));
                        }
                    }
                }

                log::info!("Render thread exiting");
            })
            .expect("failed to spawn render thread");

        Ok(Self {
            tx: std::sync::Arc::new(tx),
        })
    }

    // -----------------------------------------------------------------------
    // Async helpers — each sends a command and awaits the oneshot reply.
    // These are called from Tauri async commands.
    // -----------------------------------------------------------------------

    pub async fn open_document(&self, path: PathBuf, doc_id: String) -> Result<u32> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RenderCmd::OpenDocument {
                path,
                doc_id,
                reply: reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("render thread gone"))?;
        reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("render thread dropped reply"))?
    }

    pub async fn close_document(&self, doc_id: String) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RenderCmd::CloseDocument {
                doc_id,
                reply: reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("render thread gone"))?;
        reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("render thread dropped reply"))
    }

    pub async fn page_count(&self, doc_id: String) -> Result<Option<u32>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RenderCmd::PageCount {
                doc_id,
                reply: reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("render thread gone"))?;
        reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("render thread dropped reply"))
    }

    pub async fn page_size(&self, doc_id: String, page_index: u32) -> Result<PageSize> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RenderCmd::PageSize {
                doc_id,
                page_index,
                reply: reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("render thread gone"))?;
        reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("render thread dropped reply"))?
    }

    pub async fn render_tile(&self, req: TileRequest) -> Result<RenderedTile> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RenderCmd::RenderTile {
                req,
                reply: reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("render thread gone"))?;
        reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("render thread dropped reply"))?
    }
}

/// Send an init-error reply to any command that arrived before PDFium loaded.
fn send_init_error(cmd: RenderCmd, err: &anyhow::Error) {
    let msg = format!("PDFium init failed: {:#}", err);
    match cmd {
        RenderCmd::OpenDocument { reply, .. } => {
            let _ = reply.send(Err(anyhow::anyhow!("{}", msg)));
        }
        RenderCmd::CloseDocument { reply, .. } => {
            let _ = reply.send(());
        }
        RenderCmd::PageCount { reply, .. } => {
            let _ = reply.send(None);
        }
        RenderCmd::PageSize { reply, .. } => {
            let _ = reply.send(Err(anyhow::anyhow!("{}", msg)));
        }
        RenderCmd::RenderTile { reply, .. } => {
            let _ = reply.send(Err(anyhow::anyhow!("{}", msg)));
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the pixel dimensions of a tile given the page size.
fn compute_tile_pixel_dims(req: &TileRequest, page_size: &PageSize) -> (u32, u32) {
    let scale = req.zoom * req.dpr;
    let tile_px = (req.tile_size_css as f32 * scale) as u32;
    let full_w = (page_size.width_pts as f32 * scale) as u32;
    let full_h = (page_size.height_pts as f32 * scale) as u32;
    let tile_origin_x = req.tile_x * tile_px;
    let tile_origin_y = req.tile_y * tile_px;
    let w = tile_px.min(full_w.saturating_sub(tile_origin_x));
    let h = tile_px.min(full_h.saturating_sub(tile_origin_y));
    (w, h)
}

/// Minimal base64 encoding without pulling in a full base64 crate.
/// Uses the standard alphabet. For M1 this is fine; swap for `base64` crate if needed.
fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() * 4 / 3) + 4);
    let mut i = 0;
    while i + 2 < data.len() {
        let b0 = data[i] as usize;
        let b1 = data[i + 1] as usize;
        let b2 = data[i + 2] as usize;
        let _ = write!(
            out,
            "{}{}{}{}",
            CHARS[b0 >> 2] as char,
            CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char,
            CHARS[((b1 & 0xf) << 2) | (b2 >> 6)] as char,
            CHARS[b2 & 0x3f] as char,
        );
        i += 3;
    }
    if i < data.len() {
        let b0 = data[i] as usize;
        let _ = write!(out, "{}", CHARS[b0 >> 2] as char);
        if i + 1 < data.len() {
            let b1 = data[i + 1] as usize;
            let _ = write!(
                out,
                "{}{}=",
                CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char,
                CHARS[(b1 & 0xf) << 2] as char,
            );
        } else {
            let _ = write!(out, "{}==", CHARS[(b0 & 3) << 4] as char);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn base64_encode_hello_world() {
        // "Hello, World!" → standard base64
        let encoded = base64_encode(b"Hello, World!");
        assert_eq!(encoded, "SGVsbG8sIFdvcmxkIQ==");
    }

    #[test]
    fn base64_encode_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn render_engine_new_fails_without_pdfium() {
        // In CI / dev without PDFium binary, new() returns an Err.
        // This test confirms the error path is reachable and doesn't panic.
        // When PDFIUM_DYNAMIC_LIB_PATH is set correctly this test would
        // succeed via Ok(...) — adapt in integration harness accordingly.
        let _ = RenderEngine::new(); // either Ok or Err — both are valid here
    }

    // -----------------------------------------------------------------------
    // Corpus-gated integration tests.
    //
    // These exercise the real render core against the bench corpus. They run
    // ONLY when PDFIUM_DYNAMIC_LIB_PATH is set AND the corpus file exists,
    // otherwise they skip (return early) so the default `cargo test` stays green
    // without the PDFium binary or the (gitignored) corpus.
    //
    // IMPORTANT — run PDFium tests SINGLE-THREADED:
    //   cargo test --release -- --test-threads=1
    // PDFium keeps process-global C state and is NOT safe to drive from multiple
    // `Pdfium` instances on multiple threads at once — concurrent test threads each
    // building their own engine SIGSEGV the process. This is a TEST-harness concern
    // only: production never hits it because `RenderHandle` funnels ALL PDFium work
    // onto one dedicated OS thread (`redline-render`). The tests below mirror that
    // by requiring --test-threads=1.
    // -----------------------------------------------------------------------

    /// Resolve a corpus-relative path to an absolute `PathBuf`.
    ///
    /// Returns `None` (causing the caller to skip) when either:
    /// - `PDFIUM_DYNAMIC_LIB_PATH` is not set (PDFium binary unavailable), or
    /// - the file does not exist (corpus not checked out).
    ///
    /// `pub(crate)` so `document::save` corpus tests can reuse the same gating
    /// without duplicating the discovery logic.
    pub(crate) fn corpus(rel: &str) -> Option<PathBuf> {
        if std::env::var("PDFIUM_DYNAMIC_LIB_PATH").is_err() {
            return None;
        }
        // CARGO_MANIFEST_DIR = src-tauri; corpus lives at ../bench/corpus.
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("bench")
            .join("corpus")
            .join(rel);
        if p.exists() {
            Some(p)
        } else {
            None
        }
    }

    pub(crate) fn one_tile(page: u32) -> TileRequest {
        TileRequest {
            doc_id: "t".into(),
            page_index: page,
            tile_size_css: 512,
            tile_x: 0,
            tile_y: 0,
            zoom: 1.0,
            dpr: 2.0,
        }
    }

    #[test]
    fn c1_typical_renders_first_tile() {
        let Some(path) = corpus("c1-typical/c1-contract-691pg-A4.pdf") else {
            eprintln!("skip c1: no PDFium env or corpus");
            return;
        };
        let mut e = RenderEngine::new().expect("pdfium");
        let pages = e.open_document(path, "t".into()).expect("open c1");
        assert!(pages > 100, "C1 should have many pages");
        let tile = e.render_tile(&one_tile(0)).expect("render c1 tile");
        assert!(!tile.png_base64.is_empty());
        assert!(tile.width_px > 0 && tile.height_px > 0);
    }

    #[test]
    fn tile_cache_respects_byte_budget() {
        let Some(path) = corpus("c1-typical/c1-contract-691pg-A4.pdf") else {
            eprintln!("skip cache-budget: no PDFium env or corpus");
            return;
        };
        // Tiny 8 MB budget so eviction is forced after a handful of tiles.
        let budget = 8 * 1024 * 1024;
        let mut e = RenderEngine::new()
            .expect("pdfium")
            .with_cache_budget(budget);
        let pages = e.open_document(path, "t".into()).expect("open c1");
        // Render distinct tiles across many pages to accumulate cache pressure.
        let mut rendered = 0;
        for pg in 0..pages.min(60) {
            for tx in 0..3u32 {
                let req = TileRequest {
                    doc_id: "t".into(),
                    page_index: pg,
                    tile_size_css: 512,
                    tile_x: tx,
                    tile_y: 0,
                    zoom: 1.0,
                    dpr: 2.0,
                };
                if e.render_tile(&req).is_ok() {
                    rendered += 1;
                    // INVARIANT: after every insert the cache must be ≤ budget.
                    assert!(
                        e.tile_cache_bytes() <= budget,
                        "cache {} B exceeded budget {} B after {rendered} tiles",
                        e.tile_cache_bytes(),
                        budget
                    );
                }
            }
        }
        assert!(
            rendered > 30,
            "expected to render many tiles (got {rendered})"
        );
        // We pushed far more tile-bytes than the budget, so eviction must have run:
        // the live cache holds fewer tiles than we rendered.
        assert!(
            e.tile_cache_len() < rendered,
            "eviction did not run: {} cached == {rendered} rendered",
            e.tile_cache_len()
        );
    }

    #[test]
    fn c4_dense_a0_tile_is_fast_after_page_cache() {
        let Some(path) = corpus("c4-dense/c4-overall-plan-A0.pdf") else {
            eprintln!("skip c4: no PDFium env or corpus");
            return;
        };
        let mut e = RenderEngine::new().expect("pdfium");
        e.open_document(path, "t".into()).expect("open c4");
        // First tile pays the one-time page-load (~1s for a dense A0).
        let _ = e.render_tile(&one_tile(0)).expect("render c4 first tile");
        // Subsequent distinct tiles reuse the cached page handle → must be fast.
        let t = Instant::now();
        let mut r = one_tile(0);
        r.tile_x = 1;
        let _ = e.render_tile(&r).expect("render c4 second tile");
        let ms = t.elapsed().as_millis();
        assert!(
            ms < 200,
            "C4 cached-page tile should be well under 200ms (got {ms}ms) — page cache regression"
        );
    }

    #[test]
    fn c5_oversized_scanned_auto_normalizes_and_renders() {
        // Heavy: normalising a 2.1 GB file via lopdf peaks ~4.6 GB RSS and takes
        // several seconds. Gated behind REDLINE_BENCH_TESTS=1 so routine
        // `cargo test` stays fast/light. Run explicitly:
        //   REDLINE_BENCH_TESTS=1 PDFIUM_DYNAMIC_LIB_PATH=... cargo test --release
        if std::env::var("REDLINE_BENCH_TESTS").is_err() {
            eprintln!("skip c5: set REDLINE_BENCH_TESTS=1 to run the heavy normalize test");
            return;
        }
        let Some(path) = corpus("c5-scanned/c5-datasheets-133pg-raster.pdf") else {
            eprintln!("skip c5: no PDFium env or corpus");
            return;
        };
        let mut e = RenderEngine::new().expect("pdfium");
        // The 2.1 GB file triggers the normalize-on-open fallback (PDFium 2 GiB
        // offset limit). It must end up renderable.
        let pages = e
            .open_document(path, "t".into())
            .expect("open c5 (auto-normalize)");
        assert!(pages > 100, "C5 should expose its pages after normalize");
        let tile = e
            .render_tile(&one_tile(0))
            .expect("render c5 tile after normalize");
        assert!(!tile.png_base64.is_empty(), "C5 page 0 must render");
    }
}
