//! Replies and review status as standard PDF annotations (ISO 32000-1 §12.5.6.2-4).
//!
//! * A **reply** is a `/Text` annotation with `/IRT` -> the annotation it answers,
//!   `/RT /R`, `/Contents` (the text), `/T` (author), `/M` + `/CreationDate`.
//! * A **status change** is a hidden `/Text` reply carrying `/StateModel` (`Review`) and
//!   `/State` (`Accepted`/`Rejected`/`Cancelled`/`Completed`/`None`). Every change is its
//!   own annotation, so the PDF holds the full history; the current status is the latest
//!   Review-model entry. Other models (`Marked`, Revu's `Migration`, custom models) are
//!   kept in the history but never drive the status.
//!
//! Revu and Acrobat read exactly these dictionaries. Behaviour spec and the evidence for
//! each choice: `docs/bluebeam-replies-status-behaviour.md`.

use chrono::{DateTime, Utc};
use lopdf::{dictionary, Dictionary, Object, ObjectId, StringFormat};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::annotation::to_pdf_date;
use super::foreign::{self, ForeignKey, Scope};
use super::{
    derive_uuid, Markup, MarkupStatus, PopupProps, Reply, SourceExtras, StatusChange, UserRef,
    REVIEW_MODEL,
};

/// `/F` for a reply: Print + NoZoom + NoRotate (28), the flags Acrobat writes for reply
/// notes (inferred from memory of Acrobat output; not re-verified against a fresh file).
pub const REPLY_FLAGS: i64 = 28;
/// `/F` for a state annotation: Hidden + Print + NoZoom + NoRotate (30), as written by
/// the status tooling studied in `bluebeam-set-status` (MIT).
pub const STATE_FLAGS: i64 = 30;
/// Side of the (invisible) note rectangle.
const NOTE_SIZE: f32 = 20.0;

// --- text strings --------------------------------------------------------------

/// Decode a PDF text string: UTF-16BE (BOM FE FF), UTF-8 (BOM EF BB BF), else bytes as
/// UTF-8 lossy (PDFDocEncoding is ASCII-compatible for the characters in practice).
pub(crate) fn decode_text(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    String::from_utf8_lossy(b).into_owned()
}

/// Encode a PDF text string: ASCII as-is, otherwise UTF-16BE with BOM (what Acrobat and
/// Revu require for non-Latin text; raw UTF-8 bytes would read as mojibake there).
pub(crate) fn encode_text(s: &str) -> Object {
    if s.is_ascii() {
        return Object::string_literal(s);
    }
    let mut v = vec![0xFE, 0xFF];
    for u in s.encode_utf16() {
        v.extend_from_slice(&u.to_be_bytes());
    }
    Object::String(v, StringFormat::Literal)
}

fn text_of(d: &Dictionary, key: &[u8]) -> Option<String> {
    d.get(key).ok()?.as_str().ok().map(decode_text)
}

/// `/StateModel` and `/State` are text strings per ISO 32000-1 table 172, but tolerate a
/// tool that wrote them as names.
fn text_or_name(d: &Dictionary, key: &[u8]) -> Option<String> {
    text_of(d, key).or_else(|| name_of(d, key))
}

/// A reply or state without any date: its `at` is this fixed instant, written back as no
/// date at all (see [`UNDATED`]) - never `now()`, which would invent a date in another
/// party's annotation and reshuffle the thread on every reopen.
pub(crate) const UNDATED: DateTime<Utc> = DateTime::<Utc>::UNIX_EPOCH;

fn name_of(d: &Dictionary, key: &[u8]) -> Option<String> {
    d.get(key)
        .ok()?
        .as_name()
        .ok()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Parse a PDF date (`D:YYYYMMDDHHmmSS` + optional `Z` / `+HH'mm'` / `-HH'mm'`) to UTC.
/// Revu and Acrobat write a local-time offset; honouring it keeps reply order and
/// "set by ... at" times right across zones. Missing fields default per ISO 32000-1 §7.9.4.
/// Input is untrusted: every step is checked, an unparseable or out-of-range value is `None`.
pub(crate) fn parse_pdf_date(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim().trim_start_matches("D:");
    let digits = |r: std::ops::Range<usize>, default: u32| -> Option<u32> {
        match s.get(r) {
            Some(t) if t.len() == 2 && t.bytes().all(|b| b.is_ascii_digit()) => t.parse().ok(),
            Some(_) => None,
            None => Some(default),
        }
    };
    let year_str = s.get(0..4)?;
    if !year_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = year_str.parse().ok()?;
    let naive = chrono::NaiveDate::from_ymd_opt(year, digits(4..6, 1)?, digits(6..8, 1)?)?
        .and_hms_opt(digits(8..10, 0)?, digits(10..12, 0)?, digits(12..14, 0)?)?;
    let tail = s.get(14..).unwrap_or("");
    let mut chars = tail.chars();
    let offset_secs: i64 = match chars.next() {
        Some(sign @ ('+' | '-')) => {
            // At most two numeric fields (hours, minutes), each clamped to 2 digits.
            let mut nums = chars
                .as_str()
                .split(|c: char| !c.is_ascii_digit())
                .filter(|t| !t.is_empty())
                .map(|t| {
                    t.chars()
                        .take(2)
                        .collect::<String>()
                        .parse::<i64>()
                        .unwrap_or(0)
                });
            let secs = nums.next().unwrap_or(0) * 3600 + nums.next().unwrap_or(0) * 60;
            if sign == '+' {
                secs
            } else {
                -secs
            }
        }
        _ => 0,
    };
    let delta = chrono::TimeDelta::try_seconds(offset_secs)?;
    // A 4-digit PDF year only: an offset can push the instant outside 0000..=9999.
    naive
        .and_utc()
        .checked_sub_signed(delta)
        .filter(|t| (0..=9999).contains(&chrono::Datelike::year(t)))
}

// --- recognising ---------------------------------------------------------------

fn irt(d: &Dictionary) -> Option<ObjectId> {
    match d.get(b"IRT").ok()? {
        Object::Reference(r) => Some(*r),
        _ => None,
    }
}

/// True for a reply or state annotation: `/Text` with `/IRT` and `/RT` not `/Group`
/// (`/RT /Group` is the grouping link, handled by the group code).
pub(crate) fn is_reply_or_state(d: &Dictionary) -> bool {
    name_of(d, b"Subtype").as_deref() == Some("Text")
        && irt(d).is_some()
        && name_of(d, b"RT").as_deref() != Some("Group")
}

enum Entry {
    Reply(Reply),
    State(StatusChange),
}

impl Entry {
    fn id(&self) -> Uuid {
        match self {
            Entry::Reply(r) => r.id,
            Entry::State(s) => s.id,
        }
    }

    /// (kind, author, time, text): what tells two entries sharing one `/NM` apart.
    fn fingerprint(&self) -> (u8, &str, DateTime<Utc>, &str) {
        match self {
            Entry::Reply(r) => (0, &r.author.display_name, r.at, &r.contents),
            Entry::State(s) => (1, &s.author.display_name, s.at, &s.state),
        }
    }

    fn reassign(&mut self, id: Uuid, original_nm: String) {
        let (slot_id, slot_nm) = match self {
            Entry::Reply(r) => (&mut r.id, &mut r.nm),
            Entry::State(s) => (&mut s.id, &mut s.nm),
        };
        *slot_id = id;
        *slot_nm = Some(original_nm);
    }
}

/// Give every entry a distinct id. Source files do repeat `/NM` values (a tool that copies
/// an annotation, a merge), and ids derive from the `/NM`: two entries would share an id,
/// so a write would address both replies' children and states through one `oid_of` slot and
/// lose track of which is which. Entries that share an id get a new id derived from the
/// `/NM` plus (kind, author, time) and a rank among entries identical in those, so the result
/// does not depend on file order: the same entries get the same ids on every load, whatever
/// order a save wrote them in. Each keeps its original `/NM` (written back unchanged: this
/// is another party's data), so a reload sees the same duplicates and derives the same ids.
fn dedupe_ids(parsed: &mut [(Option<ObjectId>, ObjectId, Entry)]) {
    let mut by_id: HashMap<Uuid, Vec<usize>> = HashMap::new();
    for (i, (_, _, e)) in parsed.iter().enumerate() {
        by_id.entry(e.id()).or_default().push(i);
    }
    let mut taken: HashSet<Uuid> = by_id.keys().copied().collect();
    let mut groups: Vec<(Uuid, Vec<usize>)> =
        by_id.into_iter().filter(|(_, v)| v.len() > 1).collect();
    groups.sort_by_key(|(id, _)| *id);
    for (base, mut idxs) in groups {
        idxs.sort_by(|&a, &b| parsed[a].2.fingerprint().cmp(&parsed[b].2.fingerprint()));
        let mut rank: HashMap<(u8, String, DateTime<Utc>), u32> = HashMap::new();
        for i in idxs {
            let (kind, author, at, _) = parsed[i].2.fingerprint();
            let slot = rank.entry((kind, author.to_string(), at)).or_insert(0);
            let mut n = *slot;
            *slot += 1;
            let original_nm = match &parsed[i].2 {
                Entry::Reply(r) => r.nm.clone(),
                Entry::State(s) => s.nm.clone(),
            }
            .unwrap_or_else(|| base.to_string());
            let id = loop {
                let candidate = derive_uuid(&format!(
                    "nm:{original_nm}:dup:{kind}:{author}:{}:{n}",
                    at.timestamp_micros()
                ));
                if taken.insert(candidate) {
                    break candidate;
                }
                n += 1;
            };
            parsed[i].2.reassign(id, original_nm);
        }
    }
}

/// The pop-up of a reply as the reader resolved it (a direct `/Popup` dictionary holding
/// only `Rect`, `Open`, `F`; see [`with_resolved_popup`]).
fn popup_of(d: &Dictionary) -> Option<PopupProps> {
    let p = d.get(b"Popup").ok()?.as_dict().ok()?;
    let a = p.get(b"Rect").ok()?.as_array().ok()?;
    let v: Vec<f32> = a.iter().filter_map(|o| o.as_float().ok()).collect();
    let rect = <[f32; 4]>::try_from(v).ok().filter(|r| r.iter().all(|c| c.is_finite()))?;
    Some(PopupProps {
        rect,
        open: matches!(p.get(b"Open").ok(), Some(Object::Boolean(true))),
        flags: p.get(b"F").ok().and_then(|o| o.as_i64().ok()),
    })
}

/// Replace a reply's `/Popup` reference by a small direct dictionary of the pop-up
/// annotation's `Rect`, `Open` and `F`, so [`parse`] (which sees one annotation at a time) can
/// carry it. A missing, dangling or non-popup target leaves no `/Popup`. Works on the
/// reader's in-memory copy only.
pub(crate) fn with_resolved_popup(doc: &lopdf::Document, mut d: Dictionary) -> Dictionary {
    let Some(Object::Reference(r)) = d.get(b"Popup").ok().cloned() else {
        return d;
    };
    d.remove(b"Popup");
    if let Ok(pd) = doc.get_dictionary(r) {
        if name_of(pd, b"Subtype").as_deref() == Some("Popup") {
            let mut small = Dictionary::new();
            for key in [&b"Rect"[..], b"Open", b"F"] {
                if let Ok(v) = pd.get(key) {
                    small.set(key.to_vec(), v.clone());
                }
            }
            d.set("Popup", Object::Dictionary(small));
        }
    }
    d
}

/// What `d` carries beyond the modelled fields (see [`SourceExtras`]); `None` when nothing.
fn capture_extras(d: &Dictionary, contents: &str) -> Option<SourceExtras> {
    let keys = foreign::capture_in(Scope::Reply, d);
    let popup = popup_of(d);
    if keys.is_empty() && popup.is_none() {
        return None;
    }
    Some(SourceExtras {
        keys,
        popup,
        contents_at_read: contents.to_string(),
    })
}

/// Write the carried keys back onto a rewritten reply / state. `written` is the `/Contents`
/// being written: a stale `/RC` (the text was edited since it was read) is dropped.
fn apply_extras(d: &mut Dictionary, x: &SourceExtras, written: &str) {
    let keep_rc = x.contents_at_read == written;
    let keys: Vec<ForeignKey> = x
        .keys
        .iter()
        .filter(|k| keep_rc || k.name != "RC")
        .cloned()
        .collect();
    foreign::apply_in(Scope::Reply, d, &keys);
}

fn parse(oid: Option<ObjectId>, d: &Dictionary) -> Option<(ObjectId, Entry)> {
    let parent = irt(d)?;
    let nm = text_of(d, b"NM");
    let id = nm
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .or_else(|| nm.as_deref().map(|s| derive_uuid(&format!("nm:{s}"))))
        .or_else(|| oid.map(|o| derive_uuid(&format!("oid:{}:{}", o.0, o.1))))
        .unwrap_or_else(Uuid::new_v4);
    let display_name = text_of(d, b"T").unwrap_or_default();
    let user_id = text_of(d, b"RLUserId")
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| derive_uuid(&format!("user:{display_name}")));
    let author = UserRef {
        user_id,
        display_name,
    };
    let at = text_of(d, b"CreationDate")
        .and_then(|s| parse_pdf_date(&s))
        .or_else(|| text_of(d, b"M").and_then(|s| parse_pdf_date(&s)))
        .unwrap_or(UNDATED);
    // Keep the original /NM when the id is not simply it, so a save writes it back.
    let nm = nm.filter(|s| *s != id.to_string());
    let contents = text_of(d, b"Contents").unwrap_or_default();
    let extras = capture_extras(d, &contents);
    let entry = if let Some(model) = text_or_name(d, b"StateModel") {
        Entry::State(StatusChange {
            id,
            author,
            at,
            model,
            state: text_or_name(d, b"State").unwrap_or_else(|| "None".to_string()),
            on_reply: None, // set by `attach` once the /IRT target is known
            nm,
            extras,
        })
    } else {
        Entry::Reply(Reply {
            id,
            author,
            at,
            contents,
            in_reply_to: None,
            nm,
            extras,
        })
    };
    Some((parent, entry))
}

/// Attach every reply / state annotation in `raw` to the markup it (transitively)
/// answers. `markup_oids[i]` is the object id of `markups[i]`'s source annotation.
/// Returns the set of consumed annotation object ids (for tests; the write side
/// recomputes ownership from the dictionaries alone). Annotations that answer something
/// that is not a markup (a Link, a foreign sticky note), or whose `/IRT` chain dangles,
/// loops or points at itself, stay foreign: they are skipped (one `log::warn!` per
/// call) and never imported. The input is untrusted PDF content, so nothing here
/// indexes unchecked, recurses, or caps chain depth: chains are resolved iteratively
/// with a memo, in time linear in the number of annotations.
///
/// Also derives `workflow.status` from the latest Review-model entry (overriding the
/// private `/RLStatus`), merges any legacy `/RLWorkflowExtra` thread already on the
/// markup (entries whose id is not already present as a standard reply), and sorts
/// thread and history oldest-first.
pub(crate) fn attach(
    markups: &mut [Markup],
    markup_oids: &[Option<ObjectId>],
    raw: &[(Option<ObjectId>, Dictionary)],
) -> HashSet<ObjectId> {
    let by_markup_oid: HashMap<ObjectId, usize> = markup_oids
        .iter()
        .take(markups.len())
        .enumerate()
        .filter_map(|(i, o)| o.map(|o| (o, i)))
        .collect();
    let mut parsed: Vec<(Option<ObjectId>, ObjectId, Entry)> = raw
        .iter()
        .filter_map(|(oid, d)| parse(*oid, d).map(|(p, e)| (*oid, p, e)))
        .collect();
    dedupe_ids(&mut parsed);
    let by_entry_oid: HashMap<ObjectId, usize> = parsed
        .iter()
        .enumerate()
        .filter_map(|(i, (oid, _, _))| oid.map(|o| (o, i)))
        .collect();
    let entry_id = Entry::id;

    // Root markup index for each entry (None = unresolvable). Iterative, memoised.
    #[derive(Clone, Copy)]
    enum Root {
        Unknown,
        InProgress,
        Markup(usize),
        None,
    }
    let mut roots = vec![Root::Unknown; parsed.len()];
    for start in 0..parsed.len() {
        let mut path: Vec<usize> = Vec::new();
        let mut cur = start;
        let result = loop {
            match roots[cur] {
                Root::Markup(m) => break Root::Markup(m),
                Root::None | Root::InProgress => break Root::None, // known dead end / cycle
                Root::Unknown => {}
            }
            roots[cur] = Root::InProgress;
            path.push(cur);
            let parent = parsed[cur].1;
            if let Some(&m) = by_markup_oid.get(&parent) {
                break Root::Markup(m);
            }
            match by_entry_oid.get(&parent) {
                Some(&next) => cur = next,
                None => break Root::None, // dangling or answers a non-markup
            }
        };
        for i in path {
            roots[i] = result;
        }
    }

    let mut consumed = HashSet::new();
    let mut std_replies: Vec<Vec<Reply>> = vec![Vec::new(); markups.len()];
    let mut std_states: Vec<Vec<StatusChange>> = vec![Vec::new(); markups.len()];
    let mut skipped = 0usize;
    for (i, (oid, parent, entry)) in parsed.iter().enumerate() {
        let Root::Markup(m) = roots[i] else {
            skipped += 1;
            continue;
        };
        // The first hop being another reply (not the markup) makes this a nested reply.
        let nested_under = by_entry_oid
            .get(parent)
            .filter(|&&j| j != i)
            .and_then(|&j| parsed.get(j))
            .map(|(_, _, e)| entry_id(e));
        let (Some(replies_slot), Some(states_slot)) =
            (std_replies.get_mut(m), std_states.get_mut(m))
        else {
            skipped += 1;
            continue;
        };
        if let Some(o) = oid {
            consumed.insert(*o);
        }
        match entry {
            Entry::Reply(r) => {
                let mut r = r.clone();
                r.in_reply_to = nested_under;
                replies_slot.push(r);
            }
            Entry::State(s) => {
                // A state whose /IRT is a reply stays on that reply: it never drives the
                // markup's status and is written back to the same reply.
                let mut s = s.clone();
                s.on_reply = nested_under;
                states_slot.push(s);
            }
        }
    }
    if skipped > 0 {
        log::warn!(
            "replies: skipped {skipped} reply/state annotation(s) with an unresolvable /IRT \
             (dangling, self-referencing, cyclic, or answering a non-markup)"
        );
    }

    for (m, (replies, states)) in markups
        .iter_mut()
        .zip(std_replies.into_iter().zip(std_states))
    {
        let wf = &mut m.workflow;
        let mut replies = replies;
        let mut states = states;
        let std_ids: HashSet<Uuid> = replies.iter().map(|r| r.id).collect();
        wf.thread.retain(|legacy| !std_ids.contains(&legacy.id));
        wf.thread.append(&mut replies);
        wf.thread.sort_by_key(|r| r.at);
        wf.status_history.append(&mut states);
        wf.status_history.sort_by_key(|c| c.at);
        if let Some(s) = wf
            .latest_review_change()
            .and_then(|c| MarkupStatus::from_review_state(&c.state))
        {
            wf.status = s;
        }
    }
    consumed
}

// --- writing -------------------------------------------------------------------

/// The state annotations to write for `m`: its recorded history, plus one synthesised
/// Review entry when `workflow.status` differs from the history's latest (a status set
/// without a history entry, or a pre-this-feature file whose status lives only in
/// `/RLStatus`). The synthesised id is derived from (markup, state, modified_at) so
/// repeated saves write the same `/NM`.
pub(crate) fn history_to_write(m: &Markup) -> Vec<StatusChange> {
    let mut h = m.workflow.status_history.clone();
    let latest = m
        .workflow
        .latest_review_change()
        .and_then(|c| MarkupStatus::from_review_state(&c.state))
        .unwrap_or(MarkupStatus::None);
    if m.workflow.status != latest {
        let state = m.workflow.status.review_state();
        h.push(StatusChange {
            id: derive_uuid(&format!(
                "status:{}:{}:{}",
                m.id(),
                state,
                m.audit.modified_at.timestamp()
            )),
            author: m.audit.modified_by.clone(),
            at: m.audit.modified_at,
            model: REVIEW_MODEL.to_string(),
            state: state.to_string(),
            on_reply: None,
            nm: None,
            extras: None,
        });
    }
    h
}

/// Thread ordered so a reply always follows the reply it answers (parents before
/// children, siblings in thread order). Linear time: children are grouped by parent and
/// walked with an explicit stack. A reply whose parent is absent, itself, or part of a
/// cycle is written as a direct reply to the markup, never dropped.
pub(crate) fn thread_in_write_order(thread: &[Reply]) -> Vec<&Reply> {
    let ids: HashSet<Uuid> = thread.iter().map(|r| r.id).collect();
    let mut children: HashMap<Uuid, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (i, r) in thread.iter().enumerate() {
        match r.in_reply_to {
            Some(p) if p != r.id && ids.contains(&p) => children.entry(p).or_default().push(i),
            _ => roots.push(i),
        }
    }
    let mut out: Vec<&Reply> = Vec::with_capacity(thread.len());
    let mut placed = vec![false; thread.len()];
    let mut stack: Vec<usize> = roots.into_iter().rev().collect();
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut placed[i], true) {
            continue;
        }
        out.push(&thread[i]);
        if let Some(kids) = children.get(&thread[i].id) {
            stack.extend(kids.iter().rev().copied());
        }
    }
    // Members of a parent cycle were never reached from a root: write them as direct replies.
    for (i, r) in thread.iter().enumerate() {
        if !placed[i] {
            out.push(r);
        }
    }
    out
}

/// Where a note annotation lives: its parent, page, rectangle and shared empty `/AP`.
pub(crate) struct NoteAnchor {
    pub parent: ObjectId,
    pub page: ObjectId,
    pub rect: [f32; 4],
    pub ap: ObjectId,
}

fn base_dict(
    id: Uuid,
    nm: Option<&str>,
    author: &UserRef,
    at: &DateTime<Utc>,
    anchor: &NoteAnchor,
    flags: i64,
) -> Dictionary {
    let mut d = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => anchor.rect.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
        "NM" => encode_text(&nm.map_or_else(|| id.to_string(), str::to_string)),
        "T" => encode_text(&author.display_name),
        "IRT" => Object::Reference(anchor.parent),
        "RT" => Object::Name(b"R".to_vec()),
        "F" => flags,
        "P" => Object::Reference(anchor.page),
        "Name" => Object::Name(b"Comment".to_vec()),
        "RLUserId" => Object::string_literal(author.user_id.to_string()),
        "AP" => dictionary! { "N" => Object::Reference(anchor.ap) },
    };
    // An undated source annotation stays undated.
    if *at != UNDATED {
        let date = to_pdf_date(at);
        d.set("M", Object::string_literal(date.clone()));
        d.set("CreationDate", Object::string_literal(date));
    }
    d
}

/// Small note rectangle at the parent's lower-left corner (true page space).
pub(crate) fn note_rect(parent_rect: Option<[f32; 4]>) -> [f32; 4] {
    let [x, y, ..] = parent_rect.unwrap_or([0.0, 0.0, 0.0, 0.0]);
    [x, y, x + NOTE_SIZE, y + NOTE_SIZE]
}

pub(crate) fn reply_dict(r: &Reply, anchor: &NoteAnchor) -> Dictionary {
    let mut d = base_dict(r.id, r.nm.as_deref(), &r.author, &r.at, anchor, REPLY_FLAGS);
    d.set("Contents", encode_text(&r.contents));
    if let Some(x) = &r.extras {
        apply_extras(&mut d, x, &r.contents);
    }
    d
}

pub(crate) fn state_dict(c: &StatusChange, anchor: &NoteAnchor) -> Dictionary {
    let mut d = base_dict(c.id, c.nm.as_deref(), &c.author, &c.at, anchor, STATE_FLAGS);
    d.set("StateModel", encode_text(&c.model));
    d.set("State", encode_text(&c.state));
    let contents = format!("{} set by {}", c.state, c.author.display_name);
    d.set("Contents", encode_text(&contents));
    if let Some(x) = &c.extras {
        apply_extras(&mut d, x, &contents);
    }
    d
}

/// Add the pop-up annotation of a rewritten reply / state (`note`) and point the note's
/// `/Popup` at it. Returns the new annotation's id for the page's `/Annots`, or `None` when
/// the carried rectangle is not finite (hostile input) and no pop-up is written.
pub(crate) fn add_popup(
    doc: &mut lopdf::Document,
    note: ObjectId,
    page: ObjectId,
    p: &PopupProps,
) -> Option<ObjectId> {
    if !p.rect.iter().all(|c| c.is_finite()) {
        return None;
    }
    let mut d = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Popup",
        "Rect" => p.rect.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
        "Open" => p.open,
        "Parent" => Object::Reference(note),
        "P" => Object::Reference(page),
    };
    if let Some(f) = p.flags {
        d.set("F", f);
    }
    let popup = doc.add_object(Object::Dictionary(d));
    doc.get_dictionary_mut(note)
        .ok()?
        .set("Popup", Object::Reference(popup));
    Some(popup)
}

/// An empty Form XObject used as the `/AP` of reply and state notes, so no viewer that
/// draws annotations paints a note icon for them (Acrobat and Revu show replies in their
/// comment lists, not as page icons).
pub(crate) fn empty_ap_stream() -> lopdf::Stream {
    lopdf::Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), (NOTE_SIZE as i64).into(), (NOTE_SIZE as i64).into()],
        },
        Vec::new(),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::document::annots::tests::{one_page_doc, redline_markup};
    use crate::document::annots::{page_annots, read_markups, write_markups};
    use chrono::TimeZone;
    use lopdf::{dictionary, Document};

    pub(crate) fn user(name: &str) -> UserRef {
        UserRef {
            user_id: derive_uuid(&format!("user:{name}")),
            display_name: name.to_string(),
        }
    }

    fn ts(min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, 9, min, 0).unwrap()
    }

    fn reply(author: &str, text: &str, min: u32, parent: Option<Uuid>) -> Reply {
        Reply {
            id: Uuid::new_v4(),
            author: user(author),
            at: ts(min),
            contents: text.to_string(),
            in_reply_to: parent,
            nm: None,
            extras: None,
        }
    }

    fn change(author: &str, model: &str, state: &str, min: u32) -> StatusChange {
        StatusChange {
            id: Uuid::new_v4(),
            author: user(author),
            at: ts(min),
            model: model.to_string(),
            state: state.to_string(),
            on_reply: None,
            nm: None,
            extras: None,
        }
    }

    fn annots(doc: &Document) -> Vec<(Option<ObjectId>, Dictionary)> {
        let page = *doc.get_pages().values().next().unwrap();
        page_annots(doc, page).unwrap()
    }

    fn reload(doc: &mut Document) -> Document {
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        Document::load_mem(&buf).unwrap()
    }

    // --- replies ------------------------------------------------------------

    #[test]
    fn replies_are_written_as_standard_irt_text_annotations_and_read_back() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        let first = reply("Architect", "Agreed, will revise", 5, None);
        let nested = reply("Martin", "Thanks", 9, Some(first.id));
        m.workflow.thread = vec![first.clone(), nested.clone()];
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let mut doc = reload(&mut doc);

        let all = annots(&doc);
        let (parent_oid, parent) = all
            .iter()
            .find(|(_, d)| d.has(b"RLType"))
            .map(|(o, d)| (o.unwrap(), d.clone()))
            .unwrap();
        assert!(
            !parent.has(b"RLWorkflowExtra"),
            "no assignee, so no private workflow key"
        );

        let r1 = all
            .iter()
            .find(|(_, d)| text_of(d, b"NM").as_deref() == Some(&first.id.to_string()))
            .unwrap();
        assert_eq!(name_of(&r1.1, b"Subtype").as_deref(), Some("Text"));
        assert_eq!(name_of(&r1.1, b"RT").as_deref(), Some("R"));
        assert_eq!(
            irt(&r1.1),
            Some(parent_oid),
            "direct reply points at the markup"
        );
        assert_eq!(
            text_of(&r1.1, b"Contents").as_deref(),
            Some("Agreed, will revise")
        );
        assert_eq!(text_of(&r1.1, b"T").as_deref(), Some("Architect"));
        assert!(text_of(&r1.1, b"M")
            .unwrap()
            .starts_with("D:20261001090500"));
        assert!(!r1.1.has(b"StateModel"), "a plain reply carries no state");

        let r2 = all
            .iter()
            .find(|(_, d)| text_of(d, b"NM").as_deref() == Some(&nested.id.to_string()))
            .unwrap();
        assert_eq!(
            irt(&r2.1),
            r1.0,
            "nested reply points at the reply, not the markup"
        );

        let back = read_markups(&doc).unwrap();
        assert_eq!(back.len(), 1, "replies must not be imported as markups");
        assert_eq!(back[0].workflow.thread, vec![first, nested]);
        assert_eq!(back[0].workflow.reply_count(), 2);
        let _ = &mut doc;
    }

    #[test]
    fn non_ascii_reply_text_is_utf16be_in_the_file_and_round_trips() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        m.workflow.thread = vec![reply("Zoë", "Überprüfung 照明", 1, None)];
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let doc = reload(&mut doc);
        let all = annots(&doc);
        let r = all.iter().find(|(_, d)| d.has(b"IRT")).unwrap();
        let raw = r.1.get(b"Contents").unwrap().as_str().unwrap();
        assert_eq!(
            &raw[..2],
            &[0xFE, 0xFF],
            "UTF-16BE BOM, as Revu/Acrobat expect"
        );
        let back = read_markups(&doc).unwrap();
        assert_eq!(back[0].workflow.thread[0].contents, "Überprüfung 照明");
        assert_eq!(back[0].workflow.thread[0].author.display_name, "Zoë");
    }

    #[test]
    fn reply_annotation_has_an_empty_appearance_so_no_viewer_paints_an_icon() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        m.workflow.thread = vec![reply("A", "x", 1, None)];
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let all = annots(&doc);
        let r = all.iter().find(|(_, d)| d.has(b"IRT")).unwrap();
        let ap =
            r.1.get(b"AP")
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"N")
                .unwrap()
                .as_reference()
                .unwrap();
        let stream = doc.get_object(ap).unwrap().as_stream().unwrap();
        assert!(stream.content.is_empty());
    }

    // --- status -------------------------------------------------------------

    #[test]
    fn status_history_is_written_as_review_state_annotations_with_history() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        m.workflow.status_history = vec![
            change("Architect", "Review", "Accepted", 1),
            change("Architect", "Review", "Rejected", 2),
            change("Martin", "Marked", "Marked", 3),
            change("Martin", "Review", "Completed", 4),
        ];
        m.workflow.status = MarkupStatus::Completed;
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let doc = reload(&mut doc);

        let all = annots(&doc);
        let states: Vec<_> = all.iter().filter(|(_, d)| d.has(b"StateModel")).collect();
        assert_eq!(
            states.len(),
            4,
            "one annotation per history entry, none synthesised"
        );
        for (_, d) in &states {
            assert!(irt(d).is_some());
            assert_eq!(name_of(d, b"RT").as_deref(), Some("R"));
            assert_eq!(
                d.get(b"F").unwrap().as_i64().unwrap(),
                STATE_FLAGS,
                "hidden"
            );
        }
        let completed = states
            .iter()
            .find(|(_, d)| text_of(d, b"State").as_deref() == Some("Completed"))
            .unwrap();
        assert_eq!(
            text_of(&completed.1, b"StateModel").as_deref(),
            Some("Review")
        );
        assert_eq!(
            text_of(&completed.1, b"Contents").as_deref(),
            Some("Completed set by Martin")
        );

        let back = read_markups(&doc).unwrap();
        assert_eq!(back[0].workflow.status, MarkupStatus::Completed);
        assert_eq!(back[0].workflow.status_history, m.workflow.status_history);
        let latest = back[0].workflow.latest_review_change().unwrap();
        assert_eq!(
            (latest.author.display_name.as_str(), latest.state.as_str()),
            ("Martin", "Completed")
        );
    }

    #[test]
    fn marked_model_never_drives_the_status() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        m.workflow.status_history = vec![
            change("A", "Review", "Rejected", 1),
            change("A", "Marked", "Marked", 2),
        ];
        m.workflow.status = MarkupStatus::Rejected;
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        assert_eq!(
            read_markups(&doc).unwrap()[0].workflow.status,
            MarkupStatus::Rejected
        );
    }

    #[test]
    fn a_status_set_without_history_synthesises_one_entry_and_is_stable_across_saves() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        m.workflow.status = MarkupStatus::Accepted; // e.g. a pre-feature file or MCP update
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let first = read_markups(&doc).unwrap();
        assert_eq!(first[0].workflow.status, MarkupStatus::Accepted);
        assert_eq!(first[0].workflow.status_history.len(), 1);
        assert_eq!(first[0].workflow.status_history[0].state, "Accepted");

        // Save -> read -> save -> read must neither duplicate nor lose entries.
        write_markups(&mut doc, &first).unwrap();
        let second = read_markups(&doc).unwrap();
        assert_eq!(
            second[0].workflow.status_history,
            first[0].workflow.status_history
        );
        write_markups(&mut doc, &second).unwrap();
        let n = annots(&doc)
            .iter()
            .filter(|(_, d)| d.has(b"StateModel"))
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn reverting_to_none_after_a_status_is_recorded() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        m.workflow.status_history = vec![change("A", "Review", "Accepted", 1)];
        m.workflow.status = MarkupStatus::None; // user cleared it
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let back = read_markups(&doc).unwrap();
        assert_eq!(back[0].workflow.status, MarkupStatus::None);
        assert_eq!(back[0].workflow.status_history.len(), 2);
    }

    #[test]
    fn canceled_spelling_is_read_and_cancelled_is_written() {
        assert_eq!(
            MarkupStatus::from_review_state("Canceled"),
            Some(MarkupStatus::Cancelled)
        );
        assert_eq!(
            MarkupStatus::from_review_state("Cancelled"),
            Some(MarkupStatus::Cancelled)
        );
        assert_eq!(MarkupStatus::Cancelled.review_state(), "Cancelled");
        assert_eq!(MarkupStatus::from_review_state("Migrated"), None);
    }

    // --- deleting / foreign / legacy ---------------------------------------

    #[test]
    fn deleting_a_markup_removes_its_replies_and_states() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        m.workflow.thread = vec![reply("A", "x", 1, None)];
        m.workflow.status = MarkupStatus::Rejected;
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        assert!(annots(&doc).len() >= 3);
        write_markups(&mut doc, &[]).unwrap();
        assert!(annots(&doc).is_empty(), "no orphaned replies left behind");
    }

    #[test]
    fn repeated_saves_do_not_grow_the_file_or_duplicate_replies() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        m.workflow.thread = vec![reply("A", "x", 1, None)];
        m.workflow.status = MarkupStatus::Accepted;
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let mut cur = read_markups(&doc).unwrap();
        let count0 = (doc.objects.len(), annots(&doc).len());
        for _ in 0..3 {
            write_markups(&mut doc, &cur).unwrap();
            cur = read_markups(&doc).unwrap();
        }
        assert_eq!((doc.objects.len(), annots(&doc).len()), count0);
        assert_eq!(cur[0].workflow.thread.len(), 1);
    }

    #[test]
    fn a_reply_to_a_foreign_non_markup_annotation_is_left_alone() {
        let (mut doc, page_id) = one_page_doc();
        let link = doc.add_object(crate::document::annots::tests::link_dict());
        let foreign_reply = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "NM" => Object::string_literal("fr"),
            "Rect" => vec![0.into(), 0.into(), 5.into(), 5.into()],
            "IRT" => Object::Reference(link), "RT" => Object::Name(b"R".to_vec()),
            "Contents" => Object::string_literal("on a link"),
        });
        doc.get_dictionary_mut(page_id).unwrap().set(
            "Annots",
            Object::Array(vec![
                Object::Reference(link),
                Object::Reference(foreign_reply),
            ]),
        );
        let m = redline_markup(0);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let all = annots(&doc);
        assert!(
            all.iter().any(|(o, _)| *o == Some(foreign_reply)),
            "foreign reply kept"
        );
        assert_eq!(
            read_markups(&doc).unwrap().len(),
            1,
            "and not imported as a markup"
        );
    }

    #[test]
    fn legacy_rlworkflowextra_thread_still_opens_and_migrates_to_standard_replies() {
        let (mut doc, _) = one_page_doc();
        let m = redline_markup(0);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        // Rewrite the saved markup as an older redline would have: thread + assignee in the
        // private JSON key, no standard reply annotations.
        let legacy = reply("Legacy", "from an old redline", 3, None);
        let json = serde_json::to_string(&(Some(user("Boss")), vec![legacy.clone()])).unwrap();
        let (oid, _) = annots(&doc)
            .into_iter()
            .find(|(_, d)| d.has(b"RLType"))
            .unwrap();
        let d = doc.get_dictionary_mut(oid.unwrap()).unwrap();
        d.set("RLWorkflowExtra", Object::string_literal(json));
        d.set("RLStatus", Object::Name(b"Accepted".to_vec()));

        let opened = read_markups(&doc).unwrap();
        assert_eq!(
            opened[0].workflow.thread,
            vec![legacy.clone()],
            "legacy thread opens"
        );
        assert_eq!(opened[0].workflow.assignee, Some(user("Boss")));
        assert_eq!(
            opened[0].workflow.status,
            MarkupStatus::Accepted,
            "legacy /RLStatus opens"
        );

        // Decision: migrate on next save.
        write_markups(&mut doc, &opened).unwrap();
        let all = annots(&doc);
        let std_reply = all
            .iter()
            .find(|(_, d)| d.has(b"IRT") && !d.has(b"StateModel"))
            .unwrap();
        assert_eq!(
            text_of(&std_reply.1, b"Contents").as_deref(),
            Some("from an old redline")
        );
        assert!(all
            .iter()
            .any(|(_, d)| text_of(d, b"State").as_deref() == Some("Accepted")));
        let parent = all.iter().find(|(_, d)| d.has(b"RLType")).unwrap();
        let extra: (Option<UserRef>, Vec<Reply>) =
            serde_json::from_str(&text_of(&parent.1, b"RLWorkflowExtra").unwrap()).unwrap();
        assert_eq!(extra.0, Some(user("Boss")), "assignee stays private");
        assert!(
            extra.1.is_empty(),
            "thread no longer duplicated into the private key"
        );

        let again = read_markups(&doc).unwrap();
        assert_eq!(
            again[0].workflow.thread,
            vec![legacy],
            "same thread after migration"
        );
        assert_eq!(again[0].workflow.assignee, Some(user("Boss")));
    }

    // --- foreign (Revu/Acrobat-authored) fixture ----------------------------

    const FIXTURE_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/reviewed-replies-status.pdf"
    );

    /// Builds the committed fixture with plain lopdf, laid out the way Acrobat/Revu write a
    /// reviewed drawing (see `docs/bluebeam-replies-status-behaviour.md` "Fixture"):
    ///  * A: Square "Relocate L4" by Martin, 2 replies (one nested) and a Review history
    ///    Accepted -> Completed plus a Marked entry.
    ///  * B: Circle carrying /BSIColumnData and /BBMeasure, status spelled "Canceled" by Revu.
    ///  * page /BSISpaces, an unrelated Link, and a reply on that Link.
    pub(crate) fn build_reviewed_fixture() -> Document {
        let (mut doc, page_id) = one_page_doc();
        let content = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            b"0.8 w 40 700 300 60 re S BT /F1 12 Tf 48 730 Td (Reviewed sheet - replies and status from Revu/Acrobat) Tj ET".to_vec(),
        ));
        let font = doc.add_object(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        );
        let uuid_a = "11111111-1111-4111-8111-111111111111";
        let uuid_b = "22222222-2222-4222-8222-222222222222";
        let square_ap = doc.add_object(lopdf::Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 100.into(), 60.into()] },
            b"1 0 0 RG 2 w 1 1 98 58 re S".to_vec(),
        ));
        let a = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "NM" => Object::string_literal(uuid_a),
            "Rect" => vec![100.into(), 500.into(), 200.into(), 560.into()],
            "C" => vec![Object::Real(1.0), 0.into(), 0.into()], "F" => 4,
            "T" => Object::string_literal("Martin"), "Subj" => Object::string_literal("Lighting"),
            "Contents" => Object::string_literal("Relocate luminaire L4"),
            "M" => Object::string_literal("D:20261001090000Z"), "CreationDate" => Object::string_literal("D:20261001090000Z"),
            "AP" => dictionary! { "N" => Object::Reference(square_ap) },
        });
        let mk = |nm: &str, parent: ObjectId, who: &str, when: &str, extra: Vec<(&str, Object)>| {
            let mut d = dictionary! {
                "Type" => "Annot", "Subtype" => "Text", "NM" => Object::string_literal(nm),
                "Rect" => vec![100.into(), 500.into(), 120.into(), 520.into()], "F" => 28,
                "T" => Object::string_literal(who),
                "M" => Object::string_literal(when), "CreationDate" => Object::string_literal(when),
                "IRT" => Object::Reference(parent), "RT" => Object::Name(b"R".to_vec()),
            };
            for (k, v) in extra {
                d.set(k, v);
            }
            d
        };
        let r1 = doc.add_object(mk(
            "33333333-3333-4333-8333-333333333333",
            a,
            "Architect",
            "D:20261001091000Z",
            vec![("Contents", Object::string_literal("Agreed, will revise"))],
        ));
        let r2 = doc.add_object(mk(
            "44444444-4444-4444-8444-444444444444",
            r1,
            "Martin",
            "D:20261001092000Z",
            vec![("Contents", Object::string_literal("Thanks"))],
        ));
        let s1 = doc.add_object(mk(
            "55555555-5555-4555-8555-555555555555",
            a,
            "Architect",
            "D:20261001093000Z",
            vec![
                ("StateModel", Object::string_literal("Review")),
                ("State", Object::string_literal("Accepted")),
                (
                    "Contents",
                    Object::string_literal("Accepted set by Architect"),
                ),
                ("F", 30.into()),
            ],
        ));
        let s2 = doc.add_object(mk(
            "66666666-6666-4666-8666-666666666666",
            a,
            "Martin",
            "D:20261001094000Z",
            vec![
                ("StateModel", Object::string_literal("Review")),
                ("State", Object::string_literal("Completed")),
                (
                    "Contents",
                    Object::string_literal("Completed set by Martin"),
                ),
                ("F", 30.into()),
            ],
        ));
        let s3 = doc.add_object(mk(
            "77777777-7777-4777-8777-777777777777",
            a,
            "Martin",
            "D:20261001095000Z",
            vec![
                ("StateModel", Object::string_literal("Marked")),
                ("State", Object::string_literal("Marked")),
                ("Contents", Object::string_literal("Marked set by Martin")),
                ("F", 30.into()),
            ],
        ));
        let b = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Circle", "NM" => Object::string_literal(uuid_b),
            "Rect" => vec![300.into(), 500.into(), 360.into(), 560.into()], "F" => 4,
            "T" => Object::string_literal("Architect"), "Contents" => Object::string_literal("Circuit note"),
            "M" => Object::string_literal("D:20261001090000Z"), "CreationDate" => Object::string_literal("D:20261001090000Z"),
            "BSIColumnData" => vec![Object::string_literal("Electrical"), Object::string_literal("L4"), Object::string_literal("")],
            "BBMeasure" => dictionary! { "Scale" => Object::Real(0.5), "Unit" => "mm" },
            "NumCounts" => 3,
        });
        let sb = doc.add_object(mk(
            "88888888-8888-4888-8888-888888888888",
            b,
            "Architect",
            "D:20261001100000Z",
            vec![
                ("StateModel", Object::string_literal("Review")),
                ("State", Object::string_literal("Canceled")),
                ("F", 30.into()),
            ],
        ));
        let link = doc.add_object(crate::document::annots::tests::link_dict());
        let lr = doc.add_object(mk(
            "99999999-9999-4999-8999-999999999999",
            link,
            "Architect",
            "D:20261001101000Z",
            vec![("Contents", Object::string_literal("reply on a link"))],
        ));
        let page = doc.get_dictionary_mut(page_id).unwrap();
        page.set("Contents", Object::Reference(content));
        page.set(
            "Resources",
            dictionary! { "Font" => dictionary! { "F1" => Object::Reference(font) } },
        );
        page.set("BSISpaces", dictionary! { "Title" => Object::string_literal("Level 1"), "Kids" => Object::Array(vec![]) });
        page.set(
            "Annots",
            Object::Array(
                [a, r1, r2, s1, s2, s3, b, sb, link, lr]
                    .iter()
                    .map(|o| Object::Reference(*o))
                    .collect(),
            ),
        );
        doc
    }

    /// `REDLINE_REGEN_FIXTURES=1 cargo test regenerate_reviewed_fixture -- --ignored`
    #[test]
    #[ignore]
    fn regenerate_reviewed_fixture() {
        let mut doc = build_reviewed_fixture();
        doc.save(FIXTURE_PATH).unwrap();
    }

    /// Manual cross-check helper: `REDLINE_RESAVE_OUT=/path/out.pdf cargo test resave_reviewed_fixture -- --ignored`
    /// re-saves the fixture through redline so an independent reader (qpdf, mutool) can inspect it.
    #[test]
    #[ignore]
    fn resave_reviewed_fixture() {
        let out = std::env::var("REDLINE_RESAVE_OUT").expect("REDLINE_RESAVE_OUT");
        let mut doc = Document::load(FIXTURE_PATH).unwrap();
        let ms = read_markups(&doc).unwrap();
        write_markups(&mut doc, &ms).unwrap();
        doc.save(out).unwrap();
    }

    #[test]
    fn foreign_reviewed_pdf_shows_its_replies_and_statuses() {
        let doc =
            Document::load(FIXTURE_PATH).expect("committed fixture (regenerate_reviewed_fixture)");
        let ms = read_markups(&doc).unwrap();
        assert_eq!(
            ms.len(),
            2,
            "replies and states are not markups; the Link is not a markup"
        );
        let a = ms
            .iter()
            .find(|m| m.contents.as_deref() == Some("Relocate luminaire L4"))
            .unwrap();
        assert_eq!(
            a.workflow.status,
            MarkupStatus::Completed,
            "latest Review entry wins"
        );
        assert_eq!(
            a.workflow.status_history.len(),
            3,
            "Accepted, Completed and the Marked entry"
        );
        assert_eq!(a.workflow.reply_count(), 2);
        let texts: Vec<_> = a
            .workflow
            .thread
            .iter()
            .map(|r| r.contents.as_str())
            .collect();
        assert_eq!(texts, ["Agreed, will revise", "Thanks"]);
        assert_eq!(
            a.workflow.thread[1].in_reply_to,
            Some(a.workflow.thread[0].id),
            "nesting kept"
        );
        let b = ms
            .iter()
            .find(|m| m.contents.as_deref() == Some("Circuit note"))
            .unwrap();
        assert_eq!(
            b.workflow.status,
            MarkupStatus::Cancelled,
            "\"Canceled\" spelling accepted"
        );
    }

    #[test]
    fn saving_a_foreign_reviewed_pdf_preserves_private_keys_and_keeps_replies_attached() {
        let mut doc = Document::load(FIXTURE_PATH).unwrap();
        let ms = read_markups(&doc).unwrap();
        write_markups(&mut doc, &ms).unwrap();
        let doc = reload(&mut doc);

        let all = annots(&doc);
        // Private keys byte-for-byte equal to the fixture's.
        let src = Document::load(FIXTURE_PATH).unwrap();
        let src_b = annots(&src)
            .into_iter()
            .map(|(_, d)| d)
            .find(|d| text_of(d, b"Contents").as_deref() == Some("Circuit note"))
            .unwrap();
        let out_b = all
            .iter()
            .map(|(_, d)| d)
            .find(|d| {
                text_of(d, b"Contents").as_deref() == Some("Circuit note") && d.has(b"RLType")
            })
            .unwrap();
        for k in [&b"BSIColumnData"[..], b"BBMeasure", b"NumCounts"] {
            assert_eq!(
                out_b.get(k).unwrap(),
                src_b.get(k).unwrap(),
                "{}",
                String::from_utf8_lossy(k)
            );
        }
        // Page-level /BSISpaces untouched.
        let page = doc
            .get_dictionary(*doc.get_pages().values().next().unwrap())
            .unwrap();
        assert!(page.has(b"BSISpaces"));
        // Every IRT in the output resolves to an annotation that is on the page.
        let oids: HashSet<ObjectId> = all.iter().filter_map(|(o, _)| *o).collect();
        for (_, d) in &all {
            if let Some(p) = irt(d) {
                assert!(oids.contains(&p), "dangling /IRT");
            }
        }
        // 2 markups + (2 replies + 3 states) + (1 cancelled state) + link + foreign link reply
        assert_eq!(all.len(), 2 + 5 + 1 + 2);
        // Re-read is identical.
        let again = read_markups(&doc).unwrap();
        assert_eq!(again.len(), 2);
        let a = again
            .iter()
            .find(|m| m.contents.as_deref() == Some("Relocate luminaire L4"))
            .unwrap();
        assert_eq!(a.workflow.thread.len(), 2);
        assert_eq!(a.workflow.status, MarkupStatus::Completed);
    }

    #[test]
    fn pdf_dates_honour_the_local_offset_revu_and_acrobat_write() {
        let utc = |h, m| Utc.with_ymd_and_hms(2026, 10, 1, h, m, 0).unwrap();
        assert_eq!(parse_pdf_date("D:20261001090000Z"), Some(utc(9, 0)));
        assert_eq!(parse_pdf_date("D:20261001090000+04'00'"), Some(utc(5, 0)));
        assert_eq!(parse_pdf_date("D:20261001090000-05'30'"), Some(utc(14, 30)));
        assert_eq!(parse_pdf_date("D:20261001090000"), Some(utc(9, 0)));
        assert_eq!(parse_pdf_date("D:20261001"), Some(utc(0, 0)));
        assert_eq!(parse_pdf_date("garbage"), None);
    }

    #[test]
    fn derive_uuid_is_deterministic_and_seed_sensitive() {
        assert_eq!(derive_uuid("a"), derive_uuid("a"));
        assert_ne!(derive_uuid("a"), derive_uuid("b"));
    }

    // --- hostile / malformed input: must degrade, never panic -----------------

    fn text_annot(nm: &str, irt: Option<Object>, extra: Vec<(&str, Object)>) -> Dictionary {
        let mut d = dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "NM" => Object::string_literal(nm),
            "Rect" => vec![0.into(), 0.into(), 5.into(), 5.into()],
            "T" => Object::string_literal("X"), "Contents" => Object::string_literal(nm),
            "RT" => Object::Name(b"R".to_vec()),
        };
        if let Some(v) = irt {
            d.set("IRT", v);
        }
        for (k, v) in extra {
            d.set(k, v);
        }
        d
    }

    fn one_markup() -> Vec<Markup> {
        vec![redline_markup(0)]
    }

    #[test]
    fn dangling_self_and_cyclic_irt_are_skipped_not_panicked_on() {
        let m_oid: ObjectId = (1, 0);
        let (a, b, c, dangling, selfref) = ((2, 0), (3, 0), (4, 0), (5, 0), (6, 0));
        let raw = vec![
            (Some(a), text_annot("a", Some(Object::Reference(b)), vec![])), // a <-> b cycle
            (Some(b), text_annot("b", Some(Object::Reference(a)), vec![])),
            (
                Some(c),
                text_annot("c", Some(Object::Reference((99, 0))), vec![]),
            ), // dangling
            (
                Some(dangling),
                text_annot("d", Some(Object::Reference(c)), vec![]),
            ), // chain into dangling
            (
                Some(selfref),
                text_annot("s", Some(Object::Reference(selfref)), vec![]),
            ), // self
            (
                None,
                text_annot("inline", Some(Object::Reference(m_oid)), vec![]),
            ), // valid, inline
        ];
        let mut ms = one_markup();
        let consumed = attach(&mut ms, &[Some(m_oid)], &raw);
        assert!(
            consumed.is_empty(),
            "only the inline reply resolves and it has no oid"
        );
        assert_eq!(ms[0].workflow.thread.len(), 1);
        assert_eq!(ms[0].workflow.thread[0].contents, "inline");
    }

    #[test]
    fn missing_or_wrong_typed_irt_is_not_a_reply_and_does_not_panic() {
        let (mut doc, page_id) = one_page_doc();
        let wrong = [
            Object::Integer(5),
            Object::string_literal("(1 0 R)"),
            Object::Null,
            Object::Array(vec![Object::Reference((1, 0))]),
            Object::Dictionary(dictionary! {}),
        ];
        let mut ids = Vec::new();
        for (i, w) in wrong.into_iter().enumerate() {
            ids.push(Object::Reference(doc.add_object(text_annot(
                &format!("w{i}"),
                Some(w),
                vec![],
            ))));
        }
        ids.push(Object::Reference(doc.add_object(text_annot(
            "none",
            None,
            vec![],
        ))));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Array(ids));
        let ms = read_markups(&doc).unwrap(); // imported as plain Text markups, no panic
        assert_eq!(ms.len(), 6);
        assert!(ms.iter().all(|m| m.workflow.thread.is_empty()));
        // and a save with them in the store is fine
        let mut doc = doc;
        write_markups(&mut doc, &ms).unwrap();
    }

    #[test]
    fn bad_state_values_never_panic_and_never_drive_the_status() {
        let m_oid: ObjectId = (1, 0);
        let st = |nm: &str, state: Option<Object>, model: Option<Object>| {
            let mut extra = vec![];
            if let Some(s) = state {
                extra.push(("State", s));
            }
            if let Some(m) = model {
                extra.push(("StateModel", m));
            }
            text_annot(nm, Some(Object::Reference(m_oid)), extra)
        };
        let raw = vec![
            (
                Some((2, 0)),
                st(
                    "a",
                    Some(Object::string_literal("Garbage")),
                    Some(Object::string_literal("Review")),
                ),
            ),
            (
                Some((3, 0)),
                st("b", None, Some(Object::string_literal("Review"))),
            ), // missing State
            (
                Some((4, 0)),
                st(
                    "c",
                    Some(Object::Integer(7)),
                    Some(Object::string_literal("Review")),
                ),
            ), // wrong type
            (
                Some((5, 0)),
                st(
                    "d",
                    Some(Object::string_literal("Accepted")),
                    Some(Object::Integer(1)),
                ),
            ), // StateModel wrong type -> reply
            (
                Some((6, 0)),
                st(
                    "e",
                    Some(Object::string_literal("\u{0}\u{ffff}".repeat(10_000))),
                    Some(Object::string_literal("Review")),
                ),
            ),
            (
                Some((7, 0)),
                st(
                    "f",
                    Some(Object::string_literal("Accepted")),
                    Some(Object::string_literal("")),
                ),
            ),
        ];
        let mut ms = one_markup();
        attach(&mut ms, &[Some(m_oid)], &raw);
        let wf = &ms[0].workflow;
        // "b" has no State: treated as the default "None" (a valid Review state); the rest are
        // kept as history entries but unparseable states are ignored for the status.
        assert_eq!(wf.status_history.len(), 5);
        assert_eq!(wf.thread.len(), 1, "wrong-typed StateModel is just a reply");
        assert!(wf.latest_review_change().is_some());
        // Writing back must not panic either, and unknown states are carried verbatim.
        let mut doc = one_page_doc().0;
        let mut m = ms[0].clone();
        m.page = 0;
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        assert_eq!(
            read_markups(&doc).unwrap()[0].workflow.status_history.len(),
            5
        );
    }

    #[test]
    fn a_very_long_reply_chain_resolves_in_linear_time_without_recursion_or_a_depth_cap() {
        const N: u32 = 30_000;
        let m_oid: ObjectId = (1, 0);
        // reply i answers reply i-1, listed in REVERSE so a naive forward pass is worst case.
        let raw: Vec<_> = (0..N)
            .rev()
            .map(|i| {
                let parent = if i == 0 { m_oid } else { (i + 1, 0) };
                (
                    Some((i + 2, 0)),
                    text_annot(&format!("r{i}"), Some(Object::Reference(parent)), vec![]),
                )
            })
            .collect();
        let mut ms = one_markup();
        let consumed = attach(&mut ms, &[Some(m_oid)], &raw);
        assert_eq!(consumed.len(), N as usize);
        assert_eq!(ms[0].workflow.thread.len(), N as usize);
        // Writing orders parents first, linear, and never drops a reply.
        let order = thread_in_write_order(&ms[0].workflow.thread);
        assert_eq!(order.len(), N as usize);
        let pos: HashMap<Uuid, usize> = order.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
        assert!(order
            .iter()
            .all(|r| r.in_reply_to.is_none_or(|p| pos[&p] < pos[&r.id])));
    }

    #[test]
    fn a_long_chain_in_a_real_document_is_stripped_and_rewritten_without_loss() {
        let (mut doc, page_id) = one_page_doc();
        let mut m = redline_markup(0);
        let mut prev: Option<Uuid> = None;
        for i in 0..2_000u32 {
            let r = reply("A", &format!("r{i}"), i % 50, prev);
            prev = Some(r.id);
            m.workflow.thread.push(r);
        }
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let first = read_markups(&doc).unwrap();
        assert_eq!(first[0].workflow.thread.len(), 2_000);
        write_markups(&mut doc, &first).unwrap();
        let n = annots(&doc).len();
        assert_eq!(n, 1 + 2_000);
        let _ = page_id;
    }

    #[test]
    fn write_order_tolerates_self_parent_unknown_parent_and_cycles() {
        let mut thread = vec![
            reply("A", "a", 1, None),
            reply("A", "b", 2, None),
            reply("A", "c", 3, None),
        ];
        thread[0].in_reply_to = Some(thread[0].id); // self
        thread[1].in_reply_to = Some(Uuid::new_v4()); // unknown
        let (id_c, id_d) = (thread[2].id, Uuid::new_v4());
        thread[2].in_reply_to = Some(id_d);
        let mut d = reply("A", "d", 4, Some(id_c)); // c <-> d cycle
        d.id = id_d;
        thread.push(d);
        assert_eq!(
            thread_in_write_order(&thread).len(),
            4,
            "every reply is still written"
        );
    }

    #[test]
    fn hostile_dates_never_panic() {
        for s in [
            "D:20261001090000+99999999999999999999'00'",
            "D:20261001090000-99'99'",
            "D:99991231235959-99'59'",
            "D:00000101000000+99'00'",
            "D:20261301000000Z",
            "D:2026\u{e9}1001",
            "D:",
            "",
            "D:2026100109000\u{1F600}",
            "D:20261001090000+\u{1F600}'00'",
        ] {
            let _ = parse_pdf_date(s); // any result, no panic
        }
        assert_eq!(
            parse_pdf_date("D:99991231235959-99'59'"),
            None,
            "overflow is None, not a panic"
        );
    }

    #[test]
    fn hostile_text_in_reply_fields_is_stored_as_data() {
        let payload = ")>> /JS (app.alert(1)) /A << /S /JavaScript >> /X (";
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        let mut r = reply(payload, payload, 1, None);
        r.author.display_name = payload.to_string();
        m.workflow.thread = vec![r];
        m.workflow.status_history = vec![change(payload, payload, payload, 2)];
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let doc = reload(&mut doc);
        for (_, d) in annots(&doc) {
            for bad in [&b"JS"[..], b"A", b"AA", b"OpenAction", b"X"] {
                assert!(
                    !d.has(bad),
                    "annotation gained /{}",
                    String::from_utf8_lossy(bad)
                );
            }
        }
        let back = read_markups(&doc).unwrap();
        assert_eq!(back[0].workflow.thread[0].contents, payload);
        assert_eq!(back[0].workflow.thread[0].author.display_name, payload);
        assert_eq!(back[0].workflow.status_history[0].state, payload);
    }

    // --- round 1 review fixes ------------------------------------------------

    fn nm_set(doc: &Document) -> std::collections::BTreeSet<String> {
        annots(doc)
            .iter()
            .filter_map(|(_, d)| text_of(d, b"NM"))
            .collect()
    }

    /// Revu-style document: 16-char /NM on markups, replies and states.
    fn revu_style_doc() -> Document {
        let (mut doc, page_id) = one_page_doc();
        let square = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "F" => 4,
            "NM" => Object::string_literal("RMCTWBEQGCYHWCHZ"),
            "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
            "Contents" => Object::string_literal("square"), "T" => Object::string_literal("Martin"),
            "BSIColumnData" => vec![Object::string_literal("Electrical")],
        });
        let circle = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Circle", "F" => 4,
            "NM" => Object::string_literal("ZZZZYYYYXXXXWWWW"),
            "Rect" => vec![100.into(), 10.into(), 150.into(), 40.into()],
            "Contents" => Object::string_literal("circle"), "T" => Object::string_literal("Martin"),
        });
        let dates = |d: &str| {
            vec![
                ("M", Object::string_literal(d)),
                ("CreationDate", Object::string_literal(d)),
            ]
        };
        let reply = doc.add_object(text_annot(
            "ABCDEFGHIJKLMNOP",
            Some(Object::Reference(square)),
            [
                vec![("Contents", Object::string_literal("a reply"))],
                dates("D:20261001091000+04'00'"),
            ]
            .concat(),
        ));
        let state = doc.add_object(text_annot(
            "QRSTUVWXYZABCDEF",
            Some(Object::Reference(square)),
            [
                vec![
                    ("StateModel", Object::string_literal("Review")),
                    ("State", Object::string_literal("Rejected")),
                    ("F", 30.into()),
                ],
                dates("D:20261001092000+04'00'"),
            ]
            .concat(),
        ));
        let creply = doc.add_object(text_annot(
            "1234567890ABCDEF",
            Some(Object::Reference(circle)),
            [
                vec![("Contents", Object::string_literal("circle reply"))],
                dates("D:20261001093000+04'00'"),
            ]
            .concat(),
        ));
        doc.get_dictionary_mut(page_id).unwrap().set(
            "Annots",
            Object::Array(
                [square, circle, reply, state, creply]
                    .iter()
                    .map(|o| Object::Reference(*o))
                    .collect(),
            ),
        );
        doc
    }

    #[test]
    fn revu_style_nm_load_save_is_idempotent_and_keeps_every_nm() {
        let mut doc = revu_style_doc();
        let nms0 = nm_set(&doc);
        assert_eq!(nms0.len(), 5);
        let mut last_ids: Vec<Uuid> = Vec::new();
        for cycle in 1..=3 {
            let ms = read_markups(&doc).unwrap();
            assert_eq!(ms.len(), 2, "cycle {cycle}: two markups, no copies");
            let mut ids: Vec<Uuid> = ms.iter().map(|m| m.id()).collect();
            ids.sort();
            if !last_ids.is_empty() {
                assert_eq!(ids, last_ids, "cycle {cycle}: ids are stable across loads");
            }
            last_ids = ids;
            let sq = ms
                .iter()
                .find(|m| m.contents.as_deref() == Some("square"))
                .unwrap();
            assert_eq!(sq.workflow.thread.len(), 1);
            assert_eq!(sq.workflow.status, MarkupStatus::Rejected);
            write_markups(&mut doc, &ms).unwrap();
            let re = reload(&mut doc);
            assert_eq!(
                annots(&re).len(),
                5,
                "cycle {cycle}: annotation count stable"
            );
            assert_eq!(
                nm_set(&re),
                nms0,
                "cycle {cycle}: every original /NM unchanged"
            );
            let squares = annots(&re)
                .iter()
                .filter(|(_, d)| name_of(d, b"Subtype").as_deref() == Some("Square"))
                .count();
            assert_eq!(squares, 1);
            // private key still there
            let sqd = annots(&re)
                .into_iter()
                .find(|(_, d)| name_of(d, b"Subtype").as_deref() == Some("Square"))
                .unwrap()
                .1;
            assert!(sqd.has(b"BSIColumnData"));
            doc = re;
        }
    }

    #[test]
    fn revu_style_nm_id_is_derived_from_the_nm_so_two_loads_agree() {
        let a = read_markups(&revu_style_doc()).unwrap();
        let b = read_markups(&revu_style_doc()).unwrap();
        let ids = |v: &[Markup]| {
            v.iter()
                .map(|m| (m.source_nm.clone(), m.id()))
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&a), ids(&b));
        assert!(a.iter().all(|m| m.source_nm.is_some()));
    }

    #[test]
    fn two_annotations_sharing_one_nm_stay_two_markups_over_saves() {
        let (mut doc, page_id) = one_page_doc();
        let mk = |doc: &mut Document, x: i64| {
            Object::Reference(doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Square", "F" => 4,
                "NM" => Object::string_literal("DUPLICATEDNAME123"),
                "Rect" => vec![x.into(), 0.into(), (x + 10).into(), 10.into()],
            }))
        };
        let (a, b) = (mk(&mut doc, 0), mk(&mut doc, 50));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Array(vec![a, b]));
        for _ in 0..3 {
            let ms = read_markups(&doc).unwrap();
            assert_eq!(ms.len(), 2);
            assert_ne!(ms[0].id(), ms[1].id(), "ids stay unique");
            write_markups(&mut doc, &ms).unwrap();
            assert_eq!(annots(&doc).len(), 2);
        }
    }

    #[test]
    fn uppercase_uuid_nm_is_not_duplicated_either() {
        let (mut doc, page_id) = one_page_doc();
        let sq = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "F" => 4,
            "NM" => Object::string_literal("11111111-2222-4333-8444-ABCDEFABCDEF"),
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        });
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Array(vec![Object::Reference(sq)]));
        for _ in 0..3 {
            let ms = read_markups(&doc).unwrap();
            write_markups(&mut doc, &ms).unwrap();
            assert_eq!(annots(&doc).len(), 1);
        }
        assert_eq!(
            text_of(&annots(&doc)[0].1, b"NM").as_deref(),
            Some("11111111-2222-4333-8444-ABCDEFABCDEF")
        );
    }

    #[test]
    fn a_state_on_a_reply_stays_on_that_reply_and_does_not_drive_the_markup_status() {
        let (mut doc, page_id) = one_page_doc();
        let sq = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "F" => 4,
            "NM" => Object::string_literal("SQUAREONEONEONE1"),
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        });
        let reply = doc.add_object(text_annot(
            "REPLYONEONEONE11",
            Some(Object::Reference(sq)),
            vec![("Contents", Object::string_literal("r"))],
        ));
        let state = doc.add_object(text_annot(
            "STATEONREPLY1111",
            Some(Object::Reference(reply)),
            vec![
                ("StateModel", Object::string_literal("Review")),
                ("State", Object::string_literal("Rejected")),
            ],
        ));
        doc.get_dictionary_mut(page_id).unwrap().set(
            "Annots",
            Object::Array(
                [sq, reply, state]
                    .iter()
                    .map(|o| Object::Reference(*o))
                    .collect(),
            ),
        );
        for cycle in 1..=2 {
            let ms = read_markups(&doc).unwrap();
            assert_eq!(
                ms[0].workflow.status,
                MarkupStatus::None,
                "cycle {cycle}: reply-level state does not drive the markup"
            );
            assert_eq!(ms[0].workflow.status_history.len(), 1);
            assert!(ms[0].workflow.status_history[0].on_reply.is_some());
            assert!(ms[0].workflow.latest_review_change().is_none());
            write_markups(&mut doc, &ms).unwrap();
            let all = annots(&doc);
            assert_eq!(all.len(), 3);
            let (reply_oid, _) = all
                .iter()
                .find(|(_, d)| text_of(d, b"NM").as_deref() == Some("REPLYONEONEONE11"))
                .unwrap();
            let (_, st) = all
                .iter()
                .find(|(_, d)| text_of(d, b"NM").as_deref() == Some("STATEONREPLY1111"))
                .unwrap();
            assert_eq!(
                irt(st),
                *reply_oid,
                "cycle {cycle}: state still answers the reply, not the markup"
            );
        }
    }

    #[test]
    fn undated_replies_stay_undated_and_never_get_now() {
        let (mut doc, page_id) = one_page_doc();
        let sq = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "F" => 4, "NM" => Object::string_literal("SQUARETWOTWOTWO2"),
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        });
        let reply = doc.add_object(text_annot(
            "REPLYTWOTWOTWO22",
            Some(Object::Reference(sq)),
            vec![("Contents", Object::string_literal("no date"))],
        ));
        doc.get_dictionary_mut(page_id).unwrap().set(
            "Annots",
            Object::Array(vec![Object::Reference(sq), Object::Reference(reply)]),
        );
        for _ in 0..2 {
            let ms = read_markups(&doc).unwrap();
            assert_eq!(ms[0].workflow.thread[0].at, UNDATED);
            write_markups(&mut doc, &ms).unwrap();
            let (_, r) = annots(&doc)
                .into_iter()
                .find(|(_, d)| d.has(b"IRT"))
                .unwrap();
            assert!(
                !r.has(b"M") && !r.has(b"CreationDate"),
                "no invented date written"
            );
        }
    }

    #[test]
    fn state_model_and_state_written_as_names_are_still_read_as_a_state() {
        let m_oid: ObjectId = (1, 0);
        let raw = vec![(
            Some((2, 0)),
            text_annot(
                "n",
                Some(Object::Reference(m_oid)),
                vec![
                    ("StateModel", Object::Name(b"Review".to_vec())),
                    ("State", Object::Name(b"Accepted".to_vec())),
                ],
            ),
        )];
        let mut ms = one_markup();
        attach(&mut ms, &[Some(m_oid)], &raw);
        assert_eq!(ms[0].workflow.status, MarkupStatus::Accepted);
        assert!(
            ms[0].workflow.thread.is_empty(),
            "not misread as an empty reply"
        );
    }

    // --- round 2 review fixes ------------------------------------------------

    fn on_reply_state(who: &str, state: &str, min: u32, reply: Uuid) -> StatusChange {
        StatusChange {
            on_reply: Some(reply),
            ..change(who, "Review", state, min)
        }
    }

    #[test]
    fn deleting_a_reply_that_carries_a_state_drops_the_orphan_state_instead_of_reanchoring_it() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        let r = reply("A", "to be deleted", 1, None);
        m.workflow.thread = vec![r.clone()];
        m.workflow.status_history = vec![on_reply_state("A", "Rejected", 2, r.id)];
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let loaded = read_markups(&doc).unwrap();
        assert_eq!(loaded[0].workflow.status, MarkupStatus::None);

        // The user deletes the reply, but the in-memory history still holds its state
        // (a stale client, or any path that does not prune): the save must not re-anchor it.
        let mut edited = loaded[0].clone();
        edited.workflow.thread.clear();
        write_markups(&mut doc, std::slice::from_ref(&edited)).unwrap();
        assert!(
            annots(&doc).iter().all(|(_, d)| !d.has(b"StateModel")),
            "no orphan state written"
        );
        let reopened = read_markups(&doc).unwrap();
        assert_eq!(
            reopened[0].workflow.status,
            MarkupStatus::None,
            "markup status unchanged"
        );
        assert!(reopened[0].workflow.status_history.is_empty());
    }

    #[test]
    fn a_state_on_a_state_is_written_under_that_state_and_the_walk_is_linear() {
        let (mut doc, _) = one_page_doc();
        let mut m = redline_markup(0);
        let r = reply("A", "r", 1, None);
        let s1 = on_reply_state("A", "Accepted", 2, r.id);
        let s2 = on_reply_state("A", "Rejected", 3, s1.id);
        m.workflow.thread = vec![r];
        // listed child-first so the writer has to wait for the parent state
        m.workflow.status_history = vec![s2.clone(), s1.clone()];
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let all = annots(&doc);
        let st = |id: &Uuid| {
            all.iter()
                .find(|(_, d)| text_of(d, b"NM").as_deref() == Some(&id.to_string()))
                .unwrap()
        };
        assert_eq!(irt(&st(&s2.id).1), st(&s1.id).0, "state answers the state");
        assert_eq!(
            read_markups(&doc).unwrap()[0].workflow.status,
            MarkupStatus::None
        );

        // a long chain of states (hostile size) does not blow up
        let mut m = redline_markup(0);
        let r = reply("A", "r", 1, None);
        let mut prev = r.id;
        let mut hist = Vec::new();
        for i in 0..5_000 {
            let c = on_reply_state("A", "Accepted", i % 50, prev);
            prev = c.id;
            hist.push(c);
        }
        hist.reverse();
        m.workflow.thread = vec![r];
        m.workflow.status_history = hist;
        let mut doc = one_page_doc().0;
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        assert_eq!(annots(&doc).len(), 1 + 1 + 5_000);
    }

    // F10: reader-owned names must never come from outside.

    fn through_json(m: &Markup, patch: impl FnOnce(&mut serde_json::Value)) -> Markup {
        let mut v = serde_json::to_value(m).unwrap();
        patch(&mut v);
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_json_supplied_source_nm_cannot_make_a_save_delete_a_foreign_annotation() {
        use crate::document::store::MarkupStore;
        let (mut doc, page_id) = one_page_doc();
        let foreign = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "F" => 4,
            "NM" => Object::string_literal("RMCTWBEQGCYHWCHZ"),
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "Contents" => Object::string_literal("someone else's markup"),
        });
        let foreign_reply = doc.add_object(text_annot(
            "FOREIGNREPLYNAME",
            Some(Object::Reference(foreign)),
            vec![("Contents", Object::string_literal("their reply"))],
        ));
        doc.get_dictionary_mut(page_id).unwrap().set(
            "Annots",
            Object::Array(vec![
                Object::Reference(foreign),
                Object::Reference(foreign_reply),
            ]),
        );

        // The attacker's markup arrives as JSON claiming the foreign annotation's names.
        let mut base = redline_markup(0);
        base.workflow.thread = vec![reply("A", "x", 1, None)];
        base.workflow.status_history = vec![change("A", "Review", "Accepted", 2)];
        let crafted = through_json(&base, |v| {
            v["source_nm"] = "RMCTWBEQGCYHWCHZ".into();
            v["workflow"]["thread"][0]["nm"] = "FOREIGNREPLYNAME".into();
            v["workflow"]["status_history"][0]["nm"] = "RMCTWBEQGCYHWCHZ".into();
        });
        assert_eq!(
            crafted.source_nm.as_deref(),
            Some("RMCTWBEQGCYHWCHZ"),
            "precondition: JSON does carry it"
        );

        let store = MarkupStore::default();
        store.register("d", std::path::PathBuf::from("/tmp/x.pdf"), None);
        store.add("d", crafted).unwrap();
        let stored = store.list("d").unwrap();
        assert!(stored[0].source_nm.is_none());
        assert!(
            stored[0].workflow.thread[0].nm.is_none()
                && stored[0].workflow.status_history[0].nm.is_none()
        );

        write_markups(&mut doc, &stored).unwrap();
        let nms = nm_set(&doc);
        assert!(nms.contains("RMCTWBEQGCYHWCHZ"), "foreign markup survives");
        assert!(nms.contains("FOREIGNREPLYNAME"), "foreign reply survives");
        let texts: Vec<_> = annots(&doc)
            .iter()
            .filter_map(|(_, d)| text_of(d, b"Contents"))
            .collect();
        assert!(texts.contains(&"someone else's markup".to_string()));
        assert!(texts.contains(&"their reply".to_string()));
    }

    #[test]
    fn an_update_keeps_the_stored_names_and_ignores_inbound_ones() {
        use crate::document::store::MarkupStore;
        let (doc, _) = one_page_doc();
        let _ = doc;
        let mut m = redline_markup(0);
        let r = reply("A", "x", 1, None);
        m.source_nm = Some("ORIGINALNAME0001".into());
        m.workflow.thread = vec![Reply {
            nm: Some("ORIGINALREPLY001".into()),
            ..r.clone()
        }];
        let store = MarkupStore::default();
        store.register("d", std::path::PathBuf::from("/tmp/x.pdf"), None);
        // loaded from a PDF = reader-owned fields present (seed_loaded is the trusted path)
        store.seed_loaded("d", vec![m.clone()]).unwrap();

        // inbound update: names tampered, plus one brand-new reply with a claimed name
        let new_reply = reply("B", "new", 2, None);
        let inbound = through_json(&m, |v| {
            v["source_nm"] = "SOMETHINGELSE0000".into();
            v["workflow"]["thread"][0]["nm"] = "TAMPERED00000000".into();
            v["workflow"]["thread"].as_array_mut().unwrap().push(
                serde_json::to_value(Reply {
                    nm: Some("CLAIMEDNAME00000".into()),
                    ..new_reply.clone()
                })
                .unwrap(),
            );
        });
        store.update("d", inbound).unwrap();
        let got = &store.list("d").unwrap()[0];
        assert_eq!(got.source_nm.as_deref(), Some("ORIGINALNAME0001"));
        let nm_of = |id| {
            got.workflow
                .thread
                .iter()
                .find(|r| r.id == id)
                .unwrap()
                .nm
                .clone()
        };
        assert_eq!(
            nm_of(r.id).as_deref(),
            Some("ORIGINALREPLY001"),
            "existing reply keeps its original name"
        );
        assert_eq!(nm_of(new_reply.id), None, "a new reply cannot claim a name");
    }

    // --- round 3: undo of a delete (tombstone) -------------------------------

    fn new_store(name: &str) -> crate::document::store::MarkupStore {
        let store = crate::document::store::MarkupStore::default();
        store.register(name, std::path::PathBuf::from("/tmp/x.pdf"), None);
        store
    }

    #[test]
    fn delete_then_undo_of_a_revu_markup_does_not_duplicate_it_or_its_replies() {
        let mut doc = revu_style_doc();
        let nms0 = nm_set(&doc);
        let store = new_store("d");
        store.seed_loaded("d", read_markups(&doc).unwrap()).unwrap();
        let sq = store
            .list("d")
            .unwrap()
            .into_iter()
            .find(|m| m.contents.as_deref() == Some("square"))
            .unwrap();
        assert!(sq.source_nm.is_some() && !sq.workflow.thread.is_empty());

        store.delete("d", sq.id()).unwrap();
        // Undo: the frontend re-sends the deleted markup as an add, through JSON.
        let resent = through_json(&sq, |_| {});
        store.add("d", resent).unwrap();

        for cycle in 1..=2 {
            let ms = store.list("d").unwrap();
            assert_eq!(ms.len(), 2);
            write_markups(&mut doc, &ms).unwrap();
            doc = reload(&mut doc);
            let all = annots(&doc);
            let squares = all
                .iter()
                .filter(|(_, d)| name_of(d, b"Subtype").as_deref() == Some("Square"))
                .count();
            let reply_annots = all.iter().filter(|(_, d)| d.has(b"IRT")).count();
            assert_eq!(
                (squares, reply_annots, all.len()),
                (1, 3, 5),
                "cycle {cycle}"
            );
            assert_eq!(nm_set(&doc), nms0, "cycle {cycle}: original /NMs preserved");
        }
    }

    #[test]
    fn a_crafted_add_cannot_claim_a_foreign_nm_even_with_the_derived_id() {
        let (mut doc, page_id) = one_page_doc();
        // A foreign annotation redline never loads (unsupported subtype).
        let foreign = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Underline", "F" => 4,
            "NM" => Object::string_literal("UNSUPPORTEDSUBT01"),
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "QuadPoints" => vec![0.into(), 10.into(), 10.into(), 10.into(), 0.into(), 0.into(), 10.into(), 0.into()],
        });
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Array(vec![Object::Reference(foreign)]));
        let crafted = through_json(&redline_markup(0), |v| {
            v["id"] = derive_uuid("nm:UNSUPPORTEDSUBT01").to_string().into();
            v["source_nm"] = "UNSUPPORTEDSUBT01".into();
        });
        assert_eq!(
            crafted.id(),
            derive_uuid("nm:UNSUPPORTEDSUBT01"),
            "precondition"
        );
        let store = new_store("d");
        store.add("d", crafted).unwrap(); // no tombstone for this id
        let ms = store.list("d").unwrap();
        assert!(
            ms[0].source_nm.is_none(),
            "no tombstone, so nothing adopted"
        );
        write_markups(&mut doc, &ms).unwrap();
        assert!(
            nm_set(&doc).contains("UNSUPPORTEDSUBT01"),
            "foreign annotation survives the save"
        );
        assert_eq!(annots(&doc).len(), 2);
    }

    #[test]
    fn a_tombstone_only_restores_what_the_store_held_not_what_the_add_claims() {
        let store = new_store("d");
        let mut m = redline_markup(0);
        m.source_nm = Some("ORIGINALNAME0001".into());
        store.seed_loaded("d", vec![m.clone()]).unwrap();
        store.delete("d", m.id()).unwrap();
        let undo = through_json(&m, |v| v["source_nm"] = "SOMETHINGELSE0000".into());
        store.add("d", undo).unwrap();
        assert_eq!(
            store.list("d").unwrap()[0].source_nm.as_deref(),
            Some("ORIGINALNAME0001")
        );
        // the entry is consumed: a second delete/add cycle works, and a tampered re-add after a
        // plain delete of a markup WITHOUT names gets nothing
        let plain = redline_markup(0);
        store.add("d", plain.clone()).unwrap();
        store.delete("d", plain.id()).unwrap();
        let tampered = through_json(&plain, |v| v["source_nm"] = "CLAIMEDNAME00000".into());
        store.add("d", tampered).unwrap();
        let got = store.list("d").unwrap();
        assert!(got
            .iter()
            .find(|x| x.id() == plain.id())
            .unwrap()
            .source_nm
            .is_none());
    }

    #[test]
    fn delete_without_undo_still_removes_the_redline_copy_on_save() {
        let (mut doc, _) = one_page_doc();
        let store = new_store("d");
        let m = redline_markup(0);
        store.add("d", m.clone()).unwrap();
        write_markups(&mut doc, &store.list("d").unwrap()).unwrap();
        assert_eq!(annots(&doc).len(), 1);
        store.delete("d", m.id()).unwrap();
        write_markups(&mut doc, &store.list("d").unwrap()).unwrap();
        assert!(
            annots(&doc).is_empty(),
            "deleted redline markup is gone from the PDF"
        );
    }

    #[test]
    fn closing_or_reloading_the_document_clears_its_tombstones() {
        let store = new_store("d");
        let mut m = redline_markup(0);
        m.source_nm = Some("ORIGINALNAME0001".into());
        store.seed_loaded("d", vec![m.clone()]).unwrap();
        store.delete("d", m.id()).unwrap();
        store.seed_loaded("d", vec![]).unwrap(); // reload
        store.add("d", through_json(&m, |_| {})).unwrap();
        assert!(
            store.list("d").unwrap()[0].source_nm.is_none(),
            "tombstone cleared by reload"
        );
        store.remove("d");
        let store2 = new_store("d"); // reopened
        store2.add("d", through_json(&m, |_| {})).unwrap();
        assert!(store2.list("d").unwrap()[0].source_nm.is_none());
    }
}
