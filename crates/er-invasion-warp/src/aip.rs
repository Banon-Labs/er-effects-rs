//! The on-disk `.aip` (auto-invade-point) record decoder.
//!
//! Each entry inside `other:/AutoInvadePoint.aipbnd` (and `_dlc02`) is one `.aip` file
//! holding one block's spawn points. Being able to decode them OFFLINE is what lets the
//! catalog be validated with no game running: the runtime array is a verbatim `memcpy` of
//! the file body, so a target decoded here is byte-for-byte the target the engine will hold.
//!
//! # Layout, PROVEN from `CS::CSAutoInvadePoint::AddForBlockId` @ 0x140a69550 (1.16.2)
//!
//! ```text
//! +0x00  char  magic[4]   == "FPIA"  (the decompile's `*(int*)magic == 0x41495046`)
//! +0x04  u32   version    == 1       (the decompile's `field1_0x4 == 1`)
//! +0x08  u8    area       \
//! +0x09  u8    block       |  passed to BlockId::BlockId(area, block, region, index)
//! +0x0A  u8    region      |  -- i.e. the REVERSE of the in-memory BlockId byte order
//! +0x0B  u8    index      /
//! +0x0C  u32   count      number of points
//! +0x10  [16 bytes] * count   x: f32, y: f32, z: f32, yaw: f32
//! ```
//!
//! The decompile allocates `fileSize - 0x10` and `memcpy`s from `data + 1` (one header), so
//! `file_len == 0x10 + 0x10 * count` is a structural invariant, and the runtime
//! `AutoInvadePoint { position: F32Vector3, yaw: f32 }` is the on-disk record unchanged.
//!
//! # Corpus, not fixtures
//!
//! Game-derived bytes are never versioned in this repo. The constants below are FINGERPRINTS
//! (lengths, counts, FNV-1a64 digests) plus a deterministic [`encode_aip`] generator that
//! builds synthetic files for the unit tests. The integration test that reads the real
//! extraction reads it from disk and SKIPs when it is absent.

use crate::invasion_warp::{BlockKey, InvasionWarpTarget};
use er_game_base::fnv1a::{fnv1a64, fnv1a64_extend};

/// `"FPIA"` -- the four magic bytes at offset 0. As a little-endian `u32` this is the
/// `0x41495046` the decompile compares against.
pub const AIP_MAGIC: [u8; 4] = *b"FPIA";
/// The only version `AddForBlockId` accepts.
pub const AIP_VERSION: u32 = 1;
/// Header bytes before the first point.
pub const AIP_HEADER_LEN: usize = 0x10;
/// Bytes per point record (`x`, `y`, `z`, `yaw`, all `f32` little-endian).
pub const AIP_POINT_LEN: usize = 0x10;

/// Why an `.aip` payload could not be decoded. Each variant mirrors one guard the engine
/// itself applies (or one the engine relies on the packer having upheld).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AipParseError {
    /// Shorter than the fixed header.
    TooShort { len: usize },
    /// Magic is not `FPIA`; `AddForBlockId` silently drops such an entry.
    BadMagic { found: [u8; 4] },
    /// Version is not 1; `AddForBlockId` silently drops such an entry.
    BadVersion { found: u32 },
    /// `count` disagrees with the payload length. The engine trusts `fileSize` for the copy
    /// and `count` for the length, so a mismatch would make the runtime array read out of
    /// bounds -- refuse it here rather than reproduce that.
    LengthMismatch {
        len: usize,
        count: u32,
        expected_len: usize,
    },
}

impl std::fmt::Display for AipParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort { len } => {
                write!(
                    f,
                    "aip payload is {len} bytes, shorter than the {AIP_HEADER_LEN}-byte header"
                )
            }
            Self::BadMagic { found } => write!(f, "aip magic is {found:02x?}, expected FPIA"),
            Self::BadVersion { found } => {
                write!(f, "aip version is {found}, expected {AIP_VERSION}")
            }
            Self::LengthMismatch {
                len,
                count,
                expected_len,
            } => write!(
                f,
                "aip declares {count} points ({expected_len} bytes) but the payload is {len} bytes"
            ),
        }
    }
}

impl std::error::Error for AipParseError {}

/// One decoded `.aip` file: the block it belongs to and its points.
#[derive(Clone, Debug, PartialEq)]
pub struct AipFile {
    pub block: BlockKey,
    pub targets: Vec<InvasionWarpTarget>,
}

/// Decode one `.aip` payload.
pub fn parse_aip(bytes: &[u8]) -> Result<AipFile, AipParseError> {
    if bytes.len() < AIP_HEADER_LEN {
        return Err(AipParseError::TooShort { len: bytes.len() });
    }
    let magic: [u8; 4] = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if magic != AIP_MAGIC {
        return Err(AipParseError::BadMagic { found: magic });
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != AIP_VERSION {
        return Err(AipParseError::BadVersion { found: version });
    }
    let block = BlockKey::from_disk_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let expected_len = AIP_HEADER_LEN + AIP_POINT_LEN * count as usize;
    if bytes.len() != expected_len {
        return Err(AipParseError::LengthMismatch {
            len: bytes.len(),
            count,
            expected_len,
        });
    }

    let mut targets = Vec::with_capacity(count as usize);
    for point_index in 0..count as usize {
        let base = AIP_HEADER_LEN + point_index * AIP_POINT_LEN;
        let read = |offset: usize| -> f32 {
            let at = base + offset;
            f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        targets.push(InvasionWarpTarget::new(
            block,
            point_index as u32,
            [read(0), read(4), read(8)],
            read(12),
        ));
    }
    Ok(AipFile { block, targets })
}

/// Build a well-formed `.aip` payload. Deterministic generator for tests and round-trip
/// proofs -- this repo never versions game-derived bytes, so every unit test builds its own
/// input here rather than embedding one.
#[must_use]
pub fn encode_aip(block: BlockKey, points: &[([f32; 3], f32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(AIP_HEADER_LEN + AIP_POINT_LEN * points.len());
    out.extend_from_slice(&AIP_MAGIC);
    out.extend_from_slice(&AIP_VERSION.to_le_bytes());
    // Disk order is area, block, region, index -- the reverse of the in-memory byte order.
    out.push(block.area());
    out.push(block.block());
    out.push(block.region());
    out.push(block.index_packed());
    out.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for (position, yaw) in points {
        for component in position {
            out.extend_from_slice(&component.to_le_bytes());
        }
        out.extend_from_slice(&yaw.to_le_bytes());
    }
    out
}

/// Digest of a catalog's CONTENT, computable identically from live memory and from disk.
///
/// This exists to answer one question with a measurement instead of an argument: **does a mod
/// rewrite the invasion point table in memory, or only change how the game picks from it?**
/// Seamless Co-op modifies invasion locations at runtime, and the answer decides whether on-disk
/// data can ever be trusted to describe what a player will actually encounter.
///
/// The existing count-based oracle cannot answer it -- a mod that MOVES points without adding or
/// removing any leaves 365 blocks and 7073 points looking untouched. This folds every position and
/// yaw, so a moved point changes the digest.
///
/// Canonical form: blocks in ascending raw-BlockId order, each contributed as the exact `.aip`
/// payload [`encode_aip`] produces. That is what makes the live and on-disk sides comparable --
/// the disk side is literally those bytes, and the live side reconstructs them.
///
/// `targets` must be grouped by block and ordered by point index within each block, which is the
/// order the catalog already guarantees.
#[must_use]
pub fn catalog_content_digest(targets: &[(BlockKey, [f32; 3], f32)]) -> u64 {
    let mut blocks: Vec<BlockKey> = targets.iter().map(|(block, _, _)| *block).collect();
    blocks.sort_unstable_by_key(|block| block.raw());
    blocks.dedup();

    let mut hash = fnv1a64(b"");
    for block in blocks {
        let points: Vec<([f32; 3], f32)> = targets
            .iter()
            .filter(|(candidate, _, _)| *candidate == block)
            .map(|(_, position, yaw)| (*position, *yaw))
            .collect();
        let payload = encode_aip(block, &points);
        hash = fnv1a64_extend(hash, &payload);
    }
    hash
}

/// [`catalog_content_digest`] over the VANILLA on-disk containers (ELDEN RING 1.16.2): all 365
/// entries, 7073 points, blocks in ascending raw-BlockId order.
///
/// A live catalog that hashes to this is byte-for-byte the shipped table. A live catalog that
/// does NOT -- while still reporting 365 blocks and 7073 points, as the count oracle would --
/// has been rewritten in memory by a mod, and that is the signal that on-disk data cannot
/// describe what the player will actually meet.
///
/// This is a fingerprint, not game data: 8 bytes derived from the containers, which is exactly
/// the form this repo versions instead of the bytes themselves.
pub const AIP_CATALOG_CONTENT_DIGEST_VANILLA: u64 = 0xadf8_f8d3_2e3e_8504;

/// Fingerprints of the shipped auto-invade-point data (ELDEN RING 1.16.2), so a corpus test
/// can assert it is looking at the same bytes these RE claims were made about without any
/// game-derived file entering the repo.
///
/// The `entries_fnv1a64` digest is taken over the container's `.aip` entries sorted by name,
/// each contributed as `name_bytes || u32le(len) || content`.
pub struct AipContainerFingerprint {
    /// Container file name, as extracted (`other/` in an unpack).
    pub container_name: &'static str,
    /// Length of the compressed `.aipbnd.dcx` container.
    pub container_len: usize,
    /// Number of `.aip` entries (== number of distinct blocks).
    pub entry_count: usize,
    /// Sum of every entry's point count.
    pub point_count: usize,
    /// The single map area every entry in this container belongs to.
    pub area: u8,
    /// FNV-1a64 over the sorted decompressed entries (see the struct doc).
    pub entries_fnv1a64: u64,
}

/// Base game: `other:/AutoInvadePoint.aipbnd`, area 60 (the Lands Between).
pub const AIP_FINGERPRINT_BASE: AipContainerFingerprint = AipContainerFingerprint {
    container_name: "autoinvadepoint.aipbnd.dcx",
    container_len: 54423,
    entry_count: 257,
    point_count: 4482,
    area: 60,
    entries_fnv1a64: 0xc9a0_987b_f23e_1e66,
};

/// DLC: `other:/AutoInvadePoint_dlc02.aipbnd`, area 61 (Shadow of the Erdtree).
pub const AIP_FINGERPRINT_DLC02: AipContainerFingerprint = AipContainerFingerprint {
    container_name: "autoinvadepoint_dlc02.aipbnd.dcx",
    container_len: 32443,
    entry_count: 108,
    point_count: 2591,
    area: 61,
    entries_fnv1a64: 0xb801_a966_3189_f57d,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invasion_warp::AUTHORED_YAW_EXTREME;

    fn sample_block() -> BlockKey {
        // m60_34_51_00 -- the smallest shipped entry (one point, 32 bytes).
        BlockKey::from_raw(0x3C22_3300)
    }

    #[test]
    fn a_generated_file_round_trips_through_the_parser() {
        let block = sample_block();
        let points = [
            ([103.79f32, 456.07, -66.27], -1.09f32),
            ([1.0, 2.0, 3.0], AUTHORED_YAW_EXTREME),
        ];
        let bytes = encode_aip(block, &points);
        assert_eq!(bytes.len(), AIP_HEADER_LEN + AIP_POINT_LEN * points.len());

        let parsed = parse_aip(&bytes).expect("generated file must parse");
        assert_eq!(parsed.block, block);
        assert_eq!(parsed.targets.len(), 2);
        assert_eq!(parsed.targets[0].position, points[0].0);
        assert_eq!(parsed.targets[0].yaw, points[0].1);
        assert_eq!(parsed.targets[1].point_index, 1);
    }

    #[test]
    fn the_header_carries_the_block_id_in_disk_byte_order() {
        let bytes = encode_aip(sample_block(), &[]);
        // The bytes the real m60_34_51_00.aip carries at 0x08.
        assert_eq!(&bytes[8..12], &[0x3C, 0x22, 0x33, 0x00]);
        assert_eq!(&bytes[0..4], b"FPIA");
        assert_eq!(
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            1
        );
    }

    #[test]
    fn an_empty_point_list_is_a_valid_header_only_file() {
        let parsed = parse_aip(&encode_aip(sample_block(), &[])).expect("header-only parses");
        assert!(parsed.targets.is_empty());
    }

    #[test]
    fn a_truncated_payload_is_rejected() {
        let bytes = encode_aip(sample_block(), &[]);
        assert_eq!(
            parse_aip(&bytes[..8]),
            Err(AipParseError::TooShort { len: 8 })
        );
        assert_eq!(parse_aip(&[]), Err(AipParseError::TooShort { len: 0 }));
    }

    #[test]
    fn a_wrong_magic_is_rejected() {
        let mut bytes = encode_aip(sample_block(), &[]);
        bytes[0] = b'X';
        assert_eq!(
            parse_aip(&bytes),
            Err(AipParseError::BadMagic { found: *b"XPIA" })
        );
    }

    #[test]
    fn a_wrong_version_is_rejected() {
        let mut bytes = encode_aip(sample_block(), &[]);
        bytes[4] = 2;
        assert_eq!(
            parse_aip(&bytes),
            Err(AipParseError::BadVersion { found: 2 })
        );
    }

    #[test]
    fn a_count_that_disagrees_with_the_length_is_rejected() {
        // One real point, but the header claims two: the engine would read past the end.
        let mut bytes = encode_aip(sample_block(), &[([0.0; 3], 0.0)]);
        bytes[12] = 2;
        assert_eq!(
            parse_aip(&bytes),
            Err(AipParseError::LengthMismatch {
                len: 32,
                count: 2,
                expected_len: 48,
            })
        );
        // ...and the same guard catches a payload with trailing junk.
        let mut padded = encode_aip(sample_block(), &[([0.0; 3], 0.0)]);
        padded.push(0);
        assert!(matches!(
            parse_aip(&padded),
            Err(AipParseError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn every_error_variant_renders_a_message_naming_its_cause() {
        assert!(
            AipParseError::TooShort { len: 3 }
                .to_string()
                .contains("shorter")
        );
        assert!(
            AipParseError::BadMagic { found: *b"XPIA" }
                .to_string()
                .contains("FPIA")
        );
        assert!(
            AipParseError::BadVersion { found: 9 }
                .to_string()
                .contains("expected 1")
        );
        assert!(
            AipParseError::LengthMismatch {
                len: 32,
                count: 2,
                expected_len: 48
            }
            .to_string()
            .contains("2 points")
        );
    }

    #[test]
    fn the_shipped_fingerprints_are_internally_consistent() {
        for fingerprint in [&AIP_FINGERPRINT_BASE, &AIP_FINGERPRINT_DLC02] {
            assert!(fingerprint.entry_count > 0);
            assert!(fingerprint.point_count > fingerprint.entry_count);
            // Both shipped containers are overworld-only, which is why every block key in
            // them takes the BCD index path.
            assert!(BlockKey::from_parts(fingerprint.area, 0, 0, 0).uses_bcd_index());
        }
        assert_ne!(
            AIP_FINGERPRINT_BASE.entries_fnv1a64,
            AIP_FINGERPRINT_DLC02.entries_fnv1a64
        );
    }
}
