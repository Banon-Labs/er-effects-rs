//! Giving the live character the build's appearance.
//!
//! # The write goes through the game's own writer, and there is a real one
//!
//! `CS::PlayerGameData::CopyFaceDataFromBuffer(PlayerGameData*, FaceDataBuffer*)` takes exactly
//! the two things this module has, and storing 288 bytes into `PGD+0x760+0x08` instead would be
//! wrong in three separate ways -- it would skip a version gate, leave a derived block stale, and
//! bump no generation stamp. [`er_game_base::rva::PLAYER_GAME_DATA_COPY_FACE_DATA_FROM_BUFFER_RVA`]
//! documents each one against the decompiled callee.
//!
//! The stamp is the interesting one, because it is what makes this feature visible at all.
//!
//! # Whether a character already in the world re-renders
//!
//! It does, for the sliders, and the engine does it by itself. Three links, each read out of the
//! 1.16.2 dump and carried to the installed build:
//!
//! 1. `PlayerIns::InitializeCharacterRendering` (`0x140650bf0`) runs
//!    `CSChrAsmModelIns::SetFaceData(playerIns->chrAsmModelIns, &GetPlayerGameData(chrIns)->faceData)`.
//!    The model instance holds a pointer to `PlayerGameData::faceData` rather than a copy of it,
//!    so there is no second buffer to keep in step.
//! 2. `SetFaceData` caches the `FaceData+0x14c` generation stamp at `modelIns+0x308`.
//! 3. `CS::PlayerIns::PrePhysicsSafe1` calls `FUN_1409ec160` every frame that
//!    `chrAsmModelIns` is non-null, which ends in `FUN_1409e9b20`: read the stamp, compare it with
//!    the cached one, and on a difference re-apply the face across the instance's 27 model slots
//!    before re-caching.
//!
//! `CopyFaceDataFromBuffer` bumps that stamp. So the write lands and the next frame picks it up --
//! no reload, no menu, nothing to pump. The game relies on exactly this path itself for other
//! players: `CS::PlayerIns::PopulateFromPcInfo` writes a face into a live `PlayerGameData` every
//! time a peer's appearance arrives over the network.
//!
//! ## The part that stays until the next load
//!
//! The eight leading `i32`s of the payload are model ids -- face mesh, hair, eyes, eyebrows,
//! beard, accessories, decal, eyelashes. Those are consumed by `FUN_1409e6fb0`, which a live
//! `PlayerIns` reaches only through vtable slot `0x500`, and the single dispatch site for that
//! slot sits inside the `ChrSet` load-state machine -- that is, while a character is streaming
//! in. So a build whose face differs from the character's only by hair style is expected to look
//! unchanged until the next load, while every slider, proportion and colour moves immediately.
//! That is inference from a single-dispatch-site search rather than a proven negative, and it is
//! the one claim here worth checking against a live run.
//!
//! # Why the character's own buffer is edited rather than a new one built
//!
//! The planner's format covers `buffer[0..264]`, which is neither the whole payload nor the
//! header. Assembling a fresh `FaceDataBuffer` around it would mean inventing a magic, a version
//! and a declared size, and guessing at twelve bytes nobody has a value for. Reading the
//! character's own buffer and overwriting only the range the layout covers means the header is
//! one the game wrote, the tail is the character's, and the version gate cannot fail for a reason
//! this crate invented.

use er_build_import_core::sliders::{self, SliderMap, SlidersRejection};
use er_game_base::rva::PLAYER_GAME_DATA_COPY_FACE_DATA_FROM_BUFFER_RVA;

use crate::read_character::pgd;

/// `CS::PlayerGameData::CopyFaceDataFromBuffer(PlayerGameData*, FaceDataBuffer*)`. RCX/RDX, no
/// return.
type CopyFaceDataFromBufferFn = unsafe extern "system" fn(usize, *const u8);

// The core's buffer length is a plain number, and this is what stops it becoming one nobody
// re-checks: it is pinned to the upstream `FaceDataBuffer` the read path already binds to, so a
// struct change fails the build rather than handing the native a short buffer. The callee copies a
// fixed `0x120` bytes and does not consult the declared size, so a short source is an
// out-of-bounds read, not a truncated face.
const _: () = assert!(sliders::FACE_BUFFER_LEN == pgd::FACE_DATA_BUFFER_LEN);

/// What the appearance pass did.
///
/// Every outcome is reported rather than collapsed, for the same reason [`crate::chr_name`] does
/// it: "the build carries no face", "the face was applied" and "the native is missing on this
/// build" all leave the character looking untouched, and only one of them is a defect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaceOutcome {
    /// The appearance changed, and the read-back confirms the bytes landed.
    Applied {
        /// How many of the planner's 264 bytes differ from what the character had.
        changed_bytes: usize,
    },
    /// The character already looks like this; nothing was written.
    AlreadyWearing,
    /// The build names no appearance, or one this character's buffer cannot carry.
    NothingToAdopt(SlidersRejection),
    /// The character's own appearance could not be read, so there was nothing to write over.
    Unreadable,
    /// `CopyFaceDataFromBuffer` has no verified mapping for the running build.
    Refused,
    /// The call was made and the buffer does not hold what was asked for. Never reported as a
    /// success: this crate derives no result from a call having returned, and the callee's own
    /// version gate is a silent no-op when it refuses.
    ReadBackDisagreed {
        /// How many of the 264 bytes still disagree.
        differing_bytes: usize,
    },
}

impl FaceOutcome {
    /// One log line's worth of what happened.
    pub fn label(&self) -> String {
        match self {
            FaceOutcome::Applied { changed_bytes } => format!(
                "applied -- {changed_bytes} of {} slider byte(s) changed. Sliders, proportions \
                 and colours re-render on the next frame; hair and face-mesh ids are expected to \
                 wait for the next load",
                sliders::SLIDER_BYTES
            ),
            FaceOutcome::AlreadyWearing => {
                "the character already has this appearance, left alone".to_owned()
            }
            FaceOutcome::NothingToAdopt(why) => {
                format!("not applied -- {}", why.indicator())
            }
            FaceOutcome::Unreadable => "NOT APPLIED -- this character's own appearance could not \
                                        be read, so there was nothing to write over"
                .to_owned(),
            FaceOutcome::Refused => {
                "NOT APPLIED -- `CS::PlayerGameData::CopyFaceDataFromBuffer` has no verified \
                 mapping for the running build. The character keeps its appearance."
                    .to_owned()
            }
            FaceOutcome::ReadBackDisagreed { differing_bytes } => format!(
                "FAILED -- the call returned and {differing_bytes} slider byte(s) still disagree. \
                 The callee ignores a buffer whose version is not 4 without reporting it."
            ),
        }
    }

    /// Whether the character's appearance is now the build's.
    pub fn adopted(&self) -> bool {
        matches!(
            self,
            FaceOutcome::Applied { .. } | FaceOutcome::AlreadyWearing
        )
    }
}

/// Give the live character the build's appearance.
///
/// # Safety
///
/// Game thread, character loaded, `pgd_address` a live `PlayerGameData*`.
pub unsafe fn adopt_build_face(
    module_base: usize,
    pgd_address: usize,
    wanted: Option<&SliderMap>,
) -> FaceOutcome {
    let Some(wanted) = wanted.filter(|set| !set.is_empty()) else {
        return FaceOutcome::NothingToAdopt(SlidersRejection::Absent);
    };

    // The character's own buffer, which is both the thing being edited and the only source of a
    // header the game will accept.
    // Safety: the caller's contract; the read is fault-checked.
    let Some(current) = (unsafe { crate::read_character::read_face_data(pgd_address) }) else {
        return FaceOutcome::Unreadable;
    };

    let mut next = current.clone();
    if let Err(why) = sliders::encode_into(wanted, &mut next) {
        return FaceOutcome::NothingToAdopt(why);
    }
    let changed_bytes = differing(&current, &next);
    if changed_bytes == 0 {
        return FaceOutcome::AlreadyWearing;
    }

    let Some(address) = crate::native::resolve(
        module_base,
        PLAYER_GAME_DATA_COPY_FACE_DATA_FROM_BUFFER_RVA,
        "CS::PlayerGameData::CopyFaceDataFromBuffer",
    ) else {
        return FaceOutcome::Refused;
    };
    // Safety: the address was resolved for the running build immediately above.
    let copy_face_data: CopyFaceDataFromBufferFn = unsafe { core::mem::transmute(address) };
    // Safety: `pgd_address` is live per the caller's contract, and `next` is exactly
    // `FACE_BUFFER_LEN` bytes -- the fixed length the callee copies without consulting the
    // buffer's own declared size. It borrows the buffer only for the duration of the call.
    unsafe { copy_face_data(pgd_address, next.as_ptr()) };

    // Safety: as above. Read back rather than trusting the call: the callee's only validation is
    // `version == 4`, and failing it returns normally having written nothing.
    let Some(after) = (unsafe { crate::read_character::read_face_data(pgd_address) }) else {
        return FaceOutcome::Unreadable;
    };
    let differing_bytes = differing(&after, &next);
    if differing_bytes == 0 {
        FaceOutcome::Applied { changed_bytes }
    } else {
        FaceOutcome::ReadBackDisagreed { differing_bytes }
    }
}

/// How many of the planner's [`sliders::SLIDER_BYTES`] differ between two whole buffers.
///
/// Only that range, because it is the only range either side of this ever writes: counting the
/// header would make every comparison differ by zero for uninteresting reasons, and counting the
/// twelve-byte tail would report a difference this module cannot cause.
fn differing(left: &[u8], right: &[u8]) -> usize {
    let payload = sliders::FACE_BUFFER_PAYLOAD_OFFSET;
    let end = payload + sliders::SLIDER_BYTES;
    let (Some(left), Some(right)) = (left.get(payload..end), right.get(payload..end)) else {
        return sliders::SLIDER_BYTES;
    };
    left.iter().zip(right).filter(|(a, b)| a != b).count()
}
