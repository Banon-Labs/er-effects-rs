//! The appearance, end to end: a character's own `FaceDataBuffer` out through the encoder, back
//! through the independent decoder and the importer's reader, and written over a fresh buffer.
//!
//! # Why this is the test the old key never had
//!
//! The document used to carry a `faceData` key: the whole 288-byte buffer as hex. It was written,
//! stored, and echoed back byte for byte -- and none of that was evidence of anything, because
//! nothing ever read it. "The hex survives" only proved a string round-tripped through a string.
//!
//! What has to be true now is that the **bytes** come back, having been taken apart into two
//! hundred named sliders by a layout a second program chose and put together again. A test that
//! only checked the sliders object would miss an offset error; a test that only checked the bytes
//! on one pass would miss a codec that dropped a field symmetrically. So this runs the whole
//! chain -- encoder, independent decoder, importer's reader, encoder again -- and differences the
//! buffer at both ends.

mod common;

use er_build_export::model::BuildExportDoc;
use er_build_export::share_payload;
use er_build_import_core::model;
use er_build_import_core::sliders::{
    self, BodyType, FACE_BUFFER_LEN, FACE_BUFFER_MAGIC, FACE_BUFFER_PAYLOAD_OFFSET,
    FACE_BUFFER_VERSION, SLIDER_BYTES, SlidersDoc,
};

/// The planner's byte range within the whole buffer.
const PAYLOAD: std::ops::Range<usize> =
    FACE_BUFFER_PAYLOAD_OFFSET..FACE_BUFFER_PAYLOAD_OFFSET + SLIDER_BYTES;

/// The `faceModelId` their own AOB importer truncates to 244, and which a character in the save
/// corpus actually has. Every codec in this chain has to carry it whole.
const WIDE_FACE_MODEL_ID: u32 = 500;

/// A well-formed buffer whose payload is a deterministic pattern.
///
/// `seed` varies it, so a test can assert two different faces stay different rather than only that
/// one face survives -- an encoder that wrote a constant would pass the second and fail nothing.
fn face_buffer(seed: u8) -> Vec<u8> {
    let mut out = vec![0u8; FACE_BUFFER_LEN];
    out[..4].copy_from_slice(&FACE_BUFFER_MAGIC);
    out[4..8].copy_from_slice(&FACE_BUFFER_VERSION.to_le_bytes());
    out[8..12].copy_from_slice(&(FACE_BUFFER_LEN as u32).to_le_bytes());
    for (index, byte) in out[FACE_BUFFER_PAYLOAD_OFFSET..].iter_mut().enumerate() {
        *byte = (index.wrapping_mul(37).wrapping_add(usize::from(seed)) % 251) as u8;
    }
    // The eight leading model ids, in the range real characters use. They are the one part of the
    // payload where an arbitrary bit pattern is not a face the game would accept: its own
    // `ValidateFaceData` requires these eight `i32`s to be non-negative.
    for index in 0..8u32 {
        let at = FACE_BUFFER_PAYLOAD_OFFSET + index as usize * 4;
        let id = if index == 0 {
            WIDE_FACE_MODEL_ID
        } else {
            100 + index * 13 + u32::from(seed)
        };
        out[at..at + 4].copy_from_slice(&id.to_le_bytes());
    }
    out
}

/// Encode a character's buffer into a share link, decode the link independently, and write the
/// appearance it carries back over a buffer belonging to the same character.
fn through_a_share_link(original: &[u8]) -> Vec<u8> {
    let decoded = sliders::decode_face_buffer(original).expect("a well-formed buffer");
    let doc = BuildExportDoc {
        sliders: Some(SlidersDoc::new(BodyType::B, decoded)),
        ..BuildExportDoc::with_level(150, false)
    };

    let json = common::decode_payload(&share_payload(&doc));
    let read = model::parse(&json).expect("the importer parses a document this crate wrote");
    let appearance = read.appearance().expect("the build carries an appearance");
    assert_eq!(
        appearance.body_type,
        BodyType::B,
        "bodyType did not survive"
    );

    // The importer writes over the character's own buffer, so that is what this reproduces:
    // header and tail from the character, payload from the document. The range the encoder owns
    // is scrubbed first, so a field it never writes shows up as a difference rather than as the
    // value that happened to be sitting there.
    let mut written = original.to_vec();
    written[PAYLOAD].fill(0);
    sliders::encode_into(&appearance.sliders, &mut written).expect("a well-formed buffer");
    written
}

#[test]
fn a_face_survives_export_import_and_export_again_byte_for_byte() {
    let original = face_buffer(11);
    let written = through_a_share_link(&original);
    assert_eq!(
        &written[PAYLOAD], &original[PAYLOAD],
        "the planner's 264-byte range did not survive the round trip"
    );
    // The whole buffer, which additionally proves the header and the twelve bytes past the
    // planner's range were carried rather than rebuilt.
    assert_eq!(written, original);

    // Exporting the recovered appearance again must produce the same document. This is the half a
    // single pass cannot see: a codec that lost a field symmetrically would still match above.
    let once = sliders::decode_face_buffer(&original).expect("well formed");
    let twice = sliders::decode_face_buffer(&written).expect("well formed");
    assert_eq!(once, twice);
}

#[test]
fn two_different_faces_do_not_collapse_to_one() {
    let (first, second) = (face_buffer(11), face_buffer(97));
    assert_ne!(first, second, "the fixture should vary with its seed");
    assert_ne!(through_a_share_link(&first), through_a_share_link(&second));
}

/// The one value their own codec loses: `importAOB` reads a single byte for a four-byte `list`
/// field while `exportAOB` writes four. This is the regression that speaks up if anyone ever
/// "fixes" our reader to match theirs.
#[test]
fn a_four_byte_model_id_is_not_truncated_to_its_low_byte() {
    let written = through_a_share_link(&face_buffer(11));
    let at = FACE_BUFFER_PAYLOAD_OFFSET;
    let id = u32::from_le_bytes(written[at..at + 4].try_into().expect("four bytes"));
    assert_eq!(id, WIDE_FACE_MODEL_ID);
    assert_ne!(
        id,
        WIDE_FACE_MODEL_ID & 0xFF,
        "this is the planner's own truncation bug"
    );
}

/// A build with no appearance asks for none. The importer has to tell "no appearance" from "an
/// appearance of all zeroes", because the second is a face and the first is not.
#[test]
fn a_build_without_cosmetics_asks_for_no_appearance() {
    let doc = BuildExportDoc::with_level(150, false);
    let json = common::decode_payload(&share_payload(&doc));
    assert!(
        !json.contains("\"sliders\""),
        "an unset appearance should write no key at all"
    );
    let read = model::parse(&json).expect("parses");
    assert!(read.appearance().is_err());
}

/// Every real character in the local save corpus, when one is present.
///
/// The synthetic fixtures above prove the codec against patterns chosen to break it; this proves
/// it against faces people actually made, which is where a layout row that is subtly the wrong
/// type shows up. Game bytes are never committed (see `AGENTS.md`), so the corpus is read from
/// disk and the test skips when it is absent -- the same shape `er-gfx` uses for its own.
///
/// Populate it with `python3 scripts/dump-face-corpus.py`, which writes one 288-byte `.bin` per
/// character; point this at another directory with `ER_FACE_CORPUS_DIR`.
#[test]
fn every_face_in_the_local_save_corpus_round_trips() {
    let root = std::env::var("ER_FACE_CORPUS_DIR").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/face-corpus").to_owned()
    });
    let Ok(entries) = std::fs::read_dir(&root) else {
        eprintln!("skipping: no face corpus at {root} (see scripts/dump-face-corpus.py)");
        return;
    };
    let mut checked = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|kind| kind != "bin") {
            continue;
        }
        let original = std::fs::read(&path).expect("a readable corpus file");
        sliders::check_face_buffer(&original)
            .unwrap_or_else(|why| panic!("{}: {}", path.display(), why.indicator()));
        assert_eq!(
            through_a_share_link(&original),
            original,
            "{} did not survive the round trip",
            path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "the corpus at {root} holds no .bin files");
    eprintln!("{checked} real character(s) round-tripped");
}
