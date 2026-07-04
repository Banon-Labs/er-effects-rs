
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
    /// Arm the FINAL OWN-LOAD step: after the proven verify-only parse yields a REAL c30 + real
    /// character, fire the GUARDED `continue_confirm`/`SetState5` to stream the character into the
    /// PLAYABLE world. SAVE-WRITING (`SetState5` autosaves): only fires behind the hard c30/fingerprint
    /// guard in `own_load_continue_drive`. Off by default so the verify-only `own_load` stays safe.
    pub own_load_continue: bool,
    /// Arm the OWN-LOAD m28 direct-enqueue lever: on our menu-free OWN-LOAD path (after our own
    /// `continue_confirm` fired), call `FD4::FD4FileCap::AddDefaultFileLoadProcess` ourselves on the
    /// player block's (m28, area 0x1c) FD4FileCap(s) so the FD4 workers stream the block to residency.
    /// Reaches ONLY world-asset file-load streaming -- no save IO, cannot autosave. Off by default and
    /// double-gated (it ALSO requires `OWN_LOAD_CONTINUE_FIRED`), so it can never fire on a vanilla
    /// native menu load. Env `ER_EFFECTS_OWN_DISPATCH=1` / `own_dispatch=1` in the autoload file.
    pub own_dispatch: bool,
    /// Arm the menu-free LoadGame-JOB install lever: instead of the guarded `continue_confirm`/
    /// `SetState5`, BUILD the native LoadGame `MenuJobWithContext<LoadJobContext>` (factory
    /// `FUN_140826510`) and INSTALL it into the title owner's `+0x130` MenuJob slot (assign helper
    /// `FUN_1407a9560`), replacing the idle `IfElseJob`. `STEP_MenuJobWait` then ticks it each frame,
    /// self-builds, deserializes the save, and streams the world -- no `SetState5`, no save write.
    /// SAVE-SAFE (build + first-tick deser only READ the save). Off by default; double-gated -- it
    /// ALSO requires `OWN_LOAD_CONTINUE_FIRED`-style arming via `own_load`. Env
    /// `ER_EFFECTS_OWN_LOAD_INSTALL_JOB=1` / `own_load_install_job=1` in the autoload file.
    pub own_load_install_job: bool,
    /// PATH B "own the load" PRIVATE-PUMP lever. When set (with `own_load`), the verify-only parse is
    /// followed by BUILD of the LoadGame `MenuJobWithContext` with REAL mss-derived ctx; the recurring
    /// game task then ticks its `Run` PRIVATELY every frame to completion (deser -> m28 stream) and, on
    /// `state==Success`, drives the title->ingame transition via the guarded `SetState5`. No
    /// owner+0x130 install, no MenuJobQueue, no CSMenuMan dialog -- the menu-free subsystem rebuild.
    /// Off by default. Env `ER_EFFECTS_OWN_LOAD_PUMP=1` / `own_load_pump=1` in the autoload file.
    pub own_load_pump: bool,
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
    /// True once the natural title flow has handed off into the live menu/save state (first
    /// system-profile save). Conservative legacy gate -- it only flips AFTER the player advances past
    /// the press-any-button title.
    pub title_handoff_complete: bool,
    /// True once the engine is "filled enough" to build+drive the LoadGame job WITHOUT the title being
    /// advanced: GameDataMan -> menuSystemSaveLoad -> a PLAUSIBLE TitleFlowContext are all present. This
    /// is the BYPASS arming signal -- it goes true at the title (GameFlow up) without the system-save
    /// handoff, so the direct own-load can arm and skip the frontend entirely. See
    /// `loadgame_build_ctx_ready` in the DLL (loadgame-build-ctx-ready-precondition-2026-06-22).
    pub loadgame_build_ctx_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveLoadStep {
    Idle,
    Waiting,
    Requested,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GameManTelemetry {
    pub save_slot: i32,
    pub requested_save_slot_load_index: i32,
    pub save_state: u32,
    pub save_requested: bool,
    pub new_game_plus_requested: bool,
    pub warp_requested: bool,
}

pub trait GameManSaveAccess {
    fn save_slot(&self) -> i32;
    fn set_save_slot(&mut self, slot: i32);
    fn requested_save_slot_load_index(&self) -> i32;
    fn set_requested_save_slot_load_index(&mut self, slot: i32);
    fn save_state(&self) -> u32;
    fn save_requested(&self) -> bool;
    fn set_save_requested(&mut self, requested: bool);
    /// `GameMan::new_game_plus_requested` -- set when a New Game (+) is requested; drives the
    /// new-game intro flow. Read-only snapshot field (a stale `true` after a load would explain a
    /// post-load bounce into the new-game/title path).
    fn new_game_plus_requested(&self) -> bool;
    /// `GameMan::warp_requested` -- the map-move/warp trigger consumed by `MoveMapStep`.
    fn warp_requested(&self) -> bool;
    /// Clear/set `GameMan::warp_requested`. The native full deserialize (`0x67b290`) sets this true
    /// as a "warp reload pending" flag; `MoveMapStep::CheckReturnToTitle` (dump `FUN_140afa7c0`)
    /// reads it every frame as a return-to-title trigger. A fresh title->world stream (our reload's
    /// SetState5) never consumes it, so it must be cleared or the freshly-loaded world bounces back
    /// to the title. Mirrors `SetCallForWarp` (dump `0x14067af90`).
    fn set_warp_requested(&mut self, requested: bool);
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

    /// Whether the autoload config armed the FINAL guarded `continue_confirm`/`SetState5` world-stream
    /// step after the verify-only OWN-LOAD parse. SAVE-WRITING when it fires (behind the c30 guard).
    #[must_use]
    pub const fn own_load_continue(&self) -> bool {
        self.request.own_load_continue
    }

    /// Whether the autoload config armed the OWN-LOAD m28 direct-enqueue lever
    /// (`AddDefaultFileLoadProcess` on the player block's FD4FileCap[s]). Reaches only world-asset
    /// file-load streaming -- no save IO. Double-gated by `OWN_LOAD_CONTINUE_FIRED` at fire time.
    #[must_use]
    pub const fn own_dispatch(&self) -> bool {
        self.request.own_dispatch
    }

    /// Whether the autoload config armed the menu-free LoadGame-JOB install lever (build the native
    /// LoadGame `MenuJobWithContext` and install it into the title owner's `+0x130` MenuJob slot).
    /// SAVE-SAFE (build + first-tick deser only read the save). Off by default.
    #[must_use]
    pub const fn own_load_install_job(&self) -> bool {
        self.request.own_load_install_job
    }

    /// Whether the autoload config armed the PATH B menu-free PRIVATE-PUMP lever (build the LoadGame
    /// `MenuJobWithContext` with REAL mss-derived ctx, then tick its `Run` privately each frame to
    /// completion + drive the transition on Success). SAVE-SAFE at build; only the final SetState5
    /// transition writes, and it stays guarded. Off by default.
    #[must_use]
    pub const fn own_load_pump(&self) -> bool {
        self.request.own_load_pump
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
                    || context.title_handoff_complete
                    // BYPASS: the engine being filled enough to build the LoadGame job (plausible
                    // TitleFlowContext) is sufficient to arm the direct own-load AT THE TITLE, without
                    // waiting for the natural press-any-button -> menu handoff. This is what lets us skip
                    // the frontend entirely (boot-singleton-order-bypass-feasible-2026-06-22).
                    || context.loadgame_build_ctx_ready
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
            own_load_continue: false,
            own_dispatch: false,
            own_load_install_job: false,
            own_load_pump: false,
        }
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(value, "1" | "true" | "yes" | "on")
}

/// The experimental-direct-menu-load gate as the DLL sees it (mirror of the DLL's
/// `experimental_direct_menu_load_enabled` in `gating.rs`): armed by EITHER the
/// `ER_EFFECTS_EXPERIMENTAL_DIRECT_MENU_LOAD` env var OR the
/// `er-effects-experimental-direct-menu-load.txt` flag file next to the game exe.
///
/// `from_env` previously consulted only the env var. A run that armed the gate via the FILE (the
/// product/portrait smoke) but supplied the method via `ER_EFFECTS_AUTOLOAD_METHOD=direct_menu_load`
/// therefore had its `DirectMenuLoad` method silently downgraded to `SaveRequested` here -> the DLL's
/// `arm_product_autoload_from_request` never set `PRODUCT_AUTOLOAD_ARMED` -> `product_core_autoload_tick`
/// never ran -> only the slot-less accept-byte fallback advanced the menu, which starts a NEW GAME
/// (a fresh Vagabond, not the configured save). Honoring the same file flag here keeps the host request
/// and the DLL gate consistent so the env-method + file-flag combination arms the product path.
fn experimental_direct_menu_load_gate_enabled() -> bool {
    if std::env::var("ER_EFFECTS_EXPERIMENTAL_DIRECT_MENU_LOAD")
        .is_ok_and(|value| parse_bool(value.trim()))
    {
        return true;
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-experimental-direct-menu-load.txt")
        .exists()
}

/// Pure downgrade policy: `DirectMenuLoad` is the experimental product path, so it is neutralized to
/// `SaveRequested` UNLESS the experimental gate is enabled. Only `DirectMenuLoad` is gated this way
/// (other `Direct*` methods are unaffected). Pure (no env/fs reads) so the policy is unit-testable;
/// callers resolve the gate and pass it in.
const fn should_downgrade_direct_menu_load(
    method: SaveLoadMethod,
    experimental_gate_enabled: bool,
) -> bool {
    matches!(method, SaveLoadMethod::DirectMenuLoad) && !experimental_gate_enabled
}

fn normalize_experimental_direct_menu_load(
    request: &mut SaveLoadRequest,
    experimental_direct_menu_load: bool,
) {
    let gate = experimental_direct_menu_load || experimental_direct_menu_load_gate_enabled();
    if should_downgrade_direct_menu_load(request.method, gate) {
        request.method = SaveLoadMethod::SaveRequested;
    }
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
        let mut method_from_env = false;
        if let Ok(method) = std::env::var("ER_EFFECTS_AUTOLOAD_METHOD") {
            request.method = SaveLoadMethod::from_label(&method);
            method_from_env = true;
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
        if matches!(
            std::env::var("ER_EFFECTS_OWN_LOAD_CONTINUE").as_deref(),
            Ok("1")
        ) {
            request.own_load_continue = true;
        }
        if matches!(std::env::var("ER_EFFECTS_OWN_DISPATCH").as_deref(), Ok("1")) {
            request.own_dispatch = true;
        }
        if matches!(
            std::env::var("ER_EFFECTS_OWN_LOAD_INSTALL_JOB").as_deref(),
            Ok("1")
        ) {
            request.own_load_install_job = true;
        }
        if method_from_env {
            normalize_experimental_direct_menu_load(&mut request, false);
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

        let mut experimental_direct_menu_load = false;
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
                "own_load_continue" => request.own_load_continue = parse_bool(value.trim()),
                "own_dispatch" => request.own_dispatch = parse_bool(value.trim()),
                "own_load_install_job" => request.own_load_install_job = parse_bool(value.trim()),
                "own_load_pump" => request.own_load_pump = parse_bool(value.trim()),
                "experimental_direct_menu_load" => {
                    experimental_direct_menu_load = parse_bool(value.trim())
                }
                _ => {}
            }
        }
        normalize_experimental_direct_menu_load(&mut request, experimental_direct_menu_load);

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
            new_game_plus_requested: game_man.new_game_plus_requested(),
            warp_requested: game_man.warp_requested(),
        }
    }
}
