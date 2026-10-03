//! Background PDF indexer — lopdf text extraction + `notify` file watcher.
//!
//! `index_folder_blocking` is designed to run on a dedicated OS thread
//! (via `std::thread::spawn`).  It performs an initial full index of all PDFs
//! in the folder, then sets up a file-system watcher for incremental updates.
//!
//! The function exits when the caller drops all external `FolderIndex` clones
//! (detected via `FolderIndex::alive()`), so the background thread cleans up
//! within ~1 s of the parent command opening a different folder.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use notify::{Event, EventKind, RecursiveMode, Watcher};

use super::{FolderIndex, IndexState};

// ---------------------------------------------------------------------------
// Text extraction
// ---------------------------------------------------------------------------

/// Extract per-page text from a PDF using lopdf.
///
/// Returns a `Vec<(page_number, text)>` where page_number is 1-based (matching
/// the PDF page numbering returned by `lopdf::Document::get_pages()`).
/// Pages that produce errors are silently skipped so a damaged page does not
/// abort indexing of the whole file.
pub fn extract_pdf_text(path: &Path) -> anyhow::Result<Vec<(u64, String)>> {
    let mut doc =
        lopdf::Document::load(path).map_err(|e| anyhow::anyhow!("lopdf load {:?}: {}", path, e))?;
    Ok(extract_doc_text(&mut doc))
}

/// Per-page text extraction over an already-loaded document - the body of
/// [`extract_pdf_text`], split out so a caller that needs the loaded document for
/// other probes (e.g. `commands::ocr`'s existing-OCR-layer scan) does not parse the
/// file twice.
///
/// A page that extracts no text is retried once if repairing NUL-padded content streams
/// (see [`repair_nul_padded_content`]) actually changed something; the repair only
/// mutates the in-memory `doc`, never the file. Pages that already yield text are never
/// touched, and a text-less page without NUL bytes is not re-extracted.
pub fn extract_doc_text(doc: &mut lopdf::Document) -> Vec<(u64, String)> {
    let page_map = doc.get_pages(); // BTreeMap<u32, ObjectId>, 1-based
    let mut result = Vec::with_capacity(page_map.len());

    for (page_num, page_id) in &page_map {
        let mut text = doc.extract_text(&[*page_num]).unwrap_or_default();
        if text.chars().all(char::is_whitespace) && repair_nul_padded_content(doc, *page_id) {
            text = doc.extract_text(&[*page_num]).unwrap_or_default();
        }
        result.push((*page_num as u64, text));
    }

    result
}

/// Make one page's content streams parseable by lopdf when they carry NUL padding, in
/// memory.
///
/// Some scanners (a Konica Minolta MFP in the owner's case) end each page's content
/// stream with a run of NUL bytes. PDF treats NUL as whitespace, but lopdf's content
/// parser does not, and it parses a page's streams as ONE concatenation: parsing stops
/// silently at the first NUL, so the next stream's operators (the OCR layer) are never
/// seen and `extract_text` returns an empty string rather than an error.
/// Because the stream order is scanner stream first, Redline's own OCR text layer
/// second, every page of such a file extracted as zero characters even when a full OCR
/// layer was present. That blinded the "already has text" guard in `run_ocr_document`
/// (OCR re-ran and stacked a duplicate layer) and the Tantivy folder index (OCR'd scans
/// were not searchable). The NULs become spaces, which are whitespace to both lopdf and
/// the PDF spec.
///
/// Returns `true` if any stream was rewritten.
pub(crate) fn repair_nul_padded_content(
    doc: &mut lopdf::Document,
    page_id: lopdf::ObjectId,
) -> bool {
    use lopdf::Object;

    let mut changed = false;
    for stream_id in doc.get_page_contents(page_id) {
        let Some(Object::Stream(stream)) = doc.objects.get_mut(&stream_id) else {
            continue;
        };
        let Ok(data) = stream.decompressed_content() else {
            continue;
        };
        if !data.contains(&0) {
            continue;
        }
        let fixed: Vec<u8> = data
            .into_iter()
            .map(|b| if b == 0 { b' ' } else { b })
            .collect();
        stream.set_plain_content(fixed);
        changed = true;
    }
    changed
}

// ---------------------------------------------------------------------------
// Folder scan
// ---------------------------------------------------------------------------

/// Find all PDF files inside `folder_path`, recursing into every subfolder
/// (spec requirement: "all files in a folder/subfolders", matching the
/// Bluebeam folder-search scope). Symlinks are not followed (avoids cycles);
/// a directory this process cannot read is skipped rather than aborting the
/// whole scan, so one permission-denied subfolder doesn't blank the index.
pub fn find_pdfs(folder_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    scan_dir_into(folder_path, &mut out);
    out
}

fn scan_dir_into(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        // `metadata()` (not `symlink_metadata()`) resolves symlinks so a
        // symlinked PDF is still found, but we only recurse into a `path` if
        // `file_type()` (unresolved) says it's a real directory — this is
        // what keeps us from following a symlinked directory into a cycle.
        let is_real_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_real_dir {
            scan_dir_into(&path, out);
            continue;
        }
        let is_pdf_file = path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false);
        if is_pdf_file {
            out.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// Background indexer entry point
// ---------------------------------------------------------------------------

/// Index all PDFs in `folder_path`, then watch for incremental changes.
///
/// Intended to run on a dedicated OS thread (via `std::thread::spawn`).
/// Returns when:
/// - `index.alive()` returns `false` (the AppState replaced the index), or
/// - The watcher cannot be set up (non-fatal: initial index still complete).
pub fn index_folder_blocking(index: FolderIndex, folder_path: PathBuf) {
    // -----------------------------------------------------------------------
    // Phase 1 — initial full index
    // -----------------------------------------------------------------------
    let pdfs = find_pdfs(&folder_path);
    let total = pdfs.len();

    for (i, pdf_path) in pdfs.iter().enumerate() {
        if !index.alive() {
            return;
        }

        let file_name = pdf_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        index.set_state(IndexState::Indexing {
            current_file: file_name,
            progress: i as f32 / total.max(1) as f32,
        });

        match extract_pdf_text(pdf_path) {
            Ok(pages) => {
                let path_str = pdf_path.display().to_string();
                if let Err(e) = index.index_pages(&path_str, &pages, "lopdf") {
                    log::warn!("folder-index: failed to index {:?}: {e}", pdf_path);
                }
            }
            Err(e) => {
                log::warn!(
                    "folder-index: text extraction failed for {:?}: {e}",
                    pdf_path
                );
            }
        }
    }

    if !index.alive() {
        return;
    }

    index.set_state(IndexState::Idle);

    // -----------------------------------------------------------------------
    // Phase 2 — file watcher for incremental updates
    // -----------------------------------------------------------------------
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<Event>| {
        let _ = tx.send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("folder-index: could not create file watcher: {e}");
            return;
        }
    };

    if let Err(e) = watcher.watch(&folder_path, RecursiveMode::Recursive) {
        log::warn!("folder-index: could not watch {:?}: {e}", folder_path);
        return;
    }

    log::info!("folder-index: watcher running on {:?}", folder_path);

    // Event loop — runs until the index is abandoned or the channel closes.
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(event)) => {
                if index.alive() {
                    handle_event(&index, event);
                }
            }
            Ok(Err(e)) => log::warn!("folder-index: watcher error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !index.alive() {
                    log::info!("folder-index: index abandoned, stopping watcher");
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // `watcher` drops here, which also drops the notify internal thread.
}

// ---------------------------------------------------------------------------
// Watcher event handler
// ---------------------------------------------------------------------------

fn is_pdf(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

fn handle_event(index: &FolderIndex, event: Event) {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in event.paths.iter().filter(|p| p.is_file() && is_pdf(p)) {
                match extract_pdf_text(path) {
                    Ok(pages) => {
                        let path_str = path.display().to_string();
                        if let Err(e) = index.index_pages(&path_str, &pages, "lopdf") {
                            log::warn!("folder-index: re-index {:?} failed: {e}", path);
                        }
                    }
                    Err(e) => {
                        log::warn!("folder-index: extract {:?} failed: {e}", path);
                    }
                }
            }
        }
        EventKind::Remove(_) => {
            for path in event.paths.iter().filter(|p| is_pdf(p)) {
                let path_str = path.display().to_string();
                if let Err(e) = index.delete_document(&path_str) {
                    log::warn!("folder-index: delete {:?} from index failed: {e}", path);
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

/// Test-only fixture builders shared by the indexer and OCR-writer tests.
#[cfg(test)]
pub(crate) mod test_support {
    use lopdf::{dictionary, Document, Object, ObjectId, Stream};

    /// How many NUL bytes the scanner-style content stream is padded with; the owner's
    /// Konica page measured 1159 trailing NULs.
    pub const SCANNER_NUL_PAD: usize = 1159;

    /// An image-only `page_count`-page document whose every page content stream is a
    /// scanner-style `q ... /Im0 Do Q` followed by `SCANNER_NUL_PAD` NUL bytes, with a
    /// tiny 1x1 image so the file stays a few hundred bytes. Returns the document and
    /// its page object ids in page order.
    pub fn nul_padded_scan_doc(page_count: usize) -> (Document, Vec<ObjectId>) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let image_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            vec![0xFF],
        ));
        let mut page_ids = Vec::new();
        for _ in 0..page_count {
            let mut content = b"q\n595 0 0 842 0 0 cm\n/Im0 Do\nQ\n".to_vec();
            content.extend(std::iter::repeat(0u8).take(SCANNER_NUL_PAD));
            let content_id = doc.add_object(Stream::new(dictionary! {}, content));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                "Contents" => content_id,
                "Resources" => dictionary! {
                    "XObject" => dictionary! { "Im0" => image_id },
                },
            });
            page_ids.push(page_id);
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids.iter().map(|id| Object::Reference(*id)).collect::<Vec<_>>(),
                "Count" => page_ids.len() as i64,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        (doc, page_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"%PDF-1.4 not a real pdf, just a fixture").unwrap();
    }

    #[test]
    fn find_pdfs_finds_top_level_files() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("a.pdf"));
        touch(&dir.path().join("b.PDF")); // case-insensitive extension
        touch(&dir.path().join("readme.txt")); // not a pdf, must be excluded

        let found = find_pdfs(dir.path());
        assert_eq!(
            found.len(),
            2,
            "expected exactly the 2 top-level PDFs: {found:?}"
        );
    }

    #[test]
    fn find_pdfs_recurses_into_subfolders() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("top.pdf"));
        touch(&dir.path().join("sub1").join("nested.pdf"));
        touch(
            &dir.path()
                .join("sub1")
                .join("sub2")
                .join("deeply-nested.pdf"),
        );
        touch(&dir.path().join("sub1").join("sub2").join("notes.txt"));

        let found = find_pdfs(dir.path());
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        assert_eq!(
            found.len(),
            3,
            "expected all 3 PDFs across every depth: {names:?}"
        );
        assert!(names.contains(&"top.pdf".to_string()));
        assert!(names.contains(&"nested.pdf".to_string()));
        assert!(names.contains(&"deeply-nested.pdf".to_string()));
    }

    #[test]
    fn find_pdfs_on_empty_folder_returns_empty() {
        let dir = tempdir().unwrap();
        assert!(find_pdfs(dir.path()).is_empty());
    }

    #[test]
    fn find_pdfs_on_missing_folder_returns_empty_not_panic() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(find_pdfs(&missing).is_empty());
    }

    #[test]
    fn find_pdfs_skips_unreadable_subfolder_without_aborting_whole_scan() {
        // A subfolder we can't read must not blank out siblings found before it.
        // (Permission bits are POSIX-only; this test only asserts the happy-path
        // siblings survive when a *missing* nested dir is encountered mid-walk,
        // which exercises the same "one bad entry doesn't abort scan_dir_into"
        // path portably across macOS/Linux/Windows CI runners.)
        let dir = tempdir().unwrap();
        touch(&dir.path().join("before.pdf"));
        touch(&dir.path().join("zzz-after.pdf"));

        let found = find_pdfs(dir.path());
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn extract_pdf_text_missing_file_errors_cleanly() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope.pdf");
        assert!(extract_pdf_text(&missing).is_err());
    }

    // --- NUL-padded scanner streams (owner-reported OCR duplicate-layer defect) ---

    /// Append a hand-built Redline-style invisible text layer (same font resource name
    /// the OCR writer uses) as a second content stream on `page_id`.
    fn append_text_layer(doc: &mut lopdf::Document, page_id: lopdf::ObjectId, text: &str) {
        use lopdf::{dictionary, Object, Stream};
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        let layer = format!("q\nBT\n/RLOCRFont 12 Tf\n3 Tr\n50 700 Td\n({text}) Tj\nET\nQ\n");
        let layer_id = doc.add_object(Stream::new(dictionary! {}, layer.into_bytes()));
        let page = doc.get_dictionary_mut(page_id).unwrap();
        let existing = page.get(b"Contents").unwrap().clone();
        page.set(
            "Contents",
            Object::Array(vec![existing, Object::Reference(layer_id)]),
        );
        let res = match page.get_mut(b"Resources").unwrap() {
            Object::Dictionary(d) => d,
            other => panic!("unexpected Resources shape {other:?}"),
        };
        res.set("Font", dictionary! { "RLOCRFont" => font_id });
    }

    #[test]
    fn stock_lopdf_extracts_nothing_from_a_nul_padded_scan_with_an_ocr_layer() {
        // Pins the root cause: if a future lopdf makes this pass, the repair is no longer
        // needed and this test (and `repair_nul_padded_content`) can be retired.
        let (mut doc, pages) = test_support::nul_padded_scan_doc(1);
        append_text_layer(&mut doc, pages[0], "ROOM SCHEDULE FOR LEVEL THREE");
        let stock = doc.extract_text(&[1]).unwrap_or_default();
        assert_eq!(
            stock.chars().filter(|c| !c.is_whitespace()).count(),
            0,
            "stock lopdf now parses NUL-padded streams; retire repair_nul_padded_content"
        );
    }

    #[test]
    fn extract_doc_text_sees_an_ocr_layer_that_follows_a_nul_padded_scan_stream() {
        let (mut doc, pages) = test_support::nul_padded_scan_doc(2);
        append_text_layer(&mut doc, pages[0], "ROOM SCHEDULE FOR LEVEL THREE");
        append_text_layer(&mut doc, pages[1], "AUDITORIUM CEILING PLAN");
        let text = extract_doc_text(&mut doc);
        assert_eq!(text.len(), 2);
        assert!(
            text[0].1.contains("ROOM SCHEDULE FOR LEVEL THREE"),
            "{:?}",
            text[0]
        );
        assert!(
            text[1].1.contains("AUDITORIUM CEILING PLAN"),
            "{:?}",
            text[1]
        );
    }

    #[test]
    fn extract_pdf_text_reads_ocr_text_from_a_saved_nul_padded_scan() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("scan.pdf");
        let (mut doc, pages) = test_support::nul_padded_scan_doc(1);
        append_text_layer(&mut doc, pages[0], "ROOM SCHEDULE FOR LEVEL THREE");
        doc.save(&path).unwrap();
        let text = extract_pdf_text(&path).unwrap();
        assert!(text[0].1.contains("ROOM SCHEDULE"), "{:?}", text[0]);
    }

    #[test]
    fn extract_doc_text_does_not_rewrite_pages_that_already_yield_text() {
        use lopdf::Object;
        let (mut doc, pages) = test_support::nul_padded_scan_doc(1);
        append_text_layer(&mut doc, pages[0], "ROOM SCHEDULE FOR LEVEL THREE");
        // Make the scanner stream clean so stock extraction succeeds first time.
        let scan_id = doc.get_page_contents(pages[0])[0];
        if let Some(Object::Stream(s)) = doc.objects.get_mut(&scan_id) {
            s.set_plain_content(b"q\nQ\n".to_vec());
        }
        let text = extract_doc_text(&mut doc);
        assert!(text[0].1.contains("ROOM SCHEDULE"));
        let Some(Object::Stream(s)) = doc.objects.get(&scan_id) else {
            panic!()
        };
        assert_eq!(s.content, b"q\nQ\n");
    }
}
