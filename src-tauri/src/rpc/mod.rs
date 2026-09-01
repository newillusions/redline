//! Local RPC bridge for the redline MCP companion process (MCP server design,
//! 2026-09-01, `docs/superpowers/specs/2026-09-01-mcp-server-design.md`, §2 + §5).
//!
//! Loopback-only transport between the running GUI app (this module, server side) and
//! the `redline-mcp` stdio binary (client side, `src/bin/redline_mcp.rs`): a Unix
//! domain socket on macOS/Linux, filesystem-permission gated (`0600`) - never a network
//! listener. `redline-mcp` is a pure protocol translator (MCP JSON-RPC over stdio <->
//! this module's line-delimited JSON over the socket, `protocol::RpcRequest`/
//! `RpcResponse`); every operation here calls the exact same `commands::*`/
//! `MarkupStore` logic the Svelte frontend already invokes via Tauri `invoke`
//! (`dispatch::dispatch`), so there is exactly one process that ever mutates
//! `MarkupStore` or calls `document::save` - design §2's single-writer-correctness
//! argument, which is the whole reason this is a companion bridge and not a standalone
//! file-mutating binary.
//!
//! v1 simplification, named plainly: the design's per-document socket lifecycle
//! ("started when a document is opened, torn down when the last document closes") is
//! not implemented - the listener runs for the app's full lifetime instead, started
//! once from `setup()`. The security posture is identical either way (filesystem-
//! permission gated), and an MCP call against a `doc_id` that isn't open already gets
//! the clear "unknown doc_id" refusal the design requires (design §2: "never a silent
//! fallback to direct file access"), via the exact same path GUI commands already use -
//! so the stated requirement holds without doc-lifecycle-tied socket management.

mod dispatch;
pub mod protocol;
mod tools;

use std::path::PathBuf;

use tauri::AppHandle;

/// Directory the socket lives inside, created `0700` (owner-only) at creation time -
/// see [`ensure_private_dir`]. Closes a TOCTOU window a bare `bind()` + later
/// `chmod()` on the socket FILE alone would leave open (reviewer finding on PR #92,
/// 2026-09-01): a connection landing in the gap between bind and chmod, or - on a
/// Linux host with `$TMPDIR` unset, where `std::env::temp_dir()` falls back to the
/// world-writable `/tmp` - another local user simply being able to see or open the
/// socket at all before its own permissions are restricted. Once this directory is
/// `0700`, nothing outside this user can resolve a path into it at all, regardless of
/// what the socket file's own permissions are for the brief moment before its chmod
/// runs.
fn socket_dir() -> PathBuf {
    std::env::temp_dir().join("redline-mcp")
}

/// Loopback-only socket path both this module and `redline-mcp` compute independently
/// (the bin has no Tauri context, so this must not depend on one). `std::env::temp_dir`
/// is a std-library primitive both binaries call identically, and is per-user on macOS
/// (`confstr(_CS_DARWIN_USER_TEMP_DIR)`) and conventionally per-user on Linux via
/// `$TMPDIR`. v1 assumes a single redline instance per user session - a second
/// concurrent instance would collide on this path - the same single-instance
/// assumption the design left open for multi-document sessions (§3 open question 3)
/// and does not resolve for multi-INSTANCE either; named here, not solved.
pub fn socket_path() -> PathBuf {
    socket_dir().join("mcp.sock")
}

/// Ensure `dir` exists, is not a symlink (the classic shared-`/tmp` attack: another
/// local user pre-plants a symlink at the expected path), and is `0700`.
/// `DirBuilder::mode(0o700)` sets the permission AT CREATION (intersected with umask,
/// which can only ever narrow a mode with no group/other bits set - never widen it), so
/// there is no window during which a freshly-created directory is more permissive than
/// `0700`. If the directory already exists, its permissions are tightened rather than
/// trusted (it may be left over from an older version of this code); if that `chmod`
/// fails (e.g. another user owns it), this errors out and the caller aborts startup
/// rather than binding into a directory it does not control.
#[cfg(unix)]
fn ensure_private_dir(dir: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            anyhow::bail!(
                "{} is a symlink - refusing to use it as the MCP socket directory",
                dir.display()
            );
        }
        Ok(_) => {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Err(_) => {
            std::fs::DirBuilder::new().mode(0o700).create(dir)?;
        }
    }
    Ok(())
}

/// Named-pipe endpoint for the Windows build (see [`start`] below) - a fixed name in
/// the pipe namespace, not a filesystem path, since Windows named pipes don't live
/// under `%TEMP%`.
pub const WINDOWS_PIPE_NAME: &str = r"\\.\pipe\redline-mcp";

/// Start the RPC listener as a background task. Call once from `setup()`, after
/// `app.manage(AppState { .. })`. Errors are logged, not fatal - the GUI must not fail
/// to start because the MCP bridge couldn't bind (e.g. a permissions problem on
/// `$TMPDIR` should degrade to "no MCP" rather than "no app"; a stale socket file from
/// an unclean previous exit is handled by [`run_unix`]'s own cleanup).
#[cfg(unix)]
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_unix(app).await {
            log::error!("redline-mcp RPC bridge failed to start: {e:#}");
        }
    });
}

#[cfg(unix)]
async fn run_unix(app: AppHandle) -> anyhow::Result<()> {
    use tokio::net::UnixListener;

    ensure_private_dir(&socket_dir())?;
    let path = socket_path();
    // Remove a stale socket from an unclean previous exit - bind fails with
    // AddrInUse otherwise even though nothing is listening on it. Safe to do
    // unconditionally now: the containing directory is already 0700, so nothing but
    // this user could have raced a replacement into place since the last run.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        // 0600: owner read/write only (design §5) - defense in depth on top of the
        // containing directory's 0700, which is what actually closes the TOCTOU window
        // (see ensure_private_dir's doc comment) rather than this chmod alone.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    log::info!("redline-mcp RPC bridge listening on {}", path.display());

    loop {
        let (stream, _addr) = listener.accept().await?;
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = handle_connection(app, stream).await {
                log::warn!("redline-mcp RPC connection error: {e:#}");
            }
        });
    }
}

#[cfg(unix)]
async fn handle_connection(app: AppHandle, stream: tokio::net::UnixStream) -> anyhow::Result<()> {
    serve_lines(app, stream).await
}

/// Read one `\n`-delimited line, refusing to grow the buffer past
/// [`protocol::MAX_LINE_BYTES`] - an unterminated or malicious frame gets a clean error
/// instead of unbounded memory growth (reviewer finding on PR #92, 2026-09-01). Reads a
/// byte at a time via `AsyncReadExt::read` against `reader` - when `reader` is a
/// `BufReader`, each call is served from its internal buffer rather than costing a
/// syscall per byte, so this stays cheap despite the naive-looking loop. `Ok(None)` on
/// clean EOF (no bytes read at all); a trailing `\r` (CRLF) is tolerated and stripped.
async fn read_capped_line<R>(reader: &mut R) -> anyhow::Result<Option<String>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte).await?;
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
        if buf.len() > protocol::MAX_LINE_BYTES {
            anyhow::bail!(
                "line exceeds MAX_LINE_BYTES ({}) with no terminator",
                protocol::MAX_LINE_BYTES
            );
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(Some(String::from_utf8(buf)?))
}

/// Shared per-connection loop: read one line-delimited request, dispatch it, write one
/// line-delimited response, repeat until the peer disconnects. Generic over any
/// `AsyncRead + AsyncWrite` stream so the Unix and Windows transports share this exact
/// logic instead of duplicating it.
async fn serve_lines<S>(app: AppHandle, stream: S) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncWriteExt, BufReader};

    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    while let Some(line) = read_capped_line(&mut reader).await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<protocol::RpcRequest>(&line) {
            Ok(req) => {
                let id = req.id;
                match dispatch::dispatch(&app, req).await {
                    Ok(result) => protocol::RpcResponse {
                        id,
                        result: Some(result),
                        error: None,
                    },
                    Err(err) => protocol::RpcResponse {
                        id,
                        result: None,
                        error: Some(err),
                    },
                }
            }
            Err(e) => protocol::RpcResponse {
                id: 0,
                result: None,
                error: Some(serde_json::json!({ "error": "bad_request", "detail": e.to_string() })),
            },
        };
        let mut out = serde_json::to_vec(&response)?;
        out.push(b'\n');
        writer.write_all(&out).await?;
    }
    Ok(())
}

/// Windows named-pipe bridge — implements the same wire protocol as [`run_unix`] via
/// the shared [`serve_lines`] loop, but is **UNVERIFIED**: this build/test environment
/// is macOS-only (no Windows target compiled or run against here), so this path has
/// never been compiled, let alone exercised. Written to the documented
/// `tokio::net::windows::named_pipe` API shape from memory. Falls short of the design's
/// full requirement (§5: "a named pipe with an ACL restricted to the current user's
/// SID") - `ServerOptions` here uses tokio's defaults rather than a custom security
/// descriptor restricting the ACL to the current user's SID explicitly, because that
/// needs `windows-sys` calls this session cannot verify without a Windows target.
/// Flagged as a follow-up, not implemented. Treat this whole function as a draft to be
/// compiled and fixed on Windows (the redline Windows testbench,
/// `reference_redline_windows_testbench.md`, is how this repo verifies Windows-only
/// code in practice), not as shipped/proven coverage.
#[cfg(windows)]
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_windows(app).await {
            log::error!("redline-mcp RPC bridge failed to start: {e:#}");
        }
    });
}

#[cfg(windows)]
async fn run_windows(app: AppHandle) -> anyhow::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    loop {
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .create(WINDOWS_PIPE_NAME)?;
        server.connect().await?;
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = serve_lines(app, server).await {
                log::warn!("redline-mcp RPC connection error: {e:#}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_is_under_a_private_subdirectory_of_the_process_temp_dir() {
        let p = socket_path();
        assert!(p.starts_with(socket_dir()));
        assert!(socket_dir().starts_with(std::env::temp_dir()));
        assert_eq!(p.file_name().unwrap(), "mcp.sock");
    }

    #[test]
    fn windows_pipe_name_is_in_the_pipe_namespace() {
        assert!(WINDOWS_PIPE_NAME.starts_with(r"\\.\pipe\"));
    }

    #[cfg(unix)]
    mod ensure_private_dir_tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn mode_bits(path: &std::path::Path) -> u32 {
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        }

        #[test]
        fn creates_a_fresh_directory_as_0700() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("mcp-private");
            assert!(!dir.exists());

            ensure_private_dir(&dir).unwrap();

            assert!(dir.is_dir());
            assert_eq!(mode_bits(&dir), 0o700);
        }

        #[test]
        fn tightens_an_existing_directory_left_loose_by_an_older_version() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("mcp-private");
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(mode_bits(&dir), 0o755);

            ensure_private_dir(&dir).unwrap();

            assert_eq!(mode_bits(&dir), 0o700);
        }

        #[test]
        fn is_idempotent_on_an_already_private_directory() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("mcp-private");
            ensure_private_dir(&dir).unwrap();
            ensure_private_dir(&dir).unwrap(); // must not error the second time
            assert_eq!(mode_bits(&dir), 0o700);
        }

        #[test]
        fn refuses_a_symlinked_path_the_classic_shared_tmp_attack() {
            let tmp = tempfile::tempdir().unwrap();
            let real_target = tmp.path().join("attacker-controlled");
            std::fs::create_dir(&real_target).unwrap();
            let planted_symlink = tmp.path().join("mcp-private-symlink");
            std::os::unix::fs::symlink(&real_target, &planted_symlink).unwrap();

            let err = ensure_private_dir(&planted_symlink).unwrap_err();
            assert!(err.to_string().contains("symlink"), "got: {err}");
        }
    }

    mod read_capped_line_tests {
        use super::*;
        use std::io::Cursor;

        #[tokio::test]
        async fn reads_a_normal_newline_terminated_line() {
            let mut r = Cursor::new(b"hello\nworld\n".to_vec());
            assert_eq!(
                read_capped_line(&mut r).await.unwrap(),
                Some("hello".to_string())
            );
            assert_eq!(
                read_capped_line(&mut r).await.unwrap(),
                Some("world".to_string())
            );
            assert_eq!(read_capped_line(&mut r).await.unwrap(), None);
        }

        #[tokio::test]
        async fn strips_a_trailing_crlf() {
            let mut r = Cursor::new(b"hello\r\n".to_vec());
            assert_eq!(
                read_capped_line(&mut r).await.unwrap(),
                Some("hello".to_string())
            );
        }

        #[tokio::test]
        async fn returns_an_unterminated_final_line_before_eof() {
            let mut r = Cursor::new(b"no newline at all".to_vec());
            assert_eq!(
                read_capped_line(&mut r).await.unwrap(),
                Some("no newline at all".to_string())
            );
        }

        #[tokio::test]
        async fn clean_eof_with_no_bytes_returns_none() {
            let mut r = Cursor::new(Vec::<u8>::new());
            assert_eq!(read_capped_line(&mut r).await.unwrap(), None);
        }

        #[tokio::test]
        async fn refuses_a_line_exceeding_max_line_bytes_instead_of_growing_unbounded() {
            // One byte over the cap, no newline - must error, not allocate without bound.
            let oversized = vec![b'a'; protocol::MAX_LINE_BYTES + 1];
            let mut r = Cursor::new(oversized);
            let err = read_capped_line(&mut r).await.unwrap_err();
            assert!(err.to_string().contains("MAX_LINE_BYTES"), "got: {err}");
        }

        #[tokio::test]
        async fn a_line_exactly_at_the_cap_with_a_terminator_is_accepted() {
            let mut exact = vec![b'a'; protocol::MAX_LINE_BYTES];
            exact.push(b'\n');
            let mut r = Cursor::new(exact);
            let line = read_capped_line(&mut r).await.unwrap().unwrap();
            assert_eq!(line.len(), protocol::MAX_LINE_BYTES);
        }
    }
}
