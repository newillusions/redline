//! Image-aware recompression for `docops::optimize` (spec §8).
//!
//! # Why this module exists
//!
//! `optimize_in_place`'s original v1 baseline (prune + generic Deflate on uncompressed
//! streams) never touches raster image content: JPEG (`DCTDecode`) streams are already
//! filtered so `Document::compress()` skips them by construction (it only compresses
//! streams with no `/Filter` at all), and raw uncompressed bitmap samples wrapped in
//! `FlateDecode` are *already* Deflate-compressed as bytes, so re-running Deflate on them
//! saves nothing. On real AEC plan sets, raster images are the overwhelming majority of
//! file bytes (measured on the workspace's own c1 corpus tier: 2,488 images = 89.6% of a
//! 110 MB file; the old baseline achieved a 0.07% reduction), which is exactly the
//! "reduced by 9 KB or something insanely silly" complaint this module fixes.
//!
//! # Design
//!
//! Mirrors Bluebeam's "Reduce File Size" model: a compression/quality preset
//! ([`ImageQualityPreset`]) drives both a target effective resolution (DPI) and a JPEG
//! quality factor. For each eligible image XObject:
//!
//! 1. Determine its *physical* placement size in PDF user-space points by walking page
//!    (and one level of nested Form XObject) content streams, tracking the CTM through
//!    `q`/`Q`/`cm`, and recording the transformed unit square for every `Do` that
//!    references it. The **largest** placement across all occurrences wins, so an image
//!    reused at multiple scales is never downsampled below what its biggest placement
//!    needs.
//! 2. Compute the pixel dimensions that satisfy `preset.target_dpi()` at that placement
//!    size, capped so we **never upsample** (only ever shrink or leave alone). An image
//!    with no discoverable placement (not reached by the content-stream walk) is treated
//!    conservatively: quality re-encode only, no resize.
//! 3. Decode, optionally resize (Lanczos3), re-encode as baseline JPEG at
//!    `preset.jpeg_quality()`, and only replace the stream if the result is strictly
//!    smaller than the original (never regress an already-efficient image).
//!
//! # Safety bar (spec §8, "never risk a wrong pixel on a construction drawing")
//!
//! Every image is checked against [`eligibility`] before being touched. Anything outside
//! the supported class is passed through byte-for-byte and counted in
//! [`ImageOptimizeStats::skip_reasons`] rather than guessed at:
//!
//! - `/ImageMask true` (stencil masks) — always skipped.
//! - An explicit `/Mask` (color-key array or separate stencil-mask image) — always
//!   skipped; combining a lossy recompress with an unrelated masking channel is exactly
//!   the kind of interaction this module refuses to guess about.
//! - A `/Decode` array — always skipped (signals a non-default sample interpretation;
//!   reinterpreting it correctly is out of scope for v1).
//! - Color spaces other than `DeviceGray`/`CalGray`/`ICCBased` N=1 (gray) and
//!   `DeviceRGB`/`CalRGB`/`ICCBased` N=3 (RGB) — `Indexed`, `DeviceCMYK`, `Separation`,
//!   `DeviceN`, `Lab`, and anything else are passed through untouched.
//! - Anything other than `BitsPerComponent` 8 for raw (unfiltered/`FlateDecode`) samples.
//! - Filters other than: absent, `FlateDecode` (raw samples), `DCTDecode` (JPEG), or the
//!   `[FlateDecode DCTDecode]` double-wrap some PDF writers emit. In particular
//!   `JPXDecode` (JPEG2000), `CCITTFaxDecode`/`JBIG2Decode` (bitonal fax/scan formats —
//!   already extremely size-efficient and easy to corrupt via a naive RGB/Gray
//!   reinterpretation), and multi-filter combinations beyond the one named double-wrap
//!   are passed through untouched.
//! - A decode or re-encode that doesn't strictly shrink the stream is discarded — the
//!   original bytes are kept, and the image is counted as passed-through, not
//!   recompressed.
//!
//! `/SMask` (soft-mask alpha channel) is recompressed through the same eligibility path,
//! at the same placement size as its owning image, when present and itself eligible —
//! otherwise it is left untouched (a soft mask's own `/Width`/`/Height` are independent of
//! the base image's, so leaving it at native resolution is always valid, just less
//! space-efficient).

use anyhow::Result;
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::{BTreeMap, HashMap, HashSet};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compression/quality preset for image recompression, in the spirit of Bluebeam's
/// "Reduce File Size" slider (a single knob trading file size against visual quality).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageQualityPreset {
    /// Minimal quality loss: 300 DPI effective resolution cap, JPEG quality 90.
    /// Close to Bluebeam's "High Quality (Print)" preset.
    High,
    /// Default trade-off: 200 DPI, JPEG quality 75. Matches Bluebeam's mid-slider
    /// "Standard" behaviour for on-screen/plan-review use.
    Balanced,
    /// Aggressive size reduction: 150 DPI, JPEG quality 50. For distribution copies
    /// where file size matters more than print fidelity.
    Small,
}

impl ImageQualityPreset {
    /// Target effective resolution, in pixels per inch at the image's *largest* placed
    /// size on the page. Images already at or below this are never upsampled.
    fn target_dpi(self) -> f64 {
        match self {
            ImageQualityPreset::High => 300.0,
            ImageQualityPreset::Balanced => 200.0,
            ImageQualityPreset::Small => 150.0,
        }
    }

    /// Baseline JPEG quality factor (1-100) used for re-encoding.
    fn jpeg_quality(self) -> u8 {
        match self {
            ImageQualityPreset::High => 90,
            ImageQualityPreset::Balanced => 75,
            ImageQualityPreset::Small => 50,
        }
    }
}

/// Outcome of an [`optimize_images_in_place`] pass, broken down so the caller (and
/// ultimately the user, via the Optimize banner) can see what actually happened instead
/// of a single opaque byte count.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ImageOptimizeStats {
    /// Every image XObject found in the document (recompressed, downsampled, or passed
    /// through — the sum of all three below plus any zero-byte/degenerate images).
    pub images_total: u32,
    /// Images whose stream was actually replaced with a smaller re-encoded version.
    pub images_recompressed: u32,
    /// Subset of `images_recompressed` where the pixel dimensions were also reduced
    /// (as opposed to a quality-only re-encode at the same resolution).
    pub images_downsampled: u32,
    /// Images left byte-for-byte untouched, for any reason (unsupported class, or a
    /// recompression attempt that didn't come out smaller).
    pub images_passed_through: u32,
    /// Sum of original stream bytes for every image that was actually recompressed.
    pub image_bytes_before: u64,
    /// Sum of new stream bytes for every image that was actually recompressed.
    pub image_bytes_after: u64,
    /// Why each passed-through image was skipped, keyed by reason, for diagnostics and
    /// the measurement table (e.g. `"colorspace-unsupported" -> 92`).
    pub skip_reasons: BTreeMap<String, u32>,
}

impl ImageOptimizeStats {
    fn record_skip(&mut self, reason: &'static str) {
        self.images_passed_through += 1;
        *self.skip_reasons.entry(reason.to_string()).or_insert(0) += 1;
    }
}

/// Recompress every eligible image XObject in `doc` per `preset`. See the module doc for
/// the full eligibility/safety bar. Never removes or reorders objects — only rewrites the
/// content bytes (and `/Filter`, `/Width`, `/Height`, `/BitsPerComponent`, `/ColorSpace`)
/// of image streams it chooses to touch.
pub fn optimize_images_in_place(
    doc: &mut Document,
    preset: ImageQualityPreset,
) -> Result<ImageOptimizeStats> {
    let mut stats = ImageOptimizeStats::default();

    let placements = collect_image_placements(doc);

    // Every image object in the document, and — separately — every object referenced as
    // some other image's /SMask (processed alongside its owner, not as a top-level
    // candidate, so it isn't visited twice).
    let mut smask_of: HashMap<ObjectId, ObjectId> = HashMap::new(); // smask_id -> owner_id
    let mut all_image_ids: Vec<ObjectId> = Vec::new();
    for (id, obj) in &doc.objects {
        if let Object::Stream(s) = obj {
            if is_image_stream(&s.dict) {
                all_image_ids.push(*id);
                if let Ok(Object::Reference(smask_ref)) = s.dict.get(b"SMask") {
                    smask_of.insert(*smask_ref, *id);
                }
            }
        }
    }
    let smask_ids: HashSet<ObjectId> = smask_of.keys().copied().collect();
    let top_level: Vec<ObjectId> = all_image_ids
        .iter()
        .copied()
        .filter(|id| !smask_ids.contains(id))
        .collect();

    stats.images_total = all_image_ids.len() as u32;

    for id in top_level {
        let placement = placements.get(&id).copied();
        process_one_image(doc, id, None, placement, preset, &mut stats);

        // Recompress the SMask (if any) at the same physical placement as its owner —
        // read fresh since the base image's dict may have just been rewritten above
        // (SMask reference itself is untouched by that rewrite).
        let smask_id = match doc.get_object(id).ok().and_then(|o| o.as_stream().ok()) {
            Some(s) => match s.dict.get(b"SMask") {
                Ok(Object::Reference(r)) => Some(*r),
                _ => None,
            },
            None => None,
        };
        if let Some(smask_id) = smask_id {
            process_one_image(
                doc,
                smask_id,
                Some(ColorClass::Gray), // SMask is always DeviceGray per spec.
                placement,
                preset,
                &mut stats,
            );
        }
    }

    Ok(stats)
}

// ---------------------------------------------------------------------------
// Eligibility + per-image recompression
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ColorClass {
    Gray,
    Rgb,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FilterKind {
    /// No filter, or a single `FlateDecode` — raw 8-bit samples once decoded.
    Raw,
    /// A single `DCTDecode` — baseline JPEG bytes as stored.
    Jpeg,
    /// `[FlateDecode DCTDecode]` — some writers wrap an already-JPEG stream in an outer
    /// (largely pointless) Deflate layer. Inflate first, then decode as JPEG.
    JpegDoubleWrapped,
}

fn is_image_stream(dict: &Dictionary) -> bool {
    matches!(dict.get(b"Subtype"), Ok(Object::Name(n)) if n == b"Image")
}

fn classify_filter(dict: &Dictionary) -> Option<FilterKind> {
    match dict.get(b"Filter") {
        Err(_) => Some(FilterKind::Raw),
        Ok(Object::Name(n)) => match n.as_slice() {
            b"FlateDecode" => Some(FilterKind::Raw),
            b"DCTDecode" => Some(FilterKind::Jpeg),
            _ => None,
        },
        Ok(Object::Array(arr)) => {
            let names: Vec<&[u8]> = arr.iter().filter_map(|o| o.as_name().ok()).collect();
            match names.as_slice() {
                [b"FlateDecode"] => Some(FilterKind::Raw),
                [b"DCTDecode"] => Some(FilterKind::Jpeg),
                [b"FlateDecode", b"DCTDecode"] => Some(FilterKind::JpegDoubleWrapped),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Resolve a `/ColorSpace` value to a supported [`ColorClass`], or `None` for anything
/// outside the v1 safety bar (Indexed, DeviceCMYK, Separation, DeviceN, Lab, ...).
fn color_class_of_object(doc: &Document, obj: &Object) -> Option<ColorClass> {
    match obj {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"CalGray" | b"G" => Some(ColorClass::Gray),
            b"DeviceRGB" | b"CalRGB" | b"RGB" => Some(ColorClass::Rgb),
            _ => None,
        },
        Object::Reference(r) => doc
            .get_object(*r)
            .ok()
            .and_then(|o| color_class_of_object(doc, o)),
        Object::Array(arr) => {
            let head = arr.first()?.as_name().ok()?;
            match head {
                b"CalGray" => Some(ColorClass::Gray),
                b"CalRGB" => Some(ColorClass::Rgb),
                b"ICCBased" => {
                    let stream = match arr.get(1)? {
                        Object::Reference(r) => doc.get_object(*r).ok()?.as_stream().ok()?,
                        Object::Stream(s) => s,
                        _ => return None,
                    };
                    match stream.dict.get(b"N").ok().and_then(|o| o.as_i64().ok()) {
                        Some(1) => Some(ColorClass::Gray),
                        Some(3) => Some(ColorClass::Rgb),
                        _ => None,
                    }
                }
                // Indexed, Separation, DeviceN, Lab, Pattern, DeviceCMYK-in-array-form:
                // all outside the v1 safety bar.
                _ => None,
            }
        }
        _ => None,
    }
}

fn color_class_of(doc: &Document, dict: &Dictionary) -> Option<ColorClass> {
    let cs = dict.get(b"ColorSpace").ok()?;
    color_class_of_object(doc, cs)
}

fn inflate(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::ZlibDecoder::new(bytes);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).ok()?;
    Some(out)
}

/// Decode an image stream's pixel data into a flat 8-bit buffer (1 byte/component,
/// row-major, no padding) matching `color_class`'s component count. Returns `None` for
/// anything that doesn't decode cleanly or whose decoded shape doesn't match expectations
/// — the caller treats that as "leave untouched", never as license to guess.
fn decode_pixels(
    stream: &Stream,
    color_class: ColorClass,
    width: u32,
    height: u32,
    filter_kind: FilterKind,
) -> Option<Vec<u8>> {
    match filter_kind {
        FilterKind::Jpeg | FilterKind::JpegDoubleWrapped => {
            let jpeg_bytes = match filter_kind {
                FilterKind::Jpeg => stream.content.clone(),
                FilterKind::JpegDoubleWrapped => inflate(&stream.content)?,
                FilterKind::Raw => unreachable!(),
            };
            let dynimg =
                image::load_from_memory_with_format(&jpeg_bytes, image::ImageFormat::Jpeg).ok()?;
            if dynimg.width() != width || dynimg.height() != height {
                return None; // dict/pixel-data disagreement — don't guess, skip.
            }
            let want_channels: u8 = match color_class {
                ColorClass::Gray => 1,
                ColorClass::Rgb => 3,
            };
            if dynimg.color().channel_count() != want_channels {
                return None; // e.g. a JPEG that decoded with alpha or as CMYK-derived RGB.
            }
            Some(match color_class {
                ColorClass::Gray => dynimg.into_luma8().into_raw(),
                ColorClass::Rgb => dynimg.into_rgb8().into_raw(),
            })
        }
        FilterKind::Raw => {
            let bytes = stream.get_plain_content().ok()?;
            let components: usize = match color_class {
                ColorClass::Gray => 1,
                ColorClass::Rgb => 3,
            };
            let expected = (width as usize)
                .checked_mul(height as usize)?
                .checked_mul(components)?;
            if bytes.len() < expected {
                return None;
            }
            Some(bytes[..expected].to_vec())
        }
    }
}

/// Encode a flat 8-bit pixel buffer as baseline JPEG at `quality`.
fn encode_jpeg(
    pixels: &[u8],
    width: u32,
    height: u32,
    color_class: ColorClass,
    quality: u8,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let color_type = match color_class {
        ColorClass::Gray => image::ExtendedColorType::L8,
        ColorClass::Rgb => image::ExtendedColorType::Rgb8,
    };
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder.encode(pixels, width, height, color_type).ok()?;
    Some(out)
}

fn resize_pixels(
    pixels: &[u8],
    width: u32,
    height: u32,
    new_width: u32,
    new_height: u32,
    color_class: ColorClass,
) -> Option<Vec<u8>> {
    match color_class {
        ColorClass::Gray => {
            let buf: image::GrayImage =
                image::ImageBuffer::from_raw(width, height, pixels.to_vec())?;
            let resized = image::imageops::resize(
                &buf,
                new_width,
                new_height,
                image::imageops::FilterType::Lanczos3,
            );
            Some(resized.into_raw())
        }
        ColorClass::Rgb => {
            let buf: image::RgbImage =
                image::ImageBuffer::from_raw(width, height, pixels.to_vec())?;
            let resized = image::imageops::resize(
                &buf,
                new_width,
                new_height,
                image::imageops::FilterType::Lanczos3,
            );
            Some(resized.into_raw())
        }
    }
}

/// Attempt to recompress one image object. `color_override` forces the color class
/// (used for `/SMask`, which is always `DeviceGray` regardless of its own `/ColorSpace`
/// key, per spec) instead of resolving it from the dict. `placement_pts` is the
/// `(width, height)` in PDF user-space points of the image's largest known placement, or
/// `None` if the content-stream walk never found one (quality-only, no resize).
fn process_one_image(
    doc: &mut Document,
    id: ObjectId,
    color_override: Option<ColorClass>,
    placement_pts: Option<(f64, f64)>,
    preset: ImageQualityPreset,
    stats: &mut ImageOptimizeStats,
) {
    // ---- Read phase: gather everything needed, or bail with a skip reason. ----
    struct Plan {
        width: u32,
        height: u32,
        color_class: ColorClass,
        filter_kind: FilterKind,
        orig_len: u64,
    }

    let plan: Result<Plan, &'static str> = (|| {
        let obj = doc.get_object(id).map_err(|_| "missing-object")?;
        let stream = obj.as_stream().map_err(|_| "not-a-stream")?;

        if matches!(stream.dict.get(b"ImageMask"), Ok(Object::Boolean(true))) {
            return Err("image-mask");
        }
        if stream.dict.get(b"Decode").is_ok() {
            return Err("custom-decode");
        }
        if stream.dict.get(b"Mask").is_ok() {
            return Err("explicit-mask");
        }

        let width = stream.dict.get(b"Width").ok().and_then(|o| o.as_i64().ok());
        let height = stream
            .dict
            .get(b"Height")
            .ok()
            .and_then(|o| o.as_i64().ok());
        let (width, height) = match (width, height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w as u32, h as u32),
            _ => return Err("missing-dimensions"),
        };

        let filter_kind = classify_filter(&stream.dict).ok_or("filter-unsupported")?;

        let bpc = stream
            .dict
            .get(b"BitsPerComponent")
            .ok()
            .and_then(|o| o.as_i64().ok());
        if matches!(filter_kind, FilterKind::Raw) && bpc != Some(8) {
            return Err("bpc-unsupported");
        }

        let color_class = match color_override {
            Some(c) => c,
            None => color_class_of(doc, &stream.dict).ok_or("colorspace-unsupported")?,
        };

        Ok(Plan {
            width,
            height,
            color_class,
            filter_kind,
            orig_len: stream.content.len() as u64,
        })
    })();

    let plan = match plan {
        Ok(p) => p,
        Err(reason) => {
            stats.record_skip(reason);
            return;
        }
    };

    // ---- Decode. ----
    let pixels = {
        let stream = match doc.get_object(id).ok().and_then(|o| o.as_stream().ok()) {
            Some(s) => s,
            None => {
                stats.record_skip("missing-object");
                return;
            }
        };
        decode_pixels(
            stream,
            plan.color_class,
            plan.width,
            plan.height,
            plan.filter_kind,
        )
    };
    let Some(pixels) = pixels else {
        stats.record_skip("decode-failed");
        return;
    };

    // ---- Decide target dimensions (never upsample; unknown placement = no resize). ----
    let target_dpi = preset.target_dpi();
    let (new_w, new_h) = match placement_pts {
        Some((wp, hp)) if wp > 0.0 && hp > 0.0 => {
            let want_w = ((target_dpi * wp / 72.0).round() as u32)
                .max(8)
                .min(plan.width);
            let want_h = ((target_dpi * hp / 72.0).round() as u32)
                .max(8)
                .min(plan.height);
            (want_w, want_h)
        }
        _ => (plan.width, plan.height),
    };
    let downsampled = new_w != plan.width || new_h != plan.height;

    // ---- Resize (if needed) + re-encode. ----
    let final_pixels = if downsampled {
        match resize_pixels(
            &pixels,
            plan.width,
            plan.height,
            new_w,
            new_h,
            plan.color_class,
        ) {
            Some(p) => p,
            None => {
                stats.record_skip("resize-failed");
                return;
            }
        }
    } else {
        pixels
    };

    let Some(jpeg_bytes) = encode_jpeg(
        &final_pixels,
        new_w,
        new_h,
        plan.color_class,
        preset.jpeg_quality(),
    ) else {
        stats.record_skip("encode-failed");
        return;
    };

    // Never regress: keep the original unless the new encoding is strictly smaller.
    if (jpeg_bytes.len() as u64) >= plan.orig_len {
        stats.record_skip("not-smaller");
        return;
    }

    // ---- Write phase. ----
    let Ok(Object::Stream(stream)) = doc.get_object_mut(id) else {
        stats.record_skip("missing-object");
        return;
    };
    let new_len = jpeg_bytes.len() as u64;
    stream.dict.remove(b"DecodeParms");
    stream.dict.remove(b"Decode");
    stream.content = jpeg_bytes;
    stream
        .dict
        .set("Filter", Object::Name(b"DCTDecode".to_vec()));
    stream.dict.set("Width", new_w as i64);
    stream.dict.set("Height", new_h as i64);
    stream.dict.set("BitsPerComponent", 8_i64);
    stream.dict.set(
        "ColorSpace",
        Object::Name(match plan.color_class {
            ColorClass::Gray => b"DeviceGray".to_vec(),
            ColorClass::Rgb => b"DeviceRGB".to_vec(),
        }),
    );
    stream.dict.set("Length", new_len as i64);

    stats.images_recompressed += 1;
    if downsampled {
        stats.images_downsampled += 1;
    }
    stats.image_bytes_before += plan.orig_len;
    stats.image_bytes_after += new_len;
}

// ---------------------------------------------------------------------------
// Content-stream CTM tracking (page + one level of Form XObject nesting)
// ---------------------------------------------------------------------------

/// A 2D affine transform in PDF's row-vector convention: `[x' y' 1] = [x y 1] * M`.
#[derive(Clone, Copy, Debug)]
struct Matrix {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Matrix {
    fn identity() -> Self {
        Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    /// The matrix equivalent to applying `self` first, then `after` (i.e. `self * after`
    /// under row-vector composition) — used both for `cm` (new local matrix applied
    /// before the existing CTM) and for composing a Form XObject's own `/Matrix` before
    /// the CTM in effect at its `Do`.
    fn then(&self, after: Matrix) -> Matrix {
        Matrix {
            a: self.a * after.a + self.b * after.c,
            b: self.a * after.b + self.b * after.d,
            c: self.c * after.a + self.d * after.c,
            d: self.c * after.b + self.d * after.d,
            e: self.e * after.a + self.f * after.c + after.e,
            f: self.e * after.b + self.f * after.d + after.f,
        }
    }

    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            x * self.a + y * self.c + self.e,
            x * self.b + y * self.d + self.f,
        )
    }
}

fn resolve_dict(doc: &Document, obj: &Object) -> Option<Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d.clone()),
        Object::Reference(r) => doc.get_dictionary(*r).ok().cloned(),
        _ => None,
    }
}

fn page_resources(doc: &Document, page_id: ObjectId) -> Dictionary {
    doc.get_dictionary(page_id)
        .ok()
        .and_then(|p| p.get(b"Resources").ok())
        .and_then(|o| resolve_dict(doc, o))
        .unwrap_or_default()
}

fn xobject_dict_of(doc: &Document, resources: &Dictionary) -> Option<Dictionary> {
    resources
        .get(b"XObject")
        .ok()
        .and_then(|o| resolve_dict(doc, o))
}

fn form_matrix_of(dict: &Dictionary) -> Matrix {
    match dict.get(b"Matrix") {
        Ok(Object::Array(arr)) if arr.len() == 6 => {
            let v: Vec<f64> = arr
                .iter()
                .map(|o| o.as_float().unwrap_or(0.0) as f64)
                .collect();
            Matrix {
                a: v[0],
                b: v[1],
                c: v[2],
                d: v[3],
                e: v[4],
                f: v[5],
            }
        }
        _ => Matrix::identity(),
    }
}

/// Record the transformed-unit-square bounding box of a `Do`'d image under `ctm`,
/// keeping the largest width/height seen across every placement of this object id.
fn record_placement(placements: &mut HashMap<ObjectId, (f64, f64)>, id: ObjectId, ctm: &Matrix) {
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
    let pts: Vec<(f64, f64)> = corners.iter().map(|&(x, y)| ctm.apply(x, y)).collect();
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in pts {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let w = (max_x - min_x).abs();
    let h = (max_y - min_y).abs();
    let entry = placements.entry(id).or_insert((0.0, 0.0));
    if w > entry.0 {
        entry.0 = w;
    }
    if h > entry.1 {
        entry.1 = h;
    }
}

/// Walk a decoded content stream's operations, tracking the graphics-state matrix stack
/// (`q`/`Q`/`cm`) and recursing one level into Form XObjects invoked via `Do`, recording
/// every Image XObject placement it finds. Depth-limited defensively against pathological
/// or cyclic XObject graphs (real PDFs never need more than one level for this purpose —
/// background scans and stamps are placed directly in page or single-Form content).
fn walk_content(
    doc: &Document,
    ops: &[Operation],
    resources: &Dictionary,
    base_ctm: Matrix,
    placements: &mut HashMap<ObjectId, (f64, f64)>,
    depth: u8,
) {
    if depth > 6 {
        return;
    }
    let xobj_dict = xobject_dict_of(doc, resources);
    let mut stack: Vec<Matrix> = Vec::new();
    let mut ctm = base_ctm;

    for op in ops {
        match op.operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => {
                if let Some(m) = stack.pop() {
                    ctm = m;
                }
            }
            "cm" if op.operands.len() == 6 => {
                let v: Vec<f64> = op
                    .operands
                    .iter()
                    .map(|o| o.as_float().unwrap_or(0.0) as f64)
                    .collect();
                let m = Matrix {
                    a: v[0],
                    b: v[1],
                    c: v[2],
                    d: v[3],
                    e: v[4],
                    f: v[5],
                };
                ctm = m.then(ctm);
            }
            "Do" => {
                let Some(Object::Name(name)) = op.operands.first() else {
                    continue;
                };
                let Some(xd) = &xobj_dict else { continue };
                let Ok(Object::Reference(obj_id)) = xd.get(name) else {
                    continue;
                };
                let obj_id = *obj_id;
                let Ok(Object::Stream(s)) = doc.get_object(obj_id) else {
                    continue;
                };
                match s.dict.get(b"Subtype") {
                    Ok(Object::Name(n)) if n == b"Image" => {
                        record_placement(placements, obj_id, &ctm);
                    }
                    Ok(Object::Name(n)) if n == b"Form" => {
                        let form_ctm = form_matrix_of(&s.dict).then(ctm);
                        let form_resources = s
                            .dict
                            .get(b"Resources")
                            .ok()
                            .and_then(|o| resolve_dict(doc, o))
                            .unwrap_or_else(|| resources.clone());
                        if let Ok(bytes) = s.get_plain_content() {
                            if let Ok(form_content) = Content::decode(&bytes) {
                                walk_content(
                                    doc,
                                    &form_content.operations,
                                    &form_resources,
                                    form_ctm,
                                    placements,
                                    depth + 1,
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Walk every page's content stream to find the largest physical placement (PDF
/// user-space points) of every image XObject reachable from page content.
fn collect_image_placements(doc: &Document) -> HashMap<ObjectId, (f64, f64)> {
    let mut placements = HashMap::new();
    let page_ids: Vec<ObjectId> = doc.get_pages().values().cloned().collect();
    for page_id in page_ids {
        let resources = page_resources(doc, page_id);
        let Ok(content) = doc.get_and_decode_page_content(page_id) else {
            continue;
        };
        walk_content(
            doc,
            &content.operations,
            &resources,
            Matrix::identity(),
            &mut placements,
            0,
        );
    }
    placements
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Object};

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Deterministic pseudo-random byte, high enough entropy that JPEG quality settings
    /// produce a reliably monotonic size ordering (a smooth gradient compresses to
    /// near-nothing at almost any quality, which makes quality-delta assertions flaky).
    fn noise_byte(seed: u32) -> u8 {
        let mut v = seed.wrapping_mul(2_654_435_761);
        v ^= v >> 15;
        v = v.wrapping_mul(2_246_822_519);
        (v >> 24) as u8
    }

    /// Build a `width`x`height` high-entropy RGB image as raw 8-bit samples.
    fn synthetic_rgb(width: u32, height: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 3;
                buf.push(noise_byte(idx));
                buf.push(noise_byte(idx + 1));
                buf.push(noise_byte(idx + 2));
            }
        }
        buf
    }

    fn synthetic_gray(width: u32, height: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                buf.push(noise_byte(y * width + x));
            }
        }
        buf
    }

    /// Build a single-page document with one Image XObject placed to fill the page,
    /// with the given dict overrides layered on top of a plausible-default raw-RGB image.
    struct DocBuilder {
        doc: Document,
        page_id: ObjectId,
        pages_id: ObjectId,
    }

    impl DocBuilder {
        fn new(page_w: f64, page_h: f64) -> Self {
            let mut doc = Document::with_version("1.7");
            let pages_id = doc.new_object_id();
            let content_id = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
            let page_id = doc.add_object(Object::Dictionary(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![Object::Real(0.0), Object::Real(0.0), Object::Real(page_w as f32), Object::Real(page_h as f32)],
                "Contents" => content_id,
                "Resources" => Object::Dictionary(dictionary! {}),
            }));
            DocBuilder {
                doc,
                page_id,
                pages_id,
            }
        }

        fn finish(mut self) -> (Document, ObjectId) {
            self.doc.objects.insert(
                self.pages_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages",
                    "Kids" => vec![Object::Reference(self.page_id)],
                    "Count" => 1_i64,
                }),
            );
            let catalog_id = self.doc.add_object(Object::Dictionary(dictionary! {
                "Type" => "Catalog",
                "Pages" => self.pages_id,
            }));
            self.doc.trailer.set("Root", catalog_id);
            (self.doc, self.page_id)
        }

        /// Add an image XObject with the given dict entries + content, and a page
        /// content stream that places it filling the page's own MediaBox exactly
        /// (`cm` = MediaBox width/height, no translation).
        fn place_image(
            &mut self,
            mut dict: Dictionary,
            content: Vec<u8>,
            page_w: f64,
            page_h: f64,
        ) -> ObjectId {
            dict.set("Type", "XObject");
            dict.set("Subtype", "Image");
            let img_id = self.doc.add_object(Stream::new(dict, content));

            let xobj_name = "Im0";
            let xobj_dict = dictionary! { xobj_name => img_id };
            let res_dict = dictionary! { "XObject" => Object::Dictionary(xobj_dict) };
            {
                let page = self.doc.get_dictionary_mut(self.page_id).unwrap();
                page.set("Resources", Object::Dictionary(res_dict));
            }

            let cm = format!("q {page_w} 0 0 {page_h} 0 0 cm /{xobj_name} Do Q");
            let content_id = self
                .doc
                .add_object(Stream::new(dictionary! {}, cm.into_bytes()));
            {
                let page = self.doc.get_dictionary_mut(self.page_id).unwrap();
                page.set("Contents", Object::Reference(content_id));
            }
            img_id
        }
    }

    fn raw_rgb_dict(width: u32, height: u32) -> Dictionary {
        dictionary! {
            "Width" => width as i64,
            "Height" => height as i64,
            "BitsPerComponent" => 8_i64,
            "ColorSpace" => "DeviceRGB",
        }
    }

    fn raw_gray_dict(width: u32, height: u32) -> Dictionary {
        dictionary! {
            "Width" => width as i64,
            "Height" => height as i64,
            "BitsPerComponent" => 8_i64,
            "ColorSpace" => "DeviceGray",
        }
    }

    // -----------------------------------------------------------------------
    // Matrix math
    // -----------------------------------------------------------------------

    #[test]
    fn matrix_identity_then_identity_is_identity() {
        let m = Matrix::identity().then(Matrix::identity());
        let (x, y) = m.apply(3.0, 4.0);
        assert_eq!((x, y), (3.0, 4.0));
    }

    #[test]
    fn matrix_scale_maps_unit_square_to_expected_bbox() {
        // cm 612 0 0 792 0 0 -> unit square maps to [0,612]x[0,792].
        let m = Matrix {
            a: 612.0,
            b: 0.0,
            c: 0.0,
            d: 792.0,
            e: 0.0,
            f: 0.0,
        };
        let (x0, y0) = m.apply(0.0, 0.0);
        let (x1, y1) = m.apply(1.0, 1.0);
        assert_eq!((x0, y0), (0.0, 0.0));
        assert_eq!((x1, y1), (612.0, 792.0));
    }

    #[test]
    fn matrix_translation_composes_after_scale() {
        // Scale by 100 in both axes, then translate by (50, 25): point (1,1) -> (150, 125).
        let scale = Matrix {
            a: 100.0,
            b: 0.0,
            c: 0.0,
            d: 100.0,
            e: 0.0,
            f: 0.0,
        };
        let translate = Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 50.0,
            f: 25.0,
        };
        let combined = scale.then(translate);
        let (x, y) = combined.apply(1.0, 1.0);
        assert_eq!((x, y), (150.0, 125.0));
    }

    // -----------------------------------------------------------------------
    // Placement detection
    // -----------------------------------------------------------------------

    #[test]
    fn collect_image_placements_finds_full_page_image() {
        let mut b = DocBuilder::new(612.0, 792.0);
        let img_id = b.place_image(
            raw_rgb_dict(100, 100),
            synthetic_rgb(100, 100),
            612.0,
            792.0,
        );
        let (doc, _page_id) = b.finish();

        let placements = collect_image_placements(&doc);
        let (w, h) = placements
            .get(&img_id)
            .copied()
            .expect("placement recorded");
        assert!(
            (w - 612.0).abs() < 1e-6,
            "width should match page MediaBox: got {w}"
        );
        assert!(
            (h - 792.0).abs() < 1e-6,
            "height should match page MediaBox: got {h}"
        );
    }

    #[test]
    fn collect_image_placements_keeps_largest_of_multiple_placements() {
        let mut b = DocBuilder::new(1000.0, 1000.0);
        let img_id = b.place_image(raw_rgb_dict(50, 50), synthetic_rgb(50, 50), 200.0, 200.0);
        // Add a second, larger placement of the SAME image object on the page.
        let second_cm = b"q 400 0 0 400 0 0 cm /Im0 Do Q";
        {
            let existing = b.doc.get_and_decode_page_content(b.page_id).unwrap();
            let mut ops = existing.operations;
            ops.extend(Content::decode(second_cm).unwrap().operations);
            let encoded = Content { operations: ops }.encode().unwrap();
            let content_id = b.doc.add_object(Stream::new(dictionary! {}, encoded));
            b.doc
                .get_dictionary_mut(b.page_id)
                .unwrap()
                .set("Contents", Object::Reference(content_id));
        }
        let (doc, _) = b.finish();

        let placements = collect_image_placements(&doc);
        let (w, h) = placements.get(&img_id).copied().unwrap();
        assert!(
            (w - 400.0).abs() < 1e-6,
            "must keep the larger of the two placements: got {w}"
        );
        assert!((h - 400.0).abs() < 1e-6);
    }

    #[test]
    fn collect_image_placements_recurses_one_level_into_form_xobject() {
        // Page places a Form XObject (with its own /Matrix scaling by 2x), and the
        // Form's own content places the image filling its own unit square.
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();

        let img_dict = {
            let mut d = raw_rgb_dict(10, 10);
            d.set("Type", "XObject");
            d.set("Subtype", "Image");
            d
        };
        let img_id = doc.add_object(Stream::new(img_dict, synthetic_rgb(10, 10)));

        let form_res =
            dictionary! { "XObject" => Object::Dictionary(dictionary! { "Im0" => img_id }) };
        let form_dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(1), Object::Integer(1)],
            "Matrix" => vec![Object::Real(2.0), Object::Real(0.0), Object::Real(0.0), Object::Real(2.0), Object::Real(0.0), Object::Real(0.0)],
            "Resources" => Object::Dictionary(form_res),
        };
        let form_content = b"q 1 0 0 1 0 0 cm /Im0 Do Q".to_vec();
        let form_id = doc.add_object(Stream::new(form_dict, form_content));

        let page_res =
            dictionary! { "XObject" => Object::Dictionary(dictionary! { "Fm0" => form_id }) };
        // Page places the form scaled by 100 (so total placement = 2x form-matrix * 100 page-cm = 200).
        let page_content = doc.add_object(Stream::new(
            dictionary! {},
            b"q 100 0 0 100 0 0 cm /Fm0 Do Q".to_vec(),
        ));
        let page_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(1000), Object::Integer(1000)],
            "Contents" => page_content,
            "Resources" => Object::Dictionary(page_res),
        }));
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![Object::Reference(page_id)], "Count" => 1_i64,
            }),
        );
        let catalog_id = doc.add_object(Object::Dictionary(
            dictionary! { "Type" => "Catalog", "Pages" => pages_id },
        ));
        doc.trailer.set("Root", catalog_id);

        let placements = collect_image_placements(&doc);
        let (w, h) = placements
            .get(&img_id)
            .copied()
            .expect("placement found through nested Form");
        assert!(
            (w - 200.0).abs() < 1e-6,
            "form-matrix (2x) composed with page cm (100x) = 200: got {w}"
        );
        assert!((h - 200.0).abs() < 1e-6);
    }

    #[test]
    fn collect_image_placements_is_silent_for_image_never_dod() {
        // An image XObject that exists but is never referenced by a `Do` in any
        // walked content stream simply has no entry — not a panic, not a zero-size bbox.
        let mut b = DocBuilder::new(612.0, 792.0);
        let dict = {
            let mut d = raw_rgb_dict(10, 10);
            d.set("Type", "XObject");
            d.set("Subtype", "Image");
            d
        };
        let orphan_id = b.doc.add_object(Stream::new(dict, synthetic_rgb(10, 10)));
        let (doc, _) = b.finish();

        let placements = collect_image_placements(&doc);
        assert!(!placements.contains_key(&orphan_id));
    }

    // -----------------------------------------------------------------------
    // Eligibility / safety bar
    // -----------------------------------------------------------------------

    #[test]
    fn skips_image_mask() {
        let mut b = DocBuilder::new(100.0, 100.0);
        let mut dict = raw_gray_dict(10, 10);
        dict.set("ImageMask", true);
        dict.remove(b"ColorSpace"); // ImageMask images carry no /ColorSpace per spec.
        b.place_image(dict, vec![0u8; 13], 100.0, 100.0); // 10x10 1-bit padded to 2 bytes/row is irrelevant here.
        let (mut doc, _) = b.finish();

        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Balanced).unwrap();
        assert_eq!(stats.images_recompressed, 0);
        assert_eq!(stats.images_passed_through, 1);
        assert_eq!(stats.skip_reasons.get("image-mask"), Some(&1));
    }

    #[test]
    fn skips_indexed_colorspace() {
        let mut b = DocBuilder::new(100.0, 100.0);
        let dict = dictionary! {
            "Width" => 10_i64,
            "Height" => 10_i64,
            "BitsPerComponent" => 8_i64,
            "ColorSpace" => vec![Object::Name(b"Indexed".to_vec()), Object::Name(b"DeviceRGB".to_vec()), Object::Integer(1), Object::string_literal(vec![0,0,0,255,255,255])],
        };
        b.place_image(dict, vec![0u8; 100], 100.0, 100.0);
        let (mut doc, _) = b.finish();

        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Balanced).unwrap();
        assert_eq!(stats.images_recompressed, 0);
        assert_eq!(stats.skip_reasons.get("colorspace-unsupported"), Some(&1));
    }

    #[test]
    fn skips_ccitt_and_jpx_and_jbig2() {
        for filter in ["CCITTFaxDecode", "JPXDecode", "JBIG2Decode"] {
            let mut b = DocBuilder::new(100.0, 100.0);
            let mut dict = raw_gray_dict(10, 10);
            dict.set("Filter", Object::Name(filter.as_bytes().to_vec()));
            b.place_image(dict, vec![0u8; 20], 100.0, 100.0);
            let (mut doc, _) = b.finish();

            let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Balanced).unwrap();
            assert_eq!(
                stats.images_recompressed, 0,
                "{filter} must not be recompressed"
            );
            assert_eq!(
                stats.skip_reasons.get("filter-unsupported"),
                Some(&1),
                "{filter}"
            );
        }
    }

    #[test]
    fn skips_explicit_mask_and_custom_decode() {
        let mut b = DocBuilder::new(100.0, 100.0);
        let mut dict = raw_rgb_dict(10, 10);
        dict.set(
            "Mask",
            Object::Array(vec![Object::Integer(0), Object::Integer(0)]),
        );
        b.place_image(dict, synthetic_rgb(10, 10), 100.0, 100.0);
        let (mut doc, _) = b.finish();
        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Balanced).unwrap();
        assert_eq!(stats.skip_reasons.get("explicit-mask"), Some(&1));

        let mut b2 = DocBuilder::new(100.0, 100.0);
        let mut dict2 = raw_rgb_dict(10, 10);
        dict2.set(
            "Decode",
            Object::Array(vec![Object::Real(1.0), Object::Real(0.0)]),
        );
        b2.place_image(dict2, synthetic_rgb(10, 10), 100.0, 100.0);
        let (mut doc2, _) = b2.finish();
        let stats2 = optimize_images_in_place(&mut doc2, ImageQualityPreset::Balanced).unwrap();
        assert_eq!(stats2.skip_reasons.get("custom-decode"), Some(&1));
    }

    #[test]
    fn skips_non_8bit_raw_samples() {
        let mut b = DocBuilder::new(100.0, 100.0);
        let mut dict = raw_gray_dict(10, 10);
        dict.set("BitsPerComponent", 1_i64);
        b.place_image(dict, vec![0u8; 20], 100.0, 100.0);
        let (mut doc, _) = b.finish();
        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Balanced).unwrap();
        assert_eq!(stats.skip_reasons.get("bpc-unsupported"), Some(&1));
    }

    // -----------------------------------------------------------------------
    // Real recompression
    // -----------------------------------------------------------------------

    #[test]
    fn recompresses_large_raw_rgb_image_placed_full_page() {
        // An 800x800 raw-RGB image placed on a tiny 50x50pt page (0.694x0.694in) has an
        // effective resolution of ~1153 DPI - far above Balanced's 200 DPI target - so
        // this must trigger both downsampling and a drastic size reduction.
        let width = 800u32;
        let height = 800u32;
        let mut b = DocBuilder::new(50.0, 50.0);
        let raw = synthetic_rgb(width, height);
        let orig_len = raw.len() as u64;
        b.place_image(raw_rgb_dict(width, height), raw, 50.0, 50.0);
        let (mut doc, _) = b.finish();

        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Balanced).unwrap();
        assert_eq!(stats.images_total, 1);
        assert_eq!(
            stats.images_recompressed, 1,
            "skip_reasons: {:?}",
            stats.skip_reasons
        );
        assert_eq!(stats.images_downsampled, 1);
        assert!(
            stats.image_bytes_after < stats.image_bytes_before / 4,
            "expected a large reduction: before={} after={}",
            stats.image_bytes_before,
            stats.image_bytes_after
        );
        assert_eq!(stats.image_bytes_before, orig_len);
    }

    #[test]
    fn never_upsamples_beyond_original_pixel_dimensions() {
        // A tiny 20x20 image placed to fill a huge page: target DPI math would ask for
        // MORE pixels than the source has — must cap at the original size, not upsample.
        let mut b = DocBuilder::new(10000.0, 10000.0);
        b.place_image(
            raw_rgb_dict(20, 20),
            synthetic_rgb(20, 20),
            10000.0,
            10000.0,
        );
        let (mut doc, _) = b.finish();

        // Sanity via internal helper: recompute what the pipeline would target.
        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::High).unwrap();
        // Either it recompressed at <=20x20 (same-or-smaller, never larger) or it
        // legitimately found the JPEG re-encode wasn't smaller (small images sometimes
        // don't shrink) — both are acceptable, but "images_downsampled" must be 0 since
        // 20x20 can never need enlarging under any DPI target we ship.
        assert_eq!(stats.images_downsampled, 0, "must never upsample");
    }

    #[test]
    fn recompresses_existing_dct_jpeg_at_lower_quality() {
        // Encode a real JPEG at quality 95 first (simulating an already-JPEG source),
        // then feed it through the pipeline at Small (quality 50) — must shrink further.
        let width = 400u32;
        let height = 400u32;
        let raw = synthetic_rgb(width, height);
        let mut hi_q = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut hi_q, 95)
            .encode(&raw, width, height, image::ExtendedColorType::Rgb8)
            .unwrap();
        let orig_len = hi_q.len() as u64;

        let mut b = DocBuilder::new(612.0, 792.0);
        let mut dict = raw_rgb_dict(width, height);
        dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        b.place_image(dict, hi_q, 612.0, 792.0);
        let (mut doc, _) = b.finish();

        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Small).unwrap();
        assert_eq!(
            stats.images_recompressed, 1,
            "skip_reasons: {:?}",
            stats.skip_reasons
        );
        assert_eq!(stats.image_bytes_before, orig_len);
        assert!(stats.image_bytes_after < orig_len);
    }

    #[test]
    fn recompresses_flate_double_wrapped_jpeg() {
        let width = 300u32;
        let height = 300u32;
        let raw = synthetic_rgb(width, height);
        let mut jpeg_bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 90)
            .encode(&raw, width, height, image::ExtendedColorType::Rgb8)
            .unwrap();

        use std::io::Write;
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&jpeg_bytes).unwrap();
        let wrapped = encoder.finish().unwrap();
        let orig_len = wrapped.len() as u64;

        let mut b = DocBuilder::new(612.0, 792.0);
        let mut dict = raw_rgb_dict(width, height);
        dict.set(
            "Filter",
            Object::Array(vec![
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"DCTDecode".to_vec()),
            ]),
        );
        b.place_image(dict, wrapped, 612.0, 792.0);
        let (mut doc, _) = b.finish();

        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Balanced).unwrap();
        assert_eq!(
            stats.images_recompressed, 1,
            "skip_reasons: {:?}",
            stats.skip_reasons
        );
        assert_eq!(stats.image_bytes_before, orig_len);

        // The rewritten stream must carry a plain single DCTDecode filter, not the
        // double-wrap, since the output is a fresh single-layer JPEG.
        let img_id = doc
            .objects
            .iter()
            .find_map(|(id, o)| match o {
                Object::Stream(s) if is_image_stream(&s.dict) => Some(*id),
                _ => None,
            })
            .unwrap();
        let stream = doc.get_object(img_id).unwrap().as_stream().unwrap();
        assert!(matches!(stream.dict.get(b"Filter"), Ok(Object::Name(n)) if n == b"DCTDecode"));
    }

    #[test]
    fn recompresses_smask_alongside_owner_at_same_placement() {
        let width = 800u32;
        let height = 800u32;
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();

        let smask_dict = {
            let mut d = raw_gray_dict(width, height);
            d.set("Type", "XObject");
            d.set("Subtype", "Image");
            d
        };
        let smask_id = doc.add_object(Stream::new(smask_dict, synthetic_gray(width, height)));

        let mut img_dict = raw_rgb_dict(width, height);
        img_dict.set("Type", "XObject");
        img_dict.set("Subtype", "Image");
        img_dict.set("SMask", Object::Reference(smask_id));
        let img_id = doc.add_object(Stream::new(img_dict, synthetic_rgb(width, height)));

        let res = dictionary! { "XObject" => Object::Dictionary(dictionary! { "Im0" => img_id }) };
        let content_id = doc.add_object(Stream::new(
            dictionary! {},
            b"q 612 0 0 792 0 0 cm /Im0 Do Q".to_vec(),
        ));
        let page_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792)],
            "Contents" => content_id,
            "Resources" => Object::Dictionary(res),
        }));
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![Object::Reference(page_id)], "Count" => 1_i64,
            }),
        );
        let catalog_id = doc.add_object(Object::Dictionary(
            dictionary! { "Type" => "Catalog", "Pages" => pages_id },
        ));
        doc.trailer.set("Root", catalog_id);

        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Balanced).unwrap();
        // Both the base image and its SMask must be counted (2 total, both recompressed)
        // and the SMask must NOT also appear as an independent top-level candidate.
        assert_eq!(
            stats.images_total, 2,
            "skip_reasons: {:?}",
            stats.skip_reasons
        );
        assert_eq!(
            stats.images_recompressed, 2,
            "skip_reasons: {:?}",
            stats.skip_reasons
        );

        let smask_stream = doc.get_object(smask_id).unwrap().as_stream().unwrap();
        assert!(
            matches!(smask_stream.dict.get(b"ColorSpace"), Ok(Object::Name(n)) if n == b"DeviceGray")
        );
    }

    #[test]
    fn quality_only_reencode_when_placement_unknown() {
        // An eligible image that the content-stream walk never found a Do for: quality
        // re-encode is still attempted (if it shrinks), but never resized.
        let width = 50u32;
        let height = 50u32;
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let mut dict = raw_rgb_dict(width, height);
        dict.set("Type", "XObject");
        dict.set("Subtype", "Image");
        let img_id = doc.add_object(Stream::new(dict, synthetic_rgb(width, height)));
        let content_id = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let page_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792)],
            "Contents" => content_id,
            "Resources" => Object::Dictionary(dictionary! {}), // image NOT referenced here
        }));
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![Object::Reference(page_id)], "Count" => 1_i64,
            }),
        );
        let catalog_id = doc.add_object(Object::Dictionary(
            dictionary! { "Type" => "Catalog", "Pages" => pages_id },
        ));
        doc.trailer.set("Root", catalog_id);
        // The image is a real object in the document (e.g. reachable only via some
        // structure our content walk doesn't cover) even though never `Do`'d.
        let _ = img_id;

        let stats = optimize_images_in_place(&mut doc, ImageQualityPreset::Balanced).unwrap();
        assert_eq!(
            stats.images_downsampled, 0,
            "unknown placement must never trigger a resize"
        );
    }
}
