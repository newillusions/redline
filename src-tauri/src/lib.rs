//! Redline — Tauri backend entry point.
//!
//! Module layout follows spec §4:
//!   render    — PDFium tiled rasterization (M1)
//!   document  — PDF parse/model, open/save, page manipulation (M1 shell, M2+)
//!   geometry  — vector path extraction + spatial snap-target index (M1 shell, M2+)
//!   text      — text extraction + search (M4)
//!   search    — Tantivy folder/library index (M4)
//!   markup    — annotation model + PDF serialisation (M2)
//!   takeoff   — scale calibration, measurement, quantity (M3)
//!   docops    — flatten/optimize/redact trait (M5)
//!   compare   — page-pair diff rendering (M6 / Phase 1.1)
//!   storage   — local-first file + version management (M4)
//!   ocr       — scanned-page text recognition (Phase 2a, feature `ocr`, off by default)
//!
//! `ocr` runs Tesseract 5 via `leptess`, with a rotate-4x-and-merge pass for
//! rotated/vertical CAD text — Phase 2a (2026-09-02), switched from Phase 1's
//! `ocrs` engine after a bake-off found `ocrs`'s recognition accuracy
//! insufficient (observation:lr0cwsixkpbzei7vthon; decision:h6k4psk6n7xegql9ya9g).
//! Engine wrapper + rotate-4x + benchmark only so far. No `-ocr.pdf` writer,
//! no UI, no auto-trigger yet — see the module's own doc comment.

use log::{info, warn};
use tauri::Manager;

mod commands;
pub mod document;
pub mod geometry;
mod identity;
pub mod license;
#[cfg(feature = "ocr")]
pub mod ocr;
mod panic_guard;
pub mod render;
pub mod rpc;
pub mod sidecar;
pub mod updater_rollback;

// Stub modules — spec §4 scaffolded, implemented in future milestones
pub mod compare;
pub mod docops;
pub mod markup;
pub mod search;
pub mod storage;
pub mod takeoff;
pub mod text;
pub mod toolchest;

use std::sync::Mutex;

use document::store::MarkupStore;
use render::RenderHandle;
use takeoff::ScaleStore;
use toolchest::{SequenceCounters, ToolChestStore};

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
    /// Active folder full-text search index (M4 S4). `None` until the
    /// `open_folder_index` command is called.
    pub folder_index: Mutex<Option<search::FolderIndex>>,
    /// Tool Chest: Tool Sets + Recent Tools, persisted under the app-data dir (M2).
    pub toolchest: ToolChestStore,
    /// In-memory dynamic-stamp sequence counters (M2). See `toolchest::sequence` doc
    /// comment for the named sidecar-persistence deferral.
    pub sequence_counters: SequenceCounters,
    /// The frontend's currently-focused tab (MCP server design, Phase 2a -
    /// `get_active_document`/`list_open_documents`). Tab/active-tab state otherwise
    /// lives entirely in Svelte (`DocTabStore.activeDocId`, `src/lib/doc-tabs.svelte.ts`),
    /// so there is no other backend concept of "the active document" for a multi-tab
    /// session. The frontend pushes every change here via the `set_active_document`
    /// command (see `commands::document::set_active_document`'s doc comment for the
    /// `$effect` that keeps this synced from `App.svelte`). `None` when no document is
    /// open, or (a named limitation) when a document was closed via the MCP
    /// `close_document` tool rather than the GUI, since the frontend has no channel to
    /// learn about a backend-driven close and refresh its own tab list - this can only
    /// be cleared from the closing side instead (`commands::document::close_document`
    /// and the MCP bridge's `close_document` dispatch arm both clear it when it matches
    /// the closed doc_id).
    pub active_doc: Mutex<Option<String>>,
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
/// Windows-only: disable WebView2's own native zoom control so it can't compete with this
/// app's own JS-driven canvas-tile zoom (Viewport.svelte `onWheel` / `$lib/viewport`
/// `classifyWheelEvent`).
///
/// A Windows Precision Touchpad pinch and an explicit Ctrl+wheel both arrive at the DOM as
/// a synthetic `wheel` event with `ctrlKey: true` (Chromium
/// `WebViewImpl::HandleSyntheticWheelFromTouchpadPinchEvent`), which `classifyWheelEvent`
/// already routes to the app's own zoom. But WebView2 ALSO has its own native zoom control
/// (`ICoreWebView2Settings.IsZoomControlEnabled`, default `TRUE` per Microsoft Learn) that
/// responds to the same Ctrl+wheel/pinch-synthesized-ctrl+wheel input at the WebView2 host
/// layer, above the DOM - a host-level setting, not a page-level default action, so the
/// page's own `e.preventDefault()` in `onWheel` cannot suppress it. Left enabled, it scales
/// the whole rendered webview surface as a second, uncoordinated zoom on top of/instead of
/// the app's own tile-based one. Disabling it here makes the JS zoom path the sole zoom
/// mechanism on Windows, matching macOS (which has no equivalent competing native zoom).
/// No effect on non-Windows builds - see the stub below.
///
/// Owner-reported symptom this addresses: "pinch doesn't seem to work" on a Windows
/// trackpad (redline gesture-feedback PR, 2026-09-15). Not verified on real Windows
/// hardware in this session (no Windows CI leg, no SSH to the test laptop) - see the PR's
/// owner test checklist.
#[cfg(target_os = "windows")]
fn disable_native_zoom_control_on_windows(app: &tauri::App) {
    use tauri::Manager;

    let Some(window) = app.get_webview_window("main") else {
        warn!("main webview window not found; native WebView2 zoom control left enabled");
        return;
    };
    let result = window.with_webview(|webview| {
        // SAFETY: a synchronous COM property set on the WebView2 settings object,
        // executed on the main thread (per `with_webview`'s contract) - no aliasing or
        // lifetime hazard, `settings`/`core`/`controller` don't outlive this closure.
        unsafe {
            let controller = webview.controller();
            match controller.CoreWebView2() {
                Ok(core) => match core.Settings() {
                    Ok(settings) => {
                        if let Err(e) = settings.SetIsZoomControlEnabled(false) {
                            warn!("WebView2 SetIsZoomControlEnabled(false) failed: {e}");
                        }
                    }
                    Err(e) => warn!("WebView2 ICoreWebView2::Settings() failed: {e}"),
                },
                Err(e) => warn!("WebView2 ICoreWebView2Controller::CoreWebView2() failed: {e}"),
            }
        }
    });
    if let Err(e) = result {
        warn!("with_webview (disable native WebView2 zoom control) failed: {e}");
    }
}

/// No-op on every non-Windows platform - WebView2's competing native zoom control (see the
/// Windows implementation above) doesn't exist on macOS/WKWebView or Linux/webkit2gtk.
#[cfg(not(target_os = "windows"))]
fn disable_native_zoom_control_on_windows(_app: &tauri::App) {}

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

/// Resolve the bundled Tesseract tessdata directory and export it via
/// `TESSDATA_PREFIX` (Tesseract's own standard lookup env var — see the
/// `ocr` module doc comment) so `OcrEngineHandle::load(None)` finds
/// `eng.traineddata` without a caller having to pass an explicit path.
/// No-op if the env var is already set (dev override / floor-machine
/// runbook), mirroring `resolve_pdfium_path` above.
///
/// Lookup order (first existing wins), same shape as `resolve_pdfium_path`:
///   1. Existing `TESSDATA_PREFIX` (respected, never overwritten).
///   2. Tauri resource dir `resources/ocr/tessdata` (the bundled directory —
///      `scripts/fetch-ocr-tessdata.sh` populates `src-tauri/resources/ocr/
///      tessdata/eng.traineddata`, and `tauri.conf.json`'s
///      `bundle.resources` wholesale-maps `resources/` into the bundle).
///   3. Next to the executable `resources/ocr/tessdata` (portable layout).
///
/// If none resolve AND the `ocr` feature is compiled in, this logs loudly
/// (`log::error!`) rather than panicking: OCR is off by default (Phase 2a/2b
/// ship no auto-trigger or UI yet — see docs/ocr.md), so a missing tessdata
/// directory must not block the rest of the app from starting. The actual
/// fail-loud behavior for an end user attempting to run OCR happens at the
/// point of use, in `OcrEngineHandle::load`'s own error message (Phase 2c's
/// invoke command surfaces that error to the UI).
///
/// Returns the resolved directory (or `None`) as `ocr::TessdataDir`, which the caller
/// `app.manage`s — see that struct's doc comment for why `run_ocr_document` must be
/// handed this directory EXPLICITLY rather than relying solely on the `TESSDATA_PREFIX`
/// env var this function still sets as a secondary courtesy (Windows CRT `getenv` split,
/// found 2026-09-14).
#[cfg(feature = "ocr")]
fn resolve_tessdata_dir(app: &tauri::App) -> crate::ocr::TessdataDir {
    if let Some(existing) = std::env::var_os("TESSDATA_PREFIX") {
        info!("TESSDATA_PREFIX already set — using it");
        return crate::ocr::TessdataDir {
            dir: Some(std::path::PathBuf::from(existing)),
            candidates_checked: Vec::new(),
        };
    }

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(res_dir) = app.path().resource_dir() {
        candidates.push(res_dir.join("resources").join("ocr").join("tessdata"));
        candidates.push(res_dir.join("ocr").join("tessdata"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("resources").join("ocr").join("tessdata"));
            candidates.push(dir.join("ocr").join("tessdata"));
        }
    }

    if let Some(found) = first_valid_tessdata_candidate(&candidates) {
        info!("Bundled tessdata found: {:?}", found);
        // Secondary courtesy export only — see `ocr::TessdataDir`'s doc comment for why
        // this alone does not reach Tesseract's `getenv` on Windows. The `dir` field
        // below, threaded explicitly into `OcrEngineHandle::load`, is load-bearing.
        std::env::set_var("TESSDATA_PREFIX", &found);
        return crate::ocr::TessdataDir {
            dir: Some(found),
            candidates_checked: candidates,
        };
    }
    log::error!(
        "No bundled tessdata found ({} candidates checked); OCR will fail at first use \
         unless TESSDATA_PREFIX is set or a system Tesseract install provides eng.traineddata",
        candidates.len()
    );
    crate::ocr::TessdataDir {
        dir: None,
        candidates_checked: candidates,
    }
}

/// Pure candidate-resolution core of `resolve_tessdata_dir`, factored out so it is
/// unit-testable without a `tauri::App` (which needs a real running app context to
/// construct). Returns the first candidate directory that contains `eng.traineddata`,
/// in the given order.
#[cfg(feature = "ocr")]
fn first_valid_tessdata_candidate(candidates: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    candidates
        .iter()
        .find(|c| c.join("eng.traineddata").exists())
        .cloned()
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
    // Install before anything else (render thread, plugins) spawns, so a panic on
    // any thread - not just main - is routed through the file log rather than
    // vanishing into a release build's detached stderr.
    panic_guard::install_panic_hook();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init());

    // Tier-2 real-app E2E harness (WebdriverIO + @wdio/tauri-service, embedded provider) -
    // debug-only, never registered in a release build. `tauri-plugin-wdio-webdriver` runs the
    // embedded WebDriver server inside this process; `tauri-plugin-wdio` exposes
    // `browser.tauri.execute()`/IPC-mock/log-capture to the WDIO side. See wdio.conf.js and
    // docs/TESTING.md. Pattern copied verbatim from satchel-gui (PR #25).
    #[cfg(debug_assertions)]
    let builder = builder
        .plugin(tauri_plugin_wdio_webdriver::init())
        .plugin(tauri_plugin_wdio::init());

    builder
        .setup(|app| {
            // Resolve the bundled PDFium path BEFORE spawning the render thread
            // (which loads PDFium). Needs the AppHandle for the resource dir, so it
            // must run here, not before the builder.
            resolve_pdfium_path(app);
            #[cfg(feature = "ocr")]
            {
                let tessdata_dir = resolve_tessdata_dir(app);
                app.manage(tessdata_dir);
            }
            disable_native_zoom_control_on_windows(app);
            let render = RenderHandle::spawn().expect("failed to start render thread");
            let toolchest = app
                .path()
                .app_data_dir()
                .map_err(|e| e.to_string())
                .and_then(|dir| ToolChestStore::load(&dir).map_err(|e| e.to_string()))
                .unwrap_or_else(|e| {
                    warn!("Tool Chest store failed to load ({e}); starting empty in-memory store");
                    ToolChestStore::in_memory()
                });
            app.manage(AppState {
                render,
                markups: MarkupStore::default(),
                scales: Mutex::new(ScaleStore::default()),
                folder_index: Mutex::new(None),
                toolchest,
                sequence_counters: SequenceCounters::new(),
                active_doc: Mutex::new(None),
            });
            // MCP companion bridge (design §2/§5) - started once, after AppState is
            // managed (the bridge dispatches into it), lives for the app's lifetime
            // (see rpc module doc comment for the v1 per-app-vs-per-document
            // simplification). Errors are logged inside rpc::start, never fatal to
            // the GUI's own startup.
            rpc::start(app.handle());
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
            commands::document::set_active_document,
            commands::document::add_markup,
            commands::document::list_markups,
            commands::document::load_markups,
            commands::document::save_document,
            commands::document::save_document_as,
            commands::document::update_markup,
            commands::document::delete_markup,
            commands::document::get_user_identity,
            commands::document::save_unprotected_copy,
            commands::document::remember_password,
            // Page operation commands (M4 S1)
            commands::document::rotate_page,
            commands::document::delete_page,
            commands::document::reorder_pages,
            commands::document::insert_blank_page,
            commands::document::extract_pages,
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
            // Text selection commands (I-beam tool: text selection + text-anchored highlight)
            commands::text_select::char_index_at_point,
            commands::text_select::get_text_selection,
            // Vector snap-target index (spec §5, v1)
            commands::geometry::get_page_snap_targets,
            // Version snapshot commands (M4 S2)
            commands::versioning::snapshot_version,
            commands::versioning::list_document_versions,
            commands::versioning::restore_document_version,
            // Folder full-text search commands (M4 S4)
            commands::search::open_folder_index,
            commands::search::search_folder,
            commands::search::folder_index_status,
            commands::search::search_paths,
            // DocOps commands (M5)
            commands::docops::flatten_document,
            commands::docops::optimize_document,
            commands::docops::redact_document,
            // OCR commands (Phase 2c-ii — "OCR this document" action + auto-OCR-on-open
            // detection; run_ocr_document's real work is #[cfg(feature = "ocr")]-gated
            // inside the command itself, see commands::ocr's module doc comment)
            commands::ocr::document_needs_ocr,
            commands::ocr::run_ocr_document,
            // Compare commands (M6 Phase 1.1)
            commands::compare::compare_pages,
            // Recent docs (MRU list, Document History panel)
            commands::recent_docs::load_recent_docs,
            commands::recent_docs::save_recent_docs,
            commands::recent_docs::check_file_exists,
            // Application settings (local user preferences)
            commands::settings::load_settings,
            commands::settings::save_settings,
            // S2b client entitlement (emittiv-staff license consumer)
            commands::license::license_status,
            commands::license::activate_license,
            commands::license::renew_license,
            commands::license::license_info,
            commands::license::deactivate_license,
            // Tool Chest commands (M2)
            commands::toolchest::list_tool_sets,
            commands::toolchest::recent_tools,
            commands::toolchest::create_tool_set,
            commands::toolchest::rename_tool_set,
            commands::toolchest::delete_tool_set,
            commands::toolchest::add_tool_from_markup,
            commands::toolchest::delete_tool,
            commands::toolchest::reorder_tools,
            commands::toolchest::record_recent_tool,
            commands::toolchest::import_btx,
            commands::toolchest::next_stamp_sequence,
            commands::toolchest::compose_stamp_text,
            // About page: release history + dev-stage rollback
            commands::updater::list_available_releases,
            commands::updater::rollback_to_version,
        ])
        .run(tauri::generate_context!())
        .expect("error while running redline");

    info!("Redline started");
}

#[cfg(all(test, feature = "ocr"))]
mod tessdata_resolution_tests {
    use super::first_valid_tessdata_candidate;

    /// No candidates at all (e.g. neither the Tauri resource dir nor
    /// `current_exe().parent()` resolved) — must return `None`, not panic.
    #[test]
    fn empty_candidate_list_returns_none() {
        assert_eq!(first_valid_tessdata_candidate(&[]), None);
    }

    /// None of the candidate directories contain `eng.traineddata` — must return `None`
    /// (the "no bundled tessdata found, log loudly" path), not silently accept a
    /// directory that exists but is empty.
    #[test]
    fn candidates_without_traineddata_return_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Directory exists, but has no eng.traineddata inside it.
        let empty_candidate = dir.path().join("ocr").join("tessdata");
        std::fs::create_dir_all(&empty_candidate).expect("mkdir");

        // A second candidate that doesn't even exist as a directory.
        let missing_candidate = dir.path().join("does-not-exist").join("tessdata");

        let result = first_valid_tessdata_candidate(&[empty_candidate, missing_candidate]);
        assert_eq!(result, None);
    }

    /// The first candidate containing `eng.traineddata` wins, in list order — matching
    /// `resolve_tessdata_dir`'s documented lookup order (resource dir before
    /// next-to-executable, `resources/ocr/tessdata` before the bare `ocr/tessdata`
    /// fallback within each).
    #[test]
    fn first_candidate_with_traineddata_wins_in_order() {
        let dir = tempfile::tempdir().expect("tempdir");

        let first = dir.path().join("first");
        std::fs::create_dir_all(&first).expect("mkdir first");
        std::fs::write(first.join("eng.traineddata"), b"stub").expect("write first");

        let second = dir.path().join("second");
        std::fs::create_dir_all(&second).expect("mkdir second");
        std::fs::write(second.join("eng.traineddata"), b"stub").expect("write second");

        let result =
            first_valid_tessdata_candidate(&[first.clone(), second]).expect("some candidate");
        assert_eq!(result, first);
    }

    /// A later candidate is found when earlier ones lack `eng.traineddata` — the
    /// "portable layout" (next-to-executable) candidate resolving when the bundled
    /// resource-dir candidate doesn't exist, which is exactly the shape the Windows CI
    /// smoke test exercises (see `.github/workflows/build-releases.yml`).
    #[test]
    fn later_candidate_found_when_earlier_ones_lack_traineddata() {
        let dir = tempfile::tempdir().expect("tempdir");

        let missing = dir.path().join("resources").join("ocr").join("tessdata");
        // Deliberately not created — simulates a resource dir that doesn't resolve at all.

        let portable = dir.path().join("ocr").join("tessdata");
        std::fs::create_dir_all(&portable).expect("mkdir portable");
        std::fs::write(portable.join("eng.traineddata"), b"stub").expect("write portable");

        let result =
            first_valid_tessdata_candidate(&[missing, portable.clone()]).expect("some candidate");
        assert_eq!(result, portable);
    }
}
