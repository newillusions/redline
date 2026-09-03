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
//! Thirty tools. Ten from the owner's full-surface scope decision (2026-09-01, "We
//! need mutation as well... And the ability to flatten and reduce file size through the
//! mcp."): wave 1 read-only (list_markups, read_markup, search_markups,
//! export_markup_schedule), wave 2 mutating (create_markup, update_markup,
//! delete_markup, save_document) behind the lock guard, plus the two docops tools
//! (flatten_document, reduce_file_size). Plus four Phase 2a document-lifecycle tools
//! (2026-09-03, owner-approved "add open tools"): list_open_documents, open_document,
//! close_document, get_active_document - added because every one of the original ten
//! requires a doc_id, and prior to this a client had no way to obtain one without
//! reading it out of the app's own log file (observation:dtko8oxo8fooqt7qrt44). Plus
//! sixteen Phase 2b app-surface tools (2026-09-03, owner-approved "start 2b"): search
//! (search_document, open_folder_index, search_folder, folder_index_status), takeoff
//! (list_scales, add_scale, delete_scale, write_page_measure, export_markup_list), page
//! operations (rotate_page, delete_page, reorder_pages, insert_blank_page), compare
//! (compare_pages), docops (redact_document), and save_document_as - see the design
//! doc's "Implementation notes (Phase 2b...)" section for the mutates/persists/
//! failure-mode detail behind each one.
//!
//! Deliberately synchronous, no tokio: this binary's own loop is a blocking read of
//! stdin lines, and the socket round-trip per tool call is a single blocking
//! request/response - no concurrency needed on this side.
//!
//! **`compare_pages` is opt-in, not one of the 29 tools listed by default** (PR #99
//! review finding #2, 2026-09-03): it hangs indefinitely today due to a pre-existing
//! double-PDFium-binding conflict inside the wrapped `commands::compare::compare_pages`
//! command (see the design doc's "Implementation notes (Phase 2b...)" section and
//! `experimental_enabled()` below). Set `REDLINE_MCP_EXPERIMENTAL=1` to enable it.
//!
//! **The socket round trip has a read/write timeout** (default 120s, override via
//! `REDLINE_MCP_TIMEOUT_SECS`) - PR #99 review finding #1: this binary's loop is a
//! single blocking thread with no concurrency, so an unbounded socket read on a
//! server-side hang (`compare_pages` today) used to wedge the ENTIRE process silently
//! for every later call, not just that one. A timeout now surfaces as a structured
//! `redline_timeout` tool error instead.

use std::io::{self, BufRead, Read, Write};
use std::time::Duration;

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
            "tools/call" => handle_tools_call(
                id,
                msg.get("params"),
                &mut next_call_id,
                experimental_enabled(),
            ),
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

/// `experimental` gates the known-hanging `compare_pages` tool (PR #99 review finding
/// #2) - passed in explicitly rather than read from the environment here so this
/// function's gating behaviour is unit-testable without mutating process-global env
/// state (which is unsound under `cargo test`'s default parallel execution). The real
/// binary's `main()` loop passes `experimental_enabled()`.
fn handle_tools_call(
    id: Value,
    params: Option<&Value>,
    next_call_id: &mut u64,
    experimental: bool,
) -> Value {
    let Some(params) = params else {
        return rpc_error(id, -32602, "missing params");
    };
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

    // compare_pages is opt-in (see the module doc comment and experimental_enabled())
    // - refused HERE, before ever touching the socket, so a client that calls it by
    // name without having seen it in tools/list still gets a clear, cheap refusal
    // rather than the (now-timeout-bounded, but still costly and app-side-thread-
    // leaking) real hang.
    if name == "compare_pages" && !experimental {
        return ok(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": to_text(&json!({
                        "error": "experimental_tool_disabled",
                        "detail": "compare_pages is disabled by default - it can hang \
                                   indefinitely due to a known pre-existing PDFium \
                                   double-binding conflict inside \
                                   commands::compare::compare_pages (see the design \
                                   doc's 'Implementation notes (Phase 2b...)' section). \
                                   Set REDLINE_MCP_EXPERIMENTAL=1 to enable it at your \
                                   own risk - a hang now recovers client-side via the \
                                   socket timeout, but may still leak a stuck thread \
                                   app-side."
                    }))
                }],
                "isError": true
            }),
        );
    }

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

/// Read/write timeout on the socket round trip to the running app, in seconds
/// (PR #99 review finding #1). `redline-mcp`'s own loop is a single blocking thread -
/// with no timeout, one server-side call that never responds (`compare_pages` today,
/// see the module doc comment) wedges this ENTIRE process for every subsequent tool
/// call too, silently, until the client is killed and restarted. Default 120s is long
/// enough for a legitimate slow operation (a large save/optimize) but short enough that
/// a genuinely stuck call degrades to a clear error. Override for local debugging of a
/// known-slow operation via `REDLINE_MCP_TIMEOUT_SECS`; a non-positive or unparseable
/// value falls back to the default rather than disabling the timeout entirely - there
/// is no supported way to wait forever, on purpose.
fn socket_timeout() -> Duration {
    let secs = std::env::var("REDLINE_MCP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(120);
    Duration::from_secs(secs)
}

/// Whether experimental/known-unstable tools are enabled for this process
/// (PR #99 review finding #2). Currently gates only `compare_pages` - see the module
/// doc comment and `handle_tools_call`/`tool_defs_for`.
fn experimental_enabled() -> bool {
    matches!(
        std::env::var("REDLINE_MCP_EXPERIMENTAL").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

/// Maps a socket I/O error to a tool-level JSON error, distinguishing a genuine
/// read/write timeout (`ErrorKind::WouldBlock`/`TimedOut` - the platform-reported kinds
/// for an elapsed `set_read_timeout`/`set_write_timeout`, per std's own documentation)
/// from any other I/O failure, which keeps `fallback_tag`'s existing meaning
/// (`read_failed`/`write_failed`) unchanged.
fn socket_io_err_to_json(e: &io::Error, timeout: Duration, fallback_tag: &str) -> Value {
    if matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ) {
        json!({
            "error": "redline_timeout",
            "detail": format!(
                "redline did not respond within {}s - the connection was dropped. A tool \
                 call may be stuck server-side (known case: compare_pages, see the design \
                 doc's Phase 2b section) rather than merely slow.",
                timeout.as_secs()
            )
        })
    } else {
        json!({ "error": fallback_tag, "detail": e.to_string() })
    }
}

/// Forward one tool call to the running app over the local socket/pipe. A connection
/// failure (app not running, or no document open) surfaces as a clear tool-level error,
/// never a panic and never a silent fallback - design §2's stated requirement.
fn call_bridge(op: &str, arguments: Value, next_call_id: &mut u64) -> Result<Value, Value> {
    let stream = connect().map_err(|e| {
        json!({
            "error": "redline_not_running",
            "detail": format!(
                "could not connect to the redline app ({e}) - is redline running with a document open?"
            )
        })
    })?;

    let id = *next_call_id;
    *next_call_id += 1;
    call_bridge_over_stream(stream, op, arguments, id, socket_timeout())
}

/// Core of [`call_bridge`], generic over any `Read + Write` stream so it is testable
/// against a synthetic socket (see `call_bridge_timeout_tests` below) without touching
/// the real, fixed `redline_lib::rpc::socket_path()` - which could collide with a
/// genuinely running `redline` app during a test run (the exact class of flake named in
/// `CLAUDE.md`'s `$TMPDIR` isolation note).
fn call_bridge_over_stream<S: Read + Write>(
    mut stream: S,
    op: &str,
    arguments: Value,
    call_id: u64,
    timeout: Duration,
) -> Result<Value, Value> {
    let req = RpcRequest {
        id: call_id,
        op: op.to_string(),
        params: arguments,
    };
    let mut payload = serde_json::to_vec(&req)
        .map_err(|e| json!({ "error": "encode_failed", "detail": e.to_string() }))?;
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .map_err(|e| socket_io_err_to_json(&e, timeout, "write_failed"))?;

    let mut reader = io::BufReader::new(stream);
    let line = read_capped_line(&mut reader)
        .map_err(|e| socket_io_err_to_json(&e, timeout, "read_failed"))?
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
    // Set BEFORE boxing - `Box<dyn ReadWrite>` erases the concrete `UnixStream` type,
    // and `set_read_timeout`/`set_write_timeout` aren't part of the `Read + Write`
    // trait surface (PR #99 review finding #1). See `socket_timeout()`'s doc comment
    // for why this exists at all.
    let timeout = socket_timeout();
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    Ok(Box::new(s))
}

#[cfg(windows)]
fn connect() -> io::Result<Box<dyn ReadWrite>> {
    // Windows named pipes are openable via the standard file API once a server
    // instance is listening (CreateFileW under the hood) - see the UNVERIFIED note
    // on redline_lib::rpc's Windows server path; this client side is equally
    // unverified without a Windows target to compile/run against in this session.
    //
    // NOT YET TIMEOUT-PROTECTED (named honestly, not silently skipped): unlike the
    // Unix branch above, `std::fs::File` has no `set_read_timeout`/`set_write_timeout`
    // - a Windows named pipe needs `SetNamedPipeHandleState` or overlapped I/O via
    // `windows-sys`, which this macOS-only session cannot compile or verify (same
    // constraint already named for the Windows server path itself). Follow-up, not
    // fixed here.
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(redline_lib::rpc::WINDOWS_PIPE_NAME)?;
    Ok(Box::new(f))
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// The MCP tool-surface definitions (JSON Schema `inputSchema` per tool) actually
/// advertised by this running process - `tools/list`'s call site. Delegates to
/// [`tool_defs_for`] with the real, environment-read gate.
fn tool_defs() -> Value {
    tool_defs_for(experimental_enabled())
}

/// All defined tools, with `compare_pages` present only when `experimental` is true
/// (PR #99 review finding #2 - see the module doc comment). Split from [`tool_defs`]
/// so the gating behaviour is unit-testable without mutating process-global env state.
fn tool_defs_for(experimental: bool) -> Value {
    let mut tools = json!([
        {
            "name": "list_open_documents",
            "description": "List every document currently open in redline: doc_id, path, title, page_count, whether it's the focused tab (is_active), and whether it has unsaved changes (dirty). Use this (or get_active_document) to obtain a doc_id before calling any other tool.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "open_document",
            "description": "Open a PDF file in redline, or return the existing doc_id if that path is already open. Path must be absolute. Does not prompt for a password - an encrypted PDF with no remembered password is refused.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute path to the PDF file."}
                },
                "required": ["path"]
            }
        },
        {
            "name": "close_document",
            "description": "Close an open document. Refused with a document_dirty error if it has unsaved changes, unless discard_changes is true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "discard_changes": {"type": "boolean", "description": "Close even if there are unsaved changes, discarding them. Defaults to false."}
                },
                "required": ["doc_id"]
            }
        },
        {
            "name": "get_active_document",
            "description": "Get the doc_id, path, page_count, and dirty state of the document currently focused in the redline GUI's tab bar. Refused with a no_active_document error if no document is open/focused.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
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
        },
        {
            "name": "search_document",
            "description": "Search all pages of the open document's PDF text layer for `query` (PDFium text-search API - not markup notes, see search_markups for that). Read-only, no persistence effect. Fails with an 'unknown doc_id' error if the document isn't open.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "query": {"type": "string"},
                    "case_sensitive": {"type": "boolean"},
                    "whole_word": {"type": "boolean"}
                },
                "required": ["doc_id", "query"]
            }
        },
        {
            "name": "open_folder_index",
            "description": "Open (or reopen) the Tantivy full-text index for `root` and start background indexing. Mutates in-app search state only (creates/reuses an on-disk index cache under the app data dir) - never touches any PDF document. Indexing runs asynchronously; poll folder_index_status to see progress. Replaces any previously active folder index - redline holds only one at a time.",
            "inputSchema": {
                "type": "object",
                "properties": { "root": {"type": "string", "description": "Absolute path to the folder to index."} },
                "required": ["root"]
            }
        },
        {
            "name": "search_folder",
            "description": "Search the currently active Tantivy folder index for `query`. Read-only. Requires an index already open for this exact `root` via open_folder_index first - refused with folder_index_root_mismatch if no index is open, or it's for a different folder (redline holds one active folder index at a time, so this never silently searches the wrong folder).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": {"type": "string", "description": "Must match the root passed to the most recent open_folder_index call."},
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "description": "Max hits to return (default 50)."}
                },
                "required": ["root", "query"]
            }
        },
        {
            "name": "folder_index_status",
            "description": "Poll the indexing state (Idle/Indexing/Error) and hit counts of the currently active folder index. Read-only. The result's matches_requested_root is false if `root` differs from the actual active index's folder_path, or nothing is indexed yet - redline holds one active folder index at a time.",
            "inputSchema": {
                "type": "object",
                "properties": { "root": {"type": "string"} }
            }
        },
        {
            "name": "list_scales",
            "description": "List all saved calibration scales for the document. Read-only.",
            "inputSchema": {
                "type": "object",
                "properties": { "doc_id": {"type": "string"} },
                "required": ["doc_id"]
            }
        },
        {
            "name": "add_scale",
            "description": "Add (or replace, by applies_to target) a calibration scale for the document. WRITES IMMEDIATELY to the document's sidecar metadata file - not gated behind save_document, unlike markup create/update/delete. Fails if doc_id is unknown.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "applies_to_page": {"type": "integer", "description": "Zero-based page index. Omit for the document-default scale."},
                    "ratio": {"type": "number", "description": "Real-world units per PDF point."},
                    "unit": {"type": "string"},
                    "label": {"type": "string"},
                    "precision": {"type": "integer", "description": "Decimal places for displayed quantities."}
                },
                "required": ["doc_id", "ratio", "unit", "label", "precision"]
            }
        },
        {
            "name": "delete_scale",
            "description": "Delete a saved scale by id. WRITES IMMEDIATELY to the sidecar metadata file. Returns removed:false (not an error) if the scale_id wasn't found.",
            "inputSchema": {
                "type": "object",
                "properties": { "doc_id": {"type": "string"}, "scale_id": {"type": "string"} },
                "required": ["doc_id", "scale_id"]
            }
        },
        {
            "name": "write_page_measure",
            "description": "Embed a standard PDF /Measure viewport dictionary for a page using a saved scale, so Acrobat/Bluebeam can read the calibration. WRITES THE PDF FILE ON DISK IMMEDIATELY (atomic temp+rename, then reloads the render engine) - there is no separate save step, unlike markup create/update/delete which stay in-memory until save_document is called. Fails if scale_id isn't found for this doc.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "page_idx": {"type": "integer"},
                    "scale_id": {"type": "string"}
                },
                "required": ["doc_id", "page_idx", "scale_id"]
            }
        },
        {
            "name": "export_markup_list",
            "description": "Export the document's markup/takeoff quantity list (XLSX or CSV) to a caller-supplied path. Writes a NEW file at `path`; does not modify the source document. Distinct from export_markup_schedule (which auto-generates its own path next to the source document) only in path-selection - both call the exact same underlying writer and produce identical output for the same input.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "path": {"type": "string", "description": "Absolute destination file path."},
                    "format": {"type": "string", "enum": ["Xlsx", "Csv"]}
                },
                "required": ["doc_id", "path", "format"]
            }
        },
        {
            "name": "rotate_page",
            "description": "Rotate a page by `degrees` (multiple of 90, cumulative). WRITES THE PDF FILE ON DISK IMMEDIATELY (atomic temp+rename, then reloads the render engine) - unlike markup edits, page ops have no in-memory staging and no separate save step. No document-level lock concept exists in this codebase to check (only per-markup Locked/LockedContents flags, which don't apply to page structure).",
            "inputSchema": {
                "type": "object",
                "properties": { "doc_id": {"type": "string"}, "page_idx": {"type": "integer"}, "degrees": {"type": "integer"} },
                "required": ["doc_id", "page_idx", "degrees"]
            }
        },
        {
            "name": "delete_page",
            "description": "Delete a page (0-based index). WRITES THE PDF FILE ON DISK IMMEDIATELY, same as rotate_page - no staging, no separate save step, no document-level lock check exists. Fails if the document has only one page.",
            "inputSchema": {
                "type": "object",
                "properties": { "doc_id": {"type": "string"}, "page_idx": {"type": "integer"} },
                "required": ["doc_id", "page_idx"]
            }
        },
        {
            "name": "reorder_pages",
            "description": "Reorder pages. `new_order` must be a permutation of 0..page_count (0-based). WRITES THE PDF FILE ON DISK IMMEDIATELY, same as rotate_page - no staging, no separate save step, no document-level lock check exists.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "new_order": {"type": "array", "items": {"type": "integer"}}
                },
                "required": ["doc_id", "new_order"]
            }
        },
        {
            "name": "insert_blank_page",
            "description": "Insert a blank page at position `at` (0-based; at == page_count appends). WRITES THE PDF FILE ON DISK IMMEDIATELY, same as rotate_page - no staging, no separate save step, no document-level lock check exists.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "at": {"type": "integer"},
                    "width": {"type": "number"},
                    "height": {"type": "number"}
                },
                "required": ["doc_id", "at", "width", "height"]
            }
        },
        {
            "name": "compare_pages",
            "description": "Two-tier diff (text-layer + pixel) between one page of path_a and one page of path_b. Read-only. UNLIKE every other tool here, this takes raw file paths, not a doc_id - it does not require either document to be open in redline at all. Returns aggregate stats only (text_char_match, text_delta_count, text_rms_delta_pts, pixel_passed, changed_pct, max_pixel_delta, render_dpi) - the base64 PNG diff overlay the underlying command also produces is deliberately omitted from the MCP response.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path_a": {"type": "string"},
                    "path_b": {"type": "string"},
                    "page_a": {"type": "integer"},
                    "page_b": {"type": "integer"},
                    "dpi": {"type": "number", "description": "Render DPI for the pixel diff. Default 150."},
                    "pixel_tolerance": {"type": "integer", "description": "Per-channel delta (0-255) that counts as 'same'. Default 5."}
                },
                "required": ["path_a", "path_b", "page_a", "page_b"]
            }
        },
        {
            "name": "redact_document",
            "description": "Apply redactions (explicit rectangular regions and/or existing /Subtype /Redact annotations) by overlaying solid-black image XObjects. WRITES THE PDF FILE ON DISK IMMEDIATELY (atomic temp+rename, then reloads the render engine). Irreversible once written - the redacted content is overlaid, not removed from the underlying stream (v1 rasterize-the-region safe floor, not vector redaction).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string"},
                    "regions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "page_index": {"type": "integer"},
                                "x": {"type": "number"},
                                "y": {"type": "number"},
                                "width": {"type": "number"},
                                "height": {"type": "number"}
                            },
                            "required": ["page_index", "x", "y", "width", "height"]
                        }
                    },
                    "apply_annots": {"type": "boolean", "description": "Also consume every /Subtype /Redact annotation already on the document."}
                },
                "required": ["doc_id"]
            }
        },
        {
            "name": "save_document_as",
            "description": "Persist all in-memory markup changes to a NEW file at `path` and switch the open doc_id to point at it (save_document alone cannot write a copy - it always writes back to the currently-open path). The document's original file is left untouched.",
            "inputSchema": {
                "type": "object",
                "properties": { "doc_id": {"type": "string"}, "path": {"type": "string"} },
                "required": ["doc_id", "path"]
            }
        }
    ]);
    if !experimental {
        if let Value::Array(arr) = &mut tools {
            arr.retain(|t| t["name"] != "compare_pages");
        }
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tools that legitimately have no required `doc_id` - see each tool's own
    /// description for why: `list_open_documents`/`get_active_document` are how a
    /// client discovers a doc_id in the first place (no doc_id to require yet),
    /// `open_document` takes a `path`, not a `doc_id` (the tool call itself produces
    /// one), `open_folder_index`/`search_folder`/`folder_index_status` operate on a
    /// folder `root` rather than an open document (Phase 2b), and `compare_pages`
    /// takes `path_a`/`path_b` directly - it wraps a Tauri command that never touches
    /// `AppState`/`MarkupStore` and does not require either document to be open.
    const TOOLS_WITHOUT_A_REQUIRED_DOC_ID: &[&str] = &[
        "list_open_documents",
        "open_document",
        "get_active_document",
        "open_folder_index",
        "search_folder",
        "folder_index_status",
        "compare_pages",
    ];

    #[test]
    fn tool_defs_names_all_thirty_tools_across_phase_1_2a_and_2b() {
        // Uses `tool_defs_for(true)` - the full schema shape, independent of the
        // `compare_pages` opt-in gate (PR #99 review finding #2) - see
        // `tool_defs_default_excludes_compare_pages` for the gate itself.
        let defs = tool_defs_for(true);
        let names: Vec<&str> = defs
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for expected in [
            // Phase 2a (2026-09-03): document lifecycle
            "list_open_documents",
            "open_document",
            "close_document",
            "get_active_document",
            // Phase 1 (2026-09-01)
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
            // Phase 2b (2026-09-03): search
            "search_document",
            "open_folder_index",
            "search_folder",
            "folder_index_status",
            // Phase 2b: takeoff
            "list_scales",
            "add_scale",
            "delete_scale",
            "write_page_measure",
            "export_markup_list",
            // Phase 2b: page ops
            "rotate_page",
            "delete_page",
            "reorder_pages",
            "insert_blank_page",
            // Phase 2b: compare + docops + save-as
            "compare_pages",
            "redact_document",
            "save_document_as",
        ] {
            assert!(names.contains(&expected), "missing tool: {expected}");
        }
        assert_eq!(names.len(), 30);
    }

    #[test]
    fn tool_defs_default_excludes_compare_pages_and_lists_twenty_nine_tools() {
        // PR #99 review finding #2: compare_pages hangs indefinitely today (a
        // pre-existing double-PDFium-binding conflict) and must not be advertised to a
        // client by default.
        let defs = tool_defs_for(false);
        let names: Vec<&str> = defs
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(
            !names.contains(&"compare_pages"),
            "compare_pages must be opt-in (REDLINE_MCP_EXPERIMENTAL=1), not listed by default"
        );
        assert_eq!(names.len(), 29);
    }

    #[test]
    fn tool_defs_experimental_enabled_includes_compare_pages() {
        let defs = tool_defs_for(true);
        let names: Vec<&str> = defs
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"compare_pages"));
        assert_eq!(names.len(), 30);
    }

    #[test]
    fn every_tool_def_not_named_in_the_doc_id_free_list_requires_doc_id() {
        let defs = tool_defs_for(true);
        for tool in defs.as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            if TOOLS_WITHOUT_A_REQUIRED_DOC_ID.contains(&name) {
                continue;
            }
            let required = tool["inputSchema"]["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} has no required array"));
            assert!(
                required.iter().any(|r| r == "doc_id"),
                "{name} must require doc_id"
            );
        }
    }

    #[test]
    fn doc_id_free_tools_really_do_not_require_it() {
        let defs = tool_defs_for(true);
        for &name in TOOLS_WITHOUT_A_REQUIRED_DOC_ID {
            let tool = defs
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("tool {name} not found in tool_defs"));
            let required = tool["inputSchema"]["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !required.iter().any(|r| r == "doc_id"),
                "{name} is listed as doc_id-free but its schema requires doc_id"
            );
        }
    }

    #[test]
    fn handle_tools_call_missing_params_is_a_protocol_error_not_a_tool_error() {
        let resp = handle_tools_call(json!(1), None, &mut 1, false);
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn handle_tools_call_refuses_compare_pages_when_experimental_disabled() {
        let params = json!({
            "name": "compare_pages",
            "arguments": {"path_a": "/a.pdf", "path_b": "/b.pdf", "page_a": 0, "page_b": 0}
        });
        let resp = handle_tools_call(json!(1), Some(&params), &mut 1, false);
        // A tool-level refusal (isError:true), NOT a JSON-RPC protocol error - the call
        // itself is well-formed, the tool is simply disabled. Confirms this refusal
        // never reaches `call_bridge` (no socket touched, no real app needed).
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("experimental_tool_disabled"), "got: {text}");
    }

    #[test]
    fn handle_tools_call_allows_compare_pages_when_experimental_enabled() {
        // Enabled but no real app running -> falls through to call_bridge's own
        // connection-failure path (already covered by
        // call_bridge_connection_failure_is_a_structured_tool_error), NOT the
        // experimental_tool_disabled refusal - proves the gate actually gates.
        let params = json!({
            "name": "compare_pages",
            "arguments": {"path_a": "/a.pdf", "path_b": "/b.pdf", "page_a": 0, "page_b": 0}
        });
        let resp = handle_tools_call(json!(1), Some(&params), &mut 1, true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("experimental_tool_disabled"), "got: {text}");
    }

    #[test]
    fn call_bridge_connection_failure_is_a_structured_tool_error() {
        // No redline app is running in this test process, so this must fail cleanly
        // with a labelled error rather than panicking - design §2's "never a silent
        // fallback" requirement, exercised from the client side.
        let err = call_bridge("list_markups", json!({"doc_id": "d1"}), &mut 1).unwrap_err();
        assert_eq!(err["error"], "redline_not_running");
    }

    mod call_bridge_timeout_tests {
        use super::*;
        use std::os::unix::net::UnixStream;
        use std::time::Instant;

        #[test]
        fn call_bridge_over_stream_times_out_on_a_socket_that_never_replies() {
            // PR #99 review finding #1: before this fix, `redline-mcp` had no
            // read/write timeout at all, so a server-side call that never responds
            // (compare_pages today - see the module doc comment) hung this call - and
            // every later one, since this binary's loop is single-threaded and
            // synchronous - forever, silently. `_server` is a connected pair endpoint
            // that is held open but never written to: not a connection failure
            // (`redline_not_running`), a genuinely-stuck peer.
            let (client, _server) = UnixStream::pair().expect("unix socket pair");
            let timeout = Duration::from_millis(300);
            client
                .set_read_timeout(Some(timeout))
                .expect("set_read_timeout");
            client
                .set_write_timeout(Some(timeout))
                .expect("set_write_timeout");

            let started = Instant::now();
            let err = call_bridge_over_stream(
                client,
                "compare_pages",
                json!({"path_a": "/a.pdf", "path_b": "/b.pdf", "page_a": 0, "page_b": 0}),
                1,
                timeout,
            )
            .expect_err("a socket that never replies must time out, not hang forever");
            let elapsed = started.elapsed();

            assert_eq!(err["error"], "redline_timeout");
            assert!(
                err["detail"].as_str().unwrap().contains("did not respond"),
                "got: {err:?}"
            );
            assert!(
                elapsed < Duration::from_secs(5),
                "must return promptly once the timeout elapses (set to {timeout:?}), took {elapsed:?}"
            );
        }

        #[test]
        fn socket_io_err_to_json_distinguishes_timeout_from_other_io_errors() {
            let timeout_err = io::Error::from(io::ErrorKind::WouldBlock);
            let v = socket_io_err_to_json(&timeout_err, Duration::from_secs(7), "read_failed");
            assert_eq!(v["error"], "redline_timeout");
            assert!(v["detail"].as_str().unwrap().contains("7s"));

            let other_err = io::Error::from(io::ErrorKind::ConnectionReset);
            let v = socket_io_err_to_json(&other_err, Duration::from_secs(7), "read_failed");
            assert_eq!(v["error"], "read_failed");
        }
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
