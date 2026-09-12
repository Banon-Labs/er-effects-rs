// Render-state oracles: the CSNowLoadingHelper load-done latch, the fake-loading-screen dialog,
// and the render-manager / CSGraphics / Scaleform singleton slot pointers behind them.
//
// Grouped because every field here answers the same question -- "is the game's own loading/render
// stack up, and what is it holding" -- and because they share the `read_rend` reader and the
// pointer formatter. Nothing defined here is read by a later subsystem.

fn write_render_state_oracles(body: &mut String, base: usize) {
    const NULL_PTR: usize = 0;
    const READ_FAIL_SENTINEL: i32 = -1;
    let format_optional_ptr = format_optional_oracle_ptr;
    // World-live oracle: CSNowLoadingHelper "now loading" latch = *(u8*)([base+0x3d60ec8]+0xED).
    // NOTE (RE-corrected 2026-07-02): this reads `CSNowLoadingHelperImp::load_done` -- a load-complete
    // latch, not "loading screen visible." `Update` copies it from `request_load_done` (raised by the
    // map-load system), so it reads true after the load finishes and lingers into gameplay. Kept as a
    // telemetry field, but do not treat it as a screen-visibility signal (see CSNowLoadingHelperImp).
    const NOW_LOADING_SINGLETON_RVA: usize = RuntimeGlobalRva::NowLoadingSingleton as usize;
    const NOW_LOADING_FLAG_OFFSET: usize = core::mem::offset_of!(CSNowLoadingHelperImp, load_done);
    const NOW_LOADING_BYTE_MASK: usize = u8::MAX as usize;
    let now_loading = {
        let helper = unsafe {
            crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                NOW_LOADING_SINGLETON_RVA,
                "NOW_LOADING_SINGLETON_RVA",
            ))
        }
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
        crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            FAKE_LOADING_SCREEN_SINGLETON_RVA,
            "FAKE_LOADING_SCREEN_SINGLETON_RVA",
        ))
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
    let fake_loading_visible_samples = FAKE_LOADING_SCREEN_VISIBLE_SAMPLES.load(Ordering::SeqCst);
    const RENDMAN_SINGLETON_RVA: usize = RuntimeGlobalRva::RendManSingleton as usize;
    const CSGRAPHICS_SINGLETON_RVA: usize = RuntimeGlobalRva::CsGraphicsSingleton as usize;
    const CSSCALEFORM_SINGLETON_RVA: usize = RuntimeGlobalRva::CsScaleformSingleton as usize;
    let rendman = unsafe {
        crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            RENDMAN_SINGLETON_RVA,
            "RENDMAN_SINGLETON_RVA",
        ))
    }
    .unwrap_or(NULL_PTR);
    let csgraphics = unsafe {
        crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CSGRAPHICS_SINGLETON_RVA,
            "CSGRAPHICS_SINGLETON_RVA",
        ))
    }
    .unwrap_or(NULL_PTR);
    let csscaleform = unsafe {
        crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CSSCALEFORM_SINGLETON_RVA,
            "CSSCALEFORM_SINGLETON_RVA",
        ))
    }
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
}
