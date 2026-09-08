//! The two STRUCTS the HUD RETARGET touches, and the build gate in front of them.
//!
//! # Why this is a second offset table and not more rows in `possess::layout`
//!
//! `possess::layout` describes what the possession engine writes: `ChrCtrl`, the manipulator
//! override, the neuter flags. This describes what the HUD post-pass reads and where it puts the
//! answer. They are separate questions with separate evidence, and the one field they have in
//! common -- `CSChrDataModule.hp` -- is asserted equal to its counterpart there by a test below,
//! so the duplication cannot drift into a disagreement without failing.
//!
//! # The offsets did not move between the two builds, and that is measured rather than assumed
//!
//! Every constant here was read out of both `eldenring-deobf.bin` (1.16.2) and
//! `eldenring-deobf-1.17.bin` (1.17) at the corresponding site inside
//! `CSFeManImp::UpdatePlayerComponents`, and the two disassemblies agree instruction for
//! instruction across the whole vitals span -- 1.16.2 `0x140772b83..c4d`, 1.17
//! `0x140773a03..acd`. A second, independent witness agrees:
//! `scripts/re117_module_field_scan.py` sweeps every `componentContainer+0x190` ->
//! `module+0x00` -> `[module+disp]` chain in both flat images and finds the same six
//! data-module offsets in both with identical site counts -- `+0x138` 47/47, `+0x13c` 15/15,
//! `+0x148` 11/11, `+0x14c` 11/11, `+0x154` 11/11, `+0x158` 8/8.
//!
//! Read that for what it is: corroboration that the layout did not shift, not proof of any
//! individual site. Identical counts would also survive a renumbering that happened to preserve
//! them. The per-site proof is the disassembly of the vitals span above, plus the compile-time
//! `offset_of!` cross-check in `crate::hud::detour` against `fromsoftware-rs`'s separately
//! derived `CSChrDataModule`.
//!
//! **That is not a licence to skip the gate.** `ChrIns` grew eight bytes at `+0x3b8` on 1.17 with
//! its size unchanged, so a stale offset on this game reads the neighbouring field instead of
//! faulting -- there is no crash to notice. [`Layout::for_build`] therefore answers `None` on any
//! build nobody has measured, and the detour is not installed at all rather than installed with
//! offsets whose provenance is "they were the same last time".
//!
//! # Provenance of the names
//!
//! Not inferred from position. Both structures are curated in the 1.16.2 named Ghidra dump and
//! every field below was taken from `getStructure` by NAME:
//! `FrontEndViewValues` (`playerHp`, `maxRecoverableHp`, `hpMax`, `hpMaxUncapped`, `fp`, `fpMax`,
//! `stamina`, `staminaMax`) and `CSChrDataModule` (`hp`, `hpMax`, `hpMaxUncapped`, `fp`, `fpMax`,
//! `stamina`, `staminaMax`, `recoverableHpLeft`). `CSChrDataModule`'s names independently agree
//! with `fromsoftware-rs`'s model of the same struct, which is asserted in `hud::detour`.

// Pure arithmetic and constants; ungated so the tests below run on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use er_game_base::game_build::{FileVersion, SUPPORTED_FILE_VERSION};

use crate::possess::layout::FILE_VERSION_1170;

/// `CSFeManImp+0x80`, where the `FrontEndViewValues` sub-object begins.
///
/// Test-only, and deliberately not a term in any calculation: every offset in [`ViewOffsets`] is
/// quoted from the game's own disassembly as `[rsi+0xNN]` with `rsi` the `CSFeManImp`, so nothing
/// adds this to anything. It exists so a reader checking those numbers against the curated
/// `FrontEndViewValues` struct -- where the same fields are named at `0xNN - 0x80` -- can
/// reconcile the two numbering schemes, and so the test below can assert the offsets really do
/// land inside that sub-object rather than merely somewhere in the manager.
#[cfg(test)]
const FRONT_END_VIEW_VALUES: usize = 0x80;

/// What the HUD reads, as offsets from the `CSFeManImp`.
///
/// # These eight are the whole claim
///
/// `UpdatePlayerComponents` writes far more than eight fields; the post-pass overwrites only
/// these, which is precisely why runes, equipment, the great rune and the spell slots keep
/// reading the real player. Those come from `PlayerGameData` (vtable `+0x168`) and
/// `GetWeaponGaitemHandleBySlot` (vtable `+0x230`) and are never touched here.
///
/// # `HP_MAX` is not the maximum
///
/// The trap in this struct, and the one a reader will otherwise assume away. The game computes
/// `view.hpMax = data.hpMaxUncapped - data.hpMax`, so the field named `hpMax` holds the
/// difference -- the amount of maximum HP currently lost, i.e. the darkened right-hand segment of
/// the bar. The bar's full width is [`ViewOffsets::hp_max_uncapped`]. For a character with no
/// max-HP reduction the two source fields are equal and this field is zero, which is what looks
/// like a max that happens to be broken. See [`crate::hud::vitals::Source::view`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewOffsets {
    /// `FrontEndViewValues.playerHp`, `int`. Current HP.
    pub(crate) player_hp: usize,
    /// `maxRecoverableHp`, `int`. The far end of the rally segment.
    pub(crate) max_recoverable_hp: usize,
    /// `hpMax`, `int` -- the lost max, not the max. See the struct docs.
    pub(crate) hp_max: usize,
    /// `hpMaxUncapped`, `int`. The bar's full width.
    pub(crate) hp_max_uncapped: usize,
    /// `fp`, `int`.
    pub(crate) fp: usize,
    /// `fpMax`, `int`.
    pub(crate) fp_max: usize,
    /// `stamina`, `int`.
    pub(crate) stamina: usize,
    /// `staminaMax`, `int`.
    pub(crate) stamina_max: usize,
}

/// Where the values come from, as offsets from the `CSChrDataModule`.
///
/// The module is reached the same way the game reaches it: `ChrIns+0x190` `componentContainer`,
/// then the container's first slot (`+0x00`). This is the generic data module every `ChrIns` has,
/// which is the entire reason a creature can feed the player's HUD at all -- nothing here is
/// `PlayerIns`-shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceOffsets {
    /// `CSChrDataModule.hp`.
    pub(crate) hp: usize,
    /// `hpMax`. The effective maximum, i.e. `GetHpRate`'s denominator.
    pub(crate) hp_max: usize,
    /// `hpMaxUncapped`. Populated for NPCs: `FUN_140438a50` writes it from `NpcParam.hp`, the
    /// same value it gives `hpBase`.
    pub(crate) hp_max_uncapped: usize,
    /// `recoverableHpLeft`, a `f32`. The rally pool.
    pub(crate) recoverable_hp: usize,
    /// `fp`.
    pub(crate) fp: usize,
    /// `fpMax`. **Zero on essentially every creature**; see [`crate::hud::vitals`].
    pub(crate) fp_max: usize,
    /// `stamina`.
    pub(crate) stamina: usize,
    /// `staminaMax`.
    pub(crate) stamina_max: usize,
}

/// Both halves, for one build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) view: ViewOffsets,
    pub(crate) source: SourceOffsets,
}

/// The offsets as measured on 1.16.2 and on 1.17.
///
/// One constant shared by both arms of [`Layout::for_build`], on purpose. Writing it twice would
/// invite the two copies to drift and would imply the builds were measured to differ, which they
/// were not -- they were measured to agree, and a single shared value says that where two equal
/// literals would merely repeat it.
pub(crate) const MEASURED: Layout = Layout {
    view: ViewOffsets {
        player_hp: 0x84,
        max_recoverable_hp: 0x88,
        hp_max: 0x8c,
        hp_max_uncapped: 0x90,
        fp: 0x98,
        fp_max: 0xa4,
        stamina: 0xac,
        stamina_max: 0xb8,
    },
    source: SourceOffsets {
        hp: 0x138,
        hp_max: 0x13c,
        hp_max_uncapped: 0x140,
        recoverable_hp: 0x160,
        fp: 0x148,
        fp_max: 0x14c,
        stamina: 0x154,
        stamina_max: 0x158,
    },
};

impl Layout {
    /// The layout for the running build, or `None` when nobody has measured it.
    ///
    /// `None` is what stops the detour being installed. It is the honest answer and the only safe
    /// one: on 1.17 a wrong `ChrIns`-relative offset lands on a live neighbouring field rather
    /// than on unmapped memory, so a guess here would not crash, it would quietly drive the
    /// player's HP bar from whatever happens to sit eight bytes along.
    #[must_use]
    pub(crate) fn for_build(version: Option<FileVersion>) -> Option<Self> {
        match version? {
            v if v == SUPPORTED_FILE_VERSION => Some(MEASURED),
            v if v == FILE_VERSION_1170 => Some(MEASURED),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate: two known builds, and `None` for everything else including the host.
    #[test]
    fn the_layout_is_known_for_both_supported_builds_and_nothing_else() {
        assert_eq!(
            Layout::for_build(Some(SUPPORTED_FILE_VERSION)),
            Some(MEASURED)
        );
        assert_eq!(Layout::for_build(Some(FILE_VERSION_1170)), Some(MEASURED));
        assert_eq!(
            Layout::for_build(Some(FileVersion {
                major: 2,
                minor: 8,
                build: 0,
                revision: 0,
            })),
            None,
            "an unmeasured build must refuse, not reuse"
        );
        assert_eq!(Layout::for_build(None), None, "the host has no game image");
    }

    /// The cross-check against the possession engine's own table. `CSChrDataModule.hp` is the one
    /// field both modules name, and two tables disagreeing about it would mean one of them is
    /// reading a different struct than it thinks.
    #[test]
    fn the_shared_data_module_offset_agrees_with_the_possession_table() {
        assert_eq!(
            MEASURED.source.hp,
            crate::possess::layout::chr_data_module::HP
        );
    }

    /// Every view field must be a distinct 4-byte slot inside `FrontEndViewValues`, and none may
    /// alias another. A transposed pair here would write stamina into the FP bar -- visible, but
    /// only to someone playing, and only after a build.
    #[test]
    fn the_eight_view_offsets_are_distinct_and_do_not_overlap() {
        let all = [
            MEASURED.view.player_hp,
            MEASURED.view.max_recoverable_hp,
            MEASURED.view.hp_max,
            MEASURED.view.hp_max_uncapped,
            MEASURED.view.fp,
            MEASURED.view.fp_max,
            MEASURED.view.stamina,
            MEASURED.view.stamina_max,
        ];
        let mut sorted = all;
        sorted.sort_unstable();
        for pair in sorted.windows(2) {
            assert!(
                pair[1] - pair[0] >= 4,
                "{:#x} and {:#x} overlap as 4-byte ints",
                pair[0],
                pair[1]
            );
        }
        // ...and all of them sit inside the sub-object the struct docs name, past its 4-byte
        // leading flag.
        for offset in all {
            assert!(
                offset > FRONT_END_VIEW_VALUES,
                "{offset:#x} is not inside FrontEndViewValues"
            );
        }
    }

    /// Same for the source side, and additionally that the vitals are the one contiguous
    /// `hp/hpMax/hpMaxUncapped ... fp/fpMax ... stamina/staminaMax` block the dump describes:
    /// each triple ascends by exactly four bytes.
    #[test]
    fn the_source_offsets_form_the_contiguous_vitals_block() {
        assert_eq!(MEASURED.source.hp_max - MEASURED.source.hp, 4);
        assert_eq!(MEASURED.source.hp_max_uncapped - MEASURED.source.hp_max, 4);
        assert_eq!(MEASURED.source.fp_max - MEASURED.source.fp, 4);
        assert_eq!(MEASURED.source.stamina_max - MEASURED.source.stamina, 4);
        // `hpBase` sits between hpMaxUncapped and fp, and `fpBase` between fpMax and stamina;
        // both are skipped, so those gaps are 8 rather than 4.
        assert_eq!(MEASURED.source.fp - MEASURED.source.hp_max_uncapped, 8);
        assert_eq!(MEASURED.source.stamina - MEASURED.source.fp_max, 8);
    }

    /// The rally source is the only float among the reads. Pinning it separately is cheap and
    /// stops a future edit from folding it into the int list, where it would be read as garbage.
    #[test]
    fn the_rally_source_sits_past_the_integer_block() {
        // A `const` block rather than a plain `assert!`: both sides are constants, so this is
        // decidable at compile time and clippy's `assertions_on_constants` rightly refuses to let
        // it masquerade as a runtime check. Promoting it means a bad edit fails the build rather
        // than one test run.
        const {
            assert!(MEASURED.source.recoverable_hp > MEASURED.source.stamina_max);
            // `staminaBase` is the field between them.
            assert!(MEASURED.source.recoverable_hp - MEASURED.source.stamina_max == 8);
        }
    }
}
