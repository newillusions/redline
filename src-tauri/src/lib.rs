//! Redline — Tauri backend entry point.
//!
//! Module layout follows spec §4:
//!   render    — PDFium tiled rasterization (M1)
//!   document  — PDF parse/model, open/save, page manipulation (M1 shell, M2+)
//!   geometry  — vector path extraction + spatial snap-target index (M1 shell, M2+)
//!   text      — text extraction + search (M4)
//!   ocr       — Tesseract invisible-text layer (M4)
//!   search    — Tantivy folder/library index (M4)
//!   markup    — annotation model + PDF serialisation (M2)
//!   takeoff   — scale calibration, measurement, quantity (M3)
//!   docops    — flatten/optimize/redact trait (M5)
//!   compare   — page-pair diff rendering (M6 / Phase 1.1)
//!   storage   — local-first file + version management (M4)

use log::{info, warn};
use tauri::Manager;

mod commands;
pub mod document;
pub mod geometry;
mod identity;
pub mod render;
pub mod sidecar;

// Stub modules — spec §4 scaffolded, implemented in future milestones
pub mod compare;
pub mod docops;
pub mod markup;
pub mod ocr;
pub mod search;
pub mod storage;
pub mod takeoff;
pub mod text;

use std::sync::Mutex;

use document::store::MarkupStore;
use render::RenderHandle;
use takeoff::ScaleStore;

/// Shared application state threaded through all Tauri commands.
///
/// `RenderHandle` is Send + Sync (it wraps only an `Arc<SyncSender>`).
/// The actual `RenderEngine` + PDFium live on a dedicated render thread
/// and are never moved across thread boundaries -- which is required because
/// `Pdfium` is !Send + !Sync (PDFium uses thread-local C state).
pub struct AppState {
    pub render: RenderHandle,
    pub markups: MarkupStore,
    pub scales: Mutex<ScaleStore>,
}

/// Resolve the bundled PDFium library path and export it via `PDFIUM_DYNAMIC_LIB_PATH`
/// so `RenderEngine::new()` (on the render thread) finds it. No-op if the env var is
/// already set (dev override / floor-machine runbook).
///
/// Lookup order (first existing wins):
///   1. Existing `PDFIUM_DYNAMIC_LIB_PATH` (respected, never overwritten).
///   2. Tauri resource dir `resources/<platform libname>` (the bundled binary).
///   3. Next to the executable `resources/<platform libname>` (portable layout).
///
/// If none resolve, the env var is left unset and `RenderEngine::new()` falls back to
/// the system library (and errors clearly if absent).
fn resolve_pdfium_path(app: &tauri::App) {
    if std::env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_some() {
        info!("PDFIUM_DYNAMIC_LIB_PATH already set — using it");
        return;
    }

    let libname = pdfium_lib_filename();
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();

    if let Ok(res_dir) = app.path().resource_dir() {
        candidates.push(res_dir.join("resources").join(libname));
        candidates.push(res_dir.join(libname));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("resources").join(libname));
            candidates.push(dir.join(libname));
        }
    }

    for c in &candidates {
        if c.exists() {
            info!("Bundled PDFium found: {:?}", c);
            std::env::set_var("PDFIUM_DYNAMIC_LIB_PATH", c);
            return;
        }
    }
    warn!(
        "No bundled PDFium found ({} candidates checked); will try system library",
        candidates.len()
    );
}

/// Platform-specific PDFium shared-library filename.
fn pdfium_lib_filename() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "pdfium.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libpdfium.dylib"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "libpdfium.so"
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Resolve the bundled PDFium path BEFORE spawning the render thread
            // (which loads PDFium). Needs the AppHandle for the resource dir, so it
            // must run here, not before the builder.
            resolve_pdfium_path(app);
            let render = RenderHandle::spawn().expect("failed to start render thread");
            app.manage(AppState {
                render,
                markups: MarkupStore::default(),
                scales: Mutex::new(ScaleStore::default()),
            });
            info!("Redline started");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Render commands (M1)
            commands::render::render_tile,
            commands::render::get_page_count,
            commands::render::get_page_size,
            // Document commands (M1 shell + M2 markup store + S1 save pipeline)
            commands::document::open_document,
            commands::document::close_document,
            commands::document::add_markup,
            commands::document::list_markups,
            commands::document::load_markups,
            commands::document::save_document,
            commands::document::save_document_as,
            commands::document::update_markup,
            commands::document::delete_markup,
            commands::document::get_user_identity,
            // Page operation commands (M4 S1)
            commands::document::rotate_page,
            commands::document::delete_page,
            commands::document::reorder_pages,
            commands::document::insert_blank_page,
            // Diagnostics (in-app §20 bench overlay)
            commands::diag::process_rss_mb,
            commands::diag::auto_open_path,
            // Takeoff commands (M3)
            commands::takeoff::add_scale,
            commands::takeoff::list_scales,
            commands::takeoff::delete_scale,
            commands::takeoff::export_markup_list,
            // Takeoff commands (M4 S1 — preset picker + /Measure write)
            commands::takeoff::list_applicable_scales,
            commands::takeoff::write_page_measure,
            // Text search commands (M4 S3)
            commands::text::search_document,
            // Version snapshot commands (M4 S2)
            commands::versioning::snapshot_version,
            commands::versioning::list_document_versions,
            commands::versioning::restore_document_version,
        ])
        .run(tauri::generate_context!())
        .expect("error while running redline");

    info!("Redline started");
}
