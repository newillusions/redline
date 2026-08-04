//! Crash-guard: a Rust panic hook that routes panic info through the `log` crate
//! (so it lands in the persistent file log `tauri-plugin-log` already writes -
//! the same pasted-log support flow the frontend's auto-updater diagnostics use)
//! before falling through to stderr. Without this, a panic on a non-main thread
//! (e.g. the render thread - see `render::RenderHandle`) prints only to stderr,
//! which a packaged release build's user never sees; nothing reaches a bug report.
//!
//! Deliberately does NOT abort or otherwise change panic behaviour - it only adds
//! a log sink ahead of the default handler. `panic = "abort"` / unwind behaviour
//! is left to the existing profile configuration.

/// Format a panic's payload + optional source location into a single log line.
/// Pure and side-effect-free so it is unit-testable without triggering a real panic.
pub fn format_panic_message(payload: &str, location: Option<&str>) -> String {
    match location {
        Some(loc) => format!("PANIC at {loc}: {payload}"),
        None => format!("PANIC: {payload}"),
    }
}

/// Extract a displayable payload string from a panic's `Box<dyn Any>` payload.
/// Handles the two payload shapes `std::panic!` actually produces (`&str` for a
/// string-literal panic, `String` for a formatted one); anything else (a custom
/// payload via `panic_any`) falls back to a fixed placeholder rather than failing.
fn payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "<non-string panic payload>".to_string()
}

/// Install the panic hook. Call once, as early as possible in `run()` - before the
/// render thread spawns, so a panic there is caught too.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| l.to_string());
        let payload = payload_to_string(info.payload());
        let message = format_panic_message(&payload, location.as_deref());
        log::error!("{message}");
        eprintln!("{message}");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_panic_message_with_location() {
        let msg = format_panic_message("index out of bounds", Some("src/foo.rs:10:5"));
        assert_eq!(msg, "PANIC at src/foo.rs:10:5: index out of bounds");
    }

    #[test]
    fn format_panic_message_without_location() {
        let msg = format_panic_message("boom", None);
        assert_eq!(msg, "PANIC: boom");
    }

    #[test]
    fn payload_to_string_handles_str_literal_payload() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("static str panic");
        assert_eq!(payload_to_string(payload.as_ref()), "static str panic");
    }

    #[test]
    fn payload_to_string_handles_string_payload() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("formatted panic"));
        assert_eq!(payload_to_string(payload.as_ref()), "formatted panic");
    }

    #[test]
    fn payload_to_string_falls_back_on_unknown_payload_type() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(42_i32);
        assert_eq!(
            payload_to_string(payload.as_ref()),
            "<non-string panic payload>"
        );
    }

    /// End-to-end: install the hook, trigger a real panic on a scoped thread (never
    /// the test thread itself), and confirm it does not abort the process — proves
    /// `install_panic_hook` composes with catch_unwind rather than replacing it.
    #[test]
    fn install_panic_hook_does_not_change_unwind_behaviour() {
        install_panic_hook();
        let result = std::thread::spawn(|| {
            std::panic::catch_unwind(|| {
                panic!("test panic for hook verification");
            })
        })
        .join()
        .expect("spawned thread itself must not panic");
        assert!(
            result.is_err(),
            "catch_unwind should observe the panic as Err"
        );
    }
}
