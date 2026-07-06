//! Application settings - local user preferences (spec §15 extension).
//!
//! Stores a single JSON object of user-level preferences: theme, default
//! tool, measurement unit, author display name, last window geometry, and
//! recent markup colors. Unlike `recent_docs` (an MRU list), this is a
//! single object with a per-field default, so old settings files stay
//! loadable as new fields are added - a missing field falls back to its
//! default instead of failing the whole parse.
//!
//! ## Storage
//! One JSON file: `<app-data-dir>/settings.json`.
//! Atomic write (temp + rename) to survive a crash mid-write.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Maximum number of entries kept in `recent_colors`.
pub const MAX_RECENT_COLORS: usize = 8;

fn default_theme() -> String {
    "dark".to_string()
}

fn default_measurement_unit() -> String {
    "m".to_string()
}

/// Last known main-window geometry, restored on next launch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LastWindowState {
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

/// User-level application preferences.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppSettings {
    /// UI theme: "dark" | "light" | "system".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Tool kind selected by default when a new document tab opens
    /// (matches `ToolKind` string values in `markup-store.svelte`; `None` = last-used).
    #[serde(default)]
    pub default_tool: Option<String>,
    /// Measurement unit for new calibrations: "mm" | "m" | "km" | "in" | "ft".
    #[serde(default = "default_measurement_unit")]
    pub measurement_unit: String,
    /// Display name stamped on new markups/comments (spec §6 workflow fields).
    #[serde(default)]
    pub author_name: String,
    /// Last main-window geometry; `None` until the window has been resized/closed once.
    #[serde(default)]
    pub last_window: Option<LastWindowState>,
    /// Most-recently-used markup colors, newest first, capped at `MAX_RECENT_COLORS`.
    #[serde(default)]
    pub recent_colors: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            default_tool: None,
            measurement_unit: default_measurement_unit(),
            author_name: String::new(),
            last_window: None,
            recent_colors: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Pure logic (testable without filesystem)
// ---------------------------------------------------------------------------

/// Push `color` to the front of `colors`, deduping and capping at `MAX_RECENT_COLORS`
/// - mirrors the recent-docs MRU upsert pattern.
pub fn upsert_recent_color(colors: &mut Vec<String>, color: String) {
    colors.retain(|c| c != &color);
    colors.insert(0, color);
    if colors.len() > MAX_RECENT_COLORS {
        colors.truncate(MAX_RECENT_COLORS);
    }
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

/// Absolute path to the settings JSON file inside `data_dir`.
pub fn settings_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

/// Load settings from `data_dir/settings.json`.
///
/// Returns `AppSettings::default()` if the file does not exist yet.
/// Returns an IO error only for genuine read or parse failures.
pub fn load_settings(data_dir: &Path) -> io::Result<AppSettings> {
    let path = settings_file_path(data_dir);
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let bytes = fs::read(&path)?;
    serde_json::from_slice::<AppSettings>(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Save settings to `data_dir/settings.json` atomically (temp + rename).
///
/// Creates `data_dir` if it does not yet exist.
pub fn save_settings(data_dir: &Path, settings: &AppSettings) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let dest = settings_file_path(data_dir);
    let tmp = data_dir.join(format!(
        ".settings-{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let json = serde_json::to_vec_pretty(settings)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let write_result = fs::write(&tmp, &json);
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
        return write_result;
    }
    if let Err(e) = fs::rename(&tmp, &dest) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_are_sensible() {
        let s = AppSettings::default();
        assert_eq!(s.theme, "dark");
        assert_eq!(s.measurement_unit, "m");
        assert_eq!(s.author_name, "");
        assert!(s.default_tool.is_none());
        assert!(s.last_window.is_none());
        assert!(s.recent_colors.is_empty());
    }

    #[test]
    fn load_returns_defaults_when_file_absent() {
        let dir = tempdir().unwrap();
        let loaded = load_settings(dir.path()).unwrap();
        assert_eq!(loaded, AppSettings::default());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let settings = AppSettings {
            theme: "light".to_string(),
            default_tool: Some("Rectangle".to_string()),
            measurement_unit: "ft".to_string(),
            author_name: "Martin Robert".to_string(),
            last_window: Some(LastWindowState {
                width: 1440.0,
                height: 900.0,
                maximized: true,
            }),
            recent_colors: vec!["#ff0000".to_string(), "#00ff00".to_string()],
        };

        save_settings(dir.path(), &settings).unwrap();
        let loaded = load_settings(dir.path()).unwrap();
        assert_eq!(loaded, settings);
    }

    #[test]
    fn save_creates_data_dir_if_absent() {
        let root = tempdir().unwrap();
        let data_dir = root.path().join("app-data").join("redline");
        assert!(!data_dir.exists());
        save_settings(&data_dir, &AppSettings::default()).unwrap();
        assert!(data_dir.exists());
        assert!(settings_file_path(&data_dir).exists());
    }

    #[test]
    fn old_settings_file_missing_new_field_falls_back_to_default() {
        // Simulates a v1 settings.json written before `recent_colors` existed:
        // forward compatibility means missing fields deserialize to their
        // defaults instead of failing the whole load.
        let dir = tempdir().unwrap();
        let path = settings_file_path(dir.path());
        fs::write(
            &path,
            br#"{"theme":"light","measurement_unit":"in","author_name":"Old User"}"#,
        )
        .unwrap();

        let loaded = load_settings(dir.path()).unwrap();
        assert_eq!(loaded.theme, "light");
        assert_eq!(loaded.measurement_unit, "in");
        assert_eq!(loaded.author_name, "Old User");
        assert!(loaded.default_tool.is_none());
        assert!(loaded.last_window.is_none());
        assert!(loaded.recent_colors.is_empty());
    }

    #[test]
    fn upsert_recent_color_dedups_and_moves_to_front() {
        let mut colors = vec!["#111111".to_string(), "#222222".to_string()];
        upsert_recent_color(&mut colors, "#222222".to_string());
        assert_eq!(
            colors,
            vec!["#222222".to_string(), "#111111".to_string()]
        );
    }

    #[test]
    fn upsert_recent_color_caps_at_max() {
        let mut colors: Vec<String> = Vec::new();
        for i in 0..(MAX_RECENT_COLORS + 3) {
            upsert_recent_color(&mut colors, format!("#{i:06x}"));
        }
        assert_eq!(colors.len(), MAX_RECENT_COLORS);
    }
}
