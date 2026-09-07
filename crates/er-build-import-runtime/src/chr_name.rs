//! Adopting the imported build's name as the character's name.
//!
//! # What "inherit the name" can and cannot mean in this game
//!
//! Elden Ring has no rename UI. A character is named once, at creation, and the name then lives in
//! three places inside `PlayerGameData` plus two places on disk. The complete map, established by
//! static RE of the 1.16.2 image on 2026-08-30 and carried to 1.17 through
//! `docs/recon/rva-map-1162-to-1170.needed-verified.tsv`:
//!
//! | where | who reads it | how it is written |
//! |---|---|---|
//! | `PGD+0x9c`, `wchar_t[17]` | serialized into the save; this repo's own stats/portrait readers | `CopyChrName`'s copy loop |
//! | `PGD+0x8e8`, `CSWordCheckedStringInternal*` | `FUN_14025f8e0`, and through it `CS::ProfileSummary`'s slot update (`0x140262270`) and `CS::GetPlayerChrName` (the overhead nameplate) | refreshed by `CopyChrName` from `+0x9c` |
//! | `PGD+0x8f8`, a second such string | main-player variant, word-check flag set | refreshed by `CopyChrName` from `+0x9c` |
//! | `CS::ProfileSummary` record `+0x00`, `0x22` bytes | the save-slot list, the title Continue row, the loading-screen portrait | `wcsncpy`'d out of `FUN_14025f8e0` by the game, at every save |
//! | `.sl2` slot body `+0x94`, and `USER_DATA010` record `+0x02` | a container's slot list, including a foreign one | serialized by the game, at every save |
//!
//! Only the first three are ours to touch, and only through the one native below. The last two are
//! the game's to write, and it writes them out of the first three -- which is exactly why this
//! module does not go near them. Storing a name straight into a `ProfileSummary` record is the
//! mistake that nearly persisted the save picker's `[ new ]` row label into a user's container.
//!
//! # So what actually sticks
//!
//! Every surface agrees the moment the name is applied, except the two on-disk ones, which agree at
//! the next save the game performs. Under the product DLL that save is the System>Quit **Save
//! Game** row and nothing else (`er-save-suppress` swallows the rest), so a player who renames and
//! quits without saving comes back to the old name -- correctly, because nothing was saved.

use crate::read_character::{pgd, read_character_name};
use er_build_import_core::chr_name::{CHR_NAME_MAX_UNITS, clamp_to_field};
use er_game_base::rva::PLAYER_GAME_DATA_COPY_CHR_NAME_RVA;

/// `CS::PlayerGameData::CopyChrName(PlayerGameData*, const wchar_t*)`. RCX/RDX, no return.
type CopyChrNameFn = unsafe extern "system" fn(usize, *const u16);

// The clamp lives in `er-build-import-core` because it is pure and testable; the field it clamps to
// is here, bound to the upstream `PlayerGameData` layout. Pinning them to each other is what stops
// the core's plain `16` from becoming a number nobody re-checks: if the name window ever moves or
// resizes, this fails the build instead of silently writing a name the native refuses.
const _: () = assert!(CHR_NAME_MAX_UNITS == pgd::NAME_LEN_U16 - 1);

/// What the name pass did. Every variant is reported, because "the build had no name", "the name
/// was applied" and "the native is missing on this build" are three different events that all leave
/// the character looking untouched if nothing says which one happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameOutcome {
    /// The name changed, and the read-back confirms it.
    Applied {
        /// What the character was called before.
        before: String,
        /// What it is called now -- the clamped form, not the build's raw name.
        after: String,
        /// Whether the build's name did not fit and was cut to `CHR_NAME_MAX_UNITS`.
        truncated: bool,
    },
    /// The character already has this name; nothing was written.
    AlreadyNamed(String),
    /// The build carries no name that survives cleaning, so the character keeps its own.
    NothingToAdopt,
    /// `CopyChrName` has no verified mapping for the running build. The character keeps its name.
    Refused,
    /// The call was made and the field does not hold what was asked for. Never reported as a
    /// success: this crate derives no result from a call having returned, and a name is the one
    /// field where a wrong answer is visible to other players.
    ReadBackDisagreed {
        /// What was asked for.
        wanted: String,
        /// What `PGD+0x9c` holds instead.
        got: String,
    },
}

impl NameOutcome {
    /// One log line's worth of what happened.
    pub fn label(&self) -> String {
        match self {
            NameOutcome::Applied {
                before,
                after,
                truncated,
            } => {
                let cut = if *truncated {
                    format!(
                        " (the build's name did not fit {CHR_NAME_MAX_UNITS} UTF-16 units and was cut)"
                    )
                } else {
                    String::new()
                };
                format!("{before:?} -> {after:?}{cut}")
            }
            NameOutcome::AlreadyNamed(name) => format!("already {name:?}, left alone"),
            NameOutcome::NothingToAdopt => {
                "the build names nothing usable, character keeps its name".to_owned()
            }
            NameOutcome::Refused => {
                "NOT APPLIED -- `CS::PlayerGameData::CopyChrName` has no verified mapping for the \
                 running build. The character keeps the name it had."
                    .to_owned()
            }
            NameOutcome::ReadBackDisagreed { wanted, got } => {
                format!("FAILED -- asked for {wanted:?}, PlayerGameData holds {got:?}")
            }
        }
    }

    /// The name the character ends up with, when this pass is what gave it one.
    pub fn adopted(&self) -> Option<&str> {
        match self {
            NameOutcome::Applied { after, .. } => Some(after.as_str()),
            _ => None,
        }
    }
}

/// Give the live character the build's name, as far as the game allows.
///
/// Goes through the native writer rather than storing into `PGD+0x9c`, because the raw array is
/// only one of the three places the name lives (see the module header) and the other two are what
/// the save-slot list and the overhead nameplate read. A `memcpy` would rename the HUD and leave
/// every other surface showing the old name.
///
/// The read-back is `PGD+0x9c`, and that is sufficient for all three: the native's copy loop is the
/// only thing that writes it, and the same function refreshes both string objects from it,
/// unconditionally, after the loop. So a correct `+0x9c` means the same bytes reached `+0x8e8` and
/// `+0x8f8`. It is also the discriminating read -- the copy loop is inside the `wcslen < 0x11`
/// branch, so a refused-for-length name is exactly the case where `+0x9c` still holds the old one.
///
/// # Safety
///
/// Game thread, character loaded, `pgd` a live `PlayerGameData*`.
pub unsafe fn adopt_build_name(module_base: usize, pgd: usize, build_name: &str) -> NameOutcome {
    let Some(clamped) = clamp_to_field(build_name) else {
        return NameOutcome::NothingToAdopt;
    };
    // Safety: the caller's contract.
    let before = unsafe { read_character_name(pgd) };
    if before == clamped.text {
        return NameOutcome::AlreadyNamed(before);
    }
    let Some(address) = crate::native::resolve(
        module_base,
        PLAYER_GAME_DATA_COPY_CHR_NAME_RVA,
        "CS::PlayerGameData::CopyChrName",
    ) else {
        return NameOutcome::Refused;
    };
    // Safety: the address was resolved for the running build immediately above.
    let copy_chr_name: CopyChrNameFn = unsafe { core::mem::transmute(address) };
    // Safety: `pgd` is live per the caller's contract, and `buffer` is a NUL-terminated UTF-16
    // string of at most CHR_NAME_MAX_UNITS text units -- the two things the native requires and
    // neither of which it checks. It borrows the buffer only for the duration of the call.
    unsafe { copy_chr_name(pgd, clamped.buffer.as_ptr()) };

    // Safety: as above.
    let after = unsafe { read_character_name(pgd) };
    if after == clamped.text {
        NameOutcome::Applied {
            before,
            after,
            truncated: clamped.truncated,
        }
    } else {
        NameOutcome::ReadBackDisagreed {
            wanted: clamped.text,
            got: after,
        }
    }
}
