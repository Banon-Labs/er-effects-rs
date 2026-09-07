//! The spawn layer -- bringing a creature into the world to be, rather than finding one.
//!
//! Layers 1-3 can only possess something the map already placed. This one creates the creature
//! first, waits for it to become drivable, and takes it away again afterwards.
//!
//! # There is no residency restriction, and that was the open question
//!
//! The brief allowed for one: "if the honest answer is only chrs already resident in the current
//! map can be spawned, then ship that as an enforced restriction". It is not the answer. Nothing
//! anywhere on the spawn path validates the chr id or consults a map asset list -- the UTF-16 name
//! rides in `ChrSpawnRequest.model` into `ChrInitData.chrNameChrRes`, the `ChrIns` constructor
//! builds a `CS::ChrRes` step machine at `ChrIns+0x28`, and that goes and acquires the chrbnd,
//! anibnd, behbnd and texbnd itself. The game's own runtime spawner, `CSTalkDynamicChrCtrl`, never
//! touches `EneDatMan` either; it just hands over the name. So an arbitrary creature id is
//! spawnable and this layer enforces no residency check, because inventing one would refuse ids
//! that work.
//!
//! **What replaces it is the deadline**, and the reason it has to exist is the other half of the
//! same finding: there is no error edge and no timeout anywhere in the eight `ChrRes` states or the
//! eleven `EneDat` ones. A chr id with no chrbnd on disk does not fail -- it waits, forever, having
//! already allocated and registered a `ChrIns`. See [`readiness`], which owns that deadline and
//! reports which gate a bad pick died on.
//!
//! The one restriction that is enforced is the format: [`request::MAX_CHR_ID`], because the name is
//! `c%04d` and a five-digit id would spell a directory that cannot exist.
//!
//! # ...and one that is enforced on the placement, which is new
//!
//! `[spawn].distance_m` used to be the whole answer, for every creature. It is now a FLOOR: the
//! creature is put far enough out that its own physics capsule clears the player's, and the
//! configured distance only wins when it is already further than that. See [`placement`] for the
//! arithmetic, the measured case that produced it, and why the radius rather than the height is
//! the field that decides overlap.
//!
//! # What this layer costs, in the currency this crate counts
//!
//! Two game function addresses, taking the crate from one to three. Both are byte-verified in both
//! images and resolved through `game_rva_named`, so an unrecognised build refuses the spawn rather
//! than jumping into whatever now occupies those bytes -- see [`game::SPAWN_DYNAMIC_CHR_RVA`] and
//! [`game::REMOVE_CHR_INS_RVA`] for the evidence and for why `SpawnSummonBuddy`, the entry the
//! brief named, was the wrong one to spend an address on.
//!
//! It costs no detour. The crate still claims no prologue, which is what keeps it co-loadable with
//! the other shells in the profile.
//!
//! The readiness oracle costs nothing at all: all four of its gates are field reads, three of them
//! byte-proven identical across both builds and the fourth version-gated because its `EneDat`
//! offsets moved by `0x10` on 1.17.
//!
//! # The ordering that is a crash if it moves
//!
//! `RemoveChrIns` hands the character to `CSDelayDeleteMan`, whose eventual destruction runs
//! `ChrCtrl::Unref`, which DLPanics on a non-null `ChrCtrl+0x3b0`. So despawn must come after the
//! manipulator override is cleared, and that is not a comment: it is
//! `crate::possess::teardown::Step::DespawnCreature`'s discriminant, sitting after
//! `RestoreManipulatorVtable`, with a test that fails if anyone reorders them.

pub(crate) mod placement;
pub(crate) mod readiness;
pub(crate) mod request;

#[cfg(windows)]
pub(crate) mod game;
