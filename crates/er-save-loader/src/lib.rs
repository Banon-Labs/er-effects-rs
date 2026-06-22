use std::{fs, path::PathBuf};

use er_safe_input::{SafeButton, SafeInputAction, SafeInputConfig, SafeInputError};

pub mod bnd4;

const INITIAL_ATTEMPTS: u64 = 0;
const ATTEMPT_INCREMENT: u64 = 1;
const IDLE_SAVE_STATE: u32 = 0;
const CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX: i32 = -1;
const TITLE_ACCEPT_CONFIRM_FRAMES: u16 = 2;
const REQUEST_SAVE_ENABLED: u8 = true as u8;
const SAVE_REQUEST_PROFILE_ENABLED: u8 = true as u8;
const NULL_MODULE_BASE: usize = 0;
const MAP_LOAD_FALSE_RETURN: u8 = 0;
const LOAD_ARG_FALSE: u8 = 0;
const LOAD_ARG_TRUE: u8 = 1;
const DIRECT_SEQUENCE_PHASE_COMBINED: u8 = 0;
const DIRECT_SEQUENCE_PHASE_CONTINUE: u8 = 1;
const DIRECT_SEQUENCE_PHASE_FINAL_COMBINED: u8 = 2;
#[repr(u32)]
enum NativeSaveMenuRva {
    MenuOtherLoadStatePtr = 0x0010f060,
    SaveLoadPumpDefault = 0x00679510,
    SaveRequestProfile = 0x0067a420,
    RequestSave = 0x0067a520,
    SetSaveSlot = 0x0067a810,
    SaveLoadStateInit = 0x0067b030,
    MenuOtherLoadWrapper = 0x0082bb00,
}

const MENU_OTHER_LOAD_STATE_PTR: usize = NativeSaveMenuRva::MenuOtherLoadStatePtr as usize;
pub const SET_SAVE_SLOT_RVA: u32 = NativeSaveMenuRva::SetSaveSlot as u32;
pub const SAVE_REQUEST_PROFILE_RVA: u32 = NativeSaveMenuRva::SaveRequestProfile as u32;
pub const REQUEST_SAVE_RVA: u32 = NativeSaveMenuRva::RequestSave as u32;
const COMBINED_LOAD_RVA: u32 = 0x0067b940;
const MARK_TITLE_BOOTSTRAP_RVA: u32 = 0x0067a310;
const SAVE_LOAD_PUMP_DEFAULT_RVA: u32 = NativeSaveMenuRva::SaveLoadPumpDefault as u32;
pub const SAVE_LOAD_STATE_INIT_RVA: u32 = NativeSaveMenuRva::SaveLoadStateInit as u32;
pub const MENU_OTHER_LOAD_WRAPPER_RVA: u32 = NativeSaveMenuRva::MenuOtherLoadWrapper as u32;
const REQUIRE_TITLE_BOOTSTRAP_DEFAULT: bool = true;

#[derive(Debug)]
pub struct SaveLoader {
    request: SaveLoadRequest,
    attempts: u64,
    completed: bool,
    last_status: Option<String>,
    direct_seen_initial_save_busy: bool,
    direct_sequence_phase: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveLoadRequest {
    pub save_extension: Option<String>,
    pub slot: Option<i32>,
    pub method: SaveLoadMethod,
    pub require_title_bootstrap: bool,
    /// Arm the menu-free own-stepper path (idx10 patch) via the reliable autoload-file channel.
    /// Equivalent to the `ER_EFFECTS_OWN_STEPPER` env / `er-effects-own-stepper.txt` trigger, but
    /// configurable through `er-effects-autoload.txt` (read CWD-relative, the only channel that
    /// reliably reaches the DLL under the Proton probe harness).
    pub own_stepper: bool,
    /// Arm the menu-free cold-char-mount save-IO load through the same reliable channel.
    pub cold_char_mount: bool,
    /// Arm the SAVE-SAFE verify-only OWN-LOAD buffer-feed probe through the same reliable channel.
    /// When set, the DLL hooks the FSM-gated save read (`0x67b100`), feeds it our sliced plaintext
    /// `.sl2` slot body, calls the native parser (`0x67b290`), and reads back GameMan+0xc30 + the
    /// PlayerGameData fingerprint -- no `SetState5`, no autosave, no `continue_confirm`.
    pub own_load: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SaveLoadMethod {
    #[default]
    SaveRequested,
    RequestedIndex,
    Both,
    DirectMenuLoad,
    DirectMapLoad,
    DirectCombinedLoad,
    DirectCombinedOnly,
    DirectBootstrapCombined,
    DirectBootstrapPump,
    DirectTraceSequence,
    DirectMenuWrapper,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaveLoadContext {
    pub game_module_base: usize,
    pub title_bootstrap_seen: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveLoadStep {
    Idle,
    Waiting,
    Requested,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GameManTelemetry {
    pub save_slot: i32,
    pub requested_save_slot_load_index: i32,
    pub save_state: u32,
    pub save_requested: bool,
}

pub trait GameManSaveAccess {
    fn save_slot(&self) -> i32;
    fn set_save_slot(&mut self, slot: i32);
    fn requested_save_slot_load_index(&self) -> i32;
    fn set_requested_save_slot_load_index(&mut self, slot: i32);
    fn save_state(&self) -> u32;
    fn save_requested(&self) -> bool;
    fn set_save_requested(&mut self, requested: bool);
}

#[cfg(windows)]
impl GameManSaveAccess for eldenring::cs::GameMan {
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
}

impl SaveLoader {
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(SaveLoadRequest::from_env())
    }

    #[must_use]
    pub fn new(request: SaveLoadRequest) -> Self {
        Self {
            request,
            attempts: INITIAL_ATTEMPTS,
            completed: false,
            last_status: None,
            direct_seen_initial_save_busy: false,
            direct_sequence_phase: DIRECT_SEQUENCE_PHASE_COMBINED,
        }
    }

    #[must_use]
    pub const fn request(&self) -> &SaveLoadRequest {
        &self.request
    }

    #[must_use]
    pub const fn attempts(&self) -> u64 {
        self.attempts
    }

    #[must_use]
    pub const fn completed(&self) -> bool {
        self.completed
    }

    #[must_use]
    pub fn last_status(&self) -> Option<&str> {
        self.last_status.as_deref()
    }

    pub fn set_last_status(&mut self, status: impl Into<String>) {
        self.last_status = Some(status.into());
    }

    pub fn queue_direct_menu_load(&mut self, slot: i32) {
        self.request.slot = Some(slot);
        self.request.method = SaveLoadMethod::DirectMenuLoad;
        self.request.require_title_bootstrap = REQUIRE_TITLE_BOOTSTRAP_DEFAULT;
        self.attempts = INITIAL_ATTEMPTS;
        self.completed = false;
        self.last_status = None;
        self.direct_seen_initial_save_busy = false;
        self.direct_sequence_phase = DIRECT_SEQUENCE_PHASE_COMBINED;
    }

    #[must_use]
    pub fn save_extension(&self) -> Option<&str> {
        self.request.save_extension.as_deref()
    }

    #[must_use]
    pub const fn slot(&self) -> Option<i32> {
        self.request.slot
    }

    #[must_use]
    pub const fn method(&self) -> SaveLoadMethod {
        self.request.method
    }

    #[must_use]
    pub const fn requires_title_bootstrap(&self) -> bool {
        self.request.require_title_bootstrap
    }

    /// Whether the autoload config armed the menu-free own-stepper path (idx10 patch).
    #[must_use]
    pub const fn own_stepper(&self) -> bool {
        self.request.own_stepper
    }

    /// Whether the autoload config armed the menu-free cold-char-mount save-IO load.
    #[must_use]
    pub const fn cold_char_mount(&self) -> bool {
        self.request.cold_char_mount
    }

    /// Whether the autoload config armed the SAVE-SAFE verify-only OWN-LOAD buffer-feed probe.
    #[must_use]
    pub const fn own_load(&self) -> bool {
        self.request.own_load
    }

    /// Advance the load request state machine once.
    ///
    /// The direct menu-load path queues the same native flags observed from the
    /// title/menu Continue path, then lets Elden Ring's scheduler consume those
    /// flags. It intentionally does not synthesize host mouse/keyboard events.
    ///
    /// # Safety
    ///
    /// `context.game_module_base` must be the base address of the current
    /// Elden Ring executable module for the active process. The passed
    /// `game_man` must be the live singleton for that process.
    pub unsafe fn process<G, F>(
        &mut self,
        game_man: &mut G,
        context: SaveLoadContext,
        mut debug: F,
    ) -> Result<SaveLoadStep, String>
    where
        G: GameManSaveAccess,
        F: FnMut(String),
    {
        if self.completed {
            return Ok(SaveLoadStep::Idle);
        }

        let Some(slot) = self.request.slot else {
            return Ok(SaveLoadStep::Idle);
        };

        self.attempts += ATTEMPT_INCREMENT;
        match self.request.method {
            SaveLoadMethod::SaveRequested => {
                game_man.set_save_slot(slot);
                game_man.set_save_requested(true);
                self.last_status = Some(format!("requested slot {slot}"));
                Ok(SaveLoadStep::Requested)
            }
            SaveLoadMethod::RequestedIndex => {
                game_man.set_requested_save_slot_load_index(slot);
                self.last_status = Some(format!("requested slot {slot}"));
                Ok(SaveLoadStep::Requested)
            }
            SaveLoadMethod::Both => {
                game_man.set_save_slot(slot);
                game_man.set_requested_save_slot_load_index(slot);
                game_man.set_save_requested(true);
                self.last_status = Some(format!("requested slot {slot}"));
                Ok(SaveLoadStep::Requested)
            }
            SaveLoadMethod::DirectMenuLoad
            | SaveLoadMethod::DirectMapLoad
            | SaveLoadMethod::DirectCombinedLoad
            | SaveLoadMethod::DirectCombinedOnly
            | SaveLoadMethod::DirectBootstrapCombined
            | SaveLoadMethod::DirectBootstrapPump
            | SaveLoadMethod::DirectTraceSequence
            | SaveLoadMethod::DirectMenuWrapper => {
                if !self.request.require_title_bootstrap
                    || context.title_bootstrap_seen
                    || game_man.save_state() != IDLE_SAVE_STATE
                {
                    self.direct_seen_initial_save_busy = true;
                }
                if !self.direct_seen_initial_save_busy {
                    self.last_status = Some(
                        "waiting for title bootstrap/save activity before direct continue queue"
                            .to_owned(),
                    );
                    return Ok(SaveLoadStep::Waiting);
                }

                if self.request.method == SaveLoadMethod::DirectTraceSequence {
                    return match unsafe {
                        request_direct_trace_sequence(
                            game_man,
                            context.game_module_base,
                            slot,
                            self.attempts,
                            &mut self.direct_sequence_phase,
                            &mut debug,
                        )
                    } {
                        Ok(false) => {
                            self.last_status = Some(format!(
                                "direct trace sequence phase {} awaiting player for slot {slot}",
                                self.direct_sequence_phase
                            ));
                            Ok(SaveLoadStep::Waiting)
                        }
                        Ok(true) => {
                            self.completed = true;
                            self.last_status =
                                Some(format!("direct trace sequence requested slot {slot}"));
                            Ok(SaveLoadStep::Requested)
                        }
                        Err(error) => {
                            self.last_status = Some(error.clone());
                            Err(error)
                        }
                    };
                }

                if self.request.method == SaveLoadMethod::DirectMenuWrapper {
                    return match unsafe {
                        request_direct_menu_wrapper(
                            game_man,
                            context.game_module_base,
                            slot,
                            self.attempts,
                            &mut debug,
                        )
                    } {
                        Ok(true) => {
                            game_man.set_requested_save_slot_load_index(
                                CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                            );
                            self.completed = true;
                            self.last_status =
                                Some(format!("direct menu wrapper requested slot {slot}"));
                            Ok(SaveLoadStep::Requested)
                        }
                        Ok(false) => {
                            self.last_status =
                                Some(format!("direct menu wrapper not ready for slot {slot}"));
                            Ok(SaveLoadStep::Waiting)
                        }
                        Err(error) => {
                            self.last_status = Some(error.clone());
                            Err(error)
                        }
                    };
                }

                match unsafe {
                    request_direct_menu_load(
                        game_man,
                        context.game_module_base,
                        slot,
                        self.attempts,
                        self.request.method == SaveLoadMethod::DirectMapLoad
                            || self.request.method == SaveLoadMethod::DirectCombinedLoad,
                        self.request.method == SaveLoadMethod::DirectCombinedLoad
                            || self.request.method == SaveLoadMethod::DirectCombinedOnly
                            || self.request.method == SaveLoadMethod::DirectBootstrapCombined
                            || self.request.method == SaveLoadMethod::DirectBootstrapPump,
                        self.request.method == SaveLoadMethod::DirectBootstrapCombined
                            || self.request.method == SaveLoadMethod::DirectBootstrapPump,
                        self.request.method == SaveLoadMethod::DirectBootstrapPump,
                        &mut debug,
                    )
                } {
                    Ok(true) => {
                        game_man.set_requested_save_slot_load_index(
                            CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                        );
                        self.completed = true;
                        self.last_status = Some(match self.request.method {
                            SaveLoadMethod::DirectMapLoad => {
                                format!("direct map load requested slot {slot}")
                            }
                            SaveLoadMethod::DirectCombinedLoad => {
                                format!("direct combined load requested slot {slot}")
                            }
                            SaveLoadMethod::DirectCombinedOnly => {
                                format!("direct combined-only load requested slot {slot}")
                            }
                            SaveLoadMethod::DirectBootstrapCombined => {
                                format!("direct bootstrap combined load requested slot {slot}")
                            }
                            SaveLoadMethod::DirectBootstrapPump => {
                                format!("direct bootstrap pump requested slot {slot}")
                            }
                            _ => format!("direct continue sequence requested slot {slot}"),
                        });
                        Ok(SaveLoadStep::Requested)
                    }
                    Ok(false) => {
                        self.last_status = Some(
                            if self.request.method == SaveLoadMethod::DirectBootstrapPump {
                                format!("direct bootstrap pump awaiting player for slot {slot}")
                            } else {
                                format!("direct continue sequence not ready for slot {slot}")
                            },
                        );
                        Ok(SaveLoadStep::Waiting)
                    }
                    Err(error) => {
                        self.last_status = Some(error.clone());
                        Err(error)
                    }
                }
            }
        }
    }
}

impl Default for SaveLoadRequest {
    fn default() -> Self {
        Self {
            save_extension: None,
            slot: None,
            method: SaveLoadMethod::default(),
            require_title_bootstrap: REQUIRE_TITLE_BOOTSTRAP_DEFAULT,
            own_stepper: false,
            cold_char_mount: false,
            own_load: false,
        }
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(value, "1" | "true" | "yes" | "on")
}

impl SaveLoadRequest {
    #[must_use]
    pub fn from_env() -> Self {
        let mut request = Self::from_autoload_file();

        if let Ok(save_extension) = std::env::var("ER_EFFECTS_AUTOLOAD_SAVE_EXT") {
            request.save_extension = Some(save_extension);
        }
        if let Some(slot) = std::env::var("ER_EFFECTS_AUTOLOAD_SLOT")
            .ok()
            .and_then(|slot| slot.parse().ok())
        {
            request.slot = Some(slot);
        }
        if let Ok(method) = std::env::var("ER_EFFECTS_AUTOLOAD_METHOD") {
            request.method = SaveLoadMethod::from_label(&method);
        }
        if let Ok(require_title_bootstrap) =
            std::env::var("ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP")
        {
            request.require_title_bootstrap = parse_bool(require_title_bootstrap.trim());
        }
        if matches!(std::env::var("ER_EFFECTS_OWN_STEPPER").as_deref(), Ok("1")) {
            request.own_stepper = true;
        }
        if matches!(
            std::env::var("ER_EFFECTS_COLD_CHAR_MOUNT").as_deref(),
            Ok("1")
        ) {
            request.cold_char_mount = true;
        }
        if matches!(std::env::var("ER_EFFECTS_OWN_LOAD").as_deref(), Ok("1")) {
            request.own_load = true;
        }

        request
    }

    #[must_use]
    pub fn from_autoload_file() -> Self {
        let path = std::env::var("ER_EFFECTS_AUTOLOAD_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("er-effects-autoload.txt"));
        Self::from_autoload_file_at(path)
    }

    #[must_use]
    pub fn from_autoload_file_at(path: impl Into<PathBuf>) -> Self {
        let mut request = Self::default();
        let Ok(contents) = fs::read_to_string(path.into()) else {
            return request;
        };

        for line in contents.lines().map(str::trim) {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "save_ext" | "save_extension" => {
                    request.save_extension = Some(value.trim().to_owned())
                }
                "slot" => request.slot = value.trim().parse().ok(),
                "method" => request.method = SaveLoadMethod::from_label(value.trim()),
                "require_title_bootstrap" => {
                    request.require_title_bootstrap = parse_bool(value.trim())
                }
                "own_stepper" => request.own_stepper = parse_bool(value.trim()),
                "cold_char_mount" => request.cold_char_mount = parse_bool(value.trim()),
                "own_load" => request.own_load = parse_bool(value.trim()),
                _ => {}
            }
        }

        request
    }
}

impl SaveLoadMethod {
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        match label {
            "requested_index" => Self::RequestedIndex,
            "both" => Self::Both,
            "direct_menu_load" => Self::DirectMenuLoad,
            "direct_map_load" => Self::DirectMapLoad,
            "direct_combined_load" => Self::DirectCombinedLoad,
            "direct_combined_only" => Self::DirectCombinedOnly,
            "direct_bootstrap_combined" => Self::DirectBootstrapCombined,
            "direct_bootstrap_pump" => Self::DirectBootstrapPump,
            "direct_trace_sequence" => Self::DirectTraceSequence,
            "direct_menu_wrapper" => Self::DirectMenuWrapper,
            _ => Self::SaveRequested,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SaveRequested => "save_requested",
            Self::RequestedIndex => "requested_index",
            Self::Both => "both",
            Self::DirectMenuLoad => "direct_menu_load",
            Self::DirectMapLoad => "direct_map_load",
            Self::DirectCombinedLoad => "direct_combined_load",
            Self::DirectCombinedOnly => "direct_combined_only",
            Self::DirectBootstrapCombined => "direct_bootstrap_combined",
            Self::DirectBootstrapPump => "direct_bootstrap_pump",
            Self::DirectTraceSequence => "direct_trace_sequence",
            Self::DirectMenuWrapper => "direct_menu_wrapper",
        }
    }
}

impl GameManTelemetry {
    #[must_use]
    pub fn from_game_man(game_man: &(impl GameManSaveAccess + ?Sized)) -> Self {
        Self {
            save_slot: game_man.save_slot(),
            requested_save_slot_load_index: game_man.requested_save_slot_load_index(),
            save_state: game_man.save_state(),
            save_requested: game_man.save_requested(),
        }
    }
}

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
    // 0x1407a91e0 instead of calling map_load in isolation.
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

unsafe fn request_direct_menu_load<G, F>(
    game_man: &mut G,
    module_base: usize,
    slot: i32,
    attempt: u64,
    call_map_load: bool,
    call_combined_load: bool,
    call_title_bootstrap_marker: bool,
    call_save_load_pump: bool,
    debug: &mut F,
) -> Result<bool, String>
where
    G: GameManSaveAccess,
    F: FnMut(String),
{
    // Runtime/static RE shows the real Continue path is not a direct call to
    // the load primitives. Menu code queues GameMan flags, and the MoveMapList
    // task consumes those flags at safe scheduler points.
    const MAP_LOAD_RVA: u32 = 0x0067bc10;

    type SetSaveSlot = unsafe extern "system" fn(i32);
    type RequestSave = unsafe extern "system" fn(u8);
    type SaveRequestProfile = unsafe extern "system" fn(u8);
    type MapLoad = unsafe extern "system" fn() -> u8;
    type CombinedLoad = unsafe extern "system" fn(i32, u8, u8) -> u8;
    type MarkTitleBootstrap = unsafe extern "system" fn();
    type SaveLoadPumpDefault = unsafe extern "system" fn();

    if call_save_load_pump && game_man.save_state() != IDLE_SAVE_STATE {
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

    if call_title_bootstrap_marker {
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
    if call_combined_load && !call_map_load {
        let combined_load: CombinedLoad =
            unsafe { std::mem::transmute(game_rva(module_base, COMBINED_LOAD_RVA)?) };
        let combined_ret = unsafe {
            combined_load(
                CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                LOAD_ARG_FALSE,
                LOAD_ARG_TRUE,
            )
        };
        debug(format!(
            "attempt {attempt}: direct combined_load returned {combined_ret}"
        ));
        if call_save_load_pump {
            return Ok(false);
        }
        return Ok(combined_ret != MAP_LOAD_FALSE_RETURN);
    }
    if call_map_load {
        let map_load: MapLoad =
            unsafe { std::mem::transmute(game_rva(module_base, MAP_LOAD_RVA)?) };
        let ret = unsafe { map_load() };
        debug(format!("attempt {attempt}: direct map_load returned {ret}"));
        if ret == MAP_LOAD_FALSE_RETURN {
            return Ok(false);
        }
        if call_combined_load {
            let combined_load: CombinedLoad =
                unsafe { std::mem::transmute(game_rva(module_base, COMBINED_LOAD_RVA)?) };
            let combined_ret = unsafe {
                combined_load(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    LOAD_ARG_TRUE,
                )
            };
            debug(format!(
                "attempt {attempt}: direct combined_load returned {combined_ret}"
            ));
            if call_save_load_pump {
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
    const CONTINUE_LOAD_RVA: u32 = 0x0067b750;

    type SetSaveSlot = unsafe extern "system" fn(i32);
    type RequestSave = unsafe extern "system" fn(u8);
    type SaveRequestProfile = unsafe extern "system" fn(u8);
    type CombinedLoad = unsafe extern "system" fn(i32, u8, u8) -> u8;
    type ContinueLoad = unsafe extern "system" fn(i32, u8, u8) -> u8;
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
    let combined_load: CombinedLoad =
        unsafe { std::mem::transmute(game_rva(module_base, COMBINED_LOAD_RVA)?) };
    let continue_load: ContinueLoad =
        unsafe { std::mem::transmute(game_rva(module_base, CONTINUE_LOAD_RVA)?) };

    match *phase {
        DIRECT_SEQUENCE_PHASE_COMBINED => {
            let mark_title_bootstrap: MarkTitleBootstrap =
                unsafe { std::mem::transmute(game_rva(module_base, MARK_TITLE_BOOTSTRAP_RVA)?) };
            unsafe { mark_title_bootstrap() };
            unsafe { set_save_slot(slot) };
            unsafe { request_save(REQUEST_SAVE_ENABLED) };
            unsafe { save_request_profile(SAVE_REQUEST_PROFILE_ENABLED) };
            let ret = unsafe {
                combined_load(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    LOAD_ARG_TRUE,
                )
            };
            debug(format!(
                "attempt {attempt}: direct trace phase 0 combined_load returned {ret}"
            ));
            if ret != MAP_LOAD_FALSE_RETURN {
                *phase = DIRECT_SEQUENCE_PHASE_CONTINUE;
            }
        }
        DIRECT_SEQUENCE_PHASE_CONTINUE => {
            unsafe { save_request_profile(MAP_LOAD_FALSE_RETURN) };
            let ret = unsafe {
                continue_load(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    MAP_LOAD_FALSE_RETURN,
                )
            };
            debug(format!(
                "attempt {attempt}: direct trace phase 1 continue_load returned {ret}"
            ));
            if ret != MAP_LOAD_FALSE_RETURN {
                *phase = DIRECT_SEQUENCE_PHASE_FINAL_COMBINED;
            }
        }
        DIRECT_SEQUENCE_PHASE_FINAL_COMBINED => {
            unsafe { request_save(MAP_LOAD_FALSE_RETURN) };
            let ret = unsafe {
                combined_load(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    LOAD_ARG_TRUE,
                )
            };
            debug(format!(
                "attempt {attempt}: direct trace phase 2 combined_load returned {ret}"
            ));
            if ret != MAP_LOAD_FALSE_RETURN {
                *phase = DIRECT_SEQUENCE_PHASE_FINAL_COMBINED + LOAD_ARG_TRUE;
            }
        }
        _ => {
            unsafe { request_save(REQUEST_SAVE_ENABLED) };
            unsafe { save_request_profile(SAVE_REQUEST_PROFILE_ENABLED) };
            let ret = unsafe {
                combined_load(
                    CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX,
                    LOAD_ARG_FALSE,
                    LOAD_ARG_TRUE,
                )
            };
            debug(format!(
                "attempt {attempt}: direct trace repeat combined_load returned {ret}"
            ));
        }
    }
    Ok(false)
}

unsafe fn save_buffer_allocator_ready(module_base: usize) -> Result<bool, String> {
    const SAVE_BUFFER_ALLOCATOR_GLOBAL_RVA: u32 = 0x03d872e0;

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
            "save_ext=co2\nslot=9\nmethod=direct_menu_load\nrequire_title_bootstrap=false\nown_load=1\nignored=true\n",
        )
        .unwrap();

        let request = SaveLoadRequest::from_autoload_file_at(&path);
        let _ = fs::remove_file(&path);

        assert_eq!(request.save_extension.as_deref(), Some("co2"));
        assert_eq!(request.slot, Some(TEST_SLOT));
        assert_eq!(request.method, SaveLoadMethod::DirectMenuLoad);
        assert!(!request.require_title_bootstrap);
        assert!(request.own_load);
        assert!(!request.own_stepper);
        assert!(!request.cold_char_mount);
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
        });

        let step = unsafe {
            loader
                .process(
                    &mut game_man,
                    SaveLoadContext {
                        game_module_base: TEST_MODULE_BASE,
                        title_bootstrap_seen: false,
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
        });

        let step = unsafe {
            loader
                .process(
                    &mut game_man,
                    SaveLoadContext {
                        game_module_base: TEST_NULL_MODULE_BASE,
                        title_bootstrap_seen: false,
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
