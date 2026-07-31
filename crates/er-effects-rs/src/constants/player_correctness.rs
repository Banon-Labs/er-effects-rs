// ---- CS::PlayerGameData correctness oracle (read at in-world) ----
/// `GameDataMan::play_time` (u32, in-game play time in milliseconds, maxed at 999:59:59.999).
/// WORLD-LIVE LIVENESS signal for the render gate: the game advances this clock only while the
/// world simulation is actually stepping; it is PAUSED during loads/menus/frozen-world states.
/// So a rising `oracle_play_time_ms` across a dwell window proves the world is live (not a
/// render-frozen "present but nothing moving" reload). Bound to the typed layout so it tracks
/// fromsoftware-rs and fails the build on struct drift.
pub(crate) const GAME_DATA_MAN_PLAY_TIME_A0_OFFSET: usize =
    core::mem::offset_of!(GameDataMan, play_time);
pub(crate) const PGD_CURRENT_HP_10_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_hp);
pub(crate) const PGD_BASE_MAX_HP_18_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, base_max_hp);
pub(crate) const PGD_CURRENT_FP_1C_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_fp);
pub(crate) const PGD_BASE_MAX_FP_24_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, base_max_fp);
pub(crate) const PGD_CURRENT_STAMINA_2C_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_stamina);
pub(crate) const PGD_BASE_MAX_STAMINA_34_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, base_max_stamina);
pub(crate) const PGD_RUNE_COUNT_6C_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, rune_count);
pub(crate) const PGD_RUNE_MEMORY_70_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, rune_memory);
pub(crate) const PGD_CHR_TYPE_98_OFFSET: usize = core::mem::offset_of!(PlayerGameData, chr_type);
pub(crate) const PGD_EQUIP_GAME_DATA_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, equipment);
pub(crate) const EQUIP_GAME_DATA_CHR_ASM_OFFSET: usize =
    core::mem::offset_of!(EquipGameData, chr_asm);
pub(crate) const CHR_ASM_SIZE: usize = core::mem::size_of::<ChrAsm>();
/// Runtime `ChrAsm` member offsets, for assembling a runtime-layout image from the SERIALIZED save
/// sections (which store the same blocks in a different order; see
/// `SerializedSaveSlot::runtime_chr_asm_image`).
pub(crate) const CHR_ASM_EQUIPMENT_OFFSET: usize = core::mem::offset_of!(ChrAsm, equipment);
pub(crate) const CHR_ASM_GAITEM_HANDLES_OFFSET: usize =
    core::mem::offset_of!(ChrAsm, gaitem_handles);
pub(crate) const CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET: usize =
    core::mem::offset_of!(ChrAsm, equipment_param_ids);
pub(crate) const PGD_ARCHETYPE_BF_OFFSET: usize = core::mem::offset_of!(PlayerGameData, archetype);
pub(crate) const PGD_VOICE_TYPE_C2_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, voice_type);
pub(crate) const PGD_STARTING_GIFT_C3_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, starting_gift);
pub(crate) const PGD_UNLOCKED_TALISMAN_SLOTS_C6_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, unlocked_talisman_slots);
pub(crate) const PGD_SPIRIT_ASH_LEVEL_C7_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, matchmaking_spirit_ashes_level);
pub(crate) const PGD_MAX_CRIMSON_FLASK_101_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, max_hp_flask);
pub(crate) const PGD_MAX_CERULEAN_FLASK_102_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, max_fp_flask);
pub(crate) const PGD_FACE_DATA_OFFSET: usize = core::mem::offset_of!(PlayerGameData, face_data);
pub(crate) const FACE_DATA_BUFFER_OFFSET: usize = core::mem::offset_of!(FaceData, face_data_buffer);
pub(crate) const FACE_DATA_BUFFER_MAGIC_OFFSET: usize =
    core::mem::offset_of!(FaceDataBuffer, magic);
pub(crate) const FACE_DATA_BUFFER_VERSION_OFFSET: usize =
    core::mem::offset_of!(FaceDataBuffer, version);
pub(crate) const FACE_DATA_BUFFER_SIZE_OFFSET: usize =
    core::mem::offset_of!(FaceDataBuffer, buffer_size);
pub(crate) const FACE_DATA_BUFFER_PAYLOAD_OFFSET: usize =
    core::mem::offset_of!(FaceDataBuffer, buffer);
pub(crate) const FACE_DATA_BUFFER_PAYLOAD_SIZE: usize =
    core::mem::size_of::<FaceDataBuffer>() - FACE_DATA_BUFFER_PAYLOAD_OFFSET;
pub(crate) const FACE_DATA_BUFFER_TOTAL_SIZE: usize =
    FACE_DATA_BUFFER_PAYLOAD_OFFSET + FACE_DATA_BUFFER_PAYLOAD_SIZE;
/// Native `FaceData::CopyFromBuffer` (mirrored from the native row builder `FUN_14025f9b0`): copies an
/// inner `FaceDataBuffer` (`FACE` magic) into a live `FaceData` wrapper (e.g. a ProfileSummary record's
/// +0x38 block). The SAVED wrapper header does NOT match the live one (2026-06-27 native row dumps), so
/// records must be filled through this helper, never by memcpy'ing the saved wrapper.
pub(crate) const FACE_DATA_COPY_FROM_BUFFER_RVA: usize = 0x00252f70;
/// Native `ChrAsm` copy the row builder uses for a ProfileSummary record's equipment block (+0x1a8) --
/// the source the profile renderer reads to dress the portrait model.
///
/// NOT A MEMCPY (byte-verified 2026-07-31 at deobf 0x140245c00, 1.16.2 zero shift): it runs
/// `GaitemHandle::copy` (0x140682580) 22 times over `+0x24`, i.e. a REFCOUNTING assign that
/// increments the incoming handle and releases the previous occupant, and only then does a plain
/// 22-entry u32 copy of `equipment_param_ids` at `+0x7c`. Feeding it a FOREIGN save's handles
/// therefore touches live refcount state on a `gaitemInsTable` this process owns -- which is why
/// `SerializedSaveSlot::runtime_chr_asm_image` zeroes the handle array instead of copying it.
pub(crate) const CHR_ASM_COPY_RVA: usize = 0x00245c00;
/// `CS::CSGaitemImp::GetGaItemHandleProtector(CSGaitemImp* rcx, u32* out rdx, i32 paramId r8d) -> u32*`
/// (deobf 0x140671fd0, byte-verified). MINTS a live in-process instance for a protector param id:
/// `GetUnindexedGaItemHandle` pops a free table index (refCount 0->1), HeapAllocs a `CSProGaitemIns`,
/// registers it into `gaitemInsTable[index]`, then `SetItemIdWithProtectorCategory(ins, paramId)`.
/// Returns the `out` pointer in rax. Under free-queue exhaustion it returns an unresolved (zero)
/// handle rather than faulting, so the failure mode is a blank slot, not an AV.
pub(crate) const GET_GAITEM_HANDLE_PROTECTOR_RVA: usize = 0x00671fd0;
/// `CS::ChrAsm::EquipProtectorOrAccessory(ChrAsm* rcx, i32 slot edx, u32* handle r8)`
/// (deobf 0x1403bf490, byte-verified as literally `add $0xc,%edx; jmp 0x1403bf3c0` = `EquipItem`).
/// The `+0xc` is what pins `ChrAsm::ProtectorHead == 12`; see `CHR_ASM_PROTECTOR_HEAD_INDEX`.
pub(crate) const CHR_ASM_EQUIP_PROTECTOR_RVA: usize = 0x003bf490;
/// `GaItemHandle::~GaItemHandle(u32* rcx)` (deobf 0x140682480, byte-verified; early-outs when the
/// handle word is 0). MANDATORY after every `GET_GAITEM_HANDLE_PROTECTOR_RVA` + equip pair: the
/// per-feed refcount ledger only nets to zero because this drops the local's reference (alloc takes
/// the entry 0->1, the equip assign takes it to 2 and releases the previous occupant, this drops
/// 2->1). Skipping it pins refCount at 2 and leaks one slot of a 5119-entry pool per call.
pub(crate) const GAITEM_HANDLE_DTOR_RVA: usize = 0x00682480;
/// Index of `ProtectorHead` within `ChrAsm::gaitem_handles` / `ChrAsm::equipment_param_ids`; the four
/// armor slots are head/chest/hands/legs at `+0..+3`. Grounded in the `add $0xc,%edx` above rather
/// than assumed.
pub(crate) const CHR_ASM_PROTECTOR_HEAD_INDEX: usize = 12;
/// Number of protector (armor) slots the portrait resolution oracle covers: head, chest, hands, legs.
pub(crate) const CHR_ASM_PROTECTOR_SLOT_COUNT: usize = 4;
/// `CSMenuProfModelRend` -> its owned `ChrAsm`. The native getter `FUN_140bb9800` is literally
/// `lea 0x548(%rcx),%rax; ret`, and the profile feed `FUN_140bbe1a0` passes exactly that pointer to
/// every `EquipItem`/`EquipProtectorOrAccessory` call it makes.
pub(crate) const PROFILE_RENDERER_CHR_ASM_OFFSET: usize = 0x548;
/// Face-body values are the face payload that begins at FaceDataBuffer::buffer.
pub(crate) const FACE_BODY_FIELD_FACE_MODEL_OFFSET: usize = FACE_DATA_BUFFER_PAYLOAD_OFFSET;
pub(crate) const FACE_BODY_FIELD_HAIR_MODEL_OFFSET: usize =
    FACE_BODY_FIELD_FACE_MODEL_OFFSET + core::mem::size_of::<u32>();
/// The eyebrow field follows the hair field after one u32-sized reserved/model slot in the
/// serialized face-body payload.
pub(crate) const FACE_BODY_FIELD_EYEBROW_MODEL_OFFSET: usize =
    FACE_BODY_FIELD_HAIR_MODEL_OFFSET + core::mem::size_of::<u32>() + core::mem::size_of::<u32>();
pub(crate) const FACE_BODY_FIELD_BEARD_MODEL_OFFSET: usize =
    FACE_BODY_FIELD_EYEBROW_MODEL_OFFSET + core::mem::size_of::<u32>();
pub(crate) const FACE_BODY_FIELD_EYE_PATCH_MODEL_OFFSET: usize =
    FACE_BODY_FIELD_BEARD_MODEL_OFFSET + core::mem::size_of::<u32>();
/// The apparent-age byte follows the model-id cluster after three u32-sized face-shape slots.
pub(crate) const FACE_BODY_FIELD_APPARENT_AGE_OFFSET: usize = FACE_BODY_FIELD_EYE_PATCH_MODEL_OFFSET
    + core::mem::size_of::<u32>()
    + core::mem::size_of::<u32>()
    + core::mem::size_of::<u32>();
pub(crate) const FACE_BODY_FIELD_FACIAL_AESTHETIC_OFFSET: usize =
    FACE_BODY_FIELD_APPARENT_AGE_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_FORM_EMPHASIS_OFFSET: usize =
    FACE_BODY_FIELD_FACIAL_AESTHETIC_OFFSET + core::mem::size_of::<u8>();
#[repr(C)]
pub(crate) struct FaceBodyLayout {
    pub(crate) unknown_000: [u8; 0xac],
    pub(crate) head_size: u8,
}

pub(crate) const FACE_BODY_FIELD_HEAD_SIZE_OFFSET: usize =
    core::mem::offset_of!(FaceBodyLayout, head_size);
pub(crate) const FACE_BODY_FIELD_CHEST_SIZE_OFFSET: usize =
    FACE_BODY_FIELD_HEAD_SIZE_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_ABDOMEN_SIZE_OFFSET: usize =
    FACE_BODY_FIELD_CHEST_SIZE_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_ARMS_SIZE_OFFSET: usize =
    FACE_BODY_FIELD_ABDOMEN_SIZE_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_LEGS_SIZE_OFFSET: usize =
    FACE_BODY_FIELD_ARMS_SIZE_OFFSET + core::mem::size_of::<u8>();
/// Skin color follows the body-size bytes after two one-byte face-body values that are not part
/// of the oracle fingerprint.
pub(crate) const FACE_BODY_FIELD_SKIN_COLOR_R_OFFSET: usize = FACE_BODY_FIELD_LEGS_SIZE_OFFSET
    + core::mem::size_of::<u8>()
    + core::mem::size_of::<u8>()
    + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_SKIN_COLOR_G_OFFSET: usize =
    FACE_BODY_FIELD_SKIN_COLOR_R_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_SKIN_COLOR_B_OFFSET: usize =
    FACE_BODY_FIELD_SKIN_COLOR_G_OFFSET + core::mem::size_of::<u8>();
/// Base/end of the contiguous stat block; upstream's first post-stat field is `base_hero_point`.
pub(crate) const PGD_STAT_END_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, base_hero_point);
pub(crate) const PGD_STAT_COUNT: usize =
    (PGD_STAT_END_OFFSET - PGD_STAT_BASE_3C_OFFSET) / core::mem::size_of::<u32>();
/// GameMan last field: `character_name_is_empty` (a cheap blank/new-game discriminator).
/// RESOLVED (autoresearch 2026-06-18) via static RE of `eldenring-deobf.bin`: the in-game
/// getter at 0x140679d90 is `mov rax,[GameMan]; movzbl 0xe70(rax),eax; ret`, so the field is
/// at +0xe70 -- our prior hand-decoded offset was 8 bytes too far (read padding past the field),
/// a real BUG. Now bound to the upstream typed field, which the disassembly confirms correct.
pub(crate) const GAME_MAN_NAME_IS_EMPTY_E70_OFFSET: usize =
    core::mem::offset_of!(GameMan, character_name_is_empty);
/// One-shot latch for the in-world LOAD-CORRECTNESS dump.
pub(crate) use er_telemetry::counters::LOAD_CORRECTNESS_DUMPED;
pub(crate) const LOAD_CORRECTNESS_NOT_DUMPED: usize = 0;
/// One-shot latches for the OBSERVE-mode title->menu timing baseline (T0 at the parked title,
/// T_menu_open when the TitleTopDialog reaches TextFadeOut). Lets a true-vanilla run (no forcing,
/// modals + presses by the user) emit the SAME markers as the DLL-headless run for comparison.
pub(crate) use er_telemetry::counters::OBSERVE_T0_EMITTED;
pub(crate) use er_title_flow::OBSERVE_MENU_OPEN_EMITTED;
pub(crate) use er_title_flow::OBSERVE_MARKER_NOT_EMITTED;
pub(crate) use er_title_flow::OBSERVE_MARKER_EMITTED;
/// Synthetic `this` for the IngameInit-tail stream-worker register call 0x140b0a980
/// (+0x48 set to WORLD_WORKER_BUILD_STATE hits the build+register arm).
pub(crate) static mut OWN_STEPPER_WORKER_THIS: [u8; SYNTHETIC_STEP_THIS_SIZE] =
    [MOVIE_SKIP_FLAG_CLEAR; SYNTHETIC_STEP_THIS_SIZE];
pub(crate) const OWN_STEPPER_PATCHED_NO: usize = false as usize;
pub(crate) const OWN_STEPPER_PATCHED_YES: usize = true as usize;
/// Original idx10 func ptr (STEP_MenuJobWait), saved so our handler can pass through.
pub(crate) static OWN_STEPPER_ORIG_IDX10: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_title_flow::OWN_STEPPER_BASE;
pub(crate) static OWN_STEPPER_PATCHED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_PATCHED_NO);
pub(crate) static OWN_STEPPER_CALLS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);

