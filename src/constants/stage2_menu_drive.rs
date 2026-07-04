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
/// PHASE 6 (S2 INVOKE): fire d180's +0xa8 action functor to build the ProfileLoadDialog.
pub(crate) const OWN_STEPPER_PHASE_S2_INVOKE: usize = OwnStepperPhase::S2Invoke as usize;
/// PHASE 7 (S2 ACTIVATE): write the slot cursor [dialog+0xb0c]=N (bounds [dialog+0xb08]) then
/// call the dialog's vtable-slot-20 load_activate(rcx=dialog), registering the selector step.
pub(crate) const OWN_STEPPER_PHASE_S2_ACTIVATE: usize = OwnStepperPhase::S2Activate as usize;
/// PHASE 8 (S2 MOUNT_POLL): pass-through each frame so the native pump ticks the selector;
/// watch for the mount (ac0==N + io18/io20 set->clear; c30 leaving the new-game default).
pub(crate) const OWN_STEPPER_PHASE_S2_MOUNT_POLL: usize = OwnStepperPhase::S2MountPoll as usize;
/// PHASE 9 (S2 CONFIRM): guard (ac0==N && c30==latched-mount && io consumed) then
/// continue_confirm -> SetState(5) so the native pump streams the real world. The ONLY
/// save-write-risking step; gated entirely by a verified real mount (fail-closed otherwise).
pub(crate) const OWN_STEPPER_PHASE_S2_CONFIRM: usize = OwnStepperPhase::S2Confirm as usize;
#[repr(usize)]
pub(crate) enum ProfileLoadMenuRva {
    ProfileSlotActivate = 0x262250,
    MenuItemUpdate = 0x007ad1c0,
    ProfileLoadSelectorTick = 0x826d50,
    MenuDeser = 0x0082c240,
    CsMenuCtor = 0x009060d0,
    MenuMemberFuncJobRun = 0x9aaba0,
    MenuLoadGameFunctorVtable = 0x02ac3ea8,
    SelectorStepVtable = 0x2ac71e0,
    ProfileLoadDialogVtable = 0x2b229f8,
}

/// CS::ProfileLoadDialog vtable (RVA). The dialog built by d180's functor (dialog_factory
/// 0x14081ead0 -> ctor 0x1409a3d90 writes this vtable). Used to VALIDATE the built dialog
/// before any dialog call (a wrong this-pointer would AV).
pub(crate) const PROFILE_LOAD_DIALOG_VTABLE_RVA: usize =
    ProfileLoadMenuRva::ProfileLoadDialogVtable as usize;
/// Dialog vtable slot 20 (offset 0xa0) = load_activate 0x1409a4670. Read the live slot from
/// the dialog vtable (robust to relocation) rather than hard-calling the RVA.
#[repr(C)]
pub(crate) struct ProfileLoadDialogVtableLayout {
    pub(crate) unknown_slots_00_19: [usize; 20],
    pub(crate) load_activate: usize,
}

pub(crate) const DIALOG_LOAD_ACTIVATE_VTSLOT_A0_OFFSET: usize =
    core::mem::offset_of!(ProfileLoadDialogVtableLayout, load_activate);

#[repr(C)]
pub(crate) struct ProfileLoadDialogLayout {
    pub(crate) unknown_000: [u8; 0xb08],
    pub(crate) slot_bound: i32,
    pub(crate) slot_cursor: i32,
    pub(crate) unknown_b10: [u8; 0x11b8],
    pub(crate) load_job_ctx: usize,
}

/// Dialog selected-list-index cursor (= [dialog+0xa38+0xd4]); load_activate reads it as the
/// slot. WRITE the desired slot N here before calling load_activate.
pub(crate) const DIALOG_SLOT_CURSOR_B0C_OFFSET: usize =
    core::mem::offset_of!(ProfileLoadDialogLayout, slot_cursor);
/// Dialog list inclusive upper bound; load_activate clamps the cursor to [0, bound).
pub(crate) const DIALOG_SLOT_BOUND_B08_OFFSET: usize =
    core::mem::offset_of!(ProfileLoadDialogLayout, slot_bound);

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
pub(crate) const MENU_ITEM_SUBMIT_RVA: usize = 0x007ac890;
/// Row-result field consumed by `MenuWindowJob::Update` to choose which native accept event branch
/// to send to the built row result.
pub(crate) const MENU_ITEM_RESULT_MODE_58_OFFSET: usize = 0x58;
/// Row-result virtual event handler slot. Both native accept branches dispatch through this slot.
pub(crate) const MENU_ITEM_RESULT_EVENT_SLOT_60_OFFSET: usize = 0x60;
/// Tiny FD4 event constructor: writes `{ code: edx, payload: r8d }` to the output slot.
pub(crate) const FD4_EVENT_CONSTRUCTOR_RVA: usize = 0x007a91e0;
pub(crate) const MENU_ITEM_RESULT_MODE_EVENT3: i32 = 1;
pub(crate) const MENU_ITEM_RESULT_MODE_EVENT4: i32 = 2;
pub(crate) const MENU_ITEM_RESULT_EVENT4_CODE: i32 = 4;
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
/// The built+validated ProfileLoadDialog pointer (0 until PHASE_S2_INVOKE succeeds).
pub(crate) static OWN_STEPPER_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
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
/// One-shot latch: set once the zero-input title-confirm fire (fire_titletop_load_entry) has
/// fired the Load-Game row action, so it is not re-fired while the ProfileLoadDialog builds.
pub(crate) static OWN_STEPPER_TITLE_FIRED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);
/// The RESOLVED target slot the mount is expected to land on: the configured `slot=N` if
/// >=0, else (slot=-1 "most-recent") the dialog's natural highlight cursor read live at
/// PHASE_S2_ACTIVATE. MOUNT_POLL/CONFIRM compare `GameMan+0xac0` against this.
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
