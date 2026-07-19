// ================================================================================================
// RENDER-HANDOFF FREEZE -- reverse-engineered addresses & struct offsets (1.16.1)
// ================================================================================================
//
// Provenance: static RE via the Ghidra runtime dump + deobf ground-truthing (dump-deobf-shift.py),
// 2026-07-18. See bd memories:
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
pub(crate) const STEP_MOVEMAP_LOADLIST_INIT_RVA: usize = 0xaec570; // dump 0x140aec660
/// `CSFileImp::CreateLoadlistlistFileCap` (loadlist fileCap builder).
#[allow(dead_code)]
pub(crate) const CREATE_LOADLISTLIST_FILECAP_RVA: usize = 0x1f2b20;
/// `CS::WorldInfoOwner::ProcessMsbLoadLists(owner, fileCap, dlc02=0)` -- creates the block-res lists.
#[allow(dead_code)]
pub(crate) const PROCESS_MSB_LOADLISTS_RVA: usize = 0x66b1d0; // dump 0x14066b2c0
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
pub(crate) const SET_MOVEMAP_STEP_BLOCKID_RVA: usize = 0x67abd0; // dump 0x14067acc0
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
pub(crate) const STEP_MOVEMAP_UPDATE_RE_RVA: usize = 0xaec720; // dump 0x140aec810
/// `MoveMapStep::STEP_Finish` -- the mms child FINISH step. Reaches terminal (`requestedState=-1`) only
/// after: (1) 2-tick warmup `field_0xb0 >= 2`; (2) testNetStep child finish+reset; (3) CSRemo-idle gate.
#[allow(dead_code)]
pub(crate) const STEP_MOVEMAP_FINISH_RVA: usize = 0xaf5a20; // dump 0x140af5b10
/// `FUN_140eb5550` -- EzChildStep "is finished" poll (true at child `requestedState==-1`). Used both on
/// the MoveMap child (by STEP_MoveMap_Update) and on `testNetStep` (by STEP_Finish).
#[allow(dead_code)]
pub(crate) const MOVEMAP_CHILD_FINISHED_POLL_RVA: usize = 0xeb5530; // dump 0x140eb5550
/// `FUN_140eb54e0` -- EzChildStep reset (called on testNetStep after it finishes).
#[allow(dead_code)]
pub(crate) const EZ_CHILDSTEP_RESET_RVA: usize = 0xeb54e0;
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
pub(crate) const EXECUTE_MENU_JOB_RE_RVA: usize = 0x7a9600; // dump 0x1407a96f0
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
/// `MoveMapStep+0xb0` -- STEP_Finish 2-tick warmup counter (must reach >= 2). REGION/needs-confirm on
/// the exact field; used read-only for diagnosis first.
#[allow(dead_code)]
pub(crate) const MOVEMAPSTEP_FINISH_WARMUP_B0_OFFSET: usize = 0xb0;
/// `CSMenuMan+0x798` -- NowLoading cover MenuJob slot (the STABLE-session marker; != 0 is HEALTHY).
#[allow(dead_code)]
pub(crate) const CSMENUMAN_NOWLOADING_JOB_798_OFFSET: usize = 0x798;
/// `CSMenuMan+0x728` -- `loadingScreenData.mode` written by deobf `FUN_14067a410` via the helper at
/// `0x140860d80`: `CSMenuMan+0x720+8 = mode`.
#[allow(dead_code)]
pub(crate) const CSMENUMAN_LOADINGSCREEN_MODE_728_OFFSET: usize = 0x728;
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
pub(crate) const EZ_CHILDSTEP_REQUEST_FINISH_PINNED_RVA: usize = 0xeb5570;
/// `FUN_140eb54e0` EzChildStep reset (corrected deobf; nulls stepper + clears finish latch +0x10).
#[allow(dead_code)]
pub(crate) const EZ_CHILDSTEP_RESET_PINNED_RVA: usize = 0xeb54c0;
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
