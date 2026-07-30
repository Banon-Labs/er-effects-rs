use crate::prelude::*;

pub unsafe fn read_native_dlstring_ascii_ptr(s: usize) -> usize {
    if s == 0 || s == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let capacity = unsafe { safe_read_usize(s + 0x20) }.unwrap_or(0);
    if capacity <= 0xf {
        s + 0x8
    } else {
        unsafe { safe_read_usize(s + 0x8) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    }
}

pub unsafe fn bounded_ascii_contains(ptr: usize, needle: &[u8]) -> bool {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || needle.is_empty() {
        return false;
    }
    let mut window = [0u8; 32];
    let mut n = 0usize;
    for i in 0..96usize {
        let Some(b) = (unsafe { safe_read_u8(ptr + i) }) else {
            break;
        };
        if b == 0 {
            break;
        }
        if n < window.len() {
            window[n] = b.to_ascii_lowercase();
            n += 1;
        } else {
            window.rotate_left(1);
            window[window.len() - 1] = b.to_ascii_lowercase();
        }
        let hay = &window[..n.min(window.len())];
        if hay.windows(needle.len()).any(|w| w == needle) {
            return true;
        }
    }
    false
}

pub unsafe fn copy_ascii_preview(ptr: usize, out: &mut [u8]) -> usize {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || out.is_empty() {
        return 0;
    }
    let mut n = 0usize;
    while n + 1 < out.len() && n < 80 {
        let Some(b) = (unsafe { safe_read_u8(ptr + n) }) else {
            break;
        };
        if b == 0 {
            break;
        }
        out[n] = if b.is_ascii_graphic() || b == b' ' {
            b
        } else {
            b'?'
        };
        n += 1;
    }
    n
}

pub unsafe fn rewrite_native_dlstring_ascii(s: usize, value: &str) -> Option<usize> {
    if s == 0 || s == TITLE_OWNER_SCAN_START_ADDRESS || !value.is_ascii() {
        return None;
    }
    let len = value.len();
    let capacity = unsafe { safe_read_usize(s + 0x20) }?;
    if capacity < len {
        return None;
    }
    let dst = if capacity <= 0xf {
        s + 0x8
    } else {
        unsafe { safe_read_usize(s + 0x8) }?
    };
    if dst == 0 || dst == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    for (idx, byte) in value.as_bytes().iter().copied().enumerate() {
        unsafe { ((dst + idx) as *mut u8).write_volatile(byte) };
    }
    unsafe { ((dst + len) as *mut u8).write_volatile(0) };
    unsafe { ((s + 0x18) as *mut usize).write_volatile(len) };
    Some(dst)
}

unsafe fn sample_now_loading_helper(this: usize) {
    if this == 0 || this == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    NOW_LOADING_HELPER_LAST_THIS.store(this, Ordering::SeqCst);
    NOW_LOADING_HELPER_LAST_MENU_INDEX.store(
        unsafe { safe_read_usize(this + 0xd0) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
        Ordering::SeqCst,
    );
    NOW_LOADING_HELPER_LAST_REPLACE_TEX_INFO.store(
        unsafe { safe_read_usize(this + 0xd8) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
        Ordering::SeqCst,
    );
    NOW_LOADING_HELPER_LAST_REQUESTED_REPLACE_TEX_INFO.store(
        unsafe { safe_read_usize(this + 0xe0) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
        Ordering::SeqCst,
    );
    let request_done = unsafe { safe_read_u8(this + 0xec) }.unwrap_or(0) as usize;
    let load_done = unsafe { safe_read_u8(this + 0xed) }.unwrap_or(0) as usize;
    NOW_LOADING_HELPER_LAST_FLAGS.store(request_done | (load_done << 8), Ordering::SeqCst);
}

pub unsafe extern "system" fn now_loading_helper_ctor_hook(this: usize) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = NOW_LOADING_HELPER_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(this) }
    } else {
        this
    };
    let hits = NOW_LOADING_HELPER_CTOR_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    unsafe { sample_now_loading_helper(ret) };
    if hits <= 4 {
        append_autoload_debug(format_args!(
            "title-cover-part-b: observed CSNowLoadingHelperImp ctor this=0x{ret:x} hits={hits}; now-loading surface candidate for custom masquerade"
        ));
    }
    ret
}

pub unsafe extern "system" fn now_loading_helper_update_hook(this: usize, time: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = NOW_LOADING_HELPER_UPDATE_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize, usize) =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(this, time) };
    }
    let hits = NOW_LOADING_HELPER_UPDATE_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    unsafe { sample_now_loading_helper(this) };
    if hits <= 8 || hits.is_power_of_two() {
        append_autoload_debug(format_args!(
            "title-cover-part-b: observed CSNowLoadingHelperImp update this=0x{this:x} hits={hits} menu_index=0x{:x} replace=0x{:x} requested=0x{:x} flags=0x{:x}",
            NOW_LOADING_HELPER_LAST_MENU_INDEX.load(Ordering::SeqCst),
            NOW_LOADING_HELPER_LAST_REPLACE_TEX_INFO.load(Ordering::SeqCst),
            NOW_LOADING_HELPER_LAST_REQUESTED_REPLACE_TEX_INFO.load(Ordering::SeqCst),
            NOW_LOADING_HELPER_LAST_FLAGS.load(Ordering::SeqCst),
        ));
    }
}

unsafe fn loading_screen_progress_permille(data: usize) -> usize {
    if data == 0 || data == TITLE_OWNER_SCAN_START_ADDRESS {
        return 0;
    }
    let active_idx =
        unsafe { safe_read_i32(data + LOADING_SCREEN_DATA_ACTIVE_INDEX_OFFSET) }.unwrap_or(-1);
    if active_idx < 0 {
        return 0;
    }
    let start =
        unsafe { safe_read_f32(data + LOADING_SCREEN_DATA_START_PROGRESS_OFFSET) }.unwrap_or(0.0);
    let target =
        unsafe { safe_read_f32(data + LOADING_SCREEN_DATA_TARGET_PROGRESS_OFFSET) }.unwrap_or(0.0);
    let duration =
        unsafe { safe_read_f32(data + LOADING_SCREEN_DATA_INTERP_DURATION_OFFSET) }.unwrap_or(0.0);
    let elapsed =
        unsafe { safe_read_f32(data + LOADING_SCREEN_DATA_INTERP_ELAPSED_OFFSET) }.unwrap_or(0.0);
    let progress = if duration <= 0.0 {
        start
    } else if duration < elapsed {
        target
    } else {
        (target - start) * (elapsed / duration) + start
    };
    (progress.clamp(0.0, 1.0) * 1000.0).round() as usize
}

unsafe fn sample_loading_screen_bar(this: usize) {
    if this == 0 || this == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    LOADING_SCREEN_LAST_THIS.store(this, Ordering::SeqCst);
    let data = unsafe { safe_read_usize(this + LOADING_SCREEN_DATA_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    LOADING_SCREEN_LAST_DATA.store(data, Ordering::SeqCst);
    let enabled =
        unsafe { safe_read_u8(this + LOADING_SCREEN_GAUGE_ENABLED_OFFSET) }.unwrap_or(0) as usize;
    LOADING_SCREEN_BAR_ENABLED.store(enabled, Ordering::SeqCst);
    let component = this + LOADING_SCREEN_GAUGE_COMPONENT_OFFSET;
    let current = unsafe { safe_read_i32(component + MENU_FRAME_COMPONENT_CURRENT_FRAME_OFFSET) }
        .unwrap_or(0)
        .max(0) as usize;
    let max = unsafe { safe_read_i32(component + MENU_FRAME_COMPONENT_MAX_FRAME_OFFSET) }
        .unwrap_or(0)
        .max(0) as usize;
    LOADING_SCREEN_BAR_CURRENT_FRAME.store(current, Ordering::SeqCst);
    LOADING_SCREEN_BAR_MAX_FRAME.store(max, Ordering::SeqCst);
    let progress_pm = unsafe { loading_screen_progress_permille(data) };
    LOADING_SCREEN_BAR_PROGRESS_PERMILLE.store(progress_pm, Ordering::SeqCst);
    let finish_sent =
        unsafe { safe_read_u8(this + LOADING_SCREEN_FINISH_SENT_OFFSET) }.unwrap_or(0) as usize;
    let prev_finish_sent = LOADING_SCREEN_CLOSE_SENT.swap(finish_sent, Ordering::SeqCst);
    if enabled != 0 && max != 0 && current >= max {
        let hits = LOADING_SCREEN_BAR_FINAL_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        if hits <= 4 || hits.is_power_of_two() {
            append_autoload_debug(format_args!(
                "loading-bar: native Gauge_3 reached terminal frame {current}/{max} (progress={} permille, this=0x{this:x})",
                progress_pm
            ));
        }
    }
    if finish_sent != 0 && prev_finish_sent == 0 {
        let now_ms = crate::boot_view_epoch_ms().max(1) as usize;
        let hits = LOADING_SCREEN_CLOSE_SENT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = LOADING_SCREEN_CLOSE_SENT_FIRST_MS.compare_exchange(
            0,
            now_ms,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        append_autoload_debug(format_args!(
            "loading-bar: native LoadingScreen finish/result sent (hits={hits}, frame={current}/{max}, progress={progress_pm}permille, this=0x{this:x}, now_ms={now_ms})"
        ));
    }
}

pub unsafe extern "system" fn loading_screen_update_hook(this: usize, dt: f32, param3: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = LOADING_SCREEN_UPDATE_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize, f32, usize) =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(this, dt, param3) };
    }
    let now_ms = crate::boot_view_epoch_ms().max(1) as usize;
    LOADING_SCREEN_UPDATE_LAST_MS.store(now_ms, Ordering::SeqCst);
    let hits = LOADING_SCREEN_UPDATE_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    unsafe { sample_loading_screen_bar(this) };
    if hits <= 8 || hits.is_power_of_two() {
        append_autoload_debug(format_args!(
            "loading-bar: observed CS::LoadingScreen update this=0x{this:x} hits={hits} enabled={} frame={}/{} progress={}permille",
            LOADING_SCREEN_BAR_ENABLED.load(Ordering::SeqCst),
            LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst),
            LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst),
            LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst),
        ));
    }
}

fn stamp_loading_gfx_fadeout(source: &str, this: usize, label: usize) {
    let now_ms = crate::boot_view_epoch_ms().max(1) as usize;
    let hits = LOADING_SCREEN_GFX_FADEOUT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    LOADING_SCREEN_GFX_FADEOUT_LAST_MS.store(now_ms, Ordering::SeqCst);
    let first = LOADING_SCREEN_GFX_FADEOUT_FIRST_MS.load(Ordering::SeqCst);
    if first == 0 {
        let _ = LOADING_SCREEN_GFX_FADEOUT_FIRST_MS.compare_exchange(
            0,
            now_ms,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    if hits <= 8 || hits.is_power_of_two() {
        append_autoload_debug(format_args!(
            "loading-bar: observed Scaleform FadeOut label via {source} (hits={hits}, this=0x{this:x}, label=0x{label:x}, now_ms={now_ms}, close_hits={})",
            LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst),
        ));
    }
}

pub unsafe extern "system" fn scaleform_label_goto_hook(this: usize, label: usize) {
    if unsafe { bounded_ascii_contains(label, b"fadeout") } {
        stamp_loading_gfx_fadeout("label-goto", this, label);
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = SCALEFORM_LABEL_GOTO_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize, usize) =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(this, label) };
    }
}

pub unsafe extern "system" fn loading_screen_gfx_fadeout_hook(this: usize) {
    stamp_loading_gfx_fadeout("knowledge-method", this, 0);
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = LOADING_SCREEN_GFX_FADEOUT_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
        unsafe { original(this) };
    }
}

pub fn install_now_loading_helper_observer_hooks() {
    if NOW_LOADING_HELPER_HOOKS_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: now-loading observer MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(ctor) = game_rva(NOW_LOADING_HELPER_CTOR_RVA as u32) else {
        return;
    };
    let Ok(update) = game_rva(NOW_LOADING_HELPER_UPDATE_RVA as u32) else {
        return;
    };
    let loading_update = match game_rva(LOADING_SCREEN_UPDATE_RVA as u32) {
        Ok(addr) => Some(addr),
        Err(_) => {
            append_autoload_debug(format_args!(
                "loading-bar: failed to resolve CS::LoadingScreen update rva 0x{LOADING_SCREEN_UPDATE_RVA:x}"
            ));
            None
        }
    };
    let scaleform_label_goto = match game_rva(SCALEFORM_LABEL_GOTO_RVA as u32) {
        Ok(addr) => Some(addr),
        Err(_) => {
            append_autoload_debug(format_args!(
                "loading-bar: failed to resolve Scaleform label goto rva 0x{SCALEFORM_LABEL_GOTO_RVA:x}"
            ));
            None
        }
    };
    let loading_gfx_fadeout = match game_rva(LOADING_SCREEN_GFX_FADEOUT_RVA as u32) {
        Ok(addr) => Some(addr),
        Err(_) => {
            append_autoload_debug(format_args!(
                "loading-bar: failed to resolve KnowledgeLoadingScreen GFx FadeOut rva 0x{LOADING_SCREEN_GFX_FADEOUT_RVA:x}"
            ));
            None
        }
    };
    let mut ok = true;
    match unsafe {
        MhHook::new(
            ctor as *mut c_void,
            now_loading_helper_ctor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            NOW_LOADING_HELPER_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: now-loading ctor hook failed: {status:?}"
            ));
            ok = false;
        }
    }
    match unsafe {
        MhHook::new(
            update as *mut c_void,
            now_loading_helper_update_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            NOW_LOADING_HELPER_UPDATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: now-loading update hook failed: {status:?}"
            ));
            ok = false;
        }
    }
    if let Some(addr) = loading_update {
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                loading_screen_update_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                LOADING_SCREEN_UPDATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
                std::mem::forget(hook);
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "loading-bar: CS::LoadingScreen update hook failed: {status:?}"
                ));
                ok = false;
            }
        }
    }
    if let Some(addr) = scaleform_label_goto {
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                scaleform_label_goto_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                SCALEFORM_LABEL_GOTO_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
                std::mem::forget(hook);
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "loading-bar: Scaleform label goto hook failed: {status:?}"
                ));
                ok = false;
            }
        }
    }
    if let Some(addr) = loading_gfx_fadeout {
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                loading_screen_gfx_fadeout_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                LOADING_SCREEN_GFX_FADEOUT_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
                std::mem::forget(hook);
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "loading-bar: KnowledgeLoadingScreen GFx FadeOut hook failed: {status:?}"
                ));
                ok = false;
            }
        }
    }
    if !ok {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            NOW_LOADING_HELPER_HOOKS_INSTALLED.store(1, Ordering::SeqCst);
            if loading_update.is_some() {
                LOADING_SCREEN_UPDATE_HOOK_INSTALLED.store(1, Ordering::SeqCst);
            }
            if scaleform_label_goto.is_some() || loading_gfx_fadeout.is_some() {
                LOADING_SCREEN_GFX_FADEOUT_HOOK_INSTALLED.store(1, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "title-cover-part-b: hooked CSNowLoadingHelperImp observer ctor=0x{ctor:x} update=0x{update:x}; loading-bar-update={} scaleform-label-goto={} loading-gfx-fadeout={}; observe-only",
                loading_update.unwrap_or(0),
                scaleform_label_goto.unwrap_or(0),
                loading_gfx_fadeout.unwrap_or(0)
            ));
        }
        status => append_autoload_debug(format_args!(
            "title-cover-part-b: now-loading observer MH_ApplyQueued failed: {status:?}"
        )),
    }
}

pub unsafe fn wide_ascii_contains_ci(ptr: usize, needle: &[u8]) -> bool {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || needle.is_empty() {
        return false;
    }
    let mut hay = [0u8; 128];
    let mut n = 0usize;
    while n < hay.len() {
        let Some(ch) = (unsafe { safe_read_u16(ptr + n * core::mem::size_of::<u16>()) }) else {
            break;
        };
        if ch == 0 {
            break;
        }
        hay[n] = if ch <= 0x7f {
            (ch as u8).to_ascii_lowercase()
        } else {
            b'?'
        };
        n += 1;
    }
    hay[..n].windows(needle.len()).any(|w| w == needle)
}

pub unsafe fn copy_wide_ascii_preview(ptr: usize, out: &mut [u8]) -> usize {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || out.is_empty() {
        return 0;
    }
    let mut n = 0usize;
    while n + 1 < out.len() && n < 96 {
        let Some(ch) = (unsafe { safe_read_u16(ptr + n * core::mem::size_of::<u16>()) }) else {
            break;
        };
        if ch == 0 {
            break;
        }
        out[n] = if (0x20..=0x7e).contains(&ch) {
            ch as u8
        } else {
            b'?'
        };
        n += 1;
    }
    n
}

/// Read an incoming `DLString<wchar_t>` (the producer's symbol arg) into a `Vec<u16>` (no trailing
/// NUL) plus its encodingType byte. SSO-aware: the data is a heap pointer at `+0x8` when capacity
/// `> 7`, otherwise inline at `+0x8`.
unsafe fn read_dlstring_u16(s: usize) -> Option<(Vec<u16>, u8)> {
    if s == 0 || s == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let capacity = unsafe { safe_read_usize(s + DLSTRING_U16_CAPACITY_OFFSET) }?;
    let length = unsafe { safe_read_usize(s + DLSTRING_U16_LENGTH_OFFSET) }?;
    if length > 4096 {
        return None; // implausible symbol length
    }
    let data_ptr = if capacity > DLSTRING_U16_SSO_THRESHOLD {
        unsafe { safe_read_usize(s + DLSTRING_U16_INLINE_OFFSET) }?
    } else {
        s + DLSTRING_U16_INLINE_OFFSET
    };
    if data_ptr == 0 || data_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let mut out = Vec::with_capacity(length);
    for i in 0..length {
        out.push(unsafe { safe_read_u16(data_ptr + i * core::mem::size_of::<u16>()) }?);
    }
    let encoding = unsafe { safe_read_u8(s + DLSTRING_U16_ENCODING_OFFSET) }.unwrap_or(1);
    Some((out, encoding))
}

/// Absolute address of the profile renderer table entry for `slot` (`DAT_143d6d8d0[slot]`, the
/// `CSMenuProfModelRend*` for that ABSOLUTE save slot; offscreen tex index `slot*2`). Out-of-range
/// slots fall back to entry 0, preserving the historical table[0] behavior for `slot == 0` or unknown.
pub fn portrait_renderer_table_entry(base: usize, slot: i32) -> usize {
    let idx = if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        slot as usize
    } else {
        0
    };
    base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_TABLE_RVA + idx * core::mem::size_of::<usize>()
}

/// Walk the CSMenuProfModelRend chain for `slot` to its live portrait `CSGxTexture`, or 0 if the
/// renderer/offscreen/tex-rescap chain is not present (e.g. already torn down). Read-only.
unsafe fn sample_portrait_gxtexture(base: usize, slot: i32) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let renderer =
        unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0);
    if renderer == 0 || renderer == null {
        return 0;
    }
    let vt = unsafe { safe_read_usize(renderer) }.unwrap_or(0);
    if vt != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA {
        return 0;
    }
    let offscreen = unsafe {
        safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
    }
    .unwrap_or(0);
    if offscreen == 0 || offscreen == null {
        return 0;
    }
    let tex_rescap = unsafe {
        safe_read_usize(offscreen + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET)
    }
    .unwrap_or(0);
    if tex_rescap == 0 || tex_rescap == null {
        return 0;
    }
    unsafe { safe_read_usize(tex_rescap + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
        .unwrap_or(0)
}

/// Per-frame: keep the spared profile renderer drawing and capture the portrait once its model
/// finishes loading. After Continue the menu-owned offscreen-draw MenuJob stops, so we drive the
/// spared renderer's offscreen render ourselves each frame (`FUN_140bb8d90`); the global ResMan task
/// keeps loading/animating the model (`renderer+0x778`) automatically. Once the model has latched and
/// the GPU texture is uploaded, AddRef the `CSGxTexture` (+ its GPU child) so it survives, and cache
/// it for the now-loading forge (the next MENU_Load rotation displays the real portrait). One-shot.
/// Diagnostic: dump the captured portrait RGBA8 to `<debug-log-dir>/portrait-capture.bin`
/// (header: b"ERPX", u32 LE width, u32 LE height, then width*height*4 RGBA8) so the agent can
/// convert it to a PNG offline and visually confirm it is the loaded character's head (not the
/// depth buffer / garbage). Best-effort; gated by the same default-OFF readback path.
pub fn dump_portrait_rgba(slot: i32, width: u32, height: u32, px: &[u8]) {
    let dir = std::env::var("ER_EFFECTS_AUTOLOAD_DEBUG_PATH")
        .ok()
        .and_then(|p| PathBuf::from(p).parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let name = if slot >= 0 {
        format!("portrait-capture-slot{slot}.bin")
    } else {
        "portrait-capture.bin".to_string()
    };
    let path = dir.join(&name);
    if let Ok(mut f) = fs::File::create(&path) {
        // Encode through the erpx-rs crate (single source of truth for the ERPX container header),
        // so the on-disk format can never drift from the host-side decoder/`erpx2png` tool.
        let _ = erpx_rs::write_to(&mut f, width, height, px);
        append_autoload_debug(format_args!(
            "portrait-dump: slot={slot} wrote {width}x{height} ({} bytes) -> {name}",
            px.len()
        ));
    }
}

pub fn maybe_capture_portrait_gxtexture(base: usize, slot: i32) {
    if LOADING_BG_PORTRAIT_GX_KEPT.load(Ordering::SeqCst) != 0 {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    let valid = |p: usize| p != 0 && p != null;
    // Prefer the spared renderer (alive past Continue); before Continue use the live table slot.
    let spared = LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst);
    let renderer = if valid(spared) {
        spared
    } else {
        unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0)
    };
    if !valid(renderer) {
        return;
    }
    let vt = unsafe { safe_read_usize(renderer) }.unwrap_or(0);
    if vt != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA {
        return;
    }
    // NOTE: driving the menu offscreen render (FUN_140bb8d90) post-Continue crashes during world-load
    // (g_GxDrawContext invalid out of menu phase), and the character model never loads once the menu
    // phase ends -- so the in-loading-screen drive is disabled. The real no-delay path is to make the
    // ProfileSelect portrait render during the title phase (valid menu context) and capture it before
    // Continue. The spare + capture below stay safe (read-only) and fire only if the model ever loads.
    let _ = PROFILE_OFFSCREEN_DRIVE_RVA;
    let marked =
        unsafe { safe_read_u8(renderer + PROFILE_RENDERER_MARKED_DELETE_OFFSET) }.unwrap_or(1);
    let model =
        unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    let offscreen = unsafe {
        safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
    }
    .unwrap_or(0);
    let tex_rescap = if valid(offscreen) {
        unsafe {
            safe_read_usize(offscreen + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET)
        }
        .unwrap_or(0)
    } else {
        0
    };
    let gx = if valid(tex_rescap) {
        unsafe { safe_read_usize(tex_rescap + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
            .unwrap_or(0)
    } else {
        0
    };
    let gpu = if valid(gx) {
        unsafe { safe_read_usize(gx + GX_TEXTURE_GPU_RESOURCE_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    // +0x754/+0x755 are the refresh's "load-requested" idempotency flags: 1 = the async character
    // model build was kicked for this slot, 0 = never requested (the Continue path may not set up the
    // profile model data, so the portrait would never render no matter how long we wait).
    let req754 = unsafe { safe_read_u8(renderer + 0x754) }.unwrap_or(0xff);
    let req755 = unsafe { safe_read_u8(renderer + 0x755) }.unwrap_or(0xff);
    let seen = LOADING_BG_PORTRAIT_GX_CAPTURE_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if seen <= 60 && seen % 4 == 1 {
        append_autoload_debug(format_args!(
            "loading-portrait-capture: spared=0x{spared:x} renderer=0x{renderer:x} marked={marked} req754={req754} req755={req755} model=0x{model:x} gx=0x{gx:x} gpu=0x{gpu:x} seen={seen}"
        ));
    }
    // Require the character model to have async-loaded (`+0x778`) so we capture a rendered portrait,
    // not a blank offscreen.
    if !(marked == 0 && valid(model) && valid(gx) && valid(gpu)) {
        return;
    }
    // Ready: keepalive the CSGxTexture and its GPU child so the teardown release cannot free them.
    let gx_rc =
        unsafe { &*((gx + GX_TEXTURE_REFCOUNT_OFFSET) as *const core::sync::atomic::AtomicI32) };
    gx_rc.fetch_add(0x10000, Ordering::SeqCst);
    let gpu_rc =
        unsafe { &*((gpu + GX_TEXTURE_REFCOUNT_OFFSET) as *const core::sync::atomic::AtomicI32) };
    gpu_rc.fetch_add(0x10000, Ordering::SeqCst);
    LOADING_BG_PORTRAIT_GX_KEPT.store(gx, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "loading-portrait: CAPTURED portrait CSGxTexture gx=0x{gx:x} gpu=0x{gpu:x} renderer=0x{renderer:x} -- kept alive for now-loading forge"
    ));
    // REAL PIXELS (gated): D3D12-read the rendered offscreen render target into CPU RGBA8 once, so
    // the now-loading forge can build its TPF from the actual character head instead of the checker
    // placeholder. Default OFF -> behavior is byte-identical to the proven checker path.
    if portrait_real_pixels_enabled() {
        // Scan from the OFFSCREEN render object (renderer+0xa8), not the gx sub-nest -- the real RT
        // hangs off the offscreen; the gx sub-nest holds only 1x1 vkd3d dummy textures.
        if let Some((w, h, px)) = unsafe { readback_offscreen_rgba8(offscreen) } {
            // `readback_offscreen_rgba8` already recorded LOADING_BG_PORTRAIT_FORMAT (the DXGI value).
            let nonblack = portrait_center_nonblack(w, h, &px);
            let is_checker = portrait_looks_like_checker(w, h, &px);
            LOADING_BG_PORTRAIT_NONBLACK.store(nonblack as usize, Ordering::SeqCst);
            LOADING_BG_PORTRAIT_IS_CHECKER.store(is_checker as usize, Ordering::SeqCst);
            LOADING_BG_PORTRAIT_DIMS.store(((w as usize) << 16) | (h as usize), Ordering::SeqCst);
            let bytes = px.len();
            dump_portrait_rgba(slot, w, h, &px);
            // Readiness gate: hold back neutral/too-small transient captures (Bug A/B).
            if note_ls_portrait_capture(w, h, &px) {
                if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
                    *g = Some((w, h, px));
                }
            }
            append_autoload_debug(format_args!(
                "portrait-readback: dims={w}x{h} format={} nonblack={} is_checker={} (real-face proof = nonblack && !is_checker) bytes={bytes}",
                LOADING_BG_PORTRAIT_FORMAT.load(Ordering::SeqCst),
                nonblack as usize,
                is_checker as usize
            ));
        } else {
            append_autoload_debug(format_args!(
                "portrait-readback: readback_offscreen_rgba8 returned None (offscreen=0x{offscreen:x} gpu=0x{gpu:x})"
            ));
        }
    }
}

/// FORCE LIVE PROFILE PORTRAIT RENDER (diagnostic, `force_profile_render_enabled`). Runs each
/// menu-phase frame (no local player). One-shot: mark the target slot used
/// (`MarkProfileIndexAsUsed` -- the ONLY gate the refresh checks per STEP-0 RE: it sets
/// `ProfileSummary->saveSlotsStates[slot]=true` with no other side effect), then call the argless
/// profile-render refresh (`0x9aa680`), which equips ChrAsm + copies FaceData + kicks the async
/// character-model build that eventually sets `renderer+0x778`. The menu's OWN per-frame callbacks
/// then composite the live 3D head into the renderer's offscreen (no compositor call from us).
/// `maybe_capture_portrait_gxtexture` keeps the rendered gx once `+0x778` latches. Menu-phase only
/// (the user holds ProfileSelect; we never commit Continue) so there is no teardown/world-load crash
/// path -- this validates P1 (the model build) in isolation. Targets slot 0 (the staged single-profile
/// gold save's character). `slot` is the target save slot (0 for the staged single-profile gold
/// save; the autoload path passes its own target slot).
// `read_cursor_normalized` lived here: an OS `GetCursorPos` reading normalized against the ER window
// rect. Its last consumer was the System->Quit pointer band, a guessed screen rectangle that mapped
// the lower half of the WINDOW onto the two added rows -- which is how a click on Return to Desktop
// came to open the save picker. The row is now identified by the native grid's own cursor, which the
// game itself writes from its hit test against each cell's display object, so there is no reason to
// reimplement hit-testing from window geometry.

/// Hamilton product `a * b` of two `(x, y, z, w)` quaternions (w = scalar, matching `BoneData.q`).
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// A small look rotation: `yaw` about the local Y axis then `pitch` about the local X axis, as a
/// `(x, y, z, w)` quaternion. (Which local axis actually reads as horizontal/vertical for the head
/// bone needs one runtime visual calibration; the `LOOKAT_*_SIGN` consts flip it without a code change.)
pub fn quat_from_yaw_pitch(yaw: f32, pitch: f32) -> [f32; 4] {
    let (sy, cy) = (yaw * 0.5).sin_cos();
    let (sp, cp) = (pitch * 0.5).sin_cos();
    let q_yaw = [0.0, sy, 0.0, cy];
    let q_pitch = [sp, 0.0, 0.0, cp];
    quat_mul(q_yaw, q_pitch)
}

/// Read a bounded null-terminated ASCII bone name from an `hkStringPtr` (low bit is an ownership flag,
/// masked by the caller). `None` on unmapped memory or non-UTF8 (bone names are ASCII; no lossy decode).
pub unsafe fn read_bone_name(ptr: usize) -> Option<String> {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let mut bytes = Vec::with_capacity(32);
    for i in 0..64usize {
        let b = unsafe { safe_read_u8(ptr + i) }?;
        if b == 0 {
            break;
        }
        bytes.push(b);
    }
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Reach the live Havok `PoseHolder` from the renderer: `poseHolder = *(*(R+0x948)+0x20) + 0x48`,
/// guarded on the built model (`R+0x778`). `None` until the model + animation location are live.
pub unsafe fn profile_pose_holder(renderer: usize) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let model = unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }?;
    if !valid(model) {
        return None;
    }
    let x = unsafe { safe_read_usize(renderer + PROFILE_LOOKAT_ANIM_LOCATION_OFFSET) }?;
    if !valid(x) {
        return None;
    }
    let importer = unsafe { safe_read_usize(x + PROFILE_LOOKAT_IMPORTER_OFFSET) }?;
    if !valid(importer) {
        return None;
    }
    Some(importer + PROFILE_LOOKAT_POSEHOLDER_OFFSET)
}

/// Enumerate the skeleton's bones, dump names+indices ONCE per slot (diagnostic), and resolve the
/// Head/Neck/Spine2 indices by name. Returns `(head, neck, spine2)` indices (`-1` = not found).
unsafe fn dump_and_resolve_lookat_bones(bones: usize, count: usize, slot: i32) -> (i32, i32, i32) {
    let (mut head, mut neck, mut spine2) = (-1i32, -1i32, -1i32);
    let dump = (PROFILE_LOOKAT_BONES_DUMPED_MASK.load(Ordering::SeqCst) & (1usize << slot)) == 0;
    let mut dumped = String::new();
    for i in 0..count.min(LOOKAT_MAX_BONES) {
        let name_ptr =
            unsafe { safe_read_usize(bones + i * HKA_BONE_STRIDE + HKA_BONE_NAME_OFFSET) }
                .unwrap_or(0)
                & !1usize;
        let Some(name) = (unsafe { read_bone_name(name_ptr) }) else {
            continue;
        };
        if name.eq_ignore_ascii_case(LOOKAT_BONE_HEAD) {
            head = i as i32;
        } else if name.eq_ignore_ascii_case(LOOKAT_BONE_NECK) {
            neck = i as i32;
        } else if name.eq_ignore_ascii_case(LOOKAT_BONE_SPINE2) {
            spine2 = i as i32;
        }
        if dump {
            let _ = write!(dumped, "{i}:{name} ");
        }
    }
    if dump {
        PROFILE_LOOKAT_BONES_DUMPED_MASK.fetch_or(1usize << slot, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "lookat-bones: slot={slot} count={count} head={head} neck={neck} spine2={spine2} :: {dumped}"
        ));
    }
    (head, neck, spine2)
}

/// Retired look-at internals: resolve and cache the loaded character's Head/Neck/Spine2 pose-holder
/// metadata for diagnostics. Product portrait rendering now publishes a neutral pose and does not use cursor input.
unsafe fn apply_profile_lookat(renderer: usize, slot: i32) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let idx = slot as usize;
    if idx >= TITLE_PROFILE_SLOT_COUNT {
        return false;
    }
    let holder = match unsafe { profile_pose_holder(renderer) } {
        Some(h) => h,
        None => {
            // The engine refreshes a menu model's anim location-holder only intermittently (~6 Hz), so
            // `profile_pose_holder` returns None on ~89% of frames even though the model + its PoseHolder
            // persist. The caller only invokes us for a still-valid (vtable-checked) renderer, so a
            // transient None here is just the throttle -- KEEP the last resolved holder registered so the
            // draw-phase task can keep the renderer metadata available, decoupled from the engine's
            // throttled pose update. A genuinely stale holder (model rebuilt/torn down)
            // is dropped explicitly: the force-rebuild path clears PROFILE_LOOKAT_HOLDERS, and the
            // teardown spare hook owns post-Continue lifetime. Do NOT unregister on transient None.
            return false;
        }
    };
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(skel) {
        return false;
    }
    let bones = unsafe { safe_read_usize(skel + HKA_SKELETON_BONES_DATA_OFFSET) }.unwrap_or(0);
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if !valid(bones) || count <= 0 || count as usize > LOOKAT_MAX_BONES {
        return false;
    }
    let count = count as usize;
    PROFILE_LOOKAT_BONE_COUNT.store(count, Ordering::SeqCst);
    // Resolve the Head/Neck/Spine2 indices once per slot (+ dump bone names once). The hook reads the
    // shared PROFILE_LOOKAT_*_IDX globals; per-slot caching here just avoids re-dumping the names.
    {
        let mut guard = match PROFILE_LOOKAT_SLOTS.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard[idx].is_none() {
            let (head, neck, spine2) = unsafe { dump_and_resolve_lookat_bones(bones, count, slot) };
            if head < 0 {
                return false; // head bone not found yet; retry next tick
            }
            guard[idx] = Some(LookatSlot {
                head,
                neck,
                spine2,
                head_base: [0.0; 4],
                neck_base: [0.0; 4],
                spine2_base: [0.0; 4],
                base_latched: false,
            });
            PROFILE_LOOKAT_HEAD_IDX.store(head as usize, Ordering::SeqCst);
            PROFILE_LOOKAT_NECK_IDX.store(
                if neck >= 0 { neck as usize } else { usize::MAX },
                Ordering::SeqCst,
            );
            PROFILE_LOOKAT_SPINE2_IDX.store(
                if spine2 >= 0 {
                    spine2 as usize
                } else {
                    usize::MAX
                },
                Ordering::SeqCst,
            );
        }
    }
    // FrameBegin role: resolve + cache the Head/Neck/Spine2 indices (above) and register the holder.
    // Product portrait rendering now publishes a neutral pose; the old cursor/selftest drive is retired.
    PROFILE_LOOKAT_HOLDERS[idx].store(holder, Ordering::SeqCst);
    install_lookat_hook();
    install_per_frame_push_hook();
    PROFILE_LOOKAT_APPLY_CALLS.fetch_add(1, Ordering::SeqCst);
    true
}
