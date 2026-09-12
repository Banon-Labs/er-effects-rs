// Loaded-character identity oracles: the GameDataMan/PlayerGameData read (level, vitals, runes,
// face data, name, stats) and the play-time world-live clock derived from the same singleton.
//
// One value leaves this subsystem: `play_time_live`. The loading-screen gauge at the very end of
// the emission needs it to report the gauge's live state rather than its stale during-load latch
// (see `oracles_loading_screen_live.rs`), so it is returned rather than recomputed -- recomputing
// would sample the clock twice in one telemetry write and could disagree with itself.

/// Returns `play_time_live`: the world clock has advanced past this load epoch's threshold.
fn write_character_identity_oracles(body: &mut String) -> bool {
    const NULL_PTR: usize = 0;
    // Identity oracle: loaded character values that should match the chosen save slot.
    // These mirror ER-Save-File-Readers' player_game_data models (health/fp today, broader
    // slot attributes as that reference grows) while reading the live GameDataMan path used by
    // dump_load_correctness: GameDataMan = [base + 0x3d5df38]; PlayerGameData = [GameDataMan+8].
    const LEVEL_READ_FAIL: i64 = -1;
    const ZERO_U16: u16 = 0;
    const ZERO_U32: u32 = 0;
    const U16_STRIDE: usize = 2;
    const U32_STRIDE: usize = 4;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    let gdm = crate::game_data_man_ptr_or_null();
    let pgd = if gdm == NULL_PTR {
        NULL_PTR
    } else {
        unsafe {
            crate::experiments::safe_read_usize(
                gdm + crate::GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET,
            )
        }
        .unwrap_or(NULL_PTR)
    };
    // World-live liveness clock: GameDataMan::play_time (u32 ms). Advances only while the world
    // simulation steps; Paused during loads/menus/frozen-world. A rising value across a dwell
    // window is the render-gate's proof the world is live (not a present-but-frozen reload).
    const PLAY_TIME_READ_FAIL: i64 = -1;
    let play_time_ms: i64 = if gdm == NULL_PTR {
        PLAY_TIME_READ_FAIL
    } else {
        unsafe {
            crate::experiments::safe_read_usize(gdm + crate::GAME_DATA_MAN_PLAY_TIME_A0_OFFSET)
        }
        .map_or(PLAY_TIME_READ_FAIL, |v| i64::from((v & 0xffff_ffff) as u32))
    };
    // World-clock-live semaphore (user 2026-07-19, bd play-time-live-world-clock-semaphore): the
    // input-trace path computes this but only emits it to the trace jsonl; mirror it into the main
    // telemetry so the samechar-3x load1-vs-load2 comparison can actually use it (its
    // "world_clock:live" checkpoint reads `play_time_live`). play_time advances only while the world
    // sim steps; a >=1s rise past this load epoch's first-seen value = the world is genuinely live
    // (the loading-screen playtime the user watched ticking). Necessary-not-sufficient for control.
    const PLAY_TIME_LIVE_THRESHOLD_MS: i64 = 1000;
    static PT_ORACLE_EPOCH: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(usize::MAX);
    static PT_ORACLE_FIRST: std::sync::atomic::AtomicI64 =
        std::sync::atomic::AtomicI64::new(PLAY_TIME_READ_FAIL);
    let pt_epoch = crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT
        .load(std::sync::atomic::Ordering::SeqCst);
    if PT_ORACLE_EPOCH.swap(pt_epoch, std::sync::atomic::Ordering::Relaxed) != pt_epoch {
        // New load epoch -> re-arm; the baseline re-latches on the epoch's first real reading below.
        PT_ORACLE_FIRST.store(PLAY_TIME_READ_FAIL, std::sync::atomic::Ordering::Relaxed);
    }
    // Baseline only on a loaded-character playtime (> 0), mirroring the input-trace fix
    // (`PLAY_TIME_TRACE_FIRST` in input_trace.rs). On the boot epoch GameDataMan exists with
    // play_time == 0 long before the Continue deserialize, so baselining at 0 made the first
    // post-deserialize sample report the save's entire stored playtime as "advance" (measured run
    // product-continue-direct-20260729-205115: oracle_play_time_advanced_ms == oracle_play_time_ms
    // == 388876164, ~108h) -> play_time_live falsely latched BOOT_VIEW_EPOCH_WORLD_LIVE for epoch 0
    // at ~+16s, mid boot loading screen, freezing the loading-portrait drive tick (bd
    // er-effects-rs-io53). Requiring > 0 latches the baseline at the character's real loaded
    // playtime, so `advanced` measures only genuine world-sim stepping.
    if PT_ORACLE_FIRST.load(std::sync::atomic::Ordering::Relaxed) < 0 && play_time_ms > 0 {
        PT_ORACLE_FIRST.store(play_time_ms, std::sync::atomic::Ordering::Relaxed);
    }
    let pt_first = PT_ORACLE_FIRST.load(std::sync::atomic::Ordering::Relaxed);
    let play_time_advanced_ms: i64 = if play_time_ms >= 0 && pt_first >= 0 {
        play_time_ms - pt_first
    } else {
        PLAY_TIME_READ_FAIL
    };
    let play_time_live: bool = play_time_advanced_ms >= PLAY_TIME_LIVE_THRESHOLD_MS;
    // Consecutive-live-frames streak for the child-done-override release (bd
    // correction-STEP4-finalize-substate-is-0): count up while live, reset on any non-live frame.
    if play_time_live {
        er_telemetry_core::counters::WORLD_LIVE_STABLE_FRAMES
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    } else {
        er_telemetry_core::counters::WORLD_LIVE_STABLE_FRAMES
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
    if play_time_live {
        // Publish the per-epoch world-live signal so the boot-view compositor stops its per-frame GPU
        // readback once this switch's world is genuinely running (bd
        // fps-killer-rootcaused-per-frame-gpu-readback-boot-view-not-stopping-inworld-load2).
        crate::constants::BOOT_VIEW_EPOCH_WORLD_LIVE
            .store(pt_epoch, std::sync::atomic::Ordering::Relaxed);
    }
    const U8_MASK: usize = 0xff;
    let read_pgd_u32 = |offset: usize| -> u32 {
        if pgd == NULL_PTR {
            ZERO_U32
        } else {
            unsafe { crate::experiments::safe_read_usize(pgd + offset) }
                .map_or(ZERO_U32, |value| value as u32)
        }
    };
    let read_pgd_u8 = |offset: usize| -> u8 {
        if pgd == NULL_PTR {
            ZERO_U32 as u8
        } else {
            unsafe { crate::experiments::safe_read_usize(pgd + offset) }
                .map_or(ZERO_U32 as u8, |value| (value & U8_MASK) as u8)
        }
    };
    let level = if pgd == NULL_PTR {
        LEVEL_READ_FAIL
    } else {
        i64::from(read_pgd_u32(crate::PGD_LEVEL_68_OFFSET))
    };
    let current_hp = read_pgd_u32(crate::PGD_CURRENT_HP_10_OFFSET);
    let current_max_hp = read_pgd_u32(crate::PGD_CURRENT_MAX_HP_14_OFFSET);
    let base_max_hp = read_pgd_u32(crate::PGD_BASE_MAX_HP_18_OFFSET);
    let current_fp = read_pgd_u32(crate::PGD_CURRENT_FP_1C_OFFSET);
    let current_max_fp = read_pgd_u32(crate::PGD_CURRENT_MAX_FP_20_OFFSET);
    let base_max_fp = read_pgd_u32(crate::PGD_BASE_MAX_FP_24_OFFSET);
    let current_stamina = read_pgd_u32(crate::PGD_CURRENT_STAMINA_2C_OFFSET);
    let current_max_stamina = read_pgd_u32(crate::PGD_CURRENT_MAX_STAMINA_30_OFFSET);
    let base_max_stamina = read_pgd_u32(crate::PGD_BASE_MAX_STAMINA_34_OFFSET);
    let runes = read_pgd_u32(crate::PGD_RUNE_COUNT_6C_OFFSET);
    let rune_memory = read_pgd_u32(crate::PGD_RUNE_MEMORY_70_OFFSET);
    let chr_type = read_pgd_u32(crate::PGD_CHR_TYPE_98_OFFSET);
    let gender = read_pgd_u8(crate::PGD_GENDER_BE_OFFSET);
    let archetype = read_pgd_u8(crate::PGD_ARCHETYPE_BF_OFFSET);
    let voice_type = read_pgd_u8(crate::PGD_VOICE_TYPE_C2_OFFSET);
    let starting_gift = read_pgd_u8(crate::PGD_STARTING_GIFT_C3_OFFSET);
    let unlocked_talisman_slots = read_pgd_u8(crate::PGD_UNLOCKED_TALISMAN_SLOTS_C6_OFFSET);
    let spirit_ash_level = read_pgd_u8(crate::PGD_SPIRIT_ASH_LEVEL_C7_OFFSET);
    const ZERO_U8: u8 = 0;
    let max_crimson_flask_count = read_pgd_u8(crate::PGD_MAX_CRIMSON_FLASK_101_OFFSET);
    let max_cerulean_flask_count = read_pgd_u8(crate::PGD_MAX_CERULEAN_FLASK_102_OFFSET);
    let face_buffer_pgd_offset = crate::PGD_FACE_DATA_OFFSET + crate::FACE_DATA_BUFFER_OFFSET;
    let mut face_data_buffer = [ZERO_U8; crate::FACE_DATA_BUFFER_TOTAL_SIZE];
    let mut face_data_idx = IDX_START;
    while face_data_idx < crate::FACE_DATA_BUFFER_TOTAL_SIZE {
        face_data_buffer[face_data_idx] = read_pgd_u8(face_buffer_pgd_offset + face_data_idx);
        face_data_idx += IDX_STEP;
    }
    let face_data_magic =
        String::from_utf8(face_data_buffer[..crate::FACE_DATA_BUFFER_VERSION_OFFSET].to_vec())
            .unwrap_or_default();
    let face_data_version =
        read_pgd_u32(face_buffer_pgd_offset + crate::FACE_DATA_BUFFER_VERSION_OFFSET);
    let face_data_buffer_size =
        read_pgd_u32(face_buffer_pgd_offset + crate::FACE_DATA_BUFFER_SIZE_OFFSET);
    let mut face_data_buffer_hex = String::new();
    for byte in face_data_buffer {
        use std::fmt::Write as _;
        let _ = write!(&mut face_data_buffer_hex, "{byte:02x}");
    }
    // The five model ids are four bytes each, which is why their offset constants step by
    // `size_of::<u32>()`. `CS::FaceData::ValidateFaceData` (1.16.2 `0x140252610`) range-checks the
    // eight ids leading `FaceDataBuffer::buffer` as `cmp dword ptr [rax],0x0` / `add rax,0x4`,
    // and the er-build-planner slider table types the same eight `{"type": "list", "size": 4}`.
    // Reading one byte truncates every id past 255 -- a real character carries `faceModelId` 500.
    // The static side (`scripts/save-slot-oracle.py`) reads the same five as little-endian u32,
    // so the two `face_body_fields` blocks stay comparable.
    let face_model =
        read_pgd_u32(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_FACE_MODEL_OFFSET);
    let hair_model =
        read_pgd_u32(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_HAIR_MODEL_OFFSET);
    let eyebrow_model =
        read_pgd_u32(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_EYEBROW_MODEL_OFFSET);
    let beard_model =
        read_pgd_u32(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_BEARD_MODEL_OFFSET);
    let eye_patch_model =
        read_pgd_u32(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_EYE_PATCH_MODEL_OFFSET);
    let apparent_age =
        read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_APPARENT_AGE_OFFSET);
    let facial_aesthetic =
        read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_FACIAL_AESTHETIC_OFFSET);
    let form_emphasis =
        read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_FORM_EMPHASIS_OFFSET);
    let head_size = read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_HEAD_SIZE_OFFSET);
    let chest_size = read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_CHEST_SIZE_OFFSET);
    let abdomen_size =
        read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_ABDOMEN_SIZE_OFFSET);
    let arms_size = read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_ARMS_SIZE_OFFSET);
    let legs_size = read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_LEGS_SIZE_OFFSET);
    let skin_color_r =
        read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_SKIN_COLOR_R_OFFSET);
    let skin_color_g =
        read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_SKIN_COLOR_G_OFFSET);
    let skin_color_b =
        read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_SKIN_COLOR_B_OFFSET);
    let face_body_fields = format!(
        "{{\"face_model\": {face_model}, \"hair_model\": {hair_model}, \"eyebrow_model\": {eyebrow_model}, \"beard_model\": {beard_model}, \"eye_patch_model\": {eye_patch_model}, \"apparent_age\": {apparent_age}, \"facial_aesthetic\": {facial_aesthetic}, \"form_emphasis\": {form_emphasis}, \"head_size\": {head_size}, \"chest_size\": {chest_size}, \"abdomen_size\": {abdomen_size}, \"arms_size\": {arms_size}, \"legs_size\": {legs_size}, \"skin_color_r\": {skin_color_r}, \"skin_color_g\": {skin_color_g}, \"skin_color_b\": {skin_color_b}}}"
    );
    let mut name_units = [ZERO_U16; crate::PGD_NAME_LEN_U16];
    let mut name_idx = IDX_START;
    while pgd != NULL_PTR && name_idx < crate::PGD_NAME_LEN_U16 {
        name_units[name_idx] = unsafe {
            crate::experiments::safe_read_usize(
                pgd + crate::PGD_NAME_9C_OFFSET + name_idx * U16_STRIDE,
            )
        }
        .map_or(ZERO_U16, |value| value as u16);
        name_idx += IDX_STEP;
    }
    let mut name_len = IDX_START;
    while name_len < crate::PGD_NAME_LEN_U16 && name_units[name_len] != ZERO_U16 {
        name_len += IDX_STEP;
    }
    let name = String::from_utf16(&name_units[..name_len]).unwrap_or_default();
    let mut stats = [ZERO_U32; crate::PGD_STAT_COUNT];
    let mut stat_idx = IDX_START;
    while stat_idx < crate::PGD_STAT_COUNT {
        stats[stat_idx] = read_pgd_u32(crate::PGD_STAT_BASE_3C_OFFSET + stat_idx * U32_STRIDE);
        stat_idx += IDX_STEP;
    }
    let stat_values = stats.map(|value| value.to_string()).join(", ");
    body.push_str(&format!(
        "  \"oracle_char_current_hp\": {current_hp},\n  \"oracle_char_current_max_hp\": {current_max_hp},\n  \"oracle_char_base_max_hp\": {base_max_hp},\n  \"oracle_char_current_fp\": {current_fp},\n  \"oracle_char_current_max_fp\": {current_max_fp},\n  \"oracle_char_base_max_fp\": {base_max_fp},\n  \"oracle_char_current_stamina\": {current_stamina},\n  \"oracle_char_current_max_stamina\": {current_max_stamina},\n  \"oracle_char_base_max_stamina\": {base_max_stamina},\n  \"oracle_char_level\": {level},\n  \"oracle_char_runes\": {runes},\n  \"oracle_char_rune_memory\": {rune_memory},\n  \"oracle_char_chr_type\": {chr_type},\n  \"oracle_char_gender\": {gender},\n  \"oracle_char_archetype\": {archetype},\n  \"oracle_char_voice_type\": {voice_type},\n  \"oracle_char_starting_gift\": {starting_gift},\n  \"oracle_char_unlocked_talisman_slots\": {unlocked_talisman_slots},\n  \"oracle_char_spirit_ash_level\": {spirit_ash_level},\n  \"oracle_char_max_crimson_flask_count\": {max_crimson_flask_count},\n  \"oracle_char_max_cerulean_flask_count\": {max_cerulean_flask_count},\n  \"oracle_char_name\": \"{}\",\n  \"oracle_char_name_len\": {name_len},\n  \"oracle_play_time_ms\": {play_time_ms},\n  \"oracle_play_time_advanced_ms\": {play_time_advanced_ms},\n  \"oracle_play_time_live\": {play_time_live},\n  \"oracle_char_stats\": [{stat_values}],\n  \"oracle_face_data_magic\": \"{}\",\n  \"oracle_face_data_version\": {face_data_version},\n  \"oracle_face_data_buffer_size\": {face_data_buffer_size},\n  \"oracle_face_data_buffer_hex\": \"{face_data_buffer_hex}\",\n  \"oracle_face_body_fields\": {face_body_fields},\n",
        json_escape(&name),
        json_escape(&face_data_magic)
    ));
    play_time_live
}
