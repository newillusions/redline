//! Verbatim carry of Bluebeam-private annotation keys (`/BSIColumnData`, `/BBMeasure`,
//! `/BSIBatchQuery`, `/GroupNesting`, `/NumCounts`, ...).
//!
//! redline rebuilds every managed annotation from its model on save, so a key it does
//! not model would be dropped - for a Revu-authored markup that silently erases custom
//! column data and count metadata. The keys in [`ALLOWED`] are copied from the annotation
//! dictionary on read ([`capture`]) and re-emitted unchanged on write ([`apply`]).
//!
//! Trust model: both the PDF (on read) and the `Markup` JSON (on write - it arrives from
//! the frontend and the MCP tools) are untrusted. The allowlist and the value validation
//! in [`validate`] are therefore enforced at BOTH boundaries, and values are only ever
//! built through the lopdf object model (never by concatenating PDF syntax), so names and
//! strings are escaped by the writer. A key that is not allowlisted, or whose value is
//! not plain data of the expected shape, is dropped with a `log::warn!`. In particular no
//! action- or script-bearing key (`/A`, `/AA`, `/JS`, `/OpenAction`, `/URI`, `/Launch`,
//! ...) can be written, at the top level or nested inside an allowed value.
//!
//! Key list source: pymkup (MIT) reading Revu PDFs plus the scout report
//! `scout-bluebeam-interop` (2026-10-12). Only `/BSIColumnData` (array of strings) has a
//! shape verified by a raw source; the rest are community-decoded, so they are accepted
//! as plain data (numbers, strings, names, arrays and dictionaries of those, bounded) and
//! never interpreted. Page-level `/BSISpaces` is not an annotation key and is never
//! carried here (pages are not rewritten). Values containing indirect references are
//! skipped (an object id from the source file would dangle after redline rewrites the
//! page's objects).

use lopdf::{Dictionary, Object, StringFormat};
use serde::{Deserialize, Serialize};

/// Expected value shape of an allowlisted key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Array of strings (verified for `/BSIColumnData`).
    StringArray,
    /// A single integer or real.
    Number,
    /// Bounded plain data: scalar, or array/dictionary of plain data.
    Data,
}

/// The only annotation keys ever carried, with their expected shape.
pub const ALLOWED: &[(&str, Shape)] = &[
    ("BSIColumnData", Shape::StringArray),
    ("BSIBatchQuery", Shape::Data),
    ("BBMeasure", Shape::Data),
    ("GroupNesting", Shape::Data),
    ("NumCounts", Shape::Number),
    ("CountStyle", Shape::Data),
    ("CountScale", Shape::Data),
    ("PitchRun", Shape::Data),
    ("RiseDrop", Shape::Data),
    ("SlopeType", Shape::Data),
    // A polylength's scale (with /IT /PolyLineDimension, written by `to_annotation_dict`):
    // carried so an edited or rebuilt foreign polylength keeps its measurement.
    ("Measure", Shape::Data),
];

/// Extra keys carried on a reply / state annotation only (never on a markup, whose own
/// writer already emits `/Subj` and `/C`): the author's subject, colour and rich-text body.
/// `/Popup` is not here: it is a reference, rebuilt by the writer (`markup::replies`).
pub const REPLY_EXTRA: &[(&str, Shape)] = &[
    ("Subj", Shape::Data),
    ("C", Shape::Data),
    ("RC", Shape::Data),
];

/// Which annotation the keys belong to: decides the allowlist ([`ALLOWED`] alone for a
/// markup; plus [`REPLY_EXTRA`] for a reply or state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Markup,
    Reply,
}

/// Keys that can trigger an action, script or navigation. Never written at the top level
/// and never accepted as a dictionary key anywhere inside an allowed value.
const ACTION_KEYS: &[&str] = &[
    "A",
    "AA",
    "JS",
    "JavaScript",
    "OpenAction",
    "URI",
    "Launch",
    "Next",
    "Action",
    "SubmitForm",
    "ImportData",
    "ResetForm",
    "GoTo",
    "GoToR",
    "GoToE",
    "Named",
    "Rendition",
    "Movie",
    "Sound",
    "Hide",
    "Thread",
    "Trans",
    "SetOCGState",
    "RichMediaExecute",
    "GoTo3DView",
    "F",
    "FS",
    "Parent",
    "Popup",
    "IRT",
    "AP",
    "P",
    "Dest",
];

/// `/S` action-type names that mark a dictionary as an action.
const ACTION_TYPES: &[&str] = &[
    "JavaScript",
    "URI",
    "Launch",
    "GoTo",
    "GoToR",
    "GoToE",
    "Named",
    "SubmitForm",
    "ResetForm",
    "ImportData",
    "Hide",
    "Thread",
    "Sound",
    "Movie",
    "Rendition",
    "SetOCGState",
    "Trans",
    "GoTo3DView",
    "RichMediaExecute",
];

const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 4096;
const MAX_STRING_BYTES: usize = 64 * 1024;
const MAX_NAME_BYTES: usize = 64;

/// A reference-free PDF value (serde-able copy of the `lopdf::Object` subset).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PdfValue {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(String),
    /// Raw string bytes + whether it was written as a hex string.
    Str(Vec<u8>, bool),
    Array(Vec<PdfValue>),
    Dict(Vec<(String, PdfValue)>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub name: String,
    pub value: PdfValue,
}

/// The expected shape for a key name allowlisted in `scope`.
pub fn shape_in(scope: Scope, name: &str) -> Option<Shape> {
    let find = |list: &[(&str, Shape)]| list.iter().find(|(n, _)| *n == name).map(|(_, s)| *s);
    find(ALLOWED).or_else(|| (scope == Scope::Reply).then(|| find(REPLY_EXTRA)).flatten())
}

/// The expected shape for a markup-scope allowlisted key name.
pub fn shape_of(name: &str) -> Option<Shape> {
    shape_in(Scope::Markup, name)
}

/// True for the key names redline preserves verbatim on a markup.
pub fn is_preserved_key(name: &str) -> bool {
    shape_of(name).is_some()
}

/// A name token we are willing to emit: short, `[A-Za-z0-9_.+-]` only (no whitespace or
/// PDF delimiters), so there is nothing for the writer to escape and nothing to inject.
fn is_safe_token(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= MAX_NAME_BYTES
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
}

/// True if the borrowed `o` is small enough to be carried as plain data: within the node,
/// depth and string limits of [`validate`], with no reference or stream. Walks at most
/// `MAX_NODES` nodes, so the cost is bounded whatever the size of `o`; callers use it to
/// refuse a huge shared object BEFORE cloning it.
pub fn fits_carry_budget(o: &Object) -> bool {
    /// Total string, name and key bytes: bounds the clone whatever the node count.
    const MAX_BYTES: usize = 1024 * 1024;
    fn walk(o: &Object, depth: usize, nodes: &mut usize, bytes: &mut usize) -> bool {
        *nodes += 1;
        if depth > MAX_DEPTH || *nodes > MAX_NODES {
            return false;
        }
        let mut text = |len: usize| {
            *bytes += len;
            len <= MAX_STRING_BYTES && *bytes <= MAX_BYTES
        };
        match o {
            Object::Reference(_) | Object::Stream(_) => false,
            Object::String(b, _) => text(b.len()),
            Object::Name(n) => text(n.len()),
            Object::Array(a) => a.iter().all(|e| walk(e, depth + 1, nodes, bytes)),
            Object::Dictionary(d) => d
                .iter()
                .all(|(k, v)| {
                    let key_ok = {
                        *bytes += k.len();
                        k.len() <= MAX_NAME_BYTES && *bytes <= MAX_BYTES
                    };
                    key_ok && walk(v, depth + 1, nodes, bytes)
                }),
            _ => true,
        }
    }
    walk(o, 0, &mut 0, &mut 0)
}

/// The keys of a `/Measure` dictionary and its `/NumberFormat` entries (ISO 32000-1 12.9),
/// the only ones accepted inside a carried `/Measure`. Several (`/A`, `/F`, `/D`) share a
/// name with an action key, which is why a measure is checked against this list instead.
const MEASURE_KEYS: &[&str] = &[
    "Type", "Subtype", "R", "X", "Y", "D", "A", "T", "S", "O", "CYX", "U", "C", "F", "FD", "RT",
    "RD", "PS", "SS",
];

fn check_plain(
    v: &PdfValue,
    depth: usize,
    nodes: &mut usize,
    measure: bool,
) -> Result<(), &'static str> {
    *nodes += 1;
    if depth > MAX_DEPTH || *nodes > MAX_NODES {
        return Err("value too deep or too large");
    }
    match v {
        PdfValue::Null | PdfValue::Bool(_) | PdfValue::Int(_) => Ok(()),
        // PDF reals are written as f32: reject what would become inf.
        PdfValue::Real(r) if r.is_finite() && r.abs() <= f64::from(f32::MAX) => Ok(()),
        PdfValue::Real(_) => Err("non-finite or out-of-range real"),
        PdfValue::Name(n) if is_safe_token(n) => Ok(()),
        PdfValue::Name(_) => Err("name with unsafe characters"),
        PdfValue::Str(b, _) if b.len() <= MAX_STRING_BYTES => Ok(()),
        PdfValue::Str(..) => Err("string too long"),
        PdfValue::Array(a) => a.iter().try_for_each(|e| check_plain(e, depth + 1, nodes, measure)),
        PdfValue::Dict(d) => {
            for (k, val) in d {
                let allowed = if measure {
                    MEASURE_KEYS.contains(&k.as_str())
                } else {
                    !ACTION_KEYS.contains(&k.as_str())
                };
                if !is_safe_token(k) || !allowed {
                    return Err("action-bearing or unsafe dictionary key");
                }
                if k == "S" {
                    if let PdfValue::Name(n) = val {
                        if ACTION_TYPES.contains(&n.as_str()) {
                            return Err("action dictionary");
                        }
                    }
                }
                check_plain(val, depth + 1, nodes, measure)?;
            }
            Ok(())
        }
    }
}

/// Validate one key/value for carrying: allowlisted name, expected shape, plain bounded
/// data. `Err` carries the reason (for the log line).
pub fn validate(k: &ForeignKey) -> Result<(), &'static str> {
    validate_in(Scope::Markup, k)
}

/// [`validate`] against the allowlist of `scope`.
pub fn validate_in(scope: Scope, k: &ForeignKey) -> Result<(), &'static str> {
    if ACTION_KEYS.contains(&k.name.as_str()) {
        return Err("action-bearing key");
    }
    let Some(shape) = shape_in(scope, &k.name) else {
        return Err("key not on the allowlist");
    };
    match (shape, &k.value) {
        (Shape::StringArray, PdfValue::Array(a)) => {
            if a.len() > MAX_NODES
                || !a
                    .iter()
                    .all(|e| matches!(e, PdfValue::Str(b, _) if b.len() <= MAX_STRING_BYTES))
            {
                return Err("expected an array of strings");
            }
            Ok(())
        }
        (Shape::StringArray, _) => Err("expected an array of strings"),
        (Shape::Number, PdfValue::Int(_)) => Ok(()),
        (Shape::Number, PdfValue::Real(r)) if r.is_finite() && r.abs() <= f64::from(f32::MAX) => {
            Ok(())
        }
        (Shape::Number, _) => Err("expected a number"),
        (Shape::Data, v) => check_plain(v, 0, &mut 0, k.name == "Measure"),
    }
}

fn to_value(o: &Object, depth: usize) -> Option<PdfValue> {
    if depth > MAX_DEPTH {
        return None; // bounds recursion on hostile nesting before anything else runs
    }
    Some(match o {
        Object::Null => PdfValue::Null,
        Object::Boolean(b) => PdfValue::Bool(*b),
        Object::Integer(i) => PdfValue::Int(*i),
        Object::Real(r) => PdfValue::Real(f64::from(*r)),
        Object::Name(n) => PdfValue::Name(String::from_utf8_lossy(n).into_owned()),
        Object::String(b, f) => PdfValue::Str(b.clone(), matches!(f, StringFormat::Hexadecimal)),
        Object::Array(a) => PdfValue::Array(
            a.iter()
                .map(|e| to_value(e, depth + 1))
                .collect::<Option<_>>()?,
        ),
        Object::Dictionary(d) => PdfValue::Dict(
            d.iter()
                .map(|(k, v)| {
                    Some((
                        String::from_utf8_lossy(k).into_owned(),
                        to_value(v, depth + 1)?,
                    ))
                })
                .collect::<Option<_>>()?,
        ),
        Object::Reference(_) | Object::Stream(_) => return None,
    })
}

fn from_value(v: &PdfValue) -> Object {
    match v {
        PdfValue::Null => Object::Null,
        PdfValue::Bool(b) => Object::Boolean(*b),
        PdfValue::Int(i) => Object::Integer(*i),
        PdfValue::Real(r) => Object::Real(*r as f32),
        PdfValue::Name(n) => Object::Name(n.as_bytes().to_vec()),
        PdfValue::Str(b, hex) => Object::String(
            b.clone(),
            if *hex {
                StringFormat::Hexadecimal
            } else {
                StringFormat::Literal
            },
        ),
        PdfValue::Array(a) => Object::Array(a.iter().map(from_value).collect()),
        PdfValue::Dict(d) => Object::Dictionary(
            d.iter()
                .map(|(k, v)| (k.as_bytes().to_vec(), from_value(v)))
                .collect::<Dictionary>(),
        ),
    }
}

/// Marker redline writes on a foreign markup it wrote back unchanged ([`sanitized_original`]),
/// so the next save still knows the (rewritten) object is the one it manages.
pub const PRESERVED_MARKER: &str = "RLPreserved";

/// Most nodes (values), and string/name bytes, an original annotation dictionary may hold to
/// be carried; a hostile or absurd one is not kept and the markup is rebuilt from the model.
const MAX_ORIGINAL_NODES: usize = 200_000;
const MAX_ORIGINAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_ORIGINAL_DEPTH: usize = 16;

/// Top-level keys of an original dictionary that are never carried: every action- or
/// navigation-bearing key, plus the links redline rebuilds (`/Popup`, `/IRT`, `/RT`, `/Parent`)
/// and its own marker. `/AP` (checked by the caller), `/P` (the page) and `/F` (the
/// annotation flags, a plain integer at this level) are kept.
fn top_level_dropped(k: &[u8]) -> bool {
    let Ok(k) = std::str::from_utf8(k) else { return true };
    k == "RT"
        || k == PRESERVED_MARKER
        || (ACTION_KEYS.contains(&k) && !matches!(k, "AP" | "P" | "F"))
}

/// Largest single string or name carried in an original dictionary.
const MAX_ORIGINAL_STRING: usize = 1024 * 1024;
/// Budget for content INLINED from indirect references, counted after expansion and shared by
/// the whole annotation (fan-out of many references to one big object is charged each time).
const MAX_INLINED_NODES: usize = 20_000;
const MAX_INLINED_BYTES: usize = 1024 * 1024;
/// The same budget for a whole document read: every annotation's inlined content counts, so a
/// crafted file cannot cost more than this on open however many annotations share objects.
/// Once spent, further foreign markups are rebuilt from the model instead of preserved.
const MAX_DOC_INLINED_NODES: usize = 200_000;
const MAX_DOC_INLINED_BYTES: usize = 8 * 1024 * 1024;
/// An object whose expansion cost more than this many nodes is inlined once per document; a
/// second reach (from another annotation, or again in the same one) refuses to preserve that
/// markup rather than copying it again. Small shared objects (a /Measure) may repeat.
const SHARED_INLINE_MAX: usize = 500;

/// Document-wide state for [`sanitized_original`], shared across every annotation of one read.
#[derive(Default)]
pub struct Shared {
    inlined_nodes: usize,
    inlined_bytes: usize,
    /// Expansion cost (nodes) of each object inlined so far.
    costs: std::collections::HashMap<lopdf::ObjectId, usize>,
}

impl Shared {
    fn exhausted(&self) -> bool {
        self.inlined_nodes > MAX_DOC_INLINED_NODES || self.inlined_bytes > MAX_DOC_INLINED_BYTES
    }
}

type Resolver<'a> = &'a dyn Fn(lopdf::ObjectId) -> Option<&'a Object>;

/// Why a value could not be carried.
enum Fail {
    /// The dictionary itself is too large or deep: rebuild the whole markup from the model.
    Whole,
    /// The inlining budget is spent: drop this key, keep the rest.
    Key,
}

/// Running totals for one annotation (plus the document-wide [`Shared`]).
struct Budget<'s> {
    shared: &'s mut Shared,
    nodes: usize,
    bytes: usize,
    inlined_nodes: usize,
    inlined_bytes: usize,
    /// Reference targets currently being expanded (cycle detection).
    active: Vec<lopdf::ObjectId>,
    cycle_logged: bool,
}

impl Budget<'_> {
    fn charge(&mut self, bytes: usize) -> Result<(), Fail> {
        self.nodes += 1;
        self.bytes += bytes;
        if !self.active.is_empty() {
            self.inlined_nodes += 1;
            self.inlined_bytes += bytes;
            self.shared.inlined_nodes += 1;
            self.shared.inlined_bytes += bytes;
            if self.shared.exhausted() {
                return Err(Fail::Whole);
            }
            if self.inlined_nodes > MAX_INLINED_NODES || self.inlined_bytes > MAX_INLINED_BYTES {
                return Err(Fail::Key);
            }
        }
        if self.nodes > MAX_ORIGINAL_NODES || self.bytes > MAX_ORIGINAL_BYTES {
            return Err(Fail::Whole);
        }
        Ok(())
    }
}

/// Strip action keys and action dictionaries from a value, INLINING indirect references (an
/// object id from the file as opened can dangle or point elsewhere after redline rewrites
/// the page's objects, and a referenced action object would otherwise stay in the file).
/// `None` = drop this value (a stream, an action dictionary, an unresolvable or cyclic
/// reference). Recursion is bounded by [`MAX_ORIGINAL_DEPTH`] and every node and byte is
/// charged to `st` before it is copied, so a hostile graph cannot cost more than the budget.
fn clean_original(
    o: &Object,
    resolve: Resolver,
    depth: usize,
    st: &mut Budget,
) -> Result<Option<Object>, Fail> {
    if depth > MAX_ORIGINAL_DEPTH {
        return Err(Fail::Whole);
    }
    st.charge(0)?;
    Ok(Some(match o {
        Object::Stream(_) => return Ok(None),
        Object::Reference(r) => {
            if st.active.contains(r) {
                if !st.cycle_logged {
                    log::warn!("foreign annotation: reference cycle at {r:?}; dropping the reference");
                    st.cycle_logged = true;
                }
                return Ok(None);
            }
            let Some(target) = resolve(*r) else {
                return Ok(None);
            };
            // A big object already inlined once is not copied again, nor is anything once the
            // document-wide budget is spent: the markup is rebuilt from the model.
            if st.shared.exhausted() || st.shared.costs.get(r).is_some_and(|c| *c > SHARED_INLINE_MAX) {
                return Err(Fail::Whole);
            }
            let before = st.nodes;
            st.active.push(*r);
            let out = clean_original(target, resolve, depth + 1, st);
            st.active.pop();
            if out.is_ok() {
                st.shared.costs.insert(*r, st.nodes - before);
            }
            return out;
        }
        Object::String(b, f) => {
            if b.len() > MAX_ORIGINAL_STRING {
                return Err(Fail::Whole);
            }
            st.charge(b.len())?;
            Object::String(b.clone(), *f)
        }
        Object::Name(n) => {
            if n.len() > MAX_ORIGINAL_STRING {
                return Err(Fail::Whole);
            }
            st.charge(n.len())?;
            Object::Name(n.clone())
        }
        Object::Array(a) => {
            let mut out = Vec::with_capacity(a.len().min(1024));
            for e in a {
                if let Some(v) = clean_original(e, resolve, depth + 1, st)? {
                    out.push(v);
                }
            }
            Object::Array(out)
        }
        Object::Dictionary(d) => {
            let is_action = matches!(
                d.get(b"S").ok().and_then(|s| s.as_name().ok()).and_then(|n| std::str::from_utf8(n).ok()),
                Some(t) if ACTION_TYPES.contains(&t)
            );
            if is_action {
                return Ok(None);
            }
            let mut out = Dictionary::new();
            for (k, v) in d.iter() {
                let dropped = std::str::from_utf8(k).map_or(true, |k| k != "P" && ACTION_KEYS.contains(&k));
                if dropped {
                    continue;
                }
                st.charge(k.len())?;
                if let Some(v) = clean_original(v, resolve, depth + 1, st)? {
                    out.set(k.clone(), v);
                }
            }
            Object::Dictionary(out)
        }
        other => other.clone(),
    }))
}

/// The original dictionary of a foreign annotation as safe to write back unchanged: every
/// key kept except action/navigation keys (`/A /AA /JS /OpenAction /URI /Launch ...`, top
/// level and nested), the links redline rebuilds and its marker; action dictionaries nested
/// anywhere (also behind an indirect reference) are dropped. Indirect references are
/// INLINED through `resolve` (streams and unresolvable ones dropped), except `/AP` (kept as
/// the file's own reference when `ap_ok` accepts it), `/P` (the page) and `/OC` (optional
/// content group), which stay references. `None` when the dictionary is too large or deep to
/// carry (the caller then rebuilds the markup from its model as before).
pub fn sanitized_original(
    d: &Dictionary,
    ap_ok: impl Fn(&Object) -> bool,
    resolve: Resolver,
    shared: &mut Shared,
) -> Option<Dictionary> {
    let mut st = Budget {
        shared,
        nodes: 0,
        bytes: 0,
        inlined_nodes: 0,
        inlined_bytes: 0,
        active: Vec::new(),
        cycle_logged: false,
    };
    let mut out = Dictionary::new();
    for (k, v) in d.iter() {
        if top_level_dropped(k) {
            continue;
        }
        match k.as_slice() {
            b"AP" => {
                if ap_ok(v) {
                    out.set(k.clone(), v.clone());
                }
                continue;
            }
            b"P" | b"OC" => {
                if matches!(v, Object::Reference(_)) {
                    out.set(k.clone(), v.clone());
                    continue;
                }
            }
            _ => {}
        }
        if st.charge(k.len()).is_err() {
            return None;
        }
        match clean_original(v, resolve, 0, &mut st) {
            Ok(Some(v)) => out.set(k.clone(), v),
            Ok(None) => {}
            // A key's inlined content is over budget: a preserved original is never partial,
            // so rebuild the whole markup from the model instead of dropping the key.
            Err(Fail::Key | Fail::Whole) => {
                log::warn!("foreign annotation: original dictionary too large to carry; rebuilding");
                return None;
            }
        }
    }
    Some(out)
}

/// Collect the allowlisted, valid keys of an annotation dictionary, in key order. Anything
/// else (other keys are simply not ours; invalid values of ours are logged) is skipped.
pub fn capture(d: &Dictionary) -> Vec<ForeignKey> {
    capture_in(Scope::Markup, d)
}

/// [`capture`] against the allowlist of `scope`.
pub fn capture_in(scope: Scope, d: &Dictionary) -> Vec<ForeignKey> {
    let mut out: Vec<ForeignKey> = Vec::new();
    for (k, v) in d.iter() {
        let name = String::from_utf8_lossy(k).into_owned();
        if shape_in(scope, &name).is_none() {
            continue;
        }
        let Some(value) = to_value(v, 0) else {
            log::warn!("foreign key /{name}: dropped (indirect reference, stream or too deep)");
            continue;
        };
        let key = ForeignKey { name, value };
        match validate_in(scope, &key) {
            Ok(()) => out.push(key),
            Err(why) => log::warn!("foreign key /{}: dropped on read ({why})", key.name),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Write the keys onto an annotation dictionary. Re-validated here: the keys can come from
/// untrusted JSON, so only allowlisted, well-shaped keys are emitted, and never over a key
/// redline already set.
pub fn apply(d: &mut Dictionary, keys: &[ForeignKey]) {
    apply_in(Scope::Markup, d, keys);
}

/// [`apply`] against the allowlist of `scope`.
pub fn apply_in(scope: Scope, d: &mut Dictionary, keys: &[ForeignKey]) {
    for k in keys {
        if let Err(why) = validate_in(scope, k) {
            log::warn!("foreign key /{}: not written ({why})", k.name);
            continue;
        }
        if !d.has(k.name.as_bytes()) {
            d.set(k.name.as_bytes().to_vec(), from_value(&k.value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    #[test]
    fn captures_only_preserved_keys_and_round_trips_values_exactly() {
        let d = dictionary! {
            "Subtype" => "Square",
            "BSIColumnData" => vec![
                Object::string_literal("Electrical"),
                Object::String(b"\x00\x01".to_vec(), StringFormat::Hexadecimal),
            ],
            "BBMeasure" => dictionary! { "Scale" => Object::Real(0.5), "Unit" => "mm" },
            "NumCounts" => 7,
            "GroupNesting" => vec![Object::Integer(1), Object::Integer(2)],
            "Contents" => Object::string_literal("not preserved here"),
        };
        let keys = capture(&d);
        let names: Vec<_> = keys.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(
            names,
            ["BBMeasure", "BSIColumnData", "GroupNesting", "NumCounts"]
        );

        let mut out = Dictionary::new();
        apply(&mut out, &keys);
        for n in names {
            assert_eq!(
                out.get(n.as_bytes()).unwrap(),
                d.get(n.as_bytes()).unwrap(),
                "{n}"
            );
        }
        assert!(!out.has(b"Contents"));
    }

    #[test]
    fn value_with_an_indirect_reference_is_skipped_not_dangled() {
        let d = dictionary! { "BSIBatchQuery" => Object::Reference((9, 0)) };
        assert!(capture(&d).is_empty());
    }

    #[test]
    fn apply_never_overwrites_a_key_redline_set() {
        let mut d = dictionary! { "NumCounts" => 1 };
        apply(
            &mut d,
            &[ForeignKey {
                name: "NumCounts".into(),
                value: PdfValue::Int(9),
            }],
        );
        assert_eq!(d.get(b"NumCounts").unwrap(), &Object::Integer(1));
    }

    #[test]
    fn serde_json_round_trip() {
        let k = ForeignKey {
            name: "BSIColumnData".into(),
            value: PdfValue::Array(vec![
                PdfValue::Str(b"a".to_vec(), false),
                PdfValue::Real(1.5),
            ]),
        };
        let back: ForeignKey = serde_json::from_str(&serde_json::to_string(&k).unwrap()).unwrap();
        assert_eq!(k, back);
    }

    // --- hostile input -----------------------------------------------------

    fn key(name: &str, value: PdfValue) -> ForeignKey {
        ForeignKey {
            name: name.to_string(),
            value,
        }
    }

    fn s(text: &str) -> PdfValue {
        PdfValue::Str(text.as_bytes().to_vec(), false)
    }

    #[test]
    fn a_disallowed_key_smuggled_into_the_preserved_set_is_not_written() {
        let hostile = [
            "JS",
            "A",
            "AA",
            "OpenAction",
            "URI",
            "Launch",
            "JavaScript",
            "Next",
            "AP",
            "IRT",
            "Subtype",
            "Rect",
            "RLType",
            // the old rule accepted any /BSI* key: that prefix is no longer enough
            "BSIAnythingElse",
            "BSI",
            "BSI/JS",
            "BBMeasure/JS",
            "BB Measure",
            "",
        ];
        for name in hostile {
            let mut d = Dictionary::new();
            apply(&mut d, &[key(name, s("x")), key(name, PdfValue::Int(1))]);
            assert!(d.is_empty(), "key {name:?} must not be written");
        }
    }

    #[test]
    fn keys_arriving_as_untrusted_json_are_filtered_the_same_way() {
        let json = r#"[{"name":"JS","value":{"Str":[[97,108,101,114,116],false]}},
                       {"name":"OpenAction","value":{"Dict":[["S",{"Name":"JavaScript"}]]}},
                       {"name":"NumCounts","value":{"Int":3}}]"#;
        let keys: Vec<ForeignKey> = serde_json::from_str(json).unwrap();
        let mut d = Dictionary::new();
        apply(&mut d, &keys);
        assert!(!d.has(b"JS") && !d.has(b"OpenAction"));
        assert_eq!(d.get(b"NumCounts").unwrap(), &Object::Integer(3));
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn action_bearing_content_nested_inside_an_allowed_key_is_refused() {
        let nested_action = PdfValue::Dict(vec![(
            "A".into(),
            PdfValue::Dict(vec![("S".into(), PdfValue::Name("JavaScript".into()))]),
        )]);
        let nested_s = PdfValue::Dict(vec![
            ("S".into(), PdfValue::Name("URI".into())),
            ("URI".into(), s("http://x")),
        ]);
        let nested_js =
            PdfValue::Array(vec![PdfValue::Dict(vec![("JS".into(), s("app.alert(1)"))])]);
        for v in [nested_action, nested_s, nested_js] {
            let k = key("BBMeasure", v);
            assert!(validate(&k).is_err());
            let mut d = Dictionary::new();
            apply(&mut d, &[k]);
            assert!(d.is_empty());
        }
    }

    #[test]
    fn names_and_dictionary_keys_with_pdf_delimiters_are_refused() {
        for bad in [
            "a b", "a/b", "a(b", "a)b", "a<b", "a>b", "a[b", "a]b", "a%b", "a#41", "", "x\u{0}y",
        ] {
            assert!(
                validate(&key("CountStyle", PdfValue::Name(bad.into()))).is_err(),
                "name {bad:?}"
            );
            let dict = PdfValue::Dict(vec![(bad.into(), PdfValue::Int(1))]);
            assert!(
                validate(&key("CountStyle", dict)).is_err(),
                "dict key {bad:?}"
            );
        }
        assert!(validate(&key("CountStyle", PdfValue::Name("Normal-1.5_x".into()))).is_ok());
    }

    #[test]
    fn expected_types_are_enforced() {
        assert!(validate(&key(
            "BSIColumnData",
            PdfValue::Array(vec![PdfValue::Int(1)])
        ))
        .is_err());
        assert!(validate(&key("BSIColumnData", s("not an array"))).is_err());
        assert!(validate(&key("NumCounts", s("3"))).is_err());
        assert!(validate(&key("NumCounts", PdfValue::Real(f64::NAN))).is_err());
        assert!(validate(&key("NumCounts", PdfValue::Real(2.5))).is_ok());
        // finite as f64 but would become `inf` as an f32 PDF real
        assert!(validate(&key("NumCounts", PdfValue::Real(1e300))).is_err());
        assert!(validate(&key("CountScale", PdfValue::Real(-1e300))).is_err());
        assert!(validate(&key(
            "CountScale",
            PdfValue::Array(vec![PdfValue::Real(1e39)])
        ))
        .is_err());
        assert!(validate(&key("CountScale", PdfValue::Real(f64::from(f32::MAX)))).is_ok());
    }

    #[test]
    fn bounds_on_depth_size_and_nodes_are_enforced() {
        let mut deep = PdfValue::Int(1);
        for _ in 0..(MAX_DEPTH + 2) {
            deep = PdfValue::Array(vec![deep]);
        }
        assert!(validate(&key("GroupNesting", deep)).is_err());
        let wide = PdfValue::Array(vec![PdfValue::Int(0); MAX_NODES + 1]);
        assert!(validate(&key("GroupNesting", wide)).is_err());
        let long = PdfValue::Str(vec![b'a'; MAX_STRING_BYTES + 1], false);
        assert!(validate(&key("BSIBatchQuery", long)).is_err());
    }

    #[test]
    fn capture_survives_hostile_nesting_without_overflowing_the_stack() {
        let mut o = Object::Integer(1);
        for _ in 0..5000 {
            o = Object::Array(vec![o]);
        }
        let mut d = Dictionary::new();
        d.set("GroupNesting", o);
        assert!(capture(&d).is_empty());
        // Dropping 5000 nested arrays is fine; a million would be the PDF parser's problem.
    }

    #[test]
    fn hostile_string_values_round_trip_as_data_and_never_become_pdf_syntax() {
        use crate::document::annots::tests::{one_page_doc, redline_markup};
        use crate::document::annots::{read_markups, write_markups};
        let payload = ")>> /JS (app.alert(1)) /A << /S /JavaScript /JS (x) >> /Z (";
        let hex = PdfValue::Str(b")>>/OpenAction<</S/Launch>>".to_vec(), true);
        let (mut doc, page_id) = one_page_doc();
        let mut m = redline_markup(0);
        m.foreign_keys = vec![
            key(
                "BSIColumnData",
                PdfValue::Array(vec![s(payload), hex.clone(), s("a\\b\n(c)")]),
            ),
            key(
                "BBMeasure",
                PdfValue::Dict(vec![("Label".into(), s(payload))]),
            ),
        ];
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        let doc = lopdf::Document::load_mem(&buf).unwrap();

        // Every annotation dict: only the keys we put there. No action/script key appeared.
        let page = doc.get_dictionary(page_id).unwrap();
        for a in page.get(b"Annots").unwrap().as_array().unwrap() {
            let d = doc.get_dictionary(a.as_reference().unwrap()).unwrap();
            for bad in [
                &b"JS"[..],
                b"A",
                b"AA",
                b"OpenAction",
                b"Launch",
                b"URI",
                b"Z",
            ] {
                assert!(
                    !d.has(bad),
                    "annotation gained /{}",
                    String::from_utf8_lossy(bad)
                );
            }
        }
        // No object in the whole file is an action dictionary.
        for (_, o) in doc.objects.iter() {
            if let Object::Dictionary(d) = o {
                assert!(
                    !d.has(b"JS") && !d.has(b"OpenAction"),
                    "stray action object"
                );
            }
        }
        let back = read_markups(&doc).unwrap();
        let col = back[0]
            .foreign_keys
            .iter()
            .find(|k| k.name == "BSIColumnData")
            .unwrap();
        assert_eq!(
            col.value,
            PdfValue::Array(vec![s(payload), hex, s("a\\b\n(c)")]),
            "bytes survive exactly"
        );
        let me = back[0]
            .foreign_keys
            .iter()
            .find(|k| k.name == "BBMeasure")
            .unwrap();
        assert_eq!(me.value, PdfValue::Dict(vec![("Label".into(), s(payload))]));
    }
}
