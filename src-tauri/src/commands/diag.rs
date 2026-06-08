//! Diagnostic commands for the in-app bench/FPS overlay (spec §20 GUI metrics).
//!
//! The §20 acceptance metrics that the headless harness cannot measure —
//! interactive pan frame-time, zoom-settle, and live process RSS — are captured
//! in the running GUI. Frame-time and zoom-settle are measured in the webview
//! (rAF deltas); process RSS must come from Rust, hence `process_rss_mb`.

/// Returns the path in the `REDLINE_OPEN_PDF` env var, if set, so the frontend can
/// auto-open it on startup (§20 GUI smoke / floor-machine runbook). `None` otherwise
/// → the app shows its normal empty state and waits for the Open dialog.
#[tauri::command]
pub fn auto_open_path() -> Option<String> {
    std::env::var("REDLINE_OPEN_PDF")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Current process resident set size (RSS) in megabytes.
///
/// Read via `ps -o rss=` on macOS/Linux (KB) and via the Windows toolhelp API
/// fallback through `tasklist` — but for the dev/floor smoke we rely on `ps`
/// (macOS) and document the Windows reading in the runbook. Returns 0.0 if the
/// platform readout fails (overlay then shows "n/a").
#[tauri::command]
pub fn process_rss_mb() -> f64 {
    rss_mb()
}

#[cfg(not(target_os = "windows"))]
fn rss_mb() -> f64 {
    let pid = std::process::id();
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.trim().parse::<f64>().map(|kb| kb / 1024.0).unwrap_or(0.0)
        }
        Err(_) => 0.0,
    }
}

#[cfg(target_os = "windows")]
fn rss_mb() -> f64 {
    // `tasklist /FI "PID eq <pid>" /FO CSV /NH` → last CSV field is "1,234 K".
    let pid = std::process::id();
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            // Last quoted field, e.g. "12,345 K"
            s.rsplit(',')
                .next()
                .map(|f| {
                    f.trim()
                        .trim_matches('"')
                        .replace([',', 'K', ' '], "")
                        .parse::<f64>()
                        .map(|kb| kb / 1024.0)
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0)
        }
        Err(_) => 0.0,
    }
}
