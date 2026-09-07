//! The HUD RETARGET -- "the HP bar, stamina, etc, is tied to the spawned entity when it's in
//! control".
//!
//! Possession moves the camera, the body and the moveset onto a creature and leaves one thing
//! behind: the bars in the corner keep reading the character you walked in as. This layer fixes
//! that, and only that.
//!
//! # The shape of it, in three sentences
//!
//! `CSFeManImp::UpdatePlayerComponents` fills the HUD from the raw `WorldChrManImp+0x1e508`
//! main player, once a frame, and never consults the camera override that possession uses. So
//! [`detour`] hooks it, lets the original run **unchanged**, and then overwrites eight ints in
//! `CSFeManImp+0x80 FrontEndViewValues` from the possessed creature's generic `CSChrDataModule`.
//! Everything the original filled in that a creature could not supply -- runes, equipment, the
//! great rune, the spell slots -- is left exactly as the original wrote it.
//!
//! # What each file is for
//!
//! * [`layout`] -- the sixteen offsets and the build gate. `None` on a build nobody measured, and
//!   then no detour is installed at all.
//! * [`vitals`] -- the arithmetic, as a pure function. Two of the eight values are computed
//!   rather than copied, both transcribed from the game's own instructions, and one of them is
//!   named misleadingly enough to deserve its own section there.
//! * [`derived`] -- the `[hud]` block of `er-npc-possess.derived.toml`, which answers "why is my
//!   FP bar empty" with the `NpcParam` value that made it so.
//! * `detour` (windows only) -- the hook itself, its refusal path and its teardown.
//!
//! # What this layer does not retarget, and why that is not a shortfall
//!
//! The rune count, the equipped armaments, the great rune bar and the memorised spell slots.
//! They are read during the original call from `PlayerGameData` (`ChrIns` vtable `+0x168`) and
//! `GetWeaponGaitemHandleBySlot` (`+0x230`), neither of which an `EnemyIns` implements
//! meaningfully -- `+0x230` writes 0 and `GetChrAsm` answers 0. Retargeting them would not show
//! the creature's equipment, it would show an empty equipment HUD, which is why the design
//! substitutes eight fields rather than the character pointer.

// The offsets, the arithmetic and the report are pure and stay ungated, so `cargo test` proves
// them on the host where the windows-only game bindings do not exist.
pub(crate) mod derived;
pub(crate) mod layout;
pub(crate) mod vitals;

#[cfg(windows)]
mod detour;

#[cfg(windows)]
pub(crate) use derived::{Decision, Off, render as render_derived};
#[cfg(windows)]
pub(crate) use detour::{follow, install, outcome, read_source, shutdown, stop};
