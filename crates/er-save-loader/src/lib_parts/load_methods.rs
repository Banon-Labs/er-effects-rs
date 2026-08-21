
/// A minimal title-accept fallback plan expressed in safe logical input terms.
/// Callers must choose a backend; this crate does not move the host mouse or
/// require the game window to be focused.
pub fn title_accept_fallback_sequence(
    config: SafeInputConfig,
) -> Result<Vec<SafeInputAction>, SafeInputError> {
    Ok(vec![SafeInputAction::tap(
        SafeButton::Confirm,
        TITLE_ACCEPT_CONFIRM_FRAMES,
        config,
    )?])
}

unsafe fn request_direct_menu_wrapper<G, F>(
    game_man: &mut G,
    module_base: usize,
    slot: i32,
    attempt: u64,
    debug: &mut F,
) -> Result<bool, String>
where
    G: GameManSaveAccess,
    F: FnMut(String),
{
    // The menu wrapper's state pointer is stable across collected title-menu
    // traces. Calling the native wrapper preserves its task-state write at
    // 0x1407a91e0 instead of calling save_write_entry_0b in isolation.
    type SetSaveSlot = unsafe extern "system" fn(i32);
    type RequestSave = unsafe extern "system" fn(u8);
    type SaveRequestProfile = unsafe extern "system" fn(u8);
    type MenuOtherLoadWrapper =
        unsafe extern "system" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void;

    if game_man.save_state() != IDLE_SAVE_STATE {
        debug(format!(
            "attempt {attempt}: menu wrapper request is in flight (state={})",
            game_man.save_state()
        ));
        return Ok(true);
    }

    if !unsafe { save_buffer_allocator_ready(module_base)? } {
        debug(format!(
            "attempt {attempt}: waiting for save buffer allocator before menu wrapper"
        ));
        return Ok(false);
    }

    let set_save_slot: SetSaveSlot =
        unsafe { std::mem::transmute(game_rva(module_base, SET_SAVE_SLOT_RVA)?) };
    let request_save: RequestSave =
        unsafe { std::mem::transmute(game_rva(module_base, REQUEST_SAVE_RVA)?) };
    let save_request_profile: SaveRequestProfile =
        unsafe { std::mem::transmute(game_rva(module_base, SAVE_REQUEST_PROFILE_RVA)?) };
    let menu_other_load_wrapper: MenuOtherLoadWrapper =
        unsafe { std::mem::transmute(game_rva(module_base, MENU_OTHER_LOAD_WRAPPER_RVA)?) };

    unsafe { set_save_slot(slot) };
    unsafe { request_save(REQUEST_SAVE_ENABLED) };
    unsafe { save_request_profile(SAVE_REQUEST_PROFILE_ENABLED) };
    let state_ptr = MENU_OTHER_LOAD_STATE_PTR as *mut std::ffi::c_void;
    let ret = unsafe { menu_other_load_wrapper(state_ptr) };
    let save_state = game_man.save_state();
    debug(format!(
        "attempt {attempt}: direct menu_other_load_wrapper returned {ret:p} save_state={save_state}"
    ));
    Ok(save_state != IDLE_SAVE_STATE)
}

/// Which native load primitives [`request_direct_menu_load`] should invoke for a given
/// [`SaveLoadMethod`]. These four flags were positional `bool` arguments; grouping them keeps the
/// method -> primitive mapping in one place and out of the call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirectMenuLoadCalls {
    /// Call `save_write_entry_0b` (the map-load dispatch entry).
    map_load: bool,
    /// Call `combined_save_write` (the combined load dispatch).
    combined_load: bool,
    /// Set the native title-bootstrap load flag before queuing the continue flags.
    title_bootstrap_marker: bool,
    /// Pump the native save/load state machine instead of returning a completed request.
    save_load_pump: bool,
}

impl DirectMenuLoadCalls {
    fn for_method(method: SaveLoadMethod) -> Self {
        Self {
            map_load: matches!(
                method,
                SaveLoadMethod::DirectMapLoad | SaveLoadMethod::DirectCombinedLoad
            ),
            combined_load: matches!(
                method,
                SaveLoadMethod::DirectCombinedLoad
                    | SaveLoadMethod::DirectCombinedOnly
                    | SaveLoadMethod::DirectBootstrapCombined
                    | SaveLoadMethod::DirectBootstrapPump
            ),
            title_bootstrap_marker: matches!(
                method,
                SaveLoadMethod::DirectBootstrapCombined | SaveLoadMethod::DirectBootstrapPump
            ),
            save_load_pump: matches!(method, SaveLoadMethod::DirectBootstrapPump),
        }
    }
}

unsafe fn request_direct_menu_load<G, F>(
    game_man: &mut G,
    module_base: usize,
    slot: i32,
    attempt: u64,
    calls: DirectMenuLoadCalls,
    debug: &mut F,
) -> Result<bool, String>
where
    G: GameManSaveAccess,
    F: FnMut(String),
{
    // Runtime/static RE shows the real Continue path is not a direct call to
    // the load primitives. Menu code queues GameMan flags, and the MoveMapList
    // task consumes those flags at safe scheduler points.
    // 0x67bc10 WRITES save-container entry 0xb (allocates 0x240010, DLMemoryOutputStream,
// submits through the write lane, sets GameMan->saveState = 1). It does not load.
// Renamed 2026-08-01.
const SAVE_DISPATCH_ENTRY0B_RVA: u32 = 0x0067bc10;

    type SetSaveSlot = unsafe extern "system" fn(i32);
    type RequestSave = unsafe extern "system" fn(u8);
    type SaveRequestProfile = unsafe extern "system" fn(u8);
    type SaveWriteEntry0b = unsafe extern "system" fn() -> u8;
    type CombinedSaveWrite = unsafe extern "system" fn(i32, u8, u8) -> u8;
    type MarkTitleBootstrap = unsafe extern "system" fn();
    type SaveLoadPumpDefault = unsafe extern "system" fn();

    if calls.save_load_pump && game_man.save_state() != IDLE_SAVE_STATE {
        let save_load_pump_default: SaveLoadPumpDefault =
            unsafe { std::mem::transmute(game_rva(module_base, SAVE_LOAD_PUMP_DEFAULT_RVA)?) };
        unsafe { save_load_pump_default() };
        debug(format!(
            "attempt {attempt}: pumped save/load state (state={})",
            game_man.save_state()
        ));
        return Ok(false);
    }

    if game_man.save_state() != IDLE_SAVE_STATE {
        debug(format!(
            "attempt {attempt}: waiting for save_state 0 before queuing continue flags (state={})",
            game_man.save_state()
        ));
        return Ok(false);
    }

    if !unsafe { save_buffer_allocator_ready(module_base)? } {
        debug(format!(
            "attempt {attempt}: waiting for save buffer allocator before queuing continue flags"
        ));
        return Ok(false);
    }

    let set_save_slot: SetSaveSlot =
        unsafe { std::mem::transmute(game_rva(module_base, SET_SAVE_SLOT_RVA)?) };
    let request_save: RequestSave =
        unsafe { std::mem::transmute(game_rva(module_base, REQUEST_SAVE_RVA)?) };
    let save_request_profile: SaveRequestProfile =
        unsafe { std::mem::transmute(game_rva(module_base, SAVE_REQUEST_PROFILE_RVA)?) };

    if calls.title_bootstrap_marker {
        let mark_title_bootstrap: MarkTitleBootstrap =
            unsafe { std::mem::transmute(game_rva(module_base, MARK_TITLE_BOOTSTRAP_RVA)?) };
        unsafe { mark_title_bootstrap() };
        debug(format!(
            "attempt {attempt}: marked native title bootstrap load flag"
        ));
    }

    debug(format!(
        "attempt {attempt}: queuing traced continue flags for slot {slot}"
    ));
    unsafe { set_save_slot(slot) };
    unsafe { request_save(REQUEST_SAVE_ENABLED) };
    unsafe { save_request_profile(SAVE_REQUEST_PROFILE_ENABLED) };
    if calls.combined_load && !calls.map_load {
        let combined_save_write: CombinedSaveWrite =
            unsafe { std::mem::transmute(game_rva(module_base, SAVE_DISPATCH_COMBINED_RVA)?) };
        let combined_ret = unsafe {
            combined_save_write(
                CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                LOAD_ARG_FALSE,
                LOAD_ARG_TRUE,
            )
        };
        debug(format!(
            "attempt {attempt}: direct combined_save_write returned {combined_ret}"
        ));
        if calls.save_load_pump {
            return Ok(false);
        }
        return Ok(combined_ret != MAP_LOAD_FALSE_RETURN);
    }
    if calls.map_load {
        let save_write_entry_0b: SaveWriteEntry0b =
            unsafe { std::mem::transmute(game_rva(module_base, SAVE_DISPATCH_ENTRY0B_RVA)?) };
        let ret = unsafe { save_write_entry_0b() };
        debug(format!("attempt {attempt}: direct save_write_entry_0b returned {ret}"));
        if ret == MAP_LOAD_FALSE_RETURN {
            return Ok(false);
        }
        if calls.combined_load {
            let combined_save_write: CombinedSaveWrite =
                unsafe { std::mem::transmute(game_rva(module_base, SAVE_DISPATCH_COMBINED_RVA)?) };
            let combined_ret = unsafe {
                combined_save_write(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    LOAD_ARG_TRUE,
                )
            };
            debug(format!(
                "attempt {attempt}: direct combined_save_write returned {combined_ret}"
            ));
            if calls.save_load_pump {
                return Ok(false);
            }
            return Ok(combined_ret != MAP_LOAD_FALSE_RETURN);
        }
        return Ok(true);
    }
    Ok(true)
}

unsafe fn request_direct_trace_sequence<G, F>(
    game_man: &mut G,
    module_base: usize,
    slot: i32,
    attempt: u64,
    phase: &mut u8,
    debug: &mut F,
) -> Result<bool, String>
where
    G: GameManSaveAccess,
    F: FnMut(String),
{
    // WARNING (corrected 2026-08-01 against the 1.16.2 Ghidra dump): 0x67b750 is
    // `GameMan::WriteSaveToSlot(slot, flushToDisk, blankSlot)` -- it WRITES a save, it does
    // not read one. It was named `CONTINUE_LOAD_RVA` here and in er-effects-rs, but
    // `SAVE_DISPATCH_CHAR_RVA` in er-save-suppress (which hooks it precisely to SWALLOW
    // saves); er-save-suppress had it right. Decompile: bails on `CanShowSaveMenu()`,
    // requires `saveState == 0`, resolves slot -1 to `GameMan->saveSlot`, allocates
    // 0x280000, calls `MarkProfileIndexAsUsed`, serializes game state into the buffer via
    // 0x14067dc00 (`DLMemoryOutputStream` + `Write`), stamps `DLDateTime`, writes the buffer
    // through the IO device (0xe6e060 -> 0x140e6ec70), then sets `saveState = 1`.
    //
    // CONSEQUENCE, NOT YET CHANGED: the DIRECT_SEQUENCE_PHASE_CONTINUE arm below invokes it
    // as `(-1, 0, 0)`, which resolves to the ACTIVE save slot and takes the serialize branch
    // -- i.e. this "load method" writes current game state over the player's active slot.
    // `blankSlot == 1` would instead write a zeroed 0x280000 buffer (an erased slot); this
    // call passes 0, so it is a state overwrite rather than an erase. Left in place because
    // removing it is a behavior change that needs a runtime proof run, not a rename pass.
    // `DirectTraceSequence` is opt-in (default is `SaveLoadMethod::SaveRequested`), selected
    // only via config/request `method = direct_trace_sequence`. Tracked in bd.
    const SAVE_WRITE_TO_SLOT_RVA: u32 = 0x0067b750;

    type SetSaveSlot = unsafe extern "system" fn(i32);
    type RequestSave = unsafe extern "system" fn(u8);
    type SaveRequestProfile = unsafe extern "system" fn(u8);
    type CombinedSaveWrite = unsafe extern "system" fn(i32, u8, u8) -> u8;
    type SaveWriteToSlot = unsafe extern "system" fn(i32, u8, u8) -> u8;
    type MarkTitleBootstrap = unsafe extern "system" fn();
    type SaveLoadPumpDefault = unsafe extern "system" fn();

    if game_man.save_state() != IDLE_SAVE_STATE {
        let save_load_pump_default: SaveLoadPumpDefault =
            unsafe { std::mem::transmute(game_rva(module_base, SAVE_LOAD_PUMP_DEFAULT_RVA)?) };
        unsafe { save_load_pump_default() };
        debug(format!(
            "attempt {attempt}: direct trace phase {phase} pumped save/load state (state={})",
            game_man.save_state(),
            phase = *phase,
        ));
        return Ok(false);
    }

    if !unsafe { save_buffer_allocator_ready(module_base)? } {
        debug(format!(
            "attempt {attempt}: waiting for save buffer allocator before direct trace sequence"
        ));
        return Ok(false);
    }

    let set_save_slot: SetSaveSlot =
        unsafe { std::mem::transmute(game_rva(module_base, SET_SAVE_SLOT_RVA)?) };
    let request_save: RequestSave =
        unsafe { std::mem::transmute(game_rva(module_base, REQUEST_SAVE_RVA)?) };
    let save_request_profile: SaveRequestProfile =
        unsafe { std::mem::transmute(game_rva(module_base, SAVE_REQUEST_PROFILE_RVA)?) };
    let combined_save_write: CombinedSaveWrite =
        unsafe { std::mem::transmute(game_rva(module_base, SAVE_DISPATCH_COMBINED_RVA)?) };
    let save_write_to_slot: SaveWriteToSlot =
        unsafe { std::mem::transmute(game_rva(module_base, SAVE_WRITE_TO_SLOT_RVA)?) };

    match *phase {
        DIRECT_SEQUENCE_PHASE_COMBINED => {
            let mark_title_bootstrap: MarkTitleBootstrap =
                unsafe { std::mem::transmute(game_rva(module_base, MARK_TITLE_BOOTSTRAP_RVA)?) };
            unsafe { mark_title_bootstrap() };
            unsafe { set_save_slot(slot) };
            unsafe { request_save(REQUEST_SAVE_ENABLED) };
            unsafe { save_request_profile(SAVE_REQUEST_PROFILE_ENABLED) };
            let ret = unsafe {
                combined_save_write(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    LOAD_ARG_TRUE,
                )
            };
            debug(format!(
                "attempt {attempt}: direct trace phase 0 combined_save_write returned {ret}"
            ));
            if ret != MAP_LOAD_FALSE_RETURN {
                *phase = DIRECT_SEQUENCE_PHASE_CONTINUE;
            }
        }
        DIRECT_SEQUENCE_PHASE_CONTINUE => {
            unsafe { save_request_profile(MAP_LOAD_FALSE_RETURN) };
            // NOTE: despite the phase name, this CALLS THE SAVE WRITER (see the warning on
            // SAVE_WRITE_TO_SLOT_RVA above). `-1` resolves to GameMan->saveSlot and the
            // third arg 0 selects the serialize-real-state branch, so this overwrites the
            // active slot with current game state.
            let ret = unsafe {
                save_write_to_slot(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    MAP_LOAD_FALSE_RETURN,
                )
            };
            debug(format!(
                "attempt {attempt}: direct trace phase 1 save_write_to_slot(-1,0,0) returned {ret}"
            ));
            if ret != MAP_LOAD_FALSE_RETURN {
                *phase = DIRECT_SEQUENCE_PHASE_FINAL_COMBINED;
            }
        }
        DIRECT_SEQUENCE_PHASE_FINAL_COMBINED => {
            unsafe { request_save(MAP_LOAD_FALSE_RETURN) };
            let ret = unsafe {
                combined_save_write(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    LOAD_ARG_TRUE,
                )
            };
            debug(format!(
                "attempt {attempt}: direct trace phase 2 combined_save_write returned {ret}"
            ));
            if ret != MAP_LOAD_FALSE_RETURN {
                *phase = DIRECT_SEQUENCE_PHASE_FINAL_COMBINED + LOAD_ARG_TRUE;
            }
        }
        _ => {
            unsafe { request_save(REQUEST_SAVE_ENABLED) };
            unsafe { save_request_profile(SAVE_REQUEST_PROFILE_ENABLED) };
            let ret = unsafe {
                combined_save_write(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    LOAD_ARG_TRUE,
                )
            };
            debug(format!(
                "attempt {attempt}: direct trace repeat combined_save_write returned {ret}"
            ));
        }
    }
    Ok(false)
}

unsafe fn save_buffer_allocator_ready(module_base: usize) -> Result<bool, String> {
    // This is GLOBAL_MainHeapAllocator, not a save-specific allocator (1.16.2 dump: 1821
    // xrefs across CSTaskImp/CSWindowImp/CSEzWork). Name kept for its call site.
    const SAVE_BUFFER_ALLOCATOR_GLOBAL_RVA: u32 = er_game_base::rva::GLOBAL_MAIN_HEAP_ALLOCATOR_RVA as u32;

    let save_buffer_allocator_global = game_rva(module_base, SAVE_BUFFER_ALLOCATOR_GLOBAL_RVA)?;
    let save_buffer_allocator =
        unsafe { *(save_buffer_allocator_global as *const *const std::ffi::c_void) };
    Ok(!save_buffer_allocator.is_null())
}

fn game_rva(module_base: usize, rva: u32) -> Result<usize, String> {
    if module_base == NULL_MODULE_BASE {
        return Err("failed to resolve game module: null module base".to_owned());
    }
    Ok(module_base + rva as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TEMP_FILE_DISAMBIGUATOR: u32 = 1;
    const TEST_SLOT: i32 = 9;
    const TEST_MODULE_BASE: usize = 1;
    const TEST_NULL_MODULE_BASE: usize = 0;
    const TEST_UNSET_SLOT: i32 = 0;

    #[derive(Default)]
    struct FakeGameMan {
        save_slot: i32,
        requested_save_slot_load_index: i32,
        save_state: u32,
        save_requested: bool,
        new_game_plus_requested: bool,
        warp_requested: bool,
    }

    impl GameManSaveAccess for FakeGameMan {
        fn save_slot(&self) -> i32 {
            self.save_slot
        }

        fn set_save_slot(&mut self, slot: i32) {
            self.save_slot = slot;
        }

        fn requested_save_slot_load_index(&self) -> i32 {
            self.requested_save_slot_load_index
        }

        fn set_requested_save_slot_load_index(&mut self, slot: i32) {
            self.requested_save_slot_load_index = slot;
        }

        fn save_state(&self) -> u32 {
            self.save_state
        }

        fn save_requested(&self) -> bool {
            self.save_requested
        }

        fn set_save_requested(&mut self, requested: bool) {
            self.save_requested = requested;
        }

        fn new_game_plus_requested(&self) -> bool {
            self.new_game_plus_requested
        }

        fn warp_requested(&self) -> bool {
            self.warp_requested
        }

        fn set_warp_requested(&mut self, requested: bool) {
            self.warp_requested = requested;
        }
    }

    #[test]
    fn parses_autoload_file() {
        let path = std::env::temp_dir().join(format!(
            "er-save-loader-test-{}-{}.txt",
            std::process::id(),
            TEST_TEMP_FILE_DISAMBIGUATOR
        ));
        fs::write(
            &path,
            "save_ext=co2\nslot=9\nmethod=direct_menu_load\nexperimental_direct_menu_load=1\nrequire_title_bootstrap=false\nown_load=1\nown_load_continue=1\nignored=true\n",
        )
        .unwrap();

        let request = SaveLoadRequest::from_autoload_file_at(&path);
        let _ = fs::remove_file(&path);

        assert_eq!(request.save_extension.as_deref(), Some("co2"));
        assert_eq!(request.slot, Some(TEST_SLOT));
        assert_eq!(request.method, SaveLoadMethod::DirectMenuLoad);
        assert!(!request.require_title_bootstrap);
        assert!(request.own_load);
        assert!(request.own_load_continue);
        assert!(!request.own_stepper);
        assert!(!request.cold_char_mount);
    }

    #[test]
    fn direct_menu_load_downgrades_without_experimental_gate() {
        let path = std::env::temp_dir().join(format!(
            "er-save-loader-test-direct-gate-{}-{}.txt",
            std::process::id(),
            TEST_TEMP_FILE_DISAMBIGUATOR
        ));
        fs::write(&path, "slot=9\nmethod=direct_menu_load\n").unwrap();

        let request = SaveLoadRequest::from_autoload_file_at(&path);
        let _ = fs::remove_file(&path);

        assert_eq!(request.slot, Some(TEST_SLOT));
        assert_eq!(request.method, SaveLoadMethod::SaveRequested);
    }

    #[test]
    fn downgrade_policy_only_neutralizes_gated_direct_menu_load() {
        // The experimental product path (DirectMenuLoad) is neutralized to SaveRequested ONLY when the
        // gate is off; with the gate on it survives. No other method is touched by this policy.
        assert!(should_downgrade_direct_menu_load(
            SaveLoadMethod::DirectMenuLoad,
            false
        ));
        assert!(!should_downgrade_direct_menu_load(
            SaveLoadMethod::DirectMenuLoad,
            true
        ));
        for method in [
            SaveLoadMethod::SaveRequested,
            SaveLoadMethod::RequestedIndex,
            SaveLoadMethod::Both,
            SaveLoadMethod::DirectMapLoad,
            SaveLoadMethod::DirectCombinedLoad,
            SaveLoadMethod::DirectMenuWrapper,
        ] {
            assert!(
                !should_downgrade_direct_menu_load(method, false),
                "only DirectMenuLoad is gated, not {method:?}"
            );
        }
    }

    #[test]
    fn experimental_flag_keeps_direct_menu_load_method() {
        // The file-driven path passes `experimental_direct_menu_load=true`; that alone must keep the
        // DirectMenuLoad method regardless of env/flag-file state, so the host request then satisfies the
        // DLL arming condition `request.method() == DirectMenuLoad`. (The product/portrait smoke instead
        // arms the gate via the `er-effects-experimental-direct-menu-load.txt` flag file, which
        // `experimental_direct_menu_load_gate_enabled` now also honors for the env-method path.)
        let mut request = SaveLoadRequest {
            method: SaveLoadMethod::DirectMenuLoad,
            slot: Some(TEST_SLOT),
            ..SaveLoadRequest::default()
        };
        normalize_experimental_direct_menu_load(&mut request, true);
        assert_eq!(request.method, SaveLoadMethod::DirectMenuLoad);
    }

    #[test]
    fn own_load_continue_defaults_off_and_is_independent_of_own_load() {
        let path = std::env::temp_dir().join(format!(
            "er-save-loader-test-olc-{}-{}.txt",
            std::process::id(),
            TEST_TEMP_FILE_DISAMBIGUATOR
        ));
        // own_load armed but continue NOT set -> verify-only stays the default.
        fs::write(&path, "own_load=1\n").unwrap();
        let request = SaveLoadRequest::from_autoload_file_at(&path);
        let _ = fs::remove_file(&path);
        assert!(request.own_load);
        assert!(!request.own_load_continue);
    }

    #[test]
    fn own_load_install_job_defaults_off_and_parses() {
        // Default: off.
        assert!(!SaveLoadRequest::default().own_load_install_job);
        let path = std::env::temp_dir().join(format!(
            "er-save-loader-test-olij-{}-{}.txt",
            std::process::id(),
            TEST_TEMP_FILE_DISAMBIGUATOR
        ));
        fs::write(&path, "own_load=1\nown_load_install_job=1\n").unwrap();
        let request = SaveLoadRequest::from_autoload_file_at(&path);
        let _ = fs::remove_file(&path);
        assert!(request.own_load);
        assert!(request.own_load_install_job);
        // Independent of the save-writing continue lever.
        assert!(!request.own_load_continue);
    }

    #[test]
    fn title_accept_fallback_is_bounded_safe_input() {
        let sequence = title_accept_fallback_sequence(SafeInputConfig {
            max_hold_frames: TITLE_ACCEPT_CONFIRM_FRAMES,
        })
        .unwrap();
        assert_eq!(
            sequence,
            vec![SafeInputAction::Tap {
                button: SafeButton::Confirm,
                frames: TITLE_ACCEPT_CONFIRM_FRAMES,
            }]
        );
    }

    #[test]
    fn non_direct_methods_update_game_state_without_host_input() {
        let mut game_man = FakeGameMan::default();
        let mut loader = SaveLoader::new(SaveLoadRequest {
            save_extension: None,
            slot: Some(TEST_SLOT),
            method: SaveLoadMethod::Both,
            require_title_bootstrap: REQUIRE_TITLE_BOOTSTRAP_DEFAULT,
            own_stepper: false,
            cold_char_mount: false,
            own_load: false,
            own_load_continue: false,
            own_dispatch: false,
            own_load_install_job: false,
            own_load_pump: false,
        });

        let step = unsafe {
            loader
                .process(
                    &mut game_man,
                    SaveLoadContext {
                        game_module_base: TEST_MODULE_BASE,
                        title_handoff_complete: false,
                        loadgame_build_ctx_ready: false,
                    },
                    |_| {},
                )
                .unwrap()
        };

        assert_eq!(step, SaveLoadStep::Requested);
        assert_eq!(game_man.save_slot, TEST_SLOT);
        assert_eq!(game_man.requested_save_slot_load_index, TEST_SLOT);
        assert!(game_man.save_requested);
        assert_eq!(loader.last_status(), Some("requested slot 9"));
    }

    #[test]
    fn direct_menu_load_waits_for_title_bootstrap_without_touching_input() {
        let mut game_man = FakeGameMan::default();
        let mut loader = SaveLoader::new(SaveLoadRequest {
            save_extension: None,
            slot: Some(TEST_SLOT),
            method: SaveLoadMethod::DirectMenuLoad,
            require_title_bootstrap: REQUIRE_TITLE_BOOTSTRAP_DEFAULT,
            own_stepper: false,
            cold_char_mount: false,
            own_load: false,
            own_load_continue: false,
            own_dispatch: false,
            own_load_install_job: false,
            own_load_pump: false,
        });

        let step = unsafe {
            loader
                .process(
                    &mut game_man,
                    SaveLoadContext {
                        game_module_base: TEST_NULL_MODULE_BASE,
                        title_handoff_complete: false,
                        loadgame_build_ctx_ready: false,
                    },
                    |_| {},
                )
                .unwrap()
        };

        assert_eq!(step, SaveLoadStep::Waiting);
        assert_eq!(game_man.save_slot, TEST_UNSET_SLOT);
        assert!(
            loader
                .last_status()
                .is_some_and(|status| status.starts_with("waiting for title bootstrap"))
        );
    }

    #[test]
    fn method_labels_round_trip_known_values() {
        for method in [
            SaveLoadMethod::SaveRequested,
            SaveLoadMethod::RequestedIndex,
            SaveLoadMethod::Both,
            SaveLoadMethod::DirectMenuLoad,
            SaveLoadMethod::DirectMapLoad,
            SaveLoadMethod::DirectCombinedLoad,
            SaveLoadMethod::DirectCombinedOnly,
            SaveLoadMethod::DirectBootstrapCombined,
            SaveLoadMethod::DirectBootstrapPump,
            SaveLoadMethod::DirectTraceSequence,
            SaveLoadMethod::DirectMenuWrapper,
        ] {
            assert_eq!(SaveLoadMethod::from_label(method.label()), method);
        }
    }
}
