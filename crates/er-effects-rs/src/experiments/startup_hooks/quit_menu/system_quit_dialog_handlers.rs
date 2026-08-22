use super::*;

pub(crate) unsafe fn system_quit_open_profile_load_dialog(action_obj: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    let system_dialog =
        unsafe { safe_read_usize(action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }
            .unwrap_or(NULL);
    if system_dialog < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- action=0x{action_obj:x} dialog=0x{system_dialog:x} is not heap-like"
        ));
        return false;
    }
    unsafe { system_quit_open_profile_load_dialog_on(system_dialog) }
}

/// Submit the native `05_010_ProfileSelect` window against an already-resolved System/Quit
/// PropertyEditDialog. Split out of the action-object form (save-game-flow WP3) because the save
/// flow opens the destination browser from the dialog it captured at the row press -- it never has
/// a row action object of its own.
pub(crate) unsafe fn system_quit_open_profile_load_dialog_on(system_dialog: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- module base unavailable"
        ));
        return false;
    };
    if system_dialog < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- dialog=0x{system_dialog:x} is not heap-like"
        ));
        return false;
    }
    let scene_proxy = system_dialog + SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET;
    let scene_proxy_vt = unsafe { safe_read_usize(scene_proxy) }.unwrap_or(NULL);
    let want_scene_proxy_vt = base + SCENE_OBJ_PROXY_VTABLE_RVA;
    if scene_proxy_vt != want_scene_proxy_vt {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- dialog=0x{system_dialog:x} scene_proxy=dialog+0x{SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET:x}=0x{scene_proxy:x} vt=0x{scene_proxy_vt:x} want=0x{want_scene_proxy_vt:x}"
        ));
        return false;
    }
    // Native title/menu route callers pass `owner + 0x50` as the MenuWindowJob's
    // field2_0x50 list argument. MenuWindowJob::Run later appends the loaded
    // owning MenuWindow to this DLFixedVector via FUN_140733ff0. Passing the
    // SceneObjProxy backref here is wrong: it lets the resource load start, then
    // asserts in DLFixedVector.inl line 0x296 when Run appends to a full/wrong
    // object.
    let menu_window_list = system_dialog + 0x50;
    let menu_window_list_count = unsafe { safe_read_usize(menu_window_list + 0x48) }.unwrap_or(!0);
    if menu_window_list_count >= 8 {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- candidate menu_window_list=dialog+0x50=0x{menu_window_list:x} count@+0x48={menu_window_list_count} would overflow DLFixedVector<8>"
        ));
        return false;
    }
    let Ok(wrapper_addr) = game_rva(PROFILE_SELECT_WRAPPER_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- failed to resolve ProfileSelect wrapper rva 0x{PROFILE_SELECT_WRAPPER_RVA:x}"
        ));
        return false;
    };
    let Ok(submit_addr) = game_rva(MENU_JOB_SUBMIT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- failed to resolve menu-job submit rva 0x{MENU_JOB_SUBMIT_RVA:x}"
        ));
        return false;
    };
    let job_slot = &SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT as *const AtomicUsize as usize;
    SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.store(NULL, Ordering::SeqCst);
    // Latch the profile-load flow as active NOW (before the ProfileSelect job/Run hook runs) so the native
    // load-confirm MessageBox the own_stepper self-pump triggers is suppressed and cannot crash the game.
    // Cleared on ProfileSelect reset (system_quit_reset_profile_select_state).
    SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE.store(1, Ordering::SeqCst);
    let wrapper: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(wrapper_addr) };
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route FIRE 05_010_ProfileSelect wrapper 0x{wrapper_addr:x}(rcx=job_slot=0x{job_slot:x}, rdx=menu_window_list=dialog+0x50=0x{menu_window_list:x} count={menu_window_list_count}, r8=scene_proxy=0x{scene_proxy:x}) from system_dialog=0x{system_dialog:x}"
    ));
    let ret = unsafe { wrapper(job_slot, menu_window_list, scene_proxy) };
    let job = SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.load(Ordering::SeqCst);
    let job_vt = if job >= HEAP_LO {
        unsafe { safe_read_usize(job) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if job < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route 05_010 wrapper returned=0x{ret:x} job_slot=0x{job_slot:x} job=0x{job:x} job_vt=0x{job_vt:x}; no job to submit"
        ));
        return false;
    }
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(submit_addr) };
    let submit_queue = system_dialog + 0x10;
    SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.store(menu_window_list, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.store(system_dialog, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(system_dialog, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route SUBMIT job=0x{job:x} job_vt=0x{job_vt:x} via 0x{submit_addr:x}(queue=dialog+0x10=0x{submit_queue:x}, job_slot=0x{job_slot:x}); armed ProfileSelect list observer=0x{menu_window_list:x} -- no slot activation/no load"
    ));
    unsafe { submit(submit_queue, job_slot) };
    let job_after_submit = SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route submitted 05_010 wrapper job; job_slot_after=0x{job_after_submit:x}"
    ));
    true
}

pub(crate) unsafe extern "system" fn system_quit_menu_window_list_push_hook(
    list: usize,
    window: usize,
) -> u8 {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    let orig = SYSTEM_QUIT_WINDOW_LIST_PUSH_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: MenuWindow list push trampoline unset for list=0x{list:x} window=0x{window:x} -- fail-closed return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(orig) };
    let ret = unsafe { original(list, window) };
    let armed_list = SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.load(Ordering::SeqCst);
    let system_dialog = SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.load(Ordering::SeqCst);
    if armed_list == 0 || armed_list != list || system_dialog == 0 {
        return ret;
    }
    SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.store(0, Ordering::SeqCst);
    let count = unsafe { safe_read_usize(list + 0x48) }.unwrap_or(0);
    let slot0 = unsafe { safe_read_usize(system_quit_list_slot_addr(list, 0)) }.unwrap_or(NULL);
    let slot1 = if count > 1 {
        unsafe { safe_read_usize(system_quit_list_slot_addr(list, 1)) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let top_window = slot0;
    let top_vt = if top_window >= HEAP_LO {
        unsafe { safe_read_usize(top_window) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let top_id = if top_window >= HEAP_LO {
        unsafe { safe_read_u16(top_window + 0x180) }.unwrap_or(u16::MAX)
    } else {
        u16::MAX
    };
    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect append observed list=0x{list:x} dialog=0x{system_dialog:x} count={count} slot0/top=0x{slot0:x} top_vt=0x{top_vt:x} top_id=0x{top_id:x} slot1=0x{slot1:x} appended_window=0x{window:x} ret={ret}"
    ));
    SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW.store(window, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_LIST.store(list, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID.store(top_id as usize, Ordering::SeqCst);
    ret
}

/// Route one Quit-tab row ACTION thunk (`FUN_140961640` for the first native row, `FUN_1409610d0`
/// for the second and every row cloned from it).
///
/// `action_obj` is the thunk's `this`, and it is ONLY `controller + 0x70` -- the controller's own
/// inline `std::function` storage (see `system_quit_row_identity.rs`). It therefore cannot name a
/// row, so the row is resolved positively from the row table + the live label at the dialog's list
/// cursor. An unresolvable row is SUPPRESSED rather than forwarded, because the native action behind
/// the second row is the irreversible Return to Desktop.
pub(crate) unsafe fn system_quit_route_button_action_or_forward(
    action_obj: usize,
    orig: usize,
    hook_name: &str,
) -> usize {
    if action_obj == 0 {
        return 0;
    }
    let controller = system_quit_controller_of_action_alias(action_obj);
    let dialog =
        unsafe { safe_read_usize(action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }
            .unwrap_or(0);
    let cursor = if dialog >= 0x10000 {
        unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };
    // This `_Func_impl` thunk vtable (dump 0x142b125d0 slot +0x10) is shared by several construction
    // sites, so the hook also sees cancel/confirm rows of dialogs that are NOT the patched Quit tab.
    // Forward those untouched -- and before resolving, so foreign dialogs never pollute the row
    // oracles.
    let table_dialog = SYSTEM_QUIT_ROW_TABLE_DIALOG.load(Ordering::SeqCst);
    if dialog == 0 || table_dialog == 0 || dialog != table_dialog {
        if orig == HOOK_ORIGINAL_UNSET {
            append_autoload_debug(format_args!(
                "system-quit-save: {hook_name} action trampoline is unset for action_alias=0x{action_obj:x} dialog=0x{dialog:x} table_dialog=0x{table_dialog:x}; fail-open return 0"
            ));
            return 0;
        }
        let original: unsafe extern "system" fn(usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        return unsafe { original(action_obj) };
    }
    // No event object reaches an action thunk, so the input kind is unclassifiable here -- which
    // costs nothing, because the list cursor names the row for every input kind alike.
    let verdict = unsafe { system_quit_resolve_row_now(dialog, 0) };
    let verdict_text = system_quit_row_verdict_text(verdict);
    match verdict.resolved_row() {
        Some(QuitRow::LoadProfile) => {
            let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
                append_autoload_debug(format_args!(
                    "system-quit-dup: cloned quick-load action re-entry ignored action_alias=0x{action_obj:x} phase={phase}; native handoff already armed"
                ));
                return 0;
            }
            SYSTEM_QUIT_NOOP_SELECTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let opened = unsafe { system_quit_open_profile_load_dialog(action_obj) };
            append_autoload_debug(format_args!(
                "system-quit-dup: Load Profile action selected {hook_name} action_alias=0x{action_obj:x} controller=0x{controller:x} cursor={cursor} {verdict_text} opened={opened}; suppressing native Quit Game row action until ProfileSelect confirms slot"
            ));
            0
        }
        Some(QuitRow::LoadSaveProfiles) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let opened = unsafe { system_quit_open_save_picker_menu(action_obj) };
            append_autoload_debug(format_args!(
                "system-quit-load-save-profiles: Load Save Profiles action selected {hook_name} action_alias=0x{action_obj:x} controller=0x{controller:x} cursor={cursor} {verdict_text} opened={opened:?} (in-game save picker); suppressing native Quit Game row action"
            ));
            0
        }
        Some(QuitRow::SaveGame) => {
            if dialog < 0x10000 {
                append_autoload_debug(format_args!(
                    "system-quit-save: Save Game row press IGNORED action_alias=0x{action_obj:x}; dialog=0x{dialog:x} is not heap-like"
                ));
                return 0;
            }
            // SAVE-FLOW re-entry guard (WP1): one commit at a time, and never while a profile
            // switch owns the quit machinery -- both flows drive the same close sequence and
            // GameMan save fields.
            let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            let stage = SAVE_FLOW_STAGE.load(Ordering::SeqCst);
            if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE || stage != SAVE_FLOW_STAGE_IDLE {
                append_autoload_debug(format_args!(
                    "system-quit-save: Save Game row press IGNORED action_alias=0x{action_obj:x} quickload_phase={phase} save_flow_stage={stage}; a switch or save commit is already in flight"
                ));
                return 0;
            }
            SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.store(0, Ordering::SeqCst);
            SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let started = unsafe { system_quit_save_game_start_flow(dialog) };
            append_autoload_debug(format_args!(
                "system-quit-save: Save Game native row selected {hook_name} action_alias=0x{action_obj:x} dialog=0x{dialog:x} cursor={cursor} {verdict_text}; staged the destination list started={started} stage={}; suppressed native Quit Game/return-title action",
                SAVE_FLOW_STAGE.load(Ordering::SeqCst)
            ));
            0
        }
        Some(QuitRow::ReturnToDesktop) => {
            // The genuine Return to Desktop, positively identified. Make quit an INSTANT ALT+F4:
            // persist the save, release the cursor clip, and ExitProcess(0) BEFORE the world
            // teardown renders a loading screen. The old clean-kill (system_quit_ownership_repro)
            // fired mid-teardown, so the loading cover was already visible.
            //
            // Two further refusals guard this irreversible step (save-safety hardening, kept):
            //   * the activated controller must not be the Save Game row's -- a Save Game press can
            //     never quit the game;
            //   * `save_flow_stage == IDLE` -- never exit mid save-flow, where a commit may be armed
            //     or in flight and a process exit would tear it in half.
            let save_flow_stage = SAVE_FLOW_STAGE.load(Ordering::SeqCst);
            let save_game_controller =
                SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst);
            if (save_game_controller != 0 && controller == save_game_controller)
                || save_flow_stage != SAVE_FLOW_STAGE_IDLE
            {
                SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "quit-to-desktop: REFUSING the instant quit at {hook_name} for controller=0x{controller:x} cursor={cursor} -- save_game_controller=0x{save_game_controller:x} save_flow_stage={save_flow_stage}; a Save Game press must never quit the game and a save flow must never be torn in half"
                ));
                return 0;
            }
            if !system_quit_row_gate_instant_quit(verdict, hook_name) {
                return 0;
            }
            unsafe { system_quit_save_game_request_save_only() };
            release_input_block_now();
            append_autoload_debug(format_args!(
                "quit-to-desktop: Return-to-Desktop confirmed at {hook_name} controller=0x{controller:x} action_alias=0x{action_obj:x} cursor={cursor} {verdict_text}; requested save + released cursor clip; INSTANT ExitProcess(0) before world teardown (no loading screen)"
            ));
            unsafe { ExitProcess(0) }
        }
        None => {
            // The patched Quit dialog, row NOT identified. Suppress instead of forwarding:
            // forwarding this thunk runs the native action, and for the second Quit row that action
            // IS Return to Desktop. A row press that does nothing is a nuisance; a row press that
            // terminates the process is not shippable.
            system_quit_row_gate_instant_quit(verdict, hook_name);
            append_autoload_debug(format_args!(
                "system-quit-save: {hook_name} row press SUPPRESSED action_alias=0x{action_obj:x} controller=0x{controller:x} dialog=0x{dialog:x} cursor={cursor} {verdict_text}; the native action behind this thunk is the irreversible Return to Desktop, so an unidentified row of the patched Quit tab must not reach it"
            ));
            0
        }
    }
}

pub(crate) unsafe extern "system" fn system_quit_noop_desktop_action_hook(
    action_obj: usize,
) -> usize {
    let orig = SYSTEM_QUIT_NOOP_ACTION_ORIG.load(Ordering::SeqCst);
    unsafe { system_quit_route_button_action_or_forward(action_obj, orig, "save-game/first-row") }
}

pub(crate) unsafe extern "system" fn system_quit_return_desktop_action_hook(
    action_obj: usize,
) -> usize {
    let orig = SYSTEM_QUIT_RETURN_DESKTOP_ACTION_ORIG.load(Ordering::SeqCst);
    unsafe {
        system_quit_route_button_action_or_forward(action_obj, orig, "return-desktop/second-row")
    }
}

pub(crate) unsafe fn system_quit_forward_button_controller_activation(
    controller: usize,
    event_kind: u32,
    event_a: usize,
    event_b: usize,
) {
    let orig = PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: PropertyNewButtonController activation trampoline unset for controller=0x{controller:x}; fail-closed return"
        ));
        return;
    }
    let original: unsafe extern "system" fn(usize, u32, usize, usize) =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(controller, event_kind, event_a, event_b) };
}

pub(crate) unsafe fn system_quit_controller_should_invoke_action(
    controller: usize,
    event_a: usize,
) -> bool {
    let Ok(predicate_addr) = game_rva(PROPERTY_NEW_BUTTON_CONTROLLER_SHOULD_INVOKE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve PropertyNewButtonController action predicate rva 0x{PROPERTY_NEW_BUTTON_CONTROLLER_SHOULD_INVOKE_RVA:x}; forwarding native activation"
        ));
        return false;
    };
    let predicate: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(predicate_addr) };
    unsafe { predicate(controller, event_a) != 0 }
}

/// `PropertyNewButtonController::Activate` (dump `FUN_1409749f0`, vtable slot 2). Called once per
/// frame per DISPATCHED row with the live event; the controller's own should-invoke predicate decides
/// whether that event is a real confirm.
///
/// The controller is used ONLY to scope the hook to the patched Quit tab. It cannot name a row: the
/// dispatch collapses cloned buttons onto the native Return-to-Desktop controller, and the pointer at
/// `controller + 0xa8` is merely `controller + 0x70`. Row identity comes from
/// `system_quit_resolve_row_now`, i.e. the dialog's own list cursor.
pub(crate) unsafe extern "system" fn property_new_button_controller_activate_hook(
    controller: usize,
    event_kind: u32,
    event_a: usize,
    event_b: usize,
) {
    if !system_quit_controller_is_a_quit_row(controller) {
        // Not a row of the patched Quit tab: vanilla behaviour, untouched. While the save picker owns
        // ProfileSelect, use the same event to capture a same-row drive-strip click before forwarding;
        // the normal ProfileLoad activation hook will consume the pending cell.
        if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0
            && unsafe { system_quit_controller_should_invoke_action(controller, event_a) }
        {
            unsafe { save_picker_note_drive_strip_click_event(event_a) };
        }
        unsafe {
            system_quit_forward_button_controller_activation(
                controller, event_kind, event_a, event_b,
            )
        };
        return;
    }
    // Focus / per-frame update rather than a confirm: never routes, never quits.
    if !unsafe { system_quit_controller_should_invoke_action(controller, event_a) } {
        unsafe {
            system_quit_forward_button_controller_activation(
                controller, event_kind, event_a, event_b,
            )
        };
        return;
    }
    let action_alias =
        controller.saturating_add(PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET);
    let dialog =
        unsafe { safe_read_usize(action_alias + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }
            .unwrap_or(0);
    let verdict = unsafe { system_quit_resolve_row_now(dialog, event_a) };
    let verdict_text = system_quit_row_verdict_text(verdict);
    match verdict.resolved_row() {
        Some(QuitRow::LoadProfile) => {
            let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
                append_autoload_debug(format_args!(
                    "system-quit-dup: controller quick-load activation ignored controller=0x{controller:x} phase={phase}; native handoff already armed"
                ));
                return;
            }
            SYSTEM_QUIT_NOOP_SELECTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let opened = unsafe { system_quit_open_profile_load_dialog(action_alias) };
            append_autoload_debug(format_args!(
                "system-quit-dup: Load Profile controller selected controller=0x{controller:x} {verdict_text} event_kind={event_kind} opened={opened}; suppressing native button activation"
            ));
        }
        Some(QuitRow::LoadSaveProfiles) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let opened = unsafe { system_quit_open_save_picker_menu(action_alias) };
            append_autoload_debug(format_args!(
                "system-quit-load-save-profiles: Load Save Profiles controller selected controller=0x{controller:x} {verdict_text} event_kind={event_kind} opened={opened:?} (in-game save picker); suppressing native button activation"
            ));
        }
        Some(QuitRow::ReturnToDesktop) => {
            // Positively the genuine Return to Desktop. Make quit an INSTANT ALT+F4: persist the
            // save, release the cursor, and ExitProcess(0) BEFORE the world teardown renders any
            // loading screen / our isolated overlay.
            //
            // Require that NO profile switch is in flight: the native return-desktop controller is
            // dispatched again from ProfileSelect (observed: 12 activations carried it during one
            // switch), so without this gate a switch's activation would ExitProcess mid-switch. And
            // never exit mid save-flow, where a commit may be armed or in flight.
            let switch_in_flight = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
                || SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0
                || SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE.load(Ordering::SeqCst) != 0;
            let save_flow_in_flight =
                SAVE_FLOW_STAGE.load(Ordering::SeqCst) != SAVE_FLOW_STAGE_IDLE;
            if switch_in_flight || save_flow_in_flight {
                SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "quit-to-desktop: REFUSING the instant quit at controller=0x{controller:x} -- switch_in_flight={switch_in_flight} save_flow_in_flight={save_flow_in_flight}; forwarding the native activation instead"
                ));
                unsafe {
                    system_quit_forward_button_controller_activation(
                        controller, event_kind, event_a, event_b,
                    )
                };
                return;
            }
            if !system_quit_row_gate_instant_quit(verdict, "return-desktop/controller") {
                // Unreachable while `resolved_row()` says Return to Desktop, but the gate stays the
                // single authority over the irreversible step.
                unsafe {
                    system_quit_forward_button_controller_activation(
                        controller, event_kind, event_a, event_b,
                    )
                };
                return;
            }
            unsafe { system_quit_save_game_request_save_only() };
            release_input_block_now();
            append_autoload_debug(format_args!(
                "quit-to-desktop: Return-to-Desktop controller CLICK controller=0x{controller:x} action_alias=0x{action_alias:x} {verdict_text} event_kind={event_kind}; requested save + released cursor; INSTANT ExitProcess(0) (no teardown/loading screen)"
            ));
            unsafe { ExitProcess(0) };
        }
        // Save Game keeps flowing through its own native action thunk, which the action-route hook
        // owns (that is where the confirm chain and its re-entry guards live).
        Some(QuitRow::SaveGame) | None => {
            if verdict.resolved_row().is_none() {
                append_autoload_debug(format_args!(
                    "quit-to-desktop: controller confirm NOT routed controller=0x{controller:x} dialog=0x{dialog:x} {verdict_text} event_kind={event_kind}; forwarding the native activation, which the action-route hook gates again"
                ));
            }
            unsafe {
                system_quit_forward_button_controller_activation(
                    controller, event_kind, event_a, event_b,
                )
            };
        }
    }
}

pub(crate) fn wide_z(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// The ACTIVE save file the character-switch feature snapshots + restores + writes to. Resolved from
/// runtime GROUND TRUTH via `active_save_file_for_system_quit()`: a direct-file save selected in the
/// missing-save picker is a read-only source copied into the private redirected native save tree, so
/// this returns the game's native `%APPDATA%/EldenRing/<steamid>/ER0000.{co2|sl2}` path for writes.
/// Explicit/default saves keep using the normal configured/default resolver. Never write back to the
/// direct source file under `save-files/` or a user-picked path.
pub(crate) fn system_quit_env_save_path() -> Result<String, &'static str> {
    let Some(path) = active_save_file_for_system_quit() else {
        return Err(
            "no active save file (direct/configured save unset and no default ER0000 save resolved)",
        );
    };
    let path = path.to_string_lossy();
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("resolved active save file is blank");
    }
    Ok(trimmed.trim_end_matches(['/', '\\']).to_owned())
}

pub(crate) fn system_quit_env_save_dir() -> Result<String, &'static str> {
    let trimmed = system_quit_env_save_path()?;
    let Some(sep) = trimmed.rfind(['/', '\\']) else {
        return Err("configured save_file has no parent directory");
    };
    let dir = &trimmed[..sep];
    if dir.is_empty() {
        return Err("configured save_file parent directory is empty");
    }
    Ok(dir.to_owned())
}

pub(crate) fn system_quit_path_for_windows(path: &str) -> Vec<u16> {
    let mut win = if path.starts_with('/') {
        format!("Z:{}", path.replace('/', "\\"))
    } else {
        path.replace('/', "\\")
    };
    while win.ends_with('\\') && win.len() > 3 {
        win.pop();
    }
    wide_z(&win)
}

#[allow(dead_code)] // Retained: Wide-picker path decode, beside the live wide/Windows path helpers it belongs with.
pub(crate) fn system_quit_path_from_windows_picker(path: &[u16]) -> Option<String> {
    let end = path.iter().position(|c| *c == 0).unwrap_or(path.len());
    if end == 0 {
        return None;
    }
    String::from_utf16(&path[..end]).ok()
}

pub(crate) fn system_quit_windows_path_for_log(path: &str) -> String {
    if let Some(rest) = path
        .strip_prefix("Z:\\")
        .or_else(|| path.strip_prefix("z:\\"))
    {
        format!("/{}", rest.replace('\\', "/"))
    } else {
        path.to_owned()
    }
}

/// Validate + ingest a picked save container path (any picker UI feeds this): runtime-flavor
/// extension filter, BND4 parse, SteamID normalization, ProfileSummary slot preview, candidate
/// staging, and last-picked-directory persistence. Menu-thread only (preview writes + renderer
/// refresh).
/// The caller is responsible for the pre-pick work (`system_quit_save_swap_restore_profile_summary`
/// + `system_quit_save_swap_arm_original`), which happens at picker OPEN time.
pub(crate) unsafe fn system_quit_ingest_picked_save(selected_path: &str) -> bool {
    // Extension policy: vanilla only accepts `.sl2`; Seamless accepts its native `.co2` plus
    // vanilla `.sl2` sources so a vanilla save can be loaded/imported while ERSC owns the session.
    let seamless = save_picker_seamless_mode_after_settle("system-quit-ingest-picked-save");
    let allowed_exts: &[&str] = if seamless { &["co2", "sl2"] } else { &["sl2"] };
    let selected_log = system_quit_windows_path_for_log(selected_path);
    if !Path::new(selected_path).is_file() {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(er_save_picker::PickerStatusMessage::new(
            "SAVE NOT FOUND",
            "The selected path is missing or is not a file.",
        ));
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected path is not a file '{}' (visible reason set)",
            selected_log
        ));
        return false;
    }
    // THE shared extension filter (`save_picker.rs`), not a second copy: the in-game listing, the
    // OS dialog's post-return check and this ingest gate must not be able to disagree about which
    // container flavors the active runtime accepts.
    let ext_ok = crate::experiments::save_picker::save_picker_extension_accepted(
        Path::new(selected_path),
        allowed_exts,
    );
    if !ext_ok {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(
            er_save_picker::PickRejection::WrongExtension.status_message(&allowed_exts.join("/.")),
        );
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: rejected '{}' -- picker accepts only .{} (seamless={seamless}; visible reason set)",
            selected_log,
            allowed_exts.join("/.")
        ));
        return false;
    }
    let Ok(mut bytes) = fs::read(selected_path) else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(
            er_save_picker::PickRejection::Unreadable.status_message("SL2"),
        );
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: failed to read selected save '{}' (visible reason set)",
            selected_log
        ));
        return false;
    };
    let len = bytes.len() as u64;
    let raw_hash = system_quit_hash_bytes(&bytes);
    if er_save_loader::bnd4::parse_entries(&bytes).is_err() {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(
            er_save_picker::PickRejection::NotBnd4.status_message("SL2"),
        );
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected save is not a valid BND4 '{}' len={len} hash=0x{raw_hash:016x} (visible reason set)",
            selected_log
        ));
        return false;
    }
    let Ok(base) = game_module_base() else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(er_save_picker::PickerStatusMessage::new(
            "GAME STATE NOT READY",
            "The save is valid, but the game was not ready to preview it.",
        ));
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected save '{}' is valid but game module base is unavailable (visible reason set)",
            selected_log
        ));
        return false;
    };
    normalize_save_bytes_to_active_steam_id(base, &mut bytes, "system-quit-picker-selection");
    let hash = system_quit_hash_bytes(&bytes);
    let mask = unsafe { system_quit_apply_foreign_profile_summary_preview(base, &bytes) };
    if mask == 0 {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(
            er_save_picker::PickRejection::NoLoadableCharacter.status_message("SL2"),
        );
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected save had no readable character slots '{}' len={len} hash=0x{hash:016x} (visible reason set)",
            selected_log
        ));
        return false;
    }
    {
        let mut st = system_quit_save_swap_lock();
        st.candidate_bytes = bytes;
        st.candidate_hash = hash;
        st.candidate_slot_mask = mask;
        st.preview_applied = true;
    }
    if crate::config::autoupdate_preferred_picker_dir_enabled()
        && let Some(parent) = Path::new(selected_path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
    {
        crate::config::remember_preferred_save_picker_dir(parent);
    }
    SYSTEM_QUIT_OPEN_SAVE_DIR_SUCCESS_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-load-save-profiles: applied selected save preview '{}' len={len} hash=0x{hash:016x} slot_mask=0x{mask:x}; staged active save unchanged until a foreign slot is selected",
        selected_log
    ));
    true
}

pub(crate) unsafe fn system_quit_init_menu_string_from_static_wide(
    out: usize,
    text: &'static [u16],
) -> bool {
    let Ok(ctor_addr) = game_rva(MENU_STRING_FROM_WIDE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve MenuString ctor rva 0x{MENU_STRING_FROM_WIDE_RVA:x}; cannot build static label"
        ));
        return false;
    };
    let ctor: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(ctor_addr) };
    unsafe { ctor(out, text.as_ptr() as usize) };
    true
}

pub(crate) unsafe fn system_quit_build_static_label_component(
    out: usize,
    label: &'static [u16],
    help: &'static [u16],
) -> bool {
    unsafe { std::ptr::write_bytes(out as *mut u8, 0, MENU_HELP_LABEL_SIZE) };
    let label_ok = unsafe { system_quit_init_menu_string_from_static_wide(out, label) };
    let help_ok = unsafe {
        system_quit_init_menu_string_from_static_wide(out + MENU_HELP_LABEL_HELP_OFFSET, help)
    };
    label_ok && help_ok
}

/// Fixed capacity (UTF-16 units) of a System>Quit row label / line-help / dialog-prompt buffer.
///
/// `CS::MenuString` stores the RAW pointer it is given and reads to the first NUL, so a zero-padded
/// fixed buffer is exactly as valid as an exact-length one -- and an over-long string fails at
/// COMPILE time here instead of losing its tail at runtime. It replaced eight hand-expanded
/// `[b'L' as u16, b'o' as u16, ...]` arrays whose lengths had to be counted by hand.
pub(crate) const SYSTEM_QUIT_ROW_TEXT_CAPACITY: usize = 96;

/// Widen an ASCII row string into a NUL-terminated UTF-16 buffer at compile time. Every result must
/// live in a `const`/`static` with process lifetime: `CS::MenuString` keeps the pointer, not a copy.
pub(crate) const fn system_quit_row_text(text: &[u8]) -> [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] {
    let mut out = [0_u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY];
    let mut idx = 0;
    while idx < text.len() {
        out[idx] = text[idx] as u16;
        idx += 1;
    }
    out
}

// THE TWO CLONED ROW LABELS, AND WHY THESE WORDS (2026-07-31).
//
// They used to read "Load Profile" and "Load Save Profiles", which a reviewer could not tell apart:
// "I'm not clear on the difference between Load Profile and Load Save Profiles. It looks like Load
// Save Profiles will load a character profile and then you have access to that character's saves.
// Does it make more sense to call this 'Load Character' or 'Load Character Profile'?"
//
// The guess was inverted -- it is the OTHER row that ends in a character list -- and that inversion
// is the evidence the words were wrong, not just similar. So each row is now named after what it
// takes as INPUT: a character out of the container already loaded, or a file off the disk.
//
// BOTH FIT, MEASURED RATHER THAN ASSUMED. The Quit-tab cell renders its label through
// `02_040_optionsetting.gfx` sprite 129 -> `Text_0` -> sprite 96 -> `DefineEditText` char 95, whose
// bounds are -40..7960 twips = 400px at a 24px `MenuFont_01`, single-line with wordwrap OFF -- an
// overflow would CLIP, not wrap, so it would silently eat the tail.
// `scripts/gfx_text_width.py --height-px 24 --box-px 400` sums that font's own advance table:
// "Load Character" 144.5px, "Load Character from File" 234.6px, against the native "Return to
// Desktop" at 172.8px which is known to render on this row. Nothing is near the edge, so no
// placement matrix and no row width was touched. The line-help field on the same tab (char 87,
// -40..22760 twips = 1140px, also 24px) has room to spare for the help strings below.
pub(crate) const SYSTEM_QUIT_LOAD_PROFILE_LABEL_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Load Character");

pub(crate) const SYSTEM_QUIT_LOAD_PROFILE_HELP_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Load another character from the save you are playing");

pub(crate) const SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Load Character from File");

pub(crate) const SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Browse for another save file and load a character from it (ER0000.sl2)");

// Seamless Co-op variant of the row help above: same text but naming `ER0000.co2`, the container
// ERSC actually reads/writes. Selected at row-build time so the row never advertises the save
// flavor the active mode ignores (matches the picker's mode-locked filter; user directive
// 2026-07-06).
pub(crate) const SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_CO2_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Browse for another save file and load a character from it (ER0000.co2)");

pub(crate) const SYSTEM_QUIT_SAVE_GAME_LABEL_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Save Game");

// The help says CHOOSE, because that is what the row now does: pressing it opens the destination
// list rather than asking a question. Promising "Save and return to playing the game" would have
// described the pre-2026-07-31 flow, where the row's first act was a yes/no box.
pub(crate) const SYSTEM_QUIT_SAVE_GAME_HELP_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Choose where to save, then return to playing");

// Native dialog id 110000's text. The product row press suppresses the native activation and opens
// the destination list, so this substitution should never be reached; it stays because a build
// where the suppression did not take would otherwise show the VANILLA quit prompt on a row labelled
// Save Game, which is worse than a stale-but-harmless sentence.
pub(crate) const SYSTEM_QUIT_SAVE_GAME_DIALOG_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Save and return to playing the game?");

pub(crate) unsafe fn wide_equals_ascii(ptr: usize, ascii: &[u8]) -> bool {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || ascii.is_empty() {
        return false;
    }
    for (idx, want) in ascii.iter().copied().enumerate() {
        let Some(unit) = (unsafe { safe_read_u16(ptr + idx * core::mem::size_of::<u16>()) }) else {
            return false;
        };
        if unit != want as u16 {
            return false;
        }
    }
    matches!(
        unsafe { safe_read_u16(ptr + ascii.len() * core::mem::size_of::<u16>()) },
        Some(0)
    )
}

pub(crate) unsafe extern "system" fn system_quit_save_game_get_and_format_hook(
    out: usize,
    getter: usize,
    text_id: i32,
    fmg_name: usize,
    abbrev: usize,
) -> usize {
    let replacement = if text_id == SYSTEM_QUIT_FIRST_ROW_MENU_TEXT_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRMT") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_LABEL_W.as_ptr() as usize)
    } else if text_id == SYSTEM_QUIT_FIRST_ROW_LINEHELP_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRHK") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_HELP_W.as_ptr() as usize)
    } else if text_id == SYSTEM_QUIT_SAVE_GAME_DIALOG_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRD") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_DIALOG_W.as_ptr() as usize)
    } else {
        None
    };
    if let Some(text_ptr) = replacement {
        match game_rva(MSG_REPOSITORY_FORMAT_RVA) {
            Ok(format_addr) => {
                let format_fn: unsafe extern "system" fn(usize, usize, u32, usize, usize) -> usize =
                    unsafe { std::mem::transmute(format_addr) };
                SYSTEM_QUIT_SAVE_GAME_TEXT_SUBSTITUTION_COUNT.fetch_add(1, Ordering::SeqCst);
                return unsafe { format_fn(out, text_ptr, text_id as u32, fmg_name, abbrev) };
            }
            Err(_) => append_autoload_debug(format_args!(
                "system-quit-save: failed to resolve MsgRepository::Format rva 0x{MSG_REPOSITORY_FORMAT_RVA:x}; forwarding id={text_id}"
            )),
        }
    }
    let orig = SYSTEM_QUIT_SAVE_GAME_GET_AND_FORMAT_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return out;
    }
    let original: unsafe extern "system" fn(usize, usize, i32, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(out, getter, text_id, fmg_name, abbrev) }
}

/// Native cancel-close of one menu window. `pub(crate)` because the save-flow tick closes the
/// destination browser through the same primitive the deferred IngameTop close uses.
pub(crate) unsafe fn system_quit_save_game_close_window(window: usize, label: &str) -> bool {
    if window < 0x10000 || window == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    let vt = unsafe { safe_read_usize(window) }.unwrap_or(0);
    if vt < 0x10000 {
        append_autoload_debug(format_args!(
            "system-quit-save: skip close {label}=0x{window:x}; invalid vt=0x{vt:x}"
        ));
        return false;
    }
    let Ok(close_addr) = game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve native close rva 0x{SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA:x}; cannot close {label}=0x{window:x}"
        ));
        return false;
    };
    let close_fn: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(close_addr) };
    unsafe { close_fn(window) };
    SYSTEM_QUIT_SAVE_GAME_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-save: native cancel-close {label}=0x{window:x} vt=0x{vt:x}"
    ));
    true
}

pub(crate) unsafe fn system_quit_save_game_request_save_only() {
    let Ok(request_save_addr) = game_rva(SYSTEM_QUIT_REQUEST_SAVE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve RequestSave rva 0x{SYSTEM_QUIT_REQUEST_SAVE_RVA:x}"
        ));
        return;
    };
    let request_save: unsafe extern "system" fn(u8) =
        unsafe { std::mem::transmute(request_save_addr) };
    unsafe { request_save(true as u8) };
    match game_rva(SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA) {
        Ok(profile_addr) => {
            let save_request_profile: unsafe extern "system" fn(u8) =
                unsafe { std::mem::transmute(profile_addr) };
            unsafe { save_request_profile(true as u8) };
        }
        Err(_) => append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve SaveRequest_Profile rva 0x{SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA:x}; RequestSave already issued"
        )),
    }
}

/// Fire the native save request pair FORCED: `throttled = false` for both calls.
///
/// The bool is PINNED (1.16.2 Ghidra decompile, 2026-07-28; see the RVA consts):
/// `RequestSave(true)` runs a 60-second throttle against `GameMan+0xb98` whose early
/// return sets NO flags, and `SaveRequest_Profile(true)` the same against `+0xb88`
/// (`SetSeconds(0x3c)`). Under boot-time global suppression the dispatchers still run
/// their commit tails for swallowed autosaves, so those throttle timestamps stay warm --
/// a throttled user press within 60 s of any swallowed autosave would silently set
/// nothing and strand the armed one-shot bypass token. The Save Game commit therefore
/// always fires with `false`. The quit-to-desktop sites deliberately keep
/// `system_quit_save_game_request_save_only` (true/true): under suppression those become
/// intentional no-op saves -- the Save Game row is the ONLY path that really writes.
pub(crate) unsafe fn system_quit_save_game_request_save_forced() {
    const FORCED_NOT_THROTTLED: u8 = false as u8;
    let Ok(request_save_addr) = game_rva(SYSTEM_QUIT_REQUEST_SAVE_RVA) else {
        append_autoload_debug(format_args!(
            "save-flow: failed to resolve RequestSave rva 0x{SYSTEM_QUIT_REQUEST_SAVE_RVA:x}; forced save NOT fired"
        ));
        return;
    };
    let request_save: unsafe extern "system" fn(u8) =
        unsafe { std::mem::transmute(request_save_addr) };
    unsafe { request_save(FORCED_NOT_THROTTLED) };
    match game_rva(SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA) {
        Ok(profile_addr) => {
            let save_request_profile: unsafe extern "system" fn(u8) =
                unsafe { std::mem::transmute(profile_addr) };
            unsafe { save_request_profile(FORCED_NOT_THROTTLED) };
        }
        Err(_) => append_autoload_debug(format_args!(
            "save-flow: failed to resolve SaveRequest_Profile rva 0x{SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA:x}; forced RequestSave already issued"
        )),
    }
}

/// Call one of the game's own save-request retractions after verifying its whole body.
///
/// Fails CLOSED through `save_flow_verify_rva`: an unresolvable address or a single drifted
/// byte skips the call and reports it. Not retracting costs CPU; calling unknown code costs
/// the process.
pub(crate) unsafe fn call_verified_retract(rva: u32, expected: &[u8], name: &str) -> bool {
    let Some(address) = save_flow_verify_rva(rva, expected, name) else {
        return false;
    };
    let retract: unsafe extern "system" fn() = unsafe { std::mem::transmute(address) };
    unsafe { retract() };
    true
}

/// Retract the two native save-request flags through the game's own setters.
///
/// `b72` / `b73` select which flags to clear -- the caller decides ownership; this only
/// performs it. Returns the pair of "actually cleared" results.
pub(crate) unsafe fn system_quit_save_request_retract(b72: bool, b73: bool) -> (bool, bool) {
    let cleared_b72 = b72
        && unsafe {
            call_verified_retract(
                SAVE_REQUEST_RETRACT_B72_RVA,
                SAVE_REQUEST_RETRACT_B72_SIG,
                "b72",
            )
        };
    let cleared_b73 = b73
        && unsafe {
            call_verified_retract(
                SAVE_REQUEST_RETRACT_B73_RVA,
                SAVE_REQUEST_RETRACT_B73_SIG,
                "b73",
            )
        };
    (cleared_b72, cleared_b73)
}

/// Run the proven Save Game close-all sequence and hand the flow to `save_flow_tick`.
///
/// `commit` selects the destination stage: `true` = stage 6 CLOSING_COMMIT (the tick fires
/// the forced save request once the menus are closed and the RAM gates are green), `false` =
/// stage 5 CLOSING_ABORT (the user declined; the tick returns to the world having written
/// nothing).
///
/// CLOSE-THEN-FIRE (save-game-flow WP1, 2026-07-28): the save request is never fired here.
/// Firing while menus are open is a dispatch-split hazard -- `ShouldSave` (the b72 lane)
/// requires `!CanShowSaveMenu()` (`CSMenuMan+0x13c == 0`) but the b73 gate `FUN_140679370`
/// does NOT, so an open-menu fire lets the b73-only lane dispatch a system-only submit first,
/// that submit consumes the one-shot suppression-bypass token, and the later char-slot submit
/// is swallowed. Staging the commit and firing at stage 7 produces a single combined
/// `b72 && b73` -> `FUN_14067b940` -> one `FUN_140e6ef60` submit -> one enqueue -> one token.
///
/// WP1/WP2 commit plan: overwrite the loaded save (WP3 adds a destination target).
pub(crate) unsafe fn system_quit_save_game_close_menus(
    dialog: usize,
    source: &str,
    commit: bool,
) -> bool {
    if dialog < 0x10000 || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "system-quit-save: {source} abort -- dialog=0x{dialog:x} is not heap-like"
        ));
        return false;
    }
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    // Stage the outcome BEFORE the close sequence so the save-flow tick owns the flow
    // from the next game-task frame onward.
    SAVE_FLOW_DIALOG.store(dialog, Ordering::SeqCst);
    SAVE_FLOW_STAGE_TICKS.store(0, Ordering::SeqCst);
    SAVE_FLOW_STAGE.store(
        if commit {
            SAVE_FLOW_STAGE_CLOSING_COMMIT
        } else {
            SAVE_FLOW_STAGE_CLOSING_ABORT
        },
        Ordering::SeqCst,
    );
    if commit {
        SYSTEM_QUIT_SAVE_GAME_CONFIRM_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    // `dialog` is the System/Quit tab's PropertyEditDialog, not a `MenuWindow`; calling the
    // MenuWindow cancel-close primitive on it dispatches the wrong vfunc. Close the owning menu
    // windows only, matching the Escape/back stack semantics instead of treating the row dialog as a
    // window.
    let closed_dialog = false;
    let closed_option = if option != 0 {
        unsafe { system_quit_save_game_close_window(option, "option_window") }
    } else {
        false
    };
    // Do not close the root IngameTop in the same call stack: runtime proof showed that closing the
    // full root stack immediately after the row action terminates the process. The native Escape
    // flow unwinds from the active submenu first, so close OptionSetting now and schedule IngameTop
    // for a later game-task tick.
    let closed_top = false;
    if top != 0 && top != option {
        SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_WINDOW.store(top, Ordering::SeqCst);
        SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.store(2, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "system-quit-save: {source} -> staged {} + native menu-window close-all dialog=0x{dialog:x} option=0x{option:x} top=0x{top:x} closed_dialog={closed_dialog} closed_option={closed_option} closed_top={closed_top}",
        if commit {
            "CLOSING_COMMIT (close-then-fire)"
        } else {
            "CLOSING_ABORT (nothing will be written)"
        }
    ));
    closed_option || closed_top
}

/// Enter the Save Game flow from the row press: STRAIGHT TO THE DESTINATION LIST.
///
/// Captures the System/Quit dialog the whole flow is anchored on, then hands the browser open to
/// the menu pump via `SAVE_DEST_OPEN_PICKER_PENDING` and parks in stage 3. Nothing is asked here
/// and nothing is written here.
///
/// WHY THE PUMP AND NOT AN INLINE OPEN. Opening the destination browser stages ProfileSummary row
/// records and submits an `05_010` MenuJob; `system_quit_menu_window_run_post` is the context that
/// was runtime-proven to own that submit (2026-07-29, three consecutive commits), and it is the
/// same hand-off the OS surface needs in order to block inside comdlg32 off the row-action stack.
/// The row press therefore stages the request rather than performing it, and stage 3's
/// `SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS` bounds a pump that never picks it up.
///
/// THERE IS NO "DEGRADE TO AN IMMEDIATE COMMIT" PATH ANY MORE, and its removal is a safety fix
/// rather than a simplification. It existed because the old flow could not ask its first question
/// without the MessageBoxBuilder recipe, so a build that failed the prologue check wrote to the
/// loaded save with NO confirm at all. Opening a list needs no message box, so a broken recipe now
/// costs only the overwrite confirm -- and an unconfirmable overwrite is REFUSED at the pick
/// (`save_dest_handle_picked_target`), never performed silently. A free destination name still
/// commits, because it never needed a confirm in the first place.
pub(crate) unsafe fn system_quit_save_game_start_flow(dialog: usize) -> bool {
    if dialog < 0x10000 || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "save-flow: row press abort -- dialog=0x{dialog:x} is not heap-like"
        ));
        return false;
    }
    save_flow_box_clear();
    // Defensive: no destination state may survive from an earlier flow (a stale target would send
    // this save to the wrong file).
    save_dest_reset("save game row press");
    SAVE_FLOW_DIALOG.store(dialog, Ordering::SeqCst);
    SAVE_FLOW_STAGE_TICKS.store(0, Ordering::SeqCst);
    // The overwrite confirm is only answerable if the MessageBoxDialog BUILDER hook is live --
    // that detour is what captures the dialog pointer the stage machine polls. It is normally
    // installed at boot (`online_disable_enabled()` path in the game task), but make sure:
    // this call is idempotent, and the row press is the menu thread, i.e. the one context in
    // which no other thread can be executing the builder while MinHook patches it.
    install_auto_accept_hook();
    // Same reasoning for the answer observer: `CS::MenuJob::EmitResult` is what tells us WHICH
    // button the user pressed on the branch where the dialog never stores its result. Install
    // is idempotent; if it is not live the poll falls back to `dialog+0x1e8` and reports
    // UNDECIDABLE rather than guessing, so log the state here where the run can see it.
    install_menu_job_emit_result_hook();
    let capture_live = MSGBOX_BUILDER_ORIG.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET;
    // One press = one bypass arm = one commit. Counting presses is what makes
    // `oracle_save_bypass_allowed_total = 2` readable as "two presses" instead of "a double-arm
    // bug"; without it the two are indistinguishable from telemetry alone.
    let press = SAVE_FLOW_ROW_PRESS_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    append_autoload_debug(format_args!(
        "save-flow: row press #{press} dialog=0x{dialog:x} -> opening the destination list (no confirm is asked here); overwrite_confirm_available={} builder_capture_live={capture_live} emit_observer_installed={}",
        save_flow_box_recipe_available(),
        MENU_JOB_EMIT_RESULT_INSTALLED.load(Ordering::SeqCst)
    ));
    if !save_flow_box_recipe_available() || !capture_live {
        // NOT fatal to the press, and deliberately so: the list needs no message box. Only a pick
        // that would clobber an existing file needs one, and that pick is refused rather than
        // written blind. Say it once here so a run can attribute a later refusal.
        append_autoload_debug(format_args!(
            "save-flow: the overwrite confirm cannot be built on this build (recipe_ok={} builder_capture_live={capture_live}) -- the destination list still opens; picking an EXISTING file will be refused, picking a free name still commits",
            save_flow_box_recipe_available()
        ));
    }
    // Menu-pump owns the open (see this function's docs). Stage 3 waits for it and bounds it.
    SAVE_DEST_OPEN_PICKER_PENDING.store(1, Ordering::SeqCst);
    SAVE_FLOW_STAGE.store(SAVE_FLOW_STAGE_DEST_BROWSE, Ordering::SeqCst);
    true
}

pub(crate) unsafe fn system_quit_save_game_deferred_close_tick() {
    let frames = SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.load(Ordering::SeqCst);
    if frames == 0 {
        return;
    }
    if frames > 1 {
        SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.store(0, Ordering::SeqCst);
    let top = SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_WINDOW.swap(0, Ordering::SeqCst);
    if top != 0 {
        let closed =
            unsafe { system_quit_save_game_close_window(top, "deferred_ingame_top_window") };
        append_autoload_debug(format_args!(
            "system-quit-save: deferred IngameTop close top=0x{top:x} closed={closed}"
        ));
    }
}

pub(crate) unsafe extern "system" fn system_quit_save_game_return_title_request_hook() {
    let dialog = SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.swap(0, Ordering::SeqCst);
    if dialog >= 0x10000 && callstack_contains_game_rva(0x7a3000, 0x7a4000) {
        // Dormant safety net (the product row path clears the arming latch). It keeps the
        // WP1 semantics -- close-then-fire straight to the commit, no confirm chain.
        unsafe { system_quit_save_game_close_menus(dialog, "legacy_confirm", true) };
        append_autoload_debug(format_args!(
            "system-quit-save: legacy confirmation path suppressed native return-title request dialog=0x{dialog:x}"
        ));
        return;
    }
    if dialog != 0 {
        SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.store(dialog, Ordering::SeqCst);
    }
    let orig = SYSTEM_QUIT_SAVE_GAME_RETURN_TITLE_REQUEST_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-save: return-title request trampoline unset and no active Save Game dialog; return"
        ));
        return;
    }
    let original: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
    unsafe { original() };
}

pub(crate) unsafe extern "system" fn system_quit_duplicate_add_cancel_button_hook(
    dialog: usize,
    label: usize,
    action_fn: usize,
    enabled_fn: usize,
    keyguide_fn: usize,
) -> usize {
    let orig = SYSTEM_QUIT_DUPLICATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: original AddCancelButton trampoline is unset -- fail-open return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let first_row_call = callstack_contains_game_rva(
        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA
            .saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),
        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA + SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES,
    );
    let second_row_call = callstack_contains_game_rva(
        SYSTEM_QUIT_SECOND_ROW_TARGET_RETURN_RVA
            .saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),
        SYSTEM_QUIT_SECOND_ROW_TARGET_RETURN_RVA + SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES,
    );
    let before =
        unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
            .unwrap_or(0);
    let ret = unsafe { original(dialog, label, action_fn, enabled_fn, keyguide_fn) };
    if !(first_row_call || second_row_call) {
        return ret;
    }

    // OptionSetting constructs/lazily rebuilds hidden tab panes while another tab is visible. Mutate
    // only the Quit tab's own dialog; never write rows into the active non-Quit pane.
    let active_tab = OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst);
    let active_dialog = OPTIONSETTING_CURRENT_DIALOG.load(Ordering::SeqCst);
    let actively_shown = OPTIONSETTING_ACTIVELY_SHOWN.load(Ordering::SeqCst) != 0;
    if actively_shown && active_tab != OPTIONSETTING_QUIT_TAB_INDEX && active_dialog == dialog {
        let skip_n = SYSTEM_QUIT_DUPLICATE_COUNT.load(Ordering::SeqCst);
        if skip_n < 16 {
            append_autoload_debug(format_args!(
                "system-quit-dup: matched Quit Game AddCancelButton but target is active non-Quit tab={active_tab} dialog=0x{dialog:x}; skipping row routing so active tab stays vanilla"
            ));
        }
        return ret;
    }

    let after_native =
        unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
            .unwrap_or(0);
    let properties = dialog + PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET;
    let aligned_properties = (properties + 0x7) & !0x7;
    let mut after_final = after_native;
    if after_native > before {
        let native_row_index = after_native.saturating_sub(1);
        let native_row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(native_row_index);
        let native_controller =
            unsafe { safe_read_usize(native_row + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
        let native_action = if native_controller != 0 {
            unsafe {
                safe_read_usize(
                    native_controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET,
                )
            }
            .unwrap_or(0)
        } else {
            0
        };
        if native_action != 0 && first_row_call {
            // The FIRST native row starts a fresh row table: this is the Quit tab building its
            // dialog, and every index/controller from an earlier build is now stale (a heap address
            // may even have been reused by this build).
            system_quit_row_table_reset(dialog);
            SYSTEM_QUIT_NATIVE_SAVE_GAME_ACTION_LAST_OBJECT.store(native_action, Ordering::SeqCst);
            SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT
                .store(native_controller, Ordering::SeqCst);
            system_quit_row_table_record_index(QuitRow::SaveGame, native_row_index);
            append_autoload_debug(format_args!(
                "system-quit-dup: captured native first Quit row index={native_row_index} controller=0x{native_controller:x} action_alias=0x{native_action:x} (== controller+0x{PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET:x}); routing this in-place button to Save Game"
            ));
        } else if native_action != 0 && second_row_call {
            SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_ACTION_LAST_OBJECT
                .store(native_action, Ordering::SeqCst);
            SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_CONTROLLER_LAST_OBJECT
                .store(native_controller, Ordering::SeqCst);
            system_quit_row_table_record_index(QuitRow::ReturnToDesktop, native_row_index);
            append_autoload_debug(format_args!(
                "system-quit-dup: captured native second Quit row index={native_row_index} controller=0x{native_controller:x} action_alias=0x{native_action:x}; Return to Desktop is now identified by ROW (index + live label), never by this pointer"
            ));
        }
    }

    if second_row_call {
        let Ok(label_dtor_addr) = game_rva(MENU_HELP_LABEL_DTOR_RVA) else {
            append_autoload_debug(format_args!(
                "system-quit-dup: failed to resolve MenuHelpLabelComponent dtor rva 0x{MENU_HELP_LABEL_DTOR_RVA:x}; cannot add Load Profile/Load Save Profiles rows"
            ));
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE.store(before, Ordering::SeqCst);
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER.store(after_native, Ordering::SeqCst);
            return ret;
        };
        let label_dtor: unsafe extern "system" fn(usize) =
            unsafe { std::mem::transmute(label_dtor_addr) };
        let mut load_label_storage =
            std::mem::MaybeUninit::<SystemQuitMenuHelpLabelScratch>::uninit();
        let load_label = load_label_storage.as_mut_ptr() as usize;
        let load_label_ok = unsafe {
            system_quit_build_static_label_component(
                load_label,
                &SYSTEM_QUIT_LOAD_PROFILE_LABEL_W,
                &SYSTEM_QUIT_LOAD_PROFILE_HELP_W,
            )
        };
        let (load_ret, load_row, load_controller, load_action) = if load_label_ok {
            let r = unsafe { original(dialog, load_label, action_fn, enabled_fn, keyguide_fn) };
            unsafe { label_dtor(load_label) };
            after_final = unsafe {
                safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET)
            }
            .unwrap_or(after_native);
            let row_index = after_final.saturating_sub(1);
            let row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(row_index);
            let controller =
                unsafe { safe_read_usize(row + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
            let action = if controller != 0 {
                unsafe {
                    safe_read_usize(
                        controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET,
                    )
                }
                .unwrap_or(0)
            } else {
                0
            };
            if action != 0 {
                SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT.store(action, Ordering::SeqCst);
            }
            if controller != 0 {
                SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT.store(controller, Ordering::SeqCst);
                system_quit_row_table_record_index(QuitRow::LoadProfile, row_index);
            }
            (r, row, controller, action)
        } else {
            (0, 0, 0, 0)
        };

        let mut open_label_storage =
            std::mem::MaybeUninit::<SystemQuitMenuHelpLabelScratch>::uninit();
        let open_label = open_label_storage.as_mut_ptr() as usize;
        let open_label_ok = unsafe {
            system_quit_build_static_label_component(
                open_label,
                &SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W,
                if crate::telemetry::seamless_coop_loaded() {
                    &SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_CO2_W
                } else {
                    &SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_W
                },
            )
        };
        let (open_ret, open_row, open_controller, open_action) = if open_label_ok {
            let r = unsafe { original(dialog, open_label, action_fn, enabled_fn, keyguide_fn) };
            unsafe { label_dtor(open_label) };
            after_final = unsafe {
                safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET)
            }
            .unwrap_or(after_final);
            let row_index = after_final.saturating_sub(1);
            let row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(row_index);
            let controller =
                unsafe { safe_read_usize(row + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
            let action = if controller != 0 {
                unsafe {
                    safe_read_usize(
                        controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET,
                    )
                }
                .unwrap_or(0)
            } else {
                0
            };
            if action != 0 {
                SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT.store(action, Ordering::SeqCst);
            }
            if controller != 0 {
                SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT
                    .store(controller, Ordering::SeqCst);
                system_quit_row_table_record_index(QuitRow::LoadSaveProfiles, row_index);
            }
            (r, row, controller, action)
        } else {
            (0, 0, 0, 0)
        };
        if load_label_ok || open_label_ok {
            SYSTEM_QUIT_DUPLICATE_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        // Raise the list widget's item count through the NATIVE setter, not by poking the field.
        // `GridControl::SetItemCount` writes `+0xd0` AND recomputes the scroll/page row count on the
        // embedded scroll control at `+0x1a8` -- exactly what the native rebuild `FUN_140975040`
        // calls. A raw field write left that scroll control still describing the two-row list, which
        // is the state the vertical movement clamp reads.
        let prior_bound =
            unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
        let new_bound = (after_final.min(i32::MAX as usize)) as i32;
        let set_item_count = game_rva(GRID_CONTROL_SET_ITEM_COUNT_RVA).ok();
        if new_bound > prior_bound {
            match set_item_count {
                Some(addr) => {
                    let set_count: unsafe extern "system" fn(usize, u32) =
                        unsafe { std::mem::transmute(addr) };
                    unsafe { set_count(dialog + DIALOG_GRID_CONTROL_A38_OFFSET, new_bound as u32) };
                }
                None => append_autoload_debug(format_args!(
                    "system-quit-dup: failed to resolve GridControl::SetItemCount rva 0x{GRID_CONTROL_SET_ITEM_COUNT_RVA:x}; the added rows stay outside the list cursor's range"
                )),
            }
        }
        let bound_after =
            unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
        unsafe { system_quit_record_grid_geometry(dialog) };
        // The row TABLE is the identity from here on: index + live label per row. Log it, and log the
        // label actually readable at each captured index so a run shows the table agreeing with
        // memory rather than being trusted.
        let table = SYSTEM_QUIT_ROW_TABLE_ROWS
            .map(|row| {
                let index = system_quit_row_table_index(row);
                let label = unsafe { system_quit_row_label_at(dialog, index) };
                format!("{}=#{index}:{label:?}", row.label())
            })
            .join(" ");
        append_autoload_debug(format_args!(
            "system-quit-dup: added native GameEnd rows Load Profile + Load Save Profiles dialog=0x{dialog:x} count {before}->{after_native}->{after_final} cursor_bound {prior_bound}->{bound_after} ret=0x{ret:x} load_ok={load_label_ok} load_ret=0x{load_ret:x} load_row=0x{load_row:x} load_controller=0x{load_controller:x} load_action_alias=0x{load_action:x} open_ok={open_label_ok} open_ret=0x{open_ret:x} open_row=0x{open_row:x} open_controller=0x{open_controller:x} open_action_alias=0x{open_action:x}; row table [{table}]"
        ));
    }

    SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE.store(before, Ordering::SeqCst);
    SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER.store(after_final, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: routed native Quit rows dialog=0x{dialog:x} first_row_call={first_row_call} second_row_call={second_row_call} count {before}->{after_native}->{after_final}; native GameEnd GFx component preserved"
    ));
    ret
}

/// Scaleform handler CONSTRUCTOR hook (`FUN_1411a8890`, deobf 0x1411a8870). rcx = the object being
/// constructed (the 0x58 handler embedded at container+0x40), rdx = parent. Records the object as
/// live, then forwards to the original ctor (which returns the object pointer). Read-only w.r.t.
/// game state; only maintains our live-set. See SCALEFORM_HANDLER_LIVE.
pub(crate) unsafe extern "system" fn scaleform_handler_ctor_hook(
    obj: usize,
    parent: usize,
) -> usize {
    let orig = SCALEFORM_HANDLER_CTOR_ORIG.load(Ordering::SeqCst);
    SCALEFORM_HANDLER_CTORS.fetch_add(1, Ordering::SeqCst);
    if obj != 0
        && let Ok(mut live) = SCALEFORM_HANDLER_LIVE.lock()
    {
        // Cap guard: if a genuine leak fills the table, stop growing (drop tracking of the
        // oldest) so the probe can't OOM -- the double-free detection still works for recent objs.
        if live.len() >= SCALEFORM_HANDLER_LIVE_CAP {
            live.remove(0);
        }
        live.push(obj);
    }
    let _ = parent;
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return obj;
    }
    let f: unsafe extern "system" fn(usize, usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { f(obj, parent) }
}

/// Scaleform handler inner DESTRUCTOR hook (`FUN_1411a8920`, deobf 0x1411a8900). rcx = the object.
/// If the object is in our live-set -> a normal teardown: remove it and forward to the original.
/// If it is NOT live -> a DOUBLE-FREE (the repeated-switch ProfileSelect UAF): the original would
/// walk this object's now-garbage intrusive list and crash. Log it and RETURN WITHOUT forwarding,
/// so the freed list is never dereferenced. Safe: an already-destructed object needs no second
/// teardown. This both names the bug (counter + last-obj oracle + debug line) and stops the crash.
pub(crate) unsafe extern "system" fn scaleform_handler_dtor_hook(obj: usize) {
    let orig = SCALEFORM_HANDLER_DTOR_ORIG.load(Ordering::SeqCst);
    SCALEFORM_HANDLER_DTORS.fetch_add(1, Ordering::SeqCst);
    let live = if obj == 0 {
        false
    } else if let Ok(mut set) = SCALEFORM_HANDLER_LIVE.lock() {
        if let Some(pos) = set.iter().rposition(|&a| a == obj) {
            set.swap_remove(pos);
            true
        } else {
            false
        }
    } else {
        // Lock poisoned/unavailable: fail SAFE toward forwarding (treat as live) so we never skip a
        // legitimate destructor on a lock hiccup -- the crash is rarer than the lock being fine.
        true
    };
    if !live {
        let n = SCALEFORM_HANDLER_DOUBLE_FREES.fetch_add(1, Ordering::SeqCst) + 1;
        SCALEFORM_HANDLER_LAST_DOUBLE_FREE_OBJ.store(obj, Ordering::SeqCst);
        if n <= 32 {
            let parent = unsafe { safe_read_usize(obj + 0x18) }.unwrap_or(0);
            let list_head = unsafe { safe_read_usize(obj + 0x28) }.unwrap_or(0);
            let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            append_crash_log(format_args!(
                "scaleform-handler-guard: DOUBLE-FREE #{n} of handler obj=0x{obj:x} container=0x{:x} parent(+0x18)=0x{parent:x} list_head(+0x28)=0x{list_head:x} quickload_phase={phase} -- SKIPPED inner dtor (would have walked freed list) to prevent the ProfileSelect UAF crash",
                obj.wrapping_sub(0x40)
            ));
        }
        return;
    }
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return;
    }
    let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
    unsafe { f(obj) };
}
