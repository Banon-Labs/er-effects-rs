// ---- CS::PlayerGameData correctness oracle (read at in-world) ----
/// `GameDataMan::play_time` (u32, in-game play time in milliseconds, maxed at 999:59:59.999).
/// World-live LIVENESS signal for the render gate: the game advances this clock only while the
/// world simulation is actually stepping; it is paused during loads/menus/frozen-world states.
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
// `CHR_ASM_SIZE`, `CHR_ASM_EQUIPMENT_OFFSET`, `CHR_ASM_GAITEM_HANDLES_OFFSET` and
// `CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET` moved to `er_loading_portrait_core::chr_asm_layout` with the
// loading-cover crate extraction; `constants.rs`'s `er_loading_portrait_core::*` glob re-exports
// them, so every reference below and across the root crate resolves unchanged.
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
// `FACE_DATA_BUFFER_OFFSET`, `FACE_DATA_BUFFER_MAGIC_OFFSET`, `FACE_DATA_BUFFER_VERSION_OFFSET`,
// `FACE_DATA_BUFFER_SIZE_OFFSET`, `FACE_DATA_BUFFER_PAYLOAD_OFFSET`,
// `FACE_DATA_BUFFER_PAYLOAD_SIZE`, `FACE_DATA_BUFFER_TOTAL_SIZE`,
// `FACE_DATA_COPY_FROM_BUFFER_RVA` and `CHR_ASM_COPY_RVA` moved to
// `er_profile_summary_core::face_data` with the ProfileSummary crate extraction (same values, same
// doc comments): a record's `face_data` (+0x38) and `chr_asm` (+0x1a8) blocks are ProfileSummary
// fields, written by that crate's serialized-slot reader and read by nothing else. The
// `constants.rs` glob re-exports them into this flat namespace, exactly as the `CHR_ASM_*` move
// below already does.
// `CHR_ASM_PROTECTOR_HEAD_INDEX`, `CHR_ASM_PROTECTOR_SLOT_COUNT`, `CHR_ASM_EQUIPMENT_ENTRY_COUNT`,
// `CHR_ASM_UNK0_OFFSET`, `CHR_ASM_UNKD4_OFFSET`, `CHR_ASM_UNKD8_OFFSET` and
// `CHR_ASM_OVERRIDE_ABSENT` moved to `er_loading_portrait_core::chr_asm_layout` with the
// loading-cover crate extraction (same values, same doc comments); the `constants.rs` glob
// re-exports them into this flat namespace.
/// The per-category addends `FUN_1409e6fb0` applies to a non-negative `unkd8`/`unk0`
/// (`lea 0x64(%rax),%ebx`, `lea 0xc8(%rax),%ebx`, `lea 0x12c(%rax),%ebx`). Head takes the override
/// value verbatim, so it has no addend.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const CHR_ASM_OVERRIDE_CHEST_ADDEND: i32 = 100;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const CHR_ASM_OVERRIDE_HANDS_ADDEND: i32 = 200;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const CHR_ASM_OVERRIDE_LEGS_ADDEND: i32 = 300;
/// `CS::ChrAsm::GetDefaultProtectorParamId` (deobf 0x140d47420) is a pure switch:
/// 0 -> 10000, 1 -> 10100, 2 -> 10200, 3 -> 10300, anything else -> -1. The profile feed
/// `set_model_source` calls it only with 2 and 3, so a portrait's hands and legs are always these
/// bare-body rows -- vanilla behaviour, not a defect.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const PROTECTOR_DEFAULT_PARAM_ID_BASE: i32 = 10000;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const PROTECTOR_DEFAULT_PARAM_ID_STRIDE: i32 = 100;
// `PROFILE_RENDERER_CHR_ASM_LIVE_OFFSET` moved to `er_loading_portrait_core::chr_asm_layout` with
// the loading-cover crate extraction, beside the sampler that is its only reader.
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
/// Resolved (autoresearch 2026-06-18) via static RE of `eldenring-deobf.bin`: the in-game
/// getter at 0x140679d90 is `mov rax,[GameMan]; movzbl 0xe70(rax),eax; ret`, so the field is
/// at +0xe70 -- our prior hand-decoded offset was 8 bytes too far (read padding past the field),
/// a real bug. Now bound to the upstream typed field, which the disassembly confirms correct.
pub(crate) const GAME_MAN_NAME_IS_EMPTY_E70_OFFSET: usize =
    core::mem::offset_of!(GameMan, character_name_is_empty);
/// One-shot latch for the in-world load-correctness dump.
pub(crate) use er_telemetry_core::counters::LOAD_CORRECTNESS_DUMPED;
pub(crate) const LOAD_CORRECTNESS_NOT_DUMPED: usize = 0;
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

