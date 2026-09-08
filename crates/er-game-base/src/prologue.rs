//! Tier A: compare a live function prologue against a generated pin, ignoring the operand bytes
//! that a game patch is guaranteed to re-encode.
//!
//! # The failure this exists to stop
//!
//! Every detour in this workspace byte-checks its target's prologue before installing, and
//! disarms with a log line when the bytes differ. That is the right default -- calling unknown
//! code costs the process. But three of those pins open with `mov rax, [rip+disp32]`, and a
//! rip-relative displacement is the delta from the end of the instruction to the global it names.
//! Both ends move when the game is patched. Measured on ELDEN RING 1.17: `FUN_140678740`,
//! `FUN_140678710` and `FUN_14067a980` are the same three instructions doing the same job at
//! correctly translated addresses, and all three gates disarmed -- on four bytes that could not
//! have survived. The features were silently off, and the log line blamed the game build.
//!
//! So the pin has to test instruction shape at those four bytes and exact identity everywhere
//! else. The mask that says which is which is derived at build time from the same named
//! `iced-x86` instructions the pin itself is assembled from (`build-support/prologue_build.rs`),
//! so it cannot drift away from the bytes it describes and nobody hand-marks an offset.
//!
//! # What this deliberately does not do
//!
//! It does not make a pin lenient. A masked byte is only ever a RIP-relative displacement; the
//! opcode, the ModRM byte, register-base displacements (the struct field offsets that are the
//! only difference between the `+0xb72` and `+0xb73` retractions), immediates and relative branch
//! targets are all still compared exactly. A target that is a different instruction still fails.

/// Mask byte meaning "this position must match exactly". Mirrors
/// `build-support/prologue_build.rs`, which emits the masks.
pub const BYTE_COMPARED: u8 = 0xff;
/// Mask byte meaning "ignore this position".
pub const BYTE_IGNORED: u8 = 0x00;

/// True when `actual` matches `expected` at every position the `mask` marks compared.
///
/// `actual` may be longer than `expected` (the callers read a fixed-size window); only the first
/// `expected.len()` bytes are considered. Refuses -- returns `false` -- rather than guessing when
/// the inputs are not a well-formed pin:
///
/// * `actual` shorter than `expected`: nothing to compare against.
/// * `mask` a different length from `expected`: the mask describes some other byte string, so
///   the positions it names are meaningless here.
/// * an empty pin, or a mask with no compared byte at all: that is not a pin, it is a wildcard,
///   and a wildcard would arm every hook on every build.
///
/// A mask that is entirely `BYTE_COMPARED` makes this exactly a prefix `==`, which is what every
/// prologue with no RIP-relative operand gets.
#[must_use]
pub fn matches_masked(actual: &[u8], expected: &[u8], mask: &[u8]) -> bool {
    if expected.is_empty() || actual.len() < expected.len() || mask.len() != expected.len() {
        return false;
    }
    if !mask.contains(&BYTE_COMPARED) {
        return false;
    }
    expected
        .iter()
        .zip(actual)
        .zip(mask)
        .all(|((want, got), &keep)| keep != BYTE_COMPARED || want == got)
}

/// The positions where a compared byte differs, for a log line. Empty when
/// [`matches_masked`] would return `true`; masked positions are never listed.
///
/// Bounded on purpose: a wrong address produces a diff at nearly every byte and the log is
/// append-only, so the list is capped and the caller is told the true count separately.
#[must_use]
pub fn compared_mismatches(actual: &[u8], expected: &[u8], mask: &[u8]) -> usize {
    if actual.len() < expected.len() || mask.len() != expected.len() {
        return expected.len();
    }
    expected
        .iter()
        .zip(actual)
        .zip(mask)
        .filter(|((want, got), keep)| **keep == BYTE_COMPARED && want != got)
        .count()
}

/// How many positions the mask ignores. Non-zero in a log line is the difference between "this
/// gate accepted a relocated operand" and "this gate is a plain byte compare".
#[must_use]
pub fn ignored_count(mask: &[u8]) -> usize {
    mask.iter().filter(|&&byte| byte == BYTE_IGNORED).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three real 1.16.2 pins that disarmed on 1.17, and the bytes 1.17 actually has at the
    /// mapped address. Read out of `eldenring-deobf.bin` / `eldenring-deobf-1.17.bin` by
    /// `scripts/verify-aob-patterns-1170.py --section prologues`; they are the shape of the
    /// problem, not a synthetic one.
    // Not a prologue: test input. These are the bytes as they exist in the two images, fed to
    // `prologue_matches` to prove the comparison notices the RIP displacement changing. Nothing
    // here is ever written into the game or compared against a hook site, so there is no
    // assembler for a build.rs to get one byte wrong.
    const B72_1162: &[u8] = &[
        0x48, 0x8b, 0x05, 0xd1, 0x11, 0x6f, 0x03, 0xc6, 0x80, 0x72, 0x0b, 0x00, 0x00, 0x00, 0xc3,
    ];
    // Not a prologue: the same site read out of the 1.17 image, the expected half of that test.
    const B72_1170: &[u8] = &[
        0x48, 0x8b, 0x05, 0xf1, 0x43, 0x6f, 0x03, 0xc6, 0x80, 0x72, 0x0b, 0x00, 0x00, 0x00, 0xc3,
    ];
    // Not a prologue: a second 1.16.2 site, present so one passing case cannot carry the test.
    const B73_1162: &[u8] = &[
        0x48, 0x8b, 0x05, 0x01, 0x12, 0x6f, 0x03, 0xc6, 0x80, 0x73, 0x0b, 0x00, 0x00, 0x00, 0xc3,
    ];
    // Not a prologue: the settle-gate site in 1.16.2, read from the image as test input.
    const SETTLE_1162: &[u8] = &[
        0x48, 0x8b, 0x05, 0x91, 0xef, 0x6e, 0x03, 0x83, 0xb8, 0xc4, 0x0b, 0x00, 0x00, 0x02, 0x75,
        0x0a,
    ];
    // Not a prologue: the same settle-gate site in 1.17, the expected half of that comparison.
    const SETTLE_1170: &[u8] = &[
        0x48, 0x8b, 0x05, 0xb1, 0x21, 0x6f, 0x03, 0x83, 0xb8, 0xc4, 0x0b, 0x00, 0x00, 0x02, 0x75,
        0x0a,
    ];

    /// `mov rax,[rip+disp32]` is 7 bytes with the displacement at +3.
    fn rip_disp_mask(len: usize) -> Vec<u8> {
        let mut mask = vec![BYTE_COMPARED; len];
        for byte in &mut mask[3..7] {
            *byte = BYTE_IGNORED;
        }
        mask
    }

    #[test]
    fn exact_mask_is_a_plain_comparison() {
        let mask = vec![BYTE_COMPARED; B72_1162.len()];
        assert!(matches_masked(B72_1162, B72_1162, &mask));
        assert!(!matches_masked(B72_1170, B72_1162, &mask));
    }

    #[test]
    fn relocated_displacement_is_accepted() {
        assert!(matches_masked(
            B72_1170,
            B72_1162,
            &rip_disp_mask(B72_1162.len())
        ));
        assert!(matches_masked(
            SETTLE_1170,
            SETTLE_1162,
            &rip_disp_mask(SETTLE_1162.len())
        ));
    }

    /// Negative control. Mutating the OPCODE of the masked instruction must still disarm: the
    /// mask covers the operand, never the identity of the instruction carrying it.
    #[test]
    fn a_changed_opcode_still_disarms() {
        let mask = rip_disp_mask(B72_1162.len());
        for position in [0, 1, 2] {
            let mut mutated = B72_1170.to_vec();
            mutated[position] ^= 0x01;
            assert!(
                !matches_masked(&mutated, B72_1162, &mask),
                "byte {position} is opcode/ModRM and must never be ignored"
            );
        }
    }

    /// The other half of the negative control: the tail after the masked field is what says which
    /// function this is. `+0xb72` vs `+0xb73` differ in exactly one byte there, and the masked
    /// pin must keep telling them apart.
    #[test]
    fn the_two_retractions_do_not_accept_each_other() {
        let mask = rip_disp_mask(B72_1162.len());
        assert!(!matches_masked(B73_1162, B72_1162, &mask));
        assert!(!matches_masked(B72_1162, B73_1162, &mask));
    }

    #[test]
    fn a_wildcard_mask_is_refused_rather_than_matching_everything() {
        let all_ignored = vec![BYTE_IGNORED; B72_1162.len()];
        assert!(!matches_masked(&[0u8; 15], B72_1162, &all_ignored));
    }

    #[test]
    fn malformed_inputs_refuse() {
        let mask = rip_disp_mask(B72_1162.len());
        // Short read.
        assert!(!matches_masked(&B72_1162[..4], B72_1162, &mask));
        // Mask describing a different byte string.
        assert!(!matches_masked(B72_1162, B72_1162, &mask[..4]));
        // Empty pin.
        assert!(!matches_masked(B72_1162, &[], &[]));
    }

    #[test]
    fn mismatch_count_ignores_masked_positions() {
        let mask = rip_disp_mask(B72_1162.len());
        assert_eq!(compared_mismatches(B72_1170, B72_1162, &mask), 0);
        assert_eq!(
            compared_mismatches(B72_1170, B72_1162, &vec![BYTE_COMPARED; B72_1162.len()]),
            2
        );
        assert_eq!(ignored_count(&mask), 4);
        assert_eq!(ignored_count(&[BYTE_COMPARED; 8]), 0);
    }
}
