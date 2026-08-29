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
//!
//! ## Why the logging calls below are wrapped in `catch_unwind`
//!
//! Root-caused 2026-08-29 from a real SIGABRT (`redline-2026-08-28-163301.ips`) hit
//! while running under `@wdio/tauri-service`'s child_process spawn (`npm run e2e`),
//! never seen from a normal Terminal launch. `fern` 0.7.1 (the backend
//! `tauri-plugin-log` uses) panics internally as a last resort: when writing a log
//! record to its primary output fails, `Dispatch::finish_logging` -> `Output::log`
//! calls `backup_logging(record, &error)`
//! (`fern-0.7.1/src/log_impl.rs:883-908`), which tries to report the failure via
//! `write!(io::stderr(), ...)` - and **panics** if that stderr write also fails.
//! `finish_logging` loops over every output with no error containment, so this is
//! not specific to one target; the sole exit if the write fails is fern's own panic.
//!
//! Under WDIO's piped-stdio child process, a write to stdout can fail with EPIPE
//! (e.g. the parent no longer reading it) - which is exactly the trigger: fern's
//! primary write fails, its stderr fallback write fails too (same broken pipe),
//! fern panics (panic #1). That panic reaches this hook, which used to call
//! `log::error!` unconditionally - routing straight back through the same broken
//! fern `Dispatch` - so the SAME failure recurs and fern panics again (panic #2),
//! this time while already unwinding panic #1. A panic raised while a thread is
//! already panicking is "panic while panicking", which Rust aborts the process for
//! unconditionally, regardless of the `panic` profile setting - hence the SIGABRT.
//! (Note: `eprintln!`/`println!` have the identical footgun - both `unwrap()` their
//! write result internally - so simply reordering to try stderr first does not fix
//! this; a broken pipe makes `eprintln!` panic too.)
//!
//! The fix is not "create the log directory" (a red herring here: fern's own
//! internal fallback panics regardless of which output/target is configured, since
//! `finish_logging`'s loop aborts the whole dispatch on the first panicking output)
//! but making this hook's own two report-the-panic attempts individually
//! non-escalating: each is wrapped in `catch_unwind` and its result discarded, so a
//! nested failure while the process's stdio is broken is merely unreported, never a
//! second, process-ending panic.

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

/// Run a closure that may itself panic (e.g. `log::error!` re-entering a `fern`
/// `Dispatch` whose only failure path is an internal panic - see the module docs),
/// without letting that panic escape. Panicking again while a thread is already
/// panicking is unconditionally fatal (`process::abort`, ignoring any `panic`
/// profile setting) - `catch_unwind` is what stands between a broken logging sink
/// and a crashed app. The closure's own panic message is deliberately not
/// recovered further; there is nothing more useful this hook can do once both the
/// primary report attempt AND the failure it was reacting to have failed.
fn run_without_escalating<F: FnOnce()>(f: F) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
}

/// Install the panic hook. Call once, as early as possible in `run()` - before the
/// render thread spawns, so a panic there is caught too.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| l.to_string());
        let payload = payload_to_string(info.payload());
        let message = format_panic_message(&payload, location.as_deref());
        run_without_escalating(|| log::error!("{message}"));
        run_without_escalating(|| eprintln!("{message}"));
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

    /// Simulates fern's `backup_logging` (or a broken-pipe `eprintln!`) itself
    /// panicking while the panic hook is already handling an earlier panic -
    /// exactly the SIGABRT mechanism this module's docs describe. Reaching the
    /// final line at all IS the proof: an unwrapped nested panic here would abort
    /// the whole test binary, not just fail this one test.
    #[test]
    fn run_without_escalating_swallows_a_nested_panic() {
        run_without_escalating(|| panic!("simulated nested failure, e.g. broken stdio"));
        // Process is still alive and the test thread is still unwind-clean.
    }

    #[test]
    fn run_without_escalating_runs_a_successful_closure_normally() {
        let mut ran = false;
        run_without_escalating(|| ran = true);
        assert!(ran, "a non-panicking closure must still execute normally");
    }

    /// The real-world trigger: a panic (panic #1) is being handled while a `Drop`
    /// impl running during its unwind ALSO panics (panic #2, simulating fern's
    /// `backup_logging` panicking a second time on the same broken pipe). Without
    /// `run_without_escalating`'s `catch_unwind`, panic-while-panicking would abort
    /// the process; with it, the outer panic still unwinds normally and the spawned
    /// thread's `join()` observes a clean `Err`, proving the abort path is closed.
    #[test]
    fn nested_panic_during_unwind_does_not_abort_the_process() {
        let result = std::thread::spawn(|| {
            std::panic::catch_unwind(|| {
                struct Guard;
                impl Drop for Guard {
                    fn drop(&mut self) {
                        // Runs during panic #1's unwind - simulates the hook's own
                        // log::error!/eprintln! failing a second time.
                        run_without_escalating(|| panic!("simulated re-entrant sink failure"));
                    }
                }
                let _guard = Guard;
                panic!("original panic");
            })
        })
        .join()
        .expect("spawned thread must not abort or otherwise fail to join");
        assert!(
            result.is_err(),
            "the original panic must still be observed as Err"
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
