//! Wire types shared between the RPC server (this crate, `rpc::dispatch`, running
//! inside the `redline` GUI app) and the `redline-mcp` stdio binary
//! (`src/bin/redline_mcp.rs` - a separate crate that depends on `redline_lib` but has
//! no Tauri/PDFium context of its own). One line-delimited JSON object per
//! request/response over the local socket/pipe (MCP server design, 2026-09-01, §2/§5).
//!
//! Deliberately NOT the MCP JSON-RPC 2.0 envelope itself (`method`/`jsonrpc`/
//! `notifications/*`) - `redline-mcp` is the only thing that speaks that protocol, to
//! its Claude Code parent over stdio. This is the simpler internal transport it
//! translates a `tools/call` into and translates the result back from.

use serde::{Deserialize, Serialize};

/// Hard cap on a single line-delimited frame, enforced by both read loops (the app's
/// `rpc::serve_lines` and `redline-mcp`'s `call_bridge`) - an unterminated or malicious
/// frame refuses with a clean error instead of growing the read buffer without bound
/// (reviewer finding on PR #92, 2026-09-01). 10 MiB comfortably covers any real tool
/// call/response in this protocol (the largest payload today, `read_markup`'s full
/// envelope, is at most tens of KB) while still being a hard, finite ceiling.
pub const MAX_LINE_BYTES: usize = 10 * 1024 * 1024;

/// One request over the local socket: `op` names the tool (e.g. `"list_markups"`),
/// `params` is the tool's arguments as a JSON object (or `null` for no-arg tools).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub id: u64,
    pub op: String,
    #[serde(default = "default_params")]
    pub params: serde_json::Value,
}

fn default_params() -> serde_json::Value {
    serde_json::Value::Null
}

/// Response echoing the request `id`. Exactly one of `result`/`error` is set.
/// `error` is a real JSON value (not a stringified blob) so a structured refusal - the
/// markup-lock error shape (design §4 item 4) - round-trips as an object a caller can
/// inspect field-by-field, not prose to pattern-match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_defaults_params_to_null_when_absent() {
        let req: RpcRequest = serde_json::from_str(r#"{"id":1,"op":"list_markups"}"#).unwrap();
        assert_eq!(req.params, serde_json::Value::Null);
    }

    #[test]
    fn response_omits_absent_result_and_error_fields() {
        let resp = RpcResponse {
            id: 7,
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("\"error\""));
        assert!(json.contains("\"result\""));
    }

    #[test]
    fn response_round_trips_a_structured_error_object() {
        let resp = RpcResponse {
            id: 3,
            result: None,
            error: Some(serde_json::json!({
                "error": "markup_locked",
                "markup_id": "abc",
                "flag": "Locked"
            })),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: RpcResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.error.unwrap()["flag"], "Locked");
        assert!(back.result.is_none());
    }

    /// Documents the root cause of a live bug found 2026-09-02 (delete_markup): serde's
    /// `Option<T>` deserialization always collapses a JSON `null` to `None`, regardless
    /// of `T`. A success response carrying `result: Some(serde_json::Value::Null)` -
    /// exactly what `Ok(()).map(|()| Value::Null)` produces - therefore round-trips
    /// through the wire as INDISTINGUISHABLE from a response with no result field at
    /// all, and `redline-mcp`'s `call_bridge` (`(None, None) => Err("empty_response")`)
    /// reports a genuinely successful call as a failure. This is a property of
    /// `Option<serde_json::Value>` itself, not a bug in this struct's derive - callers
    /// on the server side (`rpc::dispatch`) must never map a unit-returning success to
    /// `Value::Null`; use a non-null sentinel (e.g. `json!({"deleted": true})`) instead.
    #[test]
    fn a_null_result_value_is_indistinguishable_from_no_result_after_round_tripping() {
        let resp = RpcResponse {
            id: 10,
            result: Some(serde_json::Value::Null),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: RpcResponse = serde_json::from_str(&json).unwrap();
        // This is the trap: `result` was `Some(Null)` before the wire, `None` after.
        assert!(
            back.result.is_none(),
            "serde_json collapsed Some(Value::Null) to None on deserialize, as expected - \
             this is why dispatch.rs must never use Value::Null for a success result"
        );
        assert!(back.error.is_none());
    }
}
