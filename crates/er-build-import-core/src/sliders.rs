//! The character's appearance, as the planner's Cosmetics tab carries it.
//!
//! # Where the bytes come from, and where they go
//!
//! Elden Ring keeps a character's appearance in `PlayerGameData::face_data.face_data_buffer`, a
//! 288-byte `FaceDataBuffer`:
//!
//! ```text
//! 0x000  magic[4]     "FACE"
//! 0x004  version      u32, 4
//! 0x008  buffer_size  u32, 288
//! 0x00c  buffer[276]  the appearance itself
//! ```
//!
//! The planner models the same appearance as `character.sliders`, and its byte format is
//! **`buffer[0..264]`** -- the payload, without the header, and twelve bytes short of its end.
//! Their offset 0 is the whole buffer's offset 12. That alignment is measured rather than
//! assumed: scoring all 25 plausible base offsets on how many `list` fields land on a value their
//! own option tables admit, across 18 characters from real saves, put base 12 at 138 of 144 and
//! the next best at 98. The prefix `0A 00 00 00 69 00 00 00` that their import box shows as a
//! placeholder decodes at that base as `faceModelId = 10, hairModelId = 105`.
//!
//! The twelve bytes their format omits, `buffer[264..276]`, are zero in all 18 of those
//! characters. This module never writes them anyway: [`encode_into`] is given the character's own
//! buffer and overwrites only the range the layout covers, so the tail, the magic, the version
//! and the declared size all survive a round trip through a share link untouched.
//!
//! # The value shapes
//!
//! [`LAYOUT`] is the planner's `q9` table, generated from `data/planner-sliders-layout.json` by
//! `build.rs`. It tiles `0..264` exactly -- no gap, no overlap -- which the generator asserts.
//! Three encodings appear in it:
//!
//! | layout type | bytes | JSON |
//! |---|---|---|
//! | `number`, `boolean` | 1 | a number `0..=255` |
//! | `colour` | 3 | a three-element array of numbers |
//! | `list` | 4 | a number, little-endian |
//!
//! `boolean` is folded into the one-byte case on purpose; `build.rs` says why.
//!
//! # `faceModelId` is three keys on the way out
//!
//! The one field the planner does not carry under its own name. It splits the value across
//! `age`, `boneStructure` and `musculature`, and this module mirrors that split exactly so a
//! document written here is one its Cosmetics tab can render:
//!
//! ```text
//! musculature   = (id >= 100) as u32
//! age           = (id - 100 * musculature) % 10
//! boneStructure = (id - 100 * musculature) / 10 * 10
//! ```
//!
//! and back, `id = boneStructure + age + 100 * musculature`. The split is lossless for every
//! value: 141 becomes `(1, 40, 1)` and 500 becomes `(0, 400, 1)`, both of which recompose.
//!
//! # One bug of theirs is deliberately not reproduced
//!
//! Their `importAOB` reads a **single byte** for a `list` field (`let r = i[e]`) while their
//! `exportAOB` writes four little-endian bytes for it. A character whose `faceModelId` is 500
//! therefore survives their export and comes back from their import as 244. This module reads and
//! writes four bytes in both directions. It costs nothing in practice: our export writes the
//! decoded `sliders` object into the share payload rather than an AOB string, so their AOB parser
//! is not in the path at all, and a build of ours opened on their site is read straight off the
//! object.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// How a layout row is encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// One byte, carried as a JSON number. The planner's `number` and `boolean` both land here.
    Byte,
    /// Three bytes, carried as a JSON array of three numbers.
    Colour,
    /// Four bytes little-endian, carried as a JSON number.
    List,
}

impl FieldKind {
    /// How many bytes of the buffer this row occupies.
    pub const fn size(self) -> usize {
        match self {
            FieldKind::Byte => 1,
            FieldKind::Colour => 3,
            FieldKind::List => 4,
        }
    }
}

/// One row of the planner's layout table.
#[derive(Clone, Copy, Debug)]
pub struct SliderField {
    /// The key the planner stores it under, e.g. `cheekboneHeight`.
    pub key: &'static str,
    /// Its first byte, relative to `FaceDataBuffer.buffer`.
    pub offset: usize,
    /// How it is encoded.
    pub kind: FieldKind,
}

include!(concat!(env!("OUT_DIR"), "/sliders_layout.rs"));

/// The planner's layout table: 200 rows tiling [`SLIDER_BYTES`] with no gap and no overlap.
pub fn layout() -> &'static [SliderField] {
    &LAYOUT
}

/// How many bytes of `FaceDataBuffer.buffer` the planner's format covers.
pub const SLIDER_BYTES: usize = 264;

/// Where the planner's byte 0 sits inside the whole `FaceDataBuffer`, i.e. past
/// `magic` + `version` + `buffer_size`.
pub const FACE_BUFFER_PAYLOAD_OFFSET: usize = 12;

/// The whole `FaceDataBuffer`, header included.
pub const FACE_BUFFER_LEN: usize = 0x120;

/// The four bytes every `FaceDataBuffer` begins with.
pub const FACE_BUFFER_MAGIC: [u8; 4] = *b"FACE";

/// The only `FaceDataBuffer.version` the game's own writer accepts.
///
/// `CS::FaceData::CopyFromBuffer` begins `if (buffer->version == 4)` and returns having done
/// nothing otherwise -- no fault, no report. So a buffer that fails this check is refused here,
/// where the reason can be said, rather than passed to a native that will silently ignore it.
pub const FACE_BUFFER_VERSION: u32 = 4;

/// The key the planner splits into [`AGE_KEY`], [`BONE_STRUCTURE_KEY`] and [`MUSCULATURE_KEY`].
pub const FACE_MODEL_ID_KEY: &str = "faceModelId";
/// `faceModelId % 10`, after the hundreds digit is taken off.
pub const AGE_KEY: &str = "age";
/// `faceModelId / 10 * 10`, after the hundreds digit is taken off.
pub const BONE_STRUCTURE_KEY: &str = "boneStructure";
/// Whether `faceModelId` was at least 100.
pub const MUSCULATURE_KEY: &str = "musculature";

/// The hundreds digit of `faceModelId`, which the planner carries as [`MUSCULATURE_KEY`].
const MUSCULATURE_STEP: u32 = 100;
/// The divisor separating [`AGE_KEY`] from [`BONE_STRUCTURE_KEY`].
const AGE_MODULUS: u32 = 10;

// A `list` field is written as the full four bytes, unclamped, and that is a decision with
// evidence behind it rather than an omission.
//
// `CS::FaceData::ValidateFaceData` (1.16.2 `0x140252610`) requires the magic, `version == 4`,
// `buffer_size == 0x120`, and the eight leading `i32`s to be non-negative -- and those eight are
// exactly this layout's eight `list` rows, 8 fields of 4 bytes tiling `buffer[0..32]`, from
// `faceModelId` through `eyelashModelId`. Clamping to `i32::MAX` to satisfy that check is what
// this code did first, and the save corpus rejected it: of 62 real characters, one carries
// `faceModelId = 0x8d000228`, whose high bit is set. The clamp rewrote that character's face.
//
// Two facts settle it. The writer this crate actually calls is
// `CS::PlayerGameData::CopyFaceDataFromBuffer`, whose only gate is `version == 4` -- it never
// calls `ValidateFaceData`. And the character above lives in a save the game loads. So a value
// the game itself stores has to survive a share link; refusing to reproduce it would be this
// codec deciding it knows better than the save does.

/// A decoded appearance: the planner's `character.sliders.sliders` object.
///
/// A map of JSON values rather than two hundred named fields, and that is the shape the planner
/// itself holds -- its importer builds the object key by key from the same table. It also means a
/// row added to a future `q9` flows through both directions the moment the JSON is re-captured,
/// with no Rust to write.
pub type SliderMap = BTreeMap<String, Value>;

/// The planner's `character.sliders` -- the appearance plus the presentation around it.
///
/// Transcribed from its own `makeDefaultSliders()`:
/// `{id: "", bodyType: "A", name: "", images: [{url: "", geometry: {...}}], sliders: {}}`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlidersDoc {
    /// The cosmetics preset's own share id, empty for one that was never stored separately.
    #[serde(default)]
    pub id: String,
    /// Which body the sliders are shown on.
    ///
    /// Not derivable from the bytes: their importer hardcodes `"A"` regardless of what it parsed,
    /// so the only way a shared build shows the right body is for the writer to set it. Ours is
    /// set from `PlayerGameData::gender`.
    #[serde(default)]
    pub body_type: BodyType,
    /// The preset's display name.
    #[serde(default)]
    pub name: String,
    /// Reference images the author attached. Always at least one entry, possibly blank.
    #[serde(default)]
    pub images: Vec<SliderImage>,
    /// The sliders themselves.
    #[serde(default)]
    pub sliders: SliderMap,
}

impl SlidersDoc {
    /// A document carrying `sliders`, with `makeDefaultSliders()`'s presentation around it.
    pub fn new(body_type: BodyType, sliders: SliderMap) -> Self {
        Self {
            body_type,
            images: vec![SliderImage::default()],
            sliders,
            ..Self::default()
        }
    }
}

/// Which body the planner renders the sliders on.
///
/// Lenient in both directions on purpose: a share link is untrusted input, and an unknown spelling
/// must not fail the whole document's parse when the worst it can cost is a preview rendered on
/// the other body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(from = "String", into = "String")]
pub enum BodyType {
    /// The planner's own default.
    #[default]
    A,
    /// The other one.
    B,
}

impl From<String> for BodyType {
    fn from(value: String) -> Self {
        if value.eq_ignore_ascii_case("b") {
            BodyType::B
        } else {
            BodyType::A
        }
    }
}

impl From<BodyType> for String {
    fn from(value: BodyType) -> Self {
        match value {
            BodyType::A => "A".to_owned(),
            BodyType::B => "B".to_owned(),
        }
    }
}

/// One attached reference image.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct SliderImage {
    /// Where the image lives. Blank in the default the planner creates.
    #[serde(default)]
    pub url: String,
    /// How it is cropped in the panel.
    #[serde(default)]
    pub geometry: SliderImageGeometry,
}

/// An attached image's crop.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct SliderImageGeometry {
    /// Left edge.
    #[serde(default)]
    pub x: f64,
    /// Top edge.
    #[serde(default)]
    pub y: f64,
    /// Cropped width.
    #[serde(default)]
    pub width: f64,
    /// Cropped height.
    #[serde(default)]
    pub height: f64,
    /// Zoom factor. One in the planner's default, which is why this type is not `Default`-derived.
    #[serde(default = "one")]
    pub zoom: f64,
}

/// `makeDefaultSliders()`'s `zoom`.
fn one() -> f64 {
    1.0
}

impl Default for SliderImageGeometry {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            zoom: one(),
        }
    }
}

/// Why a character's appearance could not be read out of a build document.
///
/// Each variant carries the one sentence a player sees, in the same shape as
/// [`crate::UrlRejection`]: the reason a thing was rejected lives with the rejection rather than
/// being reconstructed at the call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlidersRejection {
    /// The build carries no `sliders` key, or one with no slider values in it.
    Absent,
    /// The character's own buffer is not [`FACE_BUFFER_LEN`] bytes.
    BufferLength {
        /// What was supplied.
        got: usize,
    },
    /// The character's own buffer does not begin with [`FACE_BUFFER_MAGIC`].
    BufferMagic,
    /// The character's own buffer declares a version the game's writer refuses.
    BufferVersion {
        /// What it declares.
        got: u32,
    },
}

impl SlidersRejection {
    /// The line a player is shown.
    pub fn indicator(self) -> String {
        match self {
            SlidersRejection::Absent => {
                "That build carries no appearance, so the character keeps its own".to_owned()
            }
            SlidersRejection::BufferLength { got } => format!(
                "This character's appearance is {got} bytes, not {FACE_BUFFER_LEN}; it was not \
                 read from live save data"
            ),
            SlidersRejection::BufferMagic => {
                "This character's appearance does not begin with the FACE magic".to_owned()
            }
            SlidersRejection::BufferVersion { got } => format!(
                "This character's appearance declares version {got}; the game's own writer \
                 accepts only {FACE_BUFFER_VERSION}"
            ),
        }
    }

    /// Stable telemetry code. Zero means accepted, as it does for [`crate::UrlRejection`].
    pub fn code(self) -> usize {
        match self {
            SlidersRejection::Absent => 1,
            SlidersRejection::BufferLength { .. } => 2,
            SlidersRejection::BufferMagic => 3,
            SlidersRejection::BufferVersion { .. } => 4,
        }
    }
}

/// Check a whole `FaceDataBuffer` before anything reads or writes its payload.
///
/// The three things the game's own writer requires and does not report on: the length, the magic
/// and the version. Run on the character's live buffer rather than on the document, because the
/// document carries no buffer -- it carries sliders, which are written into the character's.
///
/// # Errors
///
/// Returns the specific defect, so a caller can say which one it was.
pub fn check_face_buffer(buffer: &[u8]) -> Result<(), SlidersRejection> {
    if buffer.len() != FACE_BUFFER_LEN {
        return Err(SlidersRejection::BufferLength { got: buffer.len() });
    }
    if buffer[..FACE_BUFFER_MAGIC.len()] != FACE_BUFFER_MAGIC {
        return Err(SlidersRejection::BufferMagic);
    }
    let version = u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
    if version != FACE_BUFFER_VERSION {
        return Err(SlidersRejection::BufferVersion { got: version });
    }
    Ok(())
}

/// Read the sliders out of a whole `FaceDataBuffer`.
///
/// ```
/// use er_build_import_core::sliders;
/// let mut buffer = [0u8; sliders::FACE_BUFFER_LEN];
/// buffer[..4].copy_from_slice(b"FACE");
/// buffer[4..8].copy_from_slice(&4u32.to_le_bytes());
/// buffer[8..12].copy_from_slice(&288u32.to_le_bytes());
/// // faceModelId = 141 -> age 1, boneStructure 40, musculature 1.
/// buffer[12..16].copy_from_slice(&141u32.to_le_bytes());
///
/// let decoded = sliders::decode_face_buffer(&buffer).expect("a well-formed buffer");
/// assert_eq!(decoded["age"], 1);
/// assert_eq!(decoded["boneStructure"], 40);
/// assert_eq!(decoded["musculature"], 1);
/// assert!(!decoded.contains_key("faceModelId"));
/// ```
///
/// # Errors
///
/// Returns the defect when the buffer is not one the game would accept.
pub fn decode_face_buffer(buffer: &[u8]) -> Result<SliderMap, SlidersRejection> {
    check_face_buffer(buffer)?;
    Ok(decode(
        &buffer[FACE_BUFFER_PAYLOAD_OFFSET..FACE_BUFFER_PAYLOAD_OFFSET + SLIDER_BYTES],
    ))
}

/// Read the sliders out of the planner's own [`SLIDER_BYTES`]-byte range.
///
/// A shorter slice is read as far as it goes and the rest reads as zero, which is what the
/// planner's own importer does when an AOB runs out early. Callers with a whole buffer should use
/// [`decode_face_buffer`], which checks the header first.
pub fn decode(payload: &[u8]) -> SliderMap {
    let byte = |at: usize| payload.get(at).copied().unwrap_or(0);
    let mut out = SliderMap::new();
    for field in layout() {
        match field.kind {
            FieldKind::Byte => {
                out.insert(field.key.to_owned(), Value::from(byte(field.offset)));
            }
            FieldKind::Colour => {
                let channels: Vec<Value> = (0..FieldKind::Colour.size())
                    .map(|channel| Value::from(byte(field.offset + channel)))
                    .collect();
                out.insert(field.key.to_owned(), Value::Array(channels));
            }
            FieldKind::List => {
                let value = u32::from_le_bytes([
                    byte(field.offset),
                    byte(field.offset + 1),
                    byte(field.offset + 2),
                    byte(field.offset + 3),
                ]);
                if field.key == FACE_MODEL_ID_KEY {
                    let musculature = u32::from(value >= MUSCULATURE_STEP);
                    let remainder = value - MUSCULATURE_STEP * musculature;
                    out.insert(AGE_KEY.to_owned(), Value::from(remainder % AGE_MODULUS));
                    out.insert(
                        BONE_STRUCTURE_KEY.to_owned(),
                        Value::from(remainder / AGE_MODULUS * AGE_MODULUS),
                    );
                    out.insert(MUSCULATURE_KEY.to_owned(), Value::from(musculature));
                } else {
                    out.insert(field.key.to_owned(), Value::from(value));
                }
            }
        }
    }
    out
}

/// Write the sliders back over a character's own `FaceDataBuffer`, in place.
///
/// Only `buffer[12..276]` is touched. The magic, the version, the declared size and the twelve
/// bytes past the planner's range keep whatever the character already had -- which is what makes
/// the result a buffer the game's own writer will accept rather than one assembled from a
/// document.
///
/// A key the document does not carry, or carries with the wrong JSON shape, writes zero, exactly
/// as the planner's own exporter does with `r || 0`. That is a real decision and not a
/// convenience: a slider set is dense and a missing key is the planner's own way of spelling the
/// minimum, so refusing the whole appearance over one absent key would reject documents its
/// Cosmetics tab writes.
///
/// # Errors
///
/// Returns the defect when `buffer` is not a well-formed `FaceDataBuffer`.
pub fn encode_into(sliders: &SliderMap, buffer: &mut [u8]) -> Result<(), SlidersRejection> {
    check_face_buffer(buffer)?;
    let payload =
        &mut buffer[FACE_BUFFER_PAYLOAD_OFFSET..FACE_BUFFER_PAYLOAD_OFFSET + SLIDER_BYTES];
    for field in layout() {
        match field.kind {
            FieldKind::Byte => {
                payload[field.offset] = number(sliders, field.key) as u8;
            }
            FieldKind::Colour => {
                let channels = sliders.get(field.key).and_then(Value::as_array);
                for channel in 0..FieldKind::Colour.size() {
                    let value = channels
                        .and_then(|list| list.get(channel))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    payload[field.offset + channel] = value.min(u64::from(u8::MAX)) as u8;
                }
            }
            FieldKind::List => {
                let value = if field.key == FACE_MODEL_ID_KEY {
                    let musculature = u32::from(number(sliders, MUSCULATURE_KEY) != 0);
                    number(sliders, BONE_STRUCTURE_KEY)
                        .saturating_add(number(sliders, AGE_KEY))
                        .saturating_add(MUSCULATURE_STEP * musculature)
                } else {
                    number(sliders, field.key)
                };
                payload[field.offset..field.offset + FieldKind::List.size()]
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(())
}

/// One key as a number, treating an absent or wrongly-shaped value as zero.
///
/// `true` reads as 1 so that a document written by something that took the planner's `boolean`
/// type literally still encodes, rather than silently flattening that slider to zero.
fn number(sliders: &SliderMap, key: &str) -> u32 {
    match sliders.get(key) {
        Some(Value::Number(found)) => found.as_u64().unwrap_or(0).min(u64::from(u32::MAX)) as u32,
        Some(Value::Bool(true)) => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed empty buffer, header and all.
    fn buffer() -> Vec<u8> {
        let mut out = vec![0u8; FACE_BUFFER_LEN];
        out[..4].copy_from_slice(&FACE_BUFFER_MAGIC);
        out[4..8].copy_from_slice(&FACE_BUFFER_VERSION.to_le_bytes());
        out[8..12].copy_from_slice(&(FACE_BUFFER_LEN as u32).to_le_bytes());
        out
    }

    /// A buffer whose payload is a deterministic, fully-populated pattern.
    ///
    /// Every byte distinct-ish and non-zero so that a field read from the wrong offset cannot
    /// coincide with the right one, which an all-zero or all-equal payload would hide.
    ///
    /// The eight leading `i32` model ids are written separately, in the three-digit range real
    /// characters use, so that this fixture reads as a plausible face rather than as noise. The
    /// extremes are covered by `a_model_id_survives_the_full_four_bytes` instead.
    fn populated() -> Vec<u8> {
        let mut out = buffer();
        for (index, byte) in out[FACE_BUFFER_PAYLOAD_OFFSET..].iter_mut().enumerate() {
            *byte = (index.wrapping_mul(37).wrapping_add(11) % 251) as u8;
        }
        for index in 0..8u32 {
            let at = FACE_BUFFER_PAYLOAD_OFFSET + index as usize * 4;
            // 141 is the first, so `faceModelId` exercises the hundreds digit rather than a value
            // whose musculature is always zero.
            out[at..at + 4].copy_from_slice(&(141 + index * 37).to_le_bytes());
        }
        // The tail past the planner's range, which nothing should ever write.
        for byte in out[FACE_BUFFER_PAYLOAD_OFFSET + SLIDER_BYTES..].iter_mut() {
            *byte = 0xA5;
        }
        out
    }

    #[test]
    fn the_layout_tiles_the_planners_range_exactly() {
        let mut covered = 0;
        for field in layout() {
            assert_eq!(field.offset, covered, "gap or overlap at {:?}", field.key);
            covered += field.kind.size();
        }
        assert_eq!(covered, SLIDER_BYTES);
        assert_eq!(layout().len(), 200);
    }

    #[test]
    fn face_model_id_is_carried_as_three_keys_and_not_as_itself() {
        let decoded = decode_face_buffer(&buffer()).expect("well formed");
        for key in [AGE_KEY, BONE_STRUCTURE_KEY, MUSCULATURE_KEY] {
            assert!(decoded.contains_key(key), "{key} should be present");
        }
        assert!(!decoded.contains_key(FACE_MODEL_ID_KEY));
    }

    /// The decomposition has to survive every value the field can hold, not just the small ones.
    /// 500 is the case their own AOB importer gets wrong -- it reads one byte and comes back with
    /// 244 -- and a real character in the save corpus has it.
    #[test]
    fn every_face_model_id_survives_the_three_key_split() {
        for id in (0u32..=1000).chain([0xFFFF, 0x1_0000, 60_000]) {
            let mut original = buffer();
            original[FACE_BUFFER_PAYLOAD_OFFSET..FACE_BUFFER_PAYLOAD_OFFSET + 4]
                .copy_from_slice(&id.to_le_bytes());
            let decoded = decode_face_buffer(&original).expect("well formed");
            let mut written = buffer();
            encode_into(&decoded, &mut written).expect("well formed");
            assert_eq!(written, original, "faceModelId {id} did not survive");
        }
    }

    #[test]
    fn a_populated_payload_round_trips_byte_for_byte() {
        let original = populated();
        let decoded = decode_face_buffer(&original).expect("well formed");
        let mut written = populated();
        // Scrub the range the encoder owns, so a field it never writes shows up as a difference
        // rather than as the value that happened to be there already.
        written[FACE_BUFFER_PAYLOAD_OFFSET..FACE_BUFFER_PAYLOAD_OFFSET + SLIDER_BYTES].fill(0);
        encode_into(&decoded, &mut written).expect("well formed");
        assert_eq!(written, original);
    }

    #[test]
    fn the_twelve_bytes_past_the_planners_range_are_never_written() {
        let mut written = populated();
        let empty = SliderMap::new();
        encode_into(&empty, &mut written).expect("well formed");
        assert!(
            written[FACE_BUFFER_PAYLOAD_OFFSET + SLIDER_BYTES..]
                .iter()
                .all(|byte| *byte == 0xA5),
            "the tail was overwritten"
        );
        // And the header, which the game's own writer gates on.
        assert_eq!(&written[..4], &FACE_BUFFER_MAGIC);
        check_face_buffer(&written).expect("still well formed");
    }

    #[test]
    fn a_malformed_buffer_is_refused_rather_than_written() {
        let mut short = buffer();
        short.truncate(FACE_BUFFER_LEN - 1);
        assert_eq!(
            check_face_buffer(&short),
            Err(SlidersRejection::BufferLength {
                got: FACE_BUFFER_LEN - 1
            })
        );

        let mut wrong_magic = buffer();
        wrong_magic[0] = b'f';
        assert_eq!(
            check_face_buffer(&wrong_magic),
            Err(SlidersRejection::BufferMagic)
        );

        let mut wrong_version = buffer();
        wrong_version[4..8].copy_from_slice(&5u32.to_le_bytes());
        assert_eq!(
            check_face_buffer(&wrong_version),
            Err(SlidersRejection::BufferVersion { got: 5 })
        );

        // And the write path refuses the same three, instead of writing a buffer the game's own
        // `CopyFromBuffer` would silently ignore.
        let sliders = decode_face_buffer(&buffer()).expect("well formed");
        assert!(encode_into(&sliders, &mut wrong_version).is_err());
    }

    #[test]
    fn a_hostile_document_encodes_rather_than_panicking() {
        let mut sliders = SliderMap::new();
        // Every wrong shape a `JSON.parse` can hand us for a key we expect to be a number.
        sliders.insert("age".to_owned(), Value::String("nine".to_owned()));
        sliders.insert("boneStructure".to_owned(), Value::Null);
        sliders.insert("musculature".to_owned(), Value::Bool(true));
        sliders.insert("hairModelId".to_owned(), Value::from(u64::MAX));
        sliders.insert("apparentAge".to_owned(), Value::from(-7));
        sliders.insert("eyeColour".to_owned(), Value::from(3));
        let mut written = buffer();
        encode_into(&sliders, &mut written).expect("well formed");
        check_face_buffer(&written).expect("still well formed");
        // `musculature: true` is the one shape that is honoured rather than zeroed.
        assert_eq!(
            u32::from_le_bytes(written[12..16].try_into().expect("four bytes")),
            MUSCULATURE_STEP
        );
    }

    /// The eight `list` rows are the eight leading `i32`s of the payload, and every one of them
    /// survives whole -- including a value whose high bit is set, which one of the 62 real
    /// characters in the save corpus carries and which an earlier `i32::MAX` clamp here silently
    /// rewrote.
    #[test]
    fn a_model_id_survives_the_full_four_bytes() {
        let lists: Vec<&SliderField> = layout()
            .iter()
            .filter(|field| field.kind == FieldKind::List)
            .collect();
        assert_eq!(lists.len(), 8, "the leading i32 block is eight fields");
        assert_eq!(
            lists.iter().map(|field| field.kind.size()).sum::<usize>(),
            32,
            "and it tiles the first 32 bytes"
        );
        assert!(lists.iter().all(|field| field.offset < 32));

        // The corpus value, plus the extremes either side of the signed boundary.
        for id in [0x8d00_0228u32, 0x8000_0000, 0x7fff_ffff, u32::MAX, 502] {
            let mut original = buffer();
            original[FACE_BUFFER_PAYLOAD_OFFSET..FACE_BUFFER_PAYLOAD_OFFSET + 4]
                .copy_from_slice(&id.to_le_bytes());
            let decoded = decode_face_buffer(&original).expect("well formed");
            let mut written = buffer();
            encode_into(&decoded, &mut written).expect("well formed");
            assert_eq!(written, original, "faceModelId {id:#x} did not survive");
        }
    }

    #[test]
    fn a_colour_is_three_channels_in_order() {
        let mut original = buffer();
        let colour = layout()
            .iter()
            .find(|field| field.kind == FieldKind::Colour)
            .expect("the layout has colours");
        let at = FACE_BUFFER_PAYLOAD_OFFSET + colour.offset;
        original[at..at + 3].copy_from_slice(&[1, 2, 3]);
        let decoded = decode_face_buffer(&original).expect("well formed");
        assert_eq!(decoded[colour.key], Value::from(vec![1, 2, 3]));
        let mut written = buffer();
        encode_into(&decoded, &mut written).expect("well formed");
        assert_eq!(&written[at..at + 3], &[1, 2, 3]);
    }

    #[test]
    fn body_type_reads_anything_and_writes_one_of_two() {
        assert_eq!(BodyType::from("B".to_owned()), BodyType::B);
        assert_eq!(BodyType::from("b".to_owned()), BodyType::B);
        assert_eq!(BodyType::from("A".to_owned()), BodyType::A);
        assert_eq!(BodyType::from("nonsense".to_owned()), BodyType::A);
        assert_eq!(String::from(BodyType::B), "B");
    }
}
