// ============================================================================================
// OWNERSHIP LEDGER -- conservation oracle for the "took a native object, released it one-sidedly"
// bug class (the repeated-switch spared-renderer leak: we excluded a CSMenuProfModelRend from the
// engine's delete to render the portrait, then a bare `store(0)` dropped our responsibility for it
// without discharging it, leaking one live renderer per switch). A raw `usize` in an AtomicUsize
// carries no ownership semantics, so `store(0)` reads as innocuous. This ledger makes ownership
// CONSERVATION observable: every "take" (we become responsible for freeing a native object) and
// "release" (we hand it back to the native lifecycle) is counted per class, and a per-switch check
// asserts outstanding <= bound. The old leak would have tripped this at switch #2 (outstanding
// climbing 1->2->3->4) instead of crashing the GX queue at #4. It is also the acceptance test for a
// future RAII `EngineOwned` wrapper: build the invariant first, then make it structurally unbreakable.
// ============================================================================================
/// Classes of native object we take manual ownership of. Extend as the RAII wrapper subsumes more of
/// the spare/pin family; only classes with a TRUE release obligation belong here (borrowed engine
/// pointers -- the RT/depth pins, the anim-bound renderer -- are observation, not ownership).
#[derive(Clone, Copy)]
pub(crate) enum OwnedClass {
    /// The teardown-spared portrait renderer (excluded from the native delete; we must delete it).
    SparedRenderer = 0,
}
pub(crate) const OWNED_CLASS_COUNT: usize = 1;
pub(crate) const OWNED_CLASS_NAMES: [&str; OWNED_CLASS_COUNT] = ["spared_renderer"];
/// Max simultaneously outstanding (taken-but-not-released) per class. The spare holds exactly one
/// renderer per load window; the game-thread drain releases the prior before taking the next, so
/// outstanding never legitimately exceeds 1.
pub(crate) const OWNED_CLASS_BOUND: [usize; OWNED_CLASS_COUNT] = [1];
pub(crate) static OWNED_TAKEN: [AtomicUsize; OWNED_CLASS_COUNT] =
    [const { AtomicUsize::new(0) }; OWNED_CLASS_COUNT];
pub(crate) static OWNED_RELEASED: [AtomicUsize; OWNED_CLASS_COUNT] =
    [const { AtomicUsize::new(0) }; OWNED_CLASS_COUNT];
/// Per-class high-water of outstanding (should equal the bound in a healthy run, exceed it on a leak).
pub(crate) static OWNED_MAX_OUTSTANDING: [AtomicUsize; OWNED_CLASS_COUNT] =
    [const { AtomicUsize::new(0) }; OWNED_CLASS_COUNT];
/// Total ledger-check violations observed (outstanding > bound). Nonzero == a taken-without-release
/// leak of a native-owned object -- the run-stopping oracle for this bug class.
pub(crate) static OWNED_LEDGER_VIOLATIONS: AtomicUsize = AtomicUsize::new(0);

/// Gate-local `CS::MenuWindowJob::Run` hook state. `MENU_WINDOW_JOB_RUN_RVA` is defined with the
/// title-cover constants above; System Quit reuses that same live/deobf target.
pub(crate) static SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_INSTALLED_YES: usize = 1;
pub(crate) static SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_INGAME_TOP_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_OPTION_SETTING_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_SELECT_WINDOW: AtomicUsize = AtomicUsize::new(0);
/// Latched from the moment the user clicks System->Quit->Load Profile (the profile-load route FIRE) until
/// ProfileSelect is reset. `SYSTEM_QUIT_PROFILE_SELECT_WINDOW` is only set later, in the MenuWindowJob::Run
/// hook, so there is a window where the own_stepper self-pump builds the native load-confirm MessageBox
/// while that var is still 0 -- the confirm then escapes msgbox suppression and CRASHES the game (2026-07-15).
/// This flag spans the whole flow so `switch_active` in the msgbox builder hook covers that gap.
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_HIDE_REAL_WINDOWS_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_RESTORE_REAL_WINDOWS_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_SKIP_RESTORE_AFTER_QUICKLOAD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_REAL_WINDOWS_HIDDEN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_WINDOW_LIST_PUSH_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_WINDOW_LIST_PUSH_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SYSTEM_QUIT_WINDOW_LIST_PUSH_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_WINDOW_LIST_PUSH_INSTALLED_YES: usize = 1;
/// Live/deobf `CS::ProfileLoadDialog` activation vtable target (`dump 0x1409a47c0` -> deobf
/// `0x1409a4670`). This builds/submits the native confirmation dialog for the selected profile.
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_RVA: u32 = 0x9a4670;
/// Live/deobf `<lambda_4c99...>::operator()` (`dump 0x1409a4ee0` -> deobf `0x1409a4d90`). This
/// only writes `*(dialog+0x1cc8+0x14c)=2` and `dialog+0x1e8=Success`; runtime evidence showed the
/// crash happens before this lambda is reached when the confirmation is accepted, so this transition
/// is safe to allow after blocking the actual load job.
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_RVA: u32 = 0x9a4d90;
/// Live/deobf `CS::MenuJobWithContext<LoadJobContext,...>::Run` (`dump 0x140826e40` -> deobf
/// `0x140826d50`). This is the load job queued behind the native confirmation dialog; accepting
/// confirmation reaches this job and then crashed at CSGaitemImp::Deserialize live/deobf `0x14067141a`.
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_RVA: u32 = 0x826d50;
/// Native MenuWindow close-finalize `FUN_1407ac980` (dump `0x1407ac980` -> live/deobf `0x7ac890`,
/// content-unique). Takes `rcx = MenuWindow*`; does `MenuJobResult::SetResult(&r, Failed, 0)` then
/// invokes the window's own close vmethod (`window->vtable+0x60`). Calling it on the ProfileSelect
/// window sets `owningMenuWindow+0x1e8` terminal, so `CS::MenuWindowJob::Run` (0x7ad1c0) reads a
/// terminal result and `ExecuteMenuJob` (0x7a96f0) pops the job from the menu-job queue head. That
/// is the native cancel/back close (approach B): it clears `queue[0]` so the return-title chain's
/// `queue[0]==0` ready-gate finally passes and the direct chain can submit. See bd
/// `system-quit-profileselect-native-close-B-path` / `menu-job-queue-pump-dequeue-mechanism`.
pub(crate) const SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA: u32 = 0x7ac890;
/// Native ProfileLoadDialog in-place list rebuild `FUN_1409a5020` (dump `0x1409a5020` -> live/deobf
/// `0x9a4ed0`, content-unique via dump-deobf-shift). `fn(rcx = dialog)`. The game's own
/// records-changed refresh, used by the delete-save flow: re-runs the item-list builder
/// `FUN_140875680` (fresh `GetProfileSummary()` re-read of the live records), copies the new list
/// into `dialog+0x1260`, and rebinds via `FUN_1409a2e40` -- which rewrites the row count at
/// `+0xb08`, re-selects a valid cursor, and unconditionally re-decorates every visible row. This
/// is the sanctioned way to change row text while the 05_010 window stays open (the decorate pass
/// reads per-row SNAPSHOTS, so bare record writes are invisible without this rebuild). RE 2026-07-07,
/// adversarially verified (see bd save-picker RE notes).
pub(crate) const PROFILE_LOAD_DIALOG_LIST_REBUILD_RVA: u32 = 0x9a4ed0;
/// One-shot latch: set when we have invoked the native ProfileSelect close during a return-title
/// transition, so the per-tick handler closes it exactly once. Reset with the ProfileSelect state.
pub(crate) static SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED: AtomicUsize = AtomicUsize::new(0);
/// Telemetry: number of native ProfileSelect close-finalize calls issued (expected 1 per flow).
pub(crate) static SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The load-ONLY save routine `FUN_14067b380` (dump 0x14067b380 -> LIVE/deobf 0x67b290, shift -0xf0),
/// called by `CS::MoveMapStep::DoSaveStuff` when `GameMan.saveState/b80 == 2`: it reads the slot's save
/// file and runs `PlayerGameData::Deserialize -> CSGaitemImp::Deserialize` (the in-world deserialize
/// that crashes at live 0x67141a), then `warpRequested=true`. Guarded during the in-world
/// System->Quit->Load-Profile transition so the picked slot is NOT deserialized into the still-live
/// world; forwarded normally at a clean title so the autoload loads the slot. Distinct from the SAVE
/// path (DoSaveStuff `IsSaveState1` branch), so the return-title's save-on-quit is untouched. NOTE:
/// the RVA is the LIVE 0x67b290 (game_rva uses the deobf base); the dump 0x67b380 is a DIFFERENT
/// function -- hooking it silently no-ops (observed 2026-07-01: guard installed but never fired).
pub(crate) const SYSTEM_QUIT_INWORLD_LOAD_RVA: u32 = 0x67b290;
pub(crate) static SYSTEM_QUIT_INWORLD_LOAD_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_INWORLD_LOAD_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_INWORLD_LOAD_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_INWORLD_LOAD_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Count of frames the menu-pump Run hook forced GameMan.saveState/b80 back to idle to abort a
/// half-started in-world load transition so the queued return-title chain can run.
pub(crate) static SYSTEM_QUIT_INWORLD_LOAD_ABORT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `CS::GameMan::RequestLoadSlot(slot)` -- the native setter that transitions GameMan.saveState/b80
/// 0->2 to REQUEST an in-world load of an explicit slot 0-9 (dump `FUN_14067b2f0` -> LIVE/deobf
/// `0x67b200`, shift -0xf0, content-unique). It validates the slot's ProfileSummary then calls the
/// common arm worker `FUN_140e6ec30(mgr, slot, 0)` and, on success, writes `GLOBAL_GameMan->saveState
/// = 2`. Called from the per-frame MoveMapStep load steps (`STEP_LoadSaveData`, `FUN_140afb970`) once
/// the confirmed ProfileSelect chain pushes the map machine into loading -- INDEPENDENT of our load-job
/// block, which is why blocking the load-job/confirm never stopped the arm. Setting saveState=2 both
/// makes `DoSaveStuff` deserialize (guarded) AND starts the 02_904_NowLoading transition that freezes
/// the menu pump so the queued return-title chain can never run (observed 2026-07-01: bc4=0,
/// functor_call_count=0, player present; the reactive abort is TOO LATE because NowLoading commits in
/// the same frame). During the in-world switch we neutralize this at the source so saveState never
/// reaches 2. NOTE distinct from the Continue/boot variants FUN_14067b290 (sentinel slot 10) and
/// FUN_14067b570 (sentinel slot 0xb): those arm the boot/clean-title autoload and must NOT be blocked.
/// See bd system-quit-loadjob-success-commits-phantom-load-2026-07-01.
pub(crate) const SYSTEM_QUIT_REQUEST_LOAD_SLOT_RVA: u32 = 0x67b200;
pub(crate) static SYSTEM_QUIT_REQUEST_LOAD_SLOT_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_REQUEST_LOAD_SLOT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Count of in-world load requests we neutralized (returned "not armed") during the switch so
/// GameMan.saveState/b80 stayed 0 and no NowLoading transition started.
pub(crate) static SYSTEM_QUIT_REQUEST_LOAD_SLOT_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_REQUEST_LOAD_SLOT_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_GAITEM_DESERIALIZE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_GAITEM_LOOKUP_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_GAITEM_FINALIZE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ADDR: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_DESERIALIZE_ADDR: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_LOOKUP_ADDR: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_FINALIZE_ADDR: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_DISABLED: usize = 2;
pub(crate) const SYSTEM_QUIT_GAITEM_DESERIALIZE_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_GAITEM_DESERIALIZE_DISABLED: usize = 2;
pub(crate) const SYSTEM_QUIT_GAITEM_LOOKUP_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_GAITEM_LOOKUP_DISABLED: usize = 2;
pub(crate) const SYSTEM_QUIT_GAITEM_FINALIZE_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_GAITEM_FINALIZE_DISABLED: usize = 2;
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_DESERIALIZE_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_DESERIALIZE_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Times the CSGaitemImp singleton was reset to pristine right before a switch-reload's native deserialize
/// (clears char#1's stale items so char#2's deserialize does not dispatch a freed vtable -> the 0x67141a AV).
pub(crate) static SYSTEM_QUIT_GAITEM_DESERIALIZE_RESET_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_LOOKUP_EMPTY_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_LOOKUP_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_FINALIZE_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_GAITEM_FINALIZE_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_JOB: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_LIST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_PROFILE_ID: AtomicUsize =
    AtomicUsize::new(usize::MAX);
/// Captured fourth constructor argument for native ProfileSelect LoadJob builder, mirrored from the
/// consumed LoadJobContext (`job+0x60`, originally `*(ProfileLoadDialog+0x1cc8)`).
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_CONTEXT_ARG: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_POST_RETURN_TITLE_FIRED: AtomicUsize =
    AtomicUsize::new(0);
/// Native return-title semantic request used after System->ProfileSelect confirmation. This is
/// `FUN_14067a490` in the Ghidra dump and maps to live/deobf `0x14067a3a0`; it sets the same
/// GameMan return-title/save flags used by the normal Quit Game confirmation callback without
/// displaying another confirmation dialog.
pub(crate) const SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA: u32 = 0x67a3a0;
/// Guard on the native title Continue confirm `0x140b0e180` (`CONTINUE_CONFIRM_RVA`): it only reads
/// GameMan+0xc30 -> owner+0xbc -> SetState(5) and picks NO slot, so after a System->Quit switch the
/// clean-title reload would re-stream the PRE-SWITCH GameMan/PlayerGameData state (no fresh
/// deserialize of the picked slot runs anywhere on that native path -- static RE 2026-07-02, bd
/// system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02). While a switch is
/// active the hook drives ONE synchronous feed-deserialize of the PICKED slot
/// (`own_load_feed_deserialize`) before forwarding, so ac0/c30/PGD all become the picked slot and
/// the confirm streams the right character. Installed UNCONDITIONALLY at attach (single MinHook per
/// address: this hook also carries the continue-trace `CAP continue_confirm` logging that used to be
/// a separate trace-set hook -- same precedent as `install_c30_writer_hook`).
pub(crate) static SYSTEM_QUIT_CONTINUE_CONFIRM_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_CONTINUE_CONFIRM_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static START_SYSTEM_QUIT_CONTINUE_CONFIRM_HOOK: Once = Once::new();
pub(crate) static START_SYSTEM_QUIT_CHILD_FINISH_TRACE_HOOK: Once = Once::new();
/// One-shot per armed switch: 0 = the fresh picked-slot deserialize has not yet run for the active
/// System->Quit switch (reset by `system_quit_arm_quickload_autoload`); 1 = it succeeded and the
/// confirm may stream. While 0, any confirm during an active switch first drives the deserialize.
pub(crate) static SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE: AtomicUsize = AtomicUsize::new(0);
/// Count of successful fresh picked-slot deserializes driven by the confirm hook (product proof
/// expects exactly 1 per switch).
pub(crate) static SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Count of confirms BLOCKED fail-closed because the fresh deserialize could not be proven (no save
/// bytes / parse failed / fingerprint not real). Streaming stale state would load the wrong
/// character and the post-load autosave would then write it back to the picked slot.
pub(crate) static SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Count of confirms forwarded to the native original (boot autoload, normal play, or post-deser).
pub(crate) static SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE: usize = 0;
pub(crate) const SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED: usize = 1;
pub(crate) const SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED: usize = 2;
pub(crate) const SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN: usize = 3;
pub(crate) const SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF: usize = 4;
pub(crate) static SYSTEM_QUIT_QUICKLOAD_PHASE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT: AtomicUsize =
    AtomicUsize::new(0);
/// Native return-title final functor (`FUN_1407a3990` dump -> live/deobf `0x1407a3900`).
/// It sets `CSMenuMan->menuData+0x5d` and `DAT_143d6c5e8`, which request the real title/menu rebuild.
pub(crate) const SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_RVA: u32 = 0x7a3900;
