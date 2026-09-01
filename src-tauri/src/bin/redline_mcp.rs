//! `redline-mcp` — the MCP stdio server for redline (MCP server design, 2026-09-01,
//! `docs/superpowers/specs/2026-09-01-mcp-server-design.md`, §2). Pure protocol
//! translator: MCP JSON-RPC 2.0 over stdio (this binary's own protocol, spoken to
//! Claude Code or any MCP client) <-> the running `redline` GUI app's local RPC
//! socket/pipe (`redline_lib::rpc`). No PDF/model logic lives here - every tool call is
//! forwarded verbatim to the app and the result relayed back. If the app isn't running
//! or the target document isn't open, the app's own "unknown doc_id" / a socket-connect
//! failure surfaces as the tool's error - never a silent fallback to direct file access
//! (design §2, the cost accepted deliberately for single-writer correctness).
//!
//! Ten tools, matching the owner's full-surface scope decision (2026-09-01, "We need
//! mutation as well... And the ability to flatten and reduce file size through the
//! mcp."): wave 1 read-only (list_markups, read_markup, search_markups,
//! export_markup_schedule), wave 2 mutating (create_markup, update_markup,
//! delete_markup, save_document) behind the lock guard, plus the two docops tools
//! (flatten_document, reduce_file_size).
//!
//! Deliberately synchronous, no tokio: this binary's own loop is a blocking read of
//! stdin lines, and the socket round-trip per tool call is a single blocking
//! request/response - no concurrency needed on this side.

use std::io::{self, BufRead, Read, Write};

use redline_lib::rpc::protocol::{RpcRequest, RpcResponse};
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "redline-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut next_call_id: u64 = 1;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            // Malformed input line: nothing to reply to without an id - skip rather
            // than crash the server over one bad line.
            Err(_) => continue,
        };

        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        // A notification (no "id") gets no response, per JSON-RPC 2.0 - e.g.
        // "notifications/initialized". Just observe it and continue.
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };

        let response = match method {
            "initialize" => ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
                }),
            ),
            "tools/list" => ok(id, json!({ "tools": tool_defs() })),
            "tools/call" => handle_tools_call(id, msg.get("params"), &mut next_call_id),
            other => rpc_error(id, -32601, &format!("method not found: {other}")),
        };

        if let Ok(text) = serde_json::to_string(&response) {
            let _ = writeln!(stdout, "{text}");
            let _ = stdout.flush();
        }
    }
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn handle_tools_call(id: Value, params: Option<&Value>, next_call_id: &mut u64) -> Value {
    let Some(params) = params else {
        return rpc_error(id, -32602, "missing params");
    };
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

    // MCP tool-level errors (design §4 item 4: "a structured error naming the markup
    // and the blocking flag") are reported via isError:true, not a JSON-RPC protocol
    // error - the call itself succeeded, the operation was refused.
    match call_bridge(name, arguments, next_call_id) {
        Ok(result) => ok(
            id,
            json!({
                "content": [{ "type": "text", "text": to_text(&result) }],
                "isError": false
            }),
        ),
        Err(err) => ok(
            id,
            json!({
                "content": [{ "type": "text", "text": to_text(&err) }],
                "isError": true
            }),
        ),
    }
}

fn to_text(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Forward one tool call to the running app over the local socket/pipe. A connection
/// failure (app not running, or no document open) surfaces as a clear tool-level error,
/// never a panic and never a silent fallback - design §2's stated requirement.
fn call_bridge(op: &str, arguments: Value, next_call_id: &mut u64) -> Result<Value, Value> {
    let mut stream = connect().map_err(|e| {
        json!({
            "error": "redline_not_running",
            "detail": format!(
                "could not connect to the redline app ({e}) - is redline running with a document open?"
            )
        })
    })?;

    let id = *next_call_id;
    *next_call_id += 1;
    let req = RpcRequest {
        id,
        op: op.to_string(),
        params: arguments,
    };
    let mut payload = serde_json::to_vec(&req)
        .map_err(|e| json!({ "error": "encode_failed", "detail": e.to_string() }))?;
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .map_err(|e| json!({ "error": "write_failed", "detail": e.to_string() }))?;

    let mut reader = io::BufReader::new(stream);
    let line = read_capped_line(&mut reader)
        .map_err(|e| json!({ "error": "read_failed", "detail": e.to_string() }))?
        .unwrap_or_default();
    if line.trim().is_empty() {
        return Err(json!({
            "error": "empty_response",
            "detail": "app closed the connection without a response"
        }));
    }
    let resp: RpcResponse = serde_json::from_str(&line)
        .map_err(|e| json!({ "error": "bad_response", "detail": e.to_string() }))?;

    match (resp.result, resp.error) {
        (Some(r), _) => Ok(r),
        (None, Some(e)) => Err(e),
        (None, None) => Err(json!({ "error": "empty_response" })),
    }
}

/// Read one `\n`-delimited line from `reader`, refusing to grow the buffer past
/// [`redline_lib::rpc::protocol::MAX_LINE_BYTES`] - a stalled or malicious response
/// gets a clean error instead of unbounded memory growth (reviewer finding on PR #92,
/// 2026-09-01, mirrored on the server side in `rpc::read_capped_line`). Reads a byte at
/// a time; when `reader` is a `BufReader` (as it is at the one call site), each call is
/// served from its internal buffer rather than costing a syscall per byte. `Ok(None)`
/// on clean EOF with no bytes read at all; a trailing `\r` (CRLF) is tolerated and
/// stripped.
fn read_capped_line<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    use redline_lib::rpc::protocol::MAX_LINE_BYTES;

    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte)?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            break; // EOF with a trailing unterminated line - return what we have
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > MAX_LINE_BYTES {
            return Err(io::Error::other(format!(
                "line exceeds MAX_LINE_BYTES ({MAX_LINE_BYTES}) with no terminator"
            )));
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Connects to the running app's local endpoint - a Unix domain socket on macOS/Linux,
/// a named pipe on Windows (see `redline_lib::rpc` module doc comment for the Windows
/// path's unverified status). Both branches return a boxed `Read + Write` so
/// `call_bridge` above stays platform-agnostic.
#[cfg(unix)]
fn connect() -> io::Result<Box<dyn ReadWrite>> {
    let s = std::os::unix::net::UnixStream::connect(redline_lib::rpc::socket_path())?;
    Ok(Box::new(s))
}

#[cfg(windows)]
fn connect() -> io::Result<Box<dyn ReadWrite>> {
    // Windows named pipes are openable via the standard file API once a server
    // instance is listening (CreateFileW under the hood) - see the UNVERIFIED note
    // on redline_lib::rpc's Windows server path; this client side is equally
    // unverified without a Windows target to compile/run against in this session.
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(redline_lib::rpc::WINDOWS_PIPE_NAME)?;
    Ok(Box::new(f))
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// The MCP tool-surface definitions (JSON Schema `inputSchema` per tool). Kept as one
/// function so `tools/list`'s output is the single source of truth for what this
/// server advertises.
fn tool_defs() -> Value {
    json!([
        {
            "name": "list_markups",
            "description": "List markups in an open redline document, optionally filtered by page/type/status. Each result reports locked/locked_contents so a doomed mutation can be avoided up front.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string", "description": "The doc_id returned by redline when the document was opened."},
                    "page": {"type": "integer", "description": "Zero-based page index filter."},
                    "type_filter": {"type": "array", "items": {"type": "string"}},
                    "status_filter": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["doc_id"]
            }
        },
        {
            "name": "read_markup",
            "description": "Read the full envelope (all fields, untruncated) of one markup by id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "markup_id": {"type": "string"}
                },
                "required": ["doc_id", "markup_id"]
            }
        },
        {
            "name": "search_markups",
            "description": "Search markup subject/contents note text within an open document (v1: document scope only - not raw PDF text search, see search_document via the GUI for that).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "query": {"type": "string"},
                    "case_sensitive": {"type": "boolean"},
                    "scope": {"type": "string", "enum": ["document"], "description": "Only 'document' is implemented in v1."}
                },
                "required": ["doc_id", "query"]
            }
        },
        {
            "name": "export_markup_schedule",
            "description": "Export the open document's markup schedule to CSV or XLSX, written next to the source file. Returns the generated file's path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "format": {"type": "string", "enum": ["Xlsx", "Csv"]}
                },
                "required": ["doc_id", "format"]
            }
        },
        {
            "name": "create_markup",
            "description": "Create a new markup on the open document (in-memory only - call save_document to persist to the file).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "markup_type": {"type": "string", "description": "e.g. Rectangle, Highlight, Text, MeasurementCount."},
                    "page": {"type": "integer"},
                    "geometry": {"type": "object", "description": "A MarkupGeometry variant, e.g. {\"Rect\":{\"min\":{...},\"max\":{...}}}."},
                    "appearance": {"type": "object"},
                    "contents": {"type": "string"},
                    "subject": {"type": "string"},
                    "layer": {"type": "string"}
                },
                "required": ["doc_id", "markup_type", "page", "geometry"]
            }
        },
        {
            "name": "update_markup",
            "description": "Update an existing markup's contents/appearance/workflow status (whole fields, no field-level clear-to-null in v1). Refused with a markup_locked error if the markup carries the Locked or LockedContents PDF annotation flag.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "markup_id": {"type": "string"},
                    "contents": {"type": "string"},
                    "appearance": {"type": "object"},
                    "workflow_status": {"type": "string", "enum": ["None", "Accepted", "Rejected", "Completed"]}
                },
                "required": ["doc_id", "markup_id"]
            }
        },
        {
            "name": "delete_markup",
            "description": "Delete a markup by id. Refused with a markup_locked error if the markup is locked.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "markup_id": {"type": "string"}
                },
                "required": ["doc_id", "markup_id"]
            }
        },
        {
            "name": "save_document",
            "description": "Persist all in-memory markup changes (create/update/delete since the last save) to the open document's file.",
            "inputSchema": {
                "type": "object",
                "properties": { "doc_id": {"type": "string"} },
                "required": ["doc_id"]
            }
        },
        {
            "name": "flatten_document",
            "description": "Bake all markup appearances into the page content and remove them as live annotations. Irreversible once saved - the document no longer has editable markup objects for the flattened annotations.",
            "inputSchema": {
                "type": "object",
                "properties": { "doc_id": {"type": "string"} },
                "required": ["doc_id"]
            }
        },
        {
            "name": "reduce_file_size",
            "description": "Reduce the open document's file size: prune unreferenced objects, compress streams, and optionally recompress raster images at a quality preset (High/Balanced/Small).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "level": {"type": "integer", "enum": [0, 1, 2], "description": "0=no-op, 1=prune only, 2=prune+compress (default)."},
                    "image_preset": {"type": "string", "enum": ["High", "Balanced", "Small"], "description": "Omit to skip raster recompression entirely."}
                },
                "required": ["doc_id"]
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_defs_names_all_ten_tools_from_the_scope_decision() {
        let defs = tool_defs();
        let names: Vec<&str> = defs
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for expected in [
            "list_markups",
            "read_markup",
            "search_markups",
            "export_markup_schedule",
            "create_markup",
            "update_markup",
            "delete_markup",
            "save_document",
            "flatten_document",
            "reduce_file_size",
        ] {
            assert!(names.contains(&expected), "missing tool: {expected}");
        }
        assert_eq!(names.len(), 10);
    }

    #[test]
    fn every_tool_def_has_a_required_doc_id() {
        let defs = tool_defs();
        for tool in defs.as_array().unwrap() {
            let required = tool["inputSchema"]["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{} has no required array", tool["name"]));
            assert!(
                required.iter().any(|r| r == "doc_id"),
                "{} must require doc_id",
                tool["name"]
            );
        }
    }

    #[test]
    fn handle_tools_call_missing_params_is_a_protocol_error_not_a_tool_error() {
        let resp = handle_tools_call(json!(1), None, &mut 1);
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn call_bridge_connection_failure_is_a_structured_tool_error() {
        // No redline app is running in this test process, so this must fail cleanly
        // with a labelled error rather than panicking - design §2's "never a silent
        // fallback" requirement, exercised from the client side.
        let err = call_bridge("list_markups", json!({"doc_id": "d1"}), &mut 1).unwrap_err();
        assert_eq!(err["error"], "redline_not_running");
    }

    mod read_capped_line_tests {
        use super::*;
        use std::io::Cursor;

        #[test]
        fn reads_a_normal_newline_terminated_line() {
            let mut r = Cursor::new(b"hello\nworld\n".to_vec());
            assert_eq!(read_capped_line(&mut r).unwrap(), Some("hello".to_string()));
            assert_eq!(read_capped_line(&mut r).unwrap(), Some("world".to_string()));
            assert_eq!(read_capped_line(&mut r).unwrap(), None);
        }

        #[test]
        fn strips_a_trailing_crlf() {
            let mut r = Cursor::new(b"hello\r\n".to_vec());
            assert_eq!(read_capped_line(&mut r).unwrap(), Some("hello".to_string()));
        }

        #[test]
        fn clean_eof_with_no_bytes_returns_none() {
            let mut r = Cursor::new(Vec::<u8>::new());
            assert_eq!(read_capped_line(&mut r).unwrap(), None);
        }

        #[test]
        fn refuses_a_line_exceeding_max_line_bytes_instead_of_growing_unbounded() {
            let oversized = vec![b'a'; redline_lib::rpc::protocol::MAX_LINE_BYTES + 1];
            let mut r = Cursor::new(oversized);
            let err = read_capped_line(&mut r).unwrap_err();
            assert!(err.to_string().contains("MAX_LINE_BYTES"), "got: {err}");
        }
    }
}
