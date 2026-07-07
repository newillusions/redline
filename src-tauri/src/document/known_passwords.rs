//! Known-password store: remembers passwords the user has typed for encrypted
//! PDFs so future opens can try them automatically before prompting.
//!
//! ## Threat model (read before touching this file)
//! This is **obfuscation, not encryption**. Entries are XORed against a
//! per-install key (generated once, stored in a sibling file in the same
//! app-data directory) and hex-encoded. That defeats a casual look at
//! `known_passwords.dat` in a text editor or an accidental screen-share, but
//! it provides **no protection against an attacker who can read the app-data
//! directory** - the key sits right next to the data it obfuscates, same as
//! any XOR-with-adjacent-key scheme. Do not represent this store as secure
//! credential storage, and do not store anything more sensitive than PDF
//! open-passwords here. A real secret store (OS keychain) was considered and
//! deferred - see the PR description for the trade-off.
//!
//! ## Storage
//! Two files in the app-data directory:
//! - `known_passwords.dat` - JSON `{ "entries": ["<hex>", ...] }`, one entry
//!   per distinct remembered password (deduped).
//! - `.pw_obfuscation_key` - 32 random bytes, generated on first use. Removing
//!   this file makes existing entries permanently undecodable (they are
//!   skipped, not treated as a hard error - see `list_known_passwords`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

const KEY_LEN: usize = 32;

// ---------------------------------------------------------------------------
// File paths
// ---------------------------------------------------------------------------

pub fn known_passwords_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("known_passwords.dat")
}

fn key_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(".pw_obfuscation_key")
}

// ---------------------------------------------------------------------------
// Per-install obfuscation key
// ---------------------------------------------------------------------------

/// Load the per-install obfuscation key, generating and persisting a fresh
/// one on first use. Uses two v4 UUIDs (16 bytes each, OS RNG-backed via the
/// existing `uuid` dependency) rather than pulling in a dedicated `rand`
/// crate for 32 bytes of randomness.
fn load_or_create_key(data_dir: &Path) -> io::Result<Vec<u8>> {
    let path = key_file_path(data_dir);
    if let Ok(bytes) = fs::read(&path) {
        if bytes.len() == KEY_LEN {
            return Ok(bytes);
        }
        // Wrong length (corrupt/truncated) - fall through and regenerate.
    }

    let mut key = Vec::with_capacity(KEY_LEN);
    key.extend_from_slice(Uuid::new_v4().as_bytes());
    key.extend_from_slice(Uuid::new_v4().as_bytes());

    fs::create_dir_all(data_dir)?;
    let tmp = data_dir.join(format!(".pw_obfuscation_key-{}.tmp", std::process::id()));
    let write_result = fs::write(&tmp, &key);
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
        return write_result.map(|_| key);
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best-effort: restrict to owner read/write. Never fail the whole
        // operation over a permissions call not being available.
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}

// ---------------------------------------------------------------------------
// XOR obfuscation (cycling key), hex-encoded for JSON-safe storage
// ---------------------------------------------------------------------------

fn xor_cycle(bytes: &[u8], key: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn obfuscate(password: &str, key: &[u8]) -> String {
    to_hex(&xor_cycle(password.as_bytes(), key))
}

/// `None` if `hex` is not valid hex, or the decoded bytes are not valid UTF-8
/// (a corrupt/foreign entry) - callers skip such entries rather than erroring.
fn deobfuscate(hex: &str, key: &[u8]) -> Option<String> {
    let bytes = from_hex(hex)?;
    String::from_utf8(xor_cycle(&bytes, key)).ok()
}

// ---------------------------------------------------------------------------
// Store file
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoreFile {
    /// Hex-obfuscated passwords, deduped. Order is insertion order (oldest
    /// first) - callers that want "most recently used first" reverse it.
    #[serde(default)]
    entries: Vec<String>,
}

fn load_store_file(data_dir: &Path) -> io::Result<StoreFile> {
    let path = known_passwords_file_path(data_dir);
    if !path.exists() {
        return Ok(StoreFile::default());
    }
    let bytes = fs::read(&path)?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn save_store_file(data_dir: &Path, store: &StoreFile) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let dest = known_passwords_file_path(data_dir);
    let tmp = data_dir.join(format!(".known_passwords-{}.tmp", std::process::id()));
    let json = serde_json::to_vec_pretty(store)
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
// Public API
// ---------------------------------------------------------------------------

/// All remembered passwords, most-recently-remembered first. Never plaintext
/// on disk (see module doc for the obfuscation threat model). Entries that
/// fail to decode (corrupt data, or the key file was deleted/rotated) are
/// silently skipped rather than failing the whole list.
pub fn list_known_passwords(data_dir: &Path) -> io::Result<Vec<String>> {
    let path = known_passwords_file_path(data_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let key = load_or_create_key(data_dir)?;
    let store = load_store_file(data_dir)?;
    let mut out: Vec<String> = store
        .entries
        .iter()
        .filter_map(|hex| deobfuscate(hex, &key))
        .collect();
    out.reverse();
    Ok(out)
}

/// Remember `password` for future auto-try, deduped against existing
/// entries. No-op (not an error) if already remembered.
pub fn remember_password(data_dir: &Path, password: &str) -> io::Result<()> {
    let key = load_or_create_key(data_dir)?;
    let mut store = load_store_file(data_dir)?;
    let obfuscated = obfuscate(password, &key);
    // Obfuscation is deterministic for a fixed key, so comparing the
    // obfuscated hex is equivalent to comparing plaintext without decoding
    // every existing entry.
    if !store.entries.contains(&obfuscated) {
        store.entries.push(obfuscated);
        save_store_file(data_dir, &store)?;
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
    fn list_is_empty_when_never_remembered() {
        let dir = tempdir().unwrap();
        assert!(list_known_passwords(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn remember_then_list_roundtrips() {
        let dir = tempdir().unwrap();
        remember_password(dir.path(), "hunter2").unwrap();
        let got = list_known_passwords(dir.path()).unwrap();
        assert_eq!(got, vec!["hunter2".to_string()]);
    }

    #[test]
    fn remembering_duplicate_password_does_not_duplicate_entry() {
        let dir = tempdir().unwrap();
        remember_password(dir.path(), "hunter2").unwrap();
        remember_password(dir.path(), "hunter2").unwrap();
        let got = list_known_passwords(dir.path()).unwrap();
        assert_eq!(
            got.len(),
            1,
            "duplicate remember must not add a second entry"
        );
    }

    #[test]
    fn multiple_passwords_round_trip_newest_first() {
        let dir = tempdir().unwrap();
        remember_password(dir.path(), "first-pw").unwrap();
        remember_password(dir.path(), "second-pw").unwrap();
        remember_password(dir.path(), "third-pw").unwrap();
        let got = list_known_passwords(dir.path()).unwrap();
        assert_eq!(got, vec!["third-pw", "second-pw", "first-pw"]);
    }

    #[test]
    fn store_file_on_disk_is_not_plaintext() {
        let dir = tempdir().unwrap();
        let secret = "super-secret-pdf-password";
        remember_password(dir.path(), secret).unwrap();

        let raw = fs::read_to_string(known_passwords_file_path(dir.path())).unwrap();
        assert!(
            !raw.contains(secret),
            "password must not appear in cleartext in the on-disk store file"
        );
    }

    #[test]
    fn key_file_is_generated_and_reused_across_calls() {
        let dir = tempdir().unwrap();
        remember_password(dir.path(), "pw-a").unwrap();
        let key1 = load_or_create_key(dir.path()).unwrap();
        remember_password(dir.path(), "pw-b").unwrap();
        let key2 = load_or_create_key(dir.path()).unwrap();
        assert_eq!(
            key1, key2,
            "key must be stable across calls, not regenerated"
        );
        // Both passwords still decode correctly under the stable key.
        let got = list_known_passwords(dir.path()).unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn corrupt_entry_is_skipped_not_a_hard_error() {
        let dir = tempdir().unwrap();
        remember_password(dir.path(), "good-pw").unwrap();

        // Inject a corrupt (non-hex) entry directly into the store file.
        let mut store = load_store_file(dir.path()).unwrap();
        store.entries.push("not-valid-hex!!".to_string());
        save_store_file(dir.path(), &store).unwrap();

        let got = list_known_passwords(dir.path()).unwrap();
        assert_eq!(
            got,
            vec!["good-pw"],
            "corrupt entry must be skipped, not error the whole list"
        );
    }

    #[test]
    fn xor_hex_roundtrip_is_reversible() {
        let key = vec![0xAB, 0x12, 0xF0, 0x55];
        let hex = obfuscate("round-trip-me", &key);
        assert_eq!(deobfuscate(&hex, &key).as_deref(), Some("round-trip-me"));
    }
}
