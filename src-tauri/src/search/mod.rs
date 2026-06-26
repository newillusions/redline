//! Search module — Tantivy folder/library full-text index (spec §4, §14).
//!
//! `FolderIndex` wraps a Tantivy index stored on disk at:
//!   `$APPDATA/Redline/indexes/<folder_fingerprint>/`
//!
//! Each indexed document is one PDF page with fields:
//!   file_path   — STRING | STORED  (absolute path to the PDF)
//!   page_number — u64, STORED      (1-based page number)
//!   text        — TEXT | STORED    (extracted or OCR'd text)
//!   source      — STRING | STORED  ("lopdf" | "ocr" | "pdfium")
//!   indexed_at  — u64, STORED      (Unix timestamp, seconds)
//!
//! A `FolderIndex` value is a cheaply-cloneable `Arc`-backed handle; all
//! mutation goes through `Mutex`-guarded inner state, so clones may be passed
//! to background threads without additional synchronisation.

pub mod indexer;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tantivy::{
    Document, Index, IndexWriter, TantivyDocument,
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::{Field, NumericOptions, Schema, STRING, STORED, TEXT},
    snippet::SnippetGenerator,
    Term,
};

// ---------------------------------------------------------------------------
// Schema field name constants
// ---------------------------------------------------------------------------

const F_FILE_PATH: &str = "file_path";
const F_PAGE_NUMBER: &str = "page_number";
const F_TEXT: &str = "text";
const F_SOURCE: &str = "source";
const F_INDEXED_AT: &str = "indexed_at";

// ---------------------------------------------------------------------------
// Public result types
// ---------------------------------------------------------------------------

/// A single search hit returned by a folder-wide query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderSearchHit {
    /// Absolute path to the PDF that contains this hit.
    pub file_path: String,
    /// 1-based page number within `file_path`.
    pub page_number: u64,
    /// HTML snippet with matched terms wrapped in `<b>` tags.
    /// Render with `{@html hit.snippet}` in Svelte.
    pub snippet: String,
    /// Text extraction source: "lopdf", "ocr", or "pdfium".
    pub source: String,
}

/// Indexing state (serialised as a tagged JSON object for Tauri IPC).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum IndexState {
    Idle,
    Indexing {
        current_file: String,
        progress: f32,
    },
    Error {
        message: String,
    },
}

/// Status summary returned by `folder_index_status` IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStatus {
    /// Absolute path of the indexed folder.
    pub folder_path: String,
    /// Number of distinct files with at least one indexed page.
    pub indexed_files: usize,
    /// Total pages across all indexed files.
    pub indexed_pages: u64,
    pub state: IndexState,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct StatusInner {
    /// file_path → number of pages indexed for that file
    file_pages: HashMap<String, u64>,
    state: IndexState,
}

struct FolderIndexInner {
    folder_path: PathBuf,
    f_file_path: Field,
    f_page_number: Field,
    f_text: Field,
    f_source: Field,
    f_indexed_at: Field,
    index: Index,
    writer: Mutex<IndexWriter>,
    status: Mutex<StatusInner>,
}

// ---------------------------------------------------------------------------
// FolderIndex
// ---------------------------------------------------------------------------

/// Thread-safe, cheaply cloneable handle to a persistent Tantivy folder index.
#[derive(Clone)]
pub struct FolderIndex {
    inner: Arc<FolderIndexInner>,
}

impl FolderIndex {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Open an existing index at `index_dir` or create a fresh one.
    ///
    /// `folder_path` is stored in the status and returned to callers; it does
    /// not need to exist on disk at the time of this call.
    pub fn open_or_create(index_dir: &Path, folder_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(index_dir)
            .with_context(|| format!("create index dir {:?}", index_dir))?;

        let meta_exists = index_dir.join("meta.json").exists();

        let (index, f_file_path, f_page_number, f_text, f_source, f_indexed_at) =
            if meta_exists {
                let idx = Index::open_in_dir(index_dir)
                    .with_context(|| format!("open tantivy index in {:?}", index_dir))?;
                let schema = idx.schema();
                let fp = schema.get_field(F_FILE_PATH).context("missing field file_path")?;
                let pn = schema.get_field(F_PAGE_NUMBER).context("missing field page_number")?;
                let tx = schema.get_field(F_TEXT).context("missing field text")?;
                let src = schema.get_field(F_SOURCE).context("missing field source")?;
                let ia = schema.get_field(F_INDEXED_AT).context("missing field indexed_at")?;
                (idx, fp, pn, tx, src, ia)
            } else {
                let (schema, fp, pn, tx, src, ia) = build_schema();
                let idx = Index::create_in_dir(index_dir, schema)
                    .with_context(|| format!("create tantivy index in {:?}", index_dir))?;
                (idx, fp, pn, tx, src, ia)
            };

        let writer = index.writer(50_000_000).context("create index writer")?;

        Ok(FolderIndex {
            inner: Arc::new(FolderIndexInner {
                folder_path: folder_path.to_path_buf(),
                f_file_path,
                f_page_number,
                f_text,
                f_source,
                f_indexed_at,
                index,
                writer: Mutex::new(writer),
                status: Mutex::new(StatusInner {
                    file_pages: HashMap::new(),
                    state: IndexState::Idle,
                }),
            }),
        })
    }

    // -----------------------------------------------------------------------
    // Mutation
    // -----------------------------------------------------------------------

    /// Add or replace all indexed pages for `file_path`.
    ///
    /// `pages` is `(page_number, text)` pairs; page_number is 1-based.
    /// Any previously indexed pages for `file_path` are removed atomically
    /// before the new pages are committed, ensuring stale content is never
    /// visible alongside fresh content.
    pub fn index_pages(
        &self,
        file_path: &str,
        pages: &[(u64, String)],
        source: &str,
    ) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let term = Term::from_field_text(self.inner.f_file_path, file_path);
        let mut writer = self.inner.writer.lock().unwrap();

        // Delete all previously committed pages for this file.
        writer.delete_term(term);

        // Add each non-empty page as a separate Tantivy document.
        let mut indexed = 0u64;
        for (page_num, text) in pages {
            if text.trim().is_empty() {
                continue;
            }
            writer.add_document(doc!(
                self.inner.f_file_path  => file_path,
                self.inner.f_page_number => *page_num,
                self.inner.f_text       => text.as_str(),
                self.inner.f_source     => source,
                self.inner.f_indexed_at => now,
            ))?;
            indexed += 1;
        }

        writer.commit().context("commit index")?;
        drop(writer);

        // Update page-count cache.
        let mut st = self.inner.status.lock().unwrap();
        if indexed > 0 {
            st.file_pages.insert(file_path.to_string(), indexed);
        } else {
            st.file_pages.remove(file_path);
        }

        Ok(())
    }

    /// Remove all indexed pages for `file_path` (called when the file is
    /// deleted or removed from scope).
    pub fn delete_document(&self, file_path: &str) -> Result<()> {
        let term = Term::from_field_text(self.inner.f_file_path, file_path);
        let mut writer = self.inner.writer.lock().unwrap();
        writer.delete_term(term);
        writer.commit().context("commit delete")?;
        drop(writer);

        self.inner.status.lock().unwrap().file_pages.remove(file_path);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Query
    // -----------------------------------------------------------------------

    /// Search the index for `query_str`.  Returns up to `limit` hits sorted by
    /// relevance score.  Returns an empty vec for blank queries.
    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<FolderSearchHit>> {
        if query_str.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Create a fresh reader so we always see the latest committed data.
        let reader = self.inner.index.reader().context("create reader")?;
        let searcher = reader.searcher();
        let schema = self.inner.index.schema();

        let query_parser =
            QueryParser::for_index(&self.inner.index, vec![self.inner.f_text]);
        let query = query_parser.parse_query(query_str)?;

        let mut snippet_gen =
            SnippetGenerator::create(&searcher, &*query, self.inner.f_text)
                .context("create snippet generator")?;
        snippet_gen.set_max_num_chars(150);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit))
            .context("execute search")?;

        let mut hits = Vec::with_capacity(top_docs.len());
        for (_score, doc_addr) in top_docs {
            let doc: TantivyDocument =
                searcher.doc(doc_addr).context("retrieve doc")?;

            let json: serde_json::Value =
                serde_json::from_str(&doc.to_json(&schema)).unwrap_or_default();

            let file_path = json_str(&json, F_FILE_PATH);
            let page_number = json_u64(&json, F_PAGE_NUMBER);
            let source = json_str_or(&json, F_SOURCE, "lopdf");
            let snippet = snippet_gen.snippet_from_doc(&doc).to_html();

            hits.push(FolderSearchHit {
                file_path,
                page_number,
                snippet,
                source,
            });
        }

        Ok(hits)
    }

    // -----------------------------------------------------------------------
    // Status
    // -----------------------------------------------------------------------

    /// Non-blocking status read (reads from in-memory counters, no I/O).
    pub fn status(&self) -> IndexStatus {
        let st = self.inner.status.lock().unwrap();
        let indexed_pages: u64 = st.file_pages.values().sum();
        IndexStatus {
            folder_path: self.inner.folder_path.display().to_string(),
            indexed_files: st.file_pages.len(),
            indexed_pages,
            state: st.state.clone(),
        }
    }

    /// Set the indexing state from a background task.
    pub fn set_state(&self, state: IndexState) {
        self.inner.status.lock().unwrap().state = state;
    }

    /// Return the folder path this index covers.
    pub fn folder_path(&self) -> &Path {
        &self.inner.folder_path
    }

    /// True while the AppState (or any other external holder) still owns a
    /// clone.  When `strong_count == 1`, only the background indexer holds the
    /// Arc — the main code has moved on to a different folder.
    pub fn alive(&self) -> bool {
        Arc::strong_count(&self.inner) > 1
    }
}

// ---------------------------------------------------------------------------
// Schema builder
// ---------------------------------------------------------------------------

fn build_schema() -> (Schema, Field, Field, Field, Field, Field) {
    let mut b = Schema::builder();
    let f_file_path = b.add_text_field(F_FILE_PATH, STRING | STORED);
    let f_page_number =
        b.add_u64_field(F_PAGE_NUMBER, NumericOptions::default().set_stored());
    let f_text = b.add_text_field(F_TEXT, TEXT | STORED);
    let f_source = b.add_text_field(F_SOURCE, STRING | STORED);
    let f_indexed_at =
        b.add_u64_field(F_INDEXED_AT, NumericOptions::default().set_stored());
    (b.build(), f_file_path, f_page_number, f_text, f_source, f_indexed_at)
}

// ---------------------------------------------------------------------------
// JSON field extraction helpers (used in search())
// ---------------------------------------------------------------------------

fn json_str(json: &serde_json::Value, field: &str) -> String {
    json[field]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn json_str_or(json: &serde_json::Value, field: &str, default: &str) -> String {
    json[field]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

fn json_u64(json: &serde_json::Value, field: &str) -> u64 {
    json[field]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_index() -> (FolderIndex, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let idx =
            FolderIndex::open_or_create(dir.path(), Path::new("/test/folder")).unwrap();
        (idx, dir)
    }

    #[test]
    fn test_create_folder_index() {
        // Verifies construction succeeds without panicking.
        let (_idx, _dir) = make_index();
    }

    #[test]
    fn test_initial_status_empty() {
        let (idx, _dir) = make_index();
        let st = idx.status();
        assert_eq!(st.indexed_files, 0);
        assert_eq!(st.indexed_pages, 0);
        assert!(matches!(st.state, IndexState::Idle));
    }

    #[test]
    fn test_index_and_query() {
        let (idx, _dir) = make_index();
        idx.index_pages(
            "docs/plan.pdf",
            &[
                (1, "concrete foundation details".to_string()),
                (2, "structural steel specifications".to_string()),
            ],
            "lopdf",
        )
        .unwrap();

        let hits = idx.search("concrete", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_path, "docs/plan.pdf");
        assert_eq!(hits[0].page_number, 1);
        assert_eq!(hits[0].source, "lopdf");
        assert!(hits[0].snippet.contains("concrete"));
    }

    #[test]
    fn test_multi_file_search() {
        let (idx, _dir) = make_index();
        idx.index_pages(
            "a.pdf",
            &[(1, "foundation wall concrete".to_string())],
            "lopdf",
        )
        .unwrap();
        idx.index_pages(
            "b.pdf",
            &[(3, "concrete slab detail".to_string())],
            "lopdf",
        )
        .unwrap();

        let hits = idx.search("concrete", 10).unwrap();
        assert_eq!(hits.len(), 2);
        let paths: Vec<&str> = hits.iter().map(|h| h.file_path.as_str()).collect();
        assert!(paths.contains(&"a.pdf"));
        assert!(paths.contains(&"b.pdf"));
    }

    #[test]
    fn test_delete_removes_document() {
        let (idx, _dir) = make_index();
        idx.index_pages(
            "plan.pdf",
            &[(1, "concrete foundation".to_string())],
            "lopdf",
        )
        .unwrap();

        idx.delete_document("plan.pdf").unwrap();

        let hits = idx.search("concrete", 10).unwrap();
        assert!(hits.is_empty(), "expected no hits after delete");
    }

    #[test]
    fn test_re_index_replaces_document() {
        let (idx, _dir) = make_index();
        idx.index_pages(
            "plan.pdf",
            &[(1, "old content here".to_string())],
            "lopdf",
        )
        .unwrap();

        // Re-index the same path with different content.
        idx.index_pages(
            "plan.pdf",
            &[(1, "new steel specification".to_string())],
            "lopdf",
        )
        .unwrap();

        // Old content gone, new content findable.
        assert!(idx.search("old", 10).unwrap().is_empty());
        assert!(!idx.search("steel", 10).unwrap().is_empty());
    }

    #[test]
    fn test_status_tracks_indexed_files() {
        let (idx, _dir) = make_index();
        idx.index_pages(
            "a.pdf",
            &[
                (1, "foo bar baz".to_string()),
                (2, "another page text".to_string()),
            ],
            "lopdf",
        )
        .unwrap();
        idx.index_pages("b.pdf", &[(1, "more content here".to_string())], "lopdf")
            .unwrap();

        let st = idx.status();
        assert_eq!(st.indexed_files, 2);
        assert_eq!(st.indexed_pages, 3); // 2 from a.pdf + 1 from b.pdf
    }

    #[test]
    fn test_empty_query_returns_nothing() {
        let (idx, _dir) = make_index();
        idx.index_pages("a.pdf", &[(1, "content".to_string())], "lopdf")
            .unwrap();
        assert!(idx.search("", 10).unwrap().is_empty());
        assert!(idx.search("   ", 10).unwrap().is_empty());
    }

    #[test]
    fn test_open_existing_index() {
        let dir = tempdir().unwrap();
        {
            let idx =
                FolderIndex::open_or_create(dir.path(), Path::new("/folder")).unwrap();
            idx.index_pages("x.pdf", &[(1, "search me content".to_string())], "lopdf")
                .unwrap();
        }
        // Re-open the same directory — data must survive across handles.
        let idx2 =
            FolderIndex::open_or_create(dir.path(), Path::new("/folder")).unwrap();
        let hits = idx2.search("search", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }
}
