//! Per-view layer visibility without touching the file.
//!
//! PDFium reads optional-content visibility from the document it loaded and offers no call
//! to change it. So a view state is rendered by loading the SAME file with an incremental
//! update appended in memory: a new `/D` configuration (the groups to hide in `/OFF`) and,
//! where a group's own `/Usage /View /ViewState` would outrank it, that group's object.
//! [`OverlayReader`] presents `file bytes + update` to PDFium as one seekable stream; the
//! file on disk is never opened for writing and nothing is copied.
//!
//! Scope: unencrypted files whose bytes start at the `%PDF-` header and are under 4 GiB
//! (offsets are written in 4 bytes). Anything else reports a reason and the layer panel
//! stays read-only for that file.

use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use lopdf::{xref::XrefType, Dictionary, Document, Object, ObjectId};

use super::{LayerModel, MAX_LAYERS};

/// Files at or above this size are not overlaid (xref offsets are 4 bytes).
const MAX_OVERLAY_FILE: u64 = u32::MAX as u64;

/// Which object holds the `/D` configuration, and so is rewritten.
#[derive(Debug, Clone)]
enum Target {
    /// `/D` is its own indirect object.
    D(ObjectId),
    /// `/OCProperties` is an indirect object with a direct `/D`.
    OcProperties(ObjectId),
    /// `/OCProperties` and `/D` are direct inside the catalog.
    Catalog(ObjectId),
}

/// What is needed to build an update for one file, captured once from its parsed form.
#[derive(Debug, Clone)]
pub struct OverlayBase {
    pub path: PathBuf,
    pub file_len: u64,
    xref_start: u64,
    xref_stream: bool,
    /// First object number not in use.
    next_obj: u32,
    root: ObjectId,
    info: Option<Object>,
    id: Option<Object>,
    target: Target,
    catalog: Dictionary,
    ocprops: Dictionary,
    d: Dictionary,
    /// Groups carrying `/Usage /View /ViewState`: the group dictionary with `/Usage` and
    /// `/View` inlined, to be rewritten when the view state disagrees with it.
    fixups: HashMap<u32, (u16, Dictionary)>,
    /// `(id, gen)` and view intent of every layer, for the `/OFF` array.
    layers: Vec<(u32, u16, bool)>,
}

/// A direct copy of the object a value resolves to, as a dictionary.
fn resolved_dict(doc: &Document, o: &Object) -> Option<Dictionary> {
    doc.dereference(o).ok()?.1.as_dict().ok().cloned()
}

impl OverlayBase {
    /// `Err(reason)` when this file cannot be overlaid.
    pub fn new(doc: &Document, model: &LayerModel, path: &Path) -> Result<Self, String> {
        if doc.is_encrypted() || model.encrypted {
            return Err("encrypted file".into());
        }
        if model.layers_truncated {
            // The view rewrites the whole configuration from the groups read; the ones past the
            // read bound would be switched on.
            return Err("more layers than redline reads".into());
        }
        let file_len = std::fs::metadata(path)
            .map_err(|e| format!("stat: {e}"))?
            .len();
        if file_len >= MAX_OVERLAY_FILE {
            return Err("file is 4 GiB or larger".into());
        }
        let mut head = [0u8; 5];
        std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut head))
            .map_err(|e| format!("read header: {e}"))?;
        if &head != b"%PDF-" {
            return Err("data before the PDF header".into());
        }
        let xref_start = doc.xref_start as u64;
        if xref_start == 0 || xref_start >= file_len {
            return Err("cross-reference position not found".into());
        }
        let xref_stream = matches!(
            doc.reference_table.cross_reference_type,
            XrefType::CrossReferenceStream
        );
        let root = doc
            .trailer
            .get(b"Root")
            .ok()
            .and_then(|o| o.as_reference().ok())
            .ok_or("no catalog")?;
        let catalog = doc.get_dictionary(root).map_err(|_| "no catalog")?.clone();

        let ocp_obj = catalog
            .get(b"OCProperties")
            .map_err(|_| "no optional content")?;
        let ocp_ref = ocp_obj.as_reference().ok();
        let ocprops = resolved_dict(doc, ocp_obj).ok_or("unreadable optional content")?;
        let (target, d) = match ocprops.get(b"D") {
            Ok(Object::Reference(r)) => (
                Target::D(*r),
                resolved_dict(doc, &Object::Reference(*r)).unwrap_or_default(),
            ),
            Ok(o) => (
                match ocp_ref {
                    Some(r) => Target::OcProperties(r),
                    None => Target::Catalog(root),
                },
                resolved_dict(doc, o).unwrap_or_default(),
            ),
            Err(_) => (
                match ocp_ref {
                    Some(r) => Target::OcProperties(r),
                    None => Target::Catalog(root),
                },
                Dictionary::new(),
            ),
        };

        let size_in_trailer = doc
            .trailer
            .get(b"Size")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0)
            .clamp(0, i64::from(u32::MAX - 2)) as u32;
        let next_obj = size_in_trailer.max(doc.max_id.saturating_add(1));

        let mut fixups = HashMap::new();
        for l in model.layers.iter().take(MAX_LAYERS) {
            if l.usage_view_state.is_none() {
                continue;
            }
            let Ok(Object::Dictionary(g)) = doc.get_object((l.id, l.gen)) else {
                continue;
            };
            let mut g = g.clone();
            let Some(mut usage) = g.get(b"Usage").ok().and_then(|o| resolved_dict(doc, o)) else {
                continue;
            };
            let Some(view) = usage.get(b"View").ok().and_then(|o| resolved_dict(doc, o)) else {
                continue;
            };
            usage.set("View", Object::Dictionary(view));
            g.set("Usage", Object::Dictionary(usage));
            fixups.insert(l.id, (l.gen, g));
        }

        Ok(Self {
            path: path.to_path_buf(),
            file_len,
            xref_start,
            xref_stream,
            next_obj,
            root,
            info: doc.trailer.get(b"Info").ok().cloned(),
            id: doc.trailer.get(b"ID").ok().cloned(),
            target,
            catalog,
            ocprops,
            d,
            fixups,
            layers: model
                .layers
                .iter()
                .map(|l| (l.id, l.gen, l.view_intent))
                .collect(),
        })
    }

    /// The objects an update must carry so that exactly `off` is hidden.
    fn objects(&self, off: &BTreeSet<u32>) -> Vec<(ObjectId, Object)> {
        let mut d = self.d.clone();
        d.remove(b"BaseState");
        d.remove(b"ON");
        let hidden: Vec<Object> = self
            .layers
            .iter()
            .filter(|(id, _, intent)| *intent && off.contains(id))
            .map(|(id, gen, _)| Object::Reference((*id, *gen)))
            .collect();
        if hidden.is_empty() {
            d.remove(b"OFF");
        } else {
            d.set("OFF", Object::Array(hidden));
        }
        let mut out: Vec<(ObjectId, Object)> = Vec::new();
        match &self.target {
            Target::D(id) => out.push((*id, Object::Dictionary(d))),
            Target::OcProperties(id) => {
                let mut p = self.ocprops.clone();
                p.set("D", Object::Dictionary(d));
                out.push((*id, Object::Dictionary(p)));
            }
            Target::Catalog(id) => {
                let mut p = self.ocprops.clone();
                p.set("D", Object::Dictionary(d));
                let mut c = self.catalog.clone();
                c.set("OCProperties", Object::Dictionary(p));
                out.push((*id, Object::Dictionary(c)));
            }
        }
        // A group's own ViewState outranks the configuration: bring it in line.
        let mut fix: Vec<_> = self.fixups.iter().collect();
        fix.sort_by_key(|(id, _)| **id);
        for (id, (gen, g)) in fix {
            let want_on = !off.contains(id);
            let mut g = g.clone();
            let Some(Object::Dictionary(usage)) = g.get_mut(b"Usage").ok().map(|o| &mut *o) else {
                continue;
            };
            let Some(Object::Dictionary(view)) = usage.get_mut(b"View").ok().map(|o| &mut *o)
            else {
                continue;
            };
            view.set(
                "ViewState",
                Object::Name(if want_on {
                    b"ON".to_vec()
                } else {
                    b"OFF".to_vec()
                }),
            );
            out.push(((*id, *gen), Object::Dictionary(g)));
        }
        out
    }

    /// The bytes to append to the file so that exactly the groups in `off` are hidden.
    pub fn tail(&self, off: &BTreeSet<u32>) -> Result<Vec<u8>, String> {
        let objs = self.objects(off);
        let mut buf: Vec<u8> = vec![b'\n'];
        let mut entries: Vec<(u32, u16, u64)> = Vec::new();
        for ((num, gen), o) in &objs {
            entries.push((*num, *gen, self.file_len + buf.len() as u64));
            writeln!(buf, "{num} {gen} obj").map_err(|e| e.to_string())?;
            write_object(&mut buf, o)?;
            buf.extend_from_slice(b"\nendobj\n");
        }
        let max_num = entries.iter().map(|e| e.0).max().unwrap_or(0);
        let size = self.next_obj.max(max_num + 1);
        let xref_off = self.file_len + buf.len() as u64;

        let mut trailer = Dictionary::new();
        trailer.set("Root", Object::Reference(self.root));
        trailer.set("Prev", Object::Integer(self.xref_start as i64));
        if let Some(i) = &self.info {
            trailer.set("Info", i.clone());
        }
        if let Some(i) = &self.id {
            trailer.set("ID", i.clone());
        }

        if self.xref_stream {
            let xnum = size;
            entries.push((xnum, 0, xref_off));
            entries.sort_by_key(|e| e.0);
            let mut data = Vec::with_capacity(entries.len() * 7);
            let mut index = Vec::new();
            for (num, gen, off) in &entries {
                let off = u32::try_from(*off).map_err(|_| "offset beyond 4 GiB".to_string())?;
                data.push(1u8);
                data.extend_from_slice(&off.to_be_bytes());
                data.extend_from_slice(&gen.to_be_bytes());
                index.push(Object::Integer(i64::from(*num)));
                index.push(Object::Integer(1));
            }
            trailer.set("Type", Object::Name(b"XRef".to_vec()));
            trailer.set("Size", Object::Integer(i64::from(xnum) + 1));
            trailer.set(
                "W",
                Object::Array(vec![
                    Object::Integer(1),
                    Object::Integer(4),
                    Object::Integer(2),
                ]),
            );
            trailer.set("Index", Object::Array(index));
            trailer.set("Length", Object::Integer(data.len() as i64));
            writeln!(buf, "{xnum} 0 obj").map_err(|e| e.to_string())?;
            write_object(&mut buf, &Object::Dictionary(trailer))?;
            buf.extend_from_slice(b"\nstream\n");
            buf.extend_from_slice(&data);
            buf.extend_from_slice(b"\nendstream\nendobj\n");
        } else {
            entries.sort_by_key(|e| e.0);
            buf.extend_from_slice(b"xref\n");
            for (num, gen, off) in &entries {
                write!(buf, "{num} 1\n{off:010} {gen:05} n \n").map_err(|e| e.to_string())?;
            }
            trailer.set("Size", Object::Integer(i64::from(size)));
            buf.extend_from_slice(b"trailer\n");
            write_object(&mut buf, &Object::Dictionary(trailer))?;
            buf.push(b'\n');
        }
        write!(buf, "startxref\n{xref_off}\n%%EOF\n").map_err(|e| e.to_string())?;
        Ok(buf)
    }

    /// A reader presenting the file plus the update for `off`. Fails when the file's size
    /// is no longer what the base was read from.
    pub fn reader(&self, off: &BTreeSet<u32>) -> Result<OverlayReader, String> {
        let f = std::fs::File::open(&self.path).map_err(|e| format!("open: {e}"))?;
        let len = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
        if len != self.file_len {
            return Err("the file changed on disk".into());
        }
        Ok(OverlayReader {
            file: f,
            file_pos: 0,
            base_len: self.file_len,
            tail: self.tail(off)?,
            pos: 0,
        })
    }
}

/// `file bytes ++ tail` as one `Read + Seek` stream.
pub struct OverlayReader {
    file: std::fs::File,
    file_pos: u64,
    base_len: u64,
    tail: Vec<u8>,
    pos: u64,
}

impl OverlayReader {
    pub fn len(&self) -> u64 {
        self.base_len + self.tail.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Read for OverlayReader {
    /// Fills `buf` completely unless the stream ends first: PDFium's file-access callback
    /// (via pdfium-render) issues ONE `read` per block and treats a short count as the data,
    /// so a read that crosses from the file into the update must not stop at the seam.
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut filled = 0;
        while filled < buf.len() && self.pos < self.len() {
            if self.pos < self.base_len {
                if self.file_pos != self.pos {
                    self.file.seek(SeekFrom::Start(self.pos))?;
                    self.file_pos = self.pos;
                }
                let want = (buf.len() - filled).min((self.base_len - self.pos) as usize);
                let n = self.file.read(&mut buf[filled..filled + want])?;
                if n == 0 {
                    break; // the file is shorter than when it was read
                }
                self.pos += n as u64;
                self.file_pos += n as u64;
                filled += n;
            } else {
                let off = (self.pos - self.base_len) as usize;
                let n = (buf.len() - filled).min(self.tail.len() - off);
                buf[filled..filled + n].copy_from_slice(&self.tail[off..off + n]);
                self.pos += n as u64;
                filled += n;
            }
        }
        Ok(filled)
    }
}

impl Seek for OverlayReader {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let next = match from {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        match next {
            Some(p) => {
                self.pos = p;
                Ok(p)
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start",
            )),
        }
    }
}

/// Serialise a PDF object. Streams are not needed here and are refused.
fn write_object(out: &mut Vec<u8>, o: &Object) -> Result<(), String> {
    match o {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Integer(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Object::Real(r) => {
            if !r.is_finite() {
                return Err("non-finite number".into());
            }
            let s = format!("{r:.6}");
            out.extend_from_slice(s.trim_end_matches('0').trim_end_matches('.').as_bytes());
        }
        Object::Name(n) => {
            out.push(b'/');
            for &b in n {
                if b <= b' ' || b >= 0x7f || b"()<>[]{}/%#".contains(&b) {
                    out.extend_from_slice(format!("#{b:02X}").as_bytes());
                } else {
                    out.push(b);
                }
            }
        }
        Object::String(s, _) => {
            out.push(b'<');
            for b in s {
                out.extend_from_slice(format!("{b:02X}").as_bytes());
            }
            out.push(b'>');
        }
        Object::Array(a) => {
            out.push(b'[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_object(out, e)?;
            }
            out.push(b']');
        }
        Object::Dictionary(d) => {
            out.extend_from_slice(b"<<");
            for (k, v) in d.iter() {
                write_object(out, &Object::Name(k.clone()))?;
                out.push(b' ');
                write_object(out, v)?;
                out.push(b' ');
            }
            out.extend_from_slice(b">>");
        }
        Object::Reference((n, g)) => out.extend_from_slice(format!("{n} {g} R").as_bytes()),
        Object::Stream(_) => return Err("stream in an update object".into()),
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn write_object_for_test(o: &Object) -> Result<Vec<u8>, String> {
    let mut v = Vec::new();
    write_object(&mut v, o)?;
    Ok(v)
}
