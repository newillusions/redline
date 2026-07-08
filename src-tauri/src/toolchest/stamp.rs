//! Stamps - a specialized Tool kind (spec section 102-126 "Stamps").
//!
//! Static stamps place a fixed image/vector appearance. Dynamic stamps compose their
//! appearance AT PLACEMENT time from a template with auto-populated fields (date, time,
//! username, sequential auto-number, document name, prompted text/dropdown). We compose
//! the appearance ourselves (text substitution against a template string) - never via
//! embedded PDF JavaScript/form-field scripting, which is brittle and which we do not
//! execute even when importing a Bluebeam dynamic stamp (spec decision c, section 12).
//!
//! Once placed, the substituted values bake into a static appearance, so the placed
//! markup round-trips and flattens cleanly like any other markup (spec section 8).

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

/// A stamp's backing visual content. Vector/PDF sources stay crisp at any zoom (preferred,
/// spec section 102-126); raster PNG is supported but pixelates on zoom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StampAsset {
    /// Inline SVG markup.
    Svg(String),
    /// Base64-encoded PNG bytes.
    PngBase64(String),
    /// Base64-encoded single-page PDF bytes (the natural landing spot for imported
    /// Bluebeam stamps, which arrive as PDF-backed annotations).
    PdfBase64(String),
}

/// Where a dynamic stamp's sequential auto-number counter is scoped (spec section 12
/// decision c). Full persistent counter state is a NAMED deferral (see
/// `toolchest::sequence` doc comment) - v1 counts in-memory per app session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CounterScope {
    /// Counter resets per document (the default).
    PerDocument,
    /// Counter is shared across all documents for this stamp tool.
    Global,
}

/// One auto-populated field in a dynamic stamp template (spec section 12 decision c - the
/// locked v1 auto-field set: date, time, datetime, username, document name, sequential
/// auto-number, prompted text/dropdown).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DynamicField {
    Date,
    Time,
    DateTime,
    Username,
    DocumentName,
    SequenceNumber { scope: CounterScope },
    /// User-prompted free text/dropdown value; `label` is shown to the user at placement.
    PromptedText { label: String },
}

/// Stamp definition attached to a [`super::Tool`] (spec section 102-126 Stamps).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StampDef {
    /// Fixed appearance, placed as-is (PDF `/Subtype /Stamp` with an appearance stream).
    Static { asset: StampAsset },
    /// Appearance composed at placement time. `asset` is an optional static backing graphic
    /// (e.g. a "REVIEWED" box); `base_text` is the template with `{0}`, `{1}`, ... placeholders
    /// substituted positionally against `fields`, in order - see [`compose_dynamic_text`].
    Dynamic {
        asset: Option<StampAsset>,
        fields: Vec<DynamicField>,
        base_text: String,
    },
}

/// Compose a dynamic stamp's placement-time text: substitute `{0}`, `{1}`, ... in `base_text`
/// with each field's resolved value, in `fields` order.
///
/// Deliberately pure/testable: `now`, `username`, and `document_name` are injected by the
/// caller (the command layer, which owns wall-clock/OS/doc-context access) rather than read
/// here, so this function has no side effects and no hidden inputs.
///
/// `prompted` supplies values for `PromptedText` fields in the order those fields appear
/// among `fields` (i.e. `prompted[0]` answers the first `PromptedText`, `prompted[1]` the
/// second, and so on) - missing entries substitute an empty string rather than panicking.
///
/// `now` is already resolved to the OS local wall-clock time as a `DateTime<FixedOffset>`
/// (the caller - `commands::toolchest::compose_stamp_text` - reads `chrono::Local::now()`
/// and converts via `.fixed_offset()`, the one place wall-clock/OS-timezone access
/// belongs). Taking a `FixedOffset` instant here rather than calling `Local::now()`
/// directly keeps this function pure/deterministic and testable: a test can construct any
/// fixed offset it likes without depending on the machine's actual timezone.
#[allow(clippy::too_many_arguments)]
pub fn compose_dynamic_text(
    base_text: &str,
    fields: &[DynamicField],
    now: DateTime<FixedOffset>,
    username: &str,
    document_name: &str,
    sequence: u32,
    prompted: &[String],
) -> String {
    let mut prompted_iter = prompted.iter();
    let mut out = base_text.to_string();
    for (i, field) in fields.iter().enumerate() {
        let value = match field {
            DynamicField::Date => now.format("%Y-%m-%d").to_string(),
            DynamicField::Time => now.format("%H:%M").to_string(),
            DynamicField::DateTime => now.format("%Y-%m-%d %H:%M").to_string(),
            DynamicField::Username => username.to_string(),
            DynamicField::DocumentName => document_name.to_string(),
            DynamicField::SequenceNumber { .. } => sequence.to_string(),
            DynamicField::PromptedText { .. } => prompted_iter.next().cloned().unwrap_or_default(),
        };
        out = out.replace(&format!("{{{i}}}"), &value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed_now() -> DateTime<FixedOffset> {
        // 2026-07-07 14:30 in a fixed +04:00 offset (Dubai) - a stable fixture instant.
        // `compose_dynamic_text` just formats whatever offset it's handed, so existing
        // assertions below (which predate the local-tz fix and assert these same
        // y/m/d/h/m/s values) still hold; `local_offset_is_honored_not_utc_naive` below is
        // the dedicated regression test proving the offset is actually applied.
        FixedOffset::east_opt(4 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 7, 7, 14, 30, 0)
            .unwrap()
    }

    #[test]
    fn composes_date_and_username() {
        let text = compose_dynamic_text(
            "Reviewed by {0} on {1}",
            &[DynamicField::Username, DynamicField::Date],
            fixed_now(),
            "mrobert",
            "plan-set.pdf",
            1,
            &[],
        );
        assert_eq!(text, "Reviewed by mrobert on 2026-07-07");
    }

    #[test]
    fn composes_sequence_number() {
        let text = compose_dynamic_text(
            "Issue #{0}",
            &[DynamicField::SequenceNumber { scope: CounterScope::PerDocument }],
            fixed_now(),
            "mrobert",
            "plan-set.pdf",
            42,
            &[],
        );
        assert_eq!(text, "Issue #42");
    }

    #[test]
    fn composes_prompted_text_in_field_order() {
        let text = compose_dynamic_text(
            "{0} / {1}",
            &[
                DynamicField::PromptedText { label: "Reason".into() },
                DynamicField::PromptedText { label: "Ref".into() },
            ],
            fixed_now(),
            "mrobert",
            "plan-set.pdf",
            1,
            &["fire rating".to_string(), "RFI-12".to_string()],
        );
        assert_eq!(text, "fire rating / RFI-12");
    }

    #[test]
    fn missing_prompted_value_substitutes_empty_string_not_panic() {
        let text = compose_dynamic_text(
            "Note: {0}",
            &[DynamicField::PromptedText { label: "Note".into() }],
            fixed_now(),
            "mrobert",
            "plan-set.pdf",
            1,
            &[],
        );
        assert_eq!(text, "Note: ");
    }

    #[test]
    fn composes_document_name_and_datetime() {
        let text = compose_dynamic_text(
            "{0} / {1}",
            &[DynamicField::DocumentName, DynamicField::DateTime],
            fixed_now(),
            "mrobert",
            "L-101 Lighting Plan.pdf",
            1,
            &[],
        );
        assert_eq!(text, "L-101 Lighting Plan.pdf / 2026-07-07 14:30");
    }

    /// Regression test for the local-timezone fix: the SAME absolute instant, handed in
    /// with a +04:00 offset, must format using the LOCAL wall-clock date/time (which rolls
    /// over to the next day here), not the UTC date/time - proving the offset is actually
    /// applied by `.format()`, not silently dropped.
    #[test]
    fn local_offset_is_honored_not_utc_naive() {
        use chrono::Utc;
        let utc_instant = Utc.with_ymd_and_hms(2026, 7, 7, 22, 30, 0).unwrap();
        let local = utc_instant.with_timezone(&FixedOffset::east_opt(4 * 3600).unwrap());
        // Local wall-clock: 2026-07-08 02:30 (rolled to the next day past UTC midnight).
        let text = compose_dynamic_text(
            "{0}",
            &[DynamicField::DateTime],
            local,
            "mrobert",
            "plan-set.pdf",
            1,
            &[],
        );
        assert_eq!(
            text, "2026-07-08 02:30",
            "must format the LOCAL date/time (post-midnight rollover), not the UTC one"
        );
    }

    #[test]
    fn stamp_def_serde_round_trips() {
        let def = StampDef::Dynamic {
            asset: Some(StampAsset::Svg("<svg/>".to_string())),
            fields: vec![DynamicField::Date, DynamicField::SequenceNumber { scope: CounterScope::Global }],
            base_text: "{0} #{1}".to_string(),
        };
        let json = serde_json::to_string(&def).expect("serialize");
        let back: StampDef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(def, back);
    }
}
