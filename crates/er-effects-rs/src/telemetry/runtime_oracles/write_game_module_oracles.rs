fn write_game_module_oracles(body: &mut String) {
    const GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET: usize = core::mem::offset_of!(GameMan, save_state);
    const GAME_MAN_SAVED_MAP_C30_OFFSET: usize =
        core::mem::offset_of!(GameMan, stay_in_multiplay_area_saved_rotation)
            + core::mem::size_of::<fromsoftware_shared::F32Vector4>()
            + core::mem::size_of::<fromsoftware_shared::F32Vector4>();
    const READ_FAIL_SENTINEL: i32 = -1;
    let format_optional_ptr = format_optional_oracle_ptr;
    // GameMan save-mgr signals: b80 (load-in-progress lane -- the golden-capture mash-stop signal,
    // nonzero once continue is confirmed and the deserialize kicks) + c30 (saved map id, oracle item 2).
    const NULL_PTR: usize = 0;
    if let Ok(base) = crate::experiments::game_module_base() {
        let gm = crate::game_man_ptr_or_null();
        let read_i32 = |addr: usize| -> i32 {
            unsafe { crate::experiments::safe_read_usize(addr) }
                .map_or(READ_FAIL_SENTINEL, |v| v as u32 as i32)
        };
        let (b80, c30) = if gm == NULL_PTR {
            (READ_FAIL_SENTINEL, READ_FAIL_SENTINEL)
        } else {
            (
                read_i32(gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET),
                read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET),
            )
        };
        body.push_str(&format!(
            "  \"oracle_load_in_progress_b80\": {b80},\n  \"oracle_saved_map_c30\": \"{c30:#x}\",\n"
        ));
        // IDENTITY oracle: loaded character values that should match the chosen save slot.
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
        let face_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_FACE_MODEL_OFFSET);
        let hair_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_HAIR_MODEL_OFFSET);
        let eyebrow_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_EYEBROW_MODEL_OFFSET);
        let beard_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_BEARD_MODEL_OFFSET);
        let eye_patch_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_EYE_PATCH_MODEL_OFFSET);
        let apparent_age =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_APPARENT_AGE_OFFSET);
        let facial_aesthetic =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_FACIAL_AESTHETIC_OFFSET);
        let form_emphasis =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_FORM_EMPHASIS_OFFSET);
        let head_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_HEAD_SIZE_OFFSET);
        let chest_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_CHEST_SIZE_OFFSET);
        let abdomen_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_ABDOMEN_SIZE_OFFSET);
        let arms_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_ARMS_SIZE_OFFSET);
        let legs_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_LEGS_SIZE_OFFSET);
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
            "  \"oracle_char_current_hp\": {current_hp},\n  \"oracle_char_current_max_hp\": {current_max_hp},\n  \"oracle_char_base_max_hp\": {base_max_hp},\n  \"oracle_char_current_fp\": {current_fp},\n  \"oracle_char_current_max_fp\": {current_max_fp},\n  \"oracle_char_base_max_fp\": {base_max_fp},\n  \"oracle_char_current_stamina\": {current_stamina},\n  \"oracle_char_current_max_stamina\": {current_max_stamina},\n  \"oracle_char_base_max_stamina\": {base_max_stamina},\n  \"oracle_char_level\": {level},\n  \"oracle_char_runes\": {runes},\n  \"oracle_char_rune_memory\": {rune_memory},\n  \"oracle_char_chr_type\": {chr_type},\n  \"oracle_char_gender\": {gender},\n  \"oracle_char_archetype\": {archetype},\n  \"oracle_char_voice_type\": {voice_type},\n  \"oracle_char_starting_gift\": {starting_gift},\n  \"oracle_char_unlocked_talisman_slots\": {unlocked_talisman_slots},\n  \"oracle_char_spirit_ash_level\": {spirit_ash_level},\n  \"oracle_char_max_crimson_flask_count\": {max_crimson_flask_count},\n  \"oracle_char_max_cerulean_flask_count\": {max_cerulean_flask_count},\n  \"oracle_char_name\": \"{}\",\n  \"oracle_char_name_len\": {name_len},\n  \"oracle_char_stats\": [{stat_values}],\n  \"oracle_face_data_magic\": \"{}\",\n  \"oracle_face_data_version\": {face_data_version},\n  \"oracle_face_data_buffer_size\": {face_data_buffer_size},\n  \"oracle_face_data_buffer_hex\": \"{face_data_buffer_hex}\",\n  \"oracle_face_body_fields\": {face_body_fields},\n",
            json_escape(&name),
            json_escape(&face_data_magic)
        ));
        // WORLD-LIVE oracle: CSNowLoadingHelper "now loading" latch = *(u8*)([base+0x3d60ec8]+0xED).
        // NOTE (RE-corrected 2026-07-02): this reads `CSNowLoadingHelperImp::load_done` -- a load-COMPLETE
        // latch, NOT "loading screen visible." `Update` copies it from `request_load_done` (raised by the
        // map-load system), so it reads true AFTER the load finishes and lingers into gameplay. Kept as a
        // telemetry field, but do not treat it as a screen-visibility signal (see CSNowLoadingHelperImp).
        const NOW_LOADING_SINGLETON_RVA: usize = RuntimeGlobalRva::NowLoadingSingleton as usize;
        const NOW_LOADING_FLAG_OFFSET: usize =
            core::mem::offset_of!(CSNowLoadingHelperImp, load_done);
        const NOW_LOADING_BYTE_MASK: usize = u8::MAX as usize;
        let now_loading = {
            let helper =
                unsafe { crate::experiments::safe_read_usize(base + NOW_LOADING_SINGLETON_RVA) }
                    .unwrap_or(NULL_PTR);
            if helper == NULL_PTR {
                READ_FAIL_SENTINEL
            } else {
                unsafe { crate::experiments::safe_read_usize(helper + NOW_LOADING_FLAG_OFFSET) }
                    .map_or(READ_FAIL_SENTINEL, |v| (v & NOW_LOADING_BYTE_MASK) as i32)
            }
        };
        const FAKE_LOADING_SCREEN_SINGLETON_RVA: usize =
            RuntimeGlobalRva::FakeLoadingScreenSingleton as usize;
        let fake_loading_screen = unsafe {
            crate::experiments::safe_read_usize(base + FAKE_LOADING_SCREEN_SINGLETON_RVA)
        }
        .unwrap_or(NULL_PTR);
        let fake_loading_visible = if fake_loading_screen == NULL_PTR {
            READ_FAIL_SENTINEL
        } else {
            unsafe { crate::experiments::safe_read_usize(fake_loading_screen + 0x8) }
                .map_or(READ_FAIL_SENTINEL, |v| (v & NOW_LOADING_BYTE_MASK) as i32)
        };
        let fake_loading_field_c = if fake_loading_screen == NULL_PTR {
            READ_FAIL_SENTINEL
        } else {
            unsafe { crate::experiments::safe_read_usize(fake_loading_screen + 0xc) }
                .map_or(READ_FAIL_SENTINEL, |v| v as u32 as i32)
        };
        let fake_loading_field_10 = if fake_loading_screen == NULL_PTR {
            READ_FAIL_SENTINEL
        } else {
            unsafe { crate::experiments::safe_read_usize(fake_loading_screen + 0x10) }
                .map_or(READ_FAIL_SENTINEL, |v| v as u32 as i32)
        };
        if fake_loading_screen != NULL_PTR {
            FAKE_LOADING_SCREEN_SAMPLE_COUNT.fetch_add(1, Ordering::SeqCst);
            FAKE_LOADING_SCREEN_LAST_PTR.store(fake_loading_screen, Ordering::SeqCst);
            FAKE_LOADING_SCREEN_LAST_VISIBLE
                .store(fake_loading_visible.max(0) as usize, Ordering::SeqCst);
            FAKE_LOADING_SCREEN_LAST_FIELD_C
                .store(fake_loading_field_c.max(0) as usize, Ordering::SeqCst);
            FAKE_LOADING_SCREEN_LAST_FIELD_10
                .store(fake_loading_field_10.max(0) as usize, Ordering::SeqCst);
            if fake_loading_visible > 0 {
                FAKE_LOADING_SCREEN_VISIBLE_SAMPLES.fetch_add(1, Ordering::SeqCst);
            }
        }
        let fake_loading_samples = FAKE_LOADING_SCREEN_SAMPLE_COUNT.load(Ordering::SeqCst);
        let fake_loading_visible_samples =
            FAKE_LOADING_SCREEN_VISIBLE_SAMPLES.load(Ordering::SeqCst);
        const RENDMAN_SINGLETON_RVA: usize = RuntimeGlobalRva::RendManSingleton as usize;
        const CSGRAPHICS_SINGLETON_RVA: usize = RuntimeGlobalRva::CsGraphicsSingleton as usize;
        const CSSCALEFORM_SINGLETON_RVA: usize = RuntimeGlobalRva::CsScaleformSingleton as usize;
        let rendman = unsafe { crate::experiments::safe_read_usize(base + RENDMAN_SINGLETON_RVA) }
            .unwrap_or(NULL_PTR);
        let csgraphics =
            unsafe { crate::experiments::safe_read_usize(base + CSGRAPHICS_SINGLETON_RVA) }
                .unwrap_or(NULL_PTR);
        let csscaleform =
            unsafe { crate::experiments::safe_read_usize(base + CSSCALEFORM_SINGLETON_RVA) }
                .unwrap_or(NULL_PTR);
        let read_rend = |offset: usize| -> usize {
            if rendman == NULL_PTR {
                NULL_PTR
            } else {
                unsafe { crate::experiments::safe_read_usize(rendman + offset) }.unwrap_or(NULL_PTR)
            }
        };
        let rend_slot_28 = read_rend(0x28);
        let rend_slot_30 = read_rend(0x30);
        let rend_slot_38 = read_rend(0x38);
        let rend_slot_40 = read_rend(0x40);
        let rend_slot_78 = read_rend(0x78);
        let rendman_pause = if rendman == NULL_PTR {
            READ_FAIL_SENTINEL
        } else {
            unsafe { crate::experiments::safe_read_usize(rendman + 0x90) }
                .map_or(READ_FAIL_SENTINEL, |v| (v & NOW_LOADING_BYTE_MASK) as i32)
        };
        let csgraphics_field68 = if csgraphics == NULL_PTR {
            NULL_PTR
        } else {
            unsafe { crate::experiments::safe_read_usize(csgraphics + 0x68) }.unwrap_or(NULL_PTR)
        };
        let mut slots_mask = 0usize;
        if rend_slot_28 != NULL_PTR {
            slots_mask |= 1 << 0;
        }
        if rend_slot_30 != NULL_PTR {
            slots_mask |= 1 << 1;
        }
        if rend_slot_38 != NULL_PTR {
            slots_mask |= 1 << 2;
        }
        if rend_slot_40 != NULL_PTR {
            slots_mask |= 1 << 3;
        }
        if rend_slot_78 != NULL_PTR {
            slots_mask |= 1 << 4;
        }
        if csgraphics_field68 != NULL_PTR {
            slots_mask |= 1 << 5;
        }
        if csscaleform != NULL_PTR {
            slots_mask |= 1 << 6;
        }
        RENDER_LOADING_LAYER_LAST_RENDMAN.store(rendman, Ordering::SeqCst);
        RENDER_LOADING_LAYER_LAST_CSGRAPHICS.store(csgraphics, Ordering::SeqCst);
        RENDER_LOADING_LAYER_LAST_CSSCALEFORM.store(csscaleform, Ordering::SeqCst);
        RENDER_LOADING_LAYER_LAST_SLOTS_MASK.store(slots_mask, Ordering::SeqCst);
        if fake_loading_visible > 0 {
            RENDER_LOADING_LAYER_SAMPLE_COUNT.fetch_add(1, Ordering::SeqCst);
            if slots_mask != 0 {
                RENDER_LOADING_LAYER_NONNULL_SAMPLES.fetch_add(1, Ordering::SeqCst);
            }
            RENDER_LOADING_LAYER_VISIBLE_SLOTS_MASK.fetch_or(slots_mask, Ordering::SeqCst);
        }
        let render_loading_samples = RENDER_LOADING_LAYER_SAMPLE_COUNT.load(Ordering::SeqCst);
        let render_loading_nonnull_samples =
            RENDER_LOADING_LAYER_NONNULL_SAMPLES.load(Ordering::SeqCst);
        let render_loading_visible_slots_mask =
            RENDER_LOADING_LAYER_VISIBLE_SLOTS_MASK.load(Ordering::SeqCst);
        body.push_str(&format!(
            "  \"oracle_now_loading\": {now_loading},\n  \"oracle_fake_loading_screen\": {},\n  \"oracle_fake_loading_visible\": {fake_loading_visible},\n  \"oracle_fake_loading_field_c\": {fake_loading_field_c},\n  \"oracle_fake_loading_field_10\": {fake_loading_field_10},\n  \"oracle_fake_loading_sample_count\": {fake_loading_samples},\n  \"oracle_fake_loading_visible_samples\": {fake_loading_visible_samples},\n  \"oracle_fake_loading_any_visible\": {},\n  \"oracle_render_loading_rendman\": {},\n  \"oracle_render_loading_csgraphics\": {},\n  \"oracle_render_loading_csscaleform\": {},\n  \"oracle_render_loading_rendman_pause\": {rendman_pause},\n  \"oracle_render_loading_slot_28\": {},\n  \"oracle_render_loading_slot_30\": {},\n  \"oracle_render_loading_slot_38\": {},\n  \"oracle_render_loading_slot_40\": {},\n  \"oracle_render_loading_slot_78\": {},\n  \"oracle_render_loading_csgraphics_field68\": {},\n  \"oracle_render_loading_last_slots_mask\": {slots_mask},\n  \"oracle_render_loading_visible_slots_mask\": {render_loading_visible_slots_mask},\n  \"oracle_render_loading_sample_count\": {render_loading_samples},\n  \"oracle_render_loading_nonnull_samples\": {render_loading_nonnull_samples},\n",
            format_optional_ptr(fake_loading_screen),
            fake_loading_visible_samples > 0,
            format_optional_ptr(rendman),
            format_optional_ptr(csgraphics),
            format_optional_ptr(csscaleform),
            format_optional_ptr(rend_slot_28),
            format_optional_ptr(rend_slot_30),
            format_optional_ptr(rend_slot_38),
            format_optional_ptr(rend_slot_40),
            format_optional_ptr(rend_slot_78),
            format_optional_ptr(csgraphics_field68),
        ));
        let msgbox_dialog = MSGBOX_LAST_DIALOG.load(Ordering::SeqCst);
        let msgbox_vtable = if msgbox_dialog == NULL_PTR {
            NULL_PTR
        } else {
            unsafe { crate::experiments::safe_read_usize(msgbox_dialog) }.unwrap_or(NULL_PTR)
        };
        let msgbox_closing_latch = if msgbox_vtable == base + MSGBOX_DIALOG_VTABLE_RVA {
            unsafe {
                crate::experiments::safe_read_usize(msgbox_dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET)
            }
            .map(|value| value & MSGBOX_LATCH_BYTE_MASK)
            .unwrap_or(MSGBOX_CLOSING_YES)
        } else {
            MSGBOX_CLOSING_YES
        };
        const NO_MSGBOX_BUILDS: usize = MENU_TRACE_UNSEEN_SEQ;
        let msgbox_total_builds = MSGBOX_TOTAL_BUILDS.load(Ordering::SeqCst);
        let msgbox_postload_builds = MSGBOX_POSTLOAD_BUILDS.load(Ordering::SeqCst);
        let msgbox_any_seen = msgbox_total_builds != NO_MSGBOX_BUILDS;
        let postload_modal_seen = msgbox_postload_builds != NO_MSGBOX_BUILDS;
        let blocking_modal_present = msgbox_vtable == base + MSGBOX_DIALOG_VTABLE_RVA
            && msgbox_closing_latch != MSGBOX_CLOSING_YES;
        let msgbox_arg_rcx = MSGBOX_LAST_ARG_RCX.load(Ordering::SeqCst);
        let msgbox_arg_rdx = MSGBOX_LAST_ARG_RDX.load(Ordering::SeqCst);
        let msgbox_arg_r8 = MSGBOX_LAST_ARG_R8.load(Ordering::SeqCst);
        let msgbox_arg_r9 = MSGBOX_LAST_ARG_R9.load(Ordering::SeqCst);
        let policy_total_builds = POLICY_TOS_TITLE_TOTAL_BUILDS.load(Ordering::SeqCst);
        let policy_any_seen = policy_total_builds != NO_MSGBOX_BUILDS;
        let policy_ptr = POLICY_TOS_TITLE_LAST_THIS.load(Ordering::SeqCst);
        let policy_vtable = POLICY_TOS_TITLE_LAST_VTABLE.load(Ordering::SeqCst);
        let policy_arg_rdx = POLICY_TOS_TITLE_LAST_ARG_RDX.load(Ordering::SeqCst);
        let policy_arg_r8 = POLICY_TOS_TITLE_LAST_ARG_R8.load(Ordering::SeqCst);
        let policy_arg_r9 = POLICY_TOS_TITLE_LAST_ARG_R9.load(Ordering::SeqCst);
        let policy_stack_arg0 = POLICY_TOS_TITLE_LAST_STACK_ARG0.load(Ordering::SeqCst);
        let policy_backing_flag_ptr = POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR.load(Ordering::SeqCst);
        let policy_stored_backing_flag_ptr =
            POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR.load(Ordering::SeqCst);
        let policy_backing_flag_value =
            POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE.load(Ordering::SeqCst);
        let policy_requested_flag_value =
            POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst);
        let policy_caller_rva = POLICY_TOS_TITLE_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_wrapper_hits = POLICY_TOS_TITLE_WRAPPER_HITS.load(Ordering::SeqCst);
        let policy_wrapper_record = POLICY_TOS_TITLE_WRAPPER_LAST_RECORD.load(Ordering::SeqCst);
        let policy_wrapper_original_this =
            POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS.load(Ordering::SeqCst);
        let policy_wrapper_original_vtable =
            POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE.load(Ordering::SeqCst);
        let policy_wrapper_record_id =
            POLICY_TOS_TITLE_WRAPPER_LAST_RECORD_ID.load(Ordering::SeqCst);
        let policy_wrapper_stack_arg0 =
            POLICY_TOS_TITLE_WRAPPER_LAST_STACK_ARG0.load(Ordering::SeqCst);
        let policy_wrapper_backing_flag_ptr =
            POLICY_TOS_TITLE_WRAPPER_LAST_BACKING_FLAG_PTR.load(Ordering::SeqCst);
        let policy_wrapper_ret = POLICY_TOS_TITLE_WRAPPER_LAST_RET.load(Ordering::SeqCst);
        let policy_wrapper_caller_rva =
            POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_selector_hits = POLICY_TOS_SELECTOR_WRAPPER_HITS.load(Ordering::SeqCst);
        let policy_selector_record = POLICY_TOS_SELECTOR_WRAPPER_LAST_RECORD.load(Ordering::SeqCst);
        let policy_selector_original_this =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_THIS.load(Ordering::SeqCst);
        let policy_selector_original_vtable =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_VTABLE.load(Ordering::SeqCst);
        let policy_selector_owner = POLICY_TOS_SELECTOR_WRAPPER_LAST_OWNER.load(Ordering::SeqCst);
        let policy_selector_requested_flag =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG.load(Ordering::SeqCst);
        let policy_selector_arg =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG.load(Ordering::SeqCst);
        let policy_selector_ret = POLICY_TOS_SELECTOR_WRAPPER_LAST_RET.load(Ordering::SeqCst);
        let policy_selector_caller_rva =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_selector_ctor_hits = POLICY_TOS_SELECTOR_CTOR_HITS.load(Ordering::SeqCst);
        let policy_selector_ctor_this = POLICY_TOS_SELECTOR_CTOR_LAST_THIS.load(Ordering::SeqCst);
        let policy_selector_ctor_vtable =
            POLICY_TOS_SELECTOR_CTOR_LAST_VTABLE.load(Ordering::SeqCst);
        let policy_selector_ctor_owner = POLICY_TOS_SELECTOR_CTOR_LAST_OWNER.load(Ordering::SeqCst);
        let policy_selector_ctor_requested_flag_ptr =
            POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR.load(Ordering::SeqCst);
        let policy_selector_ctor_requested_flag_value =
            POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst);
        let policy_selector_ctor_selector_arg =
            POLICY_TOS_SELECTOR_CTOR_LAST_SELECTOR_ARG.load(Ordering::SeqCst);
        let policy_selector_ctor_stored_selector_arg =
            POLICY_TOS_SELECTOR_CTOR_LAST_STORED_SELECTOR_ARG.load(Ordering::SeqCst);
        let policy_selector_ctor_stored_requested_flag_ptr =
            POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR.load(Ordering::SeqCst);
        let policy_selector_ctor_ret = POLICY_TOS_SELECTOR_CTOR_LAST_RET.load(Ordering::SeqCst);
        let policy_selector_ctor_caller_rva =
            POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_status_hits = POLICY_TOS_STATUS_HITS.load(Ordering::SeqCst);
        let policy_status_this = POLICY_TOS_STATUS_LAST_THIS.load(Ordering::SeqCst);
        let policy_status_owner = POLICY_TOS_STATUS_LAST_OWNER.load(Ordering::SeqCst);
        let policy_status_flag_ptr = POLICY_TOS_STATUS_LAST_FLAG_PTR.load(Ordering::SeqCst);
        let policy_status_flag_value = POLICY_TOS_STATUS_LAST_FLAG_VALUE.load(Ordering::SeqCst);
        let policy_status_ret = POLICY_TOS_STATUS_LAST_RET.load(Ordering::SeqCst);
        let policy_status_caller_rva = POLICY_TOS_STATUS_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_flag_setter_hits = POLICY_TOS_FLAG_SETTER_HITS.load(Ordering::SeqCst);
        let policy_flag_setter_owner = POLICY_TOS_FLAG_SETTER_LAST_OWNER.load(Ordering::SeqCst);
        let policy_flag_setter_value = POLICY_TOS_FLAG_SETTER_LAST_VALUE.load(Ordering::SeqCst);
        let policy_flag_setter_force = POLICY_TOS_FLAG_SETTER_LAST_FORCE.load(Ordering::SeqCst);
        let policy_flag_setter_before = POLICY_TOS_FLAG_SETTER_LAST_BEFORE.load(Ordering::SeqCst);
        let policy_flag_setter_after = POLICY_TOS_FLAG_SETTER_LAST_AFTER.load(Ordering::SeqCst);
        let policy_flag_setter_caller_rva =
            POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let server_status_total_seen = SERVER_STATUS_TOTAL_SEEN.load(Ordering::SeqCst);
        let server_status_any_seen = server_status_total_seen != NO_MSGBOX_BUILDS;
        let server_status_state = SERVER_STATUS_LAST_STATE.load(Ordering::SeqCst);
        let server_status_text_id = SERVER_STATUS_LAST_TEXT_ID.load(Ordering::SeqCst);
        let title_visual_suppress_installed = TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED
            .load(Ordering::SeqCst)
            == TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED_YES;
        let title_visual_suppressed_builds =
            TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS.load(Ordering::SeqCst);
        let title_visual_last_out_slot =
            TITLE_NATIVE_MENU_VISUAL_LAST_OUT_SLOT.load(Ordering::SeqCst);
        let title_visual_last_prev_out =
            TITLE_NATIVE_MENU_VISUAL_LAST_PREV_OUT.load(Ordering::SeqCst);
        let title_visual_last_arg_rdx =
            TITLE_NATIVE_MENU_VISUAL_LAST_ARG_RDX.load(Ordering::SeqCst);
        let title_visual_last_arg_r8 = TITLE_NATIVE_MENU_VISUAL_LAST_ARG_R8.load(Ordering::SeqCst);
        let title_visual_last_caller_rva =
            TITLE_NATIVE_MENU_VISUAL_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_visual_native_job = TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB.load(Ordering::SeqCst);
        let title_visual_native_window =
            TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW.load(Ordering::SeqCst);
        let title_visual_render_suppress_installed =
            TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED.load(Ordering::SeqCst)
                == TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED_YES;
        let title_visual_render_suppressed_windows =
            TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESSED_WINDOWS.load(Ordering::SeqCst);
        let title_visual_render_last_window =
            TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_WINDOW.load(Ordering::SeqCst);
        let title_visual_render_last_flags_before =
            TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_BEFORE.load(Ordering::SeqCst);
        let title_visual_render_last_flags_after =
            TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_AFTER.load(Ordering::SeqCst);
        let title_visual_render_last_caller_rva =
            TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let (
            title_visual_current_menu_id,
            title_visual_current_flags,
            title_visual_current_draw_bit_set,
        ) = title_menu_window_id_flags(base, title_visual_native_window);
        // Actual visible logo surface telemetry: `TitleBackViewParts` / `05_001_Title_Logo` is an
        // embedded object at TitleTopDialog+0xaa8, separate from the preserved `05_000_Title`
        // MenuWindowJob. A real portrait cover depends on post-SL2 profile_summary readiness and the
        // SYSTEX_Menu_Profile render pipeline, so expose both in RAM telemetry before any mutation.
        // STALE-DIALOG UAF GUARD (er-effects-rs-3pc, ROOT fix 2026-07-03). `title_logo_gfx_current_frame`
        // CALLS a virtual on the title dialog's BackViewParts GFX handle. The title logo only exists at
        // the title screen; once we have loaded into a world that stored dialog is FREED (and, on every
        // character switch, freed+rebuilt). A freed object keeps its vtable, and worse, its reused
        // vtable+8 slot can point at a VALID-BUT-WRONG game function (observed: the factory FUN_1411d10f0),
        // so the earlier `vtable_in_game_image` check passes and the call still derefs freed memory ->
        // access violation deep in the game (crash write_oracle_telemetry -> game+0x11d10f3). You cannot
        // safely virtual-call a maybe-freed object. So skip this GFX walk entirely once in-world: the
        // oracle is a boot-title diagnostic and is meaningless (and unsafe) after the first load. This is
        // what actually surfaced as the "crash on opening escape after N switches" -- the telemetry tick,
        // not the menu, dereferencing the stale title dialog.
        let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
        let title_logo_dialog = PRODUCT_CORE_LAST_TITLE_DIALOG.load(Ordering::SeqCst);
        let title_logo_back_view_parts = if !in_world
            && title_logo_dialog != NULL_PTR
            && title_logo_dialog != TITLE_OWNER_SCAN_START_ADDRESS
        {
            title_logo_dialog + TITLE_LOGO_BACK_VIEW_PARTS_AA8_OFFSET
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let title_logo_back_view_parts_vtable =
            if title_logo_back_view_parts != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { crate::experiments::safe_read_usize(title_logo_back_view_parts) }
                    .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            };
        let title_logo_gfx_frame =
            if title_logo_back_view_parts_vtable != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { title_logo_gfx_current_frame(base, title_logo_back_view_parts) }
            } else {
                TITLE_LOGO_GFX_UNKNOWN_FRAME
            };
        let title_logo_gfx_alpha_mult_term = title_logo_gfx_alpha_for_frame(title_logo_gfx_frame);
        let title_logo_gfx_visibility = title_logo_gfx_alpha_mult_term > 0;
        let title_logo_gfx_hide_calls = TITLE_LOGO_GFX_HIDE_CALLS.load(Ordering::SeqCst);
        let title_logo_gfx_hide_last_dialog =
            TITLE_LOGO_GFX_HIDE_LAST_DIALOG.load(Ordering::SeqCst);
        let title_logo_gfx_hide_last_logo = TITLE_LOGO_GFX_HIDE_LAST_LOGO.load(Ordering::SeqCst);
        let title_logo_gfx_hide_last_caller_phase =
            TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE.load(Ordering::SeqCst);
        let title_logo_gfx_hide_last_requested_visible =
            TITLE_LOGO_GFX_HIDE_LAST_REQUESTED_VISIBLE.load(Ordering::SeqCst);
        let title_menu_resource_acquire_installed =
            TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED.load(Ordering::SeqCst) != 0;
        let title_menu_resource_acquire_hits =
            TITLE_MENU_RESOURCE_ACQUIRE_HITS.load(Ordering::SeqCst);
        let title_menu_resource_acquire_logo_hits =
            TITLE_MENU_RESOURCE_ACQUIRE_LOGO_HITS.load(Ordering::SeqCst);
        let title_menu_resource_acquire_last_this =
            TITLE_MENU_RESOURCE_ACQUIRE_LAST_THIS.load(Ordering::SeqCst);
        let title_menu_resource_acquire_last_load_params =
            TITLE_MENU_RESOURCE_ACQUIRE_LAST_LOAD_PARAMS.load(Ordering::SeqCst);
        let title_menu_resource_acquire_last_filename_ptr =
            TITLE_MENU_RESOURCE_ACQUIRE_LAST_FILENAME_PTR.load(Ordering::SeqCst);
        let title_menu_resource_acquire_last_param3 =
            TITLE_MENU_RESOURCE_ACQUIRE_LAST_PARAM3.load(Ordering::SeqCst);
        let title_menu_resource_acquire_last_ret =
            TITLE_MENU_RESOURCE_ACQUIRE_LAST_RET.load(Ordering::SeqCst);
        let title_menu_resource_acquire_last_caller_rva =
            TITLE_MENU_RESOURCE_ACQUIRE_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_scaleform_file_open_installed =
            TITLE_SCALEFORM_FILE_OPEN_INSTALLED.load(Ordering::SeqCst) != 0;
        let title_scaleform_file_open_hits = TITLE_SCALEFORM_FILE_OPEN_HITS.load(Ordering::SeqCst);
        let title_scaleform_file_open_logo_hits =
            TITLE_SCALEFORM_FILE_OPEN_LOGO_HITS.load(Ordering::SeqCst);
        let title_scaleform_file_open_last_loader =
            TITLE_SCALEFORM_FILE_OPEN_LAST_LOADER.load(Ordering::SeqCst);
        let title_scaleform_file_open_last_url_ptr =
            TITLE_SCALEFORM_FILE_OPEN_LAST_URL_PTR.load(Ordering::SeqCst);
        let title_scaleform_file_open_last_flags =
            TITLE_SCALEFORM_FILE_OPEN_LAST_FLAGS.load(Ordering::SeqCst);
        let title_scaleform_file_open_last_ret =
            TITLE_SCALEFORM_FILE_OPEN_LAST_RET.load(Ordering::SeqCst);
        let title_scaleform_file_open_last_ret_vtable =
            TITLE_SCALEFORM_FILE_OPEN_LAST_RET_VTABLE.load(Ordering::SeqCst);
        let title_scaleform_file_open_last_caller_rva =
            TITLE_SCALEFORM_FILE_OPEN_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_scaleform_memory_gfx_bytes =
            TITLE_SCALEFORM_MEMORY_GFX_BYTES.load(Ordering::SeqCst);
        let title_scaleform_memory_gfx_replacements =
            TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS.load(Ordering::SeqCst);
        let title_scaleform_05_000_memory_gfx_replacements =
            TITLE_SCALEFORM_05_000_MEMORY_GFX_REPLACEMENTS.load(Ordering::SeqCst);
        let title_scaleform_memory_gfx_failures =
            TITLE_SCALEFORM_MEMORY_GFX_FAILURES.load(Ordering::SeqCst);
        let title_scaleform_memory_gfx_last_file =
            TITLE_SCALEFORM_MEMORY_GFX_LAST_FILE.load(Ordering::SeqCst);
        let title_scaleform_resource_ctor_installed =
            TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED.load(Ordering::SeqCst) != 0;
        let title_scaleform_resource_ctor_hits =
            TITLE_SCALEFORM_RESOURCE_CTOR_HITS.load(Ordering::SeqCst);
        let title_scaleform_resource_ctor_logo_hits =
            TITLE_SCALEFORM_RESOURCE_CTOR_LOGO_HITS.load(Ordering::SeqCst);
        let title_scaleform_resource_ctor_last_out =
            TITLE_SCALEFORM_RESOURCE_CTOR_LAST_OUT.load(Ordering::SeqCst);
        let title_scaleform_resource_ctor_last_url_ptr =
            TITLE_SCALEFORM_RESOURCE_CTOR_LAST_URL_PTR.load(Ordering::SeqCst);
        let title_scaleform_resource_ctor_last_file =
            TITLE_SCALEFORM_RESOURCE_CTOR_LAST_FILE.load(Ordering::SeqCst);
        let title_scaleform_resource_ctor_last_ret =
            TITLE_SCALEFORM_RESOURCE_CTOR_LAST_RET.load(Ordering::SeqCst);
        let title_scaleform_resource_ctor_last_movie_data =
            TITLE_SCALEFORM_RESOURCE_CTOR_LAST_MOVIE_DATA.load(Ordering::SeqCst);
        let title_scaleform_resource_ctor_last_caller_rva =
            TITLE_SCALEFORM_RESOURCE_CTOR_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_calls =
            TITLE_PRESS_START_GFX_HIDE_CALLS.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_last_dialog =
            TITLE_PRESS_START_GFX_HIDE_LAST_DIALOG.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_last_proxy =
            TITLE_PRESS_START_GFX_HIDE_LAST_PROXY.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_last_context =
            TITLE_PRESS_START_GFX_HIDE_LAST_CONTEXT.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_last_caller_phase =
            TITLE_PRESS_START_GFX_HIDE_LAST_CALLER_PHASE.load(Ordering::SeqCst);
        let title_press_start_gfx_value = TITLE_PRESS_START_GFX_VALUE.load(Ordering::SeqCst);
        let title_press_start_gfx_force_false_calls =
            TITLE_PRESS_START_GFX_FORCE_FALSE_CALLS.load(Ordering::SeqCst);
        let title_press_start_gfx_force_false_last_value =
            TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_VALUE.load(Ordering::SeqCst);
        let title_press_start_gfx_force_false_last_requested =
            TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_REQUESTED.load(Ordering::SeqCst);
        let title_press_start_bind_hits = TITLE_PRESS_START_BIND_HITS.load(Ordering::SeqCst);
        let title_press_start_bind_last_parent =
            TITLE_PRESS_START_BIND_LAST_PARENT.load(Ordering::SeqCst);
        let title_press_start_bind_last_out =
            TITLE_PRESS_START_BIND_LAST_OUT.load(Ordering::SeqCst);
        let title_press_start_bind_last_name =
            TITLE_PRESS_START_BIND_LAST_NAME.load(Ordering::SeqCst);
        let title_press_start_bind_last_context =
            TITLE_PRESS_START_BIND_LAST_CONTEXT.load(Ordering::SeqCst);
        let title_press_start_bind_hide_calls =
            TITLE_PRESS_START_BIND_HIDE_CALLS.load(Ordering::SeqCst);
        // Real false until a later mutation binds the post-SL2 profile/SYSTEX portrait to the
        // 05_001_Title_Logo root-depth-3 surface. `05_010_ProfileSelect` dummy faces are exported
        // bitmap classes only (0 timeline placements), so profile_summary readiness alone is not a
        // visible cover binding.
        let title_profile_cover_bound_to_logo_surface = false;
        let title_overlay_cover_render_calls =
            TITLE_OVERLAY_COVER_RENDER_CALLS.load(Ordering::SeqCst);
        let title_overlay_cover_last_display_w =
            TITLE_OVERLAY_COVER_LAST_DISPLAY_W.load(Ordering::SeqCst);
        let title_overlay_cover_last_display_h =
            TITLE_OVERLAY_COVER_LAST_DISPLAY_H.load(Ordering::SeqCst);
        let title_overlay_cover_display_sane =
            title_overlay_cover_last_display_w >= 200 && title_overlay_cover_last_display_h >= 200;
        let title_overlay_cover_texture_bound =
            TITLE_OVERLAY_COVER_TEXTURE_BOUND.load(Ordering::SeqCst) != 0;
        let title_overlay_cover_last_gx_texture =
            TITLE_OVERLAY_COVER_LAST_GX_TEXTURE.load(Ordering::SeqCst);
        let title_overlay_cover_last_texture_resource =
            TITLE_OVERLAY_COVER_LAST_TEXTURE_RESOURCE.load(Ordering::SeqCst);
        let title_logo_profile_summary = {
            let game_data_man = crate::game_data_man_ptr_or_null();
            if game_data_man != NULL_PTR {
                unsafe {
                    crate::experiments::safe_read_usize(
                        game_data_man + SLOT_MANAGER_CONTAINER_OFFSET,
                    )
                }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let title_logo_profile_summary_ready = title_logo_profile_summary
            != TITLE_OWNER_SCAN_START_ADDRESS
            && title_logo_profile_summary != NULL_PTR;
        let title_profile_render_refresh_gate_ready = unsafe {
            product_core_autoload_ready(
                PRODUCT_CORE_LAST_OWNER.load(Ordering::SeqCst),
                base,
                game_man_ptr_or_null(),
                OWN_STEPPER_SLOT_ZERO,
            )
        }
        .is_some();
        let title_profile_face_bind_hits = TITLE_PROFILE_FACE_BIND_HITS.load(Ordering::SeqCst);
        let title_profile_face_transform_applied =
            TITLE_PROFILE_FACE_TRANSFORM_APPLIED.load(Ordering::SeqCst) != 0;
        let title_profile_face_other_hidden =
            TITLE_PROFILE_FACE_OTHER_HIDDEN.load(Ordering::SeqCst);
        let title_profile_face_last_proxy = TITLE_PROFILE_FACE_LAST_PROXY.load(Ordering::SeqCst);
        let title_profile_face_last_value = TITLE_PROFILE_FACE_LAST_VALUE.load(Ordering::SeqCst);
        let title_loaded_character_portrait_rendered = title_profile_face_bind_hits != 0
            && title_profile_face_transform_applied
            && title_profile_face_other_hidden >= 9
            && TITLE_SCALEFORM_BIND_OBSERVER_SYSTEX_HITS.load(Ordering::SeqCst) >= 10
            && TITLE_CUSTOM_COVER_RUN_CALLS.load(Ordering::SeqCst) == 1;
        let title_loaded_character_portrait_visible_during_boot =
            title_loaded_character_portrait_rendered
                && TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS.load(Ordering::SeqCst) != 0
                && TITLE_LOGO_GFX_HIDE_CALLS.load(Ordering::SeqCst) != 0;
        let title_loaded_character_portrait_held_until_loading_takeover =
            title_loaded_character_portrait_rendered
                && (RENDER_LOADING_LAYER_NONNULL_SAMPLES.load(Ordering::SeqCst) != 0
                    || RENDER_LOADING_LAYER_VISIBLE_SLOTS_MASK.load(Ordering::SeqCst) != 0);
        let title_scaleform_bind_observer_hits =
            TITLE_SCALEFORM_BIND_OBSERVER_HITS.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_systex_hits =
            TITLE_SCALEFORM_BIND_OBSERVER_SYSTEX_HITS.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_last_owner =
            TITLE_SCALEFORM_BIND_OBSERVER_LAST_OWNER.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_last_pair =
            TITLE_SCALEFORM_BIND_OBSERVER_LAST_PAIR.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_last_symbol_ptr =
            TITLE_SCALEFORM_BIND_OBSERVER_LAST_SYMBOL_PTR.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_last_target_ptr =
            TITLE_SCALEFORM_BIND_OBSERVER_LAST_TARGET_PTR.load(Ordering::SeqCst);
        let title_profile_visible_surface_bind_rewrites =
            TITLE_PROFILE_VISIBLE_SURFACE_BIND_REWRITES.load(Ordering::SeqCst);
        let title_profile_visible_surface_bind_last_owner =
            TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_OWNER.load(Ordering::SeqCst);
        let title_profile_visible_surface_bind_last_pair =
            TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_PAIR.load(Ordering::SeqCst);
        let title_profile_visible_surface_bind_last_symbol_ptr =
            TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_SYMBOL_PTR.load(Ordering::SeqCst);
        let now_loading_helper_hooks_installed =
            NOW_LOADING_HELPER_HOOKS_INSTALLED.load(Ordering::SeqCst);
        let now_loading_helper_ctor_hits = NOW_LOADING_HELPER_CTOR_HITS.load(Ordering::SeqCst);
        let now_loading_helper_update_hits = NOW_LOADING_HELPER_UPDATE_HITS.load(Ordering::SeqCst);
        let now_loading_helper_last_this = NOW_LOADING_HELPER_LAST_THIS.load(Ordering::SeqCst);
        let now_loading_helper_last_menu_index =
            NOW_LOADING_HELPER_LAST_MENU_INDEX.load(Ordering::SeqCst);
        let now_loading_helper_last_replace_tex_info =
            NOW_LOADING_HELPER_LAST_REPLACE_TEX_INFO.load(Ordering::SeqCst);
        let now_loading_helper_last_requested_replace_tex_info =
            NOW_LOADING_HELPER_LAST_REQUESTED_REPLACE_TEX_INFO.load(Ordering::SeqCst);
        let now_loading_helper_last_flags = NOW_LOADING_HELPER_LAST_FLAGS.load(Ordering::SeqCst);
        let loading_bg_portrait_redirect_installed =
            LOADING_BG_TEXTURE_REDIRECT_INSTALLED.load(Ordering::SeqCst);
        let loading_bg_portrait_redirect_attempts =
            LOADING_BG_TEXTURE_REDIRECT_ATTEMPTS.load(Ordering::SeqCst);
        let loading_bg_portrait_redirect_commits =
            LOADING_BG_TEXTURE_REDIRECT_COMMITS.load(Ordering::SeqCst);
        let loading_bg_live_gx_rebinds = LOADING_BG_LIVE_GX_REBINDS.load(Ordering::SeqCst);
        let loadscreen_table_builds = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst);
        let loading_bg_portrait_redirect_last_symbol_match =
            LOADING_BG_TEXTURE_REDIRECT_LAST_SYMBOL_MATCH.load(Ordering::SeqCst);
        let loading_bg_portrait_redirect_last_portrait =
            LOADING_BG_TEXTURE_REDIRECT_LAST_PORTRAIT.load(Ordering::SeqCst);
        let loading_bg_portrait_gx_nonblack =
            LOADING_BG_PORTRAIT_NONBLACK.load(Ordering::SeqCst) != 0;
        let loading_bg_portrait_is_checker =
            LOADING_BG_PORTRAIT_IS_CHECKER.load(Ordering::SeqCst) != 0;
        let portrait_render_drive_hits = PROFILE_RENDER_DRIVE_HITS.load(Ordering::SeqCst);
        let loading_bg_portrait_gx_dims = LOADING_BG_PORTRAIT_DIMS.load(Ordering::SeqCst);
        let loading_bg_portrait_gx_format = LOADING_BG_PORTRAIT_FORMAT.load(Ordering::SeqCst);
        let title_custom_cover_profile_render_refresh_calls =
            TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_CALLS.load(Ordering::SeqCst);
        let title_custom_cover_profile_render_refresh_last_profile_summary =
            TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_PROFILE_SUMMARY.load(Ordering::SeqCst);
        let title_custom_cover_profile_render_refresh_last_caller_phase =
            TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_CALLER_PHASE.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_sample_calls =
            TITLE_CUSTOM_COVER_PROFILE_SOURCE_SAMPLE_CALLS.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_slot =
            TITLE_CUSTOM_COVER_PROFILE_SOURCE_SLOT.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_renderer =
            TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_renderer_vtable =
            TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER_VTABLE.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_offscreen_rend =
            TITLE_CUSTOM_COVER_PROFILE_SOURCE_OFFSCREEN_REND.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_tex_rescap =
            TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_RESCAP.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_tex_index =
            TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_INDEX.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_ready_754 =
            TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_754.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_ready_755 =
            TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_755.load(Ordering::SeqCst);
        let title_custom_cover_profile_source_ready =
            title_custom_cover_profile_source_renderer_vtable
                == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
                && title_custom_cover_profile_source_offscreen_rend
                    != TITLE_OWNER_SCAN_START_ADDRESS
                && title_custom_cover_profile_source_offscreen_rend != NULL_PTR
                && title_custom_cover_profile_source_tex_rescap != TITLE_OWNER_SCAN_START_ADDRESS
                && title_custom_cover_profile_source_tex_rescap != NULL_PTR;
        let title_custom_cover_profile_select_builds =
            TITLE_CUSTOM_COVER_PROFILE_SELECT_BUILDS.load(Ordering::SeqCst);
        let title_custom_cover_profile_select_last_ret =
            TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_RET.load(Ordering::SeqCst);
        let title_custom_cover_profile_select_last_job =
            TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_JOB.load(Ordering::SeqCst);
        let title_custom_cover_profile_select_last_caller_rva =
            TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_custom_cover_black_builds =
            TITLE_CUSTOM_COVER_BLACK_BUILDS.load(Ordering::SeqCst);
        let title_custom_cover_black_last_ret =
            TITLE_CUSTOM_COVER_BLACK_LAST_RET.load(Ordering::SeqCst);
        let title_custom_cover_black_last_job =
            TITLE_CUSTOM_COVER_BLACK_LAST_JOB.load(Ordering::SeqCst);
        let title_custom_cover_black_last_caller_rva =
            TITLE_CUSTOM_COVER_BLACK_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_custom_cover_run_calls = TITLE_CUSTOM_COVER_RUN_CALLS.load(Ordering::SeqCst);
        let title_custom_cover_run_last_native_job =
            TITLE_CUSTOM_COVER_RUN_LAST_NATIVE_JOB.load(Ordering::SeqCst);
        let title_custom_cover_run_last_cover_job =
            TITLE_CUSTOM_COVER_RUN_LAST_COVER_JOB.load(Ordering::SeqCst);
        let title_custom_cover_run_last_cover_window =
            TITLE_CUSTOM_COVER_RUN_LAST_COVER_WINDOW.load(Ordering::SeqCst);
        let title_custom_cover_run_last_ret =
            TITLE_CUSTOM_COVER_RUN_LAST_RET.load(Ordering::SeqCst);
        let title_pab_information_visual_builds =
            TITLE_PAB_INFORMATION_VISUAL_BUILDS.load(Ordering::SeqCst);
        let title_pab_information_visual_last_job =
            TITLE_PAB_INFORMATION_VISUAL_LAST_JOB.load(Ordering::SeqCst);
        let title_pab_information_visual_last_window =
            TITLE_PAB_INFORMATION_VISUAL_LAST_WINDOW.load(Ordering::SeqCst);
        let title_pab_information_visual_last_caller_rva =
            TITLE_PAB_INFORMATION_VISUAL_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_custom_cover_black_cover_window = if title_custom_cover_run_last_cover_window
            != NULL_PTR
            && title_custom_cover_run_last_cover_window != TITLE_OWNER_SCAN_START_ADDRESS
        {
            title_custom_cover_run_last_cover_window
        } else if title_custom_cover_black_last_job != NULL_PTR
            && title_custom_cover_black_last_job != TITLE_OWNER_SCAN_START_ADDRESS
        {
            unsafe {
                crate::experiments::safe_read_usize(title_custom_cover_black_last_job + 0x130)
            }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let (
            title_custom_cover_black_cover_menu_id,
            title_custom_cover_black_cover_flags,
            title_custom_cover_black_cover_draw_bit_set,
        ) = title_menu_window_id_flags(base, title_custom_cover_black_cover_window);
        let (
            title_pab_information_visual_current_menu_id,
            title_pab_information_visual_current_flags,
            title_pab_information_visual_current_draw_bit_set,
        ) = title_menu_window_id_flags(base, title_pab_information_visual_last_window);
        let title_custom_cover_black_exclusive_visible = title_custom_cover_black_builds != 0
            && title_custom_cover_run_calls != 0
            && title_custom_cover_black_cover_draw_bit_set
            && !title_visual_current_draw_bit_set
            && !title_pab_information_visual_current_draw_bit_set;
        // Latched peak-load proof + derived load-time msgbox count (bd er-effects-rs-ns4n follow-up).
        // `oracle_load_correctness_seen > 0` proves a REAL character reached the world this run, latched
        // so a quit-to-title (which resets the live oracle_char_* fields) cannot erase it.
        // `oracle_msgbox_loadtime_builds` = total - postload isolates the load-time msgbox count (the
        // hard crash/investigation trigger) from benign in-world/quit-confirm dialogs, which the total
        // alone conflates.
        let loaded_peak_seen = LOADED_PEAK_SEEN_COUNT.load(Ordering::SeqCst);
        let loaded_peak_level = LOADED_PEAK_LEVEL.load(Ordering::SeqCst);
        let loaded_peak_c30 = LOADED_PEAK_C30.load(Ordering::SeqCst) as u32;
        let loaded_peak_name_len = LOADED_PEAK_NAME_LEN.load(Ordering::SeqCst);
        let loaded_peak_name = LOADED_PEAK_NAME
            .lock()
            .map(|latched| latched.clone())
            .unwrap_or_default();
        let msgbox_loadtime_builds = msgbox_total_builds.saturating_sub(msgbox_postload_builds);
        body.push_str(&format!(
            "  \"oracle_load_correctness_seen\": {loaded_peak_seen},\n  \"oracle_loaded_peak_level\": {loaded_peak_level},\n  \"oracle_loaded_peak_c30\": \"0x{loaded_peak_c30:x}\",\n  \"oracle_loaded_peak_name\": \"{}\",\n  \"oracle_loaded_peak_name_len\": {loaded_peak_name_len},\n  \"oracle_msgbox_loadtime_builds\": {msgbox_loadtime_builds},\n  \"oracle_msgbox_loadtime_seen\": {},\n",
            json_escape(&loaded_peak_name),
            msgbox_loadtime_builds != 0
        ));
        body.push_str(&format!(
            "  \"oracle_msgbox_total_builds\": {},\n  \"oracle_msgbox_any_seen\": {},\n  \"oracle_msgbox_postload_builds\": {},\n  \"oracle_postload_modal_seen\": {},\n  \"oracle_blocking_modal_present\": {},\n  \"oracle_blocking_modal_ptr\": {},\n  \"oracle_blocking_modal_vtable\": {},\n  \"oracle_blocking_modal_closing_latch\": {},\n  \"oracle_msgbox_builder_args\": [{}, {}, {}, {}],\n  \"oracle_policy_window_total_builds\": {},\n  \"oracle_policy_window_any_seen\": {},\n  \"oracle_policy_window_ptr\": {},\n  \"oracle_policy_window_vtable\": {},\n  \"oracle_policy_window_args\": [{}, {}, {}, {}, {}],\n  \"oracle_policy_window_stack_arg0\": {},\n  \"oracle_policy_window_backing_flag_ptr\": {},\n  \"oracle_policy_window_stored_backing_flag_ptr\": {},\n  \"oracle_policy_window_backing_flag_value\": {},\n  \"oracle_policy_window_requested_flag_value\": {},\n  \"oracle_policy_window_caller_rva\": {},\n  \"oracle_policy_ctor_wrapper_hits\": {},\n  \"oracle_policy_ctor_wrapper_record\": {},\n  \"oracle_policy_ctor_wrapper_original_this\": {},\n  \"oracle_policy_ctor_wrapper_original_vtable\": {},\n  \"oracle_policy_ctor_wrapper_record_id\": {},\n  \"oracle_policy_ctor_wrapper_stack_arg0\": {},\n  \"oracle_policy_ctor_wrapper_backing_flag_ptr\": {},\n  \"oracle_policy_ctor_wrapper_ret\": {},\n  \"oracle_policy_ctor_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_wrapper_hits\": {},\n  \"oracle_policy_selector_wrapper_record\": {},\n  \"oracle_policy_selector_wrapper_original_this\": {},\n  \"oracle_policy_selector_wrapper_original_vtable\": {},\n  \"oracle_policy_selector_wrapper_owner\": {},\n  \"oracle_policy_selector_wrapper_requested_flag\": {},\n  \"oracle_policy_selector_wrapper_selector_arg\": {},\n  \"oracle_policy_selector_wrapper_ret\": {},\n  \"oracle_policy_selector_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_ctor_hits\": {},\n  \"oracle_policy_selector_ctor_this\": {},\n  \"oracle_policy_selector_ctor_vtable\": {},\n  \"oracle_policy_selector_ctor_owner\": {},\n  \"oracle_policy_selector_ctor_requested_flag_ptr\": {},\n  \"oracle_policy_selector_ctor_requested_flag_value\": {},\n  \"oracle_policy_selector_ctor_selector_arg\": {},\n  \"oracle_policy_selector_ctor_stored_selector_arg\": {},\n  \"oracle_policy_selector_ctor_stored_requested_flag_ptr\": {},\n  \"oracle_policy_selector_ctor_ret\": {},\n  \"oracle_policy_selector_ctor_caller_rva\": {},\n  \"oracle_policy_status_predicate_hits\": {},\n  \"oracle_policy_status_predicate_this\": {},\n  \"oracle_policy_status_predicate_owner\": {},\n  \"oracle_policy_status_predicate_flag_ptr\": {},\n  \"oracle_policy_status_predicate_flag_value\": {},\n  \"oracle_policy_status_predicate_ret\": {},\n  \"oracle_policy_status_predicate_caller_rva\": {},\n  \"oracle_policy_flag_setter_hits\": {},\n  \"oracle_policy_flag_setter_owner\": {},\n  \"oracle_policy_flag_setter_value\": {},\n  \"oracle_policy_flag_setter_force\": {},\n  \"oracle_policy_flag_setter_before\": {},\n  \"oracle_policy_flag_setter_after\": {},\n  \"oracle_policy_flag_setter_caller_rva\": {},\n  \"oracle_server_status_total_seen\": {},\n  \"oracle_server_status_any_seen\": {},\n  \"oracle_server_status_state\": {},\n  \"oracle_server_status_text_id\": {},\n",
            msgbox_total_builds,
            msgbox_any_seen,
            msgbox_postload_builds,
            postload_modal_seen,
            blocking_modal_present,
            msgbox_dialog,
            msgbox_vtable,
            msgbox_closing_latch,
            msgbox_arg_rcx,
            msgbox_arg_rdx,
            msgbox_arg_r8,
            msgbox_arg_r9,
            policy_total_builds,
            policy_any_seen,
            policy_ptr,
            policy_vtable,
            policy_arg_rdx,
            policy_arg_r8,
            policy_arg_r9,
            policy_stack_arg0,
            policy_backing_flag_ptr,
            policy_stack_arg0,
            policy_backing_flag_ptr,
            policy_stored_backing_flag_ptr,
            policy_backing_flag_value,
            policy_requested_flag_value,
            policy_caller_rva,
            policy_wrapper_hits,
            policy_wrapper_record,
            policy_wrapper_original_this,
            policy_wrapper_original_vtable,
            policy_wrapper_record_id,
            policy_wrapper_stack_arg0,
            policy_wrapper_backing_flag_ptr,
            policy_wrapper_ret,
            policy_wrapper_caller_rva,
            policy_selector_hits,
            policy_selector_record,
            policy_selector_original_this,
            policy_selector_original_vtable,
            policy_selector_owner,
            policy_selector_requested_flag,
            policy_selector_arg,
            policy_selector_ret,
            policy_selector_caller_rva,
            policy_selector_ctor_hits,
            policy_selector_ctor_this,
            policy_selector_ctor_vtable,
            policy_selector_ctor_owner,
            policy_selector_ctor_requested_flag_ptr,
            policy_selector_ctor_requested_flag_value,
            policy_selector_ctor_selector_arg,
            policy_selector_ctor_stored_selector_arg,
            policy_selector_ctor_stored_requested_flag_ptr,
            policy_selector_ctor_ret,
            policy_selector_ctor_caller_rva,
            policy_status_hits,
            policy_status_this,
            policy_status_owner,
            policy_status_flag_ptr,
            policy_status_flag_value,
            policy_status_ret,
            policy_status_caller_rva,
            policy_flag_setter_hits,
            policy_flag_setter_owner,
            policy_flag_setter_value,
            policy_flag_setter_force,
            policy_flag_setter_before,
            policy_flag_setter_after,
            policy_flag_setter_caller_rva,
            server_status_total_seen,
            server_status_any_seen,
            server_status_state,
            server_status_text_id
        ));
        body.push_str(&format!(
            "  \"oracle_title_native_menu_visual_suppress_installed\": {},\n  \"oracle_title_native_menu_visual_suppressed_builds\": {},\n  \"oracle_title_native_menu_visual_any_suppressed\": {},\n  \"oracle_title_native_menu_visual_last_out_slot\": {},\n  \"oracle_title_native_menu_visual_last_prev_out\": {},\n  \"oracle_title_native_menu_visual_last_args\": [{}, {}],\n  \"oracle_title_native_menu_visual_last_caller_rva\": {},\n  \"oracle_title_native_menu_visual_native_job\": {},\n  \"oracle_title_native_menu_visual_native_window\": {},\n  \"oracle_title_native_menu_visual_current_menu_id\": {},\n  \"oracle_title_native_menu_visual_current_flags\": {},\n  \"oracle_title_native_menu_visual_current_draw_bit_set\": {},\n  \"oracle_title_native_menu_visual_render_suppress_installed\": {},\n  \"oracle_title_native_menu_visual_render_suppressed_windows\": {},\n  \"oracle_title_native_menu_visual_render_any_suppressed\": {},\n  \"oracle_title_native_menu_visual_render_last_window\": {},\n  \"oracle_title_native_menu_visual_render_last_flags_before\": {},\n  \"oracle_title_native_menu_visual_render_last_flags_after\": {},\n  \"oracle_title_native_menu_visual_render_last_caller_rva\": {},\n  \"oracle_title_logo_surface_name\": \"{}\",\n  \"oracle_title_logo_resource_name\": \"{}\",\n  \"oracle_title_logo_gfx_root_depth\": {},\n  \"oracle_title_logo_gfx_root_sprite_char\": {},\n  \"oracle_title_logo_gfx_main_asset_char\": {},\n  \"oracle_title_logo_gfx_main_asset_name\": \"{}\",\n  \"oracle_title_logo_back_view_parts\": {},\n  \"oracle_title_logo_back_view_parts_vtable\": {},\n  \"oracle_title_logo_gfx_frame\": {},\n  \"oracle_title_logo_gfx_alpha_mult_term\": {},\n  \"oracle_title_logo_gfx_visibility\": {},\n  \"oracle_title_logo_gfx_hide_calls\": {},\n  \"oracle_title_logo_gfx_any_hidden\": {},\n  \"oracle_title_logo_gfx_hide_last_dialog\": {},\n  \"oracle_title_logo_gfx_hide_last_logo\": {},\n  \"oracle_title_logo_gfx_hide_last_caller_phase\": {},\n  \"oracle_title_logo_gfx_hide_last_requested_visible\": {},\n  \"oracle_title_press_start_surface_name\": \"PressStart\",\n  \"oracle_title_press_start_text_name\": \"StaticSystemText_101000\",\n  \"oracle_title_press_start_text_initial\": \"PRESS BUTTON\",\n  \"oracle_title_press_start_gfx_hide_calls\": {},\n  \"oracle_title_press_start_gfx_any_hidden\": {},\n  \"oracle_title_press_start_gfx_hide_last_dialog\": {},\n  \"oracle_title_press_start_gfx_hide_last_proxy\": {},\n  \"oracle_title_press_start_gfx_hide_last_context\": {},\n  \"oracle_title_press_start_gfx_hide_last_caller_phase\": {},\n  \"oracle_title_press_start_gfx_value\": {},\n  \"oracle_title_press_start_gfx_force_false_calls\": {},\n  \"oracle_title_press_start_gfx_force_false_any\": {},\n  \"oracle_title_press_start_gfx_force_false_last_value\": {},\n  \"oracle_title_press_start_gfx_force_false_last_requested\": {},\n  \"oracle_title_press_start_bind_hits\": {},\n  \"oracle_title_press_start_bind_any\": {},\n  \"oracle_title_press_start_bind_last_parent\": {},\n  \"oracle_title_press_start_bind_last_out\": {},\n  \"oracle_title_press_start_bind_last_name\": {},\n  \"oracle_title_press_start_bind_last_context\": {},\n  \"oracle_title_press_start_bind_hide_calls\": {},\n  \"oracle_title_press_start_bind_any_hidden\": {},\n  \"oracle_title_profile_cover_bound_to_logo_surface\": {},\n  \"oracle_title_overlay_cover_render_calls\": {},\n  \"oracle_title_overlay_cover_rendered\": {},\n  \"oracle_title_overlay_cover_last_display_size\": [{}, {}],\n  \"oracle_title_overlay_cover_display_sane\": {},\n  \"oracle_title_overlay_cover_texture_bound\": {},\n  \"oracle_title_overlay_cover_last_gx_texture\": {},\n  \"oracle_title_overlay_cover_last_texture_resource\": {},\n  \"oracle_title_profile_face_bind_hits\": {},\n  \"oracle_title_profile_face_transform_applied\": {},\n  \"oracle_title_profile_face_other_hidden\": {},\n  \"oracle_title_profile_face_last_proxy\": {},\n  \"oracle_title_profile_face_last_value\": {},\n  \"oracle_title_loaded_character_portrait_rendered\": {},\n  \"oracle_title_loaded_character_portrait_visible_during_boot\": {},\n  \"oracle_title_loaded_character_portrait_held_until_loading_takeover\": {},\n  \"oracle_title_scaleform_bind_observer_hits\": {},\n  \"oracle_title_scaleform_bind_observer_systex_hits\": {},\n  \"oracle_title_scaleform_bind_observer_last_owner\": {},\n  \"oracle_title_scaleform_bind_observer_last_pair\": {},\n  \"oracle_title_scaleform_bind_observer_last_symbol_ptr\": {},\n  \"oracle_title_scaleform_bind_observer_last_target_ptr\": {},\n  \"oracle_title_portrait_visible_surface_symbol\": \"{}\",\n  \"oracle_title_portrait_visible_surface_bind_rewrites\": {},\n  \"oracle_title_portrait_visible_surface_bound\": {},\n  \"oracle_title_portrait_visible_surface_bind_last_owner\": {},\n  \"oracle_title_portrait_visible_surface_bind_last_pair\": {},\n  \"oracle_title_portrait_visible_surface_bind_last_symbol_ptr\": {},\n  \"oracle_title_now_loading_helper_hooks_installed\": {},\n  \"oracle_title_now_loading_helper_ctor_hits\": {},\n  \"oracle_title_now_loading_helper_update_hits\": {},\n  \"oracle_title_now_loading_helper_last_this\": {},\n  \"oracle_title_now_loading_helper_last_menu_index\": {},\n  \"oracle_title_now_loading_helper_last_replace_tex_info\": {},\n  \"oracle_title_now_loading_helper_last_requested_replace_tex_info\": {},\n  \"oracle_title_now_loading_helper_last_flags\": {},\n  \"oracle_loading_bg_portrait_redirect_installed\": {},\n  \"oracle_loading_bg_portrait_redirect_attempts\": {},\n  \"oracle_loading_bg_portrait_redirect_commits\": {},\n  \"oracle_loading_bg_live_gx_rebinds\": {},\n  \"oracle_loadscreen_table_builds\": {},\n  \"oracle_loading_bg_portrait_redirect_last_symbol_match\": {},\n  \"oracle_loading_bg_portrait_redirect_last_portrait\": {},\n  \"oracle_loading_bg_portrait_gx_nonblack\": {},\n  \"oracle_loading_bg_portrait_is_checker\": {},\n  \"oracle_portrait_render_drive_hits\": {},\n  \"oracle_loading_bg_portrait_gx_dims\": {},\n  \"oracle_loading_bg_portrait_gx_format\": {},\n  \"oracle_title_logo_profile_summary\": {},\n  \"oracle_title_logo_profile_summary_ready\": {},\n  \"oracle_title_profile_render_refresh_gate_ready\": {},\n  \"oracle_title_custom_cover_profile_render_refresh_calls\": {},\n  \"oracle_title_custom_cover_profile_render_refresh_last_profile_summary\": {},\n  \"oracle_title_custom_cover_profile_render_refresh_last_caller_phase\": {},\n  \"oracle_title_custom_cover_profile_source_sample_calls\": {},\n  \"oracle_title_custom_cover_profile_source_slot\": {},\n  \"oracle_title_custom_cover_profile_source_renderer\": {},\n  \"oracle_title_custom_cover_profile_source_renderer_vtable\": {},\n  \"oracle_title_custom_cover_profile_source_offscreen_rend\": {},\n  \"oracle_title_custom_cover_profile_source_tex_rescap\": {},\n  \"oracle_title_custom_cover_profile_source_tex_index\": {},\n  \"oracle_title_custom_cover_profile_source_ready_754\": {},\n  \"oracle_title_custom_cover_profile_source_ready_755\": {},\n  \"oracle_title_custom_cover_profile_source_ready\": {},\n  \"oracle_title_custom_cover_profile_source_name\": \"{}\",\n  \"oracle_title_custom_cover_profile_renderer_class\": \"{}\",\n  \"oracle_title_custom_cover_profile_select_builds\": {},\n  \"oracle_title_custom_cover_profile_select_any_built\": {},\n  \"oracle_title_custom_cover_profile_select_last_ret\": {},\n  \"oracle_title_custom_cover_profile_select_last_job\": {},\n  \"oracle_title_custom_cover_profile_select_last_caller_rva\": {},\n  \"oracle_title_custom_cover_black_surface_name\": \"{}\",\n  \"oracle_title_custom_cover_black_builds\": {},\n  \"oracle_title_custom_cover_black_any_built\": {},\n  \"oracle_title_custom_cover_black_last_ret\": {},\n  \"oracle_title_custom_cover_black_last_job\": {},\n  \"oracle_title_custom_cover_black_last_caller_rva\": {},\n  \"oracle_title_custom_cover_run_calls\": {},\n  \"oracle_title_custom_cover_run_any\": {},\n  \"oracle_title_custom_cover_run_last_native_job\": {},\n  \"oracle_title_custom_cover_run_last_cover_job\": {},\n  \"oracle_title_custom_cover_run_last_cover_window\": {},\n  \"oracle_title_custom_cover_run_last_ret\": {},\n  \"oracle_title_pab_information_visual_name\": \"{}\",\n  \"oracle_title_pab_information_visual_builds\": {},\n  \"oracle_title_pab_information_visual_any_built\": {},\n  \"oracle_title_pab_information_visual_last_job\": {},\n  \"oracle_title_pab_information_visual_last_window\": {},\n  \"oracle_title_pab_information_visual_last_caller_rva\": {},\n",
            title_visual_suppress_installed,
            title_visual_suppressed_builds,
            title_visual_suppressed_builds != 0,
            title_visual_last_out_slot,
            title_visual_last_prev_out,
            title_visual_last_arg_rdx,
            title_visual_last_arg_r8,
            title_visual_last_caller_rva,
            title_visual_native_job,
            title_visual_native_window,
            title_visual_current_menu_id,
            title_visual_current_flags,
            title_visual_current_draw_bit_set,
            title_visual_render_suppress_installed,
            title_visual_render_suppressed_windows,
            title_visual_render_suppressed_windows != 0,
            title_visual_render_last_window,
            title_visual_render_last_flags_before,
            title_visual_render_last_flags_after,
            title_visual_render_last_caller_rva,
            TITLE_LOGO_BACK_VIEW_PARTS_NAME,
            TITLE_LOGO_RESOURCE_NAME,
            TITLE_LOGO_GFX_ROOT_DEPTH,
            TITLE_LOGO_GFX_ROOT_SPRITE_CHAR,
            TITLE_LOGO_GFX_MAIN_ASSET_CHAR,
            TITLE_LOGO_GFX_MAIN_ASSET_NAME,
            title_logo_back_view_parts,
            title_logo_back_view_parts_vtable,
            title_logo_gfx_frame,
            title_logo_gfx_alpha_mult_term,
            title_logo_gfx_visibility,
            title_logo_gfx_hide_calls,
            title_logo_gfx_hide_calls != 0,
            title_logo_gfx_hide_last_dialog,
            title_logo_gfx_hide_last_logo,
            title_logo_gfx_hide_last_caller_phase,
            title_logo_gfx_hide_last_requested_visible,
            title_press_start_gfx_hide_calls,
            title_press_start_gfx_hide_calls != 0,
            title_press_start_gfx_hide_last_dialog,
            title_press_start_gfx_hide_last_proxy,
            title_press_start_gfx_hide_last_context,
            title_press_start_gfx_hide_last_caller_phase,
            title_press_start_gfx_value,
            title_press_start_gfx_force_false_calls,
            title_press_start_gfx_force_false_calls != 0,
            title_press_start_gfx_force_false_last_value,
            title_press_start_gfx_force_false_last_requested,
            title_press_start_bind_hits,
            title_press_start_bind_hits != 0,
            title_press_start_bind_last_parent,
            title_press_start_bind_last_out,
            title_press_start_bind_last_name,
            title_press_start_bind_last_context,
            title_press_start_bind_hide_calls,
            title_press_start_bind_hide_calls != 0,
            title_profile_cover_bound_to_logo_surface,
            title_overlay_cover_render_calls,
            title_overlay_cover_render_calls != 0,
            title_overlay_cover_last_display_w,
            title_overlay_cover_last_display_h,
            title_overlay_cover_display_sane,
            title_overlay_cover_texture_bound,
            title_overlay_cover_last_gx_texture,
            title_overlay_cover_last_texture_resource,
            title_profile_face_bind_hits,
            title_profile_face_transform_applied,
            title_profile_face_other_hidden,
            title_profile_face_last_proxy,
            title_profile_face_last_value,
            title_loaded_character_portrait_rendered,
            title_loaded_character_portrait_visible_during_boot,
            title_loaded_character_portrait_held_until_loading_takeover,
            title_scaleform_bind_observer_hits,
            title_scaleform_bind_observer_systex_hits,
            title_scaleform_bind_observer_last_owner,
            title_scaleform_bind_observer_last_pair,
            title_scaleform_bind_observer_last_symbol_ptr,
            title_scaleform_bind_observer_last_target_ptr,
            TITLE_PROFILE_VISIBLE_SURFACE_SYMBOL,
            title_profile_visible_surface_bind_rewrites,
            title_profile_visible_surface_bind_rewrites != 0,
            title_profile_visible_surface_bind_last_owner,
            title_profile_visible_surface_bind_last_pair,
            title_profile_visible_surface_bind_last_symbol_ptr,
            now_loading_helper_hooks_installed,
            now_loading_helper_ctor_hits,
            now_loading_helper_update_hits,
            now_loading_helper_last_this,
            now_loading_helper_last_menu_index,
            now_loading_helper_last_replace_tex_info,
            now_loading_helper_last_requested_replace_tex_info,
            now_loading_helper_last_flags,
            loading_bg_portrait_redirect_installed,
            loading_bg_portrait_redirect_attempts,
            loading_bg_portrait_redirect_commits,
            loading_bg_live_gx_rebinds,
            loadscreen_table_builds,
            loading_bg_portrait_redirect_last_symbol_match,
            loading_bg_portrait_redirect_last_portrait,
            loading_bg_portrait_gx_nonblack,
            loading_bg_portrait_is_checker,
            portrait_render_drive_hits,
            loading_bg_portrait_gx_dims,
            loading_bg_portrait_gx_format,
            title_logo_profile_summary,
            title_logo_profile_summary_ready,
            title_profile_render_refresh_gate_ready,
            title_custom_cover_profile_render_refresh_calls,
            title_custom_cover_profile_render_refresh_last_profile_summary,
            title_custom_cover_profile_render_refresh_last_caller_phase,
            title_custom_cover_profile_source_sample_calls,
            title_custom_cover_profile_source_slot,
            title_custom_cover_profile_source_renderer,
            title_custom_cover_profile_source_renderer_vtable,
            title_custom_cover_profile_source_offscreen_rend,
            title_custom_cover_profile_source_tex_rescap,
            title_custom_cover_profile_source_tex_index,
            title_custom_cover_profile_source_ready_754,
            title_custom_cover_profile_source_ready_755,
            title_custom_cover_profile_source_ready,
            TITLE_CUSTOM_COVER_SYSTEX_TARGET,
            TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS,
            title_custom_cover_profile_select_builds,
            title_custom_cover_profile_select_builds != 0,
            title_custom_cover_profile_select_last_ret,
            title_custom_cover_profile_select_last_job,
            title_custom_cover_profile_select_last_caller_rva,
            TITLE_CUSTOM_COVER_BLACK_NAME,
            title_custom_cover_black_builds,
            title_custom_cover_black_builds != 0,
            title_custom_cover_black_last_ret,
            title_custom_cover_black_last_job,
            title_custom_cover_black_last_caller_rva,
            title_custom_cover_run_calls,
            title_custom_cover_run_calls != 0,
            title_custom_cover_run_last_native_job,
            title_custom_cover_run_last_cover_job,
            title_custom_cover_run_last_cover_window,
            title_custom_cover_run_last_ret,
            TITLE_PAB_INFORMATION_VISUAL_NAME,
            title_pab_information_visual_builds,
            title_pab_information_visual_builds != 0,
            title_pab_information_visual_last_job,
            title_pab_information_visual_last_window,
            title_pab_information_visual_last_caller_rva
        ));
        push_json_usize(
            body,
            "oracle_title_custom_cover_black_cover_window",
            title_custom_cover_black_cover_window,
        );
        push_json_usize(
            body,
            "oracle_title_custom_cover_black_cover_menu_id",
            title_custom_cover_black_cover_menu_id,
        );
        push_json_usize(
            body,
            "oracle_title_custom_cover_black_cover_flags",
            title_custom_cover_black_cover_flags,
        );
        push_json_bool(
            body,
            "oracle_title_custom_cover_black_cover_draw_bit_set",
            title_custom_cover_black_cover_draw_bit_set,
        );
        push_json_bool(
            body,
            "oracle_title_custom_cover_black_exclusive_visible",
            title_custom_cover_black_exclusive_visible,
        );
        push_json_usize(
            body,
            "oracle_title_pab_information_visual_current_menu_id",
            title_pab_information_visual_current_menu_id,
        );
        push_json_usize(
            body,
            "oracle_title_pab_information_visual_current_flags",
            title_pab_information_visual_current_flags,
        );
        push_json_bool(
            body,
            "oracle_title_pab_information_visual_current_draw_bit_set",
            title_pab_information_visual_current_draw_bit_set,
        );
        push_json_bool(
            body,
            "oracle_title_menu_resource_acquire_observer_installed",
            title_menu_resource_acquire_installed,
        );
        push_json_usize(
            body,
            "oracle_title_menu_resource_acquire_hits",
            title_menu_resource_acquire_hits,
        );
        push_json_usize(
            body,
            "oracle_title_menu_resource_acquire_logo_hits",
            title_menu_resource_acquire_logo_hits,
        );
        push_json_bool(
            body,
            "oracle_title_menu_resource_acquire_logo_seen",
            title_menu_resource_acquire_logo_hits != 0,
        );
        push_json_usize(
            body,
            "oracle_title_menu_resource_acquire_last_this",
            title_menu_resource_acquire_last_this,
        );
        push_json_usize(
            body,
            "oracle_title_menu_resource_acquire_last_load_params",
            title_menu_resource_acquire_last_load_params,
        );
        push_json_usize(
            body,
            "oracle_title_menu_resource_acquire_last_filename_ptr",
            title_menu_resource_acquire_last_filename_ptr,
        );
        push_json_usize(
            body,
            "oracle_title_menu_resource_acquire_last_param3",
            title_menu_resource_acquire_last_param3,
        );
        push_json_usize(
            body,
            "oracle_title_menu_resource_acquire_last_ret",
            title_menu_resource_acquire_last_ret,
        );
        push_json_usize(
            body,
            "oracle_title_menu_resource_acquire_last_caller_rva",
            title_menu_resource_acquire_last_caller_rva,
        );
        push_json_bool(
            body,
            "oracle_title_scaleform_file_open_observer_installed",
            title_scaleform_file_open_installed,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_file_open_hits",
            title_scaleform_file_open_hits,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_file_open_logo_hits",
            title_scaleform_file_open_logo_hits,
        );
        push_json_bool(
            body,
            "oracle_title_scaleform_file_open_logo_seen",
            title_scaleform_file_open_logo_hits != 0,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_file_open_last_loader",
            title_scaleform_file_open_last_loader,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_file_open_last_url_ptr",
            title_scaleform_file_open_last_url_ptr,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_file_open_last_flags",
            title_scaleform_file_open_last_flags,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_file_open_last_ret",
            title_scaleform_file_open_last_ret,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_file_open_last_ret_vtable",
            title_scaleform_file_open_last_ret_vtable,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_file_open_last_caller_rva",
            title_scaleform_file_open_last_caller_rva,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_memory_gfx_bytes",
            title_scaleform_memory_gfx_bytes,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_memory_gfx_replacements",
            title_scaleform_memory_gfx_replacements,
        );
        push_json_bool(
            body,
            "oracle_title_scaleform_memory_gfx_replaced",
            title_scaleform_memory_gfx_replacements != 0,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_05_000_memory_gfx_replacements",
            title_scaleform_05_000_memory_gfx_replacements,
        );
        push_json_bool(
            body,
            "oracle_title_scaleform_05_000_memory_gfx_replaced",
            title_scaleform_05_000_memory_gfx_replacements != 0,
        );
        push_json_usize(
            body,
            "oracle_title_05_000_runtime_strip_armed",
            TITLE_05_000_RUNTIME_STRIP_ARMED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_title_05_000_runtime_strip_serves",
            TITLE_05_000_RUNTIME_STRIP_SERVES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_title_05_000_runtime_strip_failures",
            TITLE_05_000_RUNTIME_STRIP_FAILURES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_title_05_000_runtime_strip_input_len",
            TITLE_05_000_RUNTIME_STRIP_INPUT_LEN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_title_05_000_runtime_strip_output_len",
            TITLE_05_000_RUNTIME_STRIP_OUTPUT_LEN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_title_05_000_runtime_strip_input_class",
            TITLE_05_000_RUNTIME_STRIP_INPUT_CLASS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_title_05_000_runtime_strip_output_validated",
            TITLE_05_000_RUNTIME_STRIP_OUTPUT_VALIDATED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_memory_gfx_failures",
            title_scaleform_memory_gfx_failures,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_memory_gfx_last_file",
            title_scaleform_memory_gfx_last_file,
        );
        push_json_bool(
            body,
            "oracle_title_scaleform_resource_ctor_observer_installed",
            title_scaleform_resource_ctor_installed,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_resource_ctor_hits",
            title_scaleform_resource_ctor_hits,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_resource_ctor_logo_hits",
            title_scaleform_resource_ctor_logo_hits,
        );
        push_json_bool(
            body,
            "oracle_title_scaleform_resource_ctor_logo_seen",
            title_scaleform_resource_ctor_logo_hits != 0,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_resource_ctor_last_out",
            title_scaleform_resource_ctor_last_out,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_resource_ctor_last_url_ptr",
            title_scaleform_resource_ctor_last_url_ptr,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_resource_ctor_last_file",
            title_scaleform_resource_ctor_last_file,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_resource_ctor_last_ret",
            title_scaleform_resource_ctor_last_ret,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_resource_ctor_last_movie_data",
            title_scaleform_resource_ctor_last_movie_data,
        );
        push_json_usize(
            body,
            "oracle_title_scaleform_resource_ctor_last_caller_rva",
            title_scaleform_resource_ctor_last_caller_rva,
        );
        // er-tpf Tier-4 in-memory cover wire-up oracles (memory-read telemetry, NOT screenshot): a
        // runtime watcher can observe build/register/bind progress + failures without an image.
        push_json_bool(
            body,
            "oracle_tpf_texture_built",
            ER_TPF_COVER_TEXTURE_BUILT.load(Ordering::SeqCst) != 0,
        );
        push_json_usize(
            body,
            "oracle_tpf_texture_blob_len",
            ER_TPF_COVER_BLOB_LEN.load(Ordering::SeqCst),
        );
        push_json_str(body, "oracle_tpf_texture_key", ER_TPF_COVER_SYSTEX_KEY);
        push_json_bool(
            body,
            "oracle_tpf_texture_registered",
            ER_TPF_COVER_REGISTERED.load(Ordering::SeqCst) != 0,
        );
        push_json_usize(
            body,
            "oracle_tpf_texture_last_rescap",
            ER_TPF_COVER_LAST_RESCAP.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_tpf_texture_bound",
            ER_TPF_COVER_BOUND.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_tpf_texture_failures",
            ER_TPF_COVER_FAILURES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_tpf_texture_last_error",
            ER_TPF_COVER_LAST_ERROR.load(Ordering::SeqCst),
        );
        // Stats-panel neutral-background wire-up oracles (memory-read telemetry, NOT screenshot). A
        // runtime watcher confirms the character render is blanked, each per-slot neutral bg registered
        // into the repos, and each visible face bind redirected to our key -- all without an image.
        // `stats_panel_enabled` == the render-blank / stats-panel product mode is active.
        push_json_bool(body, "oracle_stats_panel_enabled", stats_panel_enabled());
        push_json_usize(
            body,
            "oracle_stats_panel_registered_mask",
            STATS_PANEL_TEX_REGISTERED_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_panel_register_attempts",
            STATS_PANEL_TEX_REGISTER_ATTEMPTS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_panel_register_failures",
            STATS_PANEL_TEX_REGISTER_FAILURES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_panel_redirect_mask",
            STATS_PANEL_BIND_REDIRECT_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_panel_redirects",
            STATS_PANEL_BIND_REDIRECTS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_panel_last_error",
            STATS_PANEL_LAST_ERROR.load(Ordering::SeqCst),
        );
        // Stats-panel NATIVE TEXT oracles (row-populate push design): native row fills observed,
        // successful ErStats pushes, and rejected pushes. subs>0 == the attribute line reached the
        // GFX-edit `ErStats` field (rendered in MenuFont_01) in its OWN field; failures>0 with
        // subs==0 == the 05_010 edit was not live (field missing) or SetText rejected the value.
        push_json_usize(
            body,
            "oracle_stats_text_installed",
            TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_INSTALLED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_text_row_populates",
            PROFILE_STATS_ROW_POPULATES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_text_settext_subs",
            PROFILE_STATS_SETTEXT_SUBS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_text_push_failures",
            PROFILE_STATS_PUSH_FAILURES.load(Ordering::SeqCst),
        );
        // 7e7 fail-closed guard: pushes skipped because the resolved component was stale (crash
        // avoided), plus the last stale component/vtable pointers for root-causing the bad link.
        push_json_usize(
            body,
            "oracle_stats_text_push_stale_skips",
            PROFILE_STATS_PUSH_STALE_SKIPS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_text_push_stale_last_comp",
            PROFILE_STATS_PUSH_STALE_LAST_COMP.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_text_push_stale_last_vt",
            PROFILE_STATS_PUSH_STALE_LAST_VT.load(Ordering::SeqCst),
        );
        // Per-slot save-stats cache (bd er-effects-rs-l90): cache_state 1 == the live `.sl2` was read
        // and parsed (each row shows ITS OWN character's attributes); 2 == read failed (fell back to
        // the loaded character). decoded == how many of the 10 save slots held a real character.
        push_json_usize(
            body,
            "oracle_stats_text_slot_cache_state",
            PROFILE_SLOT_STATS_CACHE_STATE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_stats_text_slot_decoded",
            PROFILE_SLOT_STATS_DECODED.load(Ordering::SeqCst),
        );
        // Stats-panel 05_010 runtime GFX edit oracles (mirror the 05_000 runtime-strip set).
        push_json_usize(
            body,
            "oracle_profile_05_010_runtime_edit_armed",
            PROFILE_05_010_RUNTIME_EDIT_ARMED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_05_010_runtime_edit_serves",
            PROFILE_05_010_RUNTIME_EDIT_SERVES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_05_010_runtime_edit_failures",
            PROFILE_05_010_RUNTIME_EDIT_FAILURES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_05_010_runtime_edit_input_len",
            PROFILE_05_010_RUNTIME_EDIT_INPUT_LEN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_05_010_runtime_edit_output_len",
            PROFILE_05_010_RUNTIME_EDIT_OUTPUT_LEN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_05_010_runtime_edit_input_class",
            PROFILE_05_010_RUNTIME_EDIT_INPUT_CLASS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_05_010_runtime_edit_output_validated",
            PROFILE_05_010_RUNTIME_EDIT_OUTPUT_VALIDATED.load(Ordering::SeqCst),
        );
        // Camera-lever (custom profile-portrait viewport) RAM semaphores: a runtime watcher can confirm
        // the override path ran and produced a sane matrix without an image. See bd
        // `camera-lever-RE-VERIFIED-offsets-and-call-addrs-2026-06-29`.
        push_json_usize(
            body,
            "oracle_profile_cam_apply_calls",
            PROFILE_CAM_APPLY_CALLS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_cam_latched_mask",
            PROFILE_CAM_LATCHED_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_cam_face_yaw_latched_mask",
            PROFILE_CAM_FACE_YAW_LATCHED_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_cam_last_slot",
            PROFILE_CAM_LAST_SLOT.load(Ordering::SeqCst),
        );
        push_json_bool(
            body,
            "oracle_profile_cam_last_matrix_ok",
            PROFILE_CAM_LAST_MATRIX_OK.load(Ordering::SeqCst) != 0,
        );
        // Look-at lever RAM semaphores: a watcher can confirm the pose was reached, the Head/Neck/
        // Spine2 bones were resolved, and the per-tick rotation is firing -- without an image.
        push_json_usize(
            body,
            "oracle_profile_lookat_apply_calls",
            PROFILE_LOOKAT_APPLY_CALLS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_lookat_bone_count",
            PROFILE_LOOKAT_BONE_COUNT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_lookat_head_idx",
            PROFILE_LOOKAT_HEAD_IDX.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_lookat_neck_idx",
            PROFILE_LOOKAT_NECK_IDX.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_lookat_spine2_idx",
            PROFILE_LOOKAT_SPINE2_IDX.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_lookat_bones_dumped_mask",
            PROFILE_LOOKAT_BONES_DUMPED_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_lookat_last_cursor",
            PROFILE_LOOKAT_LAST_CURSOR.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_lookat_hook_installed",
            PROFILE_LOOKAT_HOOK_INSTALLED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_lookat_hook_hits",
            PROFILE_LOOKAT_HOOK_HITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_lookat_render_drives",
            PROFILE_LOOKAT_RENDER_DRIVES.load(Ordering::SeqCst),
        );
        // CSCloth teardown guard: profile update/draw drives skipped because the world CSCloth singleton
        // was null (shutdown). 0 during normal operation = no false-skip / no render regression; nonzero
        // at teardown = the exit-time CSCloth DLPanic CTD was prevented.
        push_json_usize(
            body,
            "oracle_profile_drive_cloth_skips",
            PROFILE_DRIVE_CLOTH_SKIPS.load(Ordering::SeqCst),
        );
        // Mouse-track proof: bitmask of look-left/center/look-right head dumps captured (0b111 = all
        // three distinct poses dumped to portrait-capture-slot{200,201,202}.bin during selftest).
        push_json_usize(
            body,
            "oracle_profile_lookat_track_buckets",
            PROFILE_LOOKAT_TRACK_BUCKETS.load(Ordering::SeqCst),
        );
        // DISPLAY path (keepalive): the loading-screen image follows the cursor per-frame only if the
        // Present overlay composites + re-uploads each frame. present_hook_hits = Present detour frames;
        // overlay_draw_hits = backbuffer composites; overlay_reuploads = per-frame texture rebuilds from a
        // version-bumped LOADING_BG_PORTRAIT_RGBA (the displayed head actually tracked, not frozen).
        push_json_usize(
            body,
            "oracle_profile_readback_some",
            PROFILE_READBACK_SOME.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_readback_checker",
            PROFILE_READBACK_CHECKER.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_present_hook_hits",
            PRESENT_HOOK_HITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_overlay_draw_hits",
            OVERLAY_DRAW_HITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_overlay_reuploads",
            OVERLAY_REUPLOADS.load(Ordering::SeqCst),
        );

        let overlay_draw_hits = OVERLAY_DRAW_HITS.load(Ordering::SeqCst);
        let overlay_draw_first_ms = OVERLAY_DRAW_FIRST_MS.load(Ordering::SeqCst);
        let overlay_draw_last_ms = OVERLAY_DRAW_LAST_MS.load(Ordering::SeqCst);
        let overlay_reuploads = OVERLAY_REUPLOADS.load(Ordering::SeqCst);
        let overlay_reupload_first_ms = OVERLAY_REUPLOAD_FIRST_MS.load(Ordering::SeqCst);
        let overlay_reupload_last_ms = OVERLAY_REUPLOAD_LAST_MS.load(Ordering::SeqCst);
        let fps_x1000 = |frames: usize, first_ms: usize, last_ms: usize| -> usize {
            let dt = last_ms.saturating_sub(first_ms);
            if frames < 2 || dt == 0 {
                0
            } else {
                (frames - 1).saturating_mul(1_000_000) / dt
            }
        };
        push_json_usize(body, "oracle_overlay_draw_first_ms", overlay_draw_first_ms);
        push_json_usize(body, "oracle_overlay_draw_last_ms", overlay_draw_last_ms);
        push_json_usize(
            body,
            "oracle_overlay_draw_fps_x1000",
            fps_x1000(overlay_draw_hits, overlay_draw_first_ms, overlay_draw_last_ms),
        );
        push_json_usize(
            body,
            "oracle_overlay_reupload_first_ms",
            overlay_reupload_first_ms,
        );
        push_json_usize(
            body,
            "oracle_overlay_reupload_last_ms",
            overlay_reupload_last_ms,
        );
        push_json_usize(
            body,
            "oracle_overlay_reupload_fps_x1000",
            fps_x1000(overlay_reuploads, overlay_reupload_first_ms, overlay_reupload_last_ms),
        );
        push_json_usize(
            body,
            "oracle_overlay_stale_present_current",
            OVERLAY_STALE_PRESENT_RUN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_overlay_stale_present_max",
            OVERLAY_STALE_PRESENT_MAX.load(Ordering::SeqCst),
        );
        let avg_x1000 = |sum_ms: usize, count: usize| -> usize {
            if count == 0 {
                0
            } else {
                sum_ms.saturating_mul(1000) / count
            }
        };
        let rb_count = OVERLAY_STAGE_READBACK_WAIT_COUNT.load(Ordering::SeqCst);
        let rb_sum = OVERLAY_STAGE_READBACK_WAIT_MS_SUM.load(Ordering::SeqCst);
        push_json_usize(body, "oracle_overlay_readback_wait_count", rb_count);
        push_json_usize(
            body,
            "oracle_overlay_readback_wait_avg_ms_x1000",
            avg_x1000(rb_sum, rb_count),
        );
        push_json_usize(
            body,
            "oracle_overlay_readback_wait_max_ms",
            OVERLAY_STAGE_READBACK_WAIT_MS_MAX.load(Ordering::SeqCst),
        );
        let blend_count = OVERLAY_STAGE_BLEND_COUNT.load(Ordering::SeqCst);
        let blend_sum = OVERLAY_STAGE_BLEND_MS_SUM.load(Ordering::SeqCst);
        push_json_usize(body, "oracle_overlay_blend_count", blend_count);
        push_json_usize(
            body,
            "oracle_overlay_blend_avg_ms_x1000",
            avg_x1000(blend_sum, blend_count),
        );
        push_json_usize(
            body,
            "oracle_overlay_blend_max_ms",
            OVERLAY_STAGE_BLEND_MS_MAX.load(Ordering::SeqCst),
        );
        let up_count = OVERLAY_STAGE_UPLOAD_WAIT_COUNT.load(Ordering::SeqCst);
        let up_sum = OVERLAY_STAGE_UPLOAD_WAIT_MS_SUM.load(Ordering::SeqCst);
        push_json_usize(body, "oracle_overlay_upload_wait_count", up_count);
        push_json_usize(
            body,
            "oracle_overlay_upload_wait_avg_ms_x1000",
            avg_x1000(up_sum, up_count),
        );
        push_json_usize(
            body,
            "oracle_overlay_upload_wait_max_ms",
            OVERLAY_STAGE_UPLOAD_WAIT_MS_MAX.load(Ordering::SeqCst),
        );

        // BOOT-PROGRESS VIEW semaphores: draw_hits = strip composites actually reaching the backbuffer
        // (the pre-Continue black frames are covered); last_permille = displayed progress; milestone_mask/
        // idx = which boot semaphores latched (bit order: BOOT, GAME, OFFLINE, TITLE, MENU, CONTINUE,
        // LOADING); stopped = the handoff to the loading-portrait window fired.
        push_json_usize(
            body,
            "oracle_boot_view_draw_hits",
            BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_boot_view_last_permille",
            BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_boot_view_milestone_mask",
            BOOT_VIEW_REACHED_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_boot_view_milestone_idx",
            BOOT_VIEW_MILESTONE_IDX.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_boot_view_own_menu_load_active",
            BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_boot_view_loadscreen_table_baseline",
            BOOT_VIEW_LOADSCREEN_TABLE_BASELINE.load(Ordering::SeqCst),
        );
        // Seamless-handoff semaphores: handoff_seen_ms = boot-view epoch ms when the loading/world
        // handoff was first detected (0 = not yet; the cover holds fully lit from here);
        // stop_native_hits = CS::LoadingScreen update ticks (baselined per load) when the cover
        // cut -- >= the lit threshold proves the cut landed on a lit loading screen.
        push_json_usize(
            body,
            "oracle_boot_view_handoff_seen_ms",
            BOOT_VIEW_HANDOFF_SEEN_MS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_boot_view_stop_native_hits",
            BOOT_VIEW_STOP_NATIVE_HITS.load(Ordering::SeqCst),
        );
        // Window-reconfiguration timeline semaphores (bd er-effects-rs-rzow): user32 call counts
        // from the observe-only hooks, plus the early final-geometry apply result (1 = applied,
        // 2 = skipped WINDOWED, 3 = no window, 4 = no monitor, 5 = no config, 6 = already final).
        push_json_usize(
            body,
            "oracle_winreconfig_create_window_calls",
            WINRECONFIG_CREATE_WINDOW_CALLS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_winreconfig_set_window_pos_calls",
            WINRECONFIG_SET_WINDOW_POS_CALLS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_winreconfig_set_window_long_calls",
            WINRECONFIG_SET_WINDOW_LONG_CALLS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_winreconfig_move_window_calls",
            WINRECONFIG_MOVE_WINDOW_CALLS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_winreconfig_change_display_calls",
            WINRECONFIG_CHANGE_DISPLAY_CALLS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_winreconfig_early_apply_result",
            WINRECONFIG_EARLY_APPLY_RESULT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_winreconfig_early_apply_ms",
            WINRECONFIG_EARLY_APPLY_MS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_winreconfig_early_apply_rect",
            WINRECONFIG_EARLY_APPLY_RECT.load(Ordering::SeqCst),
        );
        // Early self-present pump: frames WE presented before the game's first own present, the
        // pump-relative ms the swapchain was found, and why the pump stopped (1 = game took over,
        // the success terminal state; 2 = budget; 3 = Present HRESULT failure).
        push_json_usize(
            body,
            "oracle_boot_view_self_presents",
            BOOT_VIEW_SELF_PRESENTS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_boot_view_swapchain_found_ms",
            BOOT_VIEW_SWAPCHAIN_FOUND_MS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_boot_view_pump_stop_reason",
            BOOT_VIEW_PUMP_STOP_REASON.load(Ordering::SeqCst),
        );
        // DEPTH-KEY transparent-background semaphores: frames where the depth key actually cut out a
        // background (clean bg/head depth separation + >0 pixels alpha'd to 0), and the last frame's
        // background-masked fraction in whole percent. A RAM/pixel oracle for the transparent bg cutout.
        push_json_usize(
            body,
            "oracle_depth_key_applied",
            DEPTH_KEY_APPLIED.load(Ordering::SeqCst),
        );
        // Coherent color+depth readback engagement (bug #3): _ok = draw ticks the single-fence path
        // captured color+depth together (from the deterministic bundle-paired depth); _fallback = ticks
        // it degraded to the separate color/depth reads. A high _ok:_fallback ratio proves the coherent
        // path is actually running (the first pass had no way to tell).
        push_json_usize(
            body,
            "oracle_portrait_coherent_read_ok",
            COHERENT_READ_OK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_coherent_read_fallback",
            COHERENT_READ_FALLBACK.load(Ordering::SeqCst),
        );
        // FAIL-FAST 2nd-character desync semaphore: >0 means a frame reused a depth mask computed for a
        // DIFFERENT character incarnation (prior character's silhouette on the new head). During the
        // System-Quit repro this also abort()s the process on first trip, so the run stops in ~40s.
        push_json_usize(
            body,
            "oracle_portrait_mask_stale_reuse",
            PROFILE_MASK_STALE_REUSE.load(Ordering::SeqCst),
        );
        // FAIL-FAST mask/head coherence (2nd-character desync): _iou_last = last frame's IoU of the kept
        // cutout vs the colour head (100=perfect match, low=the cutout doesn't match this head);
        // _mismatch_total = frames below the gross threshold. Lets the 1st-char (correct) vs 2nd-char
        // (desync) IoU be compared and the abort threshold calibrated.
        push_json_usize(
            body,
            "oracle_portrait_mask_head_iou_last",
            PROFILE_MASK_HEAD_IOU_LAST.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_mask_head_mismatch_total",
            PROFILE_MASK_HEAD_MISMATCH_TOTAL.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_depth_key_bg_pct",
            DEPTH_KEY_BG_PCT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_depth_key_fresh",
            DEPTH_KEY_FRESH.load(Ordering::SeqCst),
        );
        // The draw-tick readback's per-frame republish count (==RGBA version). If ~= render_drive_hits the
        // readback publishes per-frame (so a low overlay_reuploads means the composite upload is the
        // bottleneck); if this is itself ~4 the per-frame readback/publish is.
        push_json_usize(
            body,
            "oracle_loading_bg_portrait_rgba_version",
            LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
        );
        // LOADING-SCREEN PORTRAIT BUG SEMAPHORES (2026-07-04). Detection runs at CAPTURE time
        // (`note_ls_portrait_capture`, called wherever a portrait RGBA is stored) so a transient
        // wrong-source frame -- our neutral texture (RGB 30,28,26) flashing in right after Continue (Bug
        // B), or a too-small 256px head (Bug A) -- cannot slip between telemetry writes. Here we just
        // publish the latched values.
        push_json_usize(
            body,
            "oracle_ls_portrait_w",
            LS_PORTRAIT_LAST_W.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_ls_portrait_h",
            LS_PORTRAIT_LAST_H.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_ls_portrait_neutral_pct",
            LS_PORTRAIT_LAST_NEUTRAL_PCT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_ls_portrait_too_small_seen_version",
            LS_PORTRAIT_TOO_SMALL_SEEN_VERSION.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_ls_portrait_neutral_leak_seen_version",
            LS_PORTRAIT_NEUTRAL_LEAK_SEEN_VERSION.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_ls_portrait_rejected_publishes",
            LS_PORTRAIT_REJECTED_PUBLISHES.load(Ordering::SeqCst),
        );
        // CROSS-SLOT SWAP tripwires: the pinned content-RT candidate (0 = never latched a confirmed head),
        // how many times the pin MOVED after first latch (>0 in one load window = unstable content source,
        // the swap bug's signature), how many per-slot target build kicks fired (0 = the loaded character
        // was never requested), and the max count of NON-target renderers seen holding a live model during
        // the feed window (>0 = a foreign character built on the loading screen -- the swap precondition).
        // LOADING-COVER EXPERIMENT: frames the cover-suppress clamp actually cleared visible (0 with the
        // gate on = the cover object never resolved / was never raised).
        push_json_usize(
            body,
            "oracle_loading_cover_suppress_writes",
            LOADING_COVER_SUPPRESS_WRITES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_rt_pin",
            PROFILE_RT_PIN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_rt_pin_switches",
            PROFILE_RT_PIN_SWITCHES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_target_kicks",
            PROFILE_TARGET_KICKS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_foreign_models",
            PROFILE_FOREIGN_MODELS_MAX.load(Ordering::SeqCst),
        );
        // Scaleform menu-handler lifecycle guard (repeated-switch ProfileSelect UAF). double_frees > 0
        // proves the guard caught+skipped the crash; ctors/dtors give the churn context.
        push_json_usize(
            body,
            "oracle_scaleform_handler_double_frees",
            SCALEFORM_HANDLER_DOUBLE_FREES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_scaleform_handler_ctors",
            SCALEFORM_HANDLER_CTORS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_scaleform_handler_dtors",
            SCALEFORM_HANDLER_DTORS.load(Ordering::SeqCst),
        );
        // Game-Options pane VISIBILITY oracle (READ-ONLY, blank Game Options pane detector): on
        // OptionSetting re-entry the DLL reads each option pane's DisplayInfo.Visible. blank_detected
        // > 0 = the WindowList container resolved in the tree but its pane was not visible (tabs/footer
        // render, row list black); resolved/visible masks + last_datatype + guard_skips give context.
        push_json_usize(
            body,
            "oracle_optionsetting_pane_sample_count",
            OPTIONSETTING_PANE_SAMPLE_COUNT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_pane_windowlist_resolved",
            OPTIONSETTING_PANE_LAST_WINDOWLIST_RESOLVED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_pane_windowlist_visible",
            OPTIONSETTING_PANE_LAST_WINDOWLIST_VISIBLE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_pane_resolved_mask",
            OPTIONSETTING_PANE_LAST_RESOLVED_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_pane_visible_mask",
            OPTIONSETTING_PANE_LAST_VISIBLE_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_pane_last_datatype",
            OPTIONSETTING_PANE_LAST_DATATYPE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_pane_guard_skips",
            OPTIONSETTING_PANE_GUARD_SKIPS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_pane_composite_bound",
            OPTIONSETTING_PANE_COMPOSITE_BOUND.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_pane_blank_detected_count",
            OPTIONSETTING_PANE_BLANK_DETECTED_COUNT.load(Ordering::SeqCst),
        );
        // REAL row-pane signal: current tab dialog (composite+0xb8) and its pane proxy (dialog+0x1200)
        // DisplayInfo.Visible -- the object the game's tab-select actually toggles. real_blank_detected
        // fires only after a healthy (visible) pane was seen and then the actively-shown pane went hidden,
        // so it cannot false-fire on boot/preload (unlike the named-child mask above).
        push_json_usize(
            body,
            "oracle_optionsetting_current_dialog",
            OPTIONSETTING_CURRENT_DIALOG.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_current_pane_visible",
            OPTIONSETTING_CURRENT_PANE_VISIBLE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_current_pane_datatype",
            OPTIONSETTING_CURRENT_PANE_DATATYPE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_actively_shown",
            OPTIONSETTING_ACTIVELY_SHOWN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_last_flag",
            OPTIONSETTING_LAST_FLAG.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_current_pane_ever_visible",
            OPTIONSETTING_CURRENT_PANE_EVER_VISIBLE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_real_blank_detected_count",
            OPTIONSETTING_REAL_BLANK_DETECTED_COUNT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_current_tab",
            OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_current_tab_at_blank",
            OPTIONSETTING_CURRENT_TAB_AT_BLANK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_pane_fix_applied",
            OPTIONSETTING_PANE_FIX_APPLIED.load(Ordering::SeqCst),
        );
        // Active OptionSetting row-table oracle: classifies the currently visible tab dialog's rows by
        // action pointers. tab 0 with cloned_mask!=0 is the Game Options contamination bug; Quit tab
        // with missing cloned_mask is the "feature not injected" bug. This is read-only and independent
        // of screenshot/OCR.
        push_json_usize(
            body,
            "oracle_optionsetting_active_row_sample_count",
            OPTIONSETTING_ACTIVE_ROW_SAMPLE_COUNT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_active_row_dialog",
            OPTIONSETTING_ACTIVE_ROW_DIALOG.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_active_row_tab",
            OPTIONSETTING_ACTIVE_ROW_TAB.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_active_row_count",
            OPTIONSETTING_ACTIVE_ROW_COUNT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_active_row_cloned_mask",
            OPTIONSETTING_ACTIVE_ROW_CLONED_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_active_row_native_save_mask",
            OPTIONSETTING_ACTIVE_ROW_NATIVE_SAVE_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_active_row_action_hash",
            OPTIONSETTING_ACTIVE_ROW_ACTION_HASH.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_active_row_label_hash",
            OPTIONSETTING_ACTIVE_ROW_LABEL_HASH.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_active_row_quit_label_mask",
            OPTIONSETTING_ACTIVE_ROW_QUIT_LABEL_MASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_game_options_cloned_row_hits",
            OPTIONSETTING_GAME_OPTIONS_CLONED_ROW_HITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_optionsetting_game_options_quit_label_hits",
            OPTIONSETTING_GAME_OPTIONS_QUIT_LABEL_HITS.load(Ordering::SeqCst),
        );
        // GX command-queue overflow forensics (repeated-switch crash 0x1aeaf05): max_fill climbing
        // toward cap across switches = the accumulating-producer signature; top_producers names the
        // caller RVAs (entries tagged +self passed through our DLL).
        push_json_usize(
            body,
            "oracle_gx_cmdqueue_cap",
            GX_CMD_QUEUE_CAP_SEEN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gx_cmdqueue_max_fill",
            GX_CMD_QUEUE_MAX_FILL.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gx_cmdqueue_switch_max_fill",
            GX_CMD_QUEUE_SWITCH_MAX_FILL.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gx_cmdqueue_reserves",
            GX_CMD_QUEUE_SUBMITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gx_cmdqueue_nearfull_hits",
            GX_CMD_QUEUE_NEARFULL_HITS.load(Ordering::SeqCst),
        );
        // Repeated-switch spared-renderer leak fix: renderers reclaimed via CSDelayDeleteMan (should
        // rise ~1/switch) and the count currently spared -- proves the orphan accumulation is capped.
        push_json_usize(
            body,
            "oracle_profile_spare_orphans_deleted",
            PROFILE_SPARE_ORPHANS_DELETED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_profile_renderer_spare_hits",
            PROFILE_RENDERER_SPARE_HITS.load(Ordering::SeqCst),
        );
        // Ownership-ledger conservation oracle: violations MUST stay 0 (nonzero == a native-owned
        // object taken without a paired release -- the spared-renderer leak class). spared_outstanding
        // and its high-water should track the bound (1); a climbing value is the early leak signal.
        push_json_usize(
            body,
            "oracle_ownership_ledger_violations",
            OWNED_LEDGER_VIOLATIONS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_ownership_spared_outstanding",
            crate::experiments::ownership_outstanding(crate::constants::OwnedClass::SparedRenderer),
        );
        push_json_usize(
            body,
            "oracle_ownership_spared_max_outstanding",
            OWNED_MAX_OUTSTANDING[crate::constants::OwnedClass::SparedRenderer as usize]
                .load(Ordering::SeqCst),
        );
        // Loading-portrait select-then-show: retargets = confirm-time swaps to the newly-selected
        // character; skipped_unkeyed = frames NOT published because the depth mask was not applied yet
        // (never render an unmasked model); have_keyed = a masked frame is available to display.
        push_json_usize(
            body,
            "oracle_portrait_retargets",
            PROFILE_PORTRAIT_RETARGETS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_publish_skipped_unkeyed",
            PROFILE_PUBLISH_SKIPPED_UNKEYED.load(Ordering::SeqCst),
        );
        // HARNESS-FAILURE semaphore (user directive 2026-07-06): windows that drove the model but
        // published no portrait. The readiness watcher fails the run when this is non-zero -- the
        // publish gates must never silently degrade the product; drive this to 0 by fixing the root
        // render (per-cause in `..._fail_cause`: 1=torn 2=unkeyed 3=badiou 4=lowmask).
        push_json_usize(
            body,
            "oracle_portrait_window_publish_failures",
            PORTRAIT_WINDOW_PUBLISH_FAILURES.load(Ordering::SeqCst),
        );
        // READBACK STALL SPLIT (diagnostic): average microseconds per coherent readback for the GPU-WAIT
        // (removable by an async ring buffer) vs the CPU de-swizzle + mask/key (stay on the render
        // thread). Decides how close to the ~7.5s floor an async readback can get before the CPU pass
        // becomes the residual bottleneck.
        {
            let n = PORTRAIT_RB_COUNT.load(Ordering::SeqCst).max(1);
            let mn = PORTRAIT_RB_MASK_COUNT.load(Ordering::SeqCst).max(1);
            push_json_usize(body, "oracle_portrait_rb_count", PORTRAIT_RB_COUNT.load(Ordering::SeqCst));
            push_json_usize(
                body,
                "oracle_portrait_rb_wait_avg_us",
                PORTRAIT_RB_WAIT_US_SUM.load(Ordering::SeqCst) / n,
            );
            push_json_usize(
                body,
                "oracle_portrait_rb_deswizzle_avg_us",
                PORTRAIT_RB_DESWIZZLE_US_SUM.load(Ordering::SeqCst) / n,
            );
            push_json_usize(
                body,
                "oracle_portrait_rb_mask_avg_us",
                PORTRAIT_RB_MASK_US_SUM.load(Ordering::SeqCst) / mn,
            );
        }
        push_json_usize(
            body,
            "oracle_portrait_window_publish_fail_cause",
            PORTRAIT_WINDOW_PUBLISH_FAIL_CAUSE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_have_keyed_frame",
            PROFILE_HAVE_KEYED_FRAME.load(Ordering::SeqCst),
        );
        // Torn-readback semaphore: tear score of the last publish attempt + the run max, plus how many
        // keyed frames were skipped as torn vs published clean. A high max with clean>0 means clean
        // frames DO land (gate suffices); clean==0 with high max means every driven frame tears (the
        // readback needs real GPU sync). clean_min is the lowest clean score seen (baseline).
        push_json_usize(
            body,
            "oracle_portrait_tear_last",
            PROFILE_TEAR_SCORE_LAST.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_tear_max",
            PROFILE_TEAR_SCORE_MAX.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_tear_clean_min",
            PROFILE_TEAR_SCORE_CLEAN_MIN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_publish_clean",
            PROFILE_PUBLISH_CLEAN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_publish_skipped_torn",
            PROFILE_PUBLISH_SKIPPED_TORN.load(Ordering::SeqCst),
        );
        // Animation-stall: last loading window's animated (drive) vs displayed frames. drive<<display
        // means the head froze early (freeze-after-capture) -- the user's "stopped animating" symptom.
        push_json_usize(
            body,
            "oracle_portrait_drive_frames_last_window",
            PROFILE_DRIVE_FRAMES_WINDOW_LAST.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_display_frames_last_window",
            PROFILE_DISPLAY_FRAMES_WINDOW_LAST.load(Ordering::SeqCst),
        );
        // Teardown-fence protocol (freeze relaxation): skips = pump frames yielded to a live
        // teardown; waits = teardowns that paused for a mid-drive pump; timeouts MUST stay 0
        // (nonzero == one frame of the old TOCTOU exposure leaked past the 10ms cap).
        push_json_usize(
            body,
            "oracle_portrait_drive_fence_skips",
            PROFILE_DRIVE_FENCE_SKIPS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_teardown_fence_waits",
            PROFILE_TEARDOWN_FENCE_WAITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_teardown_fence_timeouts",
            PROFILE_TEARDOWN_FENCE_TIMEOUTS.load(Ordering::SeqCst),
        );
        // Color/depth source provenance (green-face wrong-buffer fix): only bundle-provenance color
        // may display; unpaired counts real frames held back for lacking it.
        push_json_usize(
            body,
            "oracle_portrait_color_from_bundle",
            crate::experiments::PROFILE_COLOR_FROM_BUNDLE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_color_from_scan",
            crate::experiments::PROFILE_COLOR_FROM_SCAN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_depth_from_chain",
            crate::experiments::PROFILE_DEPTH_FROM_CHAIN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_depth_from_bfs",
            crate::experiments::PROFILE_DEPTH_FROM_BFS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_publish_skipped_unpaired",
            crate::experiments::PROFILE_PUBLISH_SKIPPED_UNPAIRED.load(Ordering::SeqCst),
        );
        // hi2: partial-mask band (mask cut something but under the floor) + how long the bridge
        // held before the window's first publish.
        push_json_usize(
            body,
            "oracle_portrait_publish_skipped_lowmask",
            PROFILE_PUBLISH_SKIPPED_LOWMASK.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_first_keyed_display_last_window",
            PROFILE_WINDOW_FIRST_KEYED_DISPLAY_LAST.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_depth_key_degenerate",
            crate::experiments::DEPTH_KEY_DEGENERATE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_depth_key_second_pass",
            crate::experiments::DEPTH_KEY_SECOND_PASS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_publish_skipped_badiou",
            crate::experiments::PROFILE_PUBLISH_SKIPPED_BADIOU.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_alpha0_clears",
            PROFILE_ALPHA0_CLEARS.load(Ordering::SeqCst),
        );
        push_json_str(
            body,
            "oracle_gx_cmdqueue_top_producers",
            &crate::experiments::gx_cmd_queue_hist_top(8),
        );
        push_json_str(
            body,
            "oracle_gx_cmdqueue_buckets",
            &crate::experiments::gx_cmd_queue_bucket_summary(),
        );
        push_json_usize(
            body,
            "oracle_gx_cmdarena_min_remaining",
            GX_CMD_ARENA_MIN_REMAINING.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gx_cmdarena_switch_min_remaining",
            GX_CMD_ARENA_SWITCH_MIN_REMAINING.load(Ordering::SeqCst),
        );
        {
            let (dd_pending, dd_highwater) = unsafe { crate::experiments::delay_delete_pending() }
                .map(|(p, h)| (p as i64, h as i64))
                .unwrap_or((-1, -1));
            body.push_str(&format!(
                "  \"oracle_delaydelete_pending\": {dd_pending},\n"
            ));
            body.push_str(&format!(
                "  \"oracle_delaydelete_highwater\": {dd_highwater},\n"
            ));
        }
        push_json_usize(
            body,
            "oracle_portrait_multi_model_publish_skips",
            PROFILE_MULTI_MODEL_PUBLISH_SKIPS.load(Ordering::SeqCst),
        );
        // IDLE-ANIM BIND semaphores (bd portrait-anim-bind-RE-corrects-6hz-gate-2026-07-03):
        // bind_state 1 = an engine-grounded idle anim bound (handle real), 2 = no candidate resolved;
        // handle_before != sentinel proves the native static-pose anim-0 bind had resolved (anim
        // resources ARE loaded); sentinel is the DAT_143b39470 null-handle global (constant if the
        // corrected RE is right). MOTION vs FLICKER: motion_metric diffs the depth-keyed ALPHA
        // silhouette (lighting-immune), luma_flicker diffs luma on the same grid (quantifies the
        // per-frame lighting change). Product proof of "portrait animates" = bind_state 1 AND
        // motion_metric_max clearly above 0 with luma_flicker as the lighting control.
        push_json_usize(
            body,
            "oracle_portrait_anim_bind_state",
            PORTRAIT_ANIM_BIND_STATE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_facedata_neq_ticks",
            PORTRAIT_FACEDATA_NEQ_TICKS.load(Ordering::SeqCst),
        );
        // FACE-IDENTITY semaphore (user directive 2026-07-06): at each build kick for a slot owned by a
        // foreign-save preview, the record's inner FaceDataBuffer is re-hashed against the fingerprint
        // stored when the preview wrote it. `mismatches > 0` == the portrait was about to render a
        // DIFFERENT character's face than the one the user picked -- fail-fast signal for probe watchers.
        push_json_usize(
            body,
            "oracle_portrait_face_identity_checks",
            PORTRAIT_FACE_IDENTITY_CHECKS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_face_identity_mismatches",
            PORTRAIT_FACE_IDENTITY_MISMATCHES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_pump_draws",
            PROFILE_PERFRAME_MODEL_DRAWS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_pump_block_r",
            PORTRAIT_PUMP_BLOCK_R.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_pump_block_vtable",
            PORTRAIT_PUMP_BLOCK_VTABLE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_pump_block_off",
            PORTRAIT_PUMP_BLOCK_OFF.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_pump_block_multi",
            PORTRAIT_PUMP_BLOCK_MULTI.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_drive_ticks",
            PORTRAIT_DRIVE_TICKS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_anim_bind_attempts",
            PORTRAIT_ANIM_BIND_ATTEMPTS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_anim_bound_id",
            PORTRAIT_ANIM_BOUND_ID.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_anim_handle_before",
            PORTRAIT_ANIM_HANDLE_BEFORE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_anim_handle",
            PORTRAIT_ANIM_HANDLE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_anim_sentinel",
            PORTRAIT_ANIM_SENTINEL.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_motion_metric_last",
            PORTRAIT_MOTION_METRIC_LAST.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_motion_metric_max",
            PORTRAIT_MOTION_METRIC_MAX.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_luma_flicker_last",
            PORTRAIT_LUMA_FLICKER_LAST.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_luma_flicker_max",
            PORTRAIT_LUMA_FLICKER_MAX.load(Ordering::SeqCst),
        );
        // LOADING-SCREEN WINDOW semaphores: overlay stop count + last stop reason (1 = load-done bridge
        // elapsed; 3 = anti-runaway backstop; 4 = native now-loading Gauge_3 terminal frame / visible
        // loading bar reached 100%).
        push_json_usize(
            body,
            "oracle_overlay_window_stops",
            OVERLAY_WINDOW_STOPS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_overlay_stop_reason",
            OVERLAY_STOP_REASON.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_loading_bar_hook_installed",
            LOADING_SCREEN_UPDATE_HOOK_INSTALLED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_loading_bar_update_hits",
            LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_loading_bar_enabled",
            LOADING_SCREEN_BAR_ENABLED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_loading_bar_current_frame",
            LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_loading_bar_max_frame",
            LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_loading_bar_progress_permille",
            LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_loading_bar_final_hits",
            LOADING_SCREEN_BAR_FINAL_HITS.load(Ordering::SeqCst),
        );
        // CANDIDATE A (er-effects-rs-jsm): live head copied INTO the displayed now-loading GFx texture so
        // the native tips/bar render above it. `uploads > 0` == the head is in the movie; `overlay_yields`
        // proves the Present-overlay demoted (stopped drawing over the tips); `demote_credit` is the live
        // handoff level; `last_error` names the current fail-open reason (0 = ok).
        push_json_usize(
            body,
            "oracle_gfx_portrait_uploads",
            GFX_PORTRAIT_UPLOADS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_resolves",
            GFX_PORTRAIT_RESOLVES.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_resolve_fails",
            GFX_PORTRAIT_RESOLVE_FAILS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_overlay_yields",
            GFX_PORTRAIT_OVERLAY_YIELDS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_demote_credit",
            GFX_PORTRAIT_DEMOTE_CREDIT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_hal_dims",
            GFX_PORTRAIT_HAL_DIMS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_cached_hal",
            GFX_PORTRAIT_CACHED_HAL.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_last_error",
            GFX_PORTRAIT_LAST_ERROR.load(Ordering::SeqCst),
        );
        // BAKE path: head baked into the forged now-loading background (proven display), and whether a
        // baked artwork was actually DISPLAYED (overlay demoted -> tips render above the in-movie head).
        push_json_usize(
            body,
            "oracle_gfx_portrait_baked",
            GFX_PORTRAIT_BAKED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_baked_displayed",
            GFX_PORTRAIT_BAKED_DISPLAYED.load(Ordering::SeqCst),
        );
        // PIXEL ORACLE: did the head actually reach the loading screen (backbuffer readback vs the
        // captured head, excluding the tip/bar rects)? Resource-agnostic regression guard.
        push_json_usize(
            body,
            "oracle_gfx_portrait_head_on_screen",
            GFX_PORTRAIT_HEAD_ON_SCREEN.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_head_match_pct",
            GFX_PORTRAIT_HEAD_MATCH_PCT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_head_probe_count",
            GFX_PORTRAIT_HEAD_PROBE_COUNT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_gfx_portrait_head_ever",
            GFX_PORTRAIT_HEAD_EVER.load(Ordering::SeqCst),
        );
        // PIVOT (er-effects-rs-jsm): player-stats loading text. `stats_text_built` = cumulative count of
        // stats bitmaps rendered from the game font (content-keyed rebuilds: a character switch or the
        // record->live upgrade bumps it); `tip_suppressed_hits` = native tip-refresh calls we no-op'd.
        push_json_usize(
            body,
            "oracle_stats_text_built",
            STATS_TEXT_BUILT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_tip_suppressed_hits",
            KNOWLEDGE_TIP_SUPPRESSED_HITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_tip_suppress_installed",
            KNOWLEDGE_TIP_REFRESH_INSTALLED.load(Ordering::SeqCst),
        );
        // `tip_advance_disable_installed` = the advance enabled-predicate detour is live;
        // `tip_advance_suppressed_hits` = predicate calls we forced false (keyguide hidden + press inert).
        push_json_usize(
            body,
            "oracle_tip_advance_disable_installed",
            KNOWLEDGE_TIP_ADVANCE_ENABLED_INSTALLED.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_tip_advance_suppressed_hits",
            KNOWLEDGE_TIP_ADVANCE_SUPPRESSED_HITS.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_overlay_gpu_fail_count",
            OVERLAY_GPU_FAIL_COUNT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_overlay_gpu_fail_code",
            OVERLAY_GPU_FAIL_CODE.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_overlay_gpu_fail_version",
            OVERLAY_GPU_FAIL_VERSION.load(Ordering::SeqCst),
        );
        body.push_str(&format!(
            "  \"oracle_native_profile_capture_enabled\": {},\n  \"oracle_native_load_game_fired\": {},\n  \"oracle_native_load_game_last_node\": {},\n  \"oracle_native_load_game_last_node_vtable\": {},\n  \"oracle_native_load_game_last_member_dialog\": {},\n  \"oracle_native_load_game_last_member_fn\": {},\n  \"oracle_native_load_game_last_member_adjust\": {},\n  \"oracle_native_profile_source_ready\": {},\n  \"oracle_native_profile_source_name\": \"{}\",\n  \"oracle_native_profile_renderer_class\": \"{}\",\n",
            native_profile_capture_enabled(),
            NATIVE_LOAD_FIRED.load(Ordering::SeqCst) == NATIVE_LOAD_FIRED_YES,
            NATIVE_LOAD_LAST_NODE.load(Ordering::SeqCst),
            NATIVE_LOAD_LAST_NODE_VTABLE.load(Ordering::SeqCst),
            NATIVE_LOAD_LAST_MEMBER_DIALOG.load(Ordering::SeqCst),
            NATIVE_LOAD_LAST_MEMBER_FN.load(Ordering::SeqCst),
            NATIVE_LOAD_LAST_MEMBER_ADJUST.load(Ordering::SeqCst),
            title_custom_cover_profile_source_ready,
            TITLE_CUSTOM_COVER_SYSTEX_TARGET,
            TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS,
        ));
    }
}
