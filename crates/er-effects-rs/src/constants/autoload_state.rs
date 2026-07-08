// ---------------------------------------------------------------------------
// NATIVE-LOAD gate (observe-only own_stepper; corrected-autoload-design-observe-not-force-native-load-2026).
// A SEPARATE gate from own_stepper: when enabled, the idx10 handler does NOT force the title
// state machine (no SetState(2/3), no beginlogo-gate clear, no registrar self-fire, no
// direct_build / cold_char_mount). It lets OWN_STEPPER_ORIG_IDX10 pass-through advance the NATIVE
// title machine, and ONCE the live TitleTopDialog menu is rendered + settled, it fires the native
// Load-Game MenuMemberFuncJob node's run 0x1409aaba0(rcx=node) exactly ONCE, then observes so the
// golden oracle is written as the native pump loads the char.
// ---------------------------------------------------------------------------
/// CS::MenuMemberFuncJob<TitleTopDialog>::run 0x1409aaba0 (RVA 0x9aaba0). Takes rcx=node (a
/// MenuMemberFuncJob, vtable TITLE_TOP_DIALOG run-node = MEMBERFUNCJOB_VTABLE_RVA); internally it
/// computes rcx=[node+0x10]+[node+0x20] (the member `this`, dialog + adjustor) and calls the
/// member-fn pointer at [node+0x18] -- which chains to the Load-Game dialog factory 0x14081ead0.
/// Firing it on the NATURALLY-booted menu builds a LIVE registered ProfileLoadDialog the native
/// pump drives (the live-dialog MenuWindow wall was a forcing artifact -- this de-risks step 4).
pub(crate) const MENU_MEMBER_FUNC_JOB_RUN_RVA: usize =
    ProfileLoadMenuRva::MenuMemberFuncJobRun as usize;
/// CS::MenuMemberFuncJob<TitleTopDialog> vtable 0x142b265d0 (RVA): the registry-entry node the
/// registrar 0x1409b24e0 inserts into [dialog+0xa48]; its run is MENU_MEMBER_FUNC_JOB_RUN_RVA.
/// (Mirrors the local MEMBERFUNCJOB_VTABLE_RVA in scan_dialog_for_loadgame.)
pub(crate) const MEMBERFUNCJOB_VTABLE_RVA: usize = 0x2b265d0;
/// TitleTopDialog row registry [dialog+0xa48] (the FD4 delegate registry the registrar populates).
/// Used as the live-menu readiness signal: populated == the menu rows are registered + rendered.
pub(crate) const DIALOG_ROW_REGISTRY_A48_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, row_registry);
/// NATIVE-LOAD fire latch states (one-shot: fire the Load-Game run exactly once).
pub(crate) const NATIVE_LOAD_FIRED_NO: usize = 0;
pub(crate) const NATIVE_LOAD_FIRED_YES: usize = 1;
pub(crate) static NATIVE_LOAD_FIRED: AtomicUsize = AtomicUsize::new(NATIVE_LOAD_FIRED_NO);
pub(crate) static NATIVE_LOAD_LAST_NODE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static NATIVE_LOAD_LAST_NODE_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static NATIVE_LOAD_LAST_MEMBER_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static NATIVE_LOAD_LAST_MEMBER_FN: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static NATIVE_LOAD_LAST_MEMBER_ADJUST: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The native-load observer now fires only when `title_menu_action_ready` validates the concrete
/// Load-Game `MenuMemberFuncJob` node/action; there is no fixed post-menu settle frame count.
/// Throttle interval for native-load observe logging (frames).
pub(crate) const NATIVE_LOAD_LOG_INTERVAL: u64 = 120;

/// === NATIVE FULL-SAVE-READ observe chain (native-full-save-read-slot-resolve-chain-observe-recipe-2026). ===
/// The slot-resolve GLOBAL the menu cursor / Continue selection writes: resolver 0x1406793c0 returns
/// *(u32*)(GameMan+0xb78). Step 1 of the recipe sets GameMan+0xb78=slot before set_save_slot so the
/// native chain resolves OUR slot. (Same offset as GAME_MAN_REQUESTED_SLOT_B78_OFFSET; named per the
/// recipe for the full-read chain.)
pub(crate) const GAME_MAN_SLOT_SELECT_B78_OFFSET: usize =
    core::mem::offset_of!(GameMan, requested_save_slot_load_index);
/// GameMan+0xb80 == 3 == RESIDENT (the full-save read drained into the 0x280000 buffer). The DRAIN
/// phase ticks the lane + poll each frame until b80 reaches this.
pub(crate) const FULLREAD_B80_RESIDENT: i32 = 3;
/// GameMan+0xc30 m10 new-game default (golden-oracle-baseline). c30 == this == FAILURE (the char did
/// NOT deserialize). The step-6 guard requires c30 != this before the (gated) continue_confirm.
pub(crate) const FULLREAD_C30_M10_DEFAULT: i32 = 0xa010000;
/// Minimum REAL character level (a new-game default is <10; the golden Banon is 150). The step-6
/// guard requires the live PlayerGameData level >= this AND a non-empty name (via char_fingerprint).
pub(crate) const FULLREAD_MIN_REAL_LEVEL: u32 = 10;
/// Poll arg (0) for the b80 poll 0x140679180 and the lane driver 0x140679510 in the DRAIN phase.
pub(crate) const FULLREAD_POLL_ARG: u8 = 0;
/// DRAIN-phase budget: max frames to tick lane+poll waiting for b80==3 before TIMEOUT (no write).
pub(crate) const FULLREAD_DRAIN_MAX: u64 = 1200;
/// Throttle interval for the full-read chain per-frame logging (frames).
pub(crate) const FULLREAD_LOG_INTERVAL: u64 = 30;
/// Default slot for the full-read chain when neither OWN_STEPPER_SLOT (>=0) nor ER_EFFECTS_AUTOLOAD_SLOT
/// is set (Banon = slot 0).
pub(crate) const FULLREAD_DEFAULT_SLOT: i32 = 0;
/// continue_confirm shim field that owner+0x284 (new-game flag) must equal before the confirm runs
/// the SetState5: the native continue_confirm reads owner = *(shim[OWN_STEPPER_SHIM_OWNER_IDX]) =
/// *(base+0x3d5df38+8), checks owner+0x284==0, then sets owner+0xbc=c30 + SetState5 (autosaves).
pub(crate) const FULLREAD_OWNER_NEW_GAME_OK: u8 = 0;
/// owner = *(game_data_man_ptr_or_null() + this offset) -- the GameDataMan+0x8 chain the
/// continue_confirm shim owner is read from (recipe step 7: owner = *(base+0x3d5df38+8)).
pub(crate) const FULLREAD_OWNER_GDM_08_OFFSET: usize = 0x08;
/// Full-read chain phase machine states (one step per frame).
pub(crate) const FULLREAD_PHASE_SUBMIT: usize = 0;
pub(crate) const FULLREAD_PHASE_DRAIN: usize = 1;
pub(crate) const FULLREAD_PHASE_DESER: usize = 2;
pub(crate) const FULLREAD_PHASE_GUARD: usize = 3;
pub(crate) const FULLREAD_PHASE_DONE: usize = 4;
/// Live phase + drain-wait counters for the full-read chain (one-shot per run).
pub(crate) static FULLREAD_PHASE: AtomicUsize = AtomicUsize::new(FULLREAD_PHASE_SUBMIT);
pub(crate) static FULLREAD_DRAIN_WAITS: AtomicUsize = AtomicUsize::new(0);
/// Terminal non-commit disarm counters for the full-read chain (bd er-effects-rs-ns4n). SUBMIT arms
/// the native slot-request register (GameMan+0xb78, `requested_save_slot_load_index`); the in-game
/// save manager services any >=0 request on the first frames after world arrival, running a second
/// full deserialize into the live world (CSGaitemImp free-queue exhaustion, AV at live 0x67141a).
/// Every DONE exit that does not hand off to the native confirm chain must clear the register; these
/// count the clears and record the slot value the last clear removed (u32-packed i32; !0 == none).
pub(crate) static FULLREAD_REQ_DISARM_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static FULLREAD_REQ_DISARM_LAST_PREV_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
/// LATCHED peak-load semaphore (bd er-effects-rs-ns4n follow-up). The live `oracle_char_*` fields read
/// PlayerGameData directly, so a quit-to-title tears the character down and a final telemetry snapshot
/// reads them empty even on a fully-successful run -- the load proof lived only in the mid-run
/// `LOAD-CORRECTNESS` log line. These latch the highest-level REAL character ever confirmed in-world
/// this run (set once by `dump_load_correctness` when pgd is present with level>=1 and a non-empty
/// name), so `oracle_load_correctness_seen > 0` proves a real char reached the world regardless of a
/// later quit. Process-lifetime (never reset within a session): it attests "a real character loaded at
/// some point this run", which a quit or a later System->Quit switch cannot falsify.
pub(crate) static LOADED_PEAK_SEEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADED_PEAK_LEVEL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADED_PEAK_C30: AtomicI32 = AtomicI32::new(0);
pub(crate) static LOADED_PEAK_NAME_LEN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADED_PEAK_NAME: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
/// The native full-read chain shares the semantic `title_menu_action_ready` menu readiness gate;
/// it no longer latches a first-seen frame before starting the save-read phase machine.
/// `save_requested`: bound to the upstream typed layout (compiler-verified equal to our prior
/// hand-decoded offset).
pub(crate) const GAME_MAN_ARM_FLAG_B72_OFFSET: usize =
    core::mem::offset_of!(GameMan, save_requested);

#[repr(C)]
pub(crate) struct GameManAutoloadFlagCluster {
    pub(crate) save_requested: u8,
    pub(crate) probe_b73: u8,
    pub(crate) probe_b74: u8,
    pub(crate) probe_b75: u8,
}

pub(crate) const GAME_MAN_FLAG_B73_PROBE_OFFSET: usize =
    GAME_MAN_ARM_FLAG_B72_OFFSET + core::mem::offset_of!(GameManAutoloadFlagCluster, probe_b73);
pub(crate) const GAME_MAN_FLAG_B75_PROBE_OFFSET: usize =
    GAME_MAN_ARM_FLAG_B72_OFFSET + core::mem::offset_of!(GameManAutoloadFlagCluster, probe_b75);
/// `requested_save_slot_load_index`: bound to upstream (compiler-verified equal to our offset).
pub(crate) const GAME_MAN_REQUESTED_SLOT_B78_OFFSET: usize =
    core::mem::offset_of!(GameMan, requested_save_slot_load_index);
pub(crate) const GAME_MAN_FLAG_BC4_OFFSET: usize =
    core::mem::offset_of!(GameMan, is_in_online_mode) - core::mem::size_of::<u32>();
/// Submit-gate diagnostics (b80-submit-kick-exact-false-gate-decoded-2026). The b72
/// autoload initiator 0x14067b750 sets GameMan+0xb80=1 ONLY if the async submit
/// 0x140e6ec70 returns true; the submit body 0x140e6f940 bails FALSE if the IO device
/// has a STALE request in-flight ([iodev+0x10]!=0) or a stale request handle
/// ([iodev+0x20]!=0). The IO device global is abs 0x144589390 (RVA 0x4589390); we read
/// it both as a possible pointer-to-device and as a struct base so the log
/// disambiguates. Also: the b72 effective-getter 0x1406793d0 zeroes b72 if
/// [GameMan+0xbc4]==3 or [inputmgr+0x13c]!=0, so log those too.
pub(crate) const IODEV_GLOBAL_RVA: usize = 0x4589390;
pub(crate) const IODEV_INFLIGHT_10_OFFSET: usize = 0x10;
/// The async-IO request handle the poll 0x140e6e080 actually reads is the PAIR
/// [iodev+0x18] && [iodev+0x20] (a *started* request). 0x14067b4e0's preview read
/// (0x140e6ec80) is what populates these; 0x14067b200's queue (0x140e6eb80) goes to
/// the file-device-mgr instead, so it never appears here. Logging both pins which
/// initiator actually started the iodev read (menu-b80-mount-orchestration-sequence).
pub(crate) const IODEV_REQHANDLE_18_OFFSET: usize = 0x18;
pub(crate) const IODEV_REQHANDLE_20_OFFSET: usize = 0x20;
/// The save-DEVICE MOUNT/OPEN routine 0x140e6e8d0(rcx=iodev): the title->Continue boot
/// (single native call site 0x140defec2) runs it to BIND the .sl2 file to the IO device.
/// It opens the OS handle (via 0x140e45660), registers the save paths, then writes the
/// open status byte to [iodev+0x40] @0x140e6eb56 -- the device-ready flag the async
/// router 0x140e6eb80 tests (jne BOUND real-read 0x140e6f430 / else COLD empty-noop
/// 0x140e6f5b0). The menu-free cold path SKIPS this, so [iodev+0x40]==0 and the cold
/// async full read no-ops EMPTY (b80 2->0, never resident=3). Calling it before the
/// submit routes the read through the bound branch. Internally gated by 0x14240acd0(
/// [0x143d872e0]) which needs the IO worker registry [0x144843038+0x18]!=0. Decoded in
/// bd b80-mount-routine-0x140e6e8d0-recipe-and-guard-open-question-2026-06-21.
pub(crate) const IODEV_MOUNT_OPEN_RVA: usize = 0xe6e8d0;
/// The iodev getter 0x140e6e060() -> iodev (lazily creates the singleton if null).
pub(crate) const IODEV_GETTER_RVA: usize = 0xe6e060;
/// ROOT-CAUSE FIX (b80-ROOTCAUSE-worker-empty-iodev-dir-string-...): the cold full read
/// completes EMPTY because the worker builds a MALFORMED save path -- the request's
/// directory std::u16string is unset (the worker's `"%s\%s%s%s"` format yields a bare
/// `.sl2`). The LIVE title->Continue boot populates that directory via the iodev state
/// machine (opcode 0x17/0x18 handler 0x140e6ded0): it builds `<userdata>/EldenRing/<steamid>/`
/// then installs it on the path DB. The menu-free cold path skips that opcode, so the
/// directory is never set. PRE-submit replay is REFUTED (io20=[iodev+0x20] is NULL before
/// submit; bd b80-COLD-FIX-REFUTED-...). The correct replay is POST-submit, on the LIVE
/// io20, in the SAME game-task invocation (tightest race vs the worker drain):
///   1. SAVE_DIR_BUILDER 0x140e0e680(rcx=&wrapper): self-fetches the userdata folder
///      (SHGetFolderPathW CSIDL 0x1a) + Steam id (0x140e8d550) and formats `%s/EldenRing/%s/`
///      (fmt @0x142bda858) into the wrapper. Guarded by the Steam interface pointer
///      *0x143b48ff0 being non-null (else it would deref null).
///   2. SAVE_DIR_SETTER 0x14240a2a0(rcx=io20 path-DB, edx=slot=0, r8=raw char16_t*): stores
///      the directory into the path database (via 0x14240dce0 -> entry+0xb0, which COPIES
///      our buffer) -- exactly what the opcode-0x17/0x18 handler does. r8 is the RAW data
///      pointer (cap>=8 ? heap ptr @+0x08 : &SSO @+0x08), NOT the wrapper object.
pub(crate) const SAVE_DIR_BUILDER_RVA: usize = 0xe0e680;
pub(crate) const SAVE_DIR_SETTER_RVA: usize = 0x240a2a0;
/// The wrapper's stateful allocator getter (0x141eba960): `call 0x141ebb680; add rax,0x28`
/// -- a trivial singleton accessor returning the arena ptr SAVE_DIR_BUILDER stores at the
/// wrapper's +0x00 (the string's stateful allocator). Must be installed before the builder.
pub(crate) const SAVE_DIR_ALLOC_GETTER_RVA: usize = 0x1eba960;
/// Path-DB slot-entry lookup (0x14240c270): rcx=collection ([io20]), edx=key ([io20+8]) ->
/// entry (find-or-create; idempotent post-setter). The setter writes the directory into
/// `entry+0xb0`. Used for the post-setter readback.
pub(crate) const SAVE_DIR_SLOT_LOOKUP_RVA: usize = 0x240c270;
/// Steam-interface guard pointer (abs 0x143b48ff0): SAVE_DIR_BUILDER derefs the Steam
/// interface to read the account id; if this is null the builder must be skipped.
pub(crate) const STEAM_INTERFACE_GUARD_RVA: usize = 0x3b48ff0;
/// Active SteamID64 getter (0x140e8d590): returns the current signed-in Steam account's full
/// SteamID64 as a `u64`. Static-grounded from the SAVE_DIR_BUILDER chain; used to normalize staged
/// foreign save bytes before native deserialize stores them in GameDataMan/ProfileSummary.
pub(crate) const STEAM_ID64_GETTER_RVA: usize = 0xe8d590;
/// SAVE_DIR_BUILDER's output is a MSVC `basic_string<char16_t, ..., StatefulAllocator>`
/// (the stateful allocator occupies the first member): allocator ptr at +0x00, the _Bx
/// SSO/heap union at +0x08 (8 char16 SSO when cap<8, else `char16_t*`), _Mysize (code units)
/// at +0x18, _Myres (capacity) at +0x20. A default-empty string has size=0 and cap=7. The
/// builder ASSUMES a pre-constructed empty string, so we pre-init allocator/+0x20=7 before
/// the call. (This differs from a stateless-allocator string whose data union is at +0x00.)
pub(crate) const U16STRING_ALLOC_OFFSET: usize = 0x00;
pub(crate) const U16STRING_DATA_OFFSET: usize = 0x08;
pub(crate) const U16STRING_SIZE_OFFSET: usize = 0x18;
pub(crate) const U16STRING_CAP_OFFSET: usize = 0x20;
pub(crate) const U16STRING_SSO_CAP: usize = 7;
/// [iodev+0x40] = the device-ready/bound byte flag (0 cold; set by the mount above).
pub(crate) const IODEV_READY_FLAG_40_OFFSET: usize = 0x40;
/// [iodev+0x30] = the OS file-handle slot (0xffffffff invalid until the mount opens it).
pub(crate) const IODEV_OS_HANDLE_30_OFFSET: usize = 0x30;
/// The FD4 IO worker REGISTRY singleton (abs 0x144843038); its size/count is at +0x18.
/// The mount's guard 0x14240acd0 bails (no open) when [registry+0x18]==0 (no workers
/// registered), so logging it tells us whether the mount can fire at the cold state.
pub(crate) const IO_WORKER_REGISTRY_RVA: usize = 0x4843038;
pub(crate) const IO_WORKER_REGISTRY_COUNT_18_OFFSET: usize = 0x18;
/// The FD4 IO worker MANAGER singleton (abs 0x144852f88) the read job is posted to. The
/// enqueue 0x14240e420 IMMEDIATELY DISCARDS the request (no-op completion 0x14240a000,
/// status 0xe, b80 2->0 in one frame) when [worker+0x19]!=0 (the worker no-accept/shutdown
/// byte) @0x14240e472. Prime suspect for the read-completes-empty wall (b80-DEVICE-MOUNT-
/// REFUTED-...).
pub(crate) const FD4_IO_WORKER_MGR_RVA: usize = 0x4852f88;
pub(crate) const FD4_IO_WORKER_NOACCEPT_19_OFFSET: usize = 0x19;
/// The worker's job QUEUE fields the normal (non-discard) enqueue pushes to: 0x14240e420
/// pushes onto [worker+0x8] (via 0x14240c060) and [worker+0x10] (via 0x14240f2c0). Reading
/// these before vs after the submit DISTINGUISHES enqueued (queue changes) from DISCARDED
/// (queue unchanged) -- the decisive fork for the read-completes-empty wall.
pub(crate) const FD4_IO_WORKER_QUEUE_08_OFFSET: usize = 0x8;
pub(crate) const FD4_IO_WORKER_QUEUE_10_OFFSET: usize = 0x10;
/// The FD4 IO thread POOL singleton (abs 0x144853048).
pub(crate) const FD4_IO_POOL_RVA: usize = 0x4853048;
/// The 2nd discard gate 0x141ee1240 searches the worker-registry's intrusive list at
/// [registry+0x28] for a node matching a key from the calling context (lock 0x141ee05f0);
/// returns false (=> DISCARD) when not found (e.g. the calling thread is not a registered
/// IO context). Empty when [[registry+0x28]] == [registry+0x28].
pub(crate) const IO_WORKER_REGISTRY_LIST_28_OFFSET: usize = 0x28;
pub(crate) const INPUTMGR_PENDING_13C_OFFSET: usize = 0x13c;
pub(crate) const ARM_PROBE_MIN_TICK: u64 = 60;
pub(crate) const ARM_PROBE_TICK_INTERVAL: u64 = 30;
/// Lever 2 (zero-input title-accept via input-event injection). Inner TitleStep
/// state is at owner+0x4c (==10 MenuJobWait); the press-any-button job is at
/// owner+0x130; its vtable[+0x18] fills a descriptor whose first i32 indexes the
/// event table 0x143d6a860 (stride 0x60); eventId=[entry+4], value=[entry+8];
/// the game's node update writes inputmgr(0x143d6b7b0)+0xdc+eventId*4 = value.
/// Injecting that event makes the game's own node update accept and run the real
/// front-end bootstrap. Verdict is [job+0x1e8] >= 2.
/// The press-any-button job (owner+0x130) is an AND-combiner (vtable RVA
/// 0x2aa2958) over child condition nodes at [job+0x18 + i*8], count [job+0x60].
/// The real input node is the child with vtable RVA 0x2aa97e8; its keycode is at
/// child+0x180. Accept = set the inputmgr keystate bitmap (inputmgr+0x90+keycode
/// |= 3 pressed+triggered) so the leaf returns accepted and the combiner ANDs to
/// done -> MenuJobWait advances 10->11 and the front-end bootstraps.
/// Logical input-event array on the inputmgr (inputmgr+0xdc, i32 per event id,
/// ids 0..=0x15e). The leaf input node detects a press via this layer (then
/// mirrors into the keystate bitmap), so injecting here is what actually accepts.
pub(crate) const TITLE_ACCEPT_LATCH_RVA: usize = 0x3d856a0;
/// Boot intro/movie singleton (ptr) and its decoder skip-flag byte. The latch
/// 0x143d856a0 is set by the intro thread 0x140c8fe90 only after its movie-wait
/// loop ends; the movie-dismiss gate 0x140e90820 finishes on decode-complete or
/// when the skip-flag byte 0x14458b8a5 is non-zero (sole non-WNDPROC effect is the
/// movie's own stop). Setting the skip-flag drives a genuine zero-input dismiss.
pub(crate) const MOVIE_SINGLETON_RVA: usize = 0x458b890;
pub(crate) const MOVIE_SKIP_FLAG_RVA: usize = 0x458b8a5;
pub(crate) const MOVIE_SKIP_FLAG_CLEAR: u8 = 0;
pub(crate) const MOVIE_SKIP_FLAG_SET: u8 = 1;
/// Movie controller vtable RVA (0x142bfe088), HWND field offset (M+8), and the
/// USER32 constants for mirroring the WNDPROC WM_CLOSE teardown.
pub(crate) const MOVIE_VTABLE_RVA: usize = 0x2bfe088;
pub(crate) const MOVIE_HWND_OFFSET: usize = 0x8;
pub(crate) const WND_SC_CLOSE: u32 = 0xf060;
pub(crate) const WND_MF_BYCOMMAND: u32 = 0;
pub(crate) const WND_SW_HIDE: i32 = 0;
pub(crate) const WND_GET_SYSTEM_MENU_KEEP: i32 = false as i32;
/// Render-thread liveness probe logging cadence (in render frames).
pub(crate) const RENDER_PROBE_INTERVAL: usize = 120;
/// Splash-skip static patch (ports chozandrias76/er-skip-splash-screens to 1.16.1):
/// inside STEP_BeginLogo 0x140b0c2a0, the branch `cmp [rdi+0xb8],0; je 0x140b0c3b2`
/// at RVA 0xb0c35d plays the logo when the byte is 0; flipping je(0x74)->jg(0x7f)
/// falls through to the SetState(state 3) advance instead, skipping the logo via
/// the game's own flow. Applied early (DLL attach) before the title runs state 2.
pub(crate) const SPLASH_SKIP_RVA: usize = 0xb0c35d;
pub(crate) const SPLASH_SKIP_EXPECTED_JE: u8 = 0x74;
pub(crate) const SPLASH_SKIP_REPLACEMENT_JG: u8 = 0x7f;
pub(crate) const SPLASH_PATCH_LEN: usize = 1;
/// ONLINE-DISABLE (headless offline boot, no "Unable to start in online mode" modal).
/// `GameMan::IsOnlineMode` getter 0x14067a030 = `mov rax,[rip+..]; movzx eax,[rax+0xbc8]; ret`
/// (the canonical online/offline flag, default 1=online, read by ~22 consumers incl. the boot
/// login flow). Patching the getter body to `xor eax,eax; ret` forces every consumer onto the
/// game's own OFFLINE branch, so the boot never attempts online login and the connection-error
/// modal is never raised. Single leaf accessor, no side effects -> equivalent to "Play Offline";
/// no save/crash risk. Verified (self-disasm, online-disable RE 2026-06-17): first byte 0x48.
pub(crate) const ONLINE_DISABLE_RVA: usize = 0x67a030;
pub(crate) const ONLINE_DISABLE_EXPECTED_FIRST: u8 = 0x48;
/// `xor eax,eax; ret` -- returns 0 (offline) for the whole getter (the original body is 15
/// bytes followed by the next function, so a 3-byte stub is self-contained).
pub(crate) const ONLINE_DISABLE_STUB: [u8; 3] = [0x31, 0xc0, 0xc3];
pub(crate) const ONLINE_DISABLE_PATCH_LEN: usize = 3;
pub(crate) const ONLINE_DISABLE_BYTE_STEP: usize = 1;
/// Foreground-force: `CS::CSWindowImp::IsGameInForeground` (0x14266def0,
/// `return this->windowHandle == GetForegroundWindow()`) is the engine's foreground oracle; the
/// present/flip pacer `UpdateFlipTiming` (0x140e829d0) and friends throttle the game to a few fps
/// when it returns false. An UNFOCUSED probe window therefore runs at ~6 fps and never boots in the
/// runtime cap. Patch it to `mov al,1; ret` so the game always believes it is foreground -> full
/// speed regardless of focus. Safe for the probe: input is blocked, and "always foreground" only
/// removes the background throttle/pause. Verified prologue first byte 0x40 (`push rbx`).
// NB: address ground-truthed against the deobf/live binary (scripts/disas-deobf.sh), NOT the Ghidra
// dump -- the dump placed this fn at 0x14266def0 but the live entry is 0x14266df00 (dump<->deobf has
// regional shifts; trust the deobf binary for addresses to patch/call).
pub(crate) const FOREGROUND_FORCE_RVA: usize = 0x266df00;
pub(crate) const FOREGROUND_FORCE_EXPECTED_FIRST: u8 = 0x40;
/// `mov al,1; ret` -- returns true (foreground) for the whole getter.
pub(crate) const FOREGROUND_FORCE_STUB: [u8; 3] = [0xb0, 0x01, 0xc3];
/// Sign-in force (cold save-load gate). The SaveLoad2 storage-select op ctor (deobf 0x14240f1b0)
/// creates its runnable ONLY if the sign-in check returns true AND the user index is <= 3; cold
/// (no signed-in user) both fail, so the op is null and the load FSM parks (the b80 wall). Patch
/// both gate fns to pass so the cold menu-free path loads as if signed in as user 0. Addresses
/// ground-truthed against the deobf/live binary (the Ghidra dump's FUN_1424129a0 / FUN_14240f480
/// are shifted; live entries below). Scoped to the cold-mount attempt, not attach.
/// `CS::..::IsSignedIn`-class check (dump FUN_1424129a0) -> always true.
pub(crate) const SIGNIN_FORCE_RVA: usize = 0x24129b0;
pub(crate) const SIGNIN_FORCE_EXPECTED_FIRST: u8 = 0x40;
pub(crate) const SIGNIN_FORCE_STUB: [u8; 3] = [0xb0, 0x01, 0xc3]; // mov al,1; ret
/// User-index resolver (dump FUN_14240f480) -> return 0 (valid index, <= 3) instead of 0xffffffff.
pub(crate) const USERINDEX_FORCE_RVA: usize = 0x240f490;
pub(crate) const USERINDEX_FORCE_EXPECTED_FIRST: u8 = 0x4c;
pub(crate) const USERINDEX_FORCE_STUB: [u8; 3] = [0x31, 0xc0, 0xc3]; // xor eax,eax; ret
/// Login-readiness predicate 0x140cab230 (`sub rsp,0x18; ...`, returns 1 only if all 3 session
/// mgrs == 2). The boot/menu network-flow step calls it to decide ONLINE-attempt vs OFFLINE; a
/// non-zero return makes it attempt online login, which FAILS offline -> the connection-error
/// modal re-pops on every menu transition (the popup LOOP). Patching it to `xor eax,eax; ret`
/// (return "not ready") makes the flow take the clean OFFLINE fork and NEVER attempt online.
/// Same 3-byte stub; first byte 0x48 (verified disasm). Applied with the getter patch.
pub(crate) const ONLINE_PREDICATE_DISABLE_RVA: usize = 0xcab230;
/// MENU OFFLINE-NOTICE GATE -- the THIRD menu-open popup, root-caused 2026-06-23
/// (bd `menu-open-3rd-popup-offline-mode-notice-2026-06-23`, Ghidra RE `er-effects-rs-yvf`).
/// `Menu_IsEnableOnlineMode` (deobf 0x140e56310) is a lazy-init cached getter that DEFAULTS TRUE. The
/// TitleTopDialog ctx-init step (0x14082d0d0) computes
/// `TitleFlowContext->notReleaseFlag55 (+0x18C) = !Menu_IsEnableOnlineMode()`. With the getter TRUE and the
/// boot offline, `notReleaseFlag55 == 0` routes the title-flow offline step (0x14082fda0) into building the
/// "Starting in offline mode" `GR_System_Message` (id 401170) `CS::MessageBoxDialog` -- which BLOCKS the
/// Continue/Load/NewGame row build (the stage-3 / 0-node continue-readiness wall). Patching this getter to
/// `xor eax,eax; ret` (return false) makes the game's OWN ctx-init set `notReleaseFlag55 = 1` every time it
/// runs, so the offline step takes the clean no-popup branch and the menu rows build with ZERO MessageBoxDialog
/// builds. Race-free (re-evaluated on each ctx-init, unlike a one-shot field poke). Applied with the
/// IsOnlineMode getter patch (offline-gated -> Seamless online is unaffected). Verified prologue first byte 0x40
/// (`push rbx`; deobf disasm). Reuses `ONLINE_DISABLE_STUB` (`xor eax,eax; ret`).
pub(crate) const MENU_ONLINE_MODE_DISABLE_RVA: usize = 0xe56310;
pub(crate) const MENU_ONLINE_MODE_EXPECTED_FIRST: u8 = 0x40;
/// AUTO-ACCEPT every `CS::MessageBoxDialog` popup that appears BEFORE the character is in-world
/// (connection-error, EULA, warnings, "save data" notices, ...), so the headless autoload never
/// stops on a startup modal. We hook the dialog's finished-poll getter 0x1407b0cf0
/// (`cmp [rcx+0x25e8],2; setge al; ret`, rcx=dialog) and, for the MessageBoxDialog vtable only,
/// write the result fields (button=OK, state=decided) and return "finished" -- exactly as if OK
/// were pressed. Scoped by vtable + pre-in-world so in-game dialogs + the load flow are untouched.
/// Verified self-disasm (online-disable RE 2026-06-17 + local disasm).
#[repr(usize)]
pub(crate) enum MsgBoxRva {
    ForceStop = 0x78dfd0,
    FinishedGetter = 0x7b0cf0,
    Builder = 0x9275b0,
    OnDecide = 0x927ba0,
    DialogVtable = 0x2b03550,
}

pub(crate) const MSGBOX_FINISHED_GETTER_RVA: u32 = MsgBoxRva::FinishedGetter as u32;
pub(crate) const MSGBOX_DIALOG_VTABLE_RVA: usize = MsgBoxRva::DialogVtable as usize;

#[repr(C)]
pub(crate) struct MsgBoxDialogLayout {
    pub(crate) unknown_000: [u8; 0x3b0],
    pub(crate) closing_latch: u8,
    pub(crate) unknown_3b1: [u8; 0x180f],
    pub(crate) confirm_latch: u8,
    pub(crate) unknown_1bc1: [u8; 0xa1f],
    pub(crate) result_button: i32,
    pub(crate) unknown_25e4: [u8; 0x04],
    pub(crate) state: i32,
}

pub(crate) const MSGBOX_RESULT_BUTTON_25E0_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, result_button);
pub(crate) const MSGBOX_STATE_25E8_OFFSET: usize = core::mem::offset_of!(MsgBoxDialogLayout, state);
/// Affirmative/OK button index (the consumer treats -1 as "none yet").
pub(crate) const MSGBOX_OK_BUTTON: i32 = false as i32;
/// Dialog state >= 2 satisfies the finished-poll.
#[repr(i32)]
pub(crate) enum MsgBoxState {
    Decided = 2,
}

pub(crate) const MSGBOX_STATE_DECIDED: i32 = MsgBoxState::Decided as i32;
/// CS::SaveRetryDialog vtable (RVA). A MessageBoxDialog SUBCLASS: the wrapper 0x1407af9a0 overrides
/// the base vtable to this AFTER the builder 0x1409275b0 runs. It is the "save/load failed -- Retry?"
/// prompt the offline title flow builds (save-data/profile read error in a degraded/offline env). The
/// auto-accept must recognize it by THIS vtable -- not the base MessageBoxDialog vtable (0x2b03550) --
/// or it bails before dismissing (the vtable mismatch is why auto-accept never fired). bd
/// offline-title-modal-is-saveretrydialog + press-any-button-golden-lever-job1e8-readiness-2026-06-23.
pub(crate) const SAVE_RETRY_DIALOG_VTABLE_RVA: usize = 0x2aaabf8;
/// SaveRetryDialog fade gate the OK-handler (0x78e030) reads: it commits/closes only when
/// fade_current (+0x1278) <= fade_target (+0x2300). Writing fade_current = fade_target bits makes it
/// commit on the first frame (no fade-in animation = no visible flash) instead of ~20 frames.
pub(crate) const MSGBOX_FADE_CURRENT_1278_OFFSET: usize = 0x1278;
pub(crate) const MSGBOX_FADE_TARGET_2300_OFFSET: usize = 0x2300;
pub(crate) const MSGBOX_FINISHED_TRUE: u8 = true as u8;
pub(crate) const MSGBOX_FINISHED_FALSE: u8 = false as u8;
pub(crate) const AUTO_ACCEPT_LOG_INTERVAL: usize = 30;
/// The `MenuWindowJob` currently executing inside `system_quit_menu_window_job_run_hook` (stored at
/// its entry each call). The MessageBox builder hook -- which fires nested inside that Run when a job
/// shows a `CS::MessageBoxDialog` -- reads this to learn WHICH job is building a (Seamless-suppressed)
/// popup, so that job's next Run can be advanced past the never-shown modal.
pub(crate) static CURRENT_MENU_WINDOW_JOB_RUN_JOB: AtomicUsize = AtomicUsize::new(0);
/// A `MenuWindowJob` whose Run built a Seamless-suppressed (ERSC post-PAB) MessageBox. Its next Run
/// is forced to `MenuJobResult(Success)` so the title `FixOrderJobSequence` steps past the popup that
/// was never shown -- the same advance the ToS-skip performs. 0 = none pending.
pub(crate) static MSGBOX_STALL_JOB: AtomicUsize = AtomicUsize::new(0);
/// Original finished-poll getter trampoline (0 until the hook installs).
pub(crate) static MSGBOX_FINISHED_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static AUTO_ACCEPT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const AUTO_ACCEPT_NOT_INSTALLED: usize = 0;
pub(crate) const AUTO_ACCEPT_INSTALLED_YES: usize = 1;
pub(crate) static AUTO_ACCEPT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Set once when the local player first exists in-world; gates the auto-accept OFF so in-game
/// MessageBoxDialogs (which need real choices) are never force-accepted.
pub(crate) static IN_WORLD_REACHED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const IN_WORLD_NOT_REACHED: usize = 0;
pub(crate) const IN_WORLD_REACHED_YES: usize = 1;
/// DIAGNOSTIC: identify the REAL connection-error dialog (the inferred MessageBoxDialog vtable
/// 0x142b03550 did NOT match -- the auto-accept never fired). Hook the dialog builder
/// 0x1409275b0 to log each created dialog's vtable/class + args (the FMG message id is in an
/// arg) + caller; and log every distinct vtable that polls the finished-getter pre-world.
pub(crate) const MSGBOX_BUILDER_RVA: u32 = MsgBoxRva::Builder as u32;
pub(crate) static MSGBOX_BUILDER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MSGBOX_BUILDER_LOG: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const MSGBOX_BUILDER_LOG_MAX: usize = TraceSampleLimit::Value24 as usize;
/// Native policy/ToS surface oracle: constructor 0x1409b5970 builds the TosTitle UI object and
/// binds asset UI paths such as `TosTitle`, `TosTitle/Text`, and the ToS_win64-backed text body.
/// This is NOT a generic string-presence check; a hit means the live policy/privacy screen object
/// was constructed during runtime. Any hit is invalid product proof.
pub(crate) const POLICY_TOS_TITLE_CTOR_RVA: u32 = 0x9b5970;
pub(crate) const POLICY_TOS_TITLE_CTOR_WRAPPER_RVA: u32 = 0x9b6070;
pub(crate) const POLICY_TOS_SELECTOR_WRAPPER_RVA: u32 = 0x9b6140;
pub(crate) const POLICY_TOS_SELECTOR_CTOR_RVA: u32 = 0x9b49f0;
pub(crate) const POLICY_TOS_SELECTOR_VTABLE_RVA: usize = 0x2b27788;
pub(crate) const POLICY_TOS_TITLE_VTABLE_RVA: usize = 0x2b28100;
pub(crate) const POLICY_TOS_TITLE_TEXT_PATH_RVA: usize = 0x2b27330;
pub(crate) static POLICY_TOS_TITLE_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_TITLE_CTOR_WRAPPER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_TITLE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const POLICY_TOS_TITLE_HOOK_NOT_INSTALLED: usize = 0;
pub(crate) const POLICY_TOS_TITLE_HOOK_INSTALLED_YES: usize = 1;
pub(crate) static POLICY_TOS_TITLE_TOTAL_BUILDS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Count of TosMultiLangDialog builds our wrapper skipped (zero-input ToS-modal
/// suppression). Non-zero only when `policy_tos_suppress_enabled()` is on; the
/// suppressed build returns null, mimicking the wrapper's native allocation-failure
/// path so the unnecessary startup ToS modal is never constructed.
pub(crate) static POLICY_TOS_TITLE_SUPPRESSED_BUILDS: AtomicUsize = AtomicUsize::new(0);
/// Return value our suppressed ToS-modal wrapper hands back: 0 (null), identical to the
/// native wrapper's allocation-failure return, a path the caller already tolerates.
pub(crate) const POLICY_TOS_MODAL_SUPPRESSED_RETURN: usize = 0;
pub(crate) static POLICY_TOS_TITLE_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_ARG_RDX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_ARG_R8: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_ARG_R9: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_STACK_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const POLICY_TOS_TITLE_WRAPPER_THIS_ADJUST: usize = 0x8;
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_RECORD: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_RECORD_ID: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_STACK_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_BACKING_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_RECORD: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_SELECTOR_ARG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_STORED_SELECTOR_ARG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Native policy/status predicate 0x1409b72b0: returns true if the policy gate at 0x140e4fda0
/// is set, otherwise falls back to `[this+8]+0x29c0`. Hooked passively to explain legal/status
/// gate failures in direct/offline runs; never used to auto-accept or skip the UI.
pub(crate) const POLICY_TOS_STATUS_PREDICATE_RVA: u32 = 0x9b72b0;
pub(crate) const POLICY_TOS_FLAG_SETTER_RVA: u32 = 0x9b6b30;
pub(crate) static POLICY_TOS_STATUS_PREDICATE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_FLAG_SETTER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_STATUS_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static POLICY_TOS_STATUS_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_FLAG_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_FORCE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_BEFORE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_AFTER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static START_POLICY_TOS_TITLE_HOOK: Once = Once::new();
/// Observe-only user32 window-reconfiguration hooks (bd er-effects-rs-rzow).
pub(crate) static START_WINDOW_RECONFIG_OBSERVER: Once = Once::new();
/// Native server/login status-text formatter. Static asset/native scan (see
/// `target/autoresearch/server-semaphore-assets/server-semaphore-static-summary.json`) maps
/// `GR_System_Message_win64.fmg` status IDs 401120/401150/401160/401165 to state records at
/// 0x142acbe40. Product proof must fail if this online/login status UI appears.
pub(crate) const SERVER_STATUS_FORMATTER_RVA: u32 = 0x83ac60;
pub(crate) const SERVER_STATUS_RECORD_STATE_OFFSET: usize = 0x0;
pub(crate) const SERVER_STATUS_RECORD_TEXT_ID_OFFSET: usize = 0x10;
pub(crate) const SERVER_STATUS_CHECKING_NETWORK_TEXT_ID: usize = 401_120;
pub(crate) const SERVER_STATUS_LOGGING_IN_TEXT_ID: usize = 401_150;
pub(crate) const SERVER_STATUS_RETRIEVING_DATA_TEXT_ID: usize = 401_160;
pub(crate) const SERVER_STATUS_SAVING_DATA_TEXT_ID: usize = 401_165;
pub(crate) static SERVER_STATUS_FORMATTER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SERVER_STATUS_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SERVER_STATUS_HOOK_NOT_INSTALLED: usize = 0;
pub(crate) const SERVER_STATUS_HOOK_INSTALLED_YES: usize = 1;
pub(crate) static SERVER_STATUS_TOTAL_SEEN: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static SERVER_STATUS_LAST_STATE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static SERVER_STATUS_LAST_TEXT_ID: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static START_SERVER_STATUS_HOOK: Once = Once::new();
pub(crate) static AUTO_ACCEPT_VT_LAST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static AUTO_ACCEPT_VT_LOG: AtomicUsize = AtomicUsize::new(0);
pub(crate) const AUTO_ACCEPT_VT_LOG_MAX: usize = 24;
/// CS::SceneObjProxy ctor 0x14074a700 -- the fn the title dialog-build path runs to wrap the live
/// host MenuWindow in a transient SceneObjProxy. Disasm-verified prologue: `mov %rdx,%rbx`
/// (0x14074a720) then store `mov %rbx,0x20(%rsi)` (0x14074a735) -> proxy+0x20 = the incoming RDX =
/// the engine-VERIFIED MenuWindow (probe-6 proved the TitleTopDialog factory rdx was a std::function
/// delegate, NOT the MenuWindow). We MinHook this ctor at process attach and LATCH the validated
/// MenuWindow (arg2/rdx) on EVERY valid call (most-recent live host window wins) so the live-dialog
/// path reuses it as the Load-Game factory 0x14081ead0 rdx
/// (bd live-dialog-probe6-factory-fires-returns-dialog-rdx-not-menuwindow-2026).
pub(crate) const SCENE_OBJ_PROXY_CTOR_RVA: u32 = 0x74a700;
/// Trampoline for the SceneObjProxy-ctor latch hook (0 = unset).
pub(crate) static SCENE_OBJ_PROXY_CTOR_ORIG: AtomicUsize = AtomicUsize::new(0);
/// The host MenuWindow* latched from the SceneObjProxy ctor (incoming rdx) at title build. 0 until
/// the title builds. Updated on every VALID (vtable-checked) call. Read by
/// `locate_live_loadgame_node` (SeqCst); fail-closed when still 0.
pub(crate) static LATCHED_MENU_WINDOW: AtomicUsize = AtomicUsize::new(0);
/// One-shot install guard for the MenuWindow-latch factory hook (mirrors AUTO_ACCEPT_INSTALLED).
pub(crate) static MENU_WINDOW_LATCH_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const MENU_WINDOW_LATCH_NOT_INSTALLED: usize = 0;
pub(crate) const MENU_WINDOW_LATCH_INSTALLED_YES: usize = 1;
pub(crate) static START_MENU_WINDOW_LATCH: Once = Once::new();
/// System -> Quit Game tab hook: duplicate the native Quit Game / return-to-title
/// `AddCancelButton` call into Load Profile and Open Save Folder rows. Load Profile routes to
/// native 05_010_ProfileSelect; Open Save Folder opens the env-provided save directory. The hook is
/// always installed; slot-load activation from the injected in-world ProfileSelect is separately
/// guarded below. Address is deobf/live (dump AddCancelButton
/// 0x140920d80 -> live 0x140920c90).
pub(crate) const SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA: u32 = 0x920c90;
/// Return address immediately after the first `AddCancelButton` in the Quit Game tab builder
/// (live/deobf `FUN_140958910`). The first native row is Quit Game / return-to-title; the second
/// native row is Return to Desktop and must not be cloned for quick-load.
pub(crate) const SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA: usize = 0x958a20;
/// Return address immediately after the second native `AddCancelButton` in the Quit Game tab builder
/// (deobf `FUN_140958910`). Used to append exactly one third in-place-style row while preserving the
/// native GameEnd GFx component.
pub(crate) const SYSTEM_QUIT_SECOND_ROW_TARGET_RETURN_RVA: usize = 0x958b37;
pub(crate) const SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES: usize = 0x20;
/// Immediate byte in the Quit Game subdialog factory that selects the one-slot `GameEnd` GFX
/// component (`movb $0xe, 0x20(%rsp)` in live/deobf `FUN_14093bba0`). For the duplicate-button
/// proof, patch it to the multi-slot controls component index used by `FUN_140958d40`; the Quit
/// Game builder callback is left unchanged, so only the visible layout changes.
pub(crate) const SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA: usize = 0x93bb41;
pub(crate) const SYSTEM_QUIT_COMPONENT_INDEX_EXPECTED_GAME_END: u8 = 0x0e;
pub(crate) const SYSTEM_QUIT_COMPONENT_INDEX_REPLACEMENT_MULTI_SLOT: u8 = 0x02;
pub(crate) const SYSTEM_QUIT_COMPONENT_INDEX_PATCH_LEN: usize = 1;
/// Existing native line-help text reused as the visible label/help for the cloned quick-load row.
/// `GR_LineHelp[406000] == "Select profile to load"` in the local FMG dump.
pub(crate) const SYSTEM_QUIT_LOAD_LINEHELP_ID: u32 = 406000;
/// Live/deobf `GetGR_LineHelp(MenuString*, int)` (dump `0x140760880` -> live `0x140760790`).
pub(crate) const GET_GR_LINEHELP_ENTRY_RVA: u32 = 0x760790;
/// Live/deobf `CS::MsgRepository::GetAndFormat(MenuString*, getter, id, fmg_name, abbrev)`
/// (dump `0x1407634c0` -> live `0x1407633d0`). Hooked narrowly for System -> Quit Game
/// relabeling to Save Game without editing bundled FMGs.
pub(crate) const MSG_REPOSITORY_GET_AND_FORMAT_RVA: u32 = 0x7633d0;
/// Live/deobf `CS::MsgRepository::Format(MenuString*, wchar_t*, id, fmg_name, abbrev)`
/// (dump `0x1407639a0` -> live `0x1407638b0`). The GetAndFormat detour delegates here with
/// process-lifetime UTF-16 literals for the Save Game replacement strings.
pub(crate) const MSG_REPOSITORY_FORMAT_RVA: u32 = 0x7638b0;
/// Live/deobf `CS::MenuString::MenuString(MenuString*, wchar_t*)` (dump `0x140675990` ->
/// live `0x1406758a0`). Stores the raw UTF-16 pointer, so callers must pass process-lifetime data.
pub(crate) const MENU_STRING_FROM_WIDE_RVA: u32 = 0x6758a0;
/// FMG IDs for the two native System -> Quit Game rows. We keep the native GameEnd GFx component
/// and replace these two button slots in-place; adding rows or swapping to the multi-slot component
/// poisons the shared OptionSetting GFx list.
pub(crate) const SYSTEM_QUIT_FIRST_ROW_MENU_TEXT_ID: i32 = 110510;
pub(crate) const SYSTEM_QUIT_FIRST_ROW_LINEHELP_ID: i32 = 110500;
pub(crate) const SYSTEM_QUIT_SECOND_ROW_MENU_TEXT_ID: i32 = 110511;
pub(crate) const SYSTEM_QUIT_SECOND_ROW_LINEHELP_ID: i32 = 110501;
pub(crate) const SYSTEM_QUIT_SAVE_GAME_DIALOG_ID: i32 = 110000;
/// Native save-only routines: `SaveRequest_Profile(true)` and `RequestSave(true)`. Distinct from
/// `FUN_14067a490`, which requests save AND sets return-title teardown state.
pub(crate) const SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA: u32 = 0x67a420;
pub(crate) const SYSTEM_QUIT_REQUEST_SAVE_RVA: u32 = 0x67a520;
/// `MenuHelpLabelComponent` contains two `MenuString` objects: visible label at +0, help at +0x38.
pub(crate) const MENU_HELP_LABEL_HELP_OFFSET: usize = 0x38;
pub(crate) const MENU_HELP_LABEL_SIZE: usize = 0x70;
/// Live/deobf `MenuHelpLabelComponent::~MenuHelpLabelComponent` (dump `0x140742d90`).
pub(crate) const MENU_HELP_LABEL_DTOR_RVA: u32 = 0x742c90;
/// Quit Game / return-to-title action std::function-like vtable used by the native Quit Game builder.
pub(crate) const SYSTEM_QUIT_RETURN_TITLE_ACTION_VTABLE_RVA: usize = 0x2b12b48;
/// Vtable invoke target for the first native Quit-tab action object (`add rcx, 8; jmp native route`).
/// This is the row we relabel to Save Game; the hook suppresses the native quit behavior.
pub(crate) const SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA: u32 = 0x961640;
/// Vtable invoke target for the second native Quit-tab action object (`Return to Desktop`). Custom
/// rows are cloned from the native second AddCancelButton call, so they use this thunk, not the first
/// row thunk above. Keep this hooked separately so forwarding the real Return-to-Desktop row still
/// calls its own original trampoline.
pub(crate) const SYSTEM_QUIT_RETURN_DESKTOP_ACTION_DO_CALL_RVA: u32 = 0x9610d0;
/// `PropertyNewButtonController` activation/update method. It is the row-click layer above the
/// std::function fields; hook it for custom Quit rows because Scaleform can reach native confirmation
/// without hitting the specific action-object thunk we first captured.
pub(crate) const PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_RVA: u32 = 0x9749f0;
/// Native predicate called by `PropertyNewButtonController::Activate` before invoking the action
/// callback. It filters focus/update events from real click/confirm events; controller-level routing
/// must call this first or merely focusing a custom row opens its action.
pub(crate) const PROPERTY_NEW_BUTTON_CONTROLLER_SHOULD_INVOKE_RVA: u32 = 0x974b00;
/// Non-canonical marker copied into only the cloned quick-load action payload; the invoke hook eats it.
pub(crate) const SYSTEM_QUIT_NOOP_ACTION_SENTINEL: usize = 0x4552_5351_4e4f_4f50;
/// `PropertyEditDialog.properties.items`: 0x1260 + BasicViewItemList.items(+8).
pub(crate) const PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET: usize = 0x1268;
/// `PropertyEditDialog.properties.items.count`: 0x1260 + BasicViewItemList.items(+8) +
/// DLFixedVector<EditProperty>.count(+0x888). Pure diagnostic read only.
pub(crate) const PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET: usize = 0x1af0;
pub(crate) const EDIT_PROPERTY_SIZE: usize = 0x88;
pub(crate) const EDIT_PROPERTY_CONTROLLER_OFFSET: usize = 0x78;
/// In `PropertyNewButtonController`, first cloned action std::function stores its impl ptr at +0xa8.
pub(crate) const PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET: usize = 0xa8;
pub(crate) static SYSTEM_QUIT_DUPLICATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_NOOP_ACTION_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_RETURN_DESKTOP_ACTION_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_GET_AND_FORMAT_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_RETURN_TITLE_REQUEST_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SYSTEM_QUIT_DUPLICATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_NOOP_ACTION_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_TEXT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_CONFIRM_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SYSTEM_QUIT_DUPLICATE_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_DUPLICATE_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_NOOP_ACTION_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_NOOP_ACTION_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_RETURN_DESKTOP_ACTION_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED_YES: usize = 1;
pub(crate) const PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_NOT_INSTALLED: usize = 0;
pub(crate) const PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_SAVE_GAME_TEXT_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_SAVE_GAME_TEXT_INSTALLED_YES: usize = 1;
pub(crate) const SYSTEM_QUIT_SAVE_GAME_CONFIRM_NOT_INSTALLED: usize = 0;
pub(crate) const SYSTEM_QUIT_SAVE_GAME_CONFIRM_INSTALLED_YES: usize = 1;
pub(crate) static SYSTEM_QUIT_DUPLICATE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Native first Quit-tab row action object (label is replaced to Save Game by our text hook). Captured
/// from the row table immediately after the native first AddCancelButton call returns.
pub(crate) static SYSTEM_QUIT_NATIVE_SAVE_GAME_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// Native second Quit-tab row action object (Return to Desktop). The patched 4-slot GameEnd GFx can
/// still dispatch this native object for the lower visual buttons; the action hook disambiguates those
/// by the live dialog cursor so row 2/3 become Load Profile / Load Save Profiles instead of showing
/// the native desktop confirmation.
pub(crate) static SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_ACTION_LAST_OBJECT: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_NOOP_SELECTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_TEXT_SUBSTITUTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_CONFIRM_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_CLOSE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Recorded cloned action implementation object for the quick-load row; only this action is routed.
pub(crate) static SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// Recorded `PropertyNewButtonController` for the quick-load row. This is the authoritative click
/// dispatch identity when the GFx/native bridge bypasses the action-object thunk.
pub(crate) static SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// Recorded cloned action implementation object for the save-folder row; only this action opens the
/// env-provided save directory.
pub(crate) static SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// Recorded `PropertyNewButtonController` for the save-folder row. This is the authoritative click
/// dispatch identity when the GFx/native bridge bypasses the action-object thunk.
pub(crate) static SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_OPEN_SAVE_DIR_SUCCESS_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Legacy fallback latch for older confirmation-based Save Game routing. The product Save Game row
/// now requests save + closes menus directly and clears this latch so it never reaches the native
/// Quit Game / return-title action.
pub(crate) static SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Stable qword slot passed to the native `05_010_ProfileSelect` wrapper. The wrapper writes the
/// MenuWindowJob pointer here and captures this slot for its later ProfileLoadDialog factory call.
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT: AtomicUsize = AtomicUsize::new(0);
/// Live/deobf native `05_010_ProfileSelect` wrapper (`FUN_14081f7e0` dump -> live `0x14081f6f0`).
pub(crate) const PROFILE_SELECT_WRAPPER_RVA: u32 = 0x81f6f0;
/// Live/deobf native menu-job submit helper (`FUN_1407a9340` dump -> live `0x1407a9250`).
pub(crate) const MENU_JOB_SUBMIT_RVA: u32 = 0x7a9250;
/// Live/deobf native menu-job queue idle predicate (`FUN_1407a9320` dump -> live `0x1407a9230`).
pub(crate) const MENU_JOB_QUEUE_READY_RVA: u32 = 0x7a9230;
/// Live/deobf native `CS::MenuJob::ChainMenuJobs` (`0x1407a7ca0` dump -> live `0x1407a7bb0`).
/// ABI: `rcx=&first_job_slot, rdx=&out_job_slot, r8=&second_job_slot`; it builds a native
/// FixOrderJobSequence so the existing menu/job pump owns both jobs rather than a private manual pump.
pub(crate) const MENU_JOB_CHAIN_MENU_JOBS_RVA: u32 = 0x7a7bb0;
/// Live/deobf native ProfileSelect LoadJob builder (`FUN_140826600` dump -> live `0x140826510`).
/// ABI: `rcx=&out_job_slot, rdx=dialog+0x50/list, r8d=profile_id, r9=*(dialog+0x1cc8)`.
pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_JOB_BUILDER_RVA: u32 = 0x826510;
/// Live/deobf native Quit Game return-title chain builder (`FUN_14079d7f0` dump -> live `0x14079d700`).
pub(crate) const SYSTEM_QUIT_RETURN_TITLE_CHAIN_BUILDER_RVA: u32 = 0x79d700;
/// Live/deobf `FUN_140733ff0(list, window)`: appends a MenuWindow to a DLFixedVector-backed list.
/// Hooked as a listener to identify the ProfileSelect append/list for Back/removal restore state.
pub(crate) const MENU_WINDOW_LIST_PUSH_RVA: u32 = 0x733ef0;
/// Live/deobf `FUN_140747980(MenuWindow*, SceneObjProxy*)`: constructs a root SceneObjProxy scratch
/// from `MenuWindow+0x188`. Dump `0x140747a80` -> deobf `0x140747980`.
pub(crate) const MENU_WINDOW_ROOT_PROXY_CTOR_RVA: u32 = 0x747980;
/// Live/deobf `CSScaleformValue`/SceneObjProxy scratch destructor used by native MenuWindow fade helpers.
pub(crate) const MENU_WINDOW_ROOT_PROXY_SCRATCH_DTOR_RVA: u32 = 0xd7f850;
pub(crate) const MENU_WINDOW_ROOT_PROXY_SCRATCH_SIZE: usize = 0x80;
/// SCALEFORM MENU-HANDLER LIFECYCLE GUARD (er-effects-rs crash, repeated-switch ProfileSelect UAF).
/// The crash is the inner destructor `FUN_1411a8920` (deobf 0x1411a8900) walking a garbage intrusive
/// list of a DOUBLE-FREED 0x58-byte Scaleform handler (vtable 0x142cc22c8), embedded at +0x40 of a
/// 0x98 container cached at owner+0x28. ctor `FUN_1411a8890` (deobf 0x1411a8870). We hook both: track
/// every live object (ctor inserts, normal dtor removes); a dtor of an address NOT live is the
/// double-free -> log it + SKIP the original inner destructor so it can't dereference the freed list.
/// A true double-inner-destruct of an already-freed object is safe to skip (it was already torn down).
pub(crate) const SCALEFORM_HANDLER_CTOR_RVA: usize = 0x11a8870;
pub(crate) const SCALEFORM_HANDLER_DTOR_RVA: usize = 0x11a8900;
pub(crate) static SCALEFORM_HANDLER_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SCALEFORM_HANDLER_DTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SCALEFORM_HANDLER_TRACE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Live handler-object addresses (ctor'd, not yet dtor'd). Linear-scanned Vec -- volume is a few
/// dozen menu handlers, not a hot per-frame path. Capped so a genuine leak can't grow it unbounded.
pub(crate) static SCALEFORM_HANDLER_LIVE: std::sync::Mutex<Vec<usize>> =
    std::sync::Mutex::new(Vec::new());
pub(crate) const SCALEFORM_HANDLER_LIVE_CAP: usize = 8192;
/// Oracles: total ctors/dtors seen, double-frees detected+skipped, and the last skipped object +
/// its container/parent for correlation with the switch timeline.
pub(crate) static SCALEFORM_HANDLER_CTORS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SCALEFORM_HANDLER_DTORS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SCALEFORM_HANDLER_DOUBLE_FREES: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SCALEFORM_HANDLER_LAST_DOUBLE_FREE_OBJ: AtomicUsize = AtomicUsize::new(0);

// === Game-Options pane VISIBILITY oracle (READ-ONLY, `oracle_optionsetting_pane_*`) ===============
// Detects the "blank Game Options pane" bug on OptionSetting menu re-entry: the tab strip + footer
// render but the option-row pane display objects are not VISIBLE (the row list draws black). The
// `MenuWindowJob::Run` hook for `02_040_OptionSetting`/`_Trial` resolves the OptionSetting root
// SceneObjProxy's `WindowList` container + each option pane by name (the game's own
// `assignComponentWithName`), then reads each child DisplayObject's `DisplayInfo.Visible` byte via
// the GFx `GetDisplayInfo` vcall -- all reads, no game state mutated. Offsets verified against the
// game binary (Ghidra CSScaleformValue struct + MenuWindow layout); mirrors the 7e7 resolve/guard/
// release pattern in `push_stats_text_on_row`.
/// MenuWindow -> embedded root SceneObjProxy (the `assignComponentWithName` parent). Same +0x188
/// slot the native MenuWindow fade helper reads (`MENU_WINDOW_ROOT_PROXY_CTOR_RVA` builds a scratch
/// proxy from `MenuWindow+0x188`).
pub(crate) const OPTION_SETTING_ROOT_PROXY_OFFSET: usize = 0x188;
/// Within the embedded `CSScaleformValue` (out proxy + `SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET`):
/// objectInterface ptr, dataType i32, GFx value handle ptr (Ghidra CSScaleformValue struct).
pub(crate) const CSSCALEFORMVALUE_OBJECT_INTERFACE_OFFSET: usize = 0x18;
pub(crate) const CSSCALEFORMVALUE_DATATYPE_OFFSET: usize = 0x20;
pub(crate) const CSSCALEFORMVALUE_HANDLE_OFFSET: usize = 0x28;
/// The child is a live DisplayObject iff `(dataType & MASK) == VALUE`.
pub(crate) const CSSCALEFORMVALUE_DISPLAY_TYPE_MASK: i32 = 0x8f;
pub(crate) const CSSCALEFORMVALUE_DISPLAY_TYPE_VALUE: i32 = 10;
/// `GetDisplayInfo` is objectInterface vtable slot +0xd8: `fn(objectInterface, valueHandle, bufPtr)`.
pub(crate) const CSSCALEFORMVALUE_GET_DISPLAY_INFO_VTABLE_SLOT: usize = 0xd8;
/// DisplayInfo out buffer (>= 0xE0, zero-initialized). After the vcall the `Visible` byte is at +0xd6
/// (nonzero == visible); the VarsSet flags ushort sits at +0xd4 (reference only).
pub(crate) const OPTIONSETTING_DISPLAY_INFO_BYTES: usize = 0xE0;
pub(crate) const OPTIONSETTING_DISPLAY_INFO_VISIBLE_OFFSET: usize = 0xd6;
/// OptionSetting composite sub-dialog job slot (`MenuWindow+0x1768`, job ptr at +0xb8): nonzero when
/// the composite sub-dialog job is bound (a corroborating signal, read-only).
pub(crate) const OPTIONSETTING_COMPOSITE_SUBDIALOG_JOB_OFFSET: usize = 0x1768 + 0xb8;
/// Reject obviously-invalid OptionSetting window pointers before any dereference.
pub(crate) const OPTIONSETTING_WINDOW_MIN_PTR: usize = 0x10000;
/// Cap on the per-sample debug lines (first N), like other bounded diagnostics.
pub(crate) const OPTIONSETTING_PANE_SAMPLE_LOG_CAP: usize = 64;
/// NUL-terminated container name (resolved separately -- the direct blank-pane signature source).
pub(crate) const OPTIONSETTING_WINDOWLIST_NAME: &str = "WindowList\0";
/// NUL-terminated option-pane child names; the bit index (pane order) is used in the pane masks.
pub(crate) const OPTIONSETTING_PANE_NAMES: [&str; 8] = [
    "CameraSetting\0",
    "GameEnd\0",
    "BrightnessSetting\0",
    "ControllSetting\0",
    "NetworkSetting\0",
    "AudioSetting\0",
    "EnvironmentSetting\0",
    "PadSetting\0",
];
/// Total pane-visibility samples taken (one per OptionSetting `MenuWindowJob::Run` with a live owner).
pub(crate) static OPTIONSETTING_PANE_SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Last sample: whether the `WindowList` container resolved (0/1).
pub(crate) static OPTIONSETTING_PANE_LAST_WINDOWLIST_RESOLVED: AtomicUsize = AtomicUsize::new(0);
/// Last sample: whether the `WindowList` container's DisplayInfo.Visible was set (0/1).
pub(crate) static OPTIONSETTING_PANE_LAST_WINDOWLIST_VISIBLE: AtomicUsize = AtomicUsize::new(0);
/// Last sample: bitmask (bit N = pane N of `OPTIONSETTING_PANE_NAMES`) of panes that resolved.
pub(crate) static OPTIONSETTING_PANE_LAST_RESOLVED_MASK: AtomicUsize = AtomicUsize::new(0);
/// Last sample: bitmask of panes whose DisplayInfo.Visible was set.
pub(crate) static OPTIONSETTING_PANE_LAST_VISIBLE_MASK: AtomicUsize = AtomicUsize::new(0);
/// Last sample: the `WindowList` child's raw dataType (for gate diagnosis).
pub(crate) static OPTIONSETTING_PANE_LAST_DATATYPE: AtomicUsize = AtomicUsize::new(0);
/// Count of vcalls skipped fail-closed because objectInterface/vtable/getfn were not game-image-live.
pub(crate) static OPTIONSETTING_PANE_GUARD_SKIPS: AtomicUsize = AtomicUsize::new(0);
/// Last sample: whether the composite sub-dialog job slot was bound (0/1).
pub(crate) static OPTIONSETTING_PANE_COMPOSITE_BOUND: AtomicUsize = AtomicUsize::new(0);
/// Count of samples where the blank-pane signature fired (`WindowList` resolved but NOT visible).
pub(crate) static OPTIONSETTING_PANE_BLANK_DETECTED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The REAL row-pane signal: the current tab dialog (`*(composite+0xb8)`) and the DisplayInfo.Visible of
/// its embedded pane proxy at `dialog+0x1200` -- the object the game's own tab-select SetVisibles. The 8
/// named WindowList children are always Visible=0 and are NOT the signal (they made blank_detected fire
/// before the user could even reproduce). `actively_shown` = CSMenuMan flag bit 0x4 (drawn this frame).
pub(crate) static OPTIONSETTING_CURRENT_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_CURRENT_PANE_VISIBLE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_CURRENT_PANE_DATATYPE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_ACTIVELY_SHOWN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_LAST_FLAG: AtomicUsize = AtomicUsize::new(0);
/// Latch: the current pane was seen VISIBLE at least once (a healthy Game Options open). The teardown
/// oracle `..._REAL_BLANK_DETECTED_COUNT` only fires AFTER this latch, so a boot/preload state (pane
/// never yet shown) can never be mistaken for the bug -- the bug is healthy(visible)->blank(hidden).
pub(crate) static OPTIONSETTING_CURRENT_PANE_EVER_VISIBLE: AtomicUsize = AtomicUsize::new(0);
/// Run-stopping oracle: healthy pane was seen, THEN the actively-shown current pane went hidden.
pub(crate) static OPTIONSETTING_REAL_BLANK_DETECTED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The selected tab index the user is on (`*(*(window+0x1870+0x10)+0xd4)`) at the last sample, and the
/// cache slot the current pane dialog matches -- to identify WHICH tab is blank (e.g. the Quit/Exit tab
/// where our injected Load-Profile rows live vs the Game tab).
pub(crate) static OPTIONSETTING_CURRENT_TAB: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static OPTIONSETTING_CURRENT_TAB_AT_BLANK: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_REAPPLY_COUNT: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_TAB: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_OLD_CURRENT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_SELECTED: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_LAST_SELECTED: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Count of times the fix forced the actively-shown current tab's pane back visible (via SetVisible on
/// dialog+0x1200 -- the same proxy/call the game's own tab-select uses). Nonzero = the blank was caught
/// and corrected; the pane draws again.
pub(crate) static OPTIONSETTING_PANE_FIX_APPLIED: AtomicUsize = AtomicUsize::new(0);
/// Active OptionSetting row-table sampler: read-only row/action classification for the currently
/// visible tab dialog. This is the product-proof oracle for the Game Options/Quit contamination class:
/// tab 0 must not contain cloned quick-load/open-profile actions; Quit tab should contain them once the
/// feature is injected.
pub(crate) static OPTIONSETTING_ACTIVE_ROW_SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_ACTIVE_ROW_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_ACTIVE_ROW_TAB: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static OPTIONSETTING_ACTIVE_ROW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_ACTIVE_ROW_CLONED_MASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_ACTIVE_ROW_NATIVE_SAVE_MASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_ACTIVE_ROW_ACTION_HASH: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_ACTIVE_ROW_LABEL_HASH: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_ACTIVE_ROW_QUIT_LABEL_MASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_GAME_OPTIONS_CLONED_ROW_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OPTIONSETTING_GAME_OPTIONS_QUIT_LABEL_HITS: AtomicUsize = AtomicUsize::new(0);
/// window -> SettingTabControl (+0x1870), -> tab view (+0x10), -> selected index (view+0xd4).
pub(crate) const OPTIONSETTING_TAB_CONTROL_OFFSET: usize = 0x1870;
pub(crate) const OPTIONSETTING_TAB_VIEW_OFFSET: usize = 0x10;
pub(crate) const OPTIONSETTING_TAB_VIEW_SELECTED_INDEX_OFFSET: usize = 0xd4;
/// Composite current-dialog embedded pane proxy offset (`dialog+0x1200`; FUN_14093b850 SetVisibles it).
pub(crate) const OPTIONSETTING_DIALOG_PANE_PROXY_OFFSET: usize = 0x1200;
/// Deobf/runtime RVA for the native OptionSetting tab-select helper body. It sets composite+0xb8,
/// copies pane state old->new, refreshes the selected row, then toggles all cached panes. Call only
/// after first repairing composite+0xb8 to the target pane so its copy step is self-copy, not stale
/// Quit->Game state copy.
pub(crate) const OPTIONSETTING_DIALOG_REFRESH_SELECTED_ROW_RVA: u32 = 0x0093b760;
/// CSMenuMan flag bit meaning "menu actively shown/drawn this frame" (per-frame updater sets `|=0x4`).
pub(crate) const OPTIONSETTING_FLAG_ACTIVELY_SHOWN_BIT: u8 = 0x4;

/// GX COMMAND-QUEUE PRODUCER TELEMETRY (switch-#4 overflow, run autostep10c-directarm 2026-07-03).
/// `reserve_command_queue_slot` (deobf entry 0x141aeae60; shift-verified against dump 0x141aeae80)
/// appends a command-list slot to a fixed array: base at queue+0x28, count at +0x30, capacity at
/// +0x34 (fixed 192). When count >= capacity the append branch is skipped and the engine writes the
/// slot through a NULL pointer -- the repeated-switch crash at rva 0x1aeaf05. Switches #1-#3 survive
/// and #4 overflows, so some producer's per-frame submissions GROW per switch. This hook is
/// telemetry-ONLY (always forwards -- the 5ae3965 drop-on-overflow guard corrupted rendering and was
/// removed in c2794d9): it tracks occupancy high-water (cumulative + per-switch) and a caller
/// histogram so the run that overflows NAMES the accumulating producer instead of just crashing.
pub(crate) const GX_RESERVE_CMD_QUEUE_SLOT_RVA: usize = 0x1aeae60;
/// Queue-struct field offsets (from the reserve_command_queue_slot decompile).
pub(crate) const GX_CMD_QUEUE_COUNT_OFFSET: usize = 0x30;
pub(crate) const GX_CMD_QUEUE_CAP_OFFSET: usize = 0x34;
pub(crate) static GX_RESERVE_CMD_QUEUE_SLOT_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static GX_RESERVE_CMD_QUEUE_SLOT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Cumulative occupancy high-water, per-switch high-water (reset by `sq_repro_begin_switch`), the
/// observed capacity, and total reserve calls.
pub(crate) static GX_CMD_QUEUE_MAX_FILL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static GX_CMD_QUEUE_SWITCH_MAX_FILL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static GX_CMD_QUEUE_CAP_SEEN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static GX_CMD_QUEUE_SUBMITS: AtomicUsize = AtomicUsize::new(0);
/// Producer histogram: open-addressed key -> count. Key = first game-.text return address (as RVA)
/// above the reserve/add_command_list wrapper band, with `GX_CMD_QUEUE_SELF_TAG` ORed in when any
/// stack frame lies inside our own DLL (attributes submissions our pipeline caused vs pure-native).
pub(crate) const GX_CMD_QUEUE_HIST_SLOTS: usize = 32;
pub(crate) const GX_CMD_QUEUE_SELF_TAG: usize = 1 << 63;
/// Deobf RVA band holding reserve_command_queue_slot and its 4 thin enqueue wrappers (dump
/// 0x141aea930..0x141aeab60, shift +0x20); return addresses inside it are transport, not producers.
pub(crate) const GX_CMD_QUEUE_WRAPPER_RVA_MIN: usize = 0x1aea900;
pub(crate) const GX_CMD_QUEUE_WRAPPER_RVA_MAX: usize = 0x1aeaf60;
pub(crate) static GX_CMD_QUEUE_HIST_KEYS: [AtomicUsize; GX_CMD_QUEUE_HIST_SLOTS] =
    [const { AtomicUsize::new(0) }; GX_CMD_QUEUE_HIST_SLOTS];
pub(crate) static GX_CMD_QUEUE_HIST_COUNTS: [AtomicUsize; GX_CMD_QUEUE_HIST_SLOTS] =
    [const { AtomicUsize::new(0) }; GX_CMD_QUEUE_HIST_SLOTS];
pub(crate) static GX_CMD_QUEUE_HIST_DROPPED: AtomicUsize = AtomicUsize::new(0);
/// Near-full evidence: hits with count >= cap - margin, and a log throttle so the dump lands BEFORE
/// the crash frame without spamming (one line per 64 near-full reserves).
pub(crate) const GX_CMD_QUEUE_NEARFULL_MARGIN: usize = 24;
pub(crate) const GX_CMD_QUEUE_NEARFULL_LOG_EVERY: usize = 64;
pub(crate) static GX_CMD_QUEUE_NEARFULL_HITS: AtomicUsize = AtomicUsize::new(0);
/// BUCKET-TABLE instrument (names the RETAINER class the producer histogram cannot: run 10d proved
/// the drain pump FUN_141b3bdc0 dominates reserves by RESUBMITTING its context list each frame, so
/// the leak is list membership). The pump's context (its param_1; latched by a thin entry hook at
/// deobf 0x1b3bda0, dump 0x141b3bdc0, shift-verified) holds a 109-bucket table of per-frame queue
/// slot ranges: begin i32 at ctx+0x30+idx*0x18, end i32 at ctx+0x34+idx*0x18 (from the pump's
/// bucket-locate loop, bound 0x6d). Nonzero widths per bucket, diffed across switches, name which
/// bucket's submissions grow toward the 192 cap.
pub(crate) const GX_CMD_PUMP_RVA: usize = 0x1b3bda0;
pub(crate) static GX_CMD_PUMP_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static GX_CMD_PUMP_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static GX_CMD_PUMP_CTX: AtomicUsize = AtomicUsize::new(0);
pub(crate) const GX_CMD_QUEUE_BUCKET_COUNT: usize = 0x6d;
pub(crate) const GX_CMD_QUEUE_BUCKET_BEGIN_OFFSET: usize = 0x30;
pub(crate) const GX_CMD_QUEUE_BUCKET_END_OFFSET: usize = 0x34;
pub(crate) const GX_CMD_QUEUE_BUCKET_STRIDE: usize = 0x18;
/// A bucket width above the slot capacity is a torn/stale read (observed in run 10e's final
/// telemetry read racing the crashing render thread) -- skip it rather than report garbage.
pub(crate) const GX_CMD_QUEUE_BUCKET_WIDTH_SANE_MAX: i32 = 192;
/// PEAK-frame bucket snapshots: run 10e proved calm-frame (switch-boundary) bucket tables stay flat
/// (~30 total) while the per-switch occupancy PEAK grows 93 -> 121 -> 161 -> 183 -- the growth only
/// materializes in the teardown/reload frames, and NEAR-FULL (cap-24) fires too late to see
/// switches #1-#3. Log the bucket table whenever the switch high-water rises to >= MIN and has
/// grown by >= STEP since the last snapshot, so every switch's peak-frame composition is diffable.
pub(crate) const GX_CMD_QUEUE_PEAK_LOG_MIN: usize = 80;
pub(crate) const GX_CMD_QUEUE_PEAK_LOG_STEP: usize = 8;
pub(crate) static GX_CMD_QUEUE_PEAK_LAST_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// COMMAND-BYTE ARENA fill (user-reported render corruption during switch #3's return-title window,
/// 2026-07-03): `reserve_command_queue_slot` allocates command BYTES from a bump arena at
/// queue+0x40 (FUN_141c48e80: alloc counter at arena+0x14, limit at +0x20, cursor at +0x28;
/// remaining = limit - align_up(cursor_lo); on remaining < request it takes a refill/wrap path
/// FUN_141c48f50). If that wraps while earlier commands are unconsumed, live command bytes are
/// overwritten -> garbled draws WITHOUT a crash -- the sub-critical sibling of the 0x1aeaf05
/// slot-table overflow. Track remaining low-water (cumulative + per-switch) to correlate.
pub(crate) const GX_CMD_QUEUE_ARENA_OFFSET: usize = 0x40;
pub(crate) const GX_CMD_ARENA_ALLOC_COUNT_OFFSET: usize = 0x14;
pub(crate) const GX_CMD_ARENA_LIMIT_OFFSET: usize = 0x20;
pub(crate) const GX_CMD_ARENA_CURSOR_OFFSET: usize = 0x28;
/// Low-water sentinel: usize::MAX until the first sample lands.
pub(crate) static GX_CMD_ARENA_MIN_REMAINING: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static GX_CMD_ARENA_SWITCH_MIN_REMAINING: AtomicUsize = AtomicUsize::new(usize::MAX);
/// CSDelayDeleteMan PENDING-COUNT read (repeated-switch GX overflow root-cause probe, 2026-07-03).
/// The profile-renderer teardown (`FUN_1409b2f00`) does NOT destroy the 10 old CSMenuProfModelRend
/// per switch -- it hands each to CSDelayDeleteMan (`FUN_140e77540`) and nulls the table slot. The
/// pre-delete prep (`FUN_140bb9930`) only sets the object's +0x756 "marked" byte; it does NOT
/// unregister the renderer's ResMan draw task, so a marked-but-unfreed renderer keeps submitting to
/// the 192-slot GX command queue every frame. If the delay-delete pump does not drain them during
/// our in-world return-title/reload, they pile up -> queue climbs ~+23/switch -> null-slot crash
/// (0x1aeaf05) at switch #4-5 (A/B run 10g). CSDelayDeleteMan is a singleton whose pointer lives at
/// dump global 0x1445896a8; its enqueue (`FUN_140e77f30`) increments a pending count at
/// manager+0x40 (high-water at +0x44). Reading manager+0x40 per switch tests the pileup directly:
/// climbing +~10/switch confirms the pump is not draining our enqueued renderers. Pure guarded read
/// (validate the pointer + a sane count); RVA ground-truthed in the DEOBF binary (teardown 0x9b2db0
/// disasm: `mov 0x3bd68d1(%rip),%rcx # 0x1445896a8` -> RVA 0x1445896a8 - 0x140000000 = 0x45896a8),
/// same VA as the dump. The runtime read is self-validating so a bad RVA logs -1, not a crash.
pub(crate) const DELAY_DELETE_MAN_SINGLETON_PTR_RVA: usize = 0x45896a8;
pub(crate) const DELAY_DELETE_MAN_PENDING_COUNT_OFFSET: usize = 0x40;
pub(crate) const DELAY_DELETE_MAN_PENDING_HIGHWATER_OFFSET: usize = 0x44;
/// Sane upper bound for the pending count; a larger read means the singleton RVA/layout is wrong.
pub(crate) const DELAY_DELETE_MAN_PENDING_SANE_MAX: usize = 100_000;
/// CSDelayDeleteMan ENQUEUE `FUN_140e77540` (dump) -> deobf 0x140e77490, ground-truthed from the
/// deobf profile-renderer teardown (0x9b2db0): it calls this at 0x9b2e0d as `call 0x140e77490` with
/// rcx=manager (the singleton above), rdx=object. This is the safe delayed-destruction path the game
/// uses for the OTHER 9 renderers every teardown -- marks the object's +0x756 byte, enqueues it, and
/// the delete pump frees it when the GPU is done. We call it to destroy the previously-spared
/// portrait renderer (see `PROFILE_SPARE_ORPHAN`) instead of leaking it.
pub(crate) const DELAY_DELETE_ENQUEUE_RVA: usize = 0xe77490;
/// The previously-spared portrait renderer awaiting safe destruction. The teardown-spare excludes
/// one CSMenuProfModelRend from the native delete each load (nulls its table slot) to render the
/// now-loading portrait; the load-complete reset then dropped the pointer WITHOUT freeing it, so one
/// live renderer -- still running its ResMan offscreen draw task -- leaked per System->Quit->Load
/// switch, each filling the 192-slot GX command queue every frame until it overflowed (0x1aeaf05,
/// ~switch #4). The reset now MOVES the pointer here (render thread, a plain store); the game-thread
/// teardown-spare hook delete-enqueues it via CSDelayDeleteMan at the next teardown (thread-correct,
/// same thread the native teardown runs on).
pub(crate) static PROFILE_SPARE_ORPHAN: AtomicUsize = AtomicUsize::new(0);
/// Count of leaked spared renderers reclaimed via the native delete path (repeated-switch GX fix).
pub(crate) static PROFILE_SPARE_ORPHANS_DELETED: AtomicUsize = AtomicUsize::new(0);
