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
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
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
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
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
///
/// It also copies `unk0` (+0x00), `unkd4` (+0xd4) and `unkd8` (+0xd8) VERBATIM (`field0_0x0 =
/// *param_2; field5_0xd4 = param_2[0x35]; field6_0xd8 = param_2[0x36]`), and the profile pipeline
/// runs it twice more (record +0x1a8 -> renderer +0x548 -> +0x33c -> +0x130). A wrong value in those
/// three therefore reaches the model build unaltered -- see `CHR_ASM_OVERRIDE_ABSENT`.
pub(crate) const CHR_ASM_COPY_RVA: usize = 0x00245c00;
/// Index of `ProtectorHead` within `ChrAsm::gaitem_handles` / `ChrAsm::equipment_param_ids`; the four
/// armor slots are head/chest/hands/legs at `+0..+3`. Grounded in the disassembly, not assumed:
/// `CS::ChrAsm::EquipProtectorOrAccessory` (deobf 0x1403bf490) is literally `add $0xc,%edx; jmp
/// EquipItem`, and `CS::ChrAsm::GetProtectorParamIdBySlot` (deobf 0x1403be950) is `lea 0xc(%rdx),%eax
/// ; movslq %eax,%rdx ; mov 0x7c(%rcx,%rdx,4),%eax ; ret`.
pub(crate) const CHR_ASM_PROTECTOR_HEAD_INDEX: usize = 12;
/// Number of protector (armor) slots the portrait resolution oracle covers: head, chest, hands, legs.
pub(crate) const CHR_ASM_PROTECTOR_SLOT_COUNT: usize = 4;
/// Entries in `ChrAsm::gaitem_handles` and in `ChrAsm::equipment_param_ids`. The ctor pins it: after
/// `lea 0x7c(%rsi),%rdi` it runs `mov $0x16,%ecx ; rep stos %eax,(%rdi)` with `eax = -1`
/// (deobf 0x1403be213..0x1403be222), i.e. 0x16 = 22 dwords.
pub(crate) const CHR_ASM_EQUIPMENT_ENTRY_COUNT: usize = 22;
/// `ChrAsm::unk0` (+0x00). Together with `unkd4`/`unkd8` this is a WHOLE-OUTFIT OVERRIDE input, not
/// padding: see `CHR_ASM_OVERRIDE_ABSENT`. Its typed field is private in `fromsoftware-rs`, so the
/// offset is spelled out here rather than taken from `offset_of!`; the ctor writes it first
/// (`movl $0xffffffff,(%rcx)` at deobf 0x1403be1d0), which is what pins it to +0.
pub(crate) const CHR_ASM_UNK0_OFFSET: usize = 0x00;
/// `ChrAsm::unkd4` (+0xd4) -- the HEAD whole-outfit override. Derived from the typed layout rather
/// than hard-coded: the param-id array is the last public field, so `unkd4` is exactly one array past
/// it. The ctor's `movq $-1,0xd4(%rsi)` (deobf 0x1403be208) is the independent confirmation, and it
/// writes EIGHT bytes -- so that one instruction sets both `unkd4` and `unkd8`.
pub(crate) const CHR_ASM_UNKD4_OFFSET: usize = CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET
    + CHR_ASM_EQUIPMENT_ENTRY_COUNT * core::mem::size_of::<i32>();
/// `ChrAsm::unkd8` (+0xd8) -- the CHEST/HANDS/LEGS whole-outfit override.
pub(crate) const CHR_ASM_UNKD8_OFFSET: usize =
    CHR_ASM_UNKD4_OFFSET + core::mem::size_of::<i32>();
/// The value `unk0`/`unkd4`/`unkd8` must hold for the renderer to dress a character from its per-slot
/// `equipment_param_ids`. These three fields are read with SIGNED tests by the model-resource request
/// `FUN_1409e6fb0` (deobf 0x1409e7553..0x1409e75b6, every branch a `js`), and a NON-NEGATIVE value in
/// any of them FORCES the whole outfit from arithmetic on that value instead:
///
/// ```text
///   head  (category 1): unkd4 >= 0 -> unkd4
///   chest (category 2): unkd8 >= 0 -> unkd8 + 100
///   hands (category 3): unkd8 >= 0 -> unkd8 + 200 ; else unk0 >= 0 -> unk0 + 200
///   legs  (category 4): unkd8 >= 0 -> unkd8 + 300
/// ```
///
/// A zero-initialised image therefore renders param ids 0/100/200/300 -- rows that do not exist -- so
/// NOTHING resolves, not even the bare-body defaults the native profile feed equips into hands and
/// legs. That is the entirely-nude portrait (bd er-effects-rs-wncc). The ctor
/// `CS::ChrAsm::ChrAsm` (deobf 0x1403be1b0) sets all three to -1, which is why a ctor-built ChrAsm
/// never hits the override path and our hand-built image must match it.
pub(crate) const CHR_ASM_OVERRIDE_ABSENT: i32 = -1;
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
/// `set_model_source` calls it only with 2 and 3, so a portrait's HANDS and LEGS are always these
/// bare-body rows -- vanilla behaviour, not a defect.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const PROTECTOR_DEFAULT_PARAM_ID_BASE: i32 = 10000;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const PROTECTOR_DEFAULT_PARAM_ID_STRIDE: i32 = 100;
/// `CSMenuProfModelRend` -> its LIVE stage-0 `ChrAsm`, the one the model build actually reads.
///
/// DO NOT SUBSTITUTE +0x548. The renderer holds THREE `ChrAsm` stages: +0x548 is the INBOX that
/// `set_model_source` writes, +0x33c is the staged copy, and +0x130 is live. `STEP_Init_Setup`
/// (0x140bb9ca0) snapshots the inbox into +0x33c exactly ONCE and never dereferences it again, while
/// `STEP_Wait_Play` (0x140bba080) re-runs the model-resource request `FUN_1409e6fb0` against +0x130
/// EVERY frame a model instance exists. An oracle reading +0x548 therefore reports what we handed the
/// renderer, not what it is rendering -- which is precisely how the PR #128 oracle passed a run the
/// user saw as nude (bd er-effects-rs-91l5).
pub(crate) const PROFILE_RENDERER_CHR_ASM_LIVE_OFFSET: usize = 0x130;
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

