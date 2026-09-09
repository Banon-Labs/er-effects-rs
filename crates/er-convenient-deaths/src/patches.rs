//! The patch registry.
//!
//! Each entry rewrites exactly one byte of the game's own code to make dying cost less. Unlike
//! the crash patches in `er-seamless-bugfixes`, these are deliberate behaviour changes -- the
//! game is not wrong before they are applied -- so each one is off until its config key says
//! otherwise, and each carries its own `effect` line saying what the player will notice.
//!
//! # What was ported, and what was not
//!
//! The upstream mod is `github chozandrias76/er-convenient-deaths`. Its three offsets are dead:
//! measured 2026-09-09, none of `0x5fc1a0`, `0x25e299` or `0x5a7193` holds its expected bytes in
//! `eldenring-deobf.bin` (1.16.2) or `eldenring-deobf-1.17.1.bin`. They were re-measured here
//! from the functions themselves rather than translated, because a translated offset that lands
//! one instruction off is indistinguishable from a correct one until it runs.
//!
//! Upstream also wrote three, nine and two bytes. Only one byte in each actually changes; the
//! rest was padding written back over itself. Writing the padding widens the window in which
//! another thread can execute a half-updated instruction, for no gain, so only the byte that
//! differs is written.
//!
//! # Rules a fourth patch must satisfy
//!
//! 1. **The behaviour it selects must already be reachable.** Not "probably harmless": the
//!    function has to already contain the path being forced, and the window has to contain the
//!    evidence. All three below meet this -- two force an existing branch and the third raises an
//!    immediate the game itself writes whenever a rune arc is applied.
//! 2. **Pin the address by a window, not by the byte.** `0x74` occurs everywhere. The window is
//!    generated from named `iced-x86` instructions by `build.rs` and ground-truthed against
//!    `eldenring-deobf.bin` there, then re-checked against the live image before anything is
//!    written.
//! 3. **Write one byte.** The window is live code other threads may be executing.
//! 4. **Read it back.** A successful `VirtualProtect` is not proof the byte landed; another mod
//!    can own the same address.
//! 5. **Name the function RVA in a `const *_RVA`.** `scripts/select-needed-1170-rows.py` scans
//!    `crates/` for that spelling to decide which rows of the 1.16.2 -> 1.17 map get compiled in.
//!    A patch whose function is not in the map is refused at runtime, not mis-applied -- but it
//!    is refused every time, so the constant is what makes it work at all.

use core::sync::atomic::AtomicBool;

include!(concat!(
    env!("OUT_DIR"),
    "/generated_convenient_death_windows.rs"
));

// The three functions' addresses, and every offset within them, come from one file that
// `build.rs` includes too -- so the bytes a window is compared against and the address it is
// read at cannot drift apart. See `patch_sites.rs`.
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/patch_sites.rs"));

/// `RET`. What the bloodstain writer's first byte becomes.
const RET: u8 = 0xc3;
/// `JMP rel8`. What the fade-out lookup's `JZ rel8` becomes.
const JMP_SHORT: u8 = 0xeb;
/// The immediate the rune-arc store writes once patched: arc still active, trigger still cleared.
const RUNE_ARC_ACTIVE: u8 = 0x01;

static KEEP_RUNES_APPLIED: AtomicBool = AtomicBool::new(false);
static KEEP_RUNE_ARC_APPLIED: AtomicBool = AtomicBool::new(false);
static INSTANT_FADE_OUT_APPLIED: AtomicBool = AtomicBool::new(false);

/// One rewritten instruction byte.
pub(crate) struct Patch {
    /// Name used in the install log.
    pub(crate) name: &'static str,
    /// Key in the DLL-adjacent config that turns this patch on. Off when absent.
    pub(crate) config_key: &'static str,
    /// 1.16.2 RVA of the `.pdata` function entry that contains the window.
    ///
    /// The window's own address is deliberately not stored. A patch site is by nature
    /// mid-function, and the 1.16.2 -> 1.17 maps are keyed on function starts, so a mid-function
    /// address is structurally unmappable and must be refused rather than translated. What is
    /// mappable is the enclosing function; the offset rides along in Rust. See
    /// `er_game_base::game_build::resolve_call_site_rva`.
    pub(crate) function_rva: usize,
    /// Byte offset of the verified window's first byte within [`Self::function_rva`].
    pub(crate) offset_in_function: usize,
    /// Bytes the window must hold before anything is written. Generated and ground-truthed by
    /// `build.rs`; a mismatch abandons this patch and logs the bytes actually found.
    pub(crate) expected_window: &'static [u8],
    /// Index into the window of the single byte this patch rewrites.
    /// `expected_window[offset]` is therefore the value being replaced, which is why no separate
    /// "from" field exists.
    pub(crate) offset: usize,
    /// What that byte becomes.
    pub(crate) replacement: u8,
    /// Set once the byte has been written and read back as [`Self::replacement`].
    pub(crate) applied: &'static AtomicBool,
    /// What the player will notice. Printed at install time.
    pub(crate) effect: &'static str,
    /// Why forcing this behaviour is sound, printed beside the effect so a reader of the log does
    /// not have to open this source to judge it.
    pub(crate) rationale: &'static str,
}

impl Patch {
    /// 1.16.2 RVA of the verified window's first byte.
    ///
    /// Derived from the two stored halves rather than kept beside them, so no third field can
    /// drift out of agreement with the address the window was generated at.
    pub(crate) const fn rva(&self) -> usize {
        self.function_rva + self.offset_in_function
    }

    /// Absolute address of the byte this patch rewrites, given the window's address on the
    /// running build.
    ///
    /// It takes the resolved window rather than the module base on purpose: `base + rva` is only
    /// the window's address on the build these RVAs came from, so a target derived from the base
    /// would name a different byte than the one the install path just verified.
    pub(crate) fn target(&self, window: usize) -> usize {
        window + self.offset
    }

    /// The byte being replaced, read out of the window so the two can never disagree.
    pub(crate) fn replaced(&self) -> Option<u8> {
        self.expected_window.get(self.offset).copied()
    }
}

/// Every patch this DLL can apply.
pub(crate) static REGISTRY: &[Patch] = &[
    Patch {
        name: "keep-runes-on-death",
        config_key: "keep_runes_on_death",
        function_rva: BLOODSTAIN_WRITER_RVA,
        offset_in_function: BLOODSTAIN_WRITER_WINDOW_OFFSET,
        expected_window: KEEP_RUNES_WINDOW,
        offset: 0,
        replacement: RET,
        applied: &KEEP_RUNES_APPLIED,
        effect: "dying costs no runes, and drops no bloodstain to walk back to",
        rationale: "the bloodstain writer's first instruction is its own `param_2 == null` test, \
                    whose `JZ` lands on the function's `RET`. Returning immediately is therefore \
                    a path the game already takes, byte for byte, whenever it is handed no \
                    player. It is one function: the same body copies the rune count into the \
                    bloodstain and then zeroes it, so skipping it keeps the runes and leaves no \
                    bloodstain -- there is no state where the runes are in neither place",
    },
    Patch {
        name: "keep-rune-arc-on-death",
        config_key: "keep_rune_arc_on_death",
        function_rva: RUNE_ARC_CLEAR_RVA,
        offset_in_function: RUNE_ARC_CLEAR_WINDOW_OFFSET,
        expected_window: KEEP_RUNE_ARC_WINDOW,
        offset: 7,
        replacement: RUNE_ARC_ACTIVE,
        applied: &KEEP_RUNE_ARC_APPLIED,
        effect: "an active rune arc survives death instead of being consumed by it",
        rationale: "the store is 16-bit and covers two adjacent fields at once -- `runeArcActive` \
                    at 0xff and the trigger flag at 0x100 -- so the instruction cannot simply be \
                    removed: its caller re-enters this function until the trigger is cleared. \
                    Raising the immediate's low byte writes the state the game itself writes \
                    whenever an arc is applied, while still clearing the trigger",
    },
    Patch {
        name: "instant-death-fade-out",
        config_key: "quicker_deaths",
        function_rva: SOLO_PLAY_DEATH_RVA,
        offset_in_function: SOLO_PLAY_DEATH_WINDOW_OFFSET,
        expected_window: INSTANT_FADE_OUT_WINDOW,
        offset: 6,
        replacement: JMP_SHORT,
        applied: &INSTANT_FADE_OUT_APPLIED,
        effect: "the YOU DIED fade-out ends at once instead of holding for its param'd duration",
        rationale: "the `JZ` is the null check on the `MenuCommonParam` row. Its taken arm sets \
                    the fade time to 0.0 and its untaken arm loads \
                    `soloPlayDeath_ToFadeOutTime`; both converge on the same `OnKeyTime2` call. \
                    Forcing the jump selects the 0.0 the game already uses when that row is \
                    missing, so no value the game did not write reaches the timer",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::Ordering;

    #[test]
    fn every_patch_is_documented_and_pinned() {
        for patch in REGISTRY {
            assert!(!patch.name.is_empty());
            assert!(
                !patch.config_key.is_empty(),
                "{}: needs a config key",
                patch.name
            );
            assert!(
                !patch.effect.is_empty(),
                "{}: a patch must say what the player will notice",
                patch.name
            );
            assert!(
                !patch.rationale.is_empty(),
                "{}: a patch must say why forcing this behaviour is sound",
                patch.name
            );
            assert!(patch.rva() != 0);
            assert!(
                !patch.expected_window.is_empty(),
                "{}: a patch must pin its address by a window, not by the byte",
                patch.name
            );
            assert!(
                patch.offset < patch.expected_window.len(),
                "{}: offset {} is outside the {}-byte window it indexes",
                patch.name,
                patch.offset,
                patch.expected_window.len()
            );
        }
    }

    #[test]
    fn every_patch_actually_changes_its_byte() {
        for patch in REGISTRY {
            assert_ne!(
                patch.replaced(),
                Some(patch.replacement),
                "{}: the replacement equals the byte already there, so this patch is a no-op that \
                 would still report itself applied",
                patch.name
            );
        }
    }

    #[test]
    fn patches_start_unapplied() {
        for patch in REGISTRY {
            assert!(
                !patch.applied.load(Ordering::Relaxed),
                "{}: applied must start false so a true value means the byte really changed",
                patch.name
            );
        }
    }

    /// Two patches sharing one `applied` flag would report each other's success.
    #[test]
    fn no_two_patches_share_a_flag_or_a_key() {
        for (index, patch) in REGISTRY.iter().enumerate() {
            for other in &REGISTRY[index + 1..] {
                assert!(
                    !core::ptr::eq(patch.applied, other.applied),
                    "{} and {} share one applied flag",
                    patch.name,
                    other.name
                );
                assert_ne!(
                    patch.config_key, other.config_key,
                    "{} and {} share one config key",
                    patch.name, other.name
                );
                assert_ne!(
                    patch.rva(),
                    other.rva(),
                    "{} and {} patch the same address",
                    patch.name,
                    other.name
                );
            }
        }
    }

    /// The address written is derived, not stored, so the derivation is what a wrong patch would
    /// get wrong.
    #[test]
    fn target_is_the_window_start_plus_the_offset() {
        for patch in REGISTRY {
            let window = (GAME_IMAGE_BASE as usize) + patch.rva();
            assert_eq!(patch.target(window), window + patch.offset);
            assert!(patch.target(window) >= window);
        }
    }

    /// `target` must follow the window wherever the running build put it. This is the property
    /// the signature exists for: a target derived from the module base would be verified in one
    /// place and written in another on any build that moved this code.
    #[test]
    fn target_follows_a_window_the_running_build_moved() {
        const MOVED_BY: usize = 0x16d0;
        for patch in REGISTRY {
            let moved = (GAME_IMAGE_BASE as usize) + patch.rva() + MOVED_BY;
            assert_eq!(patch.target(moved), moved + patch.offset);
            assert_ne!(
                patch.target(moved),
                (GAME_IMAGE_BASE as usize) + patch.rva() + patch.offset
            );
        }
    }
}
