// ---- CSGaitemImp pristine-restore (post-switch reload gaitem free-queue exhaustion fix) ----
// `GLOBAL_CSGaitem` is a PROCESS-LIFETIME FD4 singleton (constructed once at boot by the giant
// repository-init FUN_140cd6d40 behind `if (GLOBAL_CSGaitem==0)`; NOT rebuilt per world-load). Its
// pointer lives at dump 0x143d69890 (data RVA, stable across deobf; the unmodified
// PlayerGameData::Deserialize reads this same global and boot loads work through it). On a normal
// quit-to-title the world/inventory teardown releases the player's gaitems back to the free-queue
// via RemoveCSGaitemIns; our lightweight return-title chain (0x67a3a0) skips that, so char#1's
// items stay resident, char#2's reload deserialize exhausts freeTableIdxQueue (head==end ->
// GetUnindexedGaItemHandle returns 0 -> gaitemInsTable[-1] OOB dispatch = the AV at live 0x67141a).
// We restore pristine at clean title before the reload by sweeping occupied gaitemInsTable slots
// and calling the native per-item release RemoveCSGaitemIns (which frees the ins AND returns its
// index to the free-queue) -- the exact primitive the native teardown would use, no hand-rebuilt
// queue. See bd system-quit-postswitch-crash-gaitem-freequeue-exhaustion-2026-07-02.
pub(crate) const GLOBAL_CSGAITEM_SINGLETON_RVA: usize = 0x3d69890;
/// `CS::GaItemImp::RemoveCSGaitemIns(CSGaitemImp*, uint* gaItemHandle)` -- dump 0x140672650 ->
/// live/deobf 0x672560 (shift -0xf0, content-unique, ground-truthed via dump-deobf-shift.py). Given
/// a handle it destructs+deallocates gaitemInsTable[index], resets the entry, and pushes index back
/// to freeTableIdxQueue[++end].
pub(crate) const CSGAITEM_REMOVE_INS_RVA: usize = 0x672560;
/// CSGaitemImp field offsets (Ghidra struct, size 0x19038): gaitemInsTable = CSGaitemIns*[0x1400],
/// entries = CSGaitemImpEntry[0x1400] (stride 8: {u32 unindexedGaItemHandle, u32 refCount}),
/// freeTableIdxQueue = uint[0x1400], then head/end ids.
pub(crate) const CSGAITEM_INS_TABLE_OFFSET: usize = 0x8;
pub(crate) const CSGAITEM_ENTRIES_OFFSET: usize = 0xa008;
pub(crate) const CSGAITEM_ENTRY_STRIDE: usize = 0x8;
pub(crate) const CSGAITEM_FREE_QUEUE_HEAD_OFFSET: usize = 0x19008;
pub(crate) const CSGAITEM_FREE_QUEUE_END_OFFSET: usize = 0x1900c;
pub(crate) const CSGAITEM_TABLE_CAPACITY: usize = 0x1400;
/// Count of gaitem ins objects released by the pristine-restore sweep (product proof: >0 exactly
/// once per switch reload, and the free-queue returns to full afterward).
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_GAITEM_RESET_RELEASED_COUNT;
/// Count of pristine-restore invocations (should be 1 per switch).
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_GAITEM_RESET_INVOCATIONS;
/// Free-queue slack (0x13ff - free_count) observed at the LAST reset, before/after the sweep. A
/// healthy result is before>0 (char#1 items resident) and after==0 (queue full again).
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_GAITEM_RESET_LAST_SLACK_BEFORE;
pub(crate) static SYSTEM_QUIT_GAITEM_RESET_LAST_SLACK_AFTER: AtomicUsize =
    AtomicUsize::new(usize::MAX);
/// The save-data subsystem gate the c30-writer 0x67bd70 checks before it writes
/// GameMan+0xc30: `[0x143d68078]` (RVA 0x3d68078). It is a 0x270-byte heap object
/// built by the save-load boot 0x6798d0..0x679904 and zeroed on teardown 0x6789bf.
/// If null at the writer's entry, 0x67bd70 returns without writing c30 (gate (a) in
/// the c30-stays-default diagnosis). The save-safe c30-writer probe logs this.
pub(crate) const SAVE_DATA_SUBSYSTEM_GATE_RVA: usize = 0x3d68078;
/// World-resource streaming lever (worldres-loadstate-creator-and-streaming-enable-
/// gate-2026). Gap 1: the block-load request is built from the InGameStep target
/// coord [InGameStep+0x100]; set it to slot 9's real map then re-submit via
/// 0x140aed820 so the builder creates the m10 load-states. Gap 2: the resmgr
/// ([InGameStep+0x250]) streaming-enable flag [resmgr+0xb7c1]==0; the virtual
/// enabler 0x14066e2e4 sets it + builds the session singletons + starts the IO jobs.
pub(crate) const INGAMESTEP_TARGET_COORD_100_OFFSET: usize = 0x100;
pub(crate) const INGAMESTEP_RESMGR_250_OFFSET: usize = 0x250;
pub(crate) const REQUEST_SUBMIT_RVA: usize = 0xaed820;
/// `InGameStep::RequestMoveMap` (deobf 0x140aebdc0). Builds the world-res loadlist virtual path via
/// `DlFixedString::FormatV` -- but ONLY when its `param_2` target BlockId (`*rdx`) is valid
/// (`!= -1` AND `IsNonDebugArea(area) == area < 0x59`). On our in-memory redirect load the caller
/// captured a stale/`-1` BlockId (GameMan+0xc30 was set too late), so FormatV is skipped, the loadlist
/// path stays empty, `STEP_MoveMap_LoadlistInit` builds nothing, `ProcessMsbLoadLists` is a no-op, the
/// dest `WorldBlockRes` is never created, and `STEP_WorldResWait` (mms_step 3) stalls forever -> the
/// render-handoff freeze. We hook this and, when armed by our own load trigger and `*param_2` is
/// invalid, substitute the freshly-deserialized saved-map BlockId from GameMan+0xc30 so the game's own
/// FormatV -> LoadlistInit -> ProcessMsbLoadLists -> world-stream -> STEP_Finish chain runs natively and
/// re-enables draw_group + dismisses the loading cover on its own. Root cause RE: bd
/// render-handoff-freeze-worldreswait-loadlist-root-2026-07-18.
pub(crate) const REQUEST_MOVE_MAP_RVA: usize = 0xaebdc0;
/// Area-id ceiling of `IsNonDebugArea` (deobf 0x140720210 == `areaId < 0x59`). A BlockId whose area
/// byte `((blockid >> 24) & 0xff)` is >= this is a debug area for which RequestMoveMap skips FormatV.
pub(crate) const REQUEST_MOVE_MAP_NONDEBUG_AREA_CEIL: u32 = 0x59;
pub(crate) const STREAMING_ENABLE_RVA: usize = 0x66e2e4;
/// Direct poke of the streaming-enable flag [resmgr+0xb7c1]=1 (the virtual enabler
/// 0x14066e2e4 crashes -- wrong receiver). The virtual also builds session singletons
/// 0x143d687a0 / 0x143d67bd0; read them to see if the poke is safe (already built) or
/// if the job machine will deref null.
pub(crate) const RESMGR_STREAM_ENABLE_B7C1_OFFSET: usize = 0xb7c1;
pub(crate) const SESSION_SINGLETON_A_RVA: usize = TitleSessionRva::SessionA as usize;
pub(crate) const SESSION_SINGLETON_B_RVA: usize = TitleSessionRva::SessionB as usize;
/// Corrected streaming-enable (worldres-enable-0x14066e2e4-decoded-receiver-and-
/// driver-singleton-2026): the CORRECT resmgr is deref(deref(MoveMapStep+0xf0)+0x10)
/// with vtable 0x142a7e030 (NOT InGameStep+0x250, which is the WorldRes-owner, vtable
/// 0x142a7de60 -- the wrong object that crashed). The hard floor is the streaming/
/// session driver singleton 0x143d7c088 (job machine asserts if null); build it via
/// the lazy getter 0x140cd6c50 before calling enable 0x14066e2e4(resmgr).
pub(crate) const RESMGR_EXPECTED_VTABLE_RVA: usize = 0x2a7e030;
pub(crate) const STREAMING_DRIVER_SINGLETON_RVA: usize = 0x3d7c088;
pub(crate) const STREAMING_DRIVER_BUILDER_RVA: usize = 0xcd6c50;
/// World-stream worker build+register: IngameInit's SetState tail 0x140b0a980, whose
/// `[this+0x48] >= 7` arm constructs the world-stream worker 0x144842d40 (ctor
/// 0x141eceb10) and registers it with the FD4 scheduler (key 0x59682f01 via
/// 0x142656b00) -- the piece our forced path skips (b80-initiate-advances-mms-but-
/// async-io-stalls). The arm uses ONLY globals/stack after the +0x48 check, so calling
/// it with a synthetic `this` (a zeroed buffer with +0x48=7) replicates the build
/// without needing the real 0x143d71340 step object.
pub(crate) const WORLD_WORKER_BUILD_RVA: usize = 0xb0a980;
pub(crate) const SYNTHETIC_STEP_THIS_SIZE: usize = 0x60;
pub(crate) const SYNTHETIC_STEP_STATE_OFFSET: usize = 0x48;
pub(crate) const WORLD_WORKER_BUILD_STATE: i32 = 7;
/// MISIDENTIFIED-CORRECTED (autoresearch 2026-06-18): 0x4842d40 is upstream `eldenring`'s
/// `runtime_heap_allocator` (the `DLAllocator` singleton, `rva::get().runtime_heap_allocator`),
/// confirmed by static RE -- it has 4057 RIP-relative refs (allocator footprint, not a task) and
/// the cached-singleton getter at 0x140078ed5. It is built at startup and is ALWAYS non-null, so
/// reading it non-null is NOT evidence that any "world-stream worker"/FD4 stream task was built.
/// The save-IO/worldres "worker present" levers below that relied on that inference are FALSE
/// POSITIVES and need the real stream-task RVA. Name kept generic and accurate; see bd
/// `rva-4842d40-is-heap-allocator-not-stream-task`.
pub(crate) fn runtime_heap_allocator_ptr_or_null() -> usize {
    DLAllocator::runtime_heap_allocator() as *const DLAllocator as usize
}
/// World/scene singletons built by MoveMapStep::STEP_MsbLoad 0x140af8f00. Non-null
/// == MsbLoad ran (the IsResident-relevant world exists). Diagnostic for whether the
/// worker is servicing the stream vs the b80 lane stalling first.
pub(crate) const WORLD_SINGLETON_A_RVA: usize = 0x3d691d8;
pub(crate) const WORLD_SINGLETON_B_RVA: usize = 0x3d69ba8;
/// World-resource manager chain for STEP_WorldResWait residency (0x14066d3e0):
/// resmgr = [[MoveMapStep+0xf0]+0x10]; loaded-block count = [resmgr+0xb3140].
/// count==0 -> no map-block registered (setup gap); count>0 but block not at load
/// phase 0xa -> streaming gap. Diagnostic for the final wall.
pub(crate) const MOVEMAPSTEP_WORLDRES_F0_OFFSET: usize = 0xf0;
pub(crate) const WORLDRES_RESMGR_10_OFFSET: usize = 0x10;
pub(crate) const RESMGR_BLOCK_COUNT_B3140_OFFSET: usize = 0xb3140;
pub(crate) const DIAG_NULL_CHAIN: i32 = -2;
/// The block coord/map-id the MoveMapStep requests in STEP_WorldResWait: at
/// [[MoveMapStep+0xf0]+0x2c] (0x140624bd0 reads byte3 as the target area). byte3 ==
/// 0x0a means slot 9's m10 IS being requested (loader/streaming issue); 0 means the
/// saved world position never loaded (coord issue).
pub(crate) const WORLDRES_COORD_2C_OFFSET: usize = 0x2c;
/// Resource-manager block array scan (mirrors 0x14066d3e0): entries at
/// [resmgr+0xb3030 + i*8]; each entry's block area = [[entry+0x8]+0xc]. We scan for
/// the target area 0x0a (m10) to learn if slot 9's block is registered (streaming
/// gap) or absent (loader never picks up the request).
pub(crate) const WORLDRES_BLOCK_ARRAY_B3030_OFFSET: usize = 0xb3030;
pub(crate) const BLOCK_ENTRY_AREAOBJ_8_OFFSET: usize = 0x8;
pub(crate) const BLOCK_AREAOBJ_AREA_C_OFFSET: usize = 0xc;
/// Recurring-observer aliases for the same resmgr block-array layout, named per the registration-vs-
/// streaming probe (own-load-worldreswait-is-block-registration-not-coord-2026-06-22). Defined as
/// aliases so the offsets live in exactly one place (no duplicated magic numbers). The recurring
/// observer scans base_arr = resmgr + RESMGR_BLOCK_ARRAY_B3030_OFFSET, stride 8, and for each
/// non-null block reads inner = *(block + BLOCK_INNER_8_OFFSET) then areaId = *(inner +
/// BLOCK_AREA_C_OFFSET) as u8 -- PURE READS, NO block->vtable call this round.
pub(crate) const RESMGR_BLOCK_ARRAY_B3030_OFFSET: usize = WORLDRES_BLOCK_ARRAY_B3030_OFFSET;
pub(crate) const BLOCK_INNER_8_OFFSET: usize = BLOCK_ENTRY_AREAOBJ_8_OFFSET;
pub(crate) const BLOCK_AREA_C_OFFSET: usize = BLOCK_AREAOBJ_AREA_C_OFFSET;
/// The target areaId is DERIVED from the requested block coord (wrm+0x2c / req_coord), not
/// hardcoded: areaId = (block_coord >> TARGET_AREA_FROM_COORD_SHIFT) & TARGET_AREA_FROM_COORD_MASK.
/// For the m28 save the low dword is 0x1c000000 so this yields 0x1c, but the value is data-driven.
pub(crate) const TARGET_AREA_FROM_COORD_SHIFT: u32 = 24;
pub(crate) const TARGET_AREA_FROM_COORD_MASK: u32 = 0xff;
/// Cap the recurring observer's block-array scan at min(block_count, this) for safety.
pub(crate) const OBSERVER_BLOCK_SCAN_CAP: i64 = 64;
/// How many distinct areaIds the observer collects for the log line.
pub(crate) const OBSERVER_AREAID_SAMPLE_MAX: usize = 8;
pub(crate) const TARGET_AREA_M10: i32 = 0x0a;
pub(crate) const BLOCK_SCAN_MAX: i32 = 64;
pub(crate) const BLOCK_ENTRY_STRIDE: usize = 8;
pub(crate) const BLOCK_SAMPLE_COUNT: usize = 4;
pub(crate) const BLOCK_AREA_BYTE_MASK: u32 = 0xff;
pub(crate) const BLOCK_SAMPLE_SHIFT: u32 = 8;
/// m10 block load-state (mirrors 0x14066d3e0 readiness tail): loadstate =
/// entry->vtable[+0x10](entry); ready iff [loadstate+0x2d]!=0 AND [loadstate+0x35]==0xa.
/// Reading [+0x35] live shows which load phase the m10 block is stuck at (<0xa).
pub(crate) const BLOCK_LOADSTATE_GETTER_VT_10_OFFSET: usize = 0x10;
pub(crate) const BLOCK_LOADSTATE_FLAG_2D_OFFSET: usize = 0x2d;
pub(crate) const BLOCK_LOADSTATE_PHASE_35_OFFSET: usize = 0x35;
/// OWN-LOAD m28 direct-enqueue lever (adddefaultfileloadprocess-lever-viable-2026-06-22).
/// `FD4::FD4FileCap::AddDefaultFileLoadProcess` deobf VA 0x142658c60 (prologue-grounded
/// `40 55 56 57 41 56 41 57`; dump 0x142658c50 is +0x10). Stored as an RVA offset from the
/// 0x140000000 image base, resolved at runtime as `module_base + RVA` like the other native-call
/// RVAs in this file (e.g. `CONTINUE_CONFIRM_RVA`). Signature (Win64 fastcall):
/// `bool AddDefaultFileLoadProcess(FD4FileCap* cap /*rcx*/, FD4FileLoadProcess* loadProcess /*rdx*/)`.
/// It builds the FD4FileLoadProcessor internally + self-enqueues IO to the already-live FD4 workers
/// (RequestDCX -> RSResourceFileRequest -> GLOBAL_LoadManager). PushTask / AssignFileCap are NOT
/// needed. Reaches ONLY world-asset file-load streaming -- no save IO, cannot autosave.
pub(crate) const ADD_DEFAULT_FILE_LOAD_PROCESS_RVA: usize = 0x142658c60 - 0x140000000;
/// FD4FileCap layout (struct len 0x90): the cap's EXISTING `FD4FileLoadProcess*` lives at +0x78 --
/// READ it for arg2, we never construct one. `loadState` at +0x88 == 4 means the cap is already
/// resident (skip). Both grounded in the Ghidra dump decomp of the lever.
pub(crate) const FILECAP_LOAD_PROCESS_78_OFFSET: usize = 0x78;
pub(crate) const FILECAP_LOADSTATE_88_OFFSET: usize = 0x88;
/// `loadState` sentinel meaning the FD4FileCap finished loading (already resident -> do not dispatch).
pub(crate) const FILECAP_LOADSTATE_COMPLETE: i32 = 4;
/// WorldBlockRes holds the m28 area's FD4FileCap(s): the primary at +0x40 and an OPTIONAL second at
/// +0x48 (the IsNonDebugArea branch; m28/0x1c populates both, and phase-2 gates on BOTH). Dispatch
/// each non-null cap. These are off the SAME WorldBlockRes entry the recurring observer block-walk
/// already finds for the player area.
pub(crate) const WORLDBLOCKRES_FILECAP_40_OFFSET: usize = 0x40;
pub(crate) const WORLDBLOCKRES_FILECAP2_48_OFFSET: usize = 0x48;
/// The resmgr 0xb3030 array entry `block` is a CONTAINER (WorldBlockData): the WorldBlockRes elements
/// live in an inline array at `*(block+0xce0)`, count `*(block+0xcd8)` (i32), stride 0xb98 -- decoded
/// from the keyed getter vt+0x8 (deobf 0x14062f470): `movslq 0xcd8(rcx); mov 0xce0(rcx),r11;
/// elem=r11+i*0xb98`. Each element is a WorldBlockRes (phase byte +0x35, caps +0x40/+0x48). We iterate
/// this array DIRECTLY (plain reads) instead of calling the getter -- the getter takes a second `key`
/// arg in rdx and AV-crashes if called without it.
pub(crate) const WORLDBLOCK_CONTAINER_COUNT_CD8_OFFSET: usize = 0xcd8;
pub(crate) const WORLDBLOCK_CONTAINER_ARRAY_CE0_OFFSET: usize = 0xce0;
pub(crate) const WORLDBLOCKRES_ELEM_STRIDE_B98: usize = 0xb98;
pub(crate) const DIAG_PHASE_NONE: i32 = -1;
pub(crate) const DIAG_COUNT_ZERO: i32 = 0;
pub(crate) const DIAG_COUNT_ONE: i32 = 1;
pub(crate) const DIAG_SAMPLE_ZERO: u32 = 0;
/// Global holding the GameMan pointer (`mov rax,[rip]` in set_save_slot 0x67a810
/// / save_slot_get 0x678ca0). Read-only diagnostics of the PlayGame load-pair
/// preconditions read GameMan through this.
pub(crate) fn game_man_ptr_or_null() -> usize {
    GameMan::instance_ptr().map_or(NULL_MODULE_BASE, |ptr| ptr as usize)
}
/// GameMan `save_slot` (compiler-verified equal to the upstream typed field).
pub(crate) const FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET: usize =
    core::mem::offset_of!(GameMan, save_slot);
/// Save-manager load-in-progress flag (GameMan/save-mgr singleton 0x143d69918):
/// `0x14067b570` sets `[mgr+0xb80]=1` when it begins the load and clears it to 0
/// when finished. The native autoload (recipe A) arms the load by setting the
/// slot (`+0xac0`) and the force flag `0x143d856a0`, then the save-manager
/// per-frame update `0x14067f5d0` performs it.
/// Bound to upstream `GameMan::save_state` (compiler-verified equal to our offset); our research
/// reads this same dword as the load-in-progress lane (set 1 on load begin, cleared on finish).
pub(crate) const GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET: usize =
    core::mem::offset_of!(GameMan, save_state);
/// Read-only autoload-arm precondition probe. The native save-mgr update
/// 0x14067f5d0 arms autoload (sets GameMan+0xb72=1 -> load) only when its gates
/// pass; the one runtime unknown is whether the slot-record container
/// [slotmgr+0x8] is populated at the pre-bootstrap title. These RVAs/offsets let
/// us read those preconditions without touching state.
/// Alias for the GameDataMan singleton RVA: the "slot manager" the save-snapshot probe reads IS
/// GameDataMan. Reference the canonical const so the RVA is decoded in exactly one place.
pub(crate) fn game_data_man_ptr_or_null() -> usize {
    GameDataMan::instance_ptr().map_or(NULL_MODULE_BASE, |ptr| ptr as usize)
}
/// GameDataMan -> main player save data (compiler-verified equal to the upstream typed field).
pub(crate) const SLOT_MANAGER_DATA_OFFSET: usize =
    core::mem::offset_of!(GameDataMan, main_player_game_data);
/// GameDataMan private tail fields used by the save/profile probes.
#[repr(C)]
pub(crate) struct GameDataManProfileSummaryLayout {
    pub(crate) unknown_000: [u8; 0x78],
    pub(crate) profile_summary: usize,
}

/// GameDataMan -> `profile_summary`; private upstream, but documented locally as a typed layout.
pub(crate) const SLOT_MANAGER_CONTAINER_OFFSET: usize =
    core::mem::offset_of!(GameDataManProfileSummaryLayout, profile_summary);
pub(crate) const CSFEMAN_SINGLETON_RVA: usize = 0x3d6b880;
/// Session manager singleton (absolute 0x1447ef360; NULL at the title, built by
/// the move-map/load path). RVA = 0x1447ef360 - 0x140000000 = 0x47ef360.
pub(crate) const SESSION_SINGLETON_RVA: usize = TitleSessionRva::MoveMapSession as usize;
pub(crate) const TITLE_INPUT_MANAGER_RVA: usize = 0x3d6b7b0;
/// Pure-observe snapshot interval (game-task ticks). Logs the title->menu->load state
/// every N ticks with NO forcing, to capture what the REAL button press does.
pub(crate) const OBSERVE_INTERVAL: u64 = 10;
/// Observe change-detection: log a snapshot only when the packed signature changes
/// (full granularity, minimal file I/O). Multiplier for the rolling signature.
pub(crate) const OBSERVE_SIG_MULT: i64 = 0x100000001b3;
pub(crate) static OBSERVE_LAST_SIG: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(i64::MIN);
/// OWN-THE-STEPPER (own-stepper-control-verified-and-driver-call-2026): the
/// SimpleTitleStep step-fn table (base abs 0x143d71580, owner+0x10) is in WRITABLE
/// .data. idx10 = STEP_MenuJobWait func slot = base + 10*0x10 = abs 0x143d71620
/// (RVA 0x3d71620) is dispatched every frame at the press-any-button title. We patch
/// it to our own handler so the FD4 scheduler runs OUR code IN-CONTEXT (rcx=owner,
/// rdx=FD4Time), instead of trampolining from an external CSTask.
pub(crate) const TITLE_STEP_IDX10_SLOT_RVA: usize = 0x3d71620;
/// Native Continue/Load confirm handler (reads owner=[rcx+8]; slot-select + child
/// request + SetState(5)). Invoked via a {[+8]=owner} shim.
pub(crate) const CONTINUE_CONFIRM_RVA: usize = 0xb0e180;
/// Continue/Load MANAGER object global (.data abs 0x143d5df38; ==0 at rest in the deobf
/// image, built at runtime). `[mgr]` = the manager vtable, `[mgr + 8]` = the recipe's
/// literal "owner" used by the native-fullread COMMIT recipe. Used READ-ONLY here for the
/// OWN-LOAD owner diagnostic; the continue_confirm owner is the threaded SetState-able
/// title owner (see `own_load_continue_fire`), NOT this literal.
pub(crate) const CONTINUE_MANAGER_GLOBAL_RVA: usize = 0x3d5df38;

/// LoadGame-JOB BUILD factory (`FUN_140826510` live; dump VA `0x140826600` lands +0xF0 mid-instr in
/// the deobf image -- the real prologue is here, prologue-grounded vs `eldenring-deobf.bin`). Builds
/// the LoadGame `CS::MenuJobWithContext<LoadJobContext>` via the menu-heap factory and returns it in
/// `*out` with refcount 1. Win64 fastcall `(out: *DLRefCountPtr<MenuJob>, ctx_parent, save_slot:i32,
/// owner_ctx)`. Only `out` (our local) and `save_slot` (the int slot) are required by the deser/map
/// self-build path; `ctx_parent`/`owner_ctx` are the OUTER profile-selection UI context (stored as
/// lambda captures, every build-path deref null-guarded) -- passed as 0 here (see `own_load_install_job`).
pub(crate) const LOADGAME_JOB_BUILD_RVA: usize = 0x826510;
/// `DLUT::DLReferenceCountPointer<MenuJob>` ASSIGN/INSTALL helper (`FUN_1407a9560` live; dump entry
/// `0x1407a9650` is the same fn, prologue-grounded vs `eldenring-deobf.bin`). Win64 fastcall
/// `(slot: *MenuJob*, src: *MenuJob* (longlong*))`: writes `*slot = *src`, `AtomicIncrement`s the new
/// occupant, then `AtomicDecrement`s/releases the PRIOR occupant and zeroes `*src` (move-assign).
/// Installs the built job into `owner+0x130`, releasing the idle `IfElseJob` it replaces.
pub(crate) const MENUJOB_ASSIGN_RVA: usize = 0x7a9560;
/// `CS::MenuJobQueue::PushBackJob` (live entry `0x1407a9250` -- prologue-grounded vs eldenring-deobf.bin:
/// `mov [rsp+0x10],rdx; push rdi; sub rsp,0x30; movq $-2,[rsp+0x20]`; dump `FUN_1407a9340`). CORRECTED
/// from the prior `0x7a9254`, which was +4 INTO the first instruction (mid-`mov`) and would execute
/// garbage -- a latent bug that likely helped kill the gated `own_load_install_job` path. APPENDS a job
/// into a MenuJobQueue's auto-growing deque ring (`AtomicIncrement`s the job, ring-push behind the
/// active job) -- does NOT replace the active job or zero `*src`, and is overflow-safe (NOT the cap-8
/// FixOrderJobSequence). Win64 fastcall `(rcx = queue_base, rdx = src: *MenuJob* (a DLReferenceCount
/// Pointer slot whose [0] is the job))`. Queue targets: `owner+0x130` (ring +0x138, count +0x178;
/// STEP_MenuJobWait's ExecuteMenuJob ticks it) OR `dialog+0x10` (ring +0x18; the per-frame menu pump
/// 0x1409aa680 over the active-screen array drains it -- the native Continue post target).
/// bd continue-load-POST-primitive-pushbackjob-kick-2026-06-22.
pub(crate) const MENUJOB_PUSHBACK_RVA: usize = 0x7a9250;
/// MenuJobQueue field offsets (for diagnostics): the queued-job ring count at +0x178 grows by 1 on a
/// successful PushBackJob; the active job stays at +0x130.
pub(crate) const MENUJOB_QUEUE_COUNT_178_OFFSET: usize = 0x178;
/// The MenuJob slot `CS::TitleStep::STEP_MenuJobWait` ticks every frame via
/// `ExecuteMenuJob((MenuJob**)&owner->field85_0x130, &time)`. Installing the LoadGame job here makes
/// the per-frame title step drive it (self-build -> deser -> world stream). Owner-relative byte offset.
pub(crate) const TITLE_OWNER_MENUJOB_SLOT_130_OFFSET: usize = 0x130;
/// LoadGame `MenuJobWithContext<LoadJobContext>` vtable (dump VA `0x142ac71e0`). DIAGNOSTIC ONLY: the
/// installed job's vtable should read back as this (modulo the dump->live `.rdata` shift) -- logged,
/// never used to gate the call. The IfElseJob it replaces reads vtable dump `0x142aa2958`.
pub(crate) const MENUJOB_LOADGAME_VTABLE_DUMP_VA: usize = 0x142ac71e0;
/// Idle title `CS::MenuJobSequence::IfElseJob` vtable (dump VA `0x142aa2958`) that occupies
/// `owner+0x130` before install. DIAGNOSTIC ONLY (logged for the before/after vtable-flip evidence).
pub(crate) const MENUJOB_IFELSE_VTABLE_DUMP_VA: usize = 0x142aa2958;
/// MenuJob `+0x68` built-flag byte (0 before first Run tick, 1 after self-build) and `+0x70` inner
/// FixOrderJobSequence ptr (0 -> built). DIAGNOSTIC ONLY: dumped before/after to witness self-build.
pub(crate) const MENUJOB_BUILT_FLAG_68_OFFSET: usize = 0x68;
pub(crate) const MENUJOB_INNER_SEQ_70_OFFSET: usize = 0x70;
/// `CS::FixOrderJobSequence::currentJobIndex` (`+0x10`) on the IfElseJob/inner seq -- advances as the
/// job sequence steps. DIAGNOSTIC ONLY (dumped before/after install).
pub(crate) const MENUJOB_CURRENT_JOB_INDEX_10_OFFSET: usize = 0x10;

