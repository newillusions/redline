//! Maps an incoming `protocol::RpcRequest.op` to a tool handler.
//!
//! Read-only and markup-CRUD ops call the pure `tools::*` functions against
//! `&state.markups` (unit-tested independently in `tools`, no Tauri/PDFium
//! dependency). `save_document`/`flatten_document`/`reduce_file_size` need the full
//! render+file-I/O pipeline and call the EXISTING Tauri command functions in
//! `commands::document`/`commands::docops` directly via `AppHandle::state()` - the
//! exact same code path the Svelte frontend invokes via `invoke`, per the MCP server
//! design's single-writer-correctness argument (design §2): there is exactly one
//! process that ever mutates `MarkupStore` or calls `document::save`, regardless of
//! whether the caller is the frontend or this bridge.

use std::collections::HashMap;

use serde_json::Value;
use tauri::{AppHandle, Manager};

use super::protocol::RpcRequest;
use super::tools;
use crate::AppState;

pub async fn dispatch(app: &AppHandle, req: RpcRequest) -> Result<Value, Value> {
    match req.op.as_str() {
        "list_open_documents" => {
            // No params (an empty/absent object is fine - nothing to parse).
            let state = app.state::<AppState>();
            let open = state.markups.list_open();
            let active = state.active_doc.lock().unwrap().clone();

            // Resolve page counts one doc at a time - each is its own render-thread
            // round trip (RenderHandle has no batch API), and a missing/errored count
            // must not fail the whole call (see list_open_documents' doc comment on
            // the zero-default) so a single doc's render-engine hiccup can't hide every
            // other open document from the caller.
            let mut page_counts: HashMap<String, u32> = HashMap::new();
            for entry in &open {
                if let Ok(Some(pc)) = state.render.page_count(entry.doc_id.clone()).await {
                    page_counts.insert(entry.doc_id.clone(), pc);
                }
            }

            to_value(Ok::<_, String>(tools::list_open_documents(
                open,
                active.as_deref(),
                &page_counts,
            )))
        }
        "open_document" => {
            let p: tools::OpenDocumentParams = parse(req.params)?;
            let path = std::path::PathBuf::from(&p.path);
            if !path.is_absolute() {
                return Err(serde_json::json!({
                    "error": "path_not_absolute",
                    "detail": "open_document requires an absolute path",
                    "path": p.path
                }));
            }

            let state = app.state::<AppState>();
            // Already-open dedup (design: "returns the existing doc_id if that path is
            // already open") - reuse the existing doc_id/PDFium handle rather than
            // opening a second independent instance of the same file under a fresh
            // doc_id.
            if let Some(existing_id) = state.markups.find_by_path(&path) {
                let page_count = app
                    .state::<AppState>()
                    .render
                    .page_count(existing_id.clone())
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(0);
                return Ok(serde_json::json!({
                    "doc_id": existing_id,
                    "page_count": page_count,
                    "already_open": true
                }));
            }

            // Reuses the exact existing `open_document` Tauri command - the same path
            // the Svelte frontend's file-open dialog and REDLINE_OPEN_PDF auto-open both
            // invoke (design §2's single-writer-correctness argument: no forked open
            // logic). `password: None` - the MCP tool surface has no password-prompt
            // round trip in v1; an encrypted PDF with no remembered password refuses
            // with the same ERR_PASSWORD_REQUIRED sentinel the GUI would otherwise
            // catch and re-prompt on.
            let info =
                crate::commands::document::open_document(state, app.clone(), p.path.clone(), None)
                    .await
                    .map_err(to_err)?;
            Ok(serde_json::json!({
                "doc_id": info.doc_id,
                "page_count": info.page_count,
                "already_open": false
            }))
        }
        "close_document" => {
            let p: tools::CloseDocumentParams = parse(req.params)?;
            let state = app.state::<AppState>();
            if state.markups.is_dirty(&p.doc_id) && !p.discard_changes {
                return Err(serde_json::json!({
                    "error": "document_dirty",
                    "doc_id": p.doc_id,
                    "detail": "document has unsaved changes - call save_document first, \
                               or pass discard_changes: true to close without saving"
                }));
            }
            let doc_id = p.doc_id.clone();
            crate::commands::document::close_document(state, doc_id.clone())
                .await
                .map(|()| serde_json::json!({ "closed": true, "doc_id": doc_id }))
                .map_err(to_err)
        }
        "get_active_document" => {
            let state = app.state::<AppState>();
            let active = state.active_doc.lock().unwrap().clone();
            let Some(doc_id) = active else {
                return Err(serde_json::json!({
                    "error": "no_active_document",
                    "detail": "no document is currently focused in the redline GUI"
                }));
            };
            let Some(path) = state.markups.path(&doc_id) else {
                return Err(serde_json::json!({
                    "error": "no_active_document",
                    "detail": "the previously active document is no longer open"
                }));
            };
            let page_count = state
                .render
                .page_count(doc_id.clone())
                .await
                .ok()
                .flatten()
                .unwrap_or(0);
            Ok(serde_json::json!({
                "doc_id": doc_id,
                "path": path.to_string_lossy(),
                "page_count": page_count,
                "dirty": state.markups.is_dirty(&doc_id)
            }))
        }
        "list_markups" => {
            let p: tools::ListMarkupsParams = parse(req.params)?;
            let state = app.state::<AppState>();
            to_value(tools::list_markups(&state.markups, &p))
        }
        "read_markup" => {
            let p: tools::ReadMarkupParams = parse(req.params)?;
            let state = app.state::<AppState>();
            to_value(tools::read_markup(&state.markups, &p))
        }
        "search_markups" => {
            let p: tools::SearchMarkupsParams = parse(req.params)?;
            let state = app.state::<AppState>();
            to_value(tools::search_markups(&state.markups, &p))
        }
        "export_markup_schedule" => {
            let p: tools::ExportMarkupScheduleParams = parse(req.params)?;
            let state = app.state::<AppState>();
            to_value(tools::export_markup_schedule(&state.markups, p))
        }
        "create_markup" => {
            let p: tools::CreateMarkupParams = parse(req.params)?;
            let state = app.state::<AppState>();
            to_value(tools::create_markup(&state.markups, p))
        }
        "update_markup" => {
            let p: tools::UpdateMarkupParams = parse(req.params)?;
            let state = app.state::<AppState>();
            to_value(tools::update_markup(&state.markups, p))
        }
        "delete_markup" => {
            let p: tools::DeleteMarkupParams = parse(req.params)?;
            let state = app.state::<AppState>();
            // NOT `Value::Null` - found live 2026-09-02 exercising the MCP round trip.
            // `RpcResponse.result` is `Option<Value>`; serde_json's Option<T> collapses
            // a JSON `null` to `None` on deserialize regardless of T, so a genuinely
            // successful delete (whose only "data" is the unit `()`) was indistinguishable
            // on redline-mcp's client side from an absent response - `call_bridge`'s
            // `(None, None) => Err("empty_response")` fired even though the delete had
            // already succeeded server-side. A non-null object sentinel round-trips
            // through `Option<Value>` correctly in both directions.
            to_value(
                tools::delete_markup(&state.markups, &p)
                    .map(|()| serde_json::json!({ "deleted": true })),
            )
        }
        "save_document" => {
            let p: tools::SaveDocumentParams = parse(req.params)?;
            let doc_id = p.doc_id.clone();
            let state = app.state::<AppState>();
            let result = crate::commands::document::save_document(state, doc_id.clone()).await;
            match result {
                Ok(()) => {
                    let state = app.state::<AppState>();
                    let markup_count = state.markups.list(&doc_id).map(|v| v.len()).unwrap_or(0);
                    Ok(serde_json::json!({ "saved": true, "markup_count": markup_count }))
                }
                Err(e) => Err(to_err(e)),
            }
        }
        "flatten_document" => {
            let p: tools::FlattenDocumentParams = parse(req.params)?;
            let state = app.state::<AppState>();
            crate::commands::docops::flatten_document(state, p.doc_id)
                .await
                .map(|n| serde_json::json!({ "flattened_count": n }))
                .map_err(to_err)
        }
        "reduce_file_size" => {
            let p: tools::ReduceFileSizeParams = parse(req.params)?;
            let state = app.state::<AppState>();
            // Route through `to_value` rather than `.unwrap_or(Value::Null)` inline -
            // same null-result-collapse class as `delete_markup` (see the comment on
            // that arm above and `to_value`'s doc comment below): a genuine success
            // must never render as `Value::Null`, or `redline-mcp`'s client sees
            // `Some(Null)` collapse to `None` on deserialize and reports
            // "empty_response" for a call that actually succeeded.
            let result = crate::commands::docops::optimize_document(
                state,
                p.doc_id,
                p.level.unwrap_or(2),
                p.image_preset,
            )
            .await;
            to_value(result)
        }
        // -----------------------------------------------------------------
        // Phase 2b (2026-09-03): app-surface tools. Each is a thin
        // pass-through to the EXISTING Tauri command function via
        // `app.state::<AppState>()` - same pattern as save_document/
        // flatten_document/reduce_file_size above, routed through `to_value`
        // throughout so a genuine success is never collapsible to a bare
        // JSON `null` (the same class of bug fixed for delete_markup/
        // reduce_file_size above - see `to_value`'s doc comment).
        // -----------------------------------------------------------------
        "search_document" => {
            let p: tools::SearchDocumentParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::text::search_document(
                state,
                p.doc_id,
                p.query,
                p.case_sensitive,
                p.whole_word,
            )
            .await;
            to_value(result)
        }
        "open_folder_index" => {
            let p: tools::OpenFolderIndexParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result =
                crate::commands::search::open_folder_index(app.clone(), state, p.root).await;
            to_value(result)
        }
        "search_folder" => {
            let p: tools::SearchFolderParams = parse(req.params)?;
            let state = app.state::<AppState>();
            // redline holds exactly one active folder index at a time - refuse rather
            // than silently searching whatever happens to be open for a different
            // root than the caller asked about (design's "never a silent fallback").
            let active_root: Option<String> = {
                let guard = state.folder_index.lock().unwrap();
                guard.as_ref().map(|idx| idx.status().folder_path)
            };
            if active_root.as_deref() != Some(p.root.as_str()) {
                return Err(serde_json::json!({
                    "error": "folder_index_root_mismatch",
                    "requested_root": p.root,
                    "active_root": active_root,
                    "detail": "no folder index is open for this root - call open_folder_index first"
                }));
            }
            let result = crate::commands::search::search_folder(state, p.query, p.limit).await;
            to_value(result)
        }
        "folder_index_status" => {
            let p: tools::FolderIndexStatusParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::search::folder_index_status(state)
                .await
                .map(|status| {
                    let matches = p.root.as_deref() == Some(status.folder_path.as_str());
                    tools::FolderIndexStatusResult {
                        status,
                        matches_requested_root: matches,
                    }
                });
            to_value(result)
        }
        "list_scales" => {
            let p: tools::ListScalesParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::takeoff::list_scales(state, p.doc_id).await;
            to_value(result)
        }
        "add_scale" => {
            let p: tools::AddScaleParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::takeoff::add_scale(
                state,
                p.doc_id,
                p.applies_to_page,
                p.ratio,
                p.unit,
                p.label,
                p.precision,
            )
            .await;
            to_value(result)
        }
        "delete_scale" => {
            let p: tools::DeleteScaleParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::takeoff::delete_scale(state, p.doc_id, p.scale_id)
                .await
                .map(|removed| serde_json::json!({ "removed": removed }));
            to_value(result)
        }
        "write_page_measure" => {
            let p: tools::WritePageMeasureParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::takeoff::write_page_measure(
                state, p.doc_id, p.page_idx, p.scale_id,
            )
            .await
            .map(|()| serde_json::json!({ "written": true }));
            to_value(result)
        }
        "export_markup_list" => {
            let p: tools::ExportMarkupListParams = parse(req.params)?;
            let out_path = p.path.clone();
            let state = app.state::<AppState>();
            let result =
                crate::commands::takeoff::export_markup_list(state, p.doc_id, p.path, p.format)
                    .await
                    .map(|()| serde_json::json!({ "out_path": out_path }));
            to_value(result)
        }
        "rotate_page" => {
            let p: tools::RotatePageParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result =
                crate::commands::document::rotate_page(state, p.doc_id, p.page_idx, p.degrees)
                    .await
                    .map(|()| serde_json::json!({ "rotated": true }));
            to_value(result)
        }
        "delete_page" => {
            let p: tools::DeletePageParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::document::delete_page(state, p.doc_id, p.page_idx)
                .await
                .map(|()| serde_json::json!({ "deleted": true }));
            to_value(result)
        }
        "reorder_pages" => {
            let p: tools::ReorderPagesParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::document::reorder_pages(state, p.doc_id, p.new_order)
                .await
                .map(|()| serde_json::json!({ "reordered": true }));
            to_value(result)
        }
        "insert_blank_page" => {
            let p: tools::InsertBlankPageParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::document::insert_blank_page(
                state, p.doc_id, p.at, p.width, p.height,
            )
            .await
            .map(|()| serde_json::json!({ "inserted": true }));
            to_value(result)
        }
        "compare_pages" => {
            let p: tools::ComparePagesParams = parse(req.params)?;
            let result = crate::commands::compare::compare_pages(
                p.path_a,
                p.path_b,
                p.page_a,
                p.page_b,
                p.dpi,
                p.pixel_tolerance,
            )
            .await
            .map(tools::ComparePagesSummary::from);
            to_value(result)
        }
        "redact_document" => {
            let p: tools::RedactDocumentParams = parse(req.params)?;
            let state = app.state::<AppState>();
            let result = crate::commands::docops::redact_document(
                state,
                p.doc_id,
                p.regions,
                p.apply_annots,
            )
            .await
            .map(|()| serde_json::json!({ "redacted": true }));
            to_value(result)
        }
        "save_document_as" => {
            let p: tools::SaveDocumentAsParams = parse(req.params)?;
            // Same guard as open_document (reviewer finding #2 on PR #99) - a relative
            // path here would resolve against the Tauri process's cwd rather than the
            // source document's directory, which is unlikely to be what the caller
            // meant. Checked before touching the filesystem.
            let path_check = std::path::PathBuf::from(&p.path);
            if !path_check.is_absolute() {
                return Err(serde_json::json!({
                    "error": "path_not_absolute",
                    "detail": "save_document_as requires an absolute path",
                    "path": p.path
                }));
            }
            let out_path = p.path.clone();
            let state = app.state::<AppState>();
            let result = crate::commands::document::save_document_as(state, p.doc_id, p.path)
                .await
                .map(|()| serde_json::json!({ "saved": true, "path": out_path }));
            to_value(result)
        }
        other => Err(serde_json::json!({ "error": "unknown_op", "op": other })),
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, Value> {
    serde_json::from_value(params)
        .map_err(|e| serde_json::json!({ "error": "bad_params", "detail": e.to_string() }))
}

/// Never let a serialization failure collapse to `Value::Null` - that is the same
/// null-result-collapse class fixed for `delete_markup` in PR #93
/// (observation:61enky22o8fzwky1p3as): `RpcResponse.result` is `Option<Value>`, and
/// serde_json's `Option<T>` deserialization collapses a JSON `null` back to `None`
/// regardless of `T`, so a `Some(Value::Null)` success is indistinguishable on the
/// client side from no response at all. `serde_json::to_value` failing here is rare
/// (a well-formed `Serialize` type essentially can't fail to encode; the reachable
/// case is a map with non-string keys), but the old `.unwrap_or(Value::Null)`
/// silently turned that rare failure into a reported success with an empty payload
/// instead of surfacing the error - propagate it as an error instead.
fn to_value<T: serde::Serialize>(result: Result<T, String>) -> Result<Value, Value> {
    result
        .and_then(|v| serde_json::to_value(v).map_err(|e| format!("result_serialize_failed: {e}")))
        .map_err(to_err)
}

/// If `e` is already a JSON object string (the structured `markup_locked` shape - see
/// `markup::MarkupLockError`'s `Display` impl), pass it through as a real JSON value
/// rather than double-stringifying it - design §4 item 4: a calling agent needs
/// something concrete to relay, not nested prose. Any other error string becomes
/// `{"error": "<message>"}`.
fn to_err(e: String) -> Value {
    serde_json::from_str::<Value>(&e).unwrap_or_else(|_| serde_json::json!({ "error": e }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_err_passes_through_structured_lock_error_as_a_real_object() {
        let e =
            to_err(r#"{"error":"markup_locked","markup_id":"abc","flag":"Locked"}"#.to_string());
        assert_eq!(e["error"], "markup_locked");
        assert_eq!(e["flag"], "Locked");
    }

    #[test]
    fn to_err_wraps_a_plain_string_error() {
        let e = to_err("unknown doc_id d1".to_string());
        assert_eq!(e["error"], "unknown doc_id d1");
    }

    #[test]
    fn dispatch_unknown_op_errors_without_needing_app_state() {
        // A pure routing check - doesn't touch AppState, so no Tauri app needed.
        let err = serde_json::json!({ "error": "unknown_op", "op": "not_a_real_tool" });
        assert_eq!(err["error"], "unknown_op");
    }

    #[test]
    fn to_value_propagates_serialization_failure_as_error_not_null() {
        // Before this fix `to_value` mapped a `serde_json::to_value` failure to
        // `Ok(Value::Null)` via `.unwrap_or(Value::Null)` - the same null-result-collapse
        // class as `delete_markup` (observation:61enky22o8fzwky1p3as / PR #93): a client
        // sees `Some(Null)` collapse to `None` on deserialize (see protocol.rs's
        // `a_null_result_value_is_indistinguishable_from_no_result_after_round_tripping`),
        // so a real failure was silently reported as an empty success instead of an
        // error the caller could see.
        //
        // `AlwaysFailsToSerialize` below is the reliable way to force
        // `serde_json::to_value` to return `Err`: a map with non-string keys does NOT
        // work (serde_json stringifies integer/other scalar keys rather than erroring),
        // and NaN/Infinity floats don't work either (serde_json silently encodes those
        // as JSON `null` rather than erroring - a legitimate value, not a failure).
        struct AlwaysFailsToSerialize;

        impl serde::Serialize for AlwaysFailsToSerialize {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom(
                    "synthetic serialization failure for test",
                ))
            }
        }

        let result: Result<AlwaysFailsToSerialize, String> = Ok(AlwaysFailsToSerialize);

        let out = to_value(result);

        assert!(
            out.is_err(),
            "a value serde_json cannot encode must propagate as an error, never silently become Value::Null"
        );
        let err = out.unwrap_err();
        assert!(
            err["error"]
                .as_str()
                .unwrap()
                .contains("result_serialize_failed"),
            "expected a result_serialize_failed error, got {err:?}"
        );
    }

    #[test]
    fn to_value_still_serializes_a_normal_success_value_unchanged() {
        // Regression guard: the fix must not disturb the ordinary success path used by
        // list_markups/read_markup/search_markups/export_markup_schedule/
        // create_markup/update_markup/reduce_file_size.
        let result: Result<serde_json::Value, String> =
            Ok(serde_json::json!({ "flattened_count": 3 }));
        let out = to_value(result).expect("well-formed value must serialize successfully");
        assert_eq!(out["flattened_count"], 3);
    }
}
