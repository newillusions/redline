//! Rollback support for the auto-updater — lists past releases discovered
//! from this repo's `update.json` manifest history on GitHub, and derives
//! the pinned-manifest URL a rollback install targets.
//!
//! **Why a real downgrade is possible at all**: the shipped updater endpoint
//! (`https://raw.githubusercontent.com/.../main/update.json`, see
//! `tauri.conf.json`) always advertises only the LATEST release — that file
//! is overwritten on every release by `.github/workflows/build-releases.yml`'s
//! `update-manifest` job. Tauri's updater plugin has no built-in "pick an
//! older version" API, because `check()` only ever consults whatever
//! manifest a single endpoint URL happens to serve right now.
//!
//! The trick: that same workflow job COMMITS `update.json` to the GitHub
//! `main` branch on every release (a plain commit, never force-pushed — see
//! the "Commit update.json to GitHub main" step), so each past release's
//! exact manifest — including its minisign signature, which only the CI
//! signing key could have produced — stays reachable forever at
//! `https://raw.githubusercontent.com/<owner>/<repo>/<commit-sha>/update.json`.
//! Pointing a custom `updater_builder().endpoints([...])` at that
//! commit-pinned URL (see `commands::updater::rollback_to_version`) hands the
//! real Tauri updater a genuine, correctly-signed manifest for that exact
//! historical release — `download_and_install()` verifies it exactly as it
//! would any other update. This is a real downgrade, not a simulated one.
//!
//! Two GitHub REST calls per listing: `GET .../commits?path=update.json`
//! (which commits touched the manifest) and, per commit,
//! `GET .../contents/update.json?ref=<sha>` (that commit's exact manifest
//! content). Both are public, unauthenticated endpoints — `newillusions/
//! redline` is a public GitHub repo (the shipped app's own updater already
//! fetches `raw.githubusercontent.com` from it with no auth header).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};

const GITHUB_OWNER: &str = "newillusions";
const GITHUB_REPO: &str = "redline";
const MANIFEST_PATH: &str = "update.json";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_RELEASE_LIMIT: u32 = 15;

/// One past release, ready for display + rollback. `manifest_url` is the
/// real install target — commit-pinned, never `main`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseInfo {
    pub version: String,
    pub pub_date: String,
    pub notes: String,
    pub manifest_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackError {
    Transport(String),
}

impl std::fmt::Display for RollbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RollbackError::Transport(msg) => write!(f, "{msg}"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CommitAuthor {
    date: String,
}
#[derive(Debug, Deserialize)]
struct CommitDetail {
    author: CommitAuthor,
}
#[derive(Debug, Deserialize)]
struct CommitEntry {
    sha: String,
    commit: CommitDetail,
}
#[derive(Debug, Deserialize)]
struct ContentsResponse {
    content: String,
    encoding: String,
}
#[derive(Debug, Deserialize)]
struct ManifestSnapshot {
    version: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    pub_date: String,
}

/// Source of the two GitHub API calls this module needs — real network in
/// production, a fixed fake in tests. Mirrors `license::service::LicenseClient`.
#[async_trait::async_trait]
pub trait ReleaseSource: Send + Sync {
    async fn list_commits(&self, path: &str, limit: u32) -> Result<Vec<u8>, RollbackError>;
    async fn get_content(&self, path: &str, git_ref: &str) -> Result<Vec<u8>, RollbackError>;
}

/// The real network-backed implementation, used by the Tauri command.
pub struct GithubReleaseSource;

#[async_trait::async_trait]
impl ReleaseSource for GithubReleaseSource {
    async fn list_commits(&self, path: &str, limit: u32) -> Result<Vec<u8>, RollbackError> {
        fetch_bytes(&commits_url(path, limit)).await
    }

    async fn get_content(&self, path: &str, git_ref: &str) -> Result<Vec<u8>, RollbackError> {
        fetch_bytes(&contents_url(path, git_ref)).await
    }
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, RollbackError> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        // GitHub's REST API 403s any request with no User-Agent header.
        .user_agent("redline-app (about-page rollback)")
        .build()
        .map_err(|e| RollbackError::Transport(e.to_string()))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| RollbackError::Transport(e.to_string()))?
        .error_for_status()
        .map_err(|e| RollbackError::Transport(e.to_string()))?;
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| RollbackError::Transport(e.to_string()))
}

fn commits_url(path: &str, limit: u32) -> String {
    format!("https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/commits?path={path}&per_page={limit}")
}

fn contents_url(path: &str, git_ref: &str) -> String {
    format!("https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/contents/{path}?ref={git_ref}")
}

/// The real rollback install target for a given commit — a plain, immutable
/// raw-content URL, NOT the GitHub contents-API wrapper used to fetch it.
/// Public so `commands::updater` never has to re-derive the convention.
pub fn manifest_raw_url(git_ref: &str, path: &str) -> String {
    format!("https://raw.githubusercontent.com/{GITHUB_OWNER}/{GITHUB_REPO}/{git_ref}/{path}")
}

fn parse_commit_entries(bytes: &[u8]) -> Result<Vec<CommitEntry>, RollbackError> {
    serde_json::from_slice(bytes).map_err(|e| RollbackError::Transport(format!("bad commits response: {e}")))
}

/// Decode one `contents` API response into its manifest. Returns `None` on
/// any shape/parse failure (wrong encoding, non-base64, malformed JSON) so
/// the caller can skip a bad entry instead of failing the whole list.
fn decode_manifest_snapshot(bytes: &[u8]) -> Option<ManifestSnapshot> {
    let contents: ContentsResponse = serde_json::from_slice(bytes).ok()?;
    if contents.encoding != "base64" {
        return None;
    }
    // GitHub's contents API line-wraps the base64 payload at 60 chars.
    let cleaned: String = contents.content.chars().filter(|c| !c.is_whitespace()).collect();
    let decoded = STANDARD.decode(cleaned).ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// List up to `limit` past releases, newest first, deduped by version (a
/// retried/failed CI push could in principle leave two commits publishing
/// the same version — the first, i.e. newest, one found wins). Individual
/// commit lookups that fail to fetch or parse are skipped rather than
/// failing the whole list — a GitHub API hiccup or one malformed commit
/// shouldn't hide every other release from the About page.
pub async fn list_releases_with(source: &impl ReleaseSource, limit: u32) -> Result<Vec<ReleaseInfo>, RollbackError> {
    let commits_bytes = source.list_commits(MANIFEST_PATH, limit).await?;
    let commits = parse_commit_entries(&commits_bytes)?;

    let mut releases = Vec::new();
    let mut seen_versions = HashSet::new();
    for entry in commits {
        let Ok(content_bytes) = source.get_content(MANIFEST_PATH, &entry.sha).await else {
            continue;
        };
        let Some(manifest) = decode_manifest_snapshot(&content_bytes) else {
            continue;
        };
        if manifest.version.is_empty() || !seen_versions.insert(manifest.version.clone()) {
            continue;
        }
        releases.push(ReleaseInfo {
            version: manifest.version,
            pub_date: if manifest.pub_date.is_empty() {
                entry.commit.author.date.clone()
            } else {
                manifest.pub_date
            },
            notes: manifest.notes,
            manifest_url: manifest_raw_url(&entry.sha, MANIFEST_PATH),
        });
    }
    Ok(releases)
}

pub async fn list_releases(limit: u32) -> Result<Vec<ReleaseInfo>, RollbackError> {
    list_releases_with(&GithubReleaseSource, limit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeSource {
        commits: Vec<u8>,
        /// sha -> contents-API JSON bytes (as GitHub's `contents` endpoint
        /// would return it: `{"content": "<base64>", "encoding": "base64"}`).
        contents: HashMap<String, Vec<u8>>,
    }

    #[async_trait::async_trait]
    impl ReleaseSource for FakeSource {
        async fn list_commits(&self, _path: &str, _limit: u32) -> Result<Vec<u8>, RollbackError> {
            Ok(self.commits.clone())
        }
        async fn get_content(&self, _path: &str, git_ref: &str) -> Result<Vec<u8>, RollbackError> {
            self.contents
                .get(git_ref)
                .cloned()
                .ok_or_else(|| RollbackError::Transport("not found".into()))
        }
    }

    fn contents_json(manifest_json: &str) -> Vec<u8> {
        let encoded = STANDARD.encode(manifest_json);
        serde_json::json!({ "content": encoded, "encoding": "base64" })
            .to_string()
            .into_bytes()
    }

    fn commits_json(entries: &[(&str, &str)]) -> Vec<u8> {
        let arr: Vec<_> = entries
            .iter()
            .map(|(sha, date)| serde_json::json!({ "sha": sha, "commit": { "author": { "date": date } } }))
            .collect();
        serde_json::Value::Array(arr).to_string().into_bytes()
    }

    #[tokio::test]
    async fn lists_releases_newest_first_from_commit_history() {
        let commits = commits_json(&[("sha2", "2026-08-08T00:00:00Z"), ("sha1", "2026-08-06T00:00:00Z")]);
        let mut contents = HashMap::new();
        contents.insert(
            "sha2".to_string(),
            contents_json(r#"{"version":"0.3.14","notes":"Release v0.3.14","pub_date":"2026-08-08T00:00:00Z"}"#),
        );
        contents.insert(
            "sha1".to_string(),
            contents_json(r#"{"version":"0.3.13","notes":"Release v0.3.13","pub_date":"2026-08-06T00:00:00Z"}"#),
        );
        let source = FakeSource { commits, contents };

        let releases = list_releases_with(&source, 15).await.unwrap();

        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].version, "0.3.14");
        assert_eq!(releases[0].manifest_url, manifest_raw_url("sha2", "update.json"));
        assert_eq!(releases[1].version, "0.3.13");
    }

    #[tokio::test]
    async fn falls_back_to_commit_date_when_manifest_has_no_pub_date() {
        let commits = commits_json(&[("sha1", "2026-08-06T00:00:00Z")]);
        let mut contents = HashMap::new();
        contents.insert(
            "sha1".to_string(),
            contents_json(r#"{"version":"0.3.13","notes":"Release v0.3.13"}"#),
        );
        let source = FakeSource { commits, contents };

        let releases = list_releases_with(&source, 15).await.unwrap();

        assert_eq!(releases[0].pub_date, "2026-08-06T00:00:00Z");
    }

    #[tokio::test]
    async fn dedupes_by_version_keeping_the_first_newest_seen() {
        let commits = commits_json(&[("sha2", "d2"), ("sha1", "d1")]);
        let mut contents = HashMap::new();
        // Both commits (e.g. a retried CI push) published the SAME version.
        contents.insert(
            "sha2".to_string(),
            contents_json(r#"{"version":"0.3.13","notes":"n","pub_date":"d2"}"#),
        );
        contents.insert(
            "sha1".to_string(),
            contents_json(r#"{"version":"0.3.13","notes":"n","pub_date":"d1"}"#),
        );
        let source = FakeSource { commits, contents };

        let releases = list_releases_with(&source, 15).await.unwrap();

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].manifest_url, manifest_raw_url("sha2", "update.json"));
    }

    #[tokio::test]
    async fn skips_commits_whose_content_fetch_fails_rather_than_failing_the_whole_list() {
        let commits = commits_json(&[("sha-missing", "d2"), ("sha1", "d1")]);
        let mut contents = HashMap::new();
        contents.insert(
            "sha1".to_string(),
            contents_json(r#"{"version":"0.3.13","notes":"n","pub_date":"d1"}"#),
        );
        let source = FakeSource { commits, contents };

        let releases = list_releases_with(&source, 15).await.unwrap();

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].version, "0.3.13");
    }

    #[tokio::test]
    async fn skips_commits_with_malformed_manifest_json() {
        let commits = commits_json(&[("sha1", "d1")]);
        let mut contents = HashMap::new();
        contents.insert("sha1".to_string(), contents_json("not json"));
        let source = FakeSource { commits, contents };

        let releases = list_releases_with(&source, 15).await.unwrap();

        assert!(releases.is_empty());
    }

    #[test]
    fn manifest_raw_url_is_commit_pinned_not_branch_pinned() {
        let url = manifest_raw_url("abc123", "update.json");
        assert_eq!(url, "https://raw.githubusercontent.com/newillusions/redline/abc123/update.json");
        assert!(!url.contains("/main/"));
    }
}
