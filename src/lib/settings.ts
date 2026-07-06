/**
 * Application settings - client-side types + IPC wrappers.
 *
 * Mirrors the recent-docs.ts split: `DEFAULT_SETTINGS` / `withDefaults` /
 * `upsertRecentColor` are pure (testable, no Tauri dependency);
 * `loadSettings` / `saveSettings` wrap Tauri invoke.
 *
 * Persisted via the Rust `load_settings` / `save_settings` commands, which
 * write to `<app-data-dir>/settings.json`. Old settings files missing newer
 * fields fall back to defaults on the Rust side (serde `#[serde(default)]`);
 * `withDefaults` gives the frontend the same forward-compatible fallback
 * when merging a partial settings object (e.g. from a future migration).
 */
import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------------------
// Types (mirror Rust AppSettings)
// ---------------------------------------------------------------------------

export type Theme = "dark" | "light" | "system";
export type MeasurementUnit = "mm" | "m" | "km" | "in" | "ft";

export interface LastWindowState {
  width: number;
  height: number;
  maximized: boolean;
}

export interface AppSettings {
  theme: Theme;
  default_tool: string | null;
  measurement_unit: MeasurementUnit;
  author_name: string;
  last_window: LastWindowState | null;
  recent_colors: string[];
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/** Maximum number of entries the recent-colors list is allowed to hold. */
export const MAX_RECENT_COLORS = 8;

export const DEFAULT_SETTINGS: AppSettings = {
  theme: "dark",
  default_tool: null,
  measurement_unit: "m",
  author_name: "",
  last_window: null,
  recent_colors: [],
};

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/** Fill in any missing fields of a partial settings object with defaults. */
export function withDefaults(partial: Partial<AppSettings>): AppSettings {
  return { ...DEFAULT_SETTINGS, ...partial };
}

/**
 * Return a new recent-colors list with `color` at the front.
 *
 * - If `color` already exists it is removed first (dedup).
 * - The result is capped at `MAX_RECENT_COLORS`.
 * - The input list is NOT mutated; a new array is returned.
 */
export function upsertRecentColor(colors: string[], color: string): string[] {
  const filtered = colors.filter((c) => c !== color);
  const updated = [color, ...filtered];
  return updated.length > MAX_RECENT_COLORS
    ? updated.slice(0, MAX_RECENT_COLORS)
    : updated;
}

// ---------------------------------------------------------------------------
// IPC wrappers
// ---------------------------------------------------------------------------

/** Load settings from the Rust backend (defaults if never saved). */
export async function loadSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("load_settings");
}

/** Persist settings to the Rust backend. */
export async function saveSettings(settings: AppSettings): Promise<void> {
  return invoke<void>("save_settings", { settings });
}
