// ================================================================================================
// RENDER-HANDOFF FREEZE -- reverse-engineered addresses & struct offsets (1.16.2)
// ================================================================================================
//
// Provenance: static RE via the Ghidra runtime dump + deobf ground-truthing, 2026-07-18.
//
// STALE-PROVENANCE WARNING (2026-08-01): the trailing `// dump 0x...` annotations below were
// produced by `dump-deobf-shift.py` against the 1.16.1 dump. For 1.16.2 the shift is ZERO for
// BOTH `.text` and `.rdata`, so the live VA equals the dump VA and those parentheticals name
// addresses that are simply wrong for the current game. Do not use them to locate anything --
// query the 1.16.2 MCP instead. They are kept only as a record of where each value came from.
// (`dump-deobf-shift.py` itself is now cross-version and actively misleading; see AGENTS.md.)
//
// See bd memories:
//   - render-handoff-freeze-worldreswait-loadlist-root-2026-07-18   (GATE 1: loadlist / WorldResWait)
//   - render-handoff-freeze-second-gate-requestcode-2026-07-18       (GATE 2: STEP_Finish / requestCode)
//   - re-correction-second-gate-requestcode-stepfinish-2026-07-18    (the polarity correction)
//
// ADDRESS CONVENTION: constants ending `_RVA` are DEOBF/live RVAs (VA - 0x140000000), i.e. usable as
// `game_module_base() + RVA` to CALL or PATCH the live binary. Values noted "dump 0x..." are Ghidra
// dump VAs (for SEMANTICS only -- NEVER call a dump VA directly). Anything flagged REGION-ESTIMATE was
// NOT exactly ground-truthed and MUST be re-verified with disasm before being called/patched.
//
// The freeze has TWO independent gates on the in-memory redirect load path:
//   GATE 1 (fixed for the -1 case): the world-res loadlist virtual path is never built, so the dest
//           WorldBlockRes is never created and STEP_WorldResWait (mms child step 3) stalls.
//   GATE 2 (open): even after the MoveMap chain reaches its FINISH label, `requestCode`
//           (InGameStep+0xd8) is stuck at 1 and never advances to 2, because MoveMapStep::STEP_Finish
//           cannot pass its completion sub-gate (2-tick warmup / testNetStep finish / CSRemo-idle).
//           requestCode==2 is what STEP_MoveMap_Update needs to hand off; while it stays 1 the
//           per-frame ChrIns omission update keeps `draw_group` off and the loading cover stays.
//           NOTE (polarity): CSMenuMan+0x798 != 0 is the HEALTHY stable-in-world marker -- draining it
//           BOUNCES to title. Do NOT drain +0x798 and do NOT force requestCode=2.

// ---- GATE 1: loadlist / WorldResWait chain (deobf RVAs, ground-truthed) ----
// InGameStep::RequestMoveMap is REQUEST_MOVE_MAP_RVA (0xaebdc0) in constants/gaitem_restore.rs.
/// `InGameStep::STEP_MoveMap_LoadlistInit` -- builds the world-res loadlist ONLY if
/// `worldloadlistlistVirtualPath.size != 0`, then `CreateLoadlistlistFileCap` -> `+0x238`.
#[allow(dead_code)]
pub(crate) const STEP_MOVEMAP_LOADLIST_INIT_RVA: usize =
    er_game_base::rva::STEP_MOVEMAP_LOADLIST_INIT_RVA; // dump 0x140aec660
/// `CSFileImp::CreateLoadlistlistFileCap` (loadlist fileCap builder).
#[allow(dead_code)]
pub(crate) const CREATE_LOADLISTLIST_FILECAP_RVA: usize = 0x1f2b20;
/// `CS::WorldInfoOwner::ProcessMsbLoadLists(owner, fileCap, dlc02=0)` -- creates the block-res lists.
#[allow(dead_code)]
pub(crate) const PROCESS_MSB_LOADLISTS_RVA: usize = er_title_flow::WORLDINFO_PROCESS_MSB_LOADLISTS_RVA as usize; // dump 0x14066b2c0
/// `MoveMapStep::STEP_WorldResWait` (mms child step 3). Stalls until the FieldArea residency gate flips.
#[allow(dead_code)]
pub(crate) const STEP_WORLDRESWAIT_RVA: usize = 0xaf9cf0; // dump 0x140af9de0
/// FieldArea residency gate `FUN_140624cb0(fieldArea, time)` -- WorldResWait advances only when nonzero.
#[allow(dead_code)]
pub(crate) const WORLDRESWAIT_FIELDAREA_GATE_RVA: usize = 0x624bd0; // dump 0x140624cb0
/// `WorldBlockRes::Update` -- drives the block's FD4FileCap loadState 2->3->9->0xa (resident).
#[allow(dead_code)]
pub(crate) const WORLDBLOCKRES_UPDATE_RE_RVA: usize = 0x614870;
/// `GameMan::SetMoveMapStepBlockId(out, in)` -- writes GameMan+0x14 (moveMapStepBlockId). NOTE: the
/// INITIAL load's RequestMoveMap param_2 does NOT read +0x14; it traces to GameMan+0xc30. So this is
/// NOT the initial-load fix (only later transitions). Kept for completeness.
#[allow(dead_code)]
pub(crate) const SET_MOVEMAP_STEP_BLOCKID_RVA: usize =
    er_game_base::rva::SET_MOVE_MAP_STEP_BLOCK_ID_RVA;
/// `GameMan::GetMoveMapStepBlockId` -- reads GameMan+0x14.
#[allow(dead_code)]
pub(crate) const GET_MOVEMAP_STEP_BLOCKID_RVA: usize = 0x679340; // dump 0x140679430
/// `IsNonDebugArea(areaId)` == literally `areaId < 0x59`. RequestMoveMap skips FormatV for debug areas.
#[allow(dead_code)]
pub(crate) const IS_NON_DEBUG_AREA_RVA: usize = 0x720210; // dump 0x140720310

// ---- GATE 2: STEP_Finish / requestCode advance chain (deobf RVAs, ground-truthed unless noted) ----
/// `InGameStep::STEP_MoveMap_Update` -- advances `requestCode` (InGameStep+0xd8) 1->2 when the
/// MoveMapStep child signals finished (`MOVEMAP_CHILD_FINISHED_POLL_RVA`).
#[allow(dead_code)]
pub(crate) const STEP_MOVEMAP_UPDATE_RE_RVA: usize = er_title_flow::INGAMESTEP_STEP_MOVEMAP_UPDATE_RVA; // dump 0x140aec810
/// `MoveMapStep::STEP_Finish` -- the mms child FINISH step. Reaches terminal (`requestedState=-1`) only
/// after: (1) 2-tick warmup `field_0xb0 >= 2`; (2) testNetStep child finish+reset; (3) CSRemo-idle gate.
#[allow(dead_code)]
pub(crate) const STEP_MOVEMAP_FINISH_RVA: usize = 0xaf5a20; // dump 0x140af5b10
/// `FUN_140eb5550` -- EzChildStep "is finished" poll (true at child `requestedState==-1`). Used both on
/// the MoveMap child (by STEP_MoveMap_Update) and on `testNetStep` (by STEP_Finish).
#[allow(dead_code)]
pub(crate) const MOVEMAP_CHILD_FINISHED_POLL_RVA: usize = 0xeb5530; // dump 0x140eb5550
// REMOVED 2026-08-01: `EZ_CHILDSTEP_RESET_RVA = 0xeb54e0` was a MID-FUNCTION address. The
// 1.16.2 dump resolves 0x140eb54e0 to entry 0x140eb54c0 (size 111,
// `FUN_140eb54c0(EzChildStepBase*)`), i.e. it pointed 0x20 bytes INTO the function. Calling or
// hooking it would have executed from the middle of a prologue-established frame. It was never
// referenced, so this is removing a loaded gun rather than fixing a live crash -- and the
// correct entry was already declared below as EZ_CHILDSTEP_RESET_PINNED_RVA. Note that "PINNED"
// there marks the CORRECTED value; it is not the dedupe suffix it looks like (unlike
// EZ_CHILDSTEP_REQUEST_FINISH{,_PINNED}_RVA, which really are two names for one address).
/// `EzChildStepBase::RequestFinish` -- forces a child stepper toward finish. LAST-RESORT lever on the
/// MoveMap child wrapper (`InGameStep+0xe0`) AFTER WorldRes is resident; may skip STEP_Finish teardown,
/// so prefer satisfying the real sub-gate. Verify state before use.
#[allow(dead_code)]
pub(crate) const EZ_CHILDSTEP_REQUEST_FINISH_RVA: usize = 0xeb5570;
/// `InGameStep::STEP_RequestWait` -- at `requestCode==2` sets loadingScreenData.field_0x11 and, iff
/// `CSMenuMan+0x798 == 0`, clears InGameStep+0xd8 (ends session -> title). While +0x798 != 0 it stays
/// stable-in-world. Confirms +0x798 != 0 is the healthy state.
#[allow(dead_code)]
pub(crate) const STEP_REQUEST_WAIT_RVA: usize = 0xaecc10; // dump 0x140aecd00
/// `CS::MenuJobQueue::ExecuteMenuJob` -- generic MenuJob drain (runs Execute vfptr[2], zeroes slot on
/// ShouldContinue). NOTE: NOT run on +0x798 by CSMenuManImp::Update (that slot is the stable marker).
#[allow(dead_code)]
pub(crate) const EXECUTE_MENU_JOB_RE_RVA: usize = er_title_flow::EXECUTE_MENU_JOB_RVA; // dump 0x1407a96f0
/// CSRemo-idle gate `FUN_140a9cdb0` (checked inside STEP_Finish): reads `GLOBAL_CSRemo+8`, returns idle
/// via `vt+0x18` OR (`vt+0x50 == 1 && +0x1a == 0`). A dangling remo/cutscene keeps this returning
/// not-idle. REGION-ESTIMATE deobf -- VERIFY with disasm before calling/patching.
#[allow(dead_code)]
pub(crate) const CSREMO_IDLE_GATE_RVA_ESTIMATE: usize = 0xa9cca0; // dump 0x140a9cdb0 (est; verify)

// ---- struct offsets (Ghidra-authoritative unless flagged) ----
/// `InGameStep+0xd8` -- `requestCode` / busy-latch (u32). Stuck at 1 in the freeze; must reach 2 for the
/// render handoff. Cleared to 0 only by STEP_RequestWait when CSMenuMan+0x798 == 0 (== end session).
#[allow(dead_code)]
pub(crate) const INGAMESTEP_REQUEST_CODE_D8_OFFSET: usize = 0xd8;
/// `InGameStep+0xe0` -- the MoveMap child-step WRAPPER (EzChildStep); its stepper ptr is at wrapper+0x8.
#[allow(dead_code)]
pub(crate) const INGAMESTEP_MOVEMAP_CHILD_WRAPPER_E0_OFFSET: usize = 0xe0;
/// EzChildStep wrapper -> inner stepper pointer. Null == finished; non-null == still running.
#[allow(dead_code)]
pub(crate) const EZ_CHILDSTEP_WRAPPER_STEPPER_08_OFFSET: usize = 0x08;
/// `MoveMapStep+0x48` -- child step state (== 3 at STEP_WorldResWait). (Also in other modules.)
#[allow(dead_code)]
pub(crate) const MOVEMAPSTEP_STATE_48_RE_OFFSET: usize = 0x48;
/// Resident-world `STEP_MoveMap` update state. This state is intentionally long-lived while the world
/// is playable; leaving it belongs to session teardown.
pub(crate) const MOVEMAPSTEP_RESIDENT_UPDATE_STATE: i32 = 18;
/// `MoveMapStep+0xb0` -- STEP_Finish 2-tick warmup counter (must reach >= 2). REGION/needs-confirm on
/// the exact field; used read-only for diagnosis first.
#[allow(dead_code)]
pub(crate) const MOVEMAPSTEP_FINISH_WARMUP_B0_OFFSET: usize = 0xb0;
/// `MoveMapStep+0x128` -- `pauseGame?`; when true, `STEP_MoveMap` disables normal task registration
/// unless the debug-pause input path overrides it.
pub(crate) const MOVEMAPSTEP_PAUSE_GAME_128_OFFSET: usize = 0x128;
/// `MoveMapStep+0x348` -- one of the native task-registration suppressors evaluated by
/// `STEP_MoveMap` after the WorldChrMan predicate.
pub(crate) const MOVEMAPSTEP_DISABLE_TASKS_348_OFFSET: usize = 0x348;
/// `MoveMapStep+0x349` -- one-shot task-registration override consumed by `STEP_MoveMap`.
pub(crate) const MOVEMAPSTEP_FORCE_TASKS_349_OFFSET: usize = 0x349;
/// `MoveMapStep+0x4b8` -- final per-frame input to the native task-registration path. If false,
/// WorldChrMan's movement/physics tasks are not registered for the frame.
pub(crate) const MOVEMAPSTEP_TASK_REGISTRATION_4B8_OFFSET: usize = 0x4b8;
/// `MoveMapStep+0x4ba` -- copied into `ChrCtrl.luaEventFlags` bit 6 by STEP_MoveMap's native
/// WorldChrMan task-registration path. It becomes 1 after the +0x100 countdown reaches zero.
pub(crate) const MOVEMAPSTEP_CONTROL_ENABLE_4BA_OFFSET: usize = 0x4ba;
/// `DAT_143d70847` -- one-shot global suppressor consumed by `STEP_MoveMap`; it clears both +0x4b8
/// task registration and +0x4ba control enable for the frame.
pub(crate) const MOVEMAPSTEP_GLOBAL_DISABLE_RVA: usize = 0x3d7_0847;
/// `ChrCtrl+0xe8` -- native movement logic requires bits 5 (logic enabled) and 6 (MoveMap control
/// enabled) together. 1.16.2 `FUN_1403cbff0` checks `(luaEventFlags & 0x60) == 0x60`.
pub(crate) const CHRCTRL_LUA_EVENT_FLAGS_E8_OFFSET: usize = 0xe8;
/// `ChrCtrl+0xe9` -- native `disableMove`; the same movement gate requires it to be false.
pub(crate) const CHRCTRL_DISABLE_MOVE_E9_OFFSET: usize = 0xe9;
/// `CSMenuMan+0x798` -- NowLoading cover MenuJob slot (the STABLE-session marker; != 0 is HEALTHY).
#[allow(dead_code)]
pub(crate) const CSMENUMAN_NOWLOADING_JOB_798_OFFSET: usize = 0x798;
/// `CSMenuMan+0x72c` -- `loadingScreenData.field_0xc`, zeroed by deobf `FUN_14067a410` when changing
/// loading-screen mode.
#[allow(dead_code)]
pub(crate) const CSMENUMAN_LOADINGSCREEN_FIELD_C_72C_OFFSET: usize = 0x72c;
/// `CSMenuMan+0x730` -- `loadingScreenData.field_0x10` (drives per-frame cover-job recreation).
#[allow(dead_code)]
pub(crate) const CSMENUMAN_LOADINGSCREEN_FIELD10_730_OFFSET: usize = 0x730;
// ---- STEP_Finish sub-gate reads (pinned 2026-07-18, bd render-handoff-freeze-second-gate-pins) ----
// STEP_Finish reaches terminal (requestedState=-1, letting STEP_MoveMap_Update set requestCode 1->2)
// only when: warmup (+0xb0) >= 2 AND testNetStep child finished AND the CSRemo-idle gate passes.
/// `MoveMapStep.testNetStep` EzChildStep WRAPPER offset. Its inner stepper ptr is at wrapper+0x8
/// (== MoveMapStep+0x110): stepper == 0 -> finished/skipped; != 0 -> still running (offline-hang suspect).
#[allow(dead_code)]
pub(crate) const MOVEMAPSTEP_TESTNETSTEP_WRAPPER_108_OFFSET: usize = 0x108;
#[allow(dead_code)]
pub(crate) const MOVEMAPSTEP_TESTNETSTEP_STEPPER_110_OFFSET: usize = 0x110;
/// `EzChildStepBase::RequestFinish` (dump 0x140eb5590) -- save-safe lever to force testNetStep to finish
/// (sets child+0xb4). Fire on the wrapper at MoveMapStep+0x108 if the stepper is hung offline.
#[allow(dead_code)]
pub(crate) const EZ_CHILDSTEP_REQUEST_FINISH_PINNED_RVA: usize = EZ_CHILDSTEP_REQUEST_FINISH_RVA;
/// `FUN_140eb54e0` EzChildStep reset (corrected deobf; nulls stepper + clears finish latch +0x10).
#[allow(dead_code)]
pub(crate) const EZ_CHILDSTEP_RESET_PINNED_RVA: usize =
    er_game_base::rva::EZ_CHILDSTEP_RESET_RVA;
/// `GLOBAL_CSRemo` singleton: `[base + 0x3d6ea58]` -> CSRemoImp*. (Region-consistent with the
/// NowLoading/FakeLoading globals; flagged estimate but in-range.)
#[allow(dead_code)]
pub(crate) const GLOBAL_CSREMO_RVA: usize = 0x3d6ea58;
/// CSRemoImp+0x8 -> CSRemoMan* (`remoMan`). remoMan == null == CSRemo-init gap (gate BUSY).
#[allow(dead_code)]
pub(crate) const CSREMO_REMOMAN_08_OFFSET: usize = 0x08;
/// CSRemoMan+0xd0 (qword) -- pending-remo/request signal (the `[0x1a]` index x8 in the decomp). != 0
/// == a remo/cutscene is pending (idle gate fails). Read-only "remo pending" instrumentation signal.
#[allow(dead_code)]
pub(crate) const CSREMOMAN_PENDING_D0_OFFSET: usize = 0xd0;
/// `TitleStep+0x2e8` -> InGameStep* (the session step). Used to resolve MoveMapStep in-world via a
/// cached title/session owner (see game_man_snapshot.rs). (Named TITLE_STEP_IN_GAME_STEP_2E8 elsewhere.)
#[allow(dead_code)]
pub(crate) const TITLESTEP_INGAMESTEP_2E8_RE_OFFSET: usize = 0x2e8;
