/// Walk `FD4FileCap::loadProcess -> FD4FileLoadProcess::fileLoadProcessor` and sample the content
/// state the MSB load-complete callback gates on, returning `(processor, content, size, acquires)`.
///
/// This is the exact chain `FD4FileCap::AcquireContent` (`FUN_1426591c0`) walks: it returns null if
/// either `loadProcess` or `fileLoadProcessor` is null, and otherwise hands back `processor.content_`
/// -- re-fetching it through a vtable call only on the acquire refcount's `0 -> 1` edge, while the
/// matching release nulls `content_` on the `1 -> 0` edge. A null content is what makes
/// `MsbFileCap::msbResCap` stay `0` even at `loadState == 4`, so sampling it says whether the freeze
/// is "no buffer at all" or "buffer present but never parsed". Read-only: no acquire, no refcount
/// touch, no vtable call.
pub unsafe fn fd4_filecap_content_state(load_process: usize) -> (usize, usize, usize, i64) {
    if load_process <= 0x10000 {
        return (0, 0, 0, -1);
    }
    let Some(processor) =
        (unsafe { safe_read_usize(load_process + FD4_FILELOADPROCESS_PROCESSOR_20_OFFSET) })
            .filter(|&v| v > 0x10000)
    else {
        return (0, 0, 0, -1);
    };
    let content =
        unsafe { safe_read_usize(processor + FD4_FILELOADPROCESSOR_CONTENT_20_OFFSET) }.unwrap_or(0);
    let size =
        unsafe { safe_read_usize(processor + FD4_FILELOADPROCESSOR_SIZE_28_OFFSET) }.unwrap_or(0);
    let acquires = unsafe { safe_read_usize(processor + FD4_FILELOADPROCESSOR_ACQUIRE_30_OFFSET) }
        .map(|v| (v & 0xffff_ffff) as i64)
        .unwrap_or(-1);
    (processor, content, size, acquires)
}

/// Read an `FD4ResCapHolderItem`'s resource name (the msb filename) off a file cap, as ASCII.
///
/// `resourceString` is an `FD4BasicHashString` whose `DLString<wchar_t>` is small-string-optimized:
/// `capacity > 7` means the union at `+0x18` holds a heap POINTER, otherwise the characters sit
/// inline in the union itself. Both `length` and the read are clamped so a garbage capacity cannot
/// walk the probe off a page, every character goes through `safe_read_u8`, and non-ASCII collapses
/// to `?` -- this runs on the game thread during a stall, so it must not fault or allocate wildly.
pub unsafe fn fd4_filecap_name(cap: usize) -> String {
    let capacity =
        unsafe { safe_read_usize(cap + FD4_FILECAP_NAME_CAPACITY_30_OFFSET) }.unwrap_or(0);
    let length = unsafe { safe_read_usize(cap + FD4_FILECAP_NAME_LENGTH_28_OFFSET) }.unwrap_or(0);
    let union_addr = cap + FD4_FILECAP_NAME_UNION_18_OFFSET;
    let chars_addr = if capacity > DLSTRING_INLINE_CAPACITY_MAX {
        match unsafe { safe_read_usize(union_addr) }.filter(|&v| v > 0x10000) {
            Some(ptr) => ptr,
            None => return String::from("<badptr>"),
        }
    } else {
        union_addr
    };
    let count = length.min(FD4_FILECAP_NAME_MAX_CHARS);
    let mut out = String::with_capacity(count);
    for i in 0..count {
        let (Some(lo), Some(hi)) = (unsafe { safe_read_u8(chars_addr + i * 2) }, unsafe {
            safe_read_u8(chars_addr + i * 2 + 1)
        }) else {
            out.push_str("<trunc>");
            break;
        };
        let unit = u16::from(lo) | (u16::from(hi) << 8);
        if unit == 0 {
            break;
        }
        out.push(if (0x20..0x7f).contains(&unit) {
            unit as u8 as char
        } else {
            '?'
        });
    }
    out
}

/// Read a `DLString<wchar_t>` (given the address of the string itself) as clamped ASCII.
///
/// Same small-string-optimization rule as `fd4_filecap_name`: `capacity > 7` means the union holds
/// a heap pointer, otherwise the characters are inline. Kept separate because that helper takes a
/// cap and bakes in the `+0x10` string base, while virtual-root entries hold bare `DLString`s.
pub unsafe fn dlstring_wide_ascii(string_base: usize) -> String {
    if string_base <= 0x10000 {
        return String::new();
    }
    let capacity =
        unsafe { safe_read_usize(string_base + DLSTRING_CAPACITY_20_OFFSET) }.unwrap_or(0);
    let length = unsafe { safe_read_usize(string_base + DLSTRING_LENGTH_18_OFFSET) }.unwrap_or(0);
    let union_addr = string_base + DLSTRING_UNION_08_OFFSET;
    let chars_addr = if capacity > DLSTRING_INLINE_CAPACITY_MAX {
        match unsafe { safe_read_usize(union_addr) }.filter(|&v| v > 0x10000) {
            Some(ptr) => ptr,
            None => return String::from("<badptr>"),
        }
    } else {
        union_addr
    };
    let count = length.min(FD4_FILECAP_NAME_MAX_CHARS);
    let mut out = String::with_capacity(count);
    for i in 0..count {
        let (Some(lo), Some(hi)) = (unsafe { safe_read_u8(chars_addr + i * 2) }, unsafe {
            safe_read_u8(chars_addr + i * 2 + 1)
        }) else {
            out.push_str("<trunc>");
            break;
        };
        let unit = u16::from(lo) | (u16::from(hi) << 8);
        if unit == 0 {
            break;
        }
        out.push(if (0x20..0x7f).contains(&unit) {
            unit as u8 as char
        } else {
            '?'
        });
    }
    out
}

/// Report the DLIO virtual-root aliases that back the stalled `mapstudio_dlc2:/m28_*.msb` reads.
///
/// The phase-2 freeze's file caps resolve through `mapstudio_dlc2:`, which is an alias in
/// `DLFileDeviceManager::virtualRoots`, NOT a data archive. That alias is registered EMPTY (`L""`)
/// by the title start-game flow and only filled in by `CSDlcImp::AddVirtualFileRoots` behind the
/// `STEP_LoadListWait` gate. So an alias present with an EMPTY path at the stall means the read had
/// nowhere to resolve to -- which is exactly a 0-byte read and a null `msbResCap`. Emitting
/// `mapstudio` alongside it is the control: base-game populated + dlc2 empty is decisive on its own.
///
/// Strictly read-only -- a vector walk with bounded length and per-field `safe_read_*`, no locks and
/// no allocation beyond the returned string, because this runs on the game thread mid-stall.
pub unsafe fn dlio_virtual_roots_summary(base: usize) -> String {
    if base == 0 {
        return String::from("<nobase>");
    }
    let Some(manager) =
        (unsafe { safe_read_usize(base + DL_FILE_DEVICE_MANAGER_SINGLETON_RVA) }).filter(|&v| v > 0x10000)
    else {
        return String::from("<mgrnull>");
    };
    let roots = manager + DL_FILE_DEVICE_MANAGER_VIRTUAL_ROOTS_48_OFFSET;
    let (Some(start), Some(end)) = (
        unsafe { safe_read_usize(roots + FILE_DEVICE_VIRTUAL_ROOT_VECTOR_START_08_OFFSET) },
        unsafe { safe_read_usize(roots + FILE_DEVICE_VIRTUAL_ROOT_VECTOR_END_10_OFFSET) },
    ) else {
        return String::from("<vecunreadable>");
    };
    if start <= 0x10000 || end <= start {
        return format!("<vecempty start={start:#x} end={end:#x}>");
    }
    let count =
        ((end - start) / FILE_DEVICE_VIRTUAL_ROOT_ENTRY_STRIDE).min(FILE_DEVICE_VIRTUAL_ROOT_MAX_ENTRIES);
    let mut out = String::new();
    let mut seen = 0usize;
    for i in 0..count {
        let entry = start + i * FILE_DEVICE_VIRTUAL_ROOT_ENTRY_STRIDE;
        let name = unsafe { dlstring_wide_ascii(entry) };
        if !VIRTUAL_ROOTS_OF_INTEREST.iter().any(|w| *w == name) {
            continue;
        }
        seen += 1;
        let path =
            unsafe { dlstring_wide_ascii(entry + FILE_DEVICE_VIRTUAL_ROOT_ENTRY_PATH_30_OFFSET) };
        // An EMPTY path on a present alias is the whole point of this probe -- label it loudly so a
        // log scan cannot mistake it for a formatting artifact.
        let verdict = if path.is_empty() { "EMPTY" } else { "ok" };
        let _ = core::fmt::Write::write_fmt(
            &mut out,
            format_args!("{name}='{path}'({verdict}),"),
        );
    }
    format!("total={count}/matched={seen}/{out}")
}

/// Read the TitleTopDialog FD4 state machine by NAME (is_in_state) given the title `owner` (rcx of
/// STEP_MenuJobWait). Returns `(dialog_ptr, in_fadein, in_loop, in_textfadeout, menu_opened_latch)` or
/// `None` if the dialog isn't the TitleTopDialog yet. Read-only / no side effects. Mirrors STAGE1d.
unsafe fn title_dialog_sm_state(
    owner: usize,
    base: usize,
) -> Option<(usize, bool, bool, bool, usize)> {
    if owner == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    if dialog == 0 {
        return None;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(0);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_fadein =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_FADEIN_RVA) } != OWN_STEPPER_FALSE;
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    let in_textfadeout =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) } != OWN_STEPPER_FALSE;
    let latch = unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
        .unwrap_or(0);
    Some((dialog, in_fadein, in_loop, in_textfadeout, latch))
}

/// Skip the title FadeIn ONCE: the first frame the dialog SM is settled in FadeIn (menu-open latch
/// clear), drive the FD4 state machine FadeIn->Loop by calling the game's OWN transition `SetState`
/// (deobf 0x1407499e0) with `(sm = dialog+0xa60, desc = Loop 0x142a8f9e8)`. This is EXACTLY the call
/// `CS::TitleTopDialog::update`'s input-skip branch makes on a confirm/cancel press (Ghidra: bd
/// fadein-* RE), so it is save-safe and routes through the SM's own vtable[0x150] request path (no
/// struct stomp) -- but ZERO input. `SetState` internally no-ops unless the current node is settled
/// (`[node+0x20]&0x8f >= 2`), so an early call before the node is eligible cannot corrupt the SM.
/// One-shot via `TITLE_FADEIN_SKIP_FIRED`; the dt-scale / frame-burst / anim-complete-predicate levers
/// were all runtime-falsified (bd title-anim-framedelta / pab-to-menuopen-real-breakdown / fadein-
/// predicate-75cea0). The FadeIn IS frame-paced animation -- it is just skipped by the state transition,
/// not by pacing.
unsafe fn title_anim_fadein_skip(owner: usize) {
    if TITLE_FADEIN_SKIP_FIRED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS {
        return; // one-shot: already transitioned
    }
    if IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
        return;
    }
    // Deliberately "not greater", not `<=`: the factor arrives through a host function pointer,
    // and a NaN has to take the bail. `NaN <= MIN` is false, while `partial_cmp` returning `None`
    // is not `Greater`, so this keeps the original `!(x > MIN)` behaviour.
    if !matches!(
        title_anim_speedup_factor().partial_cmp(&TITLE_ANIM_SPEEDUP_MIN),
        Some(core::cmp::Ordering::Greater)
    ) {
        return; // lever off / forced to 1.0
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let st = unsafe { title_dialog_sm_state(owner, base) };
    // Light diagnostic so the SM timeline stays visible across boots.
    let n = TITLE_ANIM_DIAG_CALLS.fetch_add(1, Ordering::SeqCst);
    if n.is_multiple_of(TITLE_ANIM_DIAG_INTERVAL) {
        append_autoload_debug(format_args!(
            "title-anim-diag: detour#{n} sm(dialog,fadein,loop,tfo,latch)={st:?}"
        ));
    }
    let Some((dialog, true, _, _, latch)) = st else {
        return; // not the TitleTopDialog, or not in FadeIn yet
    };
    if latch != TITLE_OWNER_SCAN_START_ADDRESS {
        return; // menu already opening -> leave the SM alone
    }
    // Fire the game's own FadeIn->Loop transition once (zero-input).
    if TITLE_FADEIN_SKIP_FIRED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return; // lost the one-shot race
    }
    let set_state: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(base + TITLE_FD4_SETSTATE_RVA) };
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    unsafe { set_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) };
    append_autoload_debug(format_args!(
        "title-anim-skip: *** SetState(sm=0x{sm:x}, Loop) via 0x{:x} -- zero-input FadeIn->Loop transition (game's own input-skip path, save-safe), skipping the title fade ***",
        base + TITLE_FD4_SETSTATE_RVA
    ));
}

/// Detour for STEP_MenuJobWait (0x140b0d400, `__fastcall(rcx=owner, rdx=task_data, ...)`). Drives the
/// one-shot FadeIn->Loop skip from the live SM state, then passes through to the original unchanged.
pub unsafe extern "system" fn title_menujob_speed_detour(
    owner: usize,
    task_data: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        title_anim_fadein_skip(owner)
    }));
    let orig_addr = TITLE_ANIM_SPEED_ORIG.load(Ordering::SeqCst);
    if orig_addr == TITLE_OWNER_SCAN_START_ADDRESS {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig_addr) };
    unsafe { orig(owner, task_data, r8, r9) }
}

/// Install the title-anim speedup hook ONCE (MinHook, mirroring `install_pab_advance_hook`). Gated by
/// `title_anim_speedup_enabled` at the call site; the detour self-gates per frame too.
pub unsafe fn install_title_anim_speed_hook(base: usize) {
    if TITLE_ANIM_SPEED_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-anim-speed-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "title_menujob_speed_b0d400",
            TITLE_MENU_JOB_WAIT_RVA as u32,
            title_menujob_speed_detour as *mut c_void,
            &TITLE_ANIM_SPEED_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "title-anim-speed-hook: INSTALLED on STEP_MenuJobWait 0x{:x} -- one-shot FadeIn->Loop skip armed (zero-input, save-safe)",
            base + TITLE_MENU_JOB_WAIT_RVA,
        )),
        status => append_autoload_debug(format_args!(
            "title-anim-speed-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}

/// After-original detour for `CS::MoveMapStep::STEP_MoveMap` (state 18). Native writes the advance gate
/// at `MoveMapStep+0x4b8` near the end of this function; a normal game-task write runs too late because
/// the state machine can consume the gate and run Cleanup/Finish in the same tick. This detour calls the
/// original first, then clears the gate only for the current same-session reload until movement proof.
pub unsafe extern "system" fn movemapstep_step_move_map_gate_detour(
    mms: usize,
    task_data: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let orig_addr = MOVEMAPSTEP_STEP_MOVEMAP_ORIG.load(Ordering::SeqCst);
    if orig_addr == TITLE_OWNER_SCAN_START_ADDRESS {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig_addr) };
    let pre_reload_epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
    let pre_movement_proven = crate::compat::CAN_MOVE_CONFIRMED.load(Ordering::SeqCst)
        && crate::compat::MOVE_PROBE_EPOCH.load(Ordering::SeqCst) == pre_reload_epoch;
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        == SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
        && pre_reload_epoch > 0
        && !pre_movement_proven
        && mms > 0x10000
    {
        unsafe {
            *((mms + MOVEMAPSTEP_COUNTDOWN_100_OFFSET) as *mut i32) = 3;
            *((mms + MOVEMAPSTEP_HOLD_TIMER_270_OFFSET) as *mut i32) = 0x3a83126f;
        }
        let n = SYSTEM_QUIT_QUICKLOAD_MMS18_TIMER_HOLD_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 8 || n.is_power_of_two() {
            append_autoload_debug(format_args!(
                "AUTOLOAD-HANDOFF MMS18 TIMER HOLD #{n}: epoch={pre_reload_epoch} mms=0x{mms:x}; reset cd100=3/hold270=0x3a83126f before STEP_MoveMap"
            ));
        }
    }
    let ret = unsafe { orig(mms, task_data, r8, r9) };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let reload_epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        let movement_proven_for_reload = crate::compat::CAN_MOVE_CONFIRMED
            .load(Ordering::SeqCst)
            && crate::compat::MOVE_PROBE_EPOCH.load(Ordering::SeqCst) == reload_epoch;
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            == SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
            && reload_epoch > 0
            && !movement_proven_for_reload
            && mms > 0x10000
        {
            let old_gate =
                unsafe { safe_read_u8(mms + MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET) }.unwrap_or(0);
            if old_gate != 0 {
                unsafe {
                    *((mms + MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET) as *mut u8) = 0;
                }
                let n = SYSTEM_QUIT_QUICKLOAD_MMS4B8_HOLD_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                if n <= 8 || n.is_power_of_two() {
                    append_autoload_debug(format_args!(
                        "AUTOLOAD-HANDOFF MMS4B8 DETOUR HOLD #{n}: epoch={reload_epoch} mms=0x{mms:x} old_gate={old_gate}; cleared after STEP_MoveMap"
                    ));
                }
            }
            let old_next =
                unsafe { safe_read_i32(mms + MOVEMAPSTEP_NEXT_STEP_4C_OFFSET) }.unwrap_or(-1);
            if old_next != MOVEMAPSTEP_STEP_MOVEMAP_INDEX {
                unsafe {
                    *((mms + MOVEMAPSTEP_NEXT_STEP_4C_OFFSET) as *mut i32) =
                        MOVEMAPSTEP_STEP_MOVEMAP_INDEX;
                    *((mms + MOVEMAPSTEP_DONE_FLAG_50_OFFSET) as *mut u8) = 0;
                }
                let n =
                    SYSTEM_QUIT_QUICKLOAD_MMS18_NEXT_HOLD_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                if n <= 8 || n.is_power_of_two() {
                    append_autoload_debug(format_args!(
                        "AUTOLOAD-HANDOFF MMS18 NEXT HOLD #{n}: epoch={reload_epoch} mms=0x{mms:x} old_next={old_next}; restored next=18/done50=0 before Cleanup/Finish"
                    ));
                }
            }
        }
    }));
    ret
}

/// Diagnostic opt-in for the failed state-18 hold hook. Default OFF so canonical semaphore-diff runs are
/// observational and not contaminated by candidate writes.
pub fn movemapstep_step_move_map_gate_hold_enabled() -> bool {
    // DE-GATED (deprecate-env-marker-gate-allowlists-2026-07-19): the state-18 candidate-write hold
    // was a diagnostic behavioral experiment gated by env; env feature gates are forbidden; retired.
    false
}

/// Install the `STEP_MoveMap` after-original advance-gate hook ONCE. Runtime-falsified task-tick holds
/// were too late; this hook runs immediately after the native state-18 body.
pub unsafe fn install_movemapstep_step_move_map_gate_hook(base: usize) {
    if MOVEMAPSTEP_STEP_MOVEMAP_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "movemapstep_step_movemap_gate_af7cf0",
            MOVEMAPSTEP_STEP_MOVEMAP_RVA as u32,
            movemapstep_step_move_map_gate_detour as *mut c_void,
            &MOVEMAPSTEP_STEP_MOVEMAP_ORIG,
        );
    }
    append_autoload_debug(format_args!(
        "movemapstep-step-movemap-gate-hook: INSTALLED on 0x{:x} -- after-original +0x4b8/+0x4c reload hold armed",
        base + MOVEMAPSTEP_STEP_MOVEMAP_RVA,
    ));
    std::mem::forget(hooks);
}

/// BEFORE-original defer detour for `CS::InGameStep::STEP_MoveMap_Update` (deobf 0x140aec720). Root fix
/// for the warm-reload revert (bd er-effects-rs-9fmm): the parent reports the ending child finished
/// (`FUN_140eb5550`, an outer-stepper vtable done-query decoupled from the MoveMapStep finalize substate)
/// while the ending advancer is still at substate 8, then sets requestCode `+0xd8=2` and tears the child
/// down (`FUN_140eb54e0`) BEFORE the advancer runs case 8 (which posts substate 9). That strands the
/// reload and native reverts to title. This detour replicates the function's OWN "child not finished"
/// early-return: while the MoveMapStep finalize substate is in [1..=8] (finalize in progress) it skips
/// the original, so the advancer (pumped elsewhere -- STEP_MoveMap_Update does NOT pump it, confirmed by
/// decompile) gets the frames to reach 9; then the original runs and advances normally. Bounded by
/// INGAMESTEP_MOVEMAP_UPDATE_DEFER_MAX (fail-soft) and scoped to a committed reload epoch so the proven
/// boot load is untouched. DEFAULT behavior (no marker/env toggle); scoped to a committed reload epoch.
pub unsafe extern "system" fn ingamestep_step_movemap_update_defer_detour(
    ingame_step: usize,
    param2: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let orig_addr = INGAMESTEP_STEP_MOVEMAP_UPDATE_ORIG.load(Ordering::SeqCst);
    if orig_addr == TITLE_OWNER_SCAN_START_ADDRESS {
        return 0;
    }
    // INSTRUMENT (bd ROOT-load2-finalize-advancer-not-ticked-fun140afa7c0): count STEP_MoveMap_Update
    // calls per reload epoch. The finalize advancer FUN_140afa7c0 is ticked ~145x for load1 but ~1x for
    // load2. This detour runs on EVERY STEP_MoveMap_Update call, so if this counter CLIMBS for epoch>=1
    // (load2) while the advancer stays at 1, STEP_MoveMap_Update runs but skips the advancer call
    // INTERNALLY (an internal branch); if it stays LOW for load2, the parent stopped calling it.
    {
        let epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        let n = INGAMESTEP_MOVEMAP_UPDATE_DEFER_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 4 || n.is_multiple_of(120) {
            let mms = unsafe { safe_read_usize(ingame_step + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) }
                .unwrap_or(0);
            let (mms_step, fin) = if mms > PAB_MIN_HEAP_PTR {
                (
                    unsafe { safe_read_i32(mms + INGAMESTEP_STEP_STATE_OFFSET) }.unwrap_or(-1),
                    unsafe { safe_read_u8(mms + MOVEMAPSTEP_FINALIZE_SUBSTATE_12A_OFFSET) }
                        .map(i32::from)
                        .unwrap_or(-1),
                )
            } else {
                (-1, -1)
            };
            append_autoload_debug(format_args!(
                "STEP_MoveMap_Update CALL #{n} epoch={epoch} ingame=0x{ingame_step:x} mms=0x{mms:x} mms_step={mms_step} fin12a={fin}"
            ));
        }
    }
    let defer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ingame_step <= PAB_MIN_HEAP_PTR
            || SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) == 0
        {
            return false;
        }
        let Some(mms) = (unsafe {
            safe_read_usize(ingame_step + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET)
        })
        .filter(|&m| m > PAB_MIN_HEAP_PTR) else {
            return false;
        };
        // The finalize substate at +0x12a is a single BYTE (the SWITCH-ORACLE reads it with
        // safe_read_u8 at the same offset). Reading it as i32 folds in the adjacent bytes so the value
        // is almost never in [1..=8] -- the cause of the 0-firings inert run (DLL 63e70e0e). Read u8.
        let fin = unsafe { safe_read_u8(mms + MOVEMAPSTEP_FINALIZE_SUBSTATE_12A_OFFSET) }
            .map(|v| v as i32)
            .unwrap_or(-1);
        // DISABLED (bd load2-mms18-real-cause-my-defer-detour-deadlock-2026-07-19): deferring
        // STEP_MoveMap_Update while finalize is in [1..=8] DEADLOCKED load2. The premise ("the advancer
        // posts substate 9, pumped elsewhere") is WRONG: STEP_MoveMap_Update itself is what advances the
        // finalize, so skipping it strands load2 at mms=18/finalize=7 forever (log 'finalize-defer #64
        // held finalize=7'). load1 (untouched, epoch 0) advances mms 18->done fine. So NEVER defer --
        // run the update every frame like load1 does, so it sets requestCode=2 and the world completes.
        let _ = fin;
        INGAMESTEP_MOVEMAP_UPDATE_DEFER_TICKS.store(0, Ordering::SeqCst);
        let _ = &INGAMESTEP_MOVEMAP_UPDATE_DEFER_COUNT;
        let _ = INGAMESTEP_MOVEMAP_UPDATE_DEFER_MAX;
        false
    }))
    .unwrap_or(false);
    if defer {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig_addr) };
    unsafe { orig(ingame_step, param2, r8, r9) }
}

/// Install the STEP_MoveMap_Update finalize-defer hook ONCE.
pub unsafe fn install_ingamestep_step_movemap_update_defer_hook(base: usize) {
    if INGAMESTEP_STEP_MOVEMAP_UPDATE_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "ingamestep_step_movemap_update_defer_aec720",
            INGAMESTEP_STEP_MOVEMAP_UPDATE_RVA as u32,
            ingamestep_step_movemap_update_defer_detour as *mut c_void,
            &INGAMESTEP_STEP_MOVEMAP_UPDATE_ORIG,
        );
    }
    append_autoload_debug(format_args!(
        "ingamestep-step-movemap-update-defer-hook: INSTALLED on 0x{:x} -- defers d8=2/teardown while MoveMapStep finalize in [1..8] on a committed reload (default, no marker)",
        base + INGAMESTEP_STEP_MOVEMAP_UPDATE_RVA,
    ));
    std::mem::forget(hooks);
}

/// After-original override for the child-done query FUN_140eb5550 (rva 0xeb5530). STEP_MoveMap_Update
/// tears the MoveMapStep child down (FUN_140eb54e0 + requestCode+0xd8=2) when this returns done; for
/// load2 it returns done PREMATURELY (field25=0) -> advancer stops -> frozen (bd COMPLETE-CHAIN-load2-
/// child-torndown-early-fun140eb5550-done-premature). Isolate the MoveMapStep child's call
/// (rcx == current MoveMapStep + 0x108, bd mms-child-ezchildstepbase-at-plus0x108) and, on a committed
/// reload while the finalize is mid-walk (field25 in 0..=8), force the result NOT-done so
/// STEP_MoveMap_Update takes its `if(!done) return` branch (keeps the child, no teardown) while the
/// FD4-ticked child keeps ticking the advancer FUN_140afa7c0 until field25 reaches 9; then the real
/// done passes -> natural teardown -> world completes. ONLY the MoveMapStep child (rcx gate) on a
/// committed reload is touched; load1 (epoch 0) and every other child/query are unchanged.
pub unsafe extern "system" fn child_done_query_override_detour(
    child_base: usize,
    param2: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let orig_addr = CHILD_DONE_QUERY_ORIG.load(Ordering::SeqCst);
    if orig_addr == TITLE_OWNER_SCAN_START_ADDRESS {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig_addr) };
    let ret = unsafe { orig(child_base, param2, r8, r9) };
    // DIAG: for every call whose child_base-0x108 is a MoveMapStep at step 18, log ret + field25 so a
    // run shows exactly why the HOLD does/doesn't fire (throttled).
    if SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) != 0
        && child_base > PAB_MIN_HEAP_PTR + MOVEMAPSTEP_CHILD_EZSTEP_BASE_OFFSET
    {
        // UNGATED: log every committed-reload child-done call whose return is DONE (ret!=0), with the
        // mms_state + field25 that child_base-0x108 points to. Reveals the ACTUAL child_base<->MoveMapStep
        // relationship for the reload freeze (run13: the ==18 gate never matched, so the single run11
        // mms+0x108 data point does not generalize). Also probe the reliable-oracle mms for comparison.
        if (ret & 0xff) != 0 {
            let mms_d = child_base - MOVEMAPSTEP_CHILD_EZSTEP_BASE_OFFSET;
            let st_d = unsafe { safe_read_i32(mms_d + INGAMESTEP_STEP_STATE_OFFSET) }.unwrap_or(-999);
            let f_d = unsafe { safe_read_u8(mms_d + MOVEMAPSTEP_FINALIZE_SUBSTATE_12A_OFFSET) }
                .map(i32::from)
                .unwrap_or(-1);
            let omms = ORACLE_RELIABLE_MMS_PTR.load(Ordering::SeqCst);
            let nd = CHILD_DONE_DIAG_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            if nd <= 12 || nd.is_multiple_of(200) {
                append_autoload_debug(format_args!(
                    "child-done DIAG #{nd}: done-call child_base=0x{child_base:x} (child_base-0x108=0x{mms_d:x} state={st_d} field25={f_d}) oracle_mms=0x{omms:x} oracle_mms+0x108=0x{:x}",
                    omms.wrapping_add(MOVEMAPSTEP_CHILD_EZSTEP_BASE_OFFSET)
                ));
            }
        }
    }
    let hold = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // PHASE-3 (bd PHASE3-render-release-is-CommonFinalize): while the outgoing-teardown fix is active,
        // NEVER force the child not-done. This hold exists only to keep the OLD in-place reload's reused
        // WorldChrMan alive by suppressing the finalize; once the OUTGOING world is torn down first and the
        // reload rebuilds fresh, the hold must not fire (else it strands the child + keeps the render heavy).
        // It re-engages automatically on fail-soft (outgoing_teardown_suppresses_holds -> false).
        if crate::compat::gating::outgoing_teardown_suppresses_holds() {
            return false;
        }
        if SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) == 0
            || (ret & 0xff) == 0
        {
            return false;
        }
        // RELEASE post-stabilization (bd CORRECTION-STEP4-finalize-substate-is-0): the override only needs to
        // prevent PREMATURE child teardown DURING the load. Once the reloaded world has been genuinely live
        // (play_time advancing) for a sustained window, the load is complete -- stop holding so the
        // MoveMapStep child tears down like vanilla (else it is stranded alive forever = ez10-set + ~4fps
        // steady-state divergence). 180 frames (~3s) is well past load completion, so no premature-teardown
        // risk (the stranding it guards against happens in the first ~1s of the reload).
        const WORLD_STABLE_RELEASE_FRAMES: usize = 180;
        if er_telemetry::counters::WORLD_LIVE_STABLE_FRAMES.load(Ordering::SeqCst)
            >= WORLD_STABLE_RELEASE_FRAMES
        {
            return false;
        }
        // Derive the MoveMapStep from the query's OWN child_base (child EzChildStepBase = mms+0x108),
        // self-consistently -- no dependence on the telemetry-published pointer (which raced/mismatched
        // in run11). Validate it IS the MoveMapStep at step 18 (state @ +0x48 == 18) so other children's
        // queries (whose child_base-0x108 is not a step-18 MoveMapStep) are never held.
        if child_base <= PAB_MIN_HEAP_PTR + MOVEMAPSTEP_CHILD_EZSTEP_BASE_OFFSET {
            return false;
        }
        let mms = child_base - MOVEMAPSTEP_CHILD_EZSTEP_BASE_OFFSET;
        let mms_state = unsafe { safe_read_i32(mms + INGAMESTEP_STEP_STATE_OFFSET) }.unwrap_or(-1);
        if mms_state != MOVEMAPSTEP_STEP_MOVEMAP_INDEX {
            return false;
        }
        let fin = unsafe { safe_read_u8(mms + MOVEMAPSTEP_FINALIZE_SUBSTATE_12A_OFFSET) }
            .map(i32::from)
            .unwrap_or(-1);
        (0..=8).contains(&fin)
    }))
    .unwrap_or(false);
    if hold {
        let n = CHILD_DONE_HELD_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 4 || n.is_multiple_of(120) {
            append_autoload_debug(format_args!(
                "child-done HOLD #{n}: MoveMapStep child (mms+0x108) done->not-done while finalize walking -- keeps child so the advancer completes (load2 premature-teardown fix)"
            ));
        }
        return 0;
    }
    ret
}

/// Install the child-done-query override hook ONCE (unioned).
pub unsafe fn install_child_done_query_override_hook(base: usize) {
    if CHILD_DONE_QUERY_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "child_done_query_override_eb5530",
            CHILD_DONE_QUERY_RVA as u32,
            child_done_query_override_detour as *mut c_void,
            &CHILD_DONE_QUERY_ORIG,
        );
    }
    append_autoload_debug(format_args!(
        "child-done-query-override-hook: INSTALLED on 0x{:x} -- holds MoveMapStep child (mms+0x108) done->not-done while finalize<9 on a committed reload (prevents premature teardown)",
        base + CHILD_DONE_QUERY_RVA,
    ));
    std::mem::forget(hooks);
}

/// STEP_MoveMap_LoadlistInit (deobf rva 0xaec480 / dump 0x140aec570). Its build is gated on
/// `worldloadlistlistVirtualPath.size != 0` (InGameStep+0x108, a DlFixedString<wchar_t,128> inline:
/// +0x00 union{pointer when capacity>7 / inline}, +0x08 size(wchars), +0x10 capacity). When that
/// string is empty the game SKIPS building the loadlist -> no block-res -> WorldResWait hangs ->
/// mms stuck 18. This must be a PRODUCT hook (the union chains a base MinHook the product owns; the
/// trace-DLL copy never fired). READ-ONLY for now: it logs the DlFixedString per load epoch so a run
/// settles whether the STALLED load's path was EMPTY (empty-loadlist root confirmed) or POPULATED
/// (root is downstream/contention). The capture-replay WRITE is added once the layout is confirmed.
// deobf entry 0x140aec570 (== dump 0x140aec570; shift 0 for this fn -- the dump-deobf-shift tool
// mislanded at 0xaec480 in the -0xf0 sub-region). Verified by prologue mov [rsp+0x10],rbx; push rsi;
// sub rsp,0x20; mov rbx,rcx then the DAT_143d5db09=1 store (0x140aec57d) + CreateLoadlistlistFileCap
// call (0x140aec5f0). bd loadlist-capture-hook-wrong-address-0xaec480-midfunction-refind-entry.
pub const LOADLIST_INIT_RVA: usize = er_game_base::rva::STEP_MOVEMAP_LOADLIST_INIT_RVA;
const INGAMESTEP_WORLDLOADLIST_VPATH_OFFSET: usize = 0x108;
pub static LOADLIST_INIT_ORIG: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
static LOADLIST_INIT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::LOADLIST_INIT_CALLS;

pub unsafe extern "system" fn loadlist_init_capture_detour(
    ingamestep: usize,
    param2: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let orig_addr = LOADLIST_INIT_ORIG.load(Ordering::SeqCst);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let n = LOADLIST_INIT_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        let epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        // worldloadlistlistVirtualPath = InGameStep+0x108, DlFixedString<wchar_t,128> (Ghidra getStructure):
        //   field+0x00 string_buffer[128] INLINE text; field+0x108 DLString union; field+0x118 size;
        //   field+0x120 capacity. The gate is size!=0. (My earlier field+0x08 read landed mid-text.)
        let field = ingamestep + INGAMESTEP_WORLDLOADLIST_VPATH_OFFSET;
        let size = unsafe { safe_read_usize(field + 0x118) }.unwrap_or(usize::MAX);
        let cap = unsafe { safe_read_usize(field + 0x120) }.unwrap_or(usize::MAX);
        // text: inline buffer at field+0x00, or the union pointer at field+0x108 when heap-promoted.
        let uptr = unsafe { safe_read_usize(field + 0x108) }.unwrap_or(0);
        let str_base = if cap != usize::MAX && cap > 7 && uptr > 0x1_0000 {
            uptr
        } else {
            field
        };
        let mut preview = String::new();
        if size != usize::MAX && size <= 260 {
            for i in 0..size.min(120) {
                // ASCII path chars sit in the low byte of each UTF-16LE unit.
                match unsafe { safe_read_u8(str_base + i * 2) } {
                    Some(w) if (0x20..0x7f).contains(&w) => preview.push(w as char),
                    _ => preview.push('.'),
                }
            }
        }
        append_autoload_debug(format_args!(
            "loadlist-init CAPTURE #{n} epoch={epoch} InGameStep=0x{ingamestep:x} size={size} cap={cap} uptr=0x{uptr:x} path='{preview}'"
        ));
    }));
    if orig_addr == TITLE_OWNER_SCAN_START_ADDRESS {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig_addr) };
    unsafe { orig(ingamestep, param2, r8, r9) }
}

/// Install the LoadlistInit capture hook ONCE (product-owned so the union detour actually fires).
pub unsafe fn install_loadlist_init_capture_hook(base: usize) {
    if LOADLIST_INIT_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "loadlist_init_capture_aec480",
            LOADLIST_INIT_RVA as u32,
            loadlist_init_capture_detour as *mut c_void,
            &LOADLIST_INIT_ORIG,
        );
    }
    append_autoload_debug(format_args!(
        "loadlist-init-capture-hook: INSTALLED on 0x{:x} -- logs worldloadlistlistVirtualPath (InGameStep+0x108) per epoch to disambiguate the mms18 stall (empty-loadlist root vs downstream)",
        base + LOADLIST_INIT_RVA,
    ));
    std::mem::forget(hooks);
}
/// READ-ONLY trace detour for the title step-setter `SetState(owner, int state)` (deobf 0x140b0d960).
/// Logs every native state transition with a timestamp + the current owner+0xe0 (TitleTopDialog
/// holder) liveness, then calls the original UNCHANGED. Pure observation -- this is the
/// "look before acting" instrument for the menu-build-overlap lever: it reveals the exact wall-clock
/// at which BeginTitle(3) fires natively (and the full state sequence during boot), so we can decide
/// whether the 05_000_Title build has any headroom to be started earlier (overlap with init) before
/// risking a forced SetState (which has NO double-build guard). bd menu-build-overlap-lever-2026-06-24.
pub unsafe extern "system" fn title_setstate_trace_detour(owner: usize, state: i32) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if owner > PAB_MIN_HEAP_PTR {
            TITLE_SETSTATE_TRACE_LAST_OWNER.store(owner, Ordering::SeqCst);
        }
        let dialog = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0)
        } else {
            0
        };
        let committed = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }.unwrap_or(-999)
        } else {
            -999
        };
        let b8 = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_usize(owner + TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET) }
                .unwrap_or(0)
        } else {
            0
        };
        if owner > PAB_MIN_HEAP_PTR
            && SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN
            && (state == 2 || state == 3 || state == TITLE_STEP_MENU_JOB_WAIT)
            && let Ok(base) = game_module_base() {
                let table = unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }
                    .unwrap_or(0);
                if table == base + INNER_TITLE_STATE_TABLE_RVA {
                    let previous = TITLE_OWNER_PTR.swap(owner, Ordering::SeqCst);
                    TITLE_OWNER_SCAN_COUNTDOWN
                        .store(TITLE_OWNER_SCAN_CALL_INTERVAL, Ordering::SeqCst);
                    if previous != owner {
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: latched native SetState title owner=0x{owner:x} state={state} previous=0x{previous:x} table=0x{table:x}; overriding stale scan candidate"
                        ));
                    }
                }
            }
        // BLOCKER ATTRIBUTION (2026-07-19): a post-finalize SetState(owner,2) from committed_was=6
        // tears down the just-entered reload world. To decide native-vs-ours WITHOUT a return-address
        // capture, log the concurrent state: our return-title chain is the only way OUR code can cause
        // a native SetState(2) (we never call the setter with state 2 directly -- we submit the game's
        // own return-title builder 0x79d700). So SetState(2) with rt_submit unchanged/old across it, at
        // phase==AUTOLOAD_HANDOFF, is a genuine native InGameStep decision; a fresh rt_submit near it is
        // ours. request_code (InGameStep+0xd8) tells whether the finalize had reached in-world (>=2).
        let ig_request_code = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
                .filter(|&ig| ig > 0x10000)
                .and_then(|ig| unsafe { safe_read_i32(ig + IN_GAME_STEP_REQUEST_CODE_D8_OFFSET) })
                .unwrap_or(-1)
        } else {
            -1
        };
        let quickload_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
        let rt_submit = SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst);
        let own_phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
        // ENDING-CONDITION SNAPSHOT at the exact SetState frame (bd er-effects-rs-9fmm): the MoveMapStep
        // ending evaluator FUN_140afa7c0 sets its cVar10 from any of {warpRequested GM+0x10, menuData+0x5d,
        // force-flag 0x143d856a0, GM+0xb7c/0xb7d, deadReset, FUN_140679460=b73&&bc4!=3}. Log ALL of them on
        // a SetState(...,2) from committed=6 so the run NAMES the revert trigger instead of us guessing.
        let gm_rt = game_man_ptr_or_null();
        let (warp_req, b73_now, bc4_now) = if gm_rt > PAB_MIN_HEAP_PTR {
            (
                unsafe { safe_read_u8(gm_rt + GAME_MAN_WARP_REQUESTED_10_OFFSET) }
                    .map_or(-1, i32::from),
                unsafe { safe_read_u8(gm_rt + GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET) }
                    .map_or(-1, i32::from),
                unsafe { safe_read_i32(gm_rt + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) }
                    .unwrap_or(-1),
            )
        } else {
            (-1, -1, -1)
        };
        let (md5d, md5e) = game_module_base()
            .ok()
            .and_then(|base| unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) })
            .filter(|&m| m > PAB_MIN_HEAP_PTR)
            .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) })
            .filter(|&m| m > PAB_MIN_HEAP_PTR)
            .map(|md| {
                (
                    unsafe { safe_read_u8(md + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) }
                        .map_or(-1, i32::from),
                    unsafe { safe_read_u8(md + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) }
                        .map_or(-1, i32::from),
                )
            })
            .unwrap_or((-1, -1));
        append_autoload_debug(format_args!(
            "title-setstate-trace: SetState(owner=0x{owner:x}, state={state}({})) committed_was={committed}({}) req_code={ig_request_code}({}) quickload_phase={quickload_phase} rt_submit={rt_submit} own_phase={own_phase} ENDCOND[warp={warp_req} b73={b73_now} bc4={bc4_now} md5d={md5d} md5e={md5e}] owner+0xe0(dialog)=0x{dialog:x} owner+0xb8(gate)=0x{b8:x}",
            title_step_state_name(state),
            title_step_state_name(committed),
            ingamestep_request_code_name(ig_request_code)
        ));
    }));
    let orig = TITLE_SETSTATE_TRACE_ORIG.load(Ordering::SeqCst);
    if orig == TITLE_OWNER_SCAN_START_ADDRESS || orig == 0 {
        return;
    }
    // Missing-save in-game picker guard: while no save has been selected, DENY only the two
    // world-load entry states (RE-verified 2026-07-07: every path into the world -- Continue,
    // Load-slot confirm, New Game, NG+ -- funnels through SetState(4=BeginNewGame) or
    // SetState(5=PlayGame); menu states 0..3/10/11 must flow or the title never becomes
    // interactive). The old behavior condvar-BLOCKED every SetState here, which froze the title
    // thread; now the title boots to its native no-save menu and the picker rides it. Skipping
    // the call (not waiting) keeps the title thread alive; the request is simply dropped.
    if crate::boot_hold::should_deny_world_entry(missing_save_selection_pending(), state) {
        append_autoload_debug(format_args!(
            "title-setstate-trace: DENIED SetState(owner=0x{owner:x}, state={state}) -- world entry blocked until the missing-save picker resolves"
        ));
        return;
    }
    let f: unsafe extern "system" fn(usize, i32) = unsafe { std::mem::transmute(orig) };
    unsafe { f(owner, state) };
}
/// Install the READ-ONLY title step-setter trace hook ONCE. Mirrors `install_pab_advance_hook`.
/// Save-safe: the detour only logs + passes through. bd menu-build-overlap-lever-2026-06-24.
pub unsafe fn install_title_setstate_trace_hook(base: usize) {
    if TITLE_SETSTATE_TRACE_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-setstate-trace-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "title_setstate_b0d960",
            TITLE_SET_STATE_RVA as u32,
            title_setstate_trace_detour as *mut c_void,
            &TITLE_SETSTATE_TRACE_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "title-setstate-trace-hook: INSTALLED on SetState(owner,int) 0x{:x} -- read-only native state-transition timeline armed",
            base + TITLE_SET_STATE_RVA,
        )),
        status => append_autoload_debug(format_args!(
            "title-setstate-trace-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}
