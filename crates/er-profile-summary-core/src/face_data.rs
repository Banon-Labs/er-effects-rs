//! `CS::FaceData` / `FaceDataBuffer` layout, and the two native copy helpers a ProfileSummary
//! record's visual blocks must be filled through.
//!
//! Moved verbatim from er-quickload `constants/player_correctness.rs`, same values and the same
//! doc comments -- the same move `CHR_ASM_*` already made into
//! `er_loading_portrait_core::chr_asm_layout`. The root re-exports every name through its
//! `constants.rs` glob, so the product's flat namespace is unchanged.
//!
//! These live here rather than beside the other `PlayerGameData` offsets because a record's
//! `face_data` (+0x38) and `chr_asm` (+0x1a8) blocks are ProfileSummary fields: they are written
//! by [`crate::serialized_slot`], read by the profile renderer, and belong to nothing else.

use eldenring::cs::{FaceData, FaceDataBuffer};

pub const FACE_DATA_BUFFER_OFFSET: usize = core::mem::offset_of!(FaceData, face_data_buffer);
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub const FACE_DATA_BUFFER_MAGIC_OFFSET: usize = core::mem::offset_of!(FaceDataBuffer, magic);
pub const FACE_DATA_BUFFER_VERSION_OFFSET: usize = core::mem::offset_of!(FaceDataBuffer, version);
pub const FACE_DATA_BUFFER_SIZE_OFFSET: usize = core::mem::offset_of!(FaceDataBuffer, buffer_size);
pub const FACE_DATA_BUFFER_PAYLOAD_OFFSET: usize = core::mem::offset_of!(FaceDataBuffer, buffer);
pub const FACE_DATA_BUFFER_PAYLOAD_SIZE: usize =
    core::mem::size_of::<FaceDataBuffer>() - FACE_DATA_BUFFER_PAYLOAD_OFFSET;
pub const FACE_DATA_BUFFER_TOTAL_SIZE: usize =
    FACE_DATA_BUFFER_PAYLOAD_OFFSET + FACE_DATA_BUFFER_PAYLOAD_SIZE;
/// Native `FaceData::CopyFromBuffer` (mirrored from the native row builder `FUN_14025f9b0`): copies an
/// inner `FaceDataBuffer` (`FACE` magic) into a live `FaceData` wrapper (e.g. a ProfileSummary record's
/// +0x38 block). The saved wrapper header does not match the live one (2026-06-27 native row dumps), so
/// records must be filled through this helper, never by memcpy'ing the saved wrapper.
pub const FACE_DATA_COPY_FROM_BUFFER_RVA: usize = 0x00252f70;
/// Native `ChrAsm` copy the row builder uses for a ProfileSummary record's equipment block (+0x1a8) --
/// the source the profile renderer reads to dress the portrait model.
///
/// Not a MEMCPY (byte-verified 2026-07-31 at deobf 0x140245c00, 1.16.2 zero shift): it runs
/// `GaitemHandle::copy` (0x140682580) 22 times over `+0x24`, i.e. a REFCOUNTING assign that
/// increments the incoming handle and releases the previous occupant, and only then does a plain
/// 22-entry u32 copy of `equipment_param_ids` at `+0x7c`. Feeding it a foreign save's handles
/// therefore touches live refcount state on a `gaitemInsTable` this process owns -- which is why
/// `SerializedSaveSlot::runtime_chr_asm_image` zeroes the handle array instead of copying it.
///
/// It also copies `unk0` (+0x00), `unkd4` (+0xd4) and `unkd8` (+0xd8) verbatim (`field0_0x0 =
/// *param_2; field5_0xd4 = param_2[0x35]; field6_0xd8 = param_2[0x36]`), and the profile pipeline
/// runs it twice more (record +0x1a8 -> renderer +0x548 -> +0x33c -> +0x130). A wrong value in those
/// three therefore reaches the model build unaltered -- see `CHR_ASM_OVERRIDE_ABSENT`.
pub const CHR_ASM_COPY_RVA: usize = 0x00245c00;
