// ---- Return-title "rebuild the title" request flags set by the final functor (0x7a3900) ----
// The functor does `*(*([GLOBAL_CSMenuMan]+0x8)+0x5d)=1` and `*(0x143d6c5e8)=1`. These are level
// flags (not edge-consumed); we set them to tear down the old char for the switch, but nothing
// resets them, so once the reloaded character's world comes up the still-set +0x5d re-requests the
// quit-to-title -> GameMan.save_requested flips true again (~3.6s post-load, proven by the gm-snap
// trace) -> a second save + SetState(2) bounces the freshly-loaded world back to the title. We clear
// both once the reload commits (continue_confirm), which is after the teardown they were needed for.
/// (`CS_MENU_MAN_GLOBAL_RVA` = `[GLOBAL_CSMenuMan]` pointer global is already defined above.)
/// `CSMenuManImp::menuData` pointer at CSMenuMan+0x8.
pub const CS_MENU_MAN_MENU_DATA_OFFSET: usize = 0x8;
/// `DAT_143d6c5e8` companion rebuild flag (data RVA). No readers found in the dump, but cleared for
/// symmetry so we fully undo what the final functor set.
pub const RETURN_TITLE_REBUILD_FLAG_DAT_RVA: usize = 0x3d6c5e8;
// ---- In-game session liveness gate (the post-reload bounce decision, static RE 2026-07-02) ----
// TitleStep state 6 (STEP_GameStepWait, dump 0x140b0ced0) exits to the quit-to-title transition
// (SetState(2) -> BeginLogo -> BeginTitle -> MenuJobWait) the first tick it sees
// `InGameStep->requestCode == 0`. The request-code register (InGameStep+0xd8, int) lifecycle:
// ctor=0; RequestMoveMap (dump 0x140aebeb0, called by STEP_PlayGame for the initial world load with
// the map from TitleStep+0xbc) =1; STEP_MoveMap_Update (dump 0x140aec810) =2 when the map move's
// child MoveMapStep finishes; STEP_RequestWait (dump 0x140aecd00) at ==2 waits for the in-game menu
// job qword at CSMenuMan+0x798 to be nonzero -- while it is nonzero the session idles at code 2
// (the stable in-world state); if that qword reads 0 it writes the request code to 0, which is what
// STEP_GameStepWait converts into the return-to-title. So a reloaded world only stays up if
// CSMenuMan+0x798 is (re)populated after the load.
/// `CS::EzChildStepBase::RequestFinish` (dump `0x140eb5590` -> live `0x140eb5570`, shift -0x20,
/// content-unique). One-shot: calls the wrapper's CSSetFinishHelper virtual (which sets the child
/// step's finish-requested byte at child+0xb4) then latches wrapper+0x10. The quit-to-title
/// teardown ends the in-world MoveMapStep session through here; the post-switch reload bounce is
/// this firing against the fresh MoveMapStep child right after streaming completes. Read-only
/// trace hook logs every call + caller RVA to identify the stale requester.
pub const EZ_CHILD_STEP_REQUEST_FINISH_RVA: u32 = EZ_CHILDSTEP_REQUEST_FINISH_RVA as u32;
/// `EzChildStep<MoveMapStep>` wrapper offset inside `InGameStep` (ctor dump 0x140aeabf3).
pub const IN_GAME_STEP_MOVE_MAP_WRAPPER_E0_OFFSET: usize = 0xe0;
/// `EzChildStep<InGameStayStep>` wrapper offset inside `InGameStep` (ctor dump 0x140aeabc3).
pub const IN_GAME_STEP_STAY_WRAPPER_B8_OFFSET: usize = 0xb8;
/// `EzChildStepBase::stepper` (the owned child step object) at wrapper+0x8; the finish latch byte
/// is wrapper+0x10 and the CSSetFinishHelper pointer wrapper+0x18 (dump 0x140eb5590 decompile).
pub const EZ_CHILD_STEP_STEPPER_OFFSET: usize = 0x8;
pub use er_telemetry_core::counters::SWITCH_ORACLE_MAX_STABLE_FRAMES;
pub use er_telemetry_core::counters::SWITCH_ORACLE_STABLE_FRAMES;
/// Switch-outcome oracle (2026-07-16, user-mandated reliable semaphore). Read-only per-frame classifier of
/// a switch/load outcome so the state is always knowable from telemetry, never from eyeballing. `_TICK` is
/// the frame counter since a switch was picked (if it stops advancing the game task froze = froze). `_STABLE`
/// is consecutive frames the game's own stable-in-world condition holds (player present + requestCode==2 +
/// in-game menu job CSMenuMan+0x798 != 0): climbing high == LOADED_STABLE; resetting to 0 after climbing ==
/// the world dropped (BOUNCED/reload). `_MAX_STABLE` latches the peak so a later drop is still visible.
pub use er_telemetry_core::counters::SWITCH_ORACLE_TICK;
pub use er_telemetry_core::counters::SYSTEM_QUIT_CHILD_FINISH_TRACE_COUNT;
pub use er_telemetry_core::counters::SYSTEM_QUIT_CHILD_FINISH_TRACE_INSTALLED;
pub use er_telemetry_core::counters::SYSTEM_QUIT_CHILD_FINISH_TRACE_ORIG;
/// Count of frames we cleared a stale `CSMenuMan->disableSaveMenu` during an active switch (the switch-2
/// quit-save gate; see [`CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET`]). Non-zero on a switch == that switch's
/// quit-save was being blocked and we unblocked it (the runtime semaphore for this fix).
pub use er_telemetry_core::counters::SYSTEM_QUIT_DISABLE_SAVE_MENU_CLEAR_COUNT;
/// Count of quick-load handoffs that invoked the original native Quit Game row action trampoline
/// instead of the low-level accepted callback alone. This is an experiment to test whether the full
/// native return-title menu-job chain is the missing teardown boundary.
pub use er_telemetry_core::counters::SYSTEM_QUIT_QUICKLOAD_NATIVE_QUIT_ACTION_COUNT;
/// Rate-limit counter for the switch-2 save-gate diagnostic (which of the save orchestrator
/// `FUN_140afb970`'s three gates -- force latch `0x143d856a0`, `save_state`, or the CSMenuMan menu gate
/// `FUN_14080d660` -- is blocking the quit-save so `bc4` freezes at 1).
pub use er_telemetry_core::counters::SYSTEM_QUIT_SAVE_GATE_DIAG_COUNT;
/// InGameStep requestCode (`InGameStep + 0xd8`) values. 1 = a MoveMap (load) request is pending/in
/// progress; 2 = stable in-world (the load handoff completed, the world is settled -- player present,
/// in-game menu job populated). STEP_MoveMap_Update drains 1 -> 2 once the child finishes.
pub const INGAMESTEP_REQUEST_CODE_NONE: i32 = 0;
/// Human name for an InGameStep requestCode (`InGameStep + 0xd8`) value (out-of-range/unreadable -> "?").
pub fn ingamestep_request_code_name(v: i32) -> &'static str {
    match v {
        INGAMESTEP_REQUEST_CODE_NONE => "NONE",
        INGAMESTEP_REQUEST_CODE_MOVEMAP_PENDING => "MOVEMAP PENDING",
        INGAMESTEP_REQUEST_CODE_STABLE_IN_WORLD => "STABLE IN-WORLD",
        _ => "?",
    }
}
/// 3RD-load root sharpened (Ghidra 1.16.1, 2026-07-16). The softlock parks the InGameStep at
/// `InGameStep_StepperArray[7] = STEP_MoveMap_Update` (dump 0x140aec810). STEP_MoveMap_Update gates
/// its advance to step 8 (STEP_MoveMap_Finish) on `FUN_140eb5550(ezChildStepBase)` == "is the
/// MoveMapStep child step finished?"; only then does it write requestCode(+0xd8)=2. On the stall the
/// child is non-NULL (created at step 6 STEP_MoveMap_Init) but its own step machine never reaches
/// Finish, so requestCode stays 1 forever. So the true stall is inside the MoveMapStep child's
/// world-load. This oracle publishes the child's current internal step so the stuck point is a RAM
/// semaphore, not an eyeball. `usize::MAX` = not sampled / no child.
pub use er_telemetry_core::counters::SWITCH_ORACLE_MMS_STEP;
/// MoveMap destination BlockId + world-stable RAM semaphore offsets (RE-verified 2026-07-19,
/// bd er-effects-rs-9fmm). `InGameStep::STEP_MoveMap_Update` @0x140aec810 loads the destination block
/// after requestCode(+0xd8)=2; it reads `GameMan+0xac8` (loadTargetMapId) when
/// `CSSessionManager.protocol_state == WaitReload(4)`, else `GameMan+0x14` (moveMapStepBlockId), and
/// skips the load when that BlockId == 0xffffffff -> the world reverts to title with nothing reloaded.
/// So these two fields are the destination-valid RAM semaphore for the reload's retention.
pub const GAME_MAN_MOVE_MAP_STEP_BLOCK_ID_14_OFFSET: usize = 0x14;
pub const GAME_MAN_LOAD_TARGET_MAP_ID_AC8_OFFSET: usize = 0xac8;
/// BlockId sentinel meaning "no destination" (skip the map load).
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub const MOVE_MAP_BLOCK_ID_NONE: u32 = 0xffff_ffff;
/// `FUN_140508d30` (dump 0x140508d30) returns `WorldChrMan.field47_0x1e524 == 2` = world genuinely
/// stable/ready -- a stronger world-ready oracle than can_move. RE-verified offset + constant.
pub const WORLD_CHR_MAN_WORLD_STABLE_1E524_OFFSET: usize = 0x1e524;
pub const WORLD_CHR_MAN_WORLD_STABLE_VALUE: i32 = 2;
/// GameMan online-state bytes. The connection-loss / network-error event handlers build their
/// "cannot connect / connection lost" GR_System_Message (whose side-effect returns to title) gated on
/// `isInOnlineMode (GameMan+0xBC8) && serverConnectionEnabled (GameMan+0xBC9)`. force_offline_connection_bytes
/// forces both to 0 each game-task frame, but that is a race: if the reload's session setup re-sets
/// BC8=1 and the handler fires before the next game-task clear, a network-error return-title reverts the
/// just-loaded world (user hypothesis 2026-07-19, bd reload-revert-likely-message-interrupt-2026-07-19).
/// Sem-traced to test whether the online flags re-enable during the reload's in-world window.
pub const GAME_MAN_IS_IN_ONLINE_MODE_BC8_OFFSET: usize = 0xBC8;
pub const GAME_MAN_SERVER_CONNECTION_ENABLED_BC9_OFFSET: usize = 0xBC9;
/// `GameMan::saveState` (b80) state names for the loading-bar / logs, as the load lane walks
/// them; the save lane shares the slot and stamps 1. See the
/// `GAME_MAN_SAVE_STATE_*` / `FULLREAD_B80_RESIDENT` constants (constants::autoload_state).
pub fn save_state_b80_name(v: i32) -> &'static str {
    match v {
        GAME_MAN_SAVE_STATE_IDLE => "IDLE",
        GAME_MAN_SAVE_STATE_OPENING => "OPENING",
        GAME_MAN_SAVE_STATE_READING => "READING",
        FULLREAD_B80_RESIDENT => "LOADED",
        _ => "?",
    }
}
pub use er_telemetry_core::counters::SWITCH_ORACLE_MENU_JOB_PRESENT;
/// Last sampled player/menu/loading-screen handoff gates for visible loading-bar sub-milestones.
pub use er_telemetry_core::counters::SWITCH_ORACLE_PLAYER_PRESENT;
/// MoveMapStep internal step index -> name. Order from the InGameStep-analogue registrar labels
/// (`u_MoveMapStep::STEP_*` at dump 0x142b5eb30..) and validated for 0..3 by the observed
/// `mms_state 1 MsbLoad -> 2 MsbLoadWait -> 3 WorldResWait` progression (own_stepper idx6 watch).
/// Indices >8 are best-effort (label order); the raw index in the log is authoritative.
/// UPPERCASE (the boot-bar 5x7 font is A-Z + space only, and it doubles as the bar's phase label).
pub const MOVEMAPSTEP_STEP_NAMES: [&str; 21] = [
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
pub fn movemapstep_step_name(idx: i32) -> &'static str {
    if idx >= 0 && (idx as usize) < MOVEMAPSTEP_STEP_NAMES.len() {
        MOVEMAPSTEP_STEP_NAMES[idx as usize]
    } else {
        "?"
    }
}

/// Terminal step of the MoveMapStep child's own sequence.
pub const MOVEMAPSTEP_STEP_FINISH_INDEX: usize = 20;

/// The InGameStep request code at `InGameStep+0xd8`, which gates the load after the MoveMapStep
/// child's own finish (20). RE grounding (2026-07-19, InGameStep step table 0x143d70190:
/// STEP_MoveMap_Init -> STEP_MoveMap_Update -> STEP_MoveMap_Finish): the MoveMapStep child (steps
/// 0..20) runs entirely inside the InGameStep's STEP_MoveMap_Update, so the child reaching finish is
/// not genuine world readiness -- the InGameStep must still advance STEP_MoveMap_Update ->
/// STEP_MoveMap_Finish (this code draining 1 -> 2) and then hand off to the resident in-world step.
///
/// Those two post-finish stages are real load progress the bar must show (user 2026-07-19: "when we
/// are at the Nth step, it is really N+1 -- add an Nth loading step"). They are SUBSTEPS of the
/// entering world phase (`MAP STEP DONE` -> `WORLD HANDOFF` -> `PLAYER IN WORLD` -> `CAN MOVE`), not
/// main phases: promoting them to main steps gave the bar a second, longer `N/M` denominator that
/// swapped in mid-load and let a stale previous-epoch request code read as near-complete progress the
/// instant a reload armed (bd er-effects-rs-ok8d).
pub const INGAMESTEP_REQUEST_CODE_MAP_FINISH: i32 = 1;
pub const INGAMESTEP_REQUEST_CODE_IN_WORLD: i32 = 2;

/// Human names for the finalize substate (`MoveMapStep+0x12a`) written by the advancer FUN_140afa7c0.
/// Grounded in the decompiled `switch(field25_0x12a)` cases (2026-07-19, bd er-effects-rs-9fmm):
///   0 idle/done; 1 fade-out wait; 2 death/retry check; 3 retry-menu + map-block setup;
///   4 map-block/session wait; 5/6 fade-in wait (+sfx); 7 remo/save-drain wait; 8 warp/server
///   finalize; 9 post-finalize. The warm reload parks at 7 (its 7->8 gate --
///   FUN_14067a170() && !ShouldSave() && !FUN_140679460() && FUN_140a9ceb0(CSRemo) -- never passes),
///   so 0x12a stays != 0 and the orchestrator never marks the world ready.
pub const MOVEMAPSTEP_FINALIZE_SUBSTATE_NAMES: [&str; 10] = [
    "IDLE/DONE",            // 0
    "FADE-OUT WAIT",        // 1
    "DEATH/RETRY CHECK",    // 2
    "RETRY MENU SETUP",     // 3  (retry-menu + map-block setup; '+' is not in the 5x7 font)
    "MAP/SESSION WAIT",     // 4  (map-block/session wait)
    "FADE-IN WAIT",         // 5
    "FADE-IN WAIT (SFX)",   // 6
    "CUTSCENE/SAVE WAIT",   // 7  <- warm-reload softlock parks here (REMO = cutscene system)
    "WARP/SERVER FINALIZE", // 8
    "POST-FINALIZE",        // 9
];
/// Name a MoveMapStep finalize substate value (out-of-range -> "?").
pub fn movemapstep_finalize_substate_name(v: i32) -> &'static str {
    if v >= 0 && (v as usize) < MOVEMAPSTEP_FINALIZE_SUBSTATE_NAMES.len() {
        MOVEMAPSTEP_FINALIZE_SUBSTATE_NAMES[v as usize]
    } else {
        "?"
    }
}
pub use er_telemetry_core::counters::SWITCH_ORACLE_MMS_FINISH_HITS;
/// MoveMapStep child edge-hook counters (STEP_MoveMap_Init fires when the child is created; Finish
/// fires when the load completes). On the softlock INIT fires but finish never does = the semaphore.
pub use er_telemetry_core::counters::SWITCH_ORACLE_MMS_INIT_HITS;
/// MoveMapStep+0x244 is the native completion bit consumed by InGameStep/TitleStep
/// (`FUN_140aebe20` returns true iff MoveMapStep exists and this byte is nonzero).
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub const MOVEMAPSTEP_TITLE_DONE_244_OFFSET: usize = 0x244;
/// Save-disabled switch completion (2026-07-16). By design the only save writer is the in-game "Save
/// Game" button; the game's quit-save on a System->Quit switch must not run. But the native return-title
/// state machine advances `GameMan+0xbc4` 1->2->3 only inside a successful quit-save write (dump
/// FUN_14067b840: bc4 1->2 is welded to `cVar4 != 0`), and our final functor (title_tick_cover.rs) only
/// fires at bc4==ready(3). So with saving disabled bc4 can never reach ready through the game and the
/// switch stalls at STEP_MoveMap(18). We therefore drive bc4 ourselves, deterministically (no frame
/// counters): at the return-title request we write bc4=ready(3) directly, which both lets the final
/// functor fire and suppresses the quit-save (the orchestrator's `ShouldSave`/`FUN_140679460` require
/// bc4 != 3), so no disk write and no "failed to save" popup. `_FORCE_READY_COUNT` = request-time
/// bc4->ready writes. Then, because bc4 != 0 keeps the incoming world's STEP_MoveMap(18) advance gate
/// cleared every frame (FUN_140679010 reads bc4), once the new character is fully streamed (b7c1=1,
/// blocks>0) and parked at STEP_MoveMap with the final functor already fired, we clear bc4->0 so it
/// advances 18->19->20 and the world enters. `_FINALIZE_CLEAR_COUNT` = those incoming-world bc4->0 clears.
pub use er_telemetry_core::counters::SYSTEM_QUIT_BC4_FORCE_READY_COUNT;
/// PopulateLists' per-area block-res source-builder (deobf 0x0066bb10, dump 0x14066bc00). The only caller
/// of the +0xce0 WorldBlockRes constructor. Its 2nd arg (rdx) is the input MSB block list; it early-outs
/// on `*(rdx+0x10) == 0` (the block count) and builds nothing. On a fresh boot this list is full (incl the
/// dest block); on the in-game reload it is empty for the dest -> +0xce0 entry never (re)created -> blk_ls=0.
/// The `*(rdx+0x10)` count is the single decisive divergence semaphore between load 1 and load 2.
pub const POPULATE_BLOCKS_LISTS_RVA: u32 = 0x0066bb10;
pub const POPULATE_BLOCKS_LIST_INPUT_COUNT_10_OFFSET: usize = 0x10;
/// Load-state entry constructor (deobf/runtime 0x006610e0, ground-truthed by disassembling the deobf
/// binary at this address: vtable `lea …142a7d4b0`, `mov (%rdx),%eax; mov %eax,0x8(%rcx)` = the BlockId
/// key `entry+0x8` the getter scans for). Creates one entry in the shared load-state pool `worldres+0x148`
/// (stride 0xe0). Called only from the reconcile 0x66bb10. If this is not called with key 0x1c000000 on the
/// second load, the load-state entry for the destination block is never created -> getter null -> world RES
/// wait stall. Args: rcx=entry, rdx=descNode.
pub const WORLDRES_ENTRY_CTOR_RVA: u32 = 0x006610e0;
/// The real WorldResWait block-res getter (deobf/runtime 0x0062f470; ground-truthed decompile:
/// `longlong FUN(WorldAreaRes* rcx, int* keyBlockId rdx)` scanning `+0xce0` [count `+0xcd8`, stride
/// 0xb98] for the entry whose WorldBlockInfo(+0x8)->BlockId(+0x34) == *key, returns that WorldBlockRes
/// (or 0). The WorldResWait check calls this with the real key and requires the returned entry's
/// +0x2d(ready) != 0 and +0x35(phase) == 0x0a. The switch-oracle's `blk_ls` calls the vtable getter
/// without this key, so it is unreliable; hook this to see the true result with the real key.
pub const WORLDRES_BLOCKRES_GETTER_RVA: u32 = 0x0062f470;
/// WorldBlockRes phase-2 handler (deobf/runtime 0x006157f0, dump 0x1406158d0). Advances the block load
/// phase +0x35 from 2 to 3 only when the block's primary FD4FileCap (block-res+0x40) has data ptr +0x90
/// != 0. On the reload the cap reports status +0x88==0x04 (loaded) but +0x90 stays null (file resident
/// from load 1, load short-circuits without re-attaching data), so it parks at phase 2. Single arg
/// (rcx=block-res). Hooked to force a bounded teardown/reload retry (phase +0x35 = 5) when that exact
/// stuck condition holds, so the block releases the stale cap and re-loads fresh.
pub const WORLDRES_BLOCKRES_PHASE2_RVA: u32 = 0x006157f0;
pub const BLOCKRES_PHASE_35_OFFSET: usize = 0x35;
pub const BLOCKRES_GATE_2F_OFFSET: usize = 0x2f;
pub const BLOCKRES_PRIMARY_FILECAP_40_OFFSET: usize = 0x40;
// Secondary FD4 file cap on the WorldBlockRes; the phase-2 handler (deobf 0x1406157f0) reads both
// block-res+0x40 and +0x48 and requires both to report status==4 before advancing.
pub const BLOCKRES_SECOND_FILECAP_48_OFFSET: usize = 0x48;
pub const FILECAP_STATUS_88_OFFSET: usize = 0x88;
pub const FILECAP_DATA_90_OFFSET: usize = 0x90;
pub const FILECAP_STATUS_LOADED: i32 = 0x04;
// Historical: the block-load phase value the game's own data-null retry writes (phase-2 handler
// 0x1406157f0 sets +0x35=5 when worldBlockInfo+0x28 != 0). The stalecap fix no longer forces this --
// RE proved forcing the phase re-runs phase-1's find-or-insert which refcount-bumps the same stale cap
// and re-issues no read. Kept as documentation of the native retry value; the fix now re-enqueues.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub const BLOCKRES_PHASE_TEARDOWN_RETRY: u8 = 5;
// --- FD4 file-cap re-issue path (RE 2026-07-17, deobf eldenring-deobf.bin) ---
// The stale second-load cap is status +0x88==4 (loaded) with data +0x90==NULL because world teardown
// releases the content child (refcount->0, freed) but leaves the parent cap registered in CSFile's name
// map (parent refcount +0x58 never reached 0). CSFile load (0x142651bb0) then find-or-inserts the same
// cap and only refcount-bumps it -- nothing re-reads. The game re-reads a cap only by ENQUEUEING it:
//   singleton  = *(CSFILE_SINGLETON_RVA)             (global holding the CSFile object)
//   holder     = *(singleton + 0x8)                  (load thunk 0x1426538b0: `mov rcx,[rcx+8]`)
//   idx        = (cap[+0x89] >> 2) & 7               (queue/priority index, set at insert 0x142651c1a)
//   queue      = *(holder + 0xe0 + idx*8)            (enqueue site 0x142651c4d; update loop 0x1426525bc)
//   cap[+0x88] = 0  then  ENQUEUE(rcx=queue, rdx=cap) via 0x14269d7b0
// The per-frame update loop (0x1426525a0) then selects the status==0 cap, sets it in-progress, and
// dispatches the async read (0x142659440) which re-attaches +0x90 on completion -> phase-2 advances.
/// CSFile singleton global (deobf VA 0x143d5b0f8): holds a pointer to the CSFile object.
pub const CSFILE_SINGLETON_RVA: u32 = 0x03d5b0f8;
/// Load-queue holder offset inside the CSFile object (`*(singleton+0x8)`; load thunk 0x1426538b0).
pub const CSFILE_HOLDER_8_OFFSET: usize = 0x8;
/// Base of the per-priority load-queue pointer array inside the holder (`holder+0xe0`, stride 8).
pub const CSFILE_QUEUE_ARRAY_E0_OFFSET: usize = 0xe0;
/// FD4FileCap +0x89: bits [2:4] hold the load-queue/priority index, `idx = (v>>2)&7`.
pub const FILECAP_QUEUEFLAGS_89_OFFSET: usize = 0x89;
/// FD4 file-cap ENQUEUE primitive (deobf VA 0x14269d7b0): `fn(rcx=queue, rdx=cap)`.
pub const CSFILE_ENQUEUE_RVA: u32 = 0x0269d7b0;
/// Warm-reload map-mount guard state root (deobf VA 0x143d5df38). The map-mount MenuJob (chain
/// 0x140836f30 -> ... -> 0x14082dbf0 -> 0x14082faf0) is enqueued only when the change-detector 0x14082d5b0
/// sees the load-phase state differ from a self-updating cached descriptor. On the warm System->Quit->Load
/// the cached descriptor already equals the controller (System->Quit resets neither) -> "unchanged" ->
/// mount skipped -> the block FD4FileCap gets +0x88=4 but +0x90 stays NULL -> world RES wait stall.
/// singleton = *(root + 0x60); the job's cached descriptor is at singleton + 0x1200.
/// The "mount guard state root" is `GameDataMan` (1.16.2 Ghidra, 734 xrefs). Worth stating
/// plainly because the guard below writes into a structure hanging off it (`*(root+0x60)`)
/// and nothing here said so. Derived from `er-game-base` (2026-08-01 RVA dedupe).
pub const MOUNT_GUARD_STATE_ROOT_RVA: u32 = er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA as u32;
/// The change-detector itself (deobf 0x14082d5b0, `fn(rcx=controller, rdx=descriptor) -> al`): al=1 changed
/// (mount runs + descriptor re-synced), al=0 unchanged (mount skipped). Instrumented read-only to identify
/// which gate instance is the m28 map-mount (al flips 1 on load1 -> 0 on load2). A clean leaf compare fn.
pub const MOUNT_GUARD_DETECTOR_RVA: u32 = 0x0082d5b0;
pub const MOUNT_GUARD_SINGLETON_OFFSET: usize = 0x60;
pub const MOUNT_GUARD_DESCRIPTOR_OFFSET: usize = 0x1200;
/// Descriptor mirror: +0x08 = cached u64 id, +0x04 = cached state bits (bits 0,3,4,5,6 mirror the
/// controller at +0x120/+0x128/+0x130..0x133). Writing id=0 and clearing those bits forces the detector
/// to return "changed" once (it then re-syncs the descriptor), enqueuing exactly one map mount+bind.
pub const MOUNT_GUARD_DESC_ID_OFFSET: usize = 0x08;
pub const MOUNT_GUARD_DESC_BITS_OFFSET: usize = 0x04;
pub const MOUNT_GUARD_DESC_BITS_CLEAR_MASK: u32 = 0x79;
/// Mounted-EBL-archive registry global (deobf VA 0x1448464a8): `R = *(this)`, lazy-created by 0x141f49f60,
/// resolver 0x141f48b40. This is the container a mount census walks (not the CSEblFileManager object at
/// 0x143d5b078). Container B (the keyed registry) at `R+0x90`(first)/`R+0x98`(last), stride 0x40; per entry
/// the archive name is an MSVC wstring at `entry+0x08` and the `Archive*` is at `entry+0x30`; lock at
/// `R+0xB8`. Walk it to see whether the m28 (area 0x1c) player-map archive is mounted on the load-2 stall.
/// RE: bd step3 CSEblFileManager mount-table subagent 2026-07-17.
///
/// Alias, not a declaration. This name and `er-game-base`'s
/// `DL_FILE_DEVICE_MANAGER_SINGLETON_RVA` are the same global under two names, and until
/// 2026-08-30 this one held a corrupted literal of its own: `0x84864a8`, the leading digits of
/// `0x48464a8` written `848` instead of `484`. That is `0x2684ca8` (38.5 MiB) past the end of the
/// `0x5e01800`-byte image, so every reader took its `else` branch from the day it was written.
///
/// `game_rva` did not refuse it, and that is the part worth carrying forward. The resolver only
/// consults the 1.16.2 -> 1.17 table for addresses inside the image (`is_game_image_address`);
/// anything outside is handed back unchanged, on the reasoning that an import or a non-image
/// address means the same thing on every patch. A corrupt RVA is outside the image by
/// construction, so it takes the passthrough and arrives at `safe_read_usize` looking resolved.
/// The failure surfaced only as `ReadProcessMemory` returning false, which the call sites fold
/// into `None`/`0` -- indistinguishable from a genuinely null global. An out-of-image constant is
/// therefore the one class of wrong address the refusal machinery cannot catch.
///
/// The byte proof and the 1.17 carry live with the owning declaration; deriving instead of
/// re-typing is what keeps the two in step.
pub const EBL_REGISTRY_GLOBAL_RVA: u32 =
    er_game_base::rva::DL_FILE_DEVICE_MANAGER_SINGLETON_RVA as u32;
/// In-game player-map mount ORCHESTRATOR (deobf 0x14082dbf0): a thin wrapper that calls 0x14082faf0
/// (which builds + dispatches the player-map EBL mount -- the `0x82dc1c` step). It is dispatched as an
/// in-game step (caller 0x14082eb7e is a step-thunk); on the warm System->Quit->Load reload the step is
/// skipped, so the destination map's archive is not re-mounted and the block read yields empty (+0x90
/// null) -> world RES wait stall. `fn(rcx=stepContext, rdx, ...)`. Not hooked by me3 (in-game fn, not the
/// file/EBL/mount path), so a read-only forwarding hook is safe. Hooked to capture its context args on
/// load 1 (fires + works) vs load 2 (skipped?) -- both the bug-fix driver interface and the own-load
/// primitive (drive the essential map mount menu-free for any save).
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub const MAP_LOAD_ORCHESTRATOR_RVA: u32 = 0x0082dbf0;
/// MountEblArchive (deobf entry VA 0x1401efc00, prologue `40 55 56 57 41 56 41 57`): mounts an EBL/BHD
/// archive so its packed block files can be read. `fn(rcx=CSEblFileManager, rdx, r8, r9)` -- all three of
/// rdx/r8/r9 are null-checked; rdx and r8 point at (largely static, ~0x1429cf6xx) archive descriptors,
/// so their pointer identity distinguishes archives. Golden trace (bd
/// golden-mount-trace-fires-during-native-load-2026-06-22) proved it fires during a native map load.
/// Probe ONLY: hooked to log which archives mount on the first autoload vs the System->Quit->Load reload,
/// to confirm/refute whether the destination map's archive is unmounted on quit and not re-mounted on the
/// warm reload (the run7 empty-EBL-read hypothesis; content child +0x90 stays null though status->4).
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub const MOUNT_EBL_ARCHIVE_RVA: u32 = 0x001efc00;
// World block constructor (deobf/runtime 0x0062ec00): the only writer of block+0x40 (load-state slice
// count) and block+0x48 (slice base), sourced from stack args (0x68/0x70(%rsp)). Not hooked -- a
// register-only forwarding hook loses those stack args and corrupts every block (runtime AV 2026-07-17).
// The slice-count/base offsets (0x40/0x48) live with the fix when it needs to repoint them.
// The matched block's load-state, read exactly as FUN_14066d4d0 does: call the block's vtable slot
// +0x10 (`block->vtable[0x10](block)`) to get the load-state object, then loaded requires +0x2d != 0
// and +0x35 == 0x0a(10). +0x35 (the stream-state/phase enum) stuck below 10 = the block is registered
// but its stream never completes (the world RES wait stall). The getter/flag/phase offsets already
// exist as BLOCK_LOADSTATE_GETTER_VT_10_OFFSET / BLOCK_LOADSTATE_FLAG_2D_OFFSET /
// BLOCK_LOADSTATE_PHASE_35_OFFSET in constants/gaitem_restore.rs -- reused here, not redefined.
/// The `dlc02` loadlist file-cap arg `_Common_Initialize` passes to `ProcessMsbLoadLists` as its
/// 3rd param: `MOV R8, [InGameStep+0x240]` (dump 0x140aed820). Null for base-game (non-DLC) areas;
/// the callee null-checks it, so passing this field (or 0) is safe.
pub const INGAMESTEP_LOADLISTLIST_DLC02_240_OFFSET: usize = 0x240;
/// `_Common_Initialize` passes the WorldInfoOwner to `ProcessMsbLoadLists` by address of an embedded
/// sub-object at `InGameStep+0x250` (`LEA RCX, [InGameStep+0x250]`, dump 0x140aed820), not the pointer
/// stored at `FieldArea+0x10`. The init-time world-res rebuild replicates the native call verbatim,
/// so it uses this embedded address as the `this`.
pub const INGAMESTEP_WORLDINFO_OWNER_EMBED_250_OFFSET: usize = 0x250;
pub static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_READY_BLOCK_COUNT: AtomicUsize =
    AtomicUsize::new(0);
pub static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_QUEUE_READY: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub use er_telemetry_core::counters::SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_DIALOG;
pub use er_telemetry_core::counters::SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT;
pub use er_telemetry_core::counters::SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER;
pub use er_telemetry_core::counters::SYSTEM_QUIT_QUICKLOAD_TITLE_OWNER_SEEN_COUNT;
pub static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_CURSOR: AtomicUsize =
    AtomicUsize::new(usize::MAX);
/// The `CS::ProfileSummary` slot the ProfileSelect cursor should be parked on, or
/// [`SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_NONE`].
///
/// Armed when a foreign save's preview lands (the lowest slot it actually occupies) and consumed by
/// the per-frame `05_010_ProfileSelect` run, which calls the game's own
/// `ProfileLoadDialog::SelectSaveSlot`. Without it the cursor stays wherever the dialog's
/// constructor left it -- row 0, which for a save whose character is not in slot 0 is a different
/// character's row, or the live session's own.
pub static SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_SLOT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_NONE);
/// "No cursor move is pending." Not a slot: slots are `0..10`.
pub const SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_NONE: i32 = -1;
pub use er_telemetry_core::counters::SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_BOUND;
pub use er_telemetry_core::counters::SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG;
pub use er_telemetry_core::counters::SYSTEM_QUIT_TOP_HIDE_ARMED_LIST;
pub use er_telemetry_core::counters::SYSTEM_QUIT_TOP_HIDE_LIST;
pub use er_telemetry_core::counters::SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW;
pub use er_telemetry_core::counters::SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID;
pub use er_telemetry_core::counters::SYSTEM_QUIT_TOP_HIDE_TOP_WINDOW;
/// `PropertyEditDialog`/System dialog embedded `SceneObjProxy` used by the Quit tab builder for child binds.
pub const SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET: usize = 0x1200;
pub use er_telemetry_core::counters::SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER;
pub use er_telemetry_core::counters::SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE;
pub static START_SYSTEM_QUIT_DUPLICATE_BUTTON_HOOK: Once = Once::new();
/// One-shot spawn guard for the save-source redirect hook install (CreateFileW/CopyFileW path
/// redirect). Armed at process attach only when `enforce_save_override_or_abort` resolved a valid
/// env save source (Redirect mode); see save-override-no-default-fallback-mandatory-env-2026-06-23.
pub static START_SAVE_REDIRECT: Once = Once::new();
/// One-shot install guard for the save-safe c30-writer diagnostic hook (mirrors
/// MENU_WINDOW_LATCH_INSTALLED). Installed unconditionally at process attach; the
/// hook is a pure passthrough that logs the c30-write gate, c30 before/after, and a
/// window of the resident save buffer to diagnose why GameMan+0xc30 stays default.
pub use er_telemetry_core::counters::C30_WRITER_HOOK_INSTALLED;
pub const C30_WRITER_HOOK_NOT_INSTALLED: usize = 0;
pub const C30_WRITER_HOOK_INSTALLED_YES: usize = 1;
pub static START_C30_WRITER_HOOK: Once = Once::new();
/// Rate limit for the c30-writer diagnostic log: only the first few calls are logged
/// (the cold deserialize drives a small bounded number of c30-writer entries).
pub use er_telemetry_core::counters::C30_WRITER_LOG_COUNT;
pub const C30_WRITER_LOG_MAX: usize = 8;
/// Bytes of the resident save buffer (rdx) to dump as hex from the c30-writer enter,
/// so the real target map record can be spotted offline. Read-only header window.
pub const C30_WRITER_BUFFER_DUMP_BYTES: usize = 0x40;
/// Last vtable-validated MessageBoxDialog built by the game. Unlike CONNECTION_ERROR_DIALOG this
/// is never used to auto-dismiss; telemetry reads it at the end of a run to fail the oracle if a
/// blocking dialog is still alive after character/world load.
pub static MSGBOX_LAST_DIALOG: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub static MSGBOX_TOTAL_BUILDS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub static MSGBOX_POSTLOAD_BUILDS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub static MSGBOX_LAST_ARG_RCX: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub static MSGBOX_LAST_ARG_RDX: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub static MSGBOX_LAST_ARG_R8: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub static MSGBOX_LAST_ARG_R9: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub use er_telemetry_core::counters::DISMISS_WRITE_LOG;
/// The dialog pointer OnDecide was last fired on, so we press OK exactly once per dialog instead
/// of every frame (re-dispatching every frame keeps the dialog stuck "deciding" and it never
/// closes). A newly-built dialog has a different pointer, so it gets its own single OK.
pub static LAST_ONDECIDE_DIALOG: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// CS::MessageBoxDialog OnDecide/finalize (sub-object vtable slot 13) -- the genuine OK handler:
/// reads the chosen-button index [dialog+0x25e0] (builder-defaulted to OK) and dispatches it,
/// driving the dialog to emit "stop" to its parent MenuWindowJob (which then tears it down).
/// This is the verified headless dismiss: call with rcx=dialog. (Field writes do not close it --
/// +0x25e8 is the button count, +0x25e0 the chosen index; both are config/output, not triggers.)
pub const MSGBOX_ONDECIDE_RVA: usize = MsgBoxRva::OnDecide as usize;
/// Force-stop / notify-owner-closed 0x14078dfd0(rcx=dialog): if owner [dialog+0x1c80]!=0 ->
/// owner->vtable[+0x10](dialog); else StepResult(3=stop)+EmitResult. Directly emits "stop" to
/// the parent MenuWindowJob so it tears the dialog down -- a more direct dismiss than OnDecide
/// (which only moved the selection to OK). Acceptable because the connection-error OK is a no-op.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub const MSGBOX_FORCE_STOP_RVA: usize = MsgBoxRva::ForceStop as usize;
// Startup modal handling is lifecycle-driven by `startup_modal_blocking_state`, not by a fixed
// grace window.

// ===========================================================================================
// the ending-request EVALUATOR'S nine inputs, and the two nobody was reading
// ===========================================================================================
//
// `MoveMapStep`'s per-frame advancer (`FUN_140afa6d0` on 1.16.2; the tree's older notes call it
// `FUN_140afa7c0`, a 1.16.1 address) computes one boolean `cVar10`, writes it to
// `menuData+0x5e`, and then `case 0: if (cVar10 == 0) return;` -- so `cVar10` alone decides
// whether the MoveMap child leaves the resident `STEP_MoveMap(18)` and walks to its terminal.
// The child reaching its terminal is what lets `InGameStep` drain `+0xd8`, and `STEP_GameStepWait`
// tears the world down to `BeginLogo` on the first frame `+0xd8 == 0` with `b7c`/`b7d` clear.
// That teardown is the second-load black screen.
//
// Read off the named 1.16.2 dump (2026-09-04), `cVar10 = 1` if any of:
//
//   1. `FUN_140e2aa70()`                          -- returns a constant 0. Dead.
//   2. `CSEzSelectBot::IsBotEnabled()`            -- the debug auto-play bot.
//   3. `CSSessionManager->protocolState == 4`     -- And `PartyMemberInfo::IsNotHost()`.  <-- (a)
//   4. `CSEventMan->deadReset->field_0x8 == 2`                                            <-- (b)
//   5. `BOOL_143d856a0`                           -- the force latch.        logged as `force`
//   6. `menuData+0x5d`                            -- the return-title byte.  logged as `rt5d`
//   7. `GameManIsWarpRequested()`                 -- `GameMan+0x10`.         logged as `warp`
//   8. `FUN_140679430()`                          -- `GameMan+0xb7c`.        logged as `b7c`
//   9. `FUN_140679440()`                          -- `GameMan+0xb7d`.        logged as `b7d`
//   ( `FUN_140e62aa0() == 8` is a tenth term in the decompile and also returns a constant 0. )
//
// The `MMS-CLEANUP` discriminator logged 5, 6, 7, 8 and 9 only, and every one of them read zero
// at the black-screen advance -- in three separate runs, including the user's own product session
// of 2026-09-04 08:58. That produced the recorded conclusion "every input to cVar10 is zero, it
// advanced anyway", which does not follow: the evaluator was never misbehaving, four of its
// inputs were simply unobserved. Terms 1 and 10 are eliminated statically (both callees are
// `return 0;`), which leaves (a) and (b) as the only candidates. The constants below exist so the
// discriminator can name which.

/// Offset of `protocolState` on `CSSessionManagerImp` (field ordinal 3 in the named 1.16.2 dump),
/// from `0x140afa8a5: CMP dword ptr [RAX + 0x10],0x4`. Same field
/// `er_invasion_warp_core::warp::SESSION_PROTOCOL_STATE_OFFSET` names.
pub const CS_SESSION_MANAGER_PROTOCOL_STATE_10_OFFSET: usize = 0x10;

/// The `WaitReload` protocol state -- the literal `4` the evaluator's compare tests, and the same
/// value `STEP_MoveMap_Update` tests before it reads `GameMan+0xac8` instead of `GameMan+0x14`
/// for the destination block (see [`GAME_MAN_LOAD_TARGET_MAP_ID_AC8_OFFSET`]).
///
/// FALSIFIED as the black-screen cause (run br-20260905-022413-272e, 2026-09-04). It was the prime
/// suspect on the reasoning that a switch-load is exactly what parks a session in "waiting for a
/// reload" -- but the instrumented run measured `session_proto=6` (InGame) at both the legitimate
/// teardown and the black-screen one, so this term never fired. Kept because it is a real input to
/// the ending-request evaluator and the discriminator still has to prove it stayed quiet.
pub const SESSION_PROTOCOL_STATE_WAIT_RELOAD: i32 = 4;

/// Offset of the `deadReset` member on `CSEventManImp` (field ordinal 2, a
/// `CSEventDeadResetState *`), from `0x140afa8f7: MOV RCX,qword ptr [RCX + 0x10]`.
pub const CS_EVENT_MAN_DEAD_RESET_10_OFFSET: usize = 0x10;

/// Offset of the state word on `CSEventDeadResetState`. `FUN_1405fef50` is the whole getter --
/// `return param_1->field1_0x8;` -- and `0x140afa900: CMP EAX,0x2` is its only consumer here.
pub const CS_EVENT_DEAD_RESET_STATE_8_OFFSET: usize = 0x8;

/// The dead-reset state the evaluator treats as an ending request.
pub const DEAD_RESET_STATE_ENDING: i32 = 2;

/// `CS::MoveMapStep`'s per-frame advancer -- the function that COMPUTES `cVar10` and writes it to
/// `menuData+0x5e`. 1.16.2 `0x140afa6d0`; 1.17 `0x140afb9f0`.
///
/// Why it is here. Every reading of the nine inputs so far has been taken from outside this
/// function: `MMS-CLEANUP` on the child's Cleanup entry (measured ~2 s late) and then `CVAR10 RISE`
/// on the game-task tick that notices the `menuData+0x5e` 0->1 edge. The edge sampler is closer but
/// still downstream -- it polls a value this function has already written -- so the seven zeros it
/// reports (`rt5d=0 warp=0 b7c=0 b7d=0 force=0 session_proto=6 dead_reset=-1`, run 2026-09-06
/// 10:04:07) are consistent with "the terms were transient and had already cleared". Sampling at
/// this function's entry is the only place the inputs are the ones the decision is actually made
/// from.
///
/// How it was identified, because the usual route failed. `scripts/map-rvas-1162-to-1170.py`
/// refuses this address: 111 shape matches, none at the nearest anchor's delta. It was found by
/// call graph instead -- `L"EnableBot"` occurs twice in the 1.17 image and only `0x142bfdfd0` has
/// xrefs; of its two referents the 137-byte `0x140e7e580` matches 1.16.2
/// `CS::CSEzSelectBot::IsBotEnabled` exactly, and the one caller of it in the MoveMap region
/// decompiles with `L"CSEzSelectBot.MoveMapStep"`. `scripts/verify-rva-map-1170.py` then confirmed
/// the pair independently: Identical-whole, ratio 1.00 over 972 instructions, both-entries,
/// `PDATA:0x11b0/0x11b0`.
pub const MOVEMAP_ADVANCER_RVA: usize = 0xafa6d0;

/// Last sampled `menuData+0x5e` (== the evaluator's `cVar10` output), so the game-task tick can
/// detect its 0->1 rising edge. `-1` is the unread sentinel and never matches an edge.
///
/// The edge is the only useful sample point. `MMS-CLEANUP` fires on the child's Cleanup entry,
/// which is downstream of the decision, and run br-20260905-022413-272e proved that is too late:
/// every live input read 0 there (`warp=0 b7c=0 b7d=0 rt5d=0 force=0 session_proto=6
/// dead_reset=0`) on a teardown that unquestionably happened, so a transient input had already
/// been consumed by the time Cleanup ran.
pub static CVAR10_LAST: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(-1);

/// Count of `cVar10` 0->1 edges seen this process. The boot load is the negative control: the same
/// run's healthy Da beast autoload emitted no MMS-cleanup at all and its world lived, so a boot
/// that also produces no rise here confirms the detector tracks the teardown and not noise.
pub static CVAR10_RISE_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
