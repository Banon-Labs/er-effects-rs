pub(crate) fn seamless_coop_loaded() -> bool {
    matches!(
        unsafe { GetModuleHandleA(PCSTR(SEAMLESS_COOP_MODULE_NAME.as_ptr())) },
        Ok(module) if module.0 as usize != 0
    )
}

pub(crate) fn bootstrap_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_BOOTSTRAP_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-bootstrap.jsonl"))
}

pub(crate) fn bootstrap_state_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_BOOTSTRAP_STATE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-bootstrap-state.json"))
}

pub(crate) fn write_bootstrap_event(stage: &str, detail: &str) {
    use std::io::Write;

    let event_path = bootstrap_path();
    let state_path = bootstrap_state_path();
    if let Some(parent) = event_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Some(parent) = state_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let payload = format!(
        "{{\"stage\":\"{}\",\"detail\":\"{}\"}}\n",
        json_escape(stage),
        json_escape(detail)
    );
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&event_path)
    {
        let _ = file.write_all(payload.as_bytes());
    }
    let _ = fs::write(state_path, payload);
}

fn title_logo_gfx_alpha_for_frame(frame: i32) -> i32 {
    match frame {
        TITLE_LOGO_GFX_UNKNOWN_FRAME => TITLE_LOGO_GFX_UNKNOWN_ALPHA,
        // Disk correlation: `target/autoresearch/gfx-analysis/script-smoke/summary.json` for
        // `05_001_title_logo.gfx` shows root depth 3 is placed at frame 1 with no color transform,
        // then moved by FadeIn frames 2..60 using alphaMultTerm 0..256, remains full through
        // Title_TopMenu/FadeOut frame 113, and fades to 0 by frame 133. The in-memory oracle reads
        // the live Scaleform current frame through `FUN_140d82620`, so convert that frame back into
        // the on-disk alpha term instead of treating the entire ramp as a generic visible boolean.
        1 => TITLE_LOGO_GFX_FULL_ALPHA,
        2..=60 => ((frame - 2) * TITLE_LOGO_GFX_FULL_ALPHA + 29) / 58,
        61..=113 => TITLE_LOGO_GFX_FULL_ALPHA,
        114..=133 => ((133 - frame) * TITLE_LOGO_GFX_FULL_ALPHA + 10) / 20,
        _ => TITLE_LOGO_GFX_UNKNOWN_ALPHA,
    }
}

fn push_json_usize(body: &mut String, name: &str, value: usize) {
    body.push_str(&format!("  \"{name}\": {value},\n"));
}

fn push_json_bool(body: &mut String, name: &str, value: bool) {
    body.push_str(&format!("  \"{name}\": {value},\n"));
}

fn push_json_str(body: &mut String, name: &str, value: &str) {
    body.push_str(&format!("  \"{name}\": \"{value}\",\n"));
}

fn title_menu_window_id_flags(base: usize, window: usize) -> (usize, usize, bool) {
    const NULL_PTR: usize = 0;
    if window == NULL_PTR || window == TITLE_OWNER_SCAN_START_ADDRESS {
        return (
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
            false,
        );
    }
    let menu_id = unsafe { crate::experiments::safe_read_u16(window + 0x180) }
        .map_or(TITLE_OWNER_SCAN_START_ADDRESS, usize::from);
    if menu_id >= 0x47 {
        return (menu_id, TITLE_OWNER_SCAN_START_ADDRESS, false);
    }
    let cs_menu_man = unsafe { crate::experiments::safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }
        .unwrap_or(NULL_PTR);
    if cs_menu_man == NULL_PTR {
        return (menu_id, TITLE_OWNER_SCAN_START_ADDRESS, false);
    }
    let flags = unsafe { crate::experiments::safe_read_u8(cs_menu_man + 0x90 + menu_id) }
        .map_or(TITLE_OWNER_SCAN_START_ADDRESS, usize::from);
    let draw_bit_set = flags != TITLE_OWNER_SCAN_START_ADDRESS
        && (flags & TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK as usize) != 0;
    (menu_id, flags, draw_bit_set)
}

unsafe fn title_logo_gfx_current_frame(base: usize, title_logo_back_view_parts: usize) -> i32 {
    if title_logo_back_view_parts == TITLE_OWNER_SCAN_START_ADDRESS
        || title_logo_back_view_parts == 0
    {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    let gfx_value = title_logo_back_view_parts + TITLE_LOGO_GFX_VALUE_88_OFFSET;
    let Some(handle) = (unsafe { crate::experiments::safe_read_usize(gfx_value) }) else {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    };
    if handle == 0 || handle == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    let Some(vtable) = (unsafe { crate::experiments::safe_read_usize(handle) }) else {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    };
    if vtable == 0 || vtable == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    // The `safe_read_*` guards only reject UNMAPPED pages -- they happily return a mapped-but-garbage
    // qword. During a System-Quit -> return-title -> reload transition `PRODUCT_CORE_LAST_TITLE_DIALOG`
    // (the source of `title_logo_back_view_parts`) points at a half-torn-down / reallocated dialog whose
    // embedded BackViewParts holds a stale `handle` whose vtable lands in the Wine heap, NOT the game
    // image. Transmuting `*(vtable+8)` from such a vtable and CALLING it dispatches through a data
    // address -> access violation (observed: handle vt=0x7ffe96aa4238, call target 0x7ffe977c61b0, both
    // outside [game_base, +SizeOfImage); crash self+0x317bd `call *rdx`). Reject any vtable / resolved
    // call target that is not inside the game module image before the transmute+call. See bd
    // er-effects-rs-3pc (post-switch reload crash).
    if !crate::experiments::vtable_in_game_image(vtable, base) {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    let Some(resolve_value_addr) = (unsafe { crate::experiments::safe_read_usize(vtable + 0x8) })
    else {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    };
    if resolve_value_addr == 0 || resolve_value_addr == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    if !crate::experiments::vtable_in_game_image(resolve_value_addr, base) {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    // Mirrors native helpers at 0x140749980/0x1407499e0: load *(gfx_value) into rcx, call vtable+8,
    // then pass the resolved Scaleform value to FUN_140d82620 to read the current 1-based frame.
    let resolve_value: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(resolve_value_addr) };
    let value = unsafe { resolve_value(handle) };
    if value == 0 || value == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    let current_frame: unsafe extern "system" fn(usize) -> i32 =
        unsafe { std::mem::transmute(base + TITLE_LOGO_GFX_CURRENT_FRAME_RVA) };
    unsafe { current_frame(value) }
}

