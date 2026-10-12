//! Tauri commands for document open/close (spec §4).

use anyhow::Context as _;
use std::path::PathBuf;
use tauri::{Manager, State};

use crate::document::known_passwords;
use crate::document::page_ops;
use crate::document::save::{load_markups_from, save_decrypted_copy, save_with_markups_scaled};
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

    // Info-level so an MCP caller (or a human tailing the log) can discover the
    // doc_id for a document opened some other way (e.g. a GUI-driven open, or the
    // REDLINE_OPEN_PDF auto-open). Originally added 2026-09-02 because there was no
    // other way to learn a doc_id an MCP client didn't itself open - Phase 2a's
    // `list_open_documents` MCP tool (`rpc::tools::list_open_documents`) is now the
    // structured way to enumerate every open doc_id/path, but this log line stays as a
    // lower-friction check when just tailing the log (e.g. while `redline` is running
    // without an MCP client attached at all).
    log::info!("document opened: doc_id={doc_id} path={}", path.display());

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
    // Scales live in the sidecar; forget the in-memory copy so a reopen reloads it.
    state.scales.lock().unwrap().clear(&doc_id);
    // Drop a stale `active_doc` pointing at a doc that no longer exists (MCP server
    // design, Phase 2a) - relevant when this close was driven by the MCP
    // `close_document` tool rather than a GUI tab close, since the frontend's own
    // `$effect` sync (see `set_active_document`'s doc comment) has no channel to learn
    // about a backend-driven close and pick a new active tab itself.
    let mut active = state.active_doc.lock().unwrap();
    if active.as_deref() == Some(doc_id.as_str()) {
        *active = None;
    }
    Ok(())
}

/// Record the frontend's currently-focused document tab (MCP server design, Phase 2a -
/// `get_active_document`/`list_open_documents`). There is no backend concept of "the
/// active tab" otherwise - `DocTabStore.activeDocId` (`src/lib/doc-tabs.svelte.ts`) is
/// Svelte-only reactive state. `App.svelte` mirrors it here with a single `$effect`
/// watching `tabStore.activeDocId`, so every path that changes it (open, switch tab,
/// close tab) stays in sync without a call at each of those call sites individually.
/// `doc_id: None` means no document is open/focused (last tab closed).
#[tauri::command]
pub fn set_active_document(state: State<'_, AppState>, doc_id: Option<String>) -> Result<(), String> {
    *state.active_doc.lock().unwrap() = doc_id;
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

    // Snapshot the file BEFORE reading it (first load only): stamp, private copy and the
    // object ids parsed below then all describe the same bytes.
    let snapshot = state.markups.begin_first_load(&doc_id);

    // Fast path: return the cached parse if the file is unchanged since the last load —
    // skips the ~tens-of-seconds lopdf parse on reopen of a large, unmodified file.
    if let Some(cached) = state.markups.check_mtime_cache(&path) {
        return state.markups.seed_loaded_with(&doc_id, cached, snapshot);
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
    state.markups.seed_loaded_with(&doc_id, loaded, snapshot)
}

/// Save the in-memory markups into the open file (atomic in-place).
#[tauri::command]
pub async fn save_document(state: State<'_, AppState>, doc_id: String) -> Result<(), String> {
    save_impl(state, doc_id, None, false).await
}

/// Save-As: write to `new_path` and switch the open document to it (same doc_id).
#[tauri::command]
pub async fn save_document_as(
    state: State<'_, AppState>,
    doc_id: String,
    new_path: String,
) -> Result<(), String> {
    save_impl(state, doc_id, Some(PathBuf::from(new_path)), false).await
}

/// Save As (rebuild): recovery for a PDF another program changed since redline loaded it
/// (`ERR_DISK_CHANGED`). Writes the in-memory mark-ups to a NEW file built from the bytes
/// redline loaded (a private copy kept at open / last write), never from the changed file;
/// every foreign mark-up is rebuilt from the model. The changed file is left untouched, and
/// the document switches to the new file like any Save As.
#[tauri::command]
pub async fn save_document_as_rebuild(
    state: State<'_, AppState>,
    doc_id: String,
    new_path: String,
) -> Result<(), String> {
    save_impl(state, doc_id, Some(PathBuf::from(new_path)), true).await
}

/// True if redline holds a private copy of the file as it was loaded, i.e. Save As (rebuild)
/// can run (no copy for a file over the size cap, or when the copy could not be made).
#[tauri::command]
pub fn can_rebuild_from_original(state: State<'_, AppState>, doc_id: String) -> bool {
    state.markups.original_copy(&doc_id).is_some_and(|p| p.exists())
}

/// Reload and keep my mark-ups: recovery for a PDF another program changed since redline
/// loaded it. Re-reads the changed file, then re-applies the user's unsaved edits where
/// their targets still exist; the report names any that could not be applied. Nothing is
/// written to disk. The caller re-reads the markups (`list_markups`) and refreshes the view.
#[tauri::command]
pub async fn reload_keeping_markups(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<crate::document::recover::ReloadReport, String> {
    state.markups.begin_save(&doc_id)?;
    let result = reload_keeping_inner(&state, &doc_id).await;
    state.markups.end_save(&doc_id);
    result
}

async fn reload_keeping_inner(
    state: &State<'_, AppState>,
    doc_id: &str,
) -> Result<crate::document::recover::ReloadReport, String> {
    let path = state
        .markups
        .path(doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    if !state.markups.is_loaded(doc_id) {
        return Err("This document's mark-ups were never loaded, so there is nothing to keep. \
                    Close and reopen it."
            .to_string());
    }
    let password = state.markups.password(doc_id);

    // Transactional: the snapshot (stamp + private copy), the parsed markups and the reopened
    // render all live in locals until every step has succeeded. Any failure returns here with
    // the old stamp, copy and markups untouched, so the stale-file refusal still applies.
    let txn = state.markups.begin_reload(doc_id)?;
    let (p, pw) = (path.clone(), password.clone());
    let loaded = tokio::task::spawn_blocking(move || load_markups_from(&p, pw.as_deref()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;

    // The render engine still shows the old file.
    state
        .render
        .close_document(doc_id.to_string())
        .await
        .map_err(|e| format!("{e:#}"))?;
    let page_count = state
        .render
        .open_document(path.clone(), doc_id.to_string(), password)
        .await
        .and_then(|outcome| outcome.into_page_count())
        .map_err(|e| format!("reopen after reload: {e:#}"))?;

    state.markups.commit_reload(doc_id, txn, loaded, page_count)
}

/// True if `a` and `b` name the same file, however spelled (case-insensitive file systems,
/// symlinked folders). A path that does not exist yet is compared through its parent.
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    let canon = |p: &std::path::Path| {
        std::fs::canonicalize(p).ok().or_else(|| {
            let parent = std::fs::canonicalize(p.parent()?).ok()?;
            Some(parent.join(p.file_name()?))
        })
    };
    a == b || matches!((canon(a), canon(b)), (Some(x), Some(y)) if x == y)
}

/// Refuse any write that would use the loaded markups' object ids (save, Save As, every page
/// op, extract) once the file on disk is no longer the one redline loaded them from. Skipped
/// for a document that was not loaded (its markups are read fresh from the current file).
/// A refusal never touches the baseline: only a verified write re-baselines it.
fn ensure_source_unchanged(
    store: &crate::document::store::MarkupStore,
    doc_id: &str,
    was_loaded: bool,
) -> Result<(), String> {
    if was_loaded {
        store.check_disk_unchanged(doc_id)
    } else {
        Ok(())
    }
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
    rebuild: bool,
) -> Result<(), String> {
    state.markups.begin_save(&doc_id)?;
    let result = save_inner(&state, &doc_id, new_path, rebuild).await;
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
    rebuild: bool,
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
    let was_loaded = state.markups.is_loaded(doc_id);
    if !was_loaded {
        let snapshot = state.markups.begin_first_load(doc_id);
        let p = src.clone();
        let pw = password.clone();
        let loaded = tokio::task::spawn_blocking(move || load_markups_from(&p, pw.as_deref()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
        state.markups.seed_loaded_with(doc_id, loaded, snapshot)?;
    }
    let markups = state.markups.list(doc_id)?;
    let removed = state.markups.removed_sources(doc_id);

    // Annotation matching and preserved originals use the loaded file's object ids: refuse
    // any write (in place or Save As, which reads the same source) once another program has
    // rewritten the file since. Only a reload, or a rebuild from redline's own copy of the
    // loaded bytes (below), is safe.
    let rebuild = rebuild && was_loaded;
    let (markups, removed, write_src) = if rebuild {
        if same_file(&dest, &src) {
            return Err("Save As (rebuild) writes a new file: choose a different name or \
                        folder, not the changed file."
                .to_string());
        }
        let copy = state.markups.original_copy(doc_id).ok_or_else(|| {
            "redline has no private copy of the file as it was loaded (it was too large to \
             keep, or could not be copied), so it cannot rebuild from it. Use \
             \"reload and keep my mark-ups\" instead."
                .to_string()
        })?;
        let (markups, removed) = crate::document::recover::rebuild_inputs(&markups, &removed);
        (markups, removed, copy)
    } else {
        ensure_source_unchanged(&state.markups, doc_id, was_loaded)?;
        (markups, removed, src.clone())
    };

    // 1. Stage the complete rewritten file next to the destination. Encrypted source
    //    documents are refused inside save_with_markups (see its doc comment) - lopdf
    //    has no re-encrypt-on-save path, so saving would silently strip the PDF's
    //    password protection.
    let staged = dest.with_extension("pdf.redline-staged");
    // Legend tables are computed from quantities under the document's current scales (the
    // recompute the exports run first), so load them like an export does.
    crate::commands::takeoff::ensure_scales_loaded(state, doc_id);
    let scales = state.scales.lock().unwrap().list(doc_id).to_vec();
    {
        let staged = staged.clone();
        tokio::task::spawn_blocking(move || {
            save_with_markups_scaled(&write_src, &staged, &markups, &removed, &scales)
        })
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
    // The current markup state is now on disk - clear dirty (MCP server design, Phase
    // 2a `close_document`/`list_open_documents`; see `DocEntry::dirty`'s doc comment).
    state.markups.clear_dirty(doc_id);
    if rebuild {
        state.markups.forget_source(doc_id);
    }
    state.markups.rebaseline_after_write(doc_id);
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
#[cfg(test)]
fn apply_edit_and_save(
    src: &std::path::Path,
    markups: &[Markup],
    removed: &std::collections::HashSet<crate::markup::SourceRef>,
    op: impl FnOnce(&mut lopdf::Document) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    apply_edit_and_save_scaled(src, markups, removed, &[], op)
}

/// [`apply_edit_and_save`] with the document's scales, so legend tables are computed from
/// quantities recomputed under them (see `annots::write_markups_scaled`).
pub(crate) fn apply_edit_and_save_scaled(
    src: &std::path::Path,
    markups: &[Markup],
    removed: &std::collections::HashSet<crate::markup::SourceRef>,
    scales: &[crate::takeoff::ScaleRecord],
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
    crate::document::annots::write_markups_scaled(&mut doc, markups, removed, scales)?;
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
    let was_loaded = state.markups.is_loaded(doc_id);
    if !was_loaded {
        let snapshot = state.markups.begin_first_load(doc_id);
        let p = src.clone();
        let pw = password.clone();
        let loaded = tokio::task::spawn_blocking(move || load_markups_from(&p, pw.as_deref()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
        state.markups.seed_loaded_with(doc_id, loaded, snapshot)?;
    }
    // The write below re-baselines the disk stamp (`invalidate_cache`), so verify it first:
    // a page op over a file another program changed would write stale object ids.
    ensure_source_unchanged(&state.markups, doc_id, was_loaded)?;
    let markups = state.markups.list(doc_id)?;
    let removed = state.markups.removed_sources(doc_id);
    crate::commands::takeoff::ensure_scales_loaded(state, doc_id);
    let scales = state.scales.lock().unwrap().list(doc_id).to_vec();

    // Apply the op + save in a blocking task.
    let src2 = src.clone();
    tokio::task::spawn_blocking(move || {
        apply_edit_and_save_scaled(&src2, &markups, &removed, &scales, op)
    })
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
    // Every caller of apply_page_edit (rotate/delete/reorder/insert, and - via
    // commands::docops - flatten/optimize/redact) writes the current markup state to
    // disk exactly like a save does (apply_edit_and_save calls write_markups before
    // `op` runs) - clear dirty here too, not just in save_inner (MCP server design,
    // Phase 2a; see DocEntry::dirty's doc comment).
    state.markups.clear_dirty(doc_id);
    state.markups.rebaseline_after_write(doc_id);
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

// ---------------------------------------------------------------------------
// Extract pages into a NEW file (owner request 2026-09-15, PR-C)
//
// Deliberately NOT built on apply_page_edit: every other page op (rotate/delete/
// reorder/insert) rewrites the SAME file the open document points at, then closes and
// reopens the render engine under the same doc_id so tiles refresh. Extraction is a
// Save-As of a SUBSET, not an edit of the open document - the source file and the open
// render session must be completely untouched; only a brand new file at `dest_path` is
// written. Shares the same load -> write_markups -> op -> atomic-save skeleton as
// apply_edit_and_save (see that function's doc comment for why markups are written
// BEFORE the page op runs), just targeting a different destination and skipping the
// render-engine swap entirely.
// ---------------------------------------------------------------------------

/// Core logic behind [`extract_pages`], extracted so it's directly testable (file
/// round-trip, tempfile + lopdf) the same way [`apply_edit_and_save`] is.
#[cfg(test)]
fn extract_pages_to_new_file(
    src: &std::path::Path,
    dest: &std::path::Path,
    markups: &[Markup],
    removed: &std::collections::HashSet<crate::markup::SourceRef>,
    page_indices: &[u32],
) -> anyhow::Result<()> {
    extract_pages_to_new_file_scaled(src, dest, markups, removed, &[], page_indices)
}

/// [`extract_pages_to_new_file`] with the document's scales (legend tables, as for a save).
pub(crate) fn extract_pages_to_new_file_scaled(
    src: &std::path::Path,
    dest: &std::path::Path,
    markups: &[Markup],
    removed: &std::collections::HashSet<crate::markup::SourceRef>,
    scales: &[crate::takeoff::ScaleRecord],
    page_indices: &[u32],
) -> anyhow::Result<()> {
    let mut doc =
        lopdf::Document::load(src).with_context(|| format!("load {}", src.display()))?;
    if doc.is_encrypted() {
        anyhow::bail!(
            "Extracting pages from a password-protected PDF is not supported yet - saving \
             would strip its password protection."
        );
    }
    // Bring annotations up to the CURRENT markup state first (matches every other page
    // op - see apply_edit_and_save's doc comment), so extracted pages carry whatever's
    // actually on screen, not a stale on-disk snapshot.
    crate::document::annots::write_markups_scaled(&mut doc, markups, removed, scales)?;
    crate::document::page_ops::extract_pages(&mut doc, page_indices)?;

    let dir = dest.parent().context("no parent dir for destination path")?;
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
        std::fs::rename(&tmp, dest).context("atomic rename to destination")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Extract `page_indices` (0-based, in the given order - not necessarily sorted or
/// covering every page) from `doc_id` into a brand-new file at `dest_path`. The source
/// document (and its currently open render session) is never modified; `dest_path`
/// must not already point at the same file the source document is open from.
#[tauri::command]
pub async fn extract_pages(
    state: State<'_, AppState>,
    doc_id: String,
    page_indices: Vec<u32>,
    dest_path: String,
) -> Result<(), String> {
    let src = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let dest = PathBuf::from(dest_path);
    if dest == src {
        return Err("extraction destination must be a different file from the source \
                     document - use Save As on the open document instead if that's the \
                     intent"
            .to_string());
    }

    let password = state.markups.password(&doc_id);
    let was_loaded = state.markups.is_loaded(&doc_id);
    if !was_loaded {
        let snapshot = state.markups.begin_first_load(&doc_id);
        let p = src.clone();
        let pw = password.clone();
        let loaded = tokio::task::spawn_blocking(move || load_markups_from(&p, pw.as_deref()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
        state.markups.seed_loaded_with(&doc_id, loaded, snapshot)?;
    }
    ensure_source_unchanged(&state.markups, &doc_id, was_loaded)?;
    let markups = state.markups.list(&doc_id)?;
    let removed = state.markups.removed_sources(&doc_id);
    crate::commands::takeoff::ensure_scales_loaded(&state, &doc_id);
    let scales = state.scales.lock().unwrap().list(&doc_id).to_vec();

    tokio::task::spawn_blocking(move || {
        extract_pages_to_new_file_scaled(&src, &dest, &markups, &removed, &scales, &page_indices)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_sees_through_spelling_and_symlinks_but_not_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(dir.path().join("b.pdf"), b"x").unwrap();
        let dotted = dir.path().join("sub").join("..").join("a.pdf");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert!(same_file(&a, &dotted));
        assert!(!same_file(&a, &dir.path().join("b.pdf")));
        assert!(!same_file(&a, &dir.path().join("new.pdf")), "a not-yet-existing name is a different file");
        #[cfg(unix)]
        {
            let link = dir.path().join("link.pdf");
            std::os::unix::fs::symlink(&a, &link).unwrap();
            assert!(same_file(&a, &link));
        }
    }
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
        apply_edit_and_save(&path, std::slice::from_ref(&m), &Default::default(), |doc| {
            flatten_annotations(doc)?;
            Ok(())
        })
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

        apply_edit_and_save(&path, std::slice::from_ref(&m), &Default::default(), |doc| {
            flatten_annotations(doc)?;
            Ok(())
        })
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
        apply_edit_and_save(&path, std::slice::from_ref(&m), &Default::default(), |doc| optimize_in_place(doc, 2))
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
    fn flatten_via_apply_edit_and_save_handles_a_realistic_mix_of_markup_types() {
        // The synthetic single-markup fixture used by the other flatten regression tests
        // (a 3-point Cloud) is not representative of a real annotated drawing. Investigating
        // the live report "flatten doesn't seem to do anything" required checking whether
        // flatten_page's requirements (indirect /AP /N, a resolvable non-degenerate /BBox)
        // hold for the FULL range of shapes redline's own appearance builder produces, not
        // just one. This drives five structurally different markup types (rect fill+stroke,
        // text-anchored quads highlight, freeform ink, a FreeText box, and a zero-size Point
        // count marker - the case most likely to hit the "degenerate appearance" skip in
        // flatten_page, since ap_bbox pads a Point to a small fixed box) through the SAME
        // write_markups -> flatten_annotations pipeline the real app uses, and asserts every
        // one of them actually gets flattened (no silent partial no-op for some shapes).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flatten-mixed-types.pdf");
        let (mut doc, _page_id) = one_page_doc();
        doc.save(&path).unwrap();

        let creator = || UserRef {
            user_id: uuid::Uuid::new_v4(),
            display_name: "Alice".into(),
        };

        let rect = Markup::new(
            MarkupType::Rectangle,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 10.0, y: 10.0 },
                max: PdfPoint { x: 110.0, y: 60.0 },
            },
            Appearance {
                fill: Some("#ffcc00".into()),
                ..Appearance::default()
            },
            creator(),
        );
        let highlight = Markup::new(
            MarkupType::Highlight,
            0,
            MarkupGeometry::Quads(vec![[
                PdfPoint { x: 72.0, y: 712.0 },
                PdfPoint { x: 200.0, y: 712.0 },
                PdfPoint { x: 72.0, y: 700.0 },
                PdfPoint { x: 200.0, y: 700.0 },
            ]]),
            Appearance::default(),
            creator(),
        );
        let ink = long_ink_markup(0);
        let mut text = Markup::new(
            MarkupType::Text,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 300.0, y: 300.0 },
                max: PdfPoint { x: 450.0, y: 330.0 },
            },
            Appearance::default(),
            creator(),
        );
        text.contents = Some("A real annotation note".into());
        let count = Markup::new(
            MarkupType::MeasurementCount,
            0,
            MarkupGeometry::Point(PdfPoint { x: 500.0, y: 500.0 }),
            Appearance::default(),
            creator(),
        );

        let markups = vec![rect, highlight, ink, text, count];
        let expected = markups.len();

        apply_edit_and_save(&path, &markups, &Default::default(), |doc| {
            let n = flatten_annotations(doc)?;
            assert_eq!(
                n, expected,
                "every markup type in this mix must be flattened - a partial count means \
                 flatten_page's /AP or /BBox assumptions silently failed to hold for one \
                 of these real shapes"
            );
            Ok(())
        })
        .unwrap();

        let reopened = lopdf::Document::load(&path).unwrap();
        let page_id = *reopened.get_pages().values().next().unwrap();
        let page = reopened.get_dictionary(page_id).unwrap();
        assert!(
            page.get(b"Annots").is_err(),
            "all five markup types must be fully flattened - none left selectable"
        );
        let contents = page.get(b"Contents").unwrap().as_array().unwrap();
        assert_eq!(
            contents.len(),
            2,
            "one overlay content stream must be added regardless of how many distinct \
             markup types it bakes (they all share one page-level overlay, per-annotation \
             q/cm/Do/Q blocks within it)"
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
        apply_edit_and_save(&path, std::slice::from_ref(&m), &Default::default(), |doc| {
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

        let err = apply_edit_and_save(&path, &[], &Default::default(), |doc| optimize_in_place(doc, 1));
        assert!(err.is_err(), "encrypted PDFs must be refused, not silently de-protected");
    }

    // ------------------------------------------------------------------
    // extract_pages_to_new_file tests (PR-C, 2026-09-15)
    // ------------------------------------------------------------------

    #[test]
    fn extract_creates_a_new_file_leaving_the_source_untouched() {
        use crate::document::page_ops::tests::n_page_doc;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.pdf");
        let dest = dir.path().join("extracted.pdf");
        let (mut doc, _) = n_page_doc(5);
        doc.save(&src).unwrap();
        let src_bytes_before = std::fs::read(&src).unwrap();

        extract_pages_to_new_file(&src, &dest, &[], &Default::default(), &[1, 3]).unwrap();

        assert!(dest.exists(), "destination file must be created");
        let src_bytes_after = std::fs::read(&src).unwrap();
        assert_eq!(
            src_bytes_before, src_bytes_after,
            "the source document must be byte-for-byte untouched by extraction"
        );

        let extracted = lopdf::Document::load(&dest).unwrap();
        assert_eq!(extracted.get_pages().len(), 2, "only the requested pages must be present");
    }

    #[test]
    fn extract_preserves_markups_on_kept_pages() {
        use crate::document::page_ops::tests::n_page_doc;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.pdf");
        let dest = dir.path().join("extracted.pdf");
        let (mut doc, _) = n_page_doc(3);
        doc.save(&src).unwrap();

        let m = redline_markup(1); // markup lives on page index 1
        extract_pages_to_new_file(&src, &dest, std::slice::from_ref(&m), &Default::default(), &[1]).unwrap();

        let extracted = lopdf::Document::load(&dest).unwrap();
        let markups = crate::document::annots::read_markups(&extracted).unwrap();
        assert_eq!(markups.len(), 1, "the markup on the kept page must survive extraction");
    }

    #[test]
    fn extract_drops_markups_on_pages_not_kept() {
        use crate::document::page_ops::tests::n_page_doc;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.pdf");
        let dest = dir.path().join("extracted.pdf");
        let (mut doc, _) = n_page_doc(3);
        doc.save(&src).unwrap();

        let m = redline_markup(2); // markup lives on page index 2 - NOT extracted below
        extract_pages_to_new_file(&src, &dest, std::slice::from_ref(&m), &Default::default(), &[0, 1]).unwrap();

        let extracted = lopdf::Document::load(&dest).unwrap();
        assert_eq!(extracted.get_pages().len(), 2);
        let markups = crate::document::annots::read_markups(&extracted).unwrap();
        assert_eq!(markups.len(), 0, "a markup on a page that was NOT extracted must not appear");
    }

    #[test]
    fn extract_encrypted_source_is_refused() {
        use crate::document::annots::tests::encrypted_one_page_doc;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("encrypted.pdf");
        let dest = dir.path().join("extracted.pdf");
        let mut doc = encrypted_one_page_doc("redline-pw", "owner-pw");
        doc.save(&src).unwrap();

        let err = extract_pages_to_new_file(&src, &dest, &[], &Default::default(), &[0]);
        assert!(err.is_err(), "encrypted PDFs must be refused, not silently de-protected");
        assert!(!dest.exists(), "no partial destination file on refusal");
    }

    #[test]
    fn extract_invalid_page_indices_propagates_the_page_ops_error() {
        use crate::document::page_ops::tests::n_page_doc;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.pdf");
        let dest = dir.path().join("extracted.pdf");
        let (mut doc, _) = n_page_doc(2);
        doc.save(&src).unwrap();

        let err = extract_pages_to_new_file(&src, &dest, &[], &Default::default(), &[5]);
        assert!(err.is_err(), "an out-of-range page index must be refused");
        assert!(!dest.exists());
    }

    #[test]
    fn a_changed_source_refuses_every_write_and_a_refusal_keeps_the_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.pdf");
        std::fs::write(&p, b"%PDF original").unwrap();
        let store = crate::document::store::MarkupStore::default();
        store.register("d", p.clone(), None);
        assert_eq!(ensure_source_unchanged(&store, "d", true), Ok(()));
        std::fs::write(&p, b"%PDF rewritten by another program").unwrap();
        // Save, Save As, page ops and extract all go through this one check; repeating it
        // never re-baselines, so a refused attempt cannot make the next one pass.
        for _ in 0..3 {
            assert_eq!(
                ensure_source_unchanged(&store, "d", true),
                Err(crate::document::store::ERR_DISK_CHANGED.to_string())
            );
        }
        // A document whose markups were never loaded reads them fresh: nothing stale.
        assert_eq!(ensure_source_unchanged(&store, "d", false), Ok(()));
    }
}
