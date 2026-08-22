// ============================================================================================
// STAGE 2 -- the VERIFIED in-context menu-drive that actually COMPLETES a character load.
// After PHASE_MENU_BUILD identifies the Load-Game leaf d180 (MENU_LOAD_GAME_ITEM), STAGE 2
// invokes its +0xa8 functor (-> ProfileLoadDialog), sets the dialog slot cursor, calls the
// dialog's vtable-slot-20 `load_activate` (which reads the cursor [dialog+0xb0c] -- NOT an
// arg), lets the NATIVE menu pump tick the registered selector step 0x140826d50 (which
// populates iodev io18/io20 and runs the menu deserialize 0x14082c240 -> ac0=N + c30=real +
// character applied, b80-INDEPENDENT), then `continue_confirm` 0x140b0e180 -> SetState(5).
// All offsets VERIFIED against the on-disk decrypted exe (STAGE-2 spec 2026-06-16).
// ============================================================================================
pub(crate) use er_title_flow::OWN_STEPPER_PHASE_S2_ACTIVATE;
pub(crate) use er_title_flow::ProfileLoadMenuRva;

pub(crate) use er_title_flow::PROFILE_LOAD_DIALOG_VTABLE_RVA;



pub(crate) use er_title_flow::DIALOG_SLOT_CURSOR_B0C_OFFSET;
pub(crate) use er_title_flow::DIALOG_SLOT_BOUND_B08_OFFSET;

/// MenuWindowJob (d180) layout: +0xa8 action std::function, +0x10 dialog ctx-out (functor
/// fires only when ==0), +0x130 built-dialog result slot.
#[repr(C)]
pub(crate) struct MenuWindowJobLayout {
    pub(crate) unknown_000: [u8; 0x10],
    pub(crate) dialog_context: usize,
    pub(crate) unknown_018: [u8; 0x90],
    pub(crate) action_functor: usize,
    pub(crate) unknown_0b0: [u8; 0x80],
    pub(crate) dialog_result: usize,
}

pub(crate) const MENU_ITEM_FUNCTOR_A8_OFFSET: usize =
    core::mem::offset_of!(MenuWindowJobLayout, action_functor);
pub(crate) const MENU_ITEM_CTX_10_OFFSET: usize =
    core::mem::offset_of!(MenuWindowJobLayout, dialog_context);
pub(crate) const MENU_ITEM_DIALOG_RESULT_130_OFFSET: usize =
    core::mem::offset_of!(MenuWindowJobLayout, dialog_result);
/// Main-title Continue row action `_Do_call` thunk. This is the `+0xa8` action on the
/// first focused MenuWindowJob after native `TitleTopDialog::open_menu`; it builds the native
/// row result consumed by the FD4 menu submit helper, not a save-load/direct-confirm shortcut.
pub(crate) const MENU_TITLE_CONTINUE_DOCALL_RVA: usize = 0x00764b80;
/// Native FD4 row submit helper used by `MenuWindowJob::Update` for one result-mode branch.
/// It forwards event `3` to the row result's own vtable slot `+0x60`.
/// `f(rcx = MenuWindow*)`: calls `MenuJobResult::SetResult(&r, Failed=3, 0)` then invokes the
/// receiver's OWN vtable slot +0x60. It is a close-with-Failed, NOT an item submit or accept
/// (Success is 2; the sibling emits 4). Its caller is `CS::MenuWindowJob::Run`, not `::Update`.
/// Renamed 2026-08-01 -- the old name and doc asserted three things the dump contradicts.
pub(crate) const MENU_WINDOW_CLOSE_WITH_FAILED_RVA: usize =
    er_game_base::rva::MENU_WINDOW_CLOSE_WITH_FAILED_RVA;
/// Row-result field consumed by `MenuWindowJob::Update` to choose which native accept event branch
/// to send to the built row result.
pub(crate) const MENU_ITEM_RESULT_MODE_58_OFFSET: usize = 0x58;
/// Row-result virtual event handler slot. Both native accept branches dispatch through this slot.
pub(crate) const MENU_ITEM_RESULT_EVENT_SLOT_60_OFFSET: usize = 0x60;
/// Tiny FD4 event constructor: writes `{ code: edx, payload: r8d }` to the output slot.
pub(crate) const FD4_EVENT_CONSTRUCTOR_RVA: usize = 0x007a91e0;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MENU_ITEM_RESULT_MODE_EVENT3: i32 = 1;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MENU_ITEM_RESULT_MODE_EVENT4: i32 = 2;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MENU_ITEM_RESULT_EVENT4_CODE: i32 = 4;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MENU_ITEM_RESULT_EVENT4_PAYLOAD: i32 = -1;
/// GameMan+0xc30 new-game DEFAULT map (m10_01_00_00). The mount writes the slot's REAL map
/// here; for a NON-m10 char `c30 != this` corroborates the mount (for an m10 char it is
/// ambiguous -- ac0 is the primary mount oracle). Packed mAA_BB_CC_DD.
#[repr(i32)]
pub(crate) enum GameManMapId {
    NewGameDefault = 0x0a01_0000,
}

pub(crate) const GAME_MAN_NEWGAME_DEFAULT_MAP: i32 = GameManMapId::NewGameDefault as i32;
/// STAGE 2 invocation is gated by concrete menu/action/dialog readiness, not by a fixed
/// post-open settle frame count.
/// Wall-clock fail-safe per S2 phase before failing closed (stay at the menu, NO SetState(5),
/// NO write). Readiness is still semantic (`ProfileLoadDialog`, selector tick, mount latch, char
/// fingerprint), not elapsed time.
pub(crate) const OWN_STEPPER_S2_PHASE_MAX: u64 = OWN_STEPPER_S2_PHASE_TIMEOUT_MS;
/// Per-phase poll counter for S2 diagnostics/log throttling, not a readiness gate.
pub(crate) static OWN_STEPPER_S2_WAITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) use er_title_flow::OWN_STEPPER_DIALOG;
/// The CS::MenuJobWithContext<LoadJobContext> selector step (vtable 0x142ac71e0) that
/// load_activate 0x1409a4670 builds at `dialog+0x18`. A cold standalone dialog is not ticked by
/// the MENU task-group, so STAGE 2 reads this and SELF-PUMPS the tick 0x140826d50 each frame
/// (installer -> io18/io20 full-save read -> menu_deser 0x14082c240 -> mount).
pub(crate) static OWN_STEPPER_SELECTOR_STEP: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The selector tick context observed at builder `owner+0xf8`; natural selector_tick calls use this
/// as arg2 while arg1 is the heap selector step stored at `[owner]` by builder 0x140826510.
pub(crate) static OWN_STEPPER_SELECTOR_CTX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// One-shot guard: fire the deserialize 0x67b290 exactly once when the full-save read is resident.
/// State machine for the shared DESER latch: NOT_FIRED -> {FIRED_FAIL, FIRED_OK}. Used by both
/// the STAGE2 mount and cold_char_mount_drive's DESER phase so the result is observable.
#[repr(usize)]
pub(crate) enum OwnStepperDeserState {
    NotFired,
    FiredFail,
    FiredOk,
}

pub(crate) static OWN_STEPPER_DESER_FIRED: AtomicUsize =
    AtomicUsize::new(OWN_STEPPER_DESER_NOT_FIRED);
pub(crate) const OWN_STEPPER_DESER_NOT_FIRED: usize = OwnStepperDeserState::NotFired as usize;
pub(crate) const OWN_STEPPER_DESER_FIRED_FAIL: usize = OwnStepperDeserState::FiredFail as usize;
pub(crate) const OWN_STEPPER_DESER_FIRED_OK: usize = OwnStepperDeserState::FiredOk as usize;
/// deserialize 0x67b290 success return code (ret==1 == real char applied + c30 written from save).
pub(crate) const OWN_STEPPER_DESER_SUCCESS_RET: i32 = true as i32;
pub(crate) use er_title_flow::OWN_STEPPER_TITLE_FIRED;
/// The RESOLVED target slot the mount is expected to land on: the configured `slot=N` if
/// >=0, else (slot=-1 "most-recent") the dialog's natural highlight cursor read live at
/// > PHASE_S2_ACTIVATE. MOUNT_POLL/CONFIRM compare `GameMan+0xac0` against this.
pub(crate) static OWN_STEPPER_EXPECTED_SLOT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(OWN_STEPPER_SLOT_NONE);
/// Latched real GameMan+0xc30 at the moment the mount is detected; re-read & required-equal
/// at PHASE_S2_CONFIRM (the save-write guard).
pub(crate) static OWN_STEPPER_MOUNT_C30: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(GAME_MAN_C30_UNSET);
/// Latch: the iodev request pair (io18 & io20) was observed non-null at least once -- so
/// "io18==0 && io20==0" means "request consumed/mounted", not "never started".
pub(crate) static OWN_STEPPER_IO_WAS_SET: AtomicUsize = AtomicUsize::new(OWN_STEPPER_IO_WAS_SET_NO);
pub(crate) const OWN_STEPPER_IO_WAS_SET_NO: usize = false as usize;
pub(crate) const OWN_STEPPER_IO_WAS_SET_YES: usize = true as usize;
/// One-shot latch so PHASE_S2_INVOKE hand-invokes the functor at most once.
pub(crate) static OWN_STEPPER_INVOKED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_FALSE as usize);
/// One-shot latch so PHASE_S2_CONFIRM fires SetState(5) at most once.
pub(crate) static OWN_STEPPER_CONFIRMED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_FALSE as usize);
