//! Shared FNV-1a 64-bit hashing primitives.
//!
//! These fingerprints are for cheap content identity and change detection, not cryptography.

/// FNV-1a 64-bit offset basis.
pub const FNV1A64_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
pub const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Mix one integer value into an FNV-1a state.
///
/// Standard byte hashing calls this with a zero-extended byte. Runtime change detectors also use
/// it with wider integer fields while retaining their existing one-multiply-per-field semantics.
#[inline]
#[must_use]
pub const fn fnv1a64_mix(hash: u64, value: u64) -> u64 {
    (hash ^ value).wrapping_mul(FNV1A64_PRIME)
}

/// Continue an FNV-1a hash from an explicit starting state.
#[inline]
#[must_use]
pub const fn fnv1a64_extend(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut index = 0;
    while index < bytes.len() {
        hash = fnv1a64_mix(hash, bytes[index] as u64);
        index += 1;
    }
    hash
}

/// Hash a byte slice with FNV-1a 64-bit.
#[inline]
#[must_use]
pub const fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_extend(FNV1A64_OFFSET_BASIS, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_canonical_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
        assert_eq!(fnv1a64(b"hello"), 0xa430_d846_80aa_bd0b);
    }

    #[test]
    fn whole_incremental_and_per_byte_paths_are_equivalent() {
        for bytes in [
            &b""[..],
            &b"a"[..],
            &b"foobar"[..],
            &[0, 1, 2, 3, 0xfe, 0xff][..],
        ] {
            let split = bytes.len() / 2;
            let incremental = fnv1a64_extend(
                fnv1a64_extend(FNV1A64_OFFSET_BASIS, &bytes[..split]),
                &bytes[split..],
            );
            let per_byte = bytes.iter().fold(FNV1A64_OFFSET_BASIS, |hash, byte| {
                fnv1a64_mix(hash, u64::from(*byte))
            });
            assert_eq!(fnv1a64(bytes), incremental);
            assert_eq!(fnv1a64(bytes), per_byte);
        }
    }

    #[test]
    fn explicit_seed_preserves_seeded_pass_semantics() {
        let seed = 0x9e37_79b9_7f4a_7c15;
        let bytes = b"pool material";
        let split = 4;
        assert_eq!(
            fnv1a64_extend(seed, bytes),
            fnv1a64_extend(fnv1a64_extend(seed, &bytes[..split]), &bytes[split..])
        );
    }
}
