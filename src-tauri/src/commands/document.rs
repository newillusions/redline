//! Tauri commands for document open/close (spec §4).

use anyhow::Context as _;
use std::path::PathBuf;
use tauri::{Manager, State};

use crate::document::known_passwords;
use crate::document::page_ops;
use crate::document::save::{load_markups_from, save_decrypted_copy, save_with_markups};
use crate::document::{new_doc_id, DocumentInfo, ERR_PASSWORD_REQUIRED, ERR_WRONG_PASSWORD};
use crate::markup::Markup;
use crate::render::OpenOutcome;
use crate::AppState;

/// Open a PDF file. Returns a `DocumentInfo` with a fresh `doc_id`.
///
/// `password` is `None` on the first attempt for a given file. If the file is
/// encrypted and no password was given, the remembered known-password list
/// (see `document::known_passwords`) is tried automatically, oldest-attempt
/// cost aside, BEFORE surfacing `ERR_PASSWORD_REQUIRED` to the frontend - this
/// is what lets a previously-unlocked-elsewhere password skip the prompt
/// entirely. If a password (given or auto-tried) doesn't decrypt it, this
/// returns `Err(ERR_WRONG_PASSWORD)` (given) or `Err(ERR_PASSWORD_REQUIRED)`
/// (auto-try exhausted the list) - the frontend re-invokes with the
/// user-entered password on either sentinel. On success, the EFFECTIVE
/// password (whichever one actually worked) is cached in the markup store
/// (session-only, in-memory) so `load_markups` and later saves/reopens can
/// decrypt the same file without re-prompting.
#[tauri::command]
pub async fn open_document(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    path: String,
    password: Option<String>,
) -> Result<DocumentInfo, String> {
    let path = PathBuf::from(&path);

    if !path.exists() {
        return Err(format!("File not found: {}", path.display()));
    }
    if path.extension().and_then(|e| e.to_str()) != Some("pdf") {
        return Err(format!("Not a PDF file: {}", path.display()));
    }

    let doc_id = new_doc_id();
    let outcome = state
        .render
        .open_document(path.clone(), doc_id.clone(), password.clone())
        .await
        .map_err(|e| format!("{:#}", e))?;

    let (page_count, effective_password) = match outcome {
        OpenOutcome::Opened(page_count) => (page_count, password),
        // No password was supplied and the file needs one: try remembered
        // passwords before giving up to the frontend prompt. A password WAS
        // supplied and still failed (WrongPassword) skips straight to the
        // error - we never second-guess an explicit user entry with the
        // known-password list.
        OpenOutcome::PasswordRequired if password.is_none() => {
            match try_known_passwords(&state, &app, &path, &doc_id).await? {
                Some((page_count, pw)) => (page_count, Some(pw)),
                None => return Err(ERR_PASSWORD_REQUIRED.to_string()),
            }
        }
        OpenOutcome::PasswordRequired => return Err(ERR_PASSWORD_REQUIRED.to_string()),
        OpenOutcome::WrongPassword => return Err(ERR_WRONG_PASSWORD.to_string()),
    };

    let was_encrypted = effective_password.is_some();
    state
        .markups
        .register(&doc_id, path.clone(), effective_password);

    Ok(DocumentInfo {
        doc_id,
        path: path.to_string_lossy().into_owned(),
        page_count,
        was_encrypted,
    })
}

/// Try each remembered password (oldest-first list from `known_passwords`,
/// already most-recent-first) against `path` under `doc_id`, stopping at the
/// first one that opens successfully. Returns `Ok(None)` (not an error) if
/// the list is empty or none of them work - the caller falls back to
/// prompting. A failed attempt leaves no render-engine state registered (see
/// `RenderEngine::open_document` doc comment), so retrying under the same
/// `doc_id` is safe.
async fn try_known_passwords(
    state: &State<'_, AppState>,
    app: &tauri::AppHandle,
    path: &std::path::Path,
    doc_id: &str,
) -> Result<Option<(u32, String)>, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;
    let candidates = {
        let data_dir = data_dir.clone();
        tokio::task::spawn_blocking(move || known_passwords::list_known_passwords(&data_dir))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("read known-password store: {e}"))?
    };

    for candidate in candidates {
        let outcome = state
            .render
            .open_document(
                path.to_path_buf(),
                doc_id.to_string(),
                Some(candidate.clone()),
            )
            .await
            .map_err(|e| format!("{:#}", e))?;
        if let OpenOutcome::Opened(page_count) = outcome {
            return Ok(Some((page_count, candidate)));
        }
    }
    Ok(None)
}

/// Save an unprotected (no open password) copy of the currently-open
/// encrypted document to `dest_path`. Existing content/annotations already
/// in the file are preserved as-is; this does not flush unsaved in-memory
/// markup edits (that's `save_document`/`save_document_as` - a different,
/// still-refused-on-encrypted operation, see `document::save::save_with_markups`).
/// Errors if the document was not opened with a password.
#[tauri::command]
pub async fn save_unprotected_copy(
    state: State<'_, AppState>,
    doc_id: String,
    dest_path: String,
) -> Result<(), String> {
    let src = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let password = state.markups.password(&doc_id).ok_or_else(|| {
        "This document is not password-protected - there is no protection to remove.".to_string()
    })?;
    let dest = PathBuf::from(dest_path);

    tokio::task::spawn_blocking(move || save_decrypted_copy(&src, &dest, &password))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// Remember `password` in the obfuscated known-password store (see
/// `document::known_passwords` for the storage/threat-model contract), for
/// future `open_document` calls to auto-try before prompting. Called after a
/// successful MANUAL password entry, only when the user opts in.
#[tauri::command]
pub async fn remember_password(app: tauri::AppHandle, password: String) -> Result<(), String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;

    tokio::task::spawn_blocking(move || known_passwords::remember_password(&data_dir, &password))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e}"))
}

/// Close an open document and release its resources.
/// Render close happens first; store entry is removed only after it succeeds.
#[tauri::command]
pub async fn close_document(state: State<'_, AppState>, doc_id: String) -> Result<(), String> {
    state
        .render
        .close_document(doc_id.clone())
        .await
        .map_err(|e| format!("{:#}", e))?;
    state.markups.remove(&doc_id);
    Ok(())
}

/// Add a markup to the open document's in-memory set (not yet saved to the file).
#[tauri::command]
pub async fn add_markup(
    state: State<'_, AppState>,
    doc_id: String,
    markup: Markup,
) -> Result<(), String> {
    state.markups.add(&doc_id, markup)
}

/// Replace an existing markup (move/resize/edit). Errors if the id is absent.
#[tauri::command]
pub async fn update_markup(
    state: State<'_, AppState>,
    doc_id: String,
    markup: Markup,
) -> Result<(), String> {
    state.markups.update(&doc_id, markup)
}

/// Delete a markup by id (string UUID from the frontend).
#[tauri::command]
pub async fn delete_markup(
    state: State<'_, AppState>,
    doc_id: String,
    markup_id: String,
) -> Result<(), String> {
    let id = uuid::Uuid::parse_str(&markup_id).map_err(|e| format!("bad markup id: {e}"))?;
    state.markups.delete(&doc_id, id)
}

/// Return the persisted app user identity, generating it on first run.
#[tauri::command]
pub fn get_user_identity(app: tauri::AppHandle) -> Result<crate::identity::Identity, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("config dir: {e}"))?;
    crate::identity::load_or_create(&dir)
}

/// List the open document's in-memory markups.
#[tauri::command]
pub async fn list_markups(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<Vec<Markup>, String> {
    state.markups.list(&doc_id)
}

/// Read existing annotations from the PDF into the store (call after open; lopdf runs
/// in a blocking task). Merges beneath unsaved in-memory markups; store wins on id.
#[tauri::command]
pub async fn load_markups(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<Vec<Markup>, String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;

    // Fast path: return the cached parse if the file is unchanged since the last load —
    // skips the ~tens-of-seconds lopdf parse on reopen of a large, unmodified file.
    if let Some(cached) = state.markups.check_mtime_cache(&path) {
        return state.markups.seed_loaded(&doc_id, cached);
    }

    // Slow path: full lopdf parse (blocking; tens of seconds on large files).
    let path_for_parse = path.clone();
    let password = state.markups.password(&doc_id);
    let loaded = tokio::task::spawn_blocking(move || {
        load_markups_from(&path_for_parse, password.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("{e:#}"))?;
    // Populate the cache so the next reopen of this unmodified file returns immediately.
    state.markups.cache_loaded(path, loaded.clone());
    state.markups.seed_loaded(&doc_id, loaded)
}

/// Save the in-memory markups into the open file (atomic in-place).
#[tauri::command]
pub async fn save_document(state: State<'_, AppState>, doc_id: String) -> Result<(), String> {
    save_impl(state, doc_id, None).await
}

/// Save-As: write to `new_path` and switch the open document to it (same doc_id).
#[tauri::command]
pub async fn save_document_as(
    state: State<'_, AppState>,
    doc_id: String,
    new_path: String,
) -> Result<(), String> {
    save_impl(state, doc_id, Some(PathBuf::from(new_path))).await
}

/// Shared save flow entry: acquires the per-doc save-in-flight guard, then runs
/// the actual save. The guard is released on EVERY exit path - `save_inner`
/// returns its Result here and `end_save` runs unconditionally before returning.
/// Two concurrent saves on the same doc_id would otherwise write the same
/// staged path and interleave the close/rename/reopen sequence (corruption).
async fn save_impl(
    state: State<'_, AppState>,
    doc_id: String,
    new_path: Option<PathBuf>,
) -> Result<(), String> {
    state.markups.begin_save(&doc_id)?;
    let result = save_inner(&state, &doc_id, new_path).await;
    state.markups.end_save(&doc_id);
    result
}

/// Actual save flow. Order matters (see save_with_markups doc contract):
/// stage the rewritten file to a sibling path FIRST (source open in the render
/// engine is fine - reads only), THEN close the render doc (Windows cannot
/// rename over an open file), swap, reopen under the SAME doc_id.
async fn save_inner(
    state: &State<'_, AppState>,
    doc_id: &str,
    new_path: Option<PathBuf>,
) -> Result<(), String> {
    let src = state
        .markups
        .path(doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let dest = new_path.clone().unwrap_or_else(|| src.clone());
    let password = state.markups.password(doc_id);

    // Load-before-save guard: never strip annotations that were never imported.
    // is_managed treats every /RLType annot as ours, so saving an un-loaded doc
    // would replace pre-existing redline annotations with only the new ones.
    if !state.markups.is_loaded(doc_id) {
        let p = src.clone();
        let pw = password.clone();
        let loaded = tokio::task::spawn_blocking(move || load_markups_from(&p, pw.as_deref()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
        state.markups.seed_loaded(doc_id, loaded)?;
    }
    let markups = state.markups.list(doc_id)?;

    // 1. Stage the complete rewritten file next to the destination. Encrypted source
    //    documents are refused inside save_with_markups (see its doc comment) - lopdf
    //    has no re-encrypt-on-save path, so saving would silently strip the PDF's
    //    password protection.
    let staged = dest.with_extension("pdf.redline-staged");
    {
        let src = src.clone();
        let staged = staged.clone();
        tokio::task::spawn_blocking(move || save_with_markups(&src, &staged, &markups))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
    }

    // 2. Release the file from the render engine, swap, reopen under the same doc_id.
    state
        .render
        .close_document(doc_id.to_string())
        .await
        .map_err(|e| format!("{e:#}"))?;
    if let Err(e) = std::fs::rename(&staged, &dest) {
        let _ = std::fs::remove_file(&staged);
        // Try to restore the render doc on the ORIGINAL path before failing.
        let _ = state
            .render
            .open_document(src.clone(), doc_id.to_string(), password.clone())
            .await;
        return Err(format!("swap failed: {e}"));
    }
    state
        .render
        .open_document(dest.clone(), doc_id.to_string(), password)
        .await
        .and_then(|outcome| outcome.into_page_count())
        .map_err(|e| format!("reopen after save: {e:#}"))?;
    // The save changed the file's content + mtime: drop the stale cache entry so the next
    // load_markups re-parses rather than returning the pre-save snapshot.
    state.markups.invalidate_cache(&dest);
    if new_path.is_some() {
        state.markups.set_path(doc_id, dest)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Page operation commands (M4 S1)
//
// All page ops follow the same pattern:
// 1. Load the PDF via lopdf (spawn_blocking).
// 2. Apply the page operation.
// 3. Save atomically (temp file + rename over original).
// 4. Close + reopen the render doc so tiles refresh.
// 5. Invalidate the markup cache.
// ---------------------------------------------------------------------------

/// Core edit-and-save logic behind [`apply_page_edit`], extracted so it's directly
/// testable (file round-trip, tempfile + lopdf) without a Tauri `State`/render engine.
///
/// Markups are written into `doc` BEFORE `op` runs, not after. The previous order
/// (`op` then `write_markups`) had two bugs, both traced back to the same ordering
/// mistake:
/// - `flatten_document`/`optimize_document` bake/compress annotations via `op`, then
///   `write_markups` immediately re-added fresh (uncompressed) annotation objects on
///   top, silently reversing the whole operation - "Flatten"/"Optimize" appeared to do
///   nothing because they were undone one line later.
/// - If a markup had been moved in-session but not yet saved to the file (`store.flush()`
///   only drains to the in-memory Rust mirror, it does not write the PDF), `op` still
///   read the STALE on-disk position via `Document::load`. For `flatten_document` this
///   baked a "background" appearance ghost at the OLD position into page content
///   (permanent, since flatten literally paints it into the content stream), while the
///   subsequent `write_markups` added a second, correctly-positioned live annotation -
///   two visible copies of the same markup, the old one an orphaned artifact that
///   `doc.prune_objects()` can never remove because it's baked content, not an
///   annotation object.
///
/// Writing markups FIRST fixes both: `op` (flatten/optimize/redact/rotate/delete/
/// reorder/insert) now always operates on the CURRENT markup state, and nothing runs
/// afterward to undo it.
fn apply_edit_and_save(
    src: &std::path::Path,
    markups: &[Markup],
    op: impl FnOnce(&mut lopdf::Document) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut doc =
        lopdf::Document::load(src).with_context(|| format!("load {}", src.display()))?;
    // Page ops rewrite the file the same way save_with_markups does (full lopdf
    // load -> save), which would silently strip encryption on an encrypted PDF
    // (lopdf has no re-encrypt-on-save path). Refuse rather than corrupt/de-protect.
    if doc.is_encrypted() {
        anyhow::bail!(
            "Page operations (rotate/delete/reorder/insert) on a password-protected \
             PDF are not supported yet - saving would strip its password protection."
        );
    }
    // Bring annotations up to the CURRENT markup state first, then apply the edit -
    // see the doc comment above for why this order matters.
    crate::document::annots::write_markups(&mut doc, markups)?;
    op(&mut doc)?;
    // Atomic write: temp + rename.
    let dir = src.parent().context("no parent dir")?;
    let tmp = dir.join(format!(
        ".redline-tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> anyhow::Result<()> {
        let f = doc
            .save(&tmp)
            .with_context(|| format!("write {}", tmp.display()))?;
        f.sync_all().context("fsync temp")?;
        std::fs::rename(&tmp, src).context("atomic rename")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Apply a document edit (closure) to the PDF on disk, then reload the render engine.
/// Shared implementation for the page ops (rotate/delete/reorder/insert) and for any
/// other lopdf-level edit that must restructure the file and refresh tiles (e.g. the
/// takeoff /Measure dict write, or a docops flatten/optimize/redact). Markups are
/// written into the loaded doc before `op` runs - see `apply_edit_and_save`.
pub(crate) async fn apply_page_edit(
    state: &State<'_, AppState>,
    doc_id: &str,
    op: impl FnOnce(&mut lopdf::Document) -> anyhow::Result<()> + Send + 'static,
) -> Result<(), String> {
    let src = state
        .markups
        .path(doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let password = state.markups.password(doc_id);

    // Load-before-op: ensure the markup store is seeded so save doesn't drop existing
    // redline annotations that haven't been loaded into memory yet.
    if !state.markups.is_loaded(doc_id) {
        let p = src.clone();
        let pw = password.clone();
        let loaded = tokio::task::spawn_blocking(move || load_markups_from(&p, pw.as_deref()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
        state.markups.seed_loaded(doc_id, loaded)?;
    }
    let markups = state.markups.list(doc_id)?;

    // Apply the op + save in a blocking task.
    let src2 = src.clone();
    tokio::task::spawn_blocking(move || apply_edit_and_save(&src2, &markups, op))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;

    // Reload the render engine so new page geometry is reflected in tile renders.
    state
        .render
        .close_document(doc_id.to_string())
        .await
        .map_err(|e| format!("{e:#}"))?;
    state
        .render
        .open_document(src.clone(), doc_id.to_string(), password)
        .await
        .and_then(|outcome| outcome.into_page_count())
        .map_err(|e| format!("reopen after page op: {e:#}"))?;
    state.markups.invalidate_cache(&src);
    Ok(())
}

/// Rotate a page by `degrees` (multiple of 90, incremental/cumulative).
#[tauri::command]
pub async fn rotate_page(
    state: State<'_, AppState>,
    doc_id: String,
    page_idx: u32,
    degrees: i32,
) -> Result<(), String> {
    apply_page_edit(&state, &doc_id, move |doc| {
        page_ops::rotate_page(doc, page_idx, degrees)
    })
    .await
}

/// Delete a page (0-based index). Errors if the document has only one page.
#[tauri::command]
pub async fn delete_page(
    state: State<'_, AppState>,
    doc_id: String,
    page_idx: u32,
) -> Result<(), String> {
    apply_page_edit(&state, &doc_id, move |doc| {
        page_ops::delete_page(doc, page_idx)
    })
    .await
}

/// Reorder pages. `new_order` is a permutation of `0..page_count` (0-based).
#[tauri::command]
pub async fn reorder_pages(
    state: State<'_, AppState>,
    doc_id: String,
    new_order: Vec<u32>,
) -> Result<(), String> {
    apply_page_edit(&state, &doc_id, move |doc| {
        page_ops::reorder_pages(doc, new_order)
    })
    .await
}

/// Insert a blank page at position `at` (0-based; `at == page_count` appends).
#[tauri::command]
pub async fn insert_blank_page(
    state: State<'_, AppState>,
    doc_id: String,
    at: u32,
    width: f32,
    height: f32,
) -> Result<(), String> {
    apply_page_edit(&state, &doc_id, move |doc| {
        page_ops::insert_blank_page(doc, at, width, height)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::annots::tests::{one_page_doc, redline_markup};
    use crate::docops::{flatten_annotations, optimize_in_place};
    use crate::geometry::PdfPoint;
    use crate::markup::{Appearance, Markup, MarkupGeometry, MarkupType, UserRef};

    /// A markup whose appearance content stream is large enough to cross lopdf's
    /// compression savings threshold (`Stream::compress()` only compresses when it
    /// saves >= 19 bytes - the 3-point `redline_markup` fixture is too small).
    fn long_ink_markup(page: u32) -> Markup {
        let stroke: Vec<PdfPoint> = (0..200)
            .map(|i| PdfPoint { x: (i % 20) as f64, y: (i / 20) as f64 })
            .collect();
        Markup::new(
            MarkupType::Ink,
            page,
            MarkupGeometry::Ink(vec![stroke]),
            Appearance::default(),
            UserRef { user_id: uuid::Uuid::new_v4(), display_name: "Alice".into() },
        )
    }

    #[test]
    fn flatten_via_apply_edit_and_save_actually_sticks() {
        // Regression test for bug #1: flatten appeared to do nothing because
        // apply_page_edit called write_markups AFTER op(), re-adding the same
        // annotations flatten had just baked into content and removed.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flatten.pdf");
        let (mut doc, _page_id) = one_page_doc();
        doc.save(&path).unwrap();

        let m = redline_markup(0);
        apply_edit_and_save(&path, std::slice::from_ref(&m), flatten_annotations)
            .unwrap();

        let reopened = lopdf::Document::load(&path).unwrap();
        let page_id = *reopened.get_pages().values().next().unwrap();
        let page = reopened.get_dictionary(page_id).unwrap();
        assert!(
            page.get(b"Annots").is_err(),
            "flattened annotation must NOT be resurrected as a live annotation"
        );
        // The bake must actually be present: /Contents grew to include the overlay.
        let contents = page.get(b"Contents").unwrap();
        assert!(
            matches!(contents, lopdf::Object::Array(a) if a.len() >= 2),
            "flatten must add its overlay content stream"
        );
    }

    #[test]
    fn flatten_via_apply_edit_and_save_bakes_the_current_not_stale_position() {
        // Regression test for bug #3: if a markup moved in-session (Rust store
        // updated) but the FILE on disk still had the old position (flush() only
        // drains to the Rust mirror, it never writes the PDF), flatten must bake
        // the CURRENT (moved) position, not the stale on-disk one - otherwise a
        // "background" ghost is baked at the old spot while a live annotation
        // reappears at the new one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flatten-moved.pdf");
        let (mut doc, _page_id) = one_page_doc();
        doc.save(&path).unwrap(); // file has NO annotation on disk yet

        // Simulate: markup was created+moved in-session (Rust store has it at its
        // final position) but the file was never saved with it.
        let mut m = redline_markup(0);
        if let MarkupGeometry::Polyline(pts) = &mut m.geometry {
            for p in pts.iter_mut() {
                p.x += 500.0;
                p.y += 500.0;
            }
        } else {
            panic!("expected polyline geometry from redline_markup fixture");
        }

        apply_edit_and_save(&path, std::slice::from_ref(&m), flatten_annotations)
            .unwrap();

        let reopened = lopdf::Document::load(&path).unwrap();
        let page_id = *reopened.get_pages().values().next().unwrap();
        // No live annotation must remain (fully flattened, no duplicate at either position).
        let page = reopened.get_dictionary(page_id).unwrap();
        assert!(page.get(b"Annots").is_err(), "must be fully flattened, no live duplicate");
        // Exactly one baked overlay content stream (not two - one per position).
        let contents = page.get(b"Contents").unwrap().as_array().unwrap();
        assert_eq!(
            contents.len(),
            2,
            "must bake exactly one overlay (original content + one overlay), not a stale-position ghost plus a fresh one"
        );
    }

    #[test]
    fn optimize_via_apply_edit_and_save_compresses_the_freshly_written_annotation() {
        // Regression test for bug #2: optimize appeared to have no effect because
        // write_markups ran AFTER optimize_in_place's compress() step, adding fresh
        // uncompressed appearance streams right after compression had already run.
        // Writing markups first means compress() (which now runs via `op` last) sees
        // and compresses the annotation appearance streams too.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("optimize.pdf");
        let (mut doc, _page_id) = one_page_doc();
        doc.save(&path).unwrap();

        // A long Ink stroke, not the small 3-point Cloud fixture: its appearance content
        // stream must be large enough to cross lopdf's compression savings threshold
        // (Stream::compress() only compresses when it saves >= 19 bytes - see
        // docops::tests::compressible_stream_content for the same constraint).
        let m = long_ink_markup(0);
        apply_edit_and_save(&path, std::slice::from_ref(&m), |doc| optimize_in_place(doc, 2))
            .unwrap();

        let reopened = lopdf::Document::load(&path).unwrap();
        let compressed_stream_exists = reopened.objects.values().any(|o| {
            matches!(o, lopdf::Object::Stream(s) if s.is_compressed())
        });
        assert!(
            compressed_stream_exists,
            "level-2 optimize must compress at least one stream, including annotation appearances written just before it"
        );
    }

    #[test]
    fn rotate_via_apply_edit_and_save_still_preserves_markups() {
        // Regression guard: reordering write_markups-before-op must not break the
        // existing page-restructuring ops, which rely on markups following the op.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rotate.pdf");
        let (mut doc, _page_id) = one_page_doc();
        doc.save(&path).unwrap();

        let m = redline_markup(0);
        apply_edit_and_save(&path, std::slice::from_ref(&m), |doc| {
            page_ops::rotate_page(doc, 0, 90)
        })
        .unwrap();

        let reopened = lopdf::Document::load(&path).unwrap();
        let page_id = *reopened.get_pages().values().next().unwrap();
        let page = reopened.get_dictionary(page_id).unwrap();
        assert_eq!(page.get(b"Rotate").unwrap().as_i64().unwrap(), 90);
        let markups = crate::document::annots::read_markups(&reopened).unwrap();
        assert_eq!(markups.len(), 1, "markup must survive a page rotation");
    }

    #[test]
    fn encrypted_source_is_refused() {
        use crate::document::annots::tests::encrypted_one_page_doc;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted.pdf");
        let mut doc = encrypted_one_page_doc("redline-pw", "owner-pw");
        doc.save(&path).unwrap();

        let err = apply_edit_and_save(&path, &[], |doc| optimize_in_place(doc, 1));
        assert!(err.is_err(), "encrypted PDFs must be refused, not silently de-protected");
    }
}
