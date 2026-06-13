use std::{fs, path::PathBuf};

use er_safe_input::{SafeButton, SafeInputAction, SafeInputConfig, SafeInputError};

const INITIAL_ATTEMPTS: u64 = 0;
const ATTEMPT_INCREMENT: u64 = 1;
const IDLE_SAVE_STATE: u32 = 0;
const CLEAR_REQUESTED_SAVE_SLOT_LOAD_INDEX: i32 = -1;
const TITLE_ACCEPT_CONFIRM_FRAMES: u16 = 2;
const REQUEST_SAVE_ENABLED: u8 = 1;
const SAVE_REQUEST_PROFILE_ENABLED: u8 = 1;
const NULL_MODULE_BASE: usize = 0;

#[derive(Debug)]
pub struct SaveLoader {
    request: SaveLoadRequest,
    attempts: u64,
    completed: bool,
    last_status: Option<String>,
    direct_seen_initial_save_busy: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SaveLoadRequest {
    pub save_extension: Option<String>,
    pub slot: Option<i32>,
    pub method: SaveLoadMethod,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SaveLoadMethod {
    #[default]
    SaveRequested,
    RequestedIndex,
    Both,
    DirectMenuLoad,
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
        self.attempts = INITIAL_ATTEMPTS;
        self.completed = false;
        self.last_status = None;
        self.direct_seen_initial_save_busy = false;
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
            SaveLoadMethod::DirectMenuLoad => {
                if context.title_bootstrap_seen || game_man.save_state() != IDLE_SAVE_STATE {
                    self.direct_seen_initial_save_busy = true;
                }
                if !self.direct_seen_initial_save_busy {
                    self.last_status = Some(
                        "waiting for title bootstrap/save activity before direct continue queue"
                            .to_owned(),
                    );
                    return Ok(SaveLoadStep::Waiting);
                }

                match unsafe {
                    request_direct_menu_load(
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
                            Some(format!("direct continue sequence requested slot {slot}"));
                        Ok(SaveLoadStep::Requested)
                    }
                    Ok(false) => {
                        self.last_status = Some(format!(
                            "direct continue sequence not ready for slot {slot}"
                        ));
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

unsafe fn request_direct_menu_load<G, F>(
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
    // Runtime/static RE shows the real Continue path is not a direct call to
    // the load primitives. Menu code queues GameMan flags, and the MoveMapList
    // task consumes those flags at safe scheduler points.
    const SET_SAVE_SLOT_RVA: u32 = 0x0067a810;
    const SAVE_REQUEST_PROFILE_RVA: u32 = 0x0067a420;
    const REQUEST_SAVE_RVA: u32 = 0x0067a520;

    type SetSaveSlot = unsafe extern "system" fn(i32);
    type RequestSave = unsafe extern "system" fn(u8);
    type SaveRequestProfile = unsafe extern "system" fn(u8);

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

    debug(format!(
        "attempt {attempt}: queuing traced continue flags for slot {slot}"
    ));
    unsafe { set_save_slot(slot) };
    unsafe { request_save(REQUEST_SAVE_ENABLED) };
    unsafe { save_request_profile(SAVE_REQUEST_PROFILE_ENABLED) };
    Ok(true)
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
            "save_ext=co2\nslot=9\nmethod=direct_menu_load\nignored=true\n",
        )
        .unwrap();

        let request = SaveLoadRequest::from_autoload_file_at(&path);
        let _ = fs::remove_file(&path);

        assert_eq!(request.save_extension.as_deref(), Some("co2"));
        assert_eq!(request.slot, Some(TEST_SLOT));
        assert_eq!(request.method, SaveLoadMethod::DirectMenuLoad);
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
        ] {
            assert_eq!(SaveLoadMethod::from_label(method.label()), method);
        }
    }
}
