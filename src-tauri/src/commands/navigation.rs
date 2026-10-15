//! Navigator commands: bookmarks, page labels and links (docs/navigation.md "Document
//! structure"). Reads load the file; every edit goes through the shared page-edit write path
//! (`commands::document::apply_page_edit_keeping_history`): stale-file guard, atomic temp
//! file + rename, render engine reload. The undo history lives in `AppState::nav_history`.

use std::sync::{Arc, Mutex};

use lopdf::Document;
use tauri::State;

use crate::commands::document::apply_page_edit_keeping_history;
use crate::document::navigation::edit::{
    apply_edit, apply_undo, read_info, NavEdit, NavUndo, NavigationInfo,
};
use crate::document::navigation::uri;
use crate::AppState;

const ENCRYPTED_READ_ONLY: &str =
    "This PDF is password-protected: bookmarks, page labels and links can be viewed but not \
     edited (saving would remove the password).";

/// Read bookmarks, page labels and links for the open document.
#[tauri::command]
pub async fn get_navigation(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<NavigationInfo, String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let password = state.markups.password(&doc_id);
    tokio::task::spawn_blocking(move || -> Result<NavigationInfo, String> {
        let doc = Document::load_with_password(&path, password.as_deref().unwrap_or(""))
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let mut info = read_info(&doc);
        if doc.was_encrypted() {
            info.read_only = Some(ENCRYPTED_READ_ONLY.to_string());
        }
        Ok(info)
    })
    .await
    .map_err(|e| e.to_string())?
    .map(|mut info| {
        fill_depths(&state, &doc_id, &mut info);
        remember_labels(&state, &doc_id, &info.labels);
        info
    })
}

/// Remember a document's page labels for the exports (see `AppState::page_labels`).
pub(crate) fn remember_labels(state: &AppState, doc_id: &str, labels: &[String]) {
    store_labels(&mut state.page_labels.lock().unwrap(), doc_id, labels);
}

/// Every document keeps an entry (empty when no page is labelled): "known to have no labels"
/// is distinct from "not read yet". Exports ignore an all-empty list, so no column appears.
fn store_labels(
    all: &mut std::collections::HashMap<String, Vec<String>>,
    doc_id: &str,
    labels: &[String],
) {
    all.insert(doc_id.to_string(), labels.to_vec());
}

/// The document's page labels for the exports (empty when it has none). Filled on first need
/// from the file itself, so an export driven over MCP carries the label column without the GUI
/// ever having opened the Navigator; the page-operation and reload paths drop the entry and the
/// next export refills it.
pub(crate) fn labels_for(state: &AppState, doc_id: &str) -> Vec<String> {
    if let Some(known) = state.page_labels.lock().unwrap().get(doc_id) {
        return known.clone();
    }
    let Some(path) = state.markups.path(doc_id) else {
        return Vec::new();
    };
    let password = state.markups.password(doc_id);
    let Ok(doc) = Document::load_with_password(&path, password.as_deref().unwrap_or("")) else {
        return Vec::new();
    };
    let labels = read_info(&doc).labels;
    remember_labels(state, doc_id, &labels);
    labels
}

fn fill_depths(state: &State<'_, AppState>, doc_id: &str, info: &mut NavigationInfo) {
    let all = state.nav_history.lock().unwrap();
    if let Some(h) = all.get(doc_id) {
        info.undo_depth = h.undo.len();
        info.redo_depth = h.redo.len();
    }
}

enum Step {
    Edit(NavEdit),
    Undo(NavUndo),
    Redo(NavUndo),
}

async fn run(
    state: &State<'_, AppState>,
    doc_id: &str,
    step: Step,
) -> Result<NavigationInfo, String> {
    let slot: Arc<Mutex<Option<(NavigationInfo, NavUndo)>>> = Arc::new(Mutex::new(None));
    let slot_in = Arc::clone(&slot);
    let is_edit = matches!(step, Step::Edit(_));
    let is_undo = matches!(step, Step::Undo(_));
    apply_page_edit_keeping_history(
        state,
        doc_id,
        move |doc| {
            let entry = match &step {
                Step::Edit(e) => apply_edit(doc, e)?,
                Step::Undo(u) | Step::Redo(u) => apply_undo(doc, u)?,
            };
            // The file on disk will be exactly this document, so this is also what the
            // panels show next.
            let info = read_info(doc);
            *slot_in.lock().unwrap() = Some((info, entry));
            Ok(())
        },
        true,
    )
    .await?;
    let (mut info, entry) = slot
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "the edit produced no result".to_string())?;
    let mut all = state.nav_history.lock().unwrap();
    let hist = all.entry(doc_id.to_string()).or_default();
    if is_edit {
        hist.record(entry);
    } else if is_undo {
        hist.undo.pop();
        hist.redo.push(entry);
    } else {
        hist.redo.pop();
        hist.undo.push(entry);
    }
    info.undo_depth = hist.undo.len();
    info.redo_depth = hist.redo.len();
    drop(all);
    remember_labels(state, doc_id, &info.labels);
    Ok(info)
}

/// Apply one navigation edit and return the refreshed state.
#[tauri::command]
pub async fn navigation_edit(
    state: State<'_, AppState>,
    doc_id: String,
    edit: NavEdit,
) -> Result<NavigationInfo, String> {
    run(&state, &doc_id, Step::Edit(edit)).await
}

fn peek(state: &State<'_, AppState>, doc_id: &str, undo: bool) -> Result<NavUndo, String> {
    let all = state.nav_history.lock().unwrap();
    let hist = all.get(doc_id);
    let top = if undo {
        hist.and_then(|h| h.undo.last())
    } else {
        hist.and_then(|h| h.redo.last())
    };
    top.cloned().ok_or_else(|| {
        if undo {
            "Nothing to undo.".to_string()
        } else {
            "Nothing to redo.".to_string()
        }
    })
}

/// Undo the last bookmark, page-label or link edit made in this session.
#[tauri::command]
pub async fn navigation_undo(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<NavigationInfo, String> {
    let entry = peek(&state, &doc_id, true)?;
    run(&state, &doc_id, Step::Undo(entry)).await
}

/// Redo the last undone navigation edit.
#[tauri::command]
pub async fn navigation_redo(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<NavigationInfo, String> {
    let entry = peek(&state, &doc_id, false)?;
    run(&state, &doc_id, Step::Redo(entry)).await
}

/// What the confirmation dialog shows: the validated, normalised address and the token that
/// answers it.
#[derive(serde::Serialize)]
pub struct PreparedOpen {
    pub token: String,
    pub uri: String,
}

/// Step one of opening a web address: validate and normalise it, remember it as the one open
/// question, and return it for the dialog. Nothing is opened. Called when the user clicks a
/// link or bookmark; redline never runs a PDF's actions by itself, and this command is not on
/// the MCP bridge.
#[tauri::command]
pub async fn prepare_external_open(
    state: State<'_, AppState>,
    uri: String,
) -> Result<PreparedOpen, String> {
    let (token, uri) = uri::with_pending(&state.pending_open, |p| {
        p.prepare(&uri, std::time::Instant::now())
    })?;
    Ok(PreparedOpen { token, uri })
}

/// Step two: the user said yes. Opens the address stored by step one for this token (not any
/// address sent now), once, through the system opener with no shell.
#[tauri::command]
pub async fn confirm_external_open(
    state: State<'_, AppState>,
    token: String,
) -> Result<(), String> {
    let uri = uri::with_pending(&state.pending_open, |p| {
        p.take(&token, std::time::Instant::now())
            .ok_or_else(|| "That question is no longer open; click the link again.".to_string())
    })?;
    tokio::task::spawn_blocking(move || uri::open_external(&uri))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn labels_are_kept_per_document_including_none() {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        store_labels(&mut m, "d", &["".into(), "E-1".into()]);
        assert_eq!(m["d"], vec!["", "E-1"]);
        store_labels(&mut m, "d", &["".into(), "  ".into()]);
        assert_eq!(m["d"], vec!["", "  "], "known-unlabelled is remembered too");
        store_labels(&mut m, "e", &[]);
        assert!(m.contains_key("e"));
    }

    /// The MCP bridge may read documents but must not edit navigation or open addresses: none of
    /// these commands is dispatchable from `rpc` (a source-level guard, since the dispatcher is a
    /// string match).
    #[test]
    fn the_mcp_bridge_cannot_edit_navigation_or_open_addresses() {
        let dispatch = include_str!("../rpc/dispatch.rs");
        let tools = include_str!("../bin/redline_mcp.rs");
        for name in [
            "navigation_edit",
            "navigation_undo",
            "navigation_redo",
            "prepare_external_open",
            "confirm_external_open",
            "open_external",
            "get_navigation",
        ] {
            assert!(
                !dispatch.contains(name),
                "rpc dispatch must not expose {name}"
            );
            assert!(
                !tools.contains(name),
                "MCP tool list must not expose {name}"
            );
        }
    }

    /// Every navigation write goes through the shared page-edit path, which refuses when the
    /// file changed on disk (`ensure_source_unchanged`) and refuses encrypted files.
    #[test]
    fn navigation_writes_use_the_guarded_page_edit_path() {
        let me = include_str!("navigation.rs");
        let doc = include_str!("document.rs");
        assert!(me.contains("apply_page_edit_keeping_history("));
        // The only write primitives here are that call; no direct file or store writes.
        for forbidden in ["std::fs::write", "fs::rename", ".save(", "save_to("] {
            assert!(
                !me.split("#[cfg(test)]").next().unwrap().contains(forbidden),
                "{forbidden}"
            );
        }
        let guarded = doc
            .split("pub(crate) async fn apply_page_edit_opts")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .unwrap();
        let guard_at = guarded
            .find("ensure_source_unchanged(")
            .expect("guard present");
        let write_at = guarded
            .find("apply_edit_and_save_inner(")
            .expect("write present");
        assert!(
            guard_at < write_at,
            "the stale-file guard runs before the write"
        );
    }
}
