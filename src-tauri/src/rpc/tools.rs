//! Tool-handler functions behind the RPC dispatcher (`rpc::dispatch`).
//!
//! Read-only and markup-CRUD handlers operate on `&MarkupStore` alone - no
//! `AppState`/Tauri/PDFium dependency - and are unit-tested directly here, mirroring
//! how `document::store`'s own tests work (MCP server design §3: these are "thin
//! wrapper[s] over infrastructure that exists" - `MarkupStore`'s existing
//! add/update/delete/list, not a new domain-logic path).
//!
//! `save_document`/`flatten_document`/`reduce_file_size` need the full render+file I/O
//! pipeline (`apply_page_edit`, PDFium reopen) and are NOT reimplemented here - see
//! `dispatch::dispatch`, which calls the exact existing `commands::document`/
//! `commands::docops` Tauri command functions instead, via `AppHandle::state()`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::document::store::{MarkupStore, OpenDocEntry};
use crate::markup::{Appearance, Markup, MarkupGeometry, MarkupStatus, MarkupType, UserRef};

/// Compact summary returned by `list_markups`/`search_markups` (MCP server design §3:
/// id, markup_type, page, subject, contents (truncated), locked, locked_contents,
/// workflow.status, count_set.name?). `read_markup` returns the full envelope instead.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkupSummary {
    pub id: String,
    pub markup_type: MarkupType,
    pub page: u32,
    pub subject: Option<String>,
    pub contents: Option<String>,
    pub locked: bool,
    pub locked_contents: bool,
    pub workflow_status: MarkupStatus,
    pub count_set_name: Option<String>,
}

/// A note beyond this length is truncated in a summary listing - `read_markup` returns
/// the untruncated envelope for the rare case an agent needs the full text.
const CONTENTS_TRUNCATE_CHARS: usize = 200;

fn truncate(s: &Option<String>) -> Option<String> {
    s.as_ref().map(|s| {
        if s.chars().count() > CONTENTS_TRUNCATE_CHARS {
            let head: String = s.chars().take(CONTENTS_TRUNCATE_CHARS).collect();
            format!("{head}\u{2026}")
        } else {
            s.clone()
        }
    })
}

fn summarize(m: &Markup) -> MarkupSummary {
    MarkupSummary {
        id: m.id().to_string(),
        markup_type: m.markup_type,
        page: m.page,
        subject: m.subject.clone(),
        contents: truncate(&m.contents),
        locked: m.is_locked(),
        locked_contents: m.is_contents_locked(),
        workflow_status: m.workflow.status,
        count_set_name: m.count_set.as_ref().map(|cs| cs.name.clone()),
    }
}

/// Fixed, stable synthetic-author identity used for markups an MCP tool call
/// creates/edits, rather than impersonating whichever human is running redline - so the
/// audit trail always makes an AI-driven edit visibly AI-driven. Constant across every
/// MCP call in every session (not per-call random), matching how a real user account
/// stays constant across their edits.
const MCP_AUTHOR_UUID: uuid::Uuid = uuid::uuid!("b3b1a2c4-3f6d-4c2e-9d0a-6a1f2b3c4d5e");

fn mcp_author() -> UserRef {
    UserRef {
        user_id: MCP_AUTHOR_UUID,
        display_name: "redline-mcp".to_string(),
    }
}

// ---------------------------------------------------------------------------
// list_markups
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListMarkupsParams {
    pub doc_id: String,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub type_filter: Vec<MarkupType>,
    #[serde(default)]
    pub status_filter: Vec<MarkupStatus>,
}

pub fn list_markups(
    store: &MarkupStore,
    p: &ListMarkupsParams,
) -> Result<Vec<MarkupSummary>, String> {
    let markups = store.list(&p.doc_id)?;
    Ok(markups
        .iter()
        .filter(|m| p.page.map_or(true, |pg| m.page == pg))
        .filter(|m| p.type_filter.is_empty() || p.type_filter.contains(&m.markup_type))
        .filter(|m| p.status_filter.is_empty() || p.status_filter.contains(&m.workflow.status))
        .map(summarize)
        .collect())
}

// ---------------------------------------------------------------------------
// read_markup
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ReadMarkupParams {
    pub doc_id: String,
    pub markup_id: String,
}

pub fn read_markup(store: &MarkupStore, p: &ReadMarkupParams) -> Result<Markup, String> {
    let id = uuid::Uuid::parse_str(&p.markup_id).map_err(|e| format!("bad markup id: {e}"))?;
    store
        .list(&p.doc_id)?
        .into_iter()
        .find(|m| m.id() == id)
        .ok_or_else(|| format!("unknown markup id {}", p.markup_id))
}

// ---------------------------------------------------------------------------
// search_markups
// ---------------------------------------------------------------------------

fn default_scope() -> String {
    "document".to_string()
}

#[derive(Debug, Deserialize)]
pub struct SearchMarkupsParams {
    pub doc_id: String,
    pub query: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default = "default_scope")]
    pub scope: String,
}

/// Search markup `subject`/`contents` text (mirrors the frontend's markup-comment
/// search, `src/lib/markup-search.ts`, per the design's own grounding for this tool -
/// design §1/§3: "thin wrapper over infrastructure that exists" for finding a markup by
/// its note text, distinct from `search_document`'s raw-PDF-text search). v1 supports
/// `scope: "document"` only; the design's other illustrative scope values
/// (`page`/`open_docs`/`recents`/`folder`) are named but refused with a clear
/// "not yet supported" error rather than silently narrowing to document scope - a
/// named simplification, not a silent gap.
pub fn search_markups(
    store: &MarkupStore,
    p: &SearchMarkupsParams,
) -> Result<Vec<MarkupSummary>, String> {
    if p.scope != "document" {
        return Err(format!(
            "search scope '{}' not yet supported by redline-mcp v1 - only 'document' is implemented",
            p.scope
        ));
    }
    if p.query.is_empty() {
        return Ok(Vec::new());
    }
    let markups = store.list(&p.doc_id)?;
    let needle = if p.case_sensitive {
        p.query.clone()
    } else {
        p.query.to_lowercase()
    };
    let field_matches = |s: &Option<String>| -> bool {
        s.as_ref().is_some_and(|s| {
            let hay = if p.case_sensitive {
                s.clone()
            } else {
                s.to_lowercase()
            };
            hay.contains(&needle)
        })
    };
    Ok(markups
        .iter()
        .filter(|m| field_matches(&m.subject) || field_matches(&m.contents))
        .map(summarize)
        .collect())
}

// ---------------------------------------------------------------------------
// export_markup_schedule
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ExportMarkupScheduleParams {
    pub doc_id: String,
    pub format: crate::commands::takeoff::ExportFormat,
}

#[derive(Debug, Serialize)]
pub struct ExportMarkupScheduleResult {
    pub out_path: String,
}

/// Exports to a generated path next to the source document (MCP server design §3
/// output: "file path of the generated export" - an OUTPUT the tool produces, not an
/// input the caller supplies, unlike the underlying `export_markup_list` Tauri command
/// which takes an explicit destination from a GUI file dialog).
pub fn export_markup_schedule(
    store: &MarkupStore,
    p: ExportMarkupScheduleParams,
) -> Result<ExportMarkupScheduleResult, String> {
    let markups = store.list(&p.doc_id)?;
    let src_path = store
        .path(&p.doc_id)
        .ok_or_else(|| format!("unknown doc_id {}", p.doc_id))?;
    let ext = match p.format {
        crate::commands::takeoff::ExportFormat::Xlsx => "xlsx",
        crate::commands::takeoff::ExportFormat::Csv => "csv",
    };
    let stem = src_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let dir = src_path
        .parent()
        .ok_or_else(|| "source document has no parent directory".to_string())?;
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let out_path = dir.join(format!("{stem}-markup-schedule-{timestamp}.{ext}"));
    crate::commands::takeoff::export_markup_list_to(&markups, &out_path, p.format)?;
    Ok(ExportMarkupScheduleResult {
        out_path: out_path.to_string_lossy().into_owned(),
    })
}

// ---------------------------------------------------------------------------
// list_open_documents / open_document / close_document / get_active_document
// (MCP server design, Phase 2a, 2026-09-03 - document lifecycle tools)
//
// Every tool in this section needs more than `&MarkupStore` alone (page counts come
// from the render engine, "active" from `AppState::active_doc`, and open_document/
// close_document must call the exact existing `commands::document::open_document`/
// `close_document` Tauri commands per the design's single-writer-correctness argument
// - see the module doc comment at the top of this file). Only the parts that ARE pure
// transforms over already-fetched data live here, unit-tested directly; the async
// AppHandle-dependent orchestration lives in `dispatch::dispatch`, matching how
// `save_document`/`flatten_document`/`reduce_file_size` are handled.
// ---------------------------------------------------------------------------

/// One open document as reported by `list_open_documents`. `title` is the file name
/// component of `path` (the only "title" concept redline has - there is no separate
/// display-name field anywhere in the document model).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpenDocumentSummary {
    pub doc_id: String,
    pub path: String,
    pub title: String,
    pub page_count: u32,
    pub is_active: bool,
    pub dirty: bool,
}

/// Build the `list_open_documents` response from data the caller (`dispatch::dispatch`)
/// has already fetched: `entries` from `MarkupStore::list_open`, `page_counts` keyed by
/// doc_id from the render engine (missing entries default to `0` - a doc registered in
/// `MarkupStore` but not yet resolvable in the render engine is a real, if narrow, race
/// between `register` and the render thread's own open completing; `0` is a visibly
/// wrong-looking sentinel an agent should not mistake for a real empty document, but a
/// missing page count must never make the whole tool call fail for every other open
/// doc), and `active_doc_id` from `AppState::active_doc`. Order matches `entries`'
/// (unspecified - `MarkupStore` is `HashMap`-backed).
pub fn list_open_documents(
    entries: Vec<OpenDocEntry>,
    active_doc_id: Option<&str>,
    page_counts: &HashMap<String, u32>,
) -> Vec<OpenDocumentSummary> {
    entries
        .into_iter()
        .map(|e| {
            let title = e
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| e.path.to_string_lossy().into_owned());
            OpenDocumentSummary {
                is_active: active_doc_id == Some(e.doc_id.as_str()),
                dirty: e.dirty,
                page_count: page_counts.get(&e.doc_id).copied().unwrap_or(0),
                path: e.path.to_string_lossy().into_owned(),
                doc_id: e.doc_id,
                title,
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct OpenDocumentParams {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct CloseDocumentParams {
    pub doc_id: String,
    #[serde(default)]
    pub discard_changes: bool,
}

// ---------------------------------------------------------------------------
// create_markup
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateMarkupParams {
    pub doc_id: String,
    pub markup_type: MarkupType,
    pub page: u32,
    pub geometry: MarkupGeometry,
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub contents: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
}

/// No lock check here - there is no existing target to lock before a markup exists
/// (see the doc comment on `markup::check_not_locked`).
pub fn create_markup(store: &MarkupStore, p: CreateMarkupParams) -> Result<Markup, String> {
    let mut m = Markup::new(
        p.markup_type,
        p.page,
        p.geometry,
        p.appearance,
        mcp_author(),
    );
    m.contents = p.contents;
    m.subject = p.subject;
    m.layer = p.layer;
    store.add(&p.doc_id, m.clone())?;
    Ok(m)
}

// ---------------------------------------------------------------------------
// update_markup
// ---------------------------------------------------------------------------

/// Partial field set (design §3: "no field-level patch API yet" at the PDF layer, but
/// the MCP tool itself accepts a partial set and merges onto the current markup before
/// calling `MarkupStore::update`, which still replaces the whole annotation in one
/// call; see `markup::check_not_locked`'s doc comment for why the lock guard stays
/// whole-mutation-refusing even though this merge is field-level).
///
/// v1 limitation, named plainly: a field can be SET but not cleared back to `None` (no
/// JSON `null`-vs-absent distinction implemented) - pass an empty string to blank
/// `contents` if needed.
#[derive(Debug, Deserialize)]
pub struct UpdateMarkupParams {
    pub doc_id: String,
    pub markup_id: String,
    #[serde(default)]
    pub contents: Option<String>,
    #[serde(default)]
    pub appearance: Option<Appearance>,
    #[serde(default)]
    pub workflow_status: Option<MarkupStatus>,
}

/// Guard enforced inside `MarkupStore::update`, not here - see its doc comment.
pub fn update_markup(store: &MarkupStore, p: UpdateMarkupParams) -> Result<Markup, String> {
    let id = uuid::Uuid::parse_str(&p.markup_id).map_err(|e| format!("bad markup id: {e}"))?;
    let mut existing = store
        .list(&p.doc_id)?
        .into_iter()
        .find(|m| m.id() == id)
        .ok_or_else(|| format!("unknown markup id {}", p.markup_id))?;

    if let Some(contents) = p.contents {
        existing.contents = Some(contents);
    }
    if let Some(appearance) = p.appearance {
        existing.appearance = appearance;
    }
    if let Some(status) = p.workflow_status {
        existing.workflow.status = status;
    }
    existing.touch(mcp_author());

    store.update(&p.doc_id, existing.clone())?;
    Ok(existing)
}

// ---------------------------------------------------------------------------
// delete_markup
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DeleteMarkupParams {
    pub doc_id: String,
    pub markup_id: String,
}

/// Guard enforced inside `MarkupStore::delete`, not here - see its doc comment.
pub fn delete_markup(store: &MarkupStore, p: &DeleteMarkupParams) -> Result<(), String> {
    let id = uuid::Uuid::parse_str(&p.markup_id).map_err(|e| format!("bad markup id: {e}"))?;
    store.delete(&p.doc_id, id)
}

// ---------------------------------------------------------------------------
// save_document / flatten_document / reduce_file_size — param shapes only.
// Execution needs `AppState` (render engine + file I/O) and is a thin pass-through in
// `dispatch::dispatch` to the existing Tauri command functions - not reimplemented or
// independently unit-tested here (same class of PDFium/render-thread dependency the
// rest of this crate's test suite already gates behind REDLINE_BENCH_TESTS).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SaveDocumentParams {
    pub doc_id: String,
}

#[derive(Debug, Deserialize)]
pub struct FlattenDocumentParams {
    pub doc_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ReduceFileSizeParams {
    pub doc_id: String,
    #[serde(default)]
    pub level: Option<u8>,
    #[serde(default)]
    pub image_preset: Option<crate::docops::ImageQualityPreset>,
}

// ---------------------------------------------------------------------------
// Phase 2b (2026-09-03): search, takeoff, page ops, compare, docops,
// save_document_as. Param shapes only, same pattern as the block above -
// execution needs the full AppState (render engine / PDFium / file I/O /
// search index / scale store) and is a thin pass-through in
// `dispatch::dispatch` to the EXISTING Tauri command functions
// (`commands::search`/`commands::takeoff`/`commands::document`/
// `commands::compare`/`commands::docops`) - not reimplemented here.
// Argument-validation tests below cover serde shape only; the wrapped
// behaviour itself is covered by each underlying command's own existing
// tests plus this PR's live-drive transcript (see PR description).
// ---------------------------------------------------------------------------

// --- search (commands::text, commands::search) ---

#[derive(Debug, Deserialize)]
pub struct SearchDocumentParams {
    pub doc_id: String,
    pub query: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
}

#[derive(Debug, Deserialize)]
pub struct OpenFolderIndexParams {
    pub root: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchFolderParams {
    pub root: String,
    pub query: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct FolderIndexStatusParams {
    #[serde(default)]
    pub root: Option<String>,
}

/// Wraps `search::IndexStatus` with a `matches_requested_root` flag - redline holds
/// exactly one active folder index at a time (`AppState.folder_index: Mutex<Option<
/// FolderIndex>>`), so a caller polling a DIFFERENT root than whatever is currently
/// indexed gets back the REAL (mismatched) status plus this flag, rather than a
/// silently-narrowed or fabricated "not found" result.
#[derive(Debug, Serialize)]
pub struct FolderIndexStatusResult {
    #[serde(flatten)]
    pub status: crate::search::IndexStatus,
    pub matches_requested_root: bool,
}

// --- takeoff (commands::takeoff) ---

#[derive(Debug, Deserialize)]
pub struct ListScalesParams {
    pub doc_id: String,
}

#[derive(Debug, Deserialize)]
pub struct AddScaleParams {
    pub doc_id: String,
    #[serde(default)]
    pub applies_to_page: Option<u32>,
    pub ratio: f64,
    pub unit: String,
    pub label: String,
    pub precision: u8,
}

#[derive(Debug, Deserialize)]
pub struct DeleteScaleParams {
    pub doc_id: String,
    pub scale_id: String,
}

#[derive(Debug, Deserialize)]
pub struct WritePageMeasureParams {
    pub doc_id: String,
    pub page_idx: u32,
    pub scale_id: String,
}

/// `export_markup_list` requires the caller to supply the destination `path`,
/// matching the underlying Tauri command 1:1 (it's driven by a GUI save dialog on
/// the frontend). This is the named difference from `export_markup_schedule`
/// (wave 1, `export_markup_schedule` above), which generates its own path next to
/// the source document - both ultimately call the exact same writer
/// (`commands::takeoff::export_markup_list_to`) and produce byte-identical output
/// for the same input; the two tools differ only in path-selection semantics, not
/// in export logic.
#[derive(Debug, Deserialize)]
pub struct ExportMarkupListParams {
    pub doc_id: String,
    pub path: String,
    pub format: crate::commands::takeoff::ExportFormat,
}

// --- page ops (commands::document) ---
//
// None of these are gated behind save_document - apply_page_edit (their shared
// implementation) writes the file atomically (temp + rename) and reloads the
// render engine IMMEDIATELY, unlike markup create/update/delete which stay in
// MarkupStore until save_document is called. There is also no document-level
// lock concept anywhere in this codebase to check - only the markup-level
// Locked/LockedContents PDF annotation flags (`markup::check_not_locked`), which
// apply to individual annotations, not page structure. Both facts are stated in
// each tool's MCP description in `redline_mcp.rs` rather than assumed.

#[derive(Debug, Deserialize)]
pub struct RotatePageParams {
    pub doc_id: String,
    pub page_idx: u32,
    pub degrees: i32,
}

#[derive(Debug, Deserialize)]
pub struct DeletePageParams {
    pub doc_id: String,
    pub page_idx: u32,
}

#[derive(Debug, Deserialize)]
pub struct ReorderPagesParams {
    pub doc_id: String,
    pub new_order: Vec<u32>,
}

#[derive(Debug, Deserialize)]
pub struct InsertBlankPageParams {
    pub doc_id: String,
    pub at: u32,
    pub width: f32,
    pub height: f32,
}

// --- compare (commands::compare) ---

/// Named deviation from every other tool: `compare_pages` takes `path_a`/`path_b`
/// directly, not `doc_id` - matching the underlying Tauri command exactly ("wrap,
/// don't reimplement"). The command itself never touches `AppState`/`MarkupStore`;
/// it runs a standalone two-tier diff over two raw file paths and does NOT require
/// either document to be open in redline.
#[derive(Debug, Deserialize)]
pub struct ComparePagesParams {
    pub path_a: String,
    pub path_b: String,
    pub page_a: u32,
    pub page_b: u32,
    #[serde(default)]
    pub dpi: Option<f32>,
    #[serde(default)]
    pub pixel_tolerance: Option<u8>,
}

/// Diff summary WITHOUT the base64 PNG overlay (`compare::PageDiffResult::
/// diff_png_b64`) - an MCP tool response is JSON-in-text, and embedding a
/// per-page raster image (tens to hundreds of KB) in every compare call serves
/// no purpose for a calling agent, which wants the numeric verdict, not to
/// render an image. Wraps the EXACT SAME `compare::run_two_tier_diff` result the
/// Tauri command returns - no new diff logic, one field dropped. The M6 diff
/// engine's public output is aggregate tier-1/tier-2 stats only (no per-region
/// breakdown), so this mirrors that shape rather than inventing a "regions" field
/// the crate doesn't produce.
#[derive(Debug, Clone, Serialize)]
pub struct ComparePagesSummary {
    pub text_char_match: bool,
    pub text_delta_count: usize,
    pub text_rms_delta_pts: f32,
    pub pixel_passed: bool,
    pub changed_pct: f32,
    pub max_pixel_delta: u8,
    pub render_dpi: f32,
}

impl From<crate::compare::PageDiffResult> for ComparePagesSummary {
    fn from(r: crate::compare::PageDiffResult) -> Self {
        Self {
            text_char_match: r.text_char_match,
            text_delta_count: r.text_delta_count,
            text_rms_delta_pts: r.text_rms_delta_pts,
            pixel_passed: r.pixel_passed,
            changed_pct: r.changed_pct,
            max_pixel_delta: r.max_pixel_delta,
            render_dpi: r.render_dpi,
        }
    }
}

// --- docops (commands::docops) ---

#[derive(Debug, Deserialize)]
pub struct RedactDocumentParams {
    pub doc_id: String,
    #[serde(default)]
    pub regions: Vec<crate::docops::RedactRegion>,
    #[serde(default)]
    pub apply_annots: bool,
}

// --- save-as (commands::document) ---

#[derive(Debug, Deserialize)]
pub struct SaveDocumentAsParams {
    pub doc_id: String,
    pub path: String,
}

#[cfg(test)]
mod phase2b_param_tests {
    use super::*;

    #[test]
    fn search_document_defaults_case_sensitive_and_whole_word_to_false() {
        let p: SearchDocumentParams =
            serde_json::from_value(serde_json::json!({"doc_id": "d1", "query": "fire"})).unwrap();
        assert!(!p.case_sensitive);
        assert!(!p.whole_word);
    }

    #[test]
    fn search_document_missing_doc_id_is_a_bad_params_error() {
        let err =
            serde_json::from_value::<SearchDocumentParams>(serde_json::json!({"query": "fire"}))
                .unwrap_err();
        assert!(err.to_string().contains("doc_id"));
    }

    #[test]
    fn open_folder_index_requires_root() {
        let err =
            serde_json::from_value::<OpenFolderIndexParams>(serde_json::json!({})).unwrap_err();
        assert!(err.to_string().contains("root"));
    }

    #[test]
    fn search_folder_limit_defaults_to_none() {
        let p: SearchFolderParams =
            serde_json::from_value(serde_json::json!({"root": "/plans", "query": "door"})).unwrap();
        assert_eq!(p.limit, None);
    }

    #[test]
    fn folder_index_status_root_is_optional() {
        let p: FolderIndexStatusParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(p.root, None);
    }

    #[test]
    fn folder_index_status_result_flattens_status_fields_alongside_the_flag() {
        let result = FolderIndexStatusResult {
            status: crate::search::IndexStatus {
                folder_path: "/plans".into(),
                indexed_files: 3,
                indexed_pages: 40,
                state: crate::search::IndexState::Idle,
            },
            matches_requested_root: true,
        };
        let v = serde_json::to_value(&result).unwrap();
        assert_eq!(v["folder_path"], "/plans");
        assert_eq!(v["matches_requested_root"], true);
    }

    #[test]
    fn add_scale_requires_ratio_unit_label_precision() {
        let err = serde_json::from_value::<AddScaleParams>(
            serde_json::json!({"doc_id": "d1", "unit": "m", "label": "1:100", "precision": 2}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("ratio"));
    }

    #[test]
    fn add_scale_applies_to_page_defaults_to_none_meaning_document_default() {
        let p: AddScaleParams = serde_json::from_value(serde_json::json!({
            "doc_id": "d1", "ratio": 0.001, "unit": "m", "label": "1:1000", "precision": 2
        }))
        .unwrap();
        assert_eq!(p.applies_to_page, None);
    }

    #[test]
    fn export_markup_list_requires_explicit_path_unlike_export_markup_schedule() {
        let err = serde_json::from_value::<ExportMarkupListParams>(
            serde_json::json!({"doc_id": "d1", "format": "Csv"}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("path"));
    }

    #[test]
    fn rotate_page_requires_degrees() {
        let err = serde_json::from_value::<RotatePageParams>(
            serde_json::json!({"doc_id": "d1", "page_idx": 0}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("degrees"));
    }

    #[test]
    fn reorder_pages_new_order_is_a_plain_index_vec() {
        let p: ReorderPagesParams = serde_json::from_value(serde_json::json!({
            "doc_id": "d1", "new_order": [2, 0, 1]
        }))
        .unwrap();
        assert_eq!(p.new_order, vec![2, 0, 1]);
    }

    #[test]
    fn compare_pages_uses_path_a_path_b_not_doc_id() {
        let p: ComparePagesParams = serde_json::from_value(serde_json::json!({
            "path_a": "/a.pdf", "path_b": "/b.pdf", "page_a": 0, "page_b": 0
        }))
        .unwrap();
        assert_eq!(p.path_a, "/a.pdf");
        assert_eq!(p.dpi, None);
        assert_eq!(p.pixel_tolerance, None);
    }

    #[test]
    fn compare_pages_summary_omits_the_png_field() {
        let r = crate::compare::PageDiffResult {
            text_char_match: true,
            text_delta_count: 0,
            text_rms_delta_pts: 0.0,
            pixel_passed: true,
            changed_pct: 0.0,
            max_pixel_delta: 0,
            diff_png_b64: "not-actually-tiny-in-real-life".repeat(1000),
            render_dpi: 150.0,
        };
        let v = serde_json::to_value(ComparePagesSummary::from(r)).unwrap();
        assert!(
            v.get("diff_png_b64").is_none(),
            "MCP compare_pages summary must not carry the base64 PNG overlay"
        );
        assert_eq!(v["changed_pct"], 0.0);
    }

    #[test]
    fn redact_document_regions_and_apply_annots_default_to_empty_and_false() {
        let p: RedactDocumentParams =
            serde_json::from_value(serde_json::json!({"doc_id": "d1"})).unwrap();
        assert!(p.regions.is_empty());
        assert!(!p.apply_annots);
    }

    #[test]
    fn save_document_as_requires_path() {
        let err =
            serde_json::from_value::<SaveDocumentAsParams>(serde_json::json!({"doc_id": "d1"}))
                .unwrap_err();
        assert!(err.to_string().contains("path"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PdfPoint;
    use std::path::PathBuf;

    // -------------------------------------------------------------------
    // list_open_documents (Phase 2a)
    // -------------------------------------------------------------------

    fn open_entry(doc_id: &str, path: &str, dirty: bool) -> OpenDocEntry {
        OpenDocEntry {
            doc_id: doc_id.to_string(),
            path: PathBuf::from(path),
            dirty,
        }
    }

    #[test]
    fn list_open_documents_marks_the_active_doc_and_carries_dirty_through() {
        let entries = vec![
            open_entry("d1", "/plans/floor-1.pdf", false),
            open_entry("d2", "/plans/floor-2.pdf", true),
        ];
        let mut page_counts = HashMap::new();
        page_counts.insert("d1".to_string(), 12u32);
        page_counts.insert("d2".to_string(), 3u32);

        let mut out = list_open_documents(entries, Some("d2"), &page_counts);
        out.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].doc_id, "d1");
        assert_eq!(out[0].title, "floor-1.pdf");
        assert_eq!(out[0].page_count, 12);
        assert!(!out[0].is_active);
        assert!(!out[0].dirty);

        assert_eq!(out[1].doc_id, "d2");
        assert_eq!(out[1].title, "floor-2.pdf");
        assert_eq!(out[1].page_count, 3);
        assert!(out[1].is_active, "d2 must be marked active");
        assert!(out[1].dirty);
    }

    #[test]
    fn list_open_documents_no_active_doc_marks_none_active() {
        let entries = vec![open_entry("d1", "/plans/a.pdf", false)];
        let out = list_open_documents(entries, None, &HashMap::new());
        assert_eq!(out.len(), 1);
        assert!(!out[0].is_active);
    }

    #[test]
    fn list_open_documents_missing_page_count_defaults_to_zero_not_a_failure() {
        // A doc registered in MarkupStore but not yet resolvable in the render engine
        // must not fail the whole call - see the doc comment on list_open_documents.
        let entries = vec![open_entry("d1", "/plans/a.pdf", false)];
        let out = list_open_documents(entries, None, &HashMap::new());
        assert_eq!(out[0].page_count, 0);
    }

    #[test]
    fn list_open_documents_empty_when_nothing_open() {
        let out = list_open_documents(vec![], None, &HashMap::new());
        assert!(out.is_empty());
    }

    #[test]
    fn list_open_documents_title_falls_back_to_full_path_when_no_file_name() {
        // A path with no final component (e.g. "/") has no file_name() - must not
        // panic, must fall back to the full (stringified) path rather than an empty
        // title.
        let entries = vec![open_entry("d1", "/", false)];
        let out = list_open_documents(entries, None, &HashMap::new());
        assert_eq!(out[0].title, "/");
    }

    fn store_with_one_markup(annot_flags: i32) -> (MarkupStore, uuid::Uuid) {
        let store = MarkupStore::default();
        store.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let mut m = Markup::new(
            MarkupType::Rectangle,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 0.0, y: 0.0 },
                max: PdfPoint { x: 10.0, y: 10.0 },
            },
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        );
        m.subject = Some("Door schedule".into());
        m.contents = Some("verify fire rating".into());
        m.annot_flags = annot_flags;
        let id = m.id();
        store.add("d1", m).unwrap();
        (store, id)
    }

    #[test]
    fn list_markups_returns_summary_with_lock_flags_surfaced() {
        let (store, id) = store_with_one_markup(0x80); // Locked
        let got = list_markups(
            &store,
            &ListMarkupsParams {
                doc_id: "d1".into(),
                page: None,
                type_filter: vec![],
                status_filter: vec![],
            },
        )
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, id.to_string());
        assert!(
            got[0].locked,
            "locked flag must be visible before a doomed mutation"
        );
        assert!(!got[0].locked_contents);
    }

    #[test]
    fn list_markups_filters_by_page_and_type() {
        let (store, _) = store_with_one_markup(4);
        let by_page = list_markups(
            &store,
            &ListMarkupsParams {
                doc_id: "d1".into(),
                page: Some(1),
                type_filter: vec![],
                status_filter: vec![],
            },
        )
        .unwrap();
        assert!(
            by_page.is_empty(),
            "page 0 markup must not match page filter 1"
        );

        let by_type = list_markups(
            &store,
            &ListMarkupsParams {
                doc_id: "d1".into(),
                page: None,
                type_filter: vec![MarkupType::Highlight],
                status_filter: vec![],
            },
        )
        .unwrap();
        assert!(
            by_type.is_empty(),
            "Rectangle markup must not match Highlight filter"
        );
    }

    #[test]
    fn list_markups_unknown_doc_errors() {
        let store = MarkupStore::default();
        assert!(list_markups(
            &store,
            &ListMarkupsParams {
                doc_id: "nope".into(),
                page: None,
                type_filter: vec![],
                status_filter: vec![],
            }
        )
        .is_err());
    }

    #[test]
    fn read_markup_returns_full_untruncated_envelope() {
        let (store, id) = store_with_one_markup(4);
        let m = read_markup(
            &store,
            &ReadMarkupParams {
                doc_id: "d1".into(),
                markup_id: id.to_string(),
            },
        )
        .unwrap();
        assert_eq!(m.contents, Some("verify fire rating".into()));
    }

    #[test]
    fn read_markup_unknown_id_errors() {
        let (store, _) = store_with_one_markup(4);
        let err = read_markup(
            &store,
            &ReadMarkupParams {
                doc_id: "d1".into(),
                markup_id: uuid::Uuid::new_v4().to_string(),
            },
        )
        .unwrap_err();
        assert!(err.contains("unknown markup id"));
    }

    #[test]
    fn read_markup_bad_uuid_string_errors_cleanly() {
        let (store, _) = store_with_one_markup(4);
        let err = read_markup(
            &store,
            &ReadMarkupParams {
                doc_id: "d1".into(),
                markup_id: "not-a-uuid".into(),
            },
        )
        .unwrap_err();
        assert!(err.contains("bad markup id"));
    }

    #[test]
    fn search_markups_matches_subject_and_contents_case_insensitive_by_default() {
        let (store, _) = store_with_one_markup(4);
        let hits = search_markups(
            &store,
            &SearchMarkupsParams {
                doc_id: "d1".into(),
                query: "FIRE RATING".into(),
                case_sensitive: false,
                scope: "document".into(),
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);

        let miss = search_markups(
            &store,
            &SearchMarkupsParams {
                doc_id: "d1".into(),
                query: "sprinkler".into(),
                case_sensitive: false,
                scope: "document".into(),
            },
        )
        .unwrap();
        assert!(miss.is_empty());
    }

    #[test]
    fn search_markups_case_sensitive_rejects_wrong_case() {
        let (store, _) = store_with_one_markup(4);
        let miss = search_markups(
            &store,
            &SearchMarkupsParams {
                doc_id: "d1".into(),
                query: "FIRE RATING".into(),
                case_sensitive: true,
                scope: "document".into(),
            },
        )
        .unwrap();
        assert!(miss.is_empty());
    }

    #[test]
    fn search_markups_unsupported_scope_returns_clear_error_not_silent_narrowing() {
        let (store, _) = store_with_one_markup(4);
        let err = search_markups(
            &store,
            &SearchMarkupsParams {
                doc_id: "d1".into(),
                query: "fire".into(),
                case_sensitive: false,
                scope: "folder".into(),
            },
        )
        .unwrap_err();
        assert!(err.contains("not yet supported"));
    }

    #[test]
    fn create_markup_adds_to_store_with_mcp_author_and_no_lock_check() {
        let store = MarkupStore::default();
        store.register("d1", PathBuf::from("/tmp/a.pdf"), None);
        let created = create_markup(
            &store,
            CreateMarkupParams {
                doc_id: "d1".into(),
                markup_type: MarkupType::Highlight,
                page: 2,
                geometry: MarkupGeometry::Quads(vec![[
                    PdfPoint { x: 0.0, y: 0.0 },
                    PdfPoint { x: 10.0, y: 0.0 },
                    PdfPoint { x: 0.0, y: 10.0 },
                    PdfPoint { x: 10.0, y: 10.0 },
                ]]),
                appearance: Appearance::default(),
                contents: Some("check clearance".into()),
                subject: None,
                layer: None,
            },
        )
        .unwrap();

        assert_eq!(created.audit.created_by.display_name, "redline-mcp");
        let listed = store.list("d1").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id(), created.id());
    }

    #[test]
    fn update_markup_merges_partial_fields_onto_existing() {
        let (store, id) = store_with_one_markup(4);
        let updated = update_markup(
            &store,
            UpdateMarkupParams {
                doc_id: "d1".into(),
                markup_id: id.to_string(),
                contents: Some("re-verified, OK".into()),
                appearance: None,
                workflow_status: Some(MarkupStatus::Completed),
            },
        )
        .unwrap();

        assert_eq!(updated.contents, Some("re-verified, OK".into()));
        assert_eq!(updated.workflow.status, MarkupStatus::Completed);
        // Untouched field survives the partial merge.
        assert_eq!(updated.subject, Some("Door schedule".into()));
        assert_eq!(updated.audit.revision, 1, "touch() must bump revision");
    }

    #[test]
    fn update_markup_refuses_locked_markup_with_structured_error() {
        let (store, id) = store_with_one_markup(0x80); // Locked
        let err = update_markup(
            &store,
            UpdateMarkupParams {
                doc_id: "d1".into(),
                markup_id: id.to_string(),
                contents: Some("attempted edit".into()),
                appearance: None,
                workflow_status: None,
            },
        )
        .unwrap_err();
        assert!(err.contains("markup_locked"), "got: {err}");
        // Refused, not applied.
        assert_eq!(
            store.list("d1").unwrap()[0].contents,
            Some("verify fire rating".into())
        );
    }

    #[test]
    fn delete_markup_removes_unlocked_markup() {
        let (store, id) = store_with_one_markup(4);
        delete_markup(
            &store,
            &DeleteMarkupParams {
                doc_id: "d1".into(),
                markup_id: id.to_string(),
            },
        )
        .unwrap();
        assert!(store.list("d1").unwrap().is_empty());
    }

    #[test]
    fn delete_markup_refuses_locked_markup() {
        let (store, id) = store_with_one_markup(0x200); // LockedContents
        let err = delete_markup(
            &store,
            &DeleteMarkupParams {
                doc_id: "d1".into(),
                markup_id: id.to_string(),
            },
        )
        .unwrap_err();
        assert!(err.contains("LockedContents"), "got: {err}");
        assert_eq!(store.list("d1").unwrap().len(), 1);
    }

    #[test]
    fn export_markup_schedule_writes_a_csv_named_after_the_source_document() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("floor-plan.pdf");
        std::fs::write(&src, b"%PDF-1.4 fake").unwrap();

        let store = MarkupStore::default();
        store.register("d1", src.clone(), None);
        let m = Markup::new(
            MarkupType::MeasurementCount,
            0,
            MarkupGeometry::Point(PdfPoint { x: 1.0, y: 1.0 }),
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        );
        store.add("d1", m).unwrap();

        let result = export_markup_schedule(
            &store,
            ExportMarkupScheduleParams {
                doc_id: "d1".into(),
                format: crate::commands::takeoff::ExportFormat::Csv,
            },
        )
        .unwrap();

        assert!(result.out_path.contains("floor-plan-markup-schedule-"));
        assert!(result.out_path.ends_with(".csv"));
        assert!(
            std::path::Path::new(&result.out_path).exists(),
            "export must actually write the file"
        );
    }

    #[test]
    fn export_markup_schedule_unknown_doc_errors() {
        let store = MarkupStore::default();
        let err = export_markup_schedule(
            &store,
            ExportMarkupScheduleParams {
                doc_id: "nope".into(),
                format: crate::commands::takeoff::ExportFormat::Csv,
            },
        )
        .unwrap_err();
        assert!(err.contains("unknown doc_id"));
    }
}
