//! Text module — PDFium text extraction + in-document search (spec §4, M4 S3).
//!
//! `search_page` runs on the render thread (PDFium owns text page handles).
//! Results are `SearchHit` structs: page index, bounding rect in PDF user space
//! (y-up, PDF coordinates), and a short snippet for the result list.
//!
//! PDF user-space coords: origin bottom-left, y increases upward.
//! The frontend converts to screen coords using the same §5 transform as markups.

use serde::{Deserialize, Serialize};

use crate::geometry::Quad;

/// A single text-search hit on one page.
///
/// `rect` is `[left, bottom, right, top]` in PDF user-space points (y-up).
/// The frontend overlays a highlight using the same `pageToScreen` math as markups.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    /// Zero-based page index.
    pub page: u32,
    /// Bounding rect `[left, bottom, right, top]` in PDF user-space points.
    pub rect: [f64; 4],
    /// Short context snippet (the matched text itself for now).
    pub snippet: String,
}

/// Search options passed down to PDFium.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchOptions {
    /// Match case-sensitively (default: false).
    pub case_sensitive: bool,
    /// Match whole words only (default: false).
    pub whole_word: bool,
}

// ---------------------------------------------------------------------------
// Text selection (redline text-selection + text-anchored highlight feature)
//
// `char_index_at_point` hit-tests a PDF-user-space point to a character index
// (drag anchor/focus for the I-beam tool); `get_text_selection` (render thread,
// PDFium owns the text page) turns a `[start, end)` character range into the
// quads for a Highlight annotation AND the plain-text string for the clipboard.
// ---------------------------------------------------------------------------

/// The result of resolving a character range `[start, end)` on one page: the
/// per-line `Quad`s for a text-anchored Highlight annotation, plus the
/// concatenated Unicode text for `Ctrl+C` clipboard copy.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TextRangeSelection {
    /// One quad per visual text line in the range (never merged - see
    /// `geometry::rects_to_quads`).
    pub quads: Vec<Quad>,
    /// Plain-text content of the range, in document character order (may not
    /// match on-screen reading order for complex layouts - same caveat as
    /// PDFium's `PdfPageText::all()`).
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure-logic unit tests that do NOT need PDFium.
    // PDFium-dependent tests live in the render module (render thread access).

    #[test]
    fn search_hit_serde_round_trip() {
        let hit = SearchHit {
            page: 3,
            rect: [10.0, 20.0, 80.0, 35.0],
            snippet: "hello world".to_string(),
        };
        let json = serde_json::to_string(&hit).unwrap();
        let back: SearchHit = serde_json::from_str(&json).unwrap();
        assert_eq!(hit, back);
    }

    #[test]
    fn search_hit_rect_order() {
        // Invariant: rect is [left, bottom, right, top] — left < right, bottom < top.
        let hit = SearchHit {
            page: 0,
            rect: [5.0, 10.0, 15.0, 20.0],
            snippet: "test".into(),
        };
        let [left, bottom, right, top] = hit.rect;
        assert!(left < right, "left must be < right");
        assert!(bottom < top, "bottom must be < top (y-up)");
    }

    #[test]
    fn search_options_default_is_case_insensitive_not_whole_word() {
        let opts = SearchOptions::default();
        assert!(!opts.case_sensitive);
        assert!(!opts.whole_word);
    }

    #[test]
    fn text_range_selection_default_is_empty() {
        let sel = TextRangeSelection::default();
        assert!(sel.quads.is_empty());
        assert_eq!(sel.text, "");
    }

    #[test]
    fn text_range_selection_serde_round_trip() {
        use crate::geometry::PdfPoint;
        let sel = TextRangeSelection {
            quads: vec![[
                PdfPoint { x: 0.0, y: 10.0 },
                PdfPoint { x: 50.0, y: 10.0 },
                PdfPoint { x: 0.0, y: 0.0 },
                PdfPoint { x: 50.0, y: 0.0 },
            ]],
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&sel).unwrap();
        let back: TextRangeSelection = serde_json::from_str(&json).unwrap();
        assert_eq!(sel, back);
    }
}
