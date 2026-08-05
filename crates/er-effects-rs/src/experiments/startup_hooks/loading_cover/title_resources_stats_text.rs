use super::*;

pub(crate) fn install_title_menu_resource_acquire_observer_hook() {
    if TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED.load(Ordering::SeqCst) != 0
        && TITLE_SCALEFORM_FILE_OPEN_INSTALLED.load(Ordering::SeqCst) != 0
        && TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED.load(Ordering::SeqCst) != 0
    {
        return;
    }
    load_title_scaleform_memory_gfx();
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-resource-observer: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_MENU_RESOURCE_ACQUIRE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-resource-observer: failed to resolve AcquireMenuResource rva 0x{TITLE_MENU_RESOURCE_ACQUIRE_RVA:x}"
        ));
        return;
    };
    let Ok(file_open_addr) = game_rva(TITLE_SCALEFORM_FILE_OPEN_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-resource-observer: failed to resolve Scaleform file-open rva 0x{TITLE_SCALEFORM_FILE_OPEN_RVA:x}"
        ));
        return;
    };
    let Ok(resource_ctor_addr) = game_rva(TITLE_SCALEFORM_RESOURCE_CTOR_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-resource-observer: failed to resolve Scaleform resource-ctor rva 0x{TITLE_SCALEFORM_RESOURCE_CTOR_RVA:x}"
        ));
        return;
    };
    let mut ok = true;
    if TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED.load(Ordering::SeqCst) == 0 {
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                title_menu_resource_acquire_observer_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                TITLE_MENU_RESOURCE_ACQUIRE_ORIG
                    .store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
                std::mem::forget(hook);
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "title-resource-observer: AcquireMenuResource MhHook::new failed: {status:?}"
                ));
                ok = false;
            }
        }
    }
    if TITLE_SCALEFORM_FILE_OPEN_INSTALLED.load(Ordering::SeqCst) == 0 {
        match unsafe {
            MhHook::new(
                file_open_addr as *mut c_void,
                title_scaleform_file_open_observer_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                TITLE_SCALEFORM_FILE_OPEN_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
                std::mem::forget(hook);
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "title-resource-observer: Scaleform file-open MhHook::new failed: {status:?}"
                ));
                ok = false;
            }
        }
    }
    if TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED.load(Ordering::SeqCst) == 0 {
        match unsafe {
            MhHook::new(
                resource_ctor_addr as *mut c_void,
                title_scaleform_resource_ctor_observer_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                TITLE_SCALEFORM_RESOURCE_CTOR_ORIG
                    .store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
                std::mem::forget(hook);
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "title-resource-observer: Scaleform resource-ctor MhHook::new failed: {status:?}"
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
            TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED.store(1, Ordering::SeqCst);
            TITLE_SCALEFORM_FILE_OPEN_INSTALLED.store(1, Ordering::SeqCst);
            TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "title-resource-observer: hooked AcquireMenuResource 0x{addr:x}, Scaleform file-open 0x{file_open_addr:x}, resource-ctor 0x{resource_ctor_addr:x}; observe-only"
            ));
        }
        status => append_autoload_debug(format_args!(
            "title-resource-observer: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

pub(crate) unsafe extern "system" fn title_scaleform_bind_observer_hook(owner: usize, pair: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let symbol_ptr = unsafe { read_native_dlstring_ascii_ptr(pair) };
    let target_ptr = unsafe { read_native_dlstring_ascii_ptr(pair + 0x30) };
    let hit = TITLE_SCALEFORM_BIND_OBSERVER_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    TITLE_SCALEFORM_BIND_OBSERVER_LAST_OWNER.store(owner, Ordering::SeqCst);
    TITLE_SCALEFORM_BIND_OBSERVER_LAST_PAIR.store(pair, Ordering::SeqCst);
    TITLE_SCALEFORM_BIND_OBSERVER_LAST_SYMBOL_PTR.store(symbol_ptr, Ordering::SeqCst);
    TITLE_SCALEFORM_BIND_OBSERVER_LAST_TARGET_PTR.store(target_ptr, Ordering::SeqCst);
    let interesting = unsafe { bounded_ascii_contains(symbol_ptr, b"menu_") }
        || unsafe { bounded_ascii_contains(target_ptr, b"systex") }
        || unsafe { bounded_ascii_contains(symbol_ptr, b"title") }
        || unsafe { bounded_ascii_contains(symbol_ptr, b"profile") };
    if unsafe { bounded_ascii_contains(target_ptr, b"systex") } {
        TITLE_SCALEFORM_BIND_OBSERVER_SYSTEX_HITS.fetch_add(1, Ordering::SeqCst);
    }
    // STATS-PANEL NEUTRAL-BG REDIRECT (2026-07-04). In stats-panel product mode, redirect each visible
    // per-slot face bind `menu_dummyprofileface_NN -> systex_menu_profileMM` TARGET to our registered
    // neutral-bg key `STATS_PANEL_SYSTEX_KEYS[MM]`. The dummy-face shapes ARE the visible per-row boxes
    // (05_010 RE 2026-07-04), so the Scaleform-repo miss on our unique key bridges to our GPU texture
    // and paints the neutral background in the box -- with the character render blanked, there is no
    // portrait to draw. Fires on EVERY matching bind (the list re-binds as it scrolls/recycles); the
    // in-place DLString rewrite is idempotent. Gated per slot on the registered bit so we never point at
    // an unregistered key. This SUPERSEDES the yoinked slot-0 FL_40135 rewrite (which was based on the
    // now-corrected belief that the dummy faces were not visible).
    let mut rewritten_visible_profile_surface = false;
    let _ = (
        TITLE_PROFILE_VISIBLE_SURFACE_SYMBOL,
        ER_TPF_COVER_SYSTEX_KEY,
    );
    if stats_panel_enabled() && unsafe { bounded_ascii_contains(symbol_ptr, b"dummyprofileface") } {
        if let Some(slot) = unsafe { systex_profile_target_slot(target_ptr) } {
            if let Some(key) = er_loading_portrait::stats_panel_registered_systex_key(
                slot,
                STATS_PANEL_TEX_REGISTERED_MASK.load(Ordering::SeqCst),
            ) {
                if unsafe { rewrite_native_dlstring_ascii(pair + 0x30, key) }.is_some() {
                    rewritten_visible_profile_surface = true;
                    let prev = STATS_PANEL_BIND_REDIRECT_MASK.fetch_or(1 << slot, Ordering::SeqCst);
                    let n = STATS_PANEL_BIND_REDIRECTS.fetch_add(1, Ordering::SeqCst) + 1;
                    // Log the FIRST redirect of each slot (prev bit was clear) so we get exactly 10
                    // lines, not one per bind.
                    if prev & (1 << slot) == 0 {
                        append_autoload_debug(format_args!(
                            "stats-panel: redirected slot {slot} face bind target -> '{key}' (redirects={n} mask=0x{:x})",
                            STATS_PANEL_BIND_REDIRECT_MASK.load(Ordering::SeqCst)
                        ));
                    }
                }
            }
        }
    }
    if interesting && hit <= 128 {
        let mut sym = [0u8; 96];
        let mut tgt = [0u8; 96];
        let sn = unsafe { copy_ascii_preview(symbol_ptr, &mut sym) };
        let tn = unsafe { copy_ascii_preview(target_ptr, &mut tgt) };
        let sym = core::str::from_utf8(&sym[..sn]).unwrap_or("?");
        let tgt = core::str::from_utf8(&tgt[..tn]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-cover-part-b: observed native Scaleform bind owner=0x{owner:x} pair=0x{pair:x} symbol='{sym}' target='{tgt}' rewritten_visible_profile_surface={rewritten_visible_profile_surface} hit={hit}"
        ));
    }
    let orig = TITLE_SCALEFORM_BIND_OBSERVER_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(orig) };
        unsafe { f(owner, pair) };
    }
}

pub(crate) unsafe extern "system" fn title_flow_context_record_regulation_fix_hook(tfc: usize) {
    let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let before = if tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR {
        unsafe { safe_read_i32(tfc + TFC_REGULATION_VERSION_148_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let orig = TITLE_FLOW_CONTEXT_RECORD_REGULATION_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
        unsafe { f(tfc) };
    }
    let after_orig = if tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR {
        unsafe { safe_read_i32(tfc + TFC_REGULATION_VERSION_148_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let reg_manager =
        unsafe { safe_read_usize(base + GLOBAL_CS_REGULATION_MANAGER_RVA) }.unwrap_or(0);
    let manager44 = if reg_manager > OWNER_CTX_MIN_PLAUSIBLE_PTR
        && reg_manager < OWNER_CTX_MAX_PLAUSIBLE_PTR
    {
        unsafe { safe_read_i32(reg_manager + REGULATION_MANAGER_VERSION_44_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    if tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR
        && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR
        && manager44 > 0
        && after_orig < manager44
    {
        unsafe {
            ((tfc + TFC_REGULATION_VERSION_148_OFFSET) as *mut i32).write_volatile(manager44)
        };
        TITLE_FLOW_CONTEXT_RECORD_REGULATION_FIXUPS
            .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let after_fix = if tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR {
        unsafe { safe_read_i32(tfc + TFC_REGULATION_VERSION_148_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    append_autoload_debug(format_args!(
        "title-flow-context-record-fix: tfc=0x{tfc:x} before={before} after_orig={after_orig} after_fix={after_fix} manager44={manager44}"
    ));
}

pub(crate) fn install_title_flow_context_record_regulation_fix_hook() {
    if TITLE_FLOW_CONTEXT_RECORD_REGULATION_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-flow-context-record-fix: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_FLOW_CONTEXT_RECORD_REGULATION_VERSION_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-flow-context-record-fix: failed to resolve record rva 0x{TITLE_FLOW_CONTEXT_RECORD_REGULATION_VERSION_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_flow_context_record_regulation_fix_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_FLOW_CONTEXT_RECORD_REGULATION_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-flow-context-record-fix: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_FLOW_CONTEXT_RECORD_REGULATION_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-flow-context-record-fix: hooked native record helper 0x{addr:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-flow-context-record-fix: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-flow-context-record-fix: MhHook::new failed: {status:?}"
        )),
    }
}

pub(crate) fn install_title_scaleform_bind_observer_hook() {
    if TITLE_SCALEFORM_BIND_OBSERVER_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: bind observer MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_SCALEFORM_BIND_OBSERVER_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-b: failed to resolve Scaleform bind observer rva 0x{TITLE_SCALEFORM_BIND_OBSERVER_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_scaleform_bind_observer_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_SCALEFORM_BIND_OBSERVER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-b: queue_enable bind observer failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_SCALEFORM_BIND_OBSERVER_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-b: hooked passive Scaleform bind observer 0x{addr:x}; no product bind calls added"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-b: bind observer MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-b: MhHook::new bind observer failed: {status:?}"
        )),
    }
}

pub(crate) unsafe extern "system" fn title_native_menu_visual_window_fadein_hook(
    window: usize,
    param_2: usize,
    param_3: usize,
    param_4: usize,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let native_fadein: unsafe extern "system" fn(usize, usize, usize, usize) =
            unsafe { std::mem::transmute(orig) };
        unsafe { native_fadein(window, param_2, param_3, param_4) };
    }

    let caller_rva = trace_first_game_caller_rva();
    // Do not gate on the caller RVA here: MinHook/trampoline unwinding can hide the direct
    // MenuWindowJob::Run return address. The preserved native window pointer is the stronger RAM
    // identity oracle, and the caller RVA remains telemetry only.
    let native_job = TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB.load(Ordering::SeqCst);
    let mut native_window = TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW.load(Ordering::SeqCst);
    if native_window == null && native_job != null {
        native_window = unsafe { safe_read_usize(native_job + 0x130) }.unwrap_or(null);
        TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW.store(native_window, Ordering::SeqCst);
    }
    if native_window == null || window != native_window {
        return;
    }

    let Some(menu_id) = (unsafe { safe_read_u16(window + 0x180) }) else {
        return;
    };
    if menu_id >= 0x47 {
        return;
    }
    let base = game_module_base().unwrap_or(null);
    let cs_menu_man = if base != null {
        unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }.unwrap_or(null)
    } else {
        null
    };
    if cs_menu_man == null {
        return;
    }
    let flags_addr = cs_menu_man + 0x90 + menu_id as usize;
    let Some(flags_before) = (unsafe { safe_read_u8(flags_addr) }) else {
        return;
    };
    let flags_after = flags_before & !TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK;
    if flags_after == flags_before {
        return;
    }
    unsafe { (flags_addr as *mut u8).write_volatile(flags_after) };
    TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESSED_WINDOWS
        .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_WINDOW.store(window, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_BEFORE
        .store(flags_before as usize, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_AFTER.store(flags_after as usize, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-a: render-suppressed preserved native {TITLE_NATIVE_MENU_VISUAL_NAME} window=0x{window:x} menu_id={menu_id} flags 0x{flags_before:02x}->0x{flags_after:02x} via CSMenuMan+0x90 caller_rva=0x{caller_rva:x}"
    ));
}

pub(crate) unsafe fn title_child_name_matches(name_ptr: usize) -> bool {
    if name_ptr == 0 || name_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    let Ok(name) = (unsafe { CStr::from_ptr(name_ptr as *const i8).to_str() }) else {
        return false;
    };
    matches!(
        name,
        "PressStart"
            | "StaticSystemText_101000"
            | "PRESS BUTTON"
            | "CopyrightText"
            | "ProgressInfo"
            | "Install_ProgressInfo"
            | "StaticSystemText_100100"
            | "Info"
    )
}

pub(crate) unsafe fn title_profile_list_container_matches(name_ptr: usize) -> bool {
    if name_ptr == 0 || name_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    let Ok(name) = (unsafe { CStr::from_ptr(name_ptr as *const i8).to_str() }) else {
        return false;
    };
    name == "ProfileList/ItemList/ItemList/ItemList"
}

pub(crate) fn record_title_text_gfx_value(value: usize) {
    if value == 0 || value == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    for slot in TITLE_TEXT_GFX_VALUES.iter() {
        if slot.load(Ordering::SeqCst) == value {
            return;
        }
    }
    for slot in TITLE_TEXT_GFX_VALUES.iter() {
        if slot
            .compare_exchange(
                TITLE_OWNER_SCAN_START_ADDRESS,
                value,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            TITLE_TEXT_GFX_VALUE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            return;
        }
    }
}

pub(crate) unsafe extern "system" fn title_scene_obj_proxy_named_child_bind_hook(
    parent: usize,
    out_proxy: usize,
    name_ptr: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG.load(Ordering::SeqCst);
    if orig == null || orig == HOOK_ORIGINAL_UNSET {
        return out_proxy;
    }
    let f: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let ret = unsafe { f(parent, out_proxy, name_ptr) };
    // NOTE: the per-slot stats push is NOT done here. This binder is called per FIELD (PlayerName,
    // Level, ...) and does not know which save slot the row belongs to, so it cannot pick per-slot
    // attributes. The push is driven from `profile_row_populate_hook` (hooks the row-populate template
    // `FUN_1408758d0`, which carries the slot index in its row model) -- see bd er-effects-rs-l90.
    if unsafe { title_profile_list_container_matches(name_ptr) } {
        TITLE_PROFILE_FACE_BIND_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        TITLE_PROFILE_FACE_LAST_PROXY.store(out_proxy, Ordering::SeqCst);
        TITLE_PROFILE_FACE_LAST_VALUE.store(out_proxy, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "title-cover-part-b: recorded ProfileSelect container receiver=out_proxy name='ProfileList/ItemList/ItemList/ItemList' proxy=0x{out_proxy:x} parent=0x{parent:x} ret=0x{ret:x}"
        ));
    }
    if unsafe { title_child_name_matches(name_ptr) } {
        let context = unsafe { safe_read_usize(out_proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }
            .unwrap_or(null);
        let value = out_proxy + 0x18;
        TITLE_PRESS_START_BIND_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        TITLE_PRESS_START_BIND_LAST_PARENT.store(parent, Ordering::SeqCst);
        TITLE_PRESS_START_BIND_LAST_OUT.store(out_proxy, Ordering::SeqCst);
        TITLE_PRESS_START_BIND_LAST_NAME.store(name_ptr, Ordering::SeqCst);
        TITLE_PRESS_START_BIND_LAST_CONTEXT.store(context, Ordering::SeqCst);
        TITLE_PRESS_START_GFX_VALUE.store(value, Ordering::SeqCst);
        record_title_text_gfx_value(value);
        let base = game_module_base().unwrap_or(null);
        if base != null {
            let set_visible: unsafe extern "system" fn(usize, u8) =
                unsafe { std::mem::transmute(base + TITLE_PRESS_START_SET_VISIBLE_RVA) };
            unsafe { set_visible(out_proxy, 0) };
            let calls = TITLE_PRESS_START_BIND_HIDE_CALLS
                .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
                + OWN_STEPPER_CALL_INC;
            if calls <= 8 {
                let name = unsafe { CStr::from_ptr(name_ptr as *const i8) }.to_string_lossy();
                append_autoload_debug(format_args!(
                    "title-cover-part-a: named-child bind hid {name} out_proxy=0x{out_proxy:x} parent=0x{parent:x} context=0x{context:x} value=0x{value:x} calls={calls}"
                ));
            }
        }
    }
    ret
}

pub(crate) fn install_title_scene_obj_proxy_named_child_bind_hook() {
    if TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: named-child bind MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve named-child bind rva 0x{TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_scene_obj_proxy_named_child_bind_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable named-child bind failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked named-child SceneObjProxy binder 0x{addr:x}; PressStart/StaticSystemText will be hidden at bind time"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: named-child bind MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new named-child bind failed: {status:?}"
        )),
    }
}

/// The eight attributes of the character in save `slot`, or `None` when the slot is empty or the save
/// is unreadable. This is the PER-SLOT source (bd er-effects-rs-l90): the attributes exist in no live
/// struct at ProfileSelect time, so they are read straight from the on-disk `.sl2` (see
/// [`ensure_profile_slot_stats_cached`]).
pub(crate) fn profile_slot_attributes(slot: i32) -> Option<[i32; STATS_ATTR_COUNT]> {
    if !(0..PROFILE_SLOT_COUNT).contains(&slot) {
        return None;
    }
    let guard = PROFILE_SLOT_STATS_CACHE.lock().ok()?;
    guard
        .as_ref()?
        .get(slot as usize)
        .copied()
        .flatten()
        .map(|s| s.attributes)
}

/// The STORED effective max vitals `[hp, fp, stamina]` of the character in save `slot`, or
/// `None` when the slot is empty or the save is unreadable. Same `.sl2` source/cache as
/// [`profile_slot_attributes`]; the values are the save's serialized `MaxHealth`/`MaxFP`/
/// `MaxSP` (== runtime `current_max_hp/fp/stamina`, effective incl. talisman/buff mods),
/// read -- never derived -- so the boot loading screen can render the SAME five-line stats
/// panel as subsequent live loads (bd er-effects-rs-qic7).
pub(crate) fn profile_slot_vitals(slot: i32) -> Option<[u32; 3]> {
    if !(0..PROFILE_SLOT_COUNT).contains(&slot) {
        return None;
    }
    let guard = PROFILE_SLOT_STATS_CACHE.lock().ok()?;
    let stats = guard.as_ref()?.get(slot as usize).copied().flatten()?;
    Some([
        stats.max_hp.max(0) as u32,
        stats.max_fp.max(0) as u32,
        stats.max_stamina.max(0) as u32,
    ])
}

/// Fallback attributes read live from `GameDataMan -> PlayerGameData` -- the CURRENTLY-LOADED
/// character. Used only when the per-slot `.sl2` read fails entirely, so the row still shows real
/// (if not per-slot) values rather than nothing. Returns `None` when no character is loaded.
pub(crate) fn build_loaded_char_attributes() -> Option<[i32; STATS_ATTR_COUNT]> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gdm = game_data_man_ptr_or_null();
    if gdm == 0 || gdm == null {
        return None;
    }
    let pgd = unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }?;
    if pgd == 0 || pgd == null {
        return None;
    }
    let mut attrs = [0i32; STATS_ATTR_COUNT];
    for (i, a) in attrs.iter_mut().enumerate() {
        *a = unsafe { safe_read_i32(pgd + PGD_STAT_BASE_3C_OFFSET + i * 4) }.unwrap_or(0);
    }
    Some(attrs)
}

/// Build the ProfileSelect stats line for `attributes[start..end]` as a NUL-terminated UTF-16
/// Scaleform-HTML string for native SetText. Pure formatting ownership lives in `er-loading-portrait`;
/// this compatibility name keeps the startup-hook callsite stable.
pub(crate) use er_loading_portrait::build_title_stats_compact_html_utf16 as build_stats_compact_html_utf16;

fn merge_scaleform_html_utf16_lines(first: &[u16], second: &[u16]) -> Vec<u16> {
    fn decode_line(line: &[u16]) -> Option<String> {
        let body = line.strip_suffix(&[0]).unwrap_or(line);
        if body.is_empty() {
            return None;
        }
        String::from_utf16(body).ok().filter(|s| !s.is_empty())
    }

    let Some(first) = decode_line(first) else {
        return second.to_vec();
    };
    let Some(second) = decode_line(second) else {
        return first.encode_utf16().chain(core::iter::once(0)).collect();
    };
    let first = first
        .strip_prefix("<p align=\"left\">")
        .and_then(|s| s.strip_suffix("</p>"))
        .unwrap_or(&first);
    let second = second
        .strip_prefix("<p align=\"left\">")
        .and_then(|s| s.strip_suffix("</p>"))
        .unwrap_or(&second);
    let merged = format!(
        "<p align=\"left\">{first} <font size=\"16\" color=\"#8f887a\">/</font> {second}</p>"
    );
    merged.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Number of character attributes (Vig..Arc).
/// Profile/save slot count on the ProfileSelect screen.
pub(crate) const PROFILE_SLOT_COUNT: i32 = 10;

/// Per-slot stats cache (the 8 attributes + stored max vitals), parsed once from the live
/// `.sl2`, indexed by save slot (0-9). A per-slot `None` means an empty slot; the outer
/// `Option` is the "have we tried to load it yet?" latch.
pub(crate) static PROFILE_SLOT_STATS_CACHE: std::sync::Mutex<
    Option<[Option<er_save_loader::stats::SlotStats>; 10]>,
> = std::sync::Mutex::new(None);

/// Populate the per-slot stats cache from the live save file if not already loaded. Reads the on-disk
/// `.sl2` (the exact file the game loads) via the native save-dir builder path (`own_load_read_sl2_bytes`),
/// then parses each `USER_DATA` slot's `PlayerGameData` attributes with `er_save_loader::stats`. Heavy
/// work (a ~26 MB read + parse) happens at most once per session; subsequent rows hit the cache.
/// Returns whether the cache is loaded (true even if some/all slots are empty).
pub(crate) unsafe fn ensure_profile_slot_stats_cached(base: usize) -> bool {
    let mut guard = match PROFILE_SLOT_STATS_CACHE.lock() {
        Ok(g) => g,
        Err(poison) => poison.into_inner(),
    };
    if guard.is_some() {
        return true;
    }
    let Some(sl2) = (unsafe { crate::experiments::own_load_read_sl2_bytes(base) }) else {
        PROFILE_SLOT_STATS_CACHE_STATE.store(2, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "stats-text: per-slot cache load FAILED (.sl2 unreadable) -- falling back to loaded character"
        ));
        return false;
    };
    let all = er_save_loader::stats::all_slot_stats(&sl2);
    let decoded = all.iter().flatten().count();
    *guard = Some(all);
    PROFILE_SLOT_STATS_DECODED.store(decoded, Ordering::SeqCst);
    PROFILE_SLOT_STATS_CACHE_STATE.store(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "stats-text: per-slot cache loaded from .sl2 ({decoded}/10 slots decoded, {} bytes)",
        sl2.len()
    ));
    true
}

/// Push `utf16` onto the row's `ErStats` field with the game's own machinery, exactly as the native
/// row-populate does per field: resolve the named child (`assignComponentWithName` -- via the installed
/// hook's trampoline when available so the resolve is not double-instrumented), SetText through the
/// null-guarded wrapper `FUN_14074a0f0` (checks the field dataType; returns 0 when the child did not
/// resolve to an editable text field, e.g. when the 05_010 GFX edit was not served), then release the
/// resolved value with `CSScaleformValue::~CSScaleformValue` on the proxy's EMBEDDED value (+0x28),
/// mirroring the native `~CSScaleformValue(&SStack_70.scaleformValue)`. Returns whether SetText
/// accepted.
///
/// er-effects-rs-7e7 hardening: the SetText wrapper's first act is `rcx = *(proxy+0x8); call
/// *0x8(*rcx)` -- an UNVALIDATED virtual dispatch on the linked component object. On the first
/// in-world ProfileSelect open the component linked for our injected `ErStats` field was a stale
/// menu-arena object with a garbage heap vtable, and that dispatch jumped into `.rdata` (hard
/// crash). Validate component -> vtable -> slot target are all game-image-plausible before letting
/// the wrapper dispatch; otherwise skip fail-closed with full diagnostics.
pub(crate) unsafe fn push_stats_text_on_row(
    base: usize,
    row_proxy: usize,
    name: &str,
    utf16: &[u16],
) -> bool {
    debug_assert!(name.ends_with('\0'), "field name must be NUL-terminated");
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let assign = match TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG.load(Ordering::SeqCst) {
        orig if orig != null && orig != HOOK_ORIGINAL_UNSET => orig,
        _ => base + TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA,
    };
    let assign: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(assign) };
    let settext: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + PROFILE_SETTEXT_RVA) };
    let dtor: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + CSSCALEFORMVALUE_DTOR_RVA) };
    // The binder fully constructs the out proxy without reading it (RE: assignComponentWithName
    // ctor-or-resolve paths both initialize before use); a zeroed buffer mirrors the native
    // uninitialized 0x70-byte stack slot with headroom. The name is a plain string (the binder
    // treats it as a printf format; `ErStats` carries no '%').
    let mut proxy_buf = [0u8; SCENE_OBJ_PROXY_STACK_BYTES];
    let out = unsafe {
        assign(
            row_proxy,
            proxy_buf.as_mut_ptr() as usize,
            name.as_ptr() as usize,
        )
    };
    if out == 0 || out == null {
        return false;
    }
    let component_slot = out + SCENE_OBJ_PROXY_COMPONENT_SLOT_OFFSET;
    let comp = unsafe { safe_read_usize(component_slot) }.unwrap_or(0);
    let comp_vt = if comp != 0 && comp != null {
        unsafe { safe_read_usize(comp) }.unwrap_or(0)
    } else {
        0
    };
    let slot_fn = if comp_vt != 0 {
        unsafe { safe_read_usize(comp_vt + COMPONENT_GET_VALUE_VTABLE_SLOT_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let component_live =
        comp_vt != 0 && vtable_in_game_image(comp_vt, base) && vtable_in_game_image(slot_fn, base);
    let accepted = if component_live {
        // `utf16` outlives the call (the wrapper copies it into a DLString synchronously).
        (unsafe { settext(component_slot, utf16.as_ptr() as usize) }) != 0
    } else {
        let skips = PROFILE_STATS_PUSH_STALE_SKIPS.fetch_add(1, Ordering::SeqCst) + 1;
        PROFILE_STATS_PUSH_STALE_LAST_COMP.store(comp, Ordering::SeqCst);
        PROFILE_STATS_PUSH_STALE_LAST_VT.store(comp_vt, Ordering::SeqCst);
        if skips <= 8 {
            append_autoload_debug(format_args!(
                "stats-text: ErStats push SKIPPED fail-closed (er-effects-rs-7e7 guard): resolved component NOT live -- comp=0x{comp:x} vt=0x{comp_vt:x} slot_fn=0x{slot_fn:x} row=0x{row_proxy:x} (skips={skips})"
            ));
        }
        false
    };
    // Destroy the proxy's EMBEDDED CSScaleformValue exactly like the native populate. The old code
    // ran the dtor on +0x8 (the component slot) -- corrupting the link node and mis-releasing
    // proxy+0x20 -- a second latent 7e7-class UAF even when SetText succeeded.
    unsafe { dtor(out + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET) };
    accepted
}

/// Show or hide ONE native row field with the game's own machinery: resolve the named child
/// (`assignComponentWithName`, through the installed hook's trampoline so the resolve is not
/// double-instrumented), call the SceneObjProxy visibility wrapper `FUN_140733340`, then release the
/// resolved value with `~CSScaleformValue` on the proxy's EMBEDDED value (+0x28) exactly as the
/// native populate does per field. Returns whether the wrapper was called.
///
/// Why visibility and not text for the level fields: their contents come from writers that cannot
/// emit nothing (an FMG static pass and a `"%d"` format -- see the field-name constants), while a
/// re-resolve after the populate is impossible (it destroys the row proxy's embedded value).
/// Visibility is also the only lever that RESTORES exactly -- `visible = true` needs no knowledge of
/// the text, so a row clip reused by a save-file row (or by a vanilla view) comes back unchanged.
///
/// Fail-closed in every direction: the wrapper itself does nothing unless the resolved value is a
/// display object, and we skip the call unless the out proxy carries the game's own
/// `CS::SceneObjProxy` vtable (the wrapper's first act is an unvalidated
/// `(*proxy->vfptr->GetScaleformValue2)(proxy)` dispatch -- the er-effects-rs-7e7 class of hazard).
pub(crate) unsafe fn set_row_field_visible(
    base: usize,
    row_proxy: usize,
    name: &str,
    visible: bool,
) -> bool {
    debug_assert!(name.ends_with('\0'), "field name must be NUL-terminated");
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let assign = match TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG.load(Ordering::SeqCst) {
        orig if orig != null && orig != HOOK_ORIGINAL_UNSET => orig,
        _ => base + TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA,
    };
    let assign: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(assign) };
    let set_visible: unsafe extern "system" fn(usize, u8) =
        unsafe { std::mem::transmute(base + TITLE_PRESS_START_SET_VISIBLE_RVA) };
    let dtor: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + CSSCALEFORMVALUE_DTOR_RVA) };
    let mut proxy_buf = [0u8; SCENE_OBJ_PROXY_STACK_BYTES];
    let out = unsafe {
        assign(
            row_proxy,
            proxy_buf.as_mut_ptr() as usize,
            name.as_ptr() as usize,
        )
    };
    if out == 0 || out == null {
        PROFILE_ROW_SLOT_INFO_VIS_SKIPS.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    let proxy_vt = unsafe { safe_read_usize(out) }.unwrap_or(0);
    // The resolved value the wrapper will act on, so its GFx type is observable as telemetry: the
    // named-child ctor writes the child straight into the proxy's embedded CSScaleformValue and
    // links no foreign component, so this is the value `GetScaleformValue2` returns.
    let datatype = unsafe {
        safe_read_i32(
            out + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET + CSSCALEFORMVALUE_DATATYPE_20_OFFSET,
        )
    }
    .map(|raw| (raw as u32 & 0x8f) as usize);
    if let Some(datatype) = datatype {
        PROFILE_ROW_SLOT_INFO_LAST_DATATYPE.store(datatype, Ordering::SeqCst);
        if datatype != GFX_VALUE_TYPE_DISPLAY_OBJECT {
            let n = PROFILE_ROW_SLOT_INFO_NON_DISPLAY.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= 4 {
                append_autoload_debug(format_args!(
                    "save-picker: row field {} resolved GFx type {datatype} (not display object {GFX_VALUE_TYPE_DISPLAY_OBJECT}) -- native visibility setter will ignore it (n={n})",
                    name.trim_end_matches('\0')
                ));
            }
        }
    }
    let vtable_ok = proxy_vt == base + SCENE_OBJ_PROXY_VTABLE_RVA;
    if vtable_ok {
        unsafe { set_visible(out, u8::from(visible)) };
    } else {
        let n = PROFILE_ROW_SLOT_INFO_VIS_SKIPS.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 4 {
            append_autoload_debug(format_args!(
                "save-picker: row field {} visibility SKIPPED fail-closed -- out proxy 0x{out:x} vtable 0x{proxy_vt:x} is not CS::SceneObjProxy 0x{:x} (n={n})",
                name.trim_end_matches('\0'),
                base + SCENE_OBJ_PROXY_VTABLE_RVA
            ));
        }
    }
    unsafe { dtor(out + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET) };
    vtable_ok
}

/// Which of a row's per-slot info fields should be on screen. Pure row-visibility decision ownership
/// lives in `er-loading-portrait`; this compatibility name keeps the startup-hook callsite stable.
pub(crate) use er_loading_portrait::RowSlotFieldVisibility;

/// Apply a row's field visibility through the game's own wrapper.
///
/// Both directions matter equally. Hiding is the point; SHOWING is what makes hiding safe, because
/// the seven native row clips are REUSED -- a clip that showed `[ up .. ]` renders a save file two
/// scrolls later, and the same movie can outlive the picker window -- so every row states the full
/// answer for all three fields rather than only touching the ones it wants gone.
pub(crate) unsafe fn apply_row_slot_info_visibility(
    base: usize,
    row_proxy: usize,
    want: RowSlotFieldVisibility,
) {
    let fields = [
        (PROFILE_ROW_LEVEL_CAPTION_FIELD_NAME, want.level),
        (PROFILE_ROW_LEVEL_VALUE_FIELD_NAME, want.level),
        (PROFILE_ROW_LOCATION_FIELD_NAME, want.location),
        (PROFILE_ROW_PLAYTIME_FIELD_NAME, want.play_time),
    ];
    let (mut hidden, mut shown) = (0usize, 0usize);
    for (name, visible) in fields {
        if unsafe { set_row_field_visible(base, row_proxy, name, visible) } {
            if visible {
                shown += 1;
            } else {
                hidden += 1;
            }
        }
    }
    if hidden > 0 {
        let rows = PROFILE_ROW_SLOT_INFO_HIDDEN_ROWS.fetch_add(1, Ordering::SeqCst) + 1;
        if rows <= 4 || rows.is_power_of_two() {
            append_autoload_debug(format_args!(
                "save-picker: hid {hidden} per-slot field(s) on row=0x{row_proxy:x} (level={} location={} play_time={} rows={rows})",
                want.level, want.location, want.play_time
            ));
        }
    }
    if shown > 0 {
        PROFILE_ROW_SLOT_INFO_SHOWN_ROWS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Point the row model's `PlayTime` `CS::MenuString` at `text` and return the pointer it displaced,
/// so the caller can put it back the moment the native populate returns.
///
/// This is the write path for the last-saved line, and it is the NATIVE one: the populate reads
/// `rawString` first and SetTexts whatever it finds, so the row's own draw writes our text. That
/// matters more than convenience. A row clip is recycled across different files, so text pushed
/// out-of-band could survive onto a row it does not describe; here there is nothing to survive --
/// the text is read once, during the populate of the row it belongs to, from a pointer that exists
/// only across that call. A row that stages nothing gets the native string, not a stale one.
///
/// `None` when the field is unreadable, in which case nothing is written and the row keeps the
/// game's own playtime.
unsafe fn stage_row_model_menu_string(
    row_model: usize,
    offset: usize,
    text: *const u16,
) -> Option<usize> {
    let field = row_model + offset;
    let displaced = unsafe { safe_read_usize(field) }?;
    unsafe { (field as *mut usize).write_volatile(text as usize) };
    Some(displaced)
}

unsafe fn restore_row_model_menu_string(row_model: usize, offset: usize, displaced: usize) {
    let field = row_model + offset;
    unsafe { (field as *mut usize).write_volatile(displaced) };
}

/// Point the row model's `Location` `CS::MenuString` at `text` and return the pointer it displaced,
/// so the caller can put it back the moment the native populate returns.
///
/// Browse save-file rows use this for the last-saved timestamp because `Location` is the top-right
/// field, on the same visual line as `PlayerName`; `PlayTime` remains hidden for those rows.
pub(crate) unsafe fn stage_row_model_location(row_model: usize, text: *const u16) -> Option<usize> {
    unsafe {
        stage_row_model_menu_string(
            row_model,
            PROFILE_ROW_MODEL_LOCATION_MENUSTRING_90_OFFSET,
            text,
        )
    }
}

/// Put back whatever [`stage_row_model_location`] displaced.
pub(crate) unsafe fn restore_row_model_location(row_model: usize, displaced: usize) {
    unsafe {
        restore_row_model_menu_string(
            row_model,
            PROFILE_ROW_MODEL_LOCATION_MENUSTRING_90_OFFSET,
            displaced,
        )
    };
}

/// Hook of the ProfileSelect row-populate template `FUN_1408758d0(rowModel, rowProxy, ...)`. Runs once
/// per visible list row with a PER-SLOT row model, so it can push the CORRECT slot's attributes (unlike
/// the per-field named-child binder, which has no slot). The push happens BEFORE the original runs: the
/// original resolves the native fields and then destroys the row proxy's embedded `CSScaleformValue` at
/// its end, so a post-call resolve of `ErStats` would operate on a released value. Our push resolves a
/// SEPARATE child proxy (`ErStats`) and releases only that child's value, leaving the native fields and
/// the row proxy untouched for the original. bd er-effects-rs-l90.
pub(crate) unsafe extern "system" fn profile_row_populate_hook(
    row_model: usize,
    row_proxy: usize,
    arg3: usize,
    arg4: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = PROFILE_ROW_POPULATE_ORIG.load(Ordering::SeqCst);
    if orig == null || orig == HOOK_ORIGINAL_UNSET {
        // Can't call through; mirror the native return (the row model pointer) rather than crash.
        return row_model;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    // The last-saved text and the row-model pointer it displaced, held across the native call: the
    // populate reads the pointer, so the buffer has to outlive it and the field has to go back
    // afterwards. For browse file rows it is staged into Location, not PlayTime, so filename and
    // timestamp share one visual line.
    let mut staged_location: Option<(usize, Vec<u16>)> = None;
    if row_model != 0
        && row_model != null
        && row_proxy != 0
        && row_proxy != null
        && PROFILE_STATS_PUSH_IN_PROGRESS.swap(1, Ordering::SeqCst) == 0
    {
        let base = game_module_base().unwrap_or(null);
        if base != null {
            let slot = unsafe { safe_read_i32(row_model + PROFILE_ROW_MODEL_SLOT_08_OFFSET) }
                .unwrap_or(-1);
            let picker_row = (0..crate::experiments::save_picker::PICKER_ROW_COUNT as i32)
                .contains(&slot)
                .then_some(slot as usize);
            // PER-SLOT INFO FIELDS ON A BROWSE ROW. A browse row is a file or a navigation entry,
            // never a profile slot, so the record behind it is staged and its numbers are zeros: the
            // native fields render "Level 0" and "0:00:00" about a character that does not exist. So
            // while the picker owns a row, the Level caption/value and bottom PlayTime are always
            // hidden -- there is no browse row they could be true of -- and the top-right Location
            // slot is repurposed: a save-FILE row shows when that file was last written on the same
            // visual line as the filename, while every row with no such time hides the field instead
            // of showing a fabricated date.
            //
            // `None` (the vanilla character-slot views, title-screen Load Game list included) always
            // means "exactly what the game drew", and until a row was actually hidden the re-assert
            // does not run at all, so the vanilla path stays untouched.
            let slot_info = picker_row.and_then(save_picker_row_slot_info);
            let (want_visibility, last_saved) = match slot_info {
                Some(info) => (
                    RowSlotFieldVisibility::browse_row(info.location.is_some()),
                    info.location,
                ),
                None => (RowSlotFieldVisibility::NATIVE, None),
            };
            if want_visibility != RowSlotFieldVisibility::NATIVE
                || PROFILE_ROW_SLOT_INFO_HIDDEN_ROWS.load(Ordering::SeqCst) != 0
            {
                unsafe { apply_row_slot_info_visibility(base, row_proxy, want_visibility) };
            }
            if let Some(text) = last_saved {
                // NUL-terminated UTF-16 for the native SetText, kept alive past the populate call.
                let utf16: Vec<u16> = text.encode_utf16().chain(core::iter::once(0)).collect();
                match unsafe { stage_row_model_location(row_model, utf16.as_ptr()) } {
                    Some(displaced) => {
                        let rows = PROFILE_ROW_LAST_SAVED_ROWS.fetch_add(1, Ordering::SeqCst) + 1;
                        if rows <= 4 || rows.is_power_of_two() {
                            append_autoload_debug(format_args!(
                                "save-picker: row slot={slot} shows last-saved '{text}' in top-right Location (rows={rows})"
                            ));
                        }
                        staged_location = Some((displaced, utf16));
                    }
                    None => {
                        let fails = PROFILE_ROW_LAST_SAVED_STAGE_FAILURES
                            .fetch_add(1, Ordering::SeqCst)
                            + 1;
                        if fails <= 4 {
                            append_autoload_debug(format_args!(
                                "save-picker: last-saved '{text}' NOT staged on slot={slot} -- row model 0x{row_model:x} location field unreadable (fails={fails})"
                            ));
                        }
                    }
                }
            }
            // BROWSE PICKER ROWS: one visual baseline. `PlayerName` carries the filename or row
            // title, `ErStats` carries file details/navigation copy, `Location` carries timestamps,
            // and drive rows draw their actual cells through separate `DriveCell_*` row children.
            // Blank every synthetic drive child on every picker-owned row first so recycled row clips
            // cannot leak a previous drive strip into file/directory rows.
            if let Some(row) = picker_row {
                for (cell, field) in er_gfx::title_05_010::DRIVE_CELL_FIELD_NAMES
                    .iter()
                    .enumerate()
                {
                    let text = save_picker_drive_cell_text(row, cell).unwrap_or_else(|| vec![0]);
                    let mut field_name = String::with_capacity(field.len() + 1);
                    field_name.push_str(field);
                    field_name.push('\0');
                    let pushed =
                        unsafe { push_stats_text_on_row(base, row_proxy, &field_name, &text) };
                    if !pushed && row == 0 && cell == 0 {
                        let fails = PROFILE_STATS_PUSH_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
                        if fails <= 4 {
                            append_autoload_debug(format_args!(
                                "save-picker: DriveCell_* push REJECTED on row=0x{row_proxy:x} (05_010 GFX edit not live?) (fails={fails})"
                            ));
                        }
                    }
                }
            }
            let browse_lines = picker_row.and_then(save_picker_browse_stats_lines);
            if let Some((top, bottom)) = browse_lines {
                let seen = PROFILE_STATS_ROW_POPULATES.fetch_add(1, Ordering::SeqCst) + 1;
                let merged = merge_scaleform_html_utf16_lines(&top, &bottom);
                let pushed =
                    unsafe { push_stats_text_on_row(base, row_proxy, "ErStats\0", &merged) };
                if pushed {
                    let subs = PROFILE_STATS_SETTEXT_SUBS.fetch_add(1, Ordering::SeqCst) + 1;
                    if subs <= 4 {
                        append_autoload_debug(format_args!(
                            "save-picker: pushed inline browse-row info slot={slot} on row=0x{row_proxy:x} (row_triggers={seen} subs={subs})"
                        ));
                    }
                } else {
                    let fails = PROFILE_STATS_PUSH_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
                    if fails <= 4 {
                        append_autoload_debug(format_args!(
                            "save-picker: inline browse-row info push REJECTED slot={slot} on row=0x{row_proxy:x} pushed={pushed} (05_010 GFX edit not live?) (fails={fails})"
                        ));
                    }
                }
            } else if stats_panel_enabled() {
                let cache_loaded = unsafe { ensure_profile_slot_stats_cached(base) };
                // Per-slot attributes from the save; if the whole cache failed to load, degrade to the
                // loaded character so a row still shows real values (rather than nothing).
                let attrs = profile_slot_attributes(slot).or_else(|| {
                    if cache_loaded {
                        None
                    } else {
                        build_loaded_char_attributes()
                    }
                });
                if let Some(attrs) = attrs {
                    let seen = PROFILE_STATS_ROW_POPULATES.fetch_add(1, Ordering::SeqCst) + 1;
                    // One compact stats line: all eight attributes use initial labels and a 16px font so
                    // the row can be shorter without keeping two injected stat fields alive.
                    let stats = build_stats_compact_html_utf16(&attrs);
                    let pushed =
                        unsafe { push_stats_text_on_row(base, row_proxy, "ErStats\0", &stats) };
                    debug_assert_eq!("ErStats", er_gfx::title_05_010::STATS_FIELD_NAME);
                    if pushed {
                        let subs = PROFILE_STATS_SETTEXT_SUBS.fetch_add(1, Ordering::SeqCst) + 1;
                        if subs <= 4 {
                            append_autoload_debug(format_args!(
                                "stats-text: pushed merged ErStats slot={slot} on row=0x{row_proxy:x} (row_triggers={seen} subs={subs})"
                            ));
                        }
                    } else {
                        let fails = PROFILE_STATS_PUSH_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
                        if fails <= 4 {
                            append_autoload_debug(format_args!(
                                "stats-text: merged ErStats push REJECTED slot={slot} on row=0x{row_proxy:x} pushed={pushed} (05_010 GFX edit not live?) (fails={fails})"
                            ));
                        }
                    }
                }
            }
        }
        PROFILE_STATS_PUSH_IN_PROGRESS.store(0, Ordering::SeqCst);
    }
    let ret = unsafe { f(row_model, row_proxy, arg3, arg4) };
    // The populate has read the string; give the row model its own pointer back so our borrow does
    // not outlive the call that needed it. `utf16` drops here, after the read, never before.
    if let Some((displaced, _utf16)) = staged_location {
        unsafe { restore_row_model_location(row_model, displaced) };
    }
    ret
}
