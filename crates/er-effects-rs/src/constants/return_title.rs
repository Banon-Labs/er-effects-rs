// ---- Return-title "rebuild the title" request flags set by the final functor (0x7a3900) ----
// The functor does `*(*([GLOBAL_CSMenuMan]+0x8)+0x5d)=1` and `*(0x143d6c5e8)=1`. These are LEVEL
// flags (not edge-consumed); we set them to tear down the OLD char for the switch, but nothing
// resets them, so once the reloaded character's world comes up the still-set +0x5d re-requests the
// quit-to-title -> GameMan.save_requested flips true again (~3.6s post-load, proven by the gm-snap
// trace) -> a second save + SetState(2) bounces the freshly-loaded world back to the title. We clear
// both once the reload commits (continue_confirm), which is after the teardown they were needed for.
/// (`CS_MENU_MAN_GLOBAL_RVA` = `[GLOBAL_CSMenuMan]` pointer global is already defined above.)
/// `CSMenuManImp::menuData` pointer at CSMenuMan+0x8.
pub(crate) const CS_MENU_MAN_MENU_DATA_OFFSET: usize = 0x8;
pub(crate) use er_title_flow::CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET;
pub(crate) use er_title_flow::CS_MENU_DATA_ENDING_FLAG_5E_OFFSET;
pub(crate) use er_title_flow::ENDING_REQUEST_FORCE_FLAG_3D856A0_RVA;
pub(crate) use er_title_flow::GAME_MAN_ENDING_FLAG_B7C_OFFSET;
pub(crate) use er_title_flow::GAME_MAN_ENDING_FLAG_B7D_OFFSET;
pub(crate) use er_title_flow::GAME_MAN_WARP_REQUESTED_10_OFFSET;
/// `DAT_143d6c5e8` companion rebuild flag (data RVA). No readers found in the dump, but cleared for
/// symmetry so we fully undo what the final functor set.
pub(crate) const RETURN_TITLE_REBUILD_FLAG_DAT_RVA: usize = 0x3d6c5e8;
pub(crate) use er_title_flow::CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET;
// ---- In-game session liveness gate (the post-reload bounce decision, static RE 2026-07-02) ----
// TitleStep state 6 (STEP_GameStepWait, dump 0x140b0ced0) exits to the quit-to-title transition
// (SetState(2) -> BeginLogo -> BeginTitle -> MenuJobWait) the first tick it sees
// `InGameStep->requestCode == 0`. The request-code register (InGameStep+0xd8, int) lifecycle:
// ctor=0; RequestMoveMap (dump 0x140aebeb0, called by STEP_PlayGame for the initial world load with
// the map from TitleStep+0xbc) =1; STEP_MoveMap_Update (dump 0x140aec810) =2 when the map move's
// child MoveMapStep finishes; STEP_RequestWait (dump 0x140aecd00) at ==2 waits for the in-game menu
// job qword at CSMenuMan+0x798 to be nonzero -- while it IS nonzero the session idles at code 2
// (the stable in-world state); if that qword reads 0 it writes the request code to 0, which is what
// STEP_GameStepWait converts into the return-to-title. So a reloaded world only STAYS up if
// CSMenuMan+0x798 is (re)populated after the load.
pub(crate) use er_title_flow::TITLE_STEP_IN_GAME_STEP_2E8_OFFSET;
pub(crate) use er_title_flow::IN_GAME_STEP_REQUEST_CODE_D8_OFFSET;
/// `CS::EzChildStepBase::RequestFinish` (dump `0x140eb5590` -> live `0x140eb5570`, shift -0x20,
/// content-unique). One-shot: calls the wrapper's CSSetFinishHelper virtual (which sets the child
/// step's finish-requested byte at child+0xb4) then latches wrapper+0x10. The quit-to-title
/// teardown ends the in-world MoveMapStep session through here; the post-switch reload bounce is
/// this firing against the FRESH MoveMapStep child right after streaming completes. Read-only
/// trace hook logs every call + caller RVA to identify the stale requester.
pub(crate) const EZ_CHILD_STEP_REQUEST_FINISH_RVA: u32 = EZ_CHILDSTEP_REQUEST_FINISH_RVA as u32;
/// `EzChildStep<MoveMapStep>` wrapper offset inside `InGameStep` (ctor dump 0x140aeabf3).
pub(crate) const IN_GAME_STEP_MOVE_MAP_WRAPPER_E0_OFFSET: usize = 0xe0;
/// `EzChildStep<InGameStayStep>` wrapper offset inside `InGameStep` (ctor dump 0x140aeabc3).
pub(crate) const IN_GAME_STEP_STAY_WRAPPER_B8_OFFSET: usize = 0xb8;
/// `EzChildStepBase::stepper` (the owned child step object) at wrapper+0x8; the finish latch byte
/// is wrapper+0x10 and the CSSetFinishHelper pointer wrapper+0x18 (dump 0x140eb5590 decompile).
pub(crate) const EZ_CHILD_STEP_STEPPER_OFFSET: usize = 0x8;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_CHILD_FINISH_TRACE_ORIG;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_CHILD_FINISH_TRACE_INSTALLED;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_CHILD_FINISH_TRACE_COUNT;
pub(crate) use er_title_flow::SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT;
/// Count of quick-load handoffs that invoked the original native Quit Game row action trampoline
/// instead of the low-level accepted callback alone. This is an experiment to test whether the full
/// native return-title menu-job chain is the missing teardown boundary.
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_QUICKLOAD_NATIVE_QUIT_ACTION_COUNT;
pub(crate) use er_title_flow::SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT;
/// Count of frames we cleared a stale `CSMenuMan->disableSaveMenu` during an active switch (the switch-2
/// quit-save gate; see [`CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET`]). Non-zero on a switch == that switch's
/// quit-save was being blocked and we unblocked it (the runtime semaphore for this fix).
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_DISABLE_SAVE_MENU_CLEAR_COUNT;
/// Rate-limit counter for the switch-2 save-gate diagnostic (which of the save orchestrator
/// `FUN_140afb970`'s three gates -- force latch `0x143d856a0`, `save_state`, or the CSMenuMan menu gate
/// `FUN_14080d660` -- is blocking the quit-save so `bc4` freezes at 1).
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GATE_DIAG_COUNT;
/// SWITCH-OUTCOME ORACLE (2026-07-16, user-mandated reliable semaphore). Read-only per-frame classifier of
/// a switch/load outcome so the state is ALWAYS knowable from telemetry, never from eyeballing. `_TICK` is
/// the frame counter since a switch was picked (if it STOPS advancing the game task froze = FROZE). `_STABLE`
/// is consecutive frames the game's own stable-in-world condition holds (player present + requestCode==2 +
/// in-game menu job CSMenuMan+0x798 != 0): climbing high == LOADED_STABLE; resetting to 0 after climbing ==
/// the world dropped (BOUNCED/reload). `_MAX_STABLE` latches the peak so a later drop is still visible.
pub(crate) use er_telemetry::counters::SWITCH_ORACLE_TICK;
pub(crate) use er_telemetry::counters::SWITCH_ORACLE_STABLE_FRAMES;
pub(crate) use er_telemetry::counters::SWITCH_ORACLE_MAX_STABLE_FRAMES;
/// InGameStep requestCode (`InGameStep + 0xd8`) values. 1 = a MoveMap (load) request is pending/in
/// progress; 2 = STABLE IN-WORLD (the load handoff completed, the world is settled -- player present,
/// in-game menu job populated). STEP_MoveMap_Update drains 1 -> 2 once the child finishes.
pub(crate) const INGAMESTEP_REQUEST_CODE_NONE: i32 = 0;
pub(crate) use er_title_flow::INGAMESTEP_REQUEST_CODE_MOVEMAP_PENDING;
pub(crate) use er_title_flow::INGAMESTEP_REQUEST_CODE_STABLE_IN_WORLD;
/// Human name for an InGameStep requestCode (`InGameStep + 0xd8`) value (out-of-range/unreadable -> "?").
pub(crate) fn ingamestep_request_code_name(v: i32) -> &'static str {
    match v {
        INGAMESTEP_REQUEST_CODE_NONE => "NONE",
        INGAMESTEP_REQUEST_CODE_MOVEMAP_PENDING => "MOVEMAP PENDING",
        INGAMESTEP_REQUEST_CODE_STABLE_IN_WORLD => "STABLE IN-WORLD",
        _ => "?",
    }
}
/// 3RD-LOAD ROOT SHARPENED (Ghidra 1.16.1, 2026-07-16). The softlock parks the InGameStep at
/// `InGameStep_StepperArray[7] = STEP_MoveMap_Update` (dump 0x140aec810). STEP_MoveMap_Update gates
/// its advance to step 8 (STEP_MoveMap_Finish) on `FUN_140eb5550(ezChildStepBase)` == "is the
/// MoveMapStep CHILD step finished?"; only then does it write requestCode(+0xd8)=2. On the stall the
/// child is NON-NULL (created at step 6 STEP_MoveMap_Init) but its own step machine never reaches
/// Finish, so requestCode stays 1 forever. So the true stall is INSIDE the MoveMapStep child's
/// world-load. This oracle publishes the child's current internal step so the stuck point is a RAM
/// semaphore, not an eyeball. `usize::MAX` = not sampled / no child.
pub(crate) use er_telemetry::counters::SWITCH_ORACLE_MMS_STEP;
pub(crate) use er_title_flow::SWITCH_ORACLE_REQUEST_CODE;
/// MoveMap destination BlockId + world-stable RAM semaphore offsets (RE-verified 2026-07-19,
/// bd er-effects-rs-9fmm). `InGameStep::STEP_MoveMap_Update` @0x140aec810 loads the destination block
/// after requestCode(+0xd8)=2; it reads `GameMan+0xac8` (loadTargetMapId) when
/// `CSSessionManager.protocol_state == WaitReload(4)`, else `GameMan+0x14` (moveMapStepBlockId), and
/// SKIPS the load when that BlockId == 0xffffffff -> the world reverts to title with nothing reloaded.
/// So these two fields are the destination-valid RAM semaphore for the reload's retention.
pub(crate) const GAME_MAN_MOVE_MAP_STEP_BLOCK_ID_14_OFFSET: usize = 0x14;
pub(crate) const GAME_MAN_LOAD_TARGET_MAP_ID_AC8_OFFSET: usize = 0xac8;
/// BlockId sentinel meaning "no destination" (skip the map load).
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MOVE_MAP_BLOCK_ID_NONE: u32 = 0xffff_ffff;
/// `FUN_140508d30` (dump 0x140508d30) returns `WorldChrMan.field47_0x1e524 == 2` = world genuinely
/// stable/ready -- a stronger world-ready oracle than can_move. RE-verified offset + constant.
pub(crate) const WORLD_CHR_MAN_WORLD_STABLE_1E524_OFFSET: usize = 0x1e524;
pub(crate) const WORLD_CHR_MAN_WORLD_STABLE_VALUE: i32 = 2;
/// GameMan online-state bytes. The connection-loss / network-error event handlers build their
/// "cannot connect / connection lost" GR_System_Message (whose side-effect returns to title) gated on
/// `isInOnlineMode (GameMan+0xBC8) && serverConnectionEnabled (GameMan+0xBC9)`. force_offline_connection_bytes
/// forces both to 0 each game-task frame, but that is a race: if the reload's session setup re-sets
/// BC8=1 and the handler fires before the next game-task clear, a network-error return-title reverts the
/// just-loaded world (user hypothesis 2026-07-19, bd reload-revert-likely-message-interrupt-2026-07-19).
/// Sem-traced to test whether the online flags re-enable during the reload's in-world window.
pub(crate) const GAME_MAN_IS_IN_ONLINE_MODE_BC8_OFFSET: usize = 0xBC8;
pub(crate) const GAME_MAN_SERVER_CONNECTION_ENABLED_BC9_OFFSET: usize = 0xBC9;
/// b80 (== GameMan.save_state) FSM state names for the loading-bar / logs. See the
/// `GAME_MAN_SAVE_STATE_*` / `FULLREAD_B80_RESIDENT` constants (constants::autoload_state).
pub(crate) fn load_in_progress_b80_name(v: i32) -> &'static str {
    match v {
        GAME_MAN_SAVE_STATE_IDLE => "IDLE",
        GAME_MAN_SAVE_STATE_OPENING => "OPENING",
        GAME_MAN_SAVE_STATE_READING => "READING",
        FULLREAD_B80_RESIDENT => "LOADED",
        _ => "?",
    }
}
/// Last sampled player/menu/loading-screen handoff gates for visible loading-bar sub-milestones.
pub(crate) use er_telemetry::counters::SWITCH_ORACLE_PLAYER_PRESENT;
pub(crate) use er_telemetry::counters::SWITCH_ORACLE_MENU_JOB_PRESENT;
/// MoveMapStep internal step index -> name. Order from the InGameStep-analogue registrar labels
/// (`u_MoveMapStep::STEP_*` at dump 0x142b5eb30..) and VALIDATED for 0..3 by the observed
/// `mms_state 1 MsbLoad -> 2 MsbLoadWait -> 3 WorldResWait` progression (own_stepper idx6 watch).
/// Indices >8 are best-effort (label order); the RAW index in the log is authoritative.
/// UPPERCASE (the boot-bar 5x7 font is A-Z + space only, and it doubles as the bar's phase label).
pub(crate) const MOVEMAPSTEP_STEP_NAMES: [&str; 21] = [
    "MAP LOAD START",     // 0  (BeginInit)
    "LOADING MAP LAYOUT", // 1  (MSB = map-layout file load)
    "MAP LAYOUT WAIT",    // 2  (MSB load wait)
    "STREAMING ASSETS",   // 3  <- classic world-resource streaming wait (resmgr+0xb7c1 gate)
    "LOADING DETAIL",     // 4  (current LOD block)
    "ENDING SESSION",     // 5  <- network/session step (stale session state suspect on a switch)
    "NETWORK SIGN IN",    // 6
    "SIGN IN WAIT",       // 7  (sign-in wait load)
    "CHARACTER SYNC",     // 8  (wait chr type sync)  [best-effort >8]
    "BUILDING RENDER",    // 9  (create draw plan)    [best-effort >8]
    "INIT ANIMATION",     // 10                        [best-effort >8]
    "PHYSICS GRID",       // 11 (fixed grid init)      [best-effort >8]
    "DEATH CHECK",        // 12 (escape death loop)    [best-effort >8]
    "COLLISION SETTLE",   // 13 (hit stabilize wait)   [best-effort >8]
    "COLLISION SETTLE",   // 14 (hit stabilize wait)   [best-effort >8]
    "COLLISION SETTLE",   // 15 (hit stabilize wait)   [best-effort >8]
    "TEXTURE SETTLE",     // 16 (tex stabilize wait)   [best-effort >8]
    "MOUNT LOAD",         // 17 (horse/Torrent wait)   [best-effort >8]
    "PLACING IN WORLD",   // 18 (MoveMap)
    "CLEANUP",            // 19
    "MAP LOAD DONE",      // 20 (Finish)
];
/// Name a MoveMapStep child step index (out-of-range -> "?").
pub(crate) fn movemapstep_step_name(idx: i32) -> &'static str {
    if idx >= 0 && (idx as usize) < MOVEMAPSTEP_STEP_NAMES.len() {
        MOVEMAPSTEP_STEP_NAMES[idx as usize]
    } else {
        "?"
    }
}

/// Terminal step of the MoveMapStep child's own sequence.
pub(crate) const MOVEMAPSTEP_STEP_FINISH_INDEX: usize = 20;

/// The InGameStep request code at `InGameStep+0xd8`, which gates the load AFTER the MoveMapStep
/// child's own FINISH (20). RE grounding (2026-07-19, InGameStep step table 0x143d70190:
/// STEP_MoveMap_Init -> STEP_MoveMap_Update -> STEP_MoveMap_Finish): the MoveMapStep child (steps
/// 0..20) runs entirely INSIDE the InGameStep's STEP_MoveMap_Update, so the child reaching FINISH is
/// NOT genuine world readiness -- the InGameStep must still advance STEP_MoveMap_Update ->
/// STEP_MoveMap_Finish (this code draining 1 -> 2) and then hand off to the resident in-world step.
///
/// Those two post-FINISH stages are real load progress the bar must show (user 2026-07-19: "when we
/// are at the Nth step, it is really N+1 -- add an Nth loading step"). They are SUBSTEPS of the
/// ENTERING WORLD phase (`MAP STEP DONE` -> `WORLD HANDOFF` -> `PLAYER IN WORLD` -> `CAN MOVE`), not
/// main phases: promoting them to main steps gave the bar a second, longer `N/M` denominator that
/// swapped in mid-load and let a STALE previous-epoch request code read as near-complete progress the
/// instant a reload armed (bd er-effects-rs-ok8d).
pub(crate) const INGAMESTEP_REQUEST_CODE_MAP_FINISH: i32 = 1;
pub(crate) const INGAMESTEP_REQUEST_CODE_IN_WORLD: i32 = 2;

pub(crate) use er_title_flow::MOVEMAPSTEP_FINALIZE_SUBSTATE_12A_OFFSET;

/// Human names for the finalize substate (`MoveMapStep+0x12a`) written by the advancer FUN_140afa7c0.
/// Grounded in the decompiled `switch(field25_0x12a)` cases (2026-07-19, bd er-effects-rs-9fmm):
///   0 idle/done; 1 fade-out wait; 2 death/retry check; 3 retry-menu + map-block setup;
///   4 map-block/session wait; 5/6 fade-in wait (+sfx); 7 remo/save-drain wait; 8 warp/server
///   finalize; 9 post-finalize. The warm reload parks at 7 (its 7->8 gate --
///   FUN_14067a170() && !ShouldSave() && !FUN_140679460() && FUN_140a9ceb0(CSRemo) -- never passes),
///   so 0x12a stays != 0 and the orchestrator never marks the world ready.
pub(crate) const MOVEMAPSTEP_FINALIZE_SUBSTATE_NAMES: [&str; 10] = [
    "IDLE/DONE",             // 0
    "FADE-OUT WAIT",         // 1
    "DEATH/RETRY CHECK",     // 2
    "RETRY MENU SETUP",      // 3  (retry-menu + map-block setup; '+' is not in the 5x7 font)
    "MAP/SESSION WAIT",      // 4  (map-block/session wait)
    "FADE-IN WAIT",          // 5
    "FADE-IN WAIT (SFX)",    // 6
    "CUTSCENE/SAVE WAIT",    // 7  <- warm-reload softlock parks here (REMO = cutscene system)
    "WARP/SERVER FINALIZE",  // 8
    "POST-FINALIZE",         // 9
];
/// Name a MoveMapStep finalize substate value (out-of-range -> "?").
pub(crate) fn movemapstep_finalize_substate_name(v: i32) -> &'static str {
    if v >= 0 && (v as usize) < MOVEMAPSTEP_FINALIZE_SUBSTATE_NAMES.len() {
        MOVEMAPSTEP_FINALIZE_SUBSTATE_NAMES[v as usize]
    } else {
        "?"
    }
}
/// MoveMapStep child edge-hook counters (STEP_MoveMap_Init fires when the child is created; Finish
/// fires when the load completes). On the softlock INIT fires but FINISH never does = the semaphore.
pub(crate) use er_telemetry::counters::SWITCH_ORACLE_MMS_INIT_HITS;
pub(crate) use er_telemetry::counters::SWITCH_ORACLE_MMS_FINISH_HITS;
pub(crate) use er_title_flow::MOVEMAPSTEP_STEP_MOVEMAP_INDEX;
pub(crate) use er_title_flow::MOVEMAPSTEP_COUNTDOWN_100_OFFSET;
/// MoveMapStep+0x244 is the native completion bit consumed by InGameStep/TitleStep
/// (`FUN_140aebe20` returns true iff MoveMapStep exists and this byte is nonzero).
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const MOVEMAPSTEP_TITLE_DONE_244_OFFSET: usize = 0x244;
/// SAVE-DISABLED SWITCH COMPLETION (2026-07-16). By design the ONLY save writer is the in-game "Save
/// Game" button; the game's quit-save on a System->Quit switch must NOT run. But the native return-title
/// state machine advances `GameMan+0xbc4` 1->2->3 ONLY inside a successful quit-save write (dump
/// FUN_14067b840: bc4 1->2 is welded to `cVar4 != 0`), and our final functor (title_tick_cover.rs) only
/// fires at bc4==READY(3). So with saving disabled bc4 can never reach READY through the game and the
/// switch stalls at STEP_MoveMap(18). We therefore drive bc4 ourselves, deterministically (no frame
/// counters): at the return-title REQUEST we write bc4=READY(3) directly, which BOTH lets the final
/// functor fire AND suppresses the quit-save (the orchestrator's `ShouldSave`/`FUN_140679460` require
/// bc4 != 3), so no disk write and no "failed to save" popup. `_FORCE_READY_COUNT` = REQUEST-time
/// bc4->READY writes. Then, because bc4 != 0 keeps the INCOMING world's STEP_MoveMap(18) advance gate
/// cleared every frame (FUN_140679010 reads bc4), once the new character is fully streamed (b7c1=1,
/// blocks>0) and parked at STEP_MoveMap with the final functor already fired, we clear bc4->0 so it
/// advances 18->19->20 and the world enters. `_FINALIZE_CLEAR_COUNT` = those incoming-world bc4->0 clears.
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_BC4_FORCE_READY_COUNT;
pub(crate) use er_title_flow::GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET;
pub(crate) use er_title_flow::FIELDAREA_CURRENT_BLOCK_ID_2C_OFFSET;
/// PopulateLists' per-area block-res source-builder (deobf 0x0066bb10, dump 0x14066bc00). The ONLY caller
/// of the +0xce0 WorldBlockRes constructor. Its 2nd arg (rdx) is the input MSB block list; it early-outs
/// on `*(rdx+0x10) == 0` (the block count) and builds nothing. On a fresh boot this list is full (incl the
/// dest block); on the in-game reload it is empty for the dest -> +0xce0 entry never (re)created -> blk_ls=0.
/// The `*(rdx+0x10)` count is the single decisive divergence semaphore between load 1 and load 2.
pub(crate) const POPULATE_BLOCKS_LISTS_RVA: u32 = 0x0066bb10;
pub(crate) const POPULATE_BLOCKS_LIST_INPUT_COUNT_10_OFFSET: usize = 0x10;
/// Load-state ENTRY constructor (deobf/runtime 0x006610e0, ground-truthed by disassembling the deobf
/// binary AT this address: vtable `lea …142a7d4b0`, `mov (%rdx),%eax; mov %eax,0x8(%rcx)` = the BlockId
/// key `entry+0x8` the getter scans for). Creates one entry in the shared load-state pool `worldres+0x148`
/// (stride 0xe0). Called only from the reconcile 0x66bb10. If this is NOT called with key 0x1c000000 on the
/// second load, the load-state entry for the destination block is never created -> getter null -> WORLD RES
/// WAIT stall. Args: rcx=entry, rdx=descNode.
pub(crate) const WORLDRES_ENTRY_CTOR_RVA: u32 = 0x006610e0;
/// The REAL WorldResWait block-res getter (deobf/runtime 0x0062f470; ground-truthed decompile:
/// `longlong FUN(WorldAreaRes* rcx, int* keyBlockId rdx)` scanning `+0xce0` [count `+0xcd8`, stride
/// 0xb98] for the entry whose WorldBlockInfo(+0x8)->BlockId(+0x34) == *key, returns that WorldBlockRes
/// (or 0). The WorldResWait check calls THIS with the real key and requires the returned entry's
/// +0x2d(ready) != 0 AND +0x35(phase) == 0x0a. The SWITCH-ORACLE's `blk_ls` calls the vtable getter
/// WITHOUT this key, so it is unreliable; hook this to see the TRUE result with the real key.
pub(crate) const WORLDRES_BLOCKRES_GETTER_RVA: u32 = 0x0062f470;
/// WorldBlockRes phase-2 handler (deobf/runtime 0x006157f0, dump 0x1406158d0). Advances the block load
/// phase +0x35 from 2 to 3 only when the block's primary FD4FileCap (block-res+0x40) has data ptr +0x90
/// != 0. On the reload the cap reports status +0x88==0x04 (loaded) but +0x90 stays null (file resident
/// from load 1, load short-circuits without re-attaching data), so it parks at phase 2. Single arg
/// (rcx=block-res). Hooked to force a bounded teardown/reload retry (phase +0x35 = 5) when that exact
/// stuck condition holds, so the block releases the stale cap and re-loads fresh.
pub(crate) const WORLDRES_BLOCKRES_PHASE2_RVA: u32 = 0x006157f0;
pub(crate) const BLOCKRES_PHASE_35_OFFSET: usize = 0x35;
pub(crate) const BLOCKRES_GATE_2F_OFFSET: usize = 0x2f;
pub(crate) const BLOCKRES_PRIMARY_FILECAP_40_OFFSET: usize = 0x40;
// Secondary FD4 file cap on the WorldBlockRes; the phase-2 handler (deobf 0x1406157f0) reads both
// block-res+0x40 and +0x48 and requires BOTH to report status==4 before advancing.
pub(crate) const BLOCKRES_SECOND_FILECAP_48_OFFSET: usize = 0x48;
pub(crate) const FILECAP_STATUS_88_OFFSET: usize = 0x88;
pub(crate) const FILECAP_DATA_90_OFFSET: usize = 0x90;
pub(crate) const FILECAP_STATUS_LOADED: i32 = 0x04;
// Historical: the block-load phase value the game's own data-null retry writes (phase-2 handler
// 0x1406157f0 sets +0x35=5 when worldBlockInfo+0x28 != 0). The stalecap fix no longer forces this --
// RE proved forcing the phase re-runs phase-1's find-or-insert which refcount-bumps the SAME stale cap
// and re-issues no read. Kept as documentation of the native retry value; the fix now re-enqueues.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const BLOCKRES_PHASE_TEARDOWN_RETRY: u8 = 5;
// --- FD4 file-cap re-issue path (RE 2026-07-17, deobf eldenring-deobf.bin) ---
// The stale second-load cap is status +0x88==4 (loaded) with data +0x90==NULL because world teardown
// releases the content child (refcount->0, freed) but leaves the PARENT cap registered in CSFile's name
// map (parent refcount +0x58 never reached 0). CSFile load (0x142651bb0) then find-or-inserts the SAME
// cap and only refcount-bumps it -- nothing re-reads. The game re-reads a cap only by ENQUEUEING it:
//   singleton  = *(CSFILE_SINGLETON_RVA)             (global holding the CSFile object)
//   holder     = *(singleton + 0x8)                  (load thunk 0x1426538b0: `mov rcx,[rcx+8]`)
//   idx        = (cap[+0x89] >> 2) & 7               (queue/priority index, set at insert 0x142651c1a)
//   queue      = *(holder + 0xe0 + idx*8)            (enqueue site 0x142651c4d; update loop 0x1426525bc)
//   cap[+0x88] = 0  then  ENQUEUE(rcx=queue, rdx=cap) via 0x14269d7b0
// The per-frame update loop (0x1426525a0) then selects the status==0 cap, sets it in-progress, and
// dispatches the async read (0x142659440) which re-attaches +0x90 on completion -> phase-2 advances.
/// CSFile singleton global (deobf VA 0x143d5b0f8): holds a pointer to the CSFile object.
pub(crate) const CSFILE_SINGLETON_RVA: u32 = 0x03d5b0f8;
/// Load-queue holder offset inside the CSFile object (`*(singleton+0x8)`; load thunk 0x1426538b0).
pub(crate) const CSFILE_HOLDER_8_OFFSET: usize = 0x8;
/// Base of the per-priority load-queue pointer array inside the holder (`holder+0xe0`, stride 8).
pub(crate) const CSFILE_QUEUE_ARRAY_E0_OFFSET: usize = 0xe0;
/// FD4FileCap +0x89: bits [2:4] hold the load-queue/priority index, `idx = (v>>2)&7`.
pub(crate) const FILECAP_QUEUEFLAGS_89_OFFSET: usize = 0x89;
/// FD4 file-cap ENQUEUE primitive (deobf VA 0x14269d7b0): `fn(rcx=queue, rdx=cap)`.
pub(crate) const CSFILE_ENQUEUE_RVA: u32 = 0x0269d7b0;
/// Warm-reload map-mount GUARD state root (deobf VA 0x143d5df38). The map-mount MenuJob (chain
/// 0x140836f30 -> ... -> 0x14082dbf0 -> 0x14082faf0) is enqueued only when the change-detector 0x14082d5b0
/// sees the load-phase state DIFFER from a self-updating cached descriptor. On the warm System->Quit->Load
/// the cached descriptor already equals the controller (System->Quit resets neither) -> "unchanged" ->
/// mount SKIPPED -> the block FD4FileCap gets +0x88=4 but +0x90 stays NULL -> WORLD RES WAIT stall.
/// singleton = *(root + 0x60); the job's cached descriptor is at singleton + 0x1200.
/// The "mount guard state root" IS `GameDataMan` (1.16.2 Ghidra, 734 xrefs). Worth stating
/// plainly because the guard below writes into a structure hanging off it (`*(root+0x60)`)
/// and nothing here said so. Derived from `er-game-base` (2026-08-01 RVA dedupe).
pub(crate) const MOUNT_GUARD_STATE_ROOT_RVA: u32 = er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA as u32;
/// The change-detector itself (deobf 0x14082d5b0, `fn(rcx=controller, rdx=descriptor) -> al`): al=1 CHANGED
/// (mount runs + descriptor re-synced), al=0 UNCHANGED (mount skipped). Instrumented read-only to identify
/// which gate instance is the m28 map-mount (al flips 1 on load1 -> 0 on load2). A clean leaf compare fn.
pub(crate) const MOUNT_GUARD_DETECTOR_RVA: u32 = 0x0082d5b0;
pub(crate) const MOUNT_GUARD_SINGLETON_OFFSET: usize = 0x60;
pub(crate) const MOUNT_GUARD_DESCRIPTOR_OFFSET: usize = 0x1200;
/// Descriptor mirror: +0x08 = cached u64 id, +0x04 = cached state bits (bits 0,3,4,5,6 mirror the
/// controller at +0x120/+0x128/+0x130..0x133). Writing id=0 and clearing those bits forces the detector
/// to return "changed" ONCE (it then re-syncs the descriptor), enqueuing exactly one map mount+bind.
pub(crate) const MOUNT_GUARD_DESC_ID_OFFSET: usize = 0x08;
pub(crate) const MOUNT_GUARD_DESC_BITS_OFFSET: usize = 0x04;
pub(crate) const MOUNT_GUARD_DESC_BITS_CLEAR_MASK: u32 = 0x79;
/// Mounted-EBL-archive REGISTRY global (deobf VA 0x1448464a8): `R = *(this)`, lazy-created by 0x141f49f60,
/// resolver 0x141f48b40. This is the container a mount census walks (NOT the CSEblFileManager object at
/// 0x143d5b078). Container B (the keyed registry) at `R+0x90`(first)/`R+0x98`(last), stride 0x40; per entry
/// the archive name is an MSVC wstring at `entry+0x08` and the `Archive*` is at `entry+0x30`; lock at
/// `R+0xB8`. Walk it to see whether the m28 (area 0x1c) player-map archive is mounted on the load-2 stall.
/// RE: bd step3 CSEblFileManager mount-table subagent 2026-07-17.
pub(crate) const EBL_REGISTRY_GLOBAL_RVA: u32 = 0x084864a8;
/// In-game player-map MOUNT ORCHESTRATOR (deobf 0x14082dbf0): a thin wrapper that calls 0x14082faf0
/// (which builds + dispatches the player-map EBL mount -- the `0x82dc1c` step). It is dispatched as an
/// in-game STEP (caller 0x14082eb7e is a step-thunk); on the warm System->Quit->Load reload the step is
/// skipped, so the destination map's archive is not re-mounted and the block read yields empty (+0x90
/// null) -> WORLD RES WAIT stall. `fn(rcx=stepContext, rdx, ...)`. NOT hooked by me3 (in-game fn, not the
/// file/EBL/mount path), so a read-only forwarding hook is safe. Hooked to capture its context args on
/// load 1 (fires + works) vs load 2 (skipped?) -- both the bug-fix driver interface and the own-load
/// primitive (drive the essential map mount menu-free for any save).
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const MAP_LOAD_ORCHESTRATOR_RVA: u32 = 0x0082dbf0;
/// MountEblArchive (deobf entry VA 0x1401efc00, prologue `40 55 56 57 41 56 41 57`): mounts an EBL/BHD
/// archive so its packed block files can be read. `fn(rcx=CSEblFileManager, rdx, r8, r9)` -- all three of
/// rdx/r8/r9 are null-checked; rdx and r8 point at (largely static, ~0x1429cf6xx) archive descriptors,
/// so their pointer identity distinguishes archives. Golden trace (bd
/// golden-mount-trace-fires-during-native-load-2026-06-22) proved it fires during a native map load.
/// PROBE ONLY: hooked to log which archives mount on the first autoload vs the System->Quit->Load reload,
/// to confirm/refute whether the destination map's archive is unmounted on quit and NOT re-mounted on the
/// warm reload (the run7 empty-EBL-read hypothesis; content child +0x90 stays null though status->4).
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const MOUNT_EBL_ARCHIVE_RVA: u32 = 0x001efc00;
// World BLOCK constructor (deobf/runtime 0x0062ec00): the ONLY writer of block+0x40 (load-state slice
// count) and block+0x48 (slice base), sourced from STACK args (0x68/0x70(%rsp)). NOT hooked -- a
// register-only forwarding hook loses those stack args and corrupts every block (runtime AV 2026-07-17).
// The slice-count/base offsets (0x40/0x48) live with the fix when it needs to repoint them.
// The matched block's load-state, read exactly as FUN_14066d4d0 does: call the block's vtable slot
// +0x10 (`block->vtable[0x10](block)`) to get the load-state object, then LOADED requires +0x2d != 0
// AND +0x35 == 0x0a(10). +0x35 (the stream-state/phase enum) stuck below 10 = the block is registered
// but its stream never completes (the WORLD RES WAIT stall). The getter/flag/phase offsets already
// exist as BLOCK_LOADSTATE_GETTER_VT_10_OFFSET / BLOCK_LOADSTATE_FLAG_2D_OFFSET /
// BLOCK_LOADSTATE_PHASE_35_OFFSET in constants/gaitem_restore.rs -- reused here, not redefined.
pub(crate) use er_title_flow::INGAMESTEP_WORLDLOADLIST_VPATH_BASE_210_OFFSET;
pub(crate) use er_title_flow::INGAMESTEP_WORLDLOADLIST_VPATH_SIZE_220_OFFSET;
pub(crate) use er_title_flow::INGAMESTEP_LOADLISTLIST_FILECAP_238_OFFSET;
/// The `dlc02` loadlist file-cap arg `_Common_Initialize` passes to `ProcessMsbLoadLists` as its
/// 3rd param: `MOV R8, [InGameStep+0x240]` (dump 0x140aed820). Null for base-game (non-DLC) areas;
/// the callee null-checks it, so passing this field (or 0) is safe.
pub(crate) const INGAMESTEP_LOADLISTLIST_DLC02_240_OFFSET: usize = 0x240;
/// `_Common_Initialize` passes the WorldInfoOwner to `ProcessMsbLoadLists` by ADDRESS of an EMBEDDED
/// sub-object at `InGameStep+0x250` (`LEA RCX, [InGameStep+0x250]`, dump 0x140aed820), NOT the pointer
/// stored at `FieldArea+0x10`. The init-time world-res rebuild replicates the native call verbatim,
/// so it uses this embedded address as the `this`.
pub(crate) const INGAMESTEP_WORLDINFO_OWNER_EMBED_250_OFFSET: usize = 0x250;
pub(crate) static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_READY_BLOCK_COUNT: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_DIALOG: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_QUEUE_READY: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_QUICKLOAD_TITLE_OWNER_SEEN_COUNT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_DIALOG;
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_CURSOR: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_BOUND;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_TOP_HIDE_ARMED_LIST;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG;
pub(crate) use er_title_flow::SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_TOP_HIDE_TOP_WINDOW;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_TOP_HIDE_LIST;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID;
/// `PropertyEditDialog`/System dialog embedded `SceneObjProxy` used by the Quit tab builder for child binds.
pub(crate) const SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET: usize = 0x1200;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER;
pub(crate) static START_SYSTEM_QUIT_DUPLICATE_BUTTON_HOOK: Once = Once::new();
/// One-shot spawn guard for the save-source redirect hook install (CreateFileW/CopyFileW path
/// redirect). Armed at process attach only when `enforce_save_override_or_abort` resolved a valid
/// env save source (Redirect mode); see save-override-no-default-fallback-mandatory-env-2026-06-23.
pub(crate) static START_SAVE_REDIRECT: Once = Once::new();
/// One-shot install guard for the SAVE-SAFE c30-writer diagnostic hook (mirrors
/// MENU_WINDOW_LATCH_INSTALLED). Installed unconditionally at process attach; the
/// hook is a pure passthrough that logs the c30-write gate, c30 before/after, and a
/// window of the resident save buffer to diagnose why GameMan+0xc30 stays default.
pub(crate) use er_telemetry::counters::C30_WRITER_HOOK_INSTALLED;
pub(crate) const C30_WRITER_HOOK_NOT_INSTALLED: usize = 0;
pub(crate) const C30_WRITER_HOOK_INSTALLED_YES: usize = 1;
pub(crate) static START_C30_WRITER_HOOK: Once = Once::new();
/// Rate limit for the c30-writer diagnostic log: only the first few calls are logged
/// (the cold deserialize drives a small bounded number of c30-writer entries).
pub(crate) use er_telemetry::counters::C30_WRITER_LOG_COUNT;
pub(crate) const C30_WRITER_LOG_MAX: usize = 8;
/// Bytes of the resident save buffer (rdx) to dump as hex from the c30-writer ENTER,
/// so the real target map record can be spotted offline. Read-only header window.
pub(crate) const C30_WRITER_BUFFER_DUMP_BYTES: usize = 0x40;
pub(crate) use er_title_flow::CONNECTION_ERROR_DIALOG;
/// Last vtable-validated MessageBoxDialog built by the game. Unlike CONNECTION_ERROR_DIALOG this
/// is never used to auto-dismiss; telemetry reads it at the end of a run to fail the oracle if a
/// blocking dialog is still alive after character/world load.
pub(crate) static MSGBOX_LAST_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MSGBOX_TOTAL_BUILDS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static MSGBOX_POSTLOAD_BUILDS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static MSGBOX_LAST_ARG_RCX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MSGBOX_LAST_ARG_RDX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MSGBOX_LAST_ARG_R8: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MSGBOX_LAST_ARG_R9: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::DISMISS_WRITE_LOG;
/// The dialog pointer OnDecide was last fired on, so we press OK exactly ONCE per dialog instead
/// of every frame (re-dispatching every frame keeps the dialog stuck "deciding" and it never
/// closes). A newly-built dialog has a different pointer, so it gets its own single OK.
pub(crate) static LAST_ONDECIDE_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// CS::MessageBoxDialog OnDecide/finalize (sub-object vtable slot 13) -- the genuine OK handler:
/// reads the chosen-button index [dialog+0x25e0] (builder-defaulted to OK) and dispatches it,
/// driving the dialog to emit "stop" to its parent MenuWindowJob (which then tears it down).
/// This is the verified headless dismiss: call with rcx=dialog. (Field writes do NOT close it --
/// +0x25e8 is the button COUNT, +0x25e0 the chosen index; both are config/output, not triggers.)
pub(crate) const MSGBOX_ONDECIDE_RVA: usize = MsgBoxRva::OnDecide as usize;
/// Force-stop / notify-owner-closed 0x14078dfd0(rcx=dialog): if owner [dialog+0x1c80]!=0 ->
/// owner->vtable[+0x10](dialog); else StepResult(3=stop)+EmitResult. Directly emits "stop" to
/// the parent MenuWindowJob so it tears the dialog down -- a more direct dismiss than OnDecide
/// (which only moved the selection to OK). Acceptable because the connection-error OK is a no-op.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const MSGBOX_FORCE_STOP_RVA: usize = MsgBoxRva::ForceStop as usize;
// Startup modal handling is lifecycle-driven by `startup_modal_blocking_state`, not by a fixed
// grace window.
