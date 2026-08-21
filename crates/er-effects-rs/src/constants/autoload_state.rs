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
pub(crate) use er_title_flow::MEMBERFUNCJOB_VTABLE_RVA;
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
pub(crate) use er_title_flow::GAME_MAN_SAVE_STATE_IDLE;
pub(crate) const GAME_MAN_SAVE_STATE_OPENING: i32 = 1;
pub(crate) const GAME_MAN_SAVE_STATE_READING: i32 = 2;
pub(crate) use er_title_flow::FULLREAD_B80_RESIDENT;
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
pub(crate) use er_title_flow::FULLREAD_PHASE_SUBMIT;
pub(crate) const FULLREAD_PHASE_DRAIN: usize = 1;
pub(crate) const FULLREAD_PHASE_DESER: usize = 2;
pub(crate) const FULLREAD_PHASE_DONE: usize = 4;
pub(crate) use er_title_flow::FULLREAD_PHASE;
pub(crate) use er_telemetry::counters::FULLREAD_DRAIN_WAITS;
/// Terminal non-commit disarm counters for the full-read chain (bd er-effects-rs-ns4n). SUBMIT arms
/// the native slot-request register (GameMan+0xb78, `requested_save_slot_load_index`); the in-game
/// save manager services any >=0 request on the first frames after world arrival, running a second
/// full deserialize into the live world (CSGaitemImp free-queue exhaustion, AV at live 0x67141a).
/// Every DONE exit that does not hand off to the native confirm chain must clear the register; these
/// count the clears and record the slot value the last clear removed (u32-packed i32; !0 == none).
pub(crate) use er_telemetry::counters::FULLREAD_REQ_DISARM_COUNT;
pub(crate) use er_telemetry::counters::FULLREAD_REQ_DISARM_LAST_PREV_SLOT;
/// LATCHED peak-load semaphore (bd er-effects-rs-ns4n follow-up). The live `oracle_char_*` fields read
/// PlayerGameData directly, so a quit-to-title tears the character down and a final telemetry snapshot
/// reads them empty even on a fully-successful run -- the load proof lived only in the mid-run
/// `LOAD-CORRECTNESS` log line. These latch the highest-level REAL character ever confirmed in-world
/// this run (set once by `dump_load_correctness` when pgd is present with level>=1 and a non-empty
/// name), so `oracle_load_correctness_seen > 0` proves a real char reached the world regardless of a
/// later quit. Process-lifetime (never reset within a session): it attests "a real character loaded at
/// some point this run", which a quit or a later System->Quit switch cannot falsify.
pub(crate) use er_telemetry::counters::LOADED_PEAK_SEEN_COUNT;
pub(crate) use er_telemetry::counters::LOADED_PEAK_LEVEL;
pub(crate) use er_telemetry::counters::LOADED_PEAK_C30;
pub(crate) use er_telemetry::counters::LOADED_PEAK_NAME_LEN;
pub(crate) static LOADED_PEAK_NAME: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());


pub(crate) use er_title_flow::GAME_MAN_FLAG_B73_PROBE_OFFSET;
pub(crate) use er_title_flow::GAME_MAN_FLAG_B75_PROBE_OFFSET;
pub(crate) use er_title_flow::GAME_MAN_REQUESTED_SLOT_B78_OFFSET;
pub(crate) use er_title_flow::GAME_MAN_FLAG_BC4_OFFSET;
pub(crate) use er_title_flow::IODEV_GLOBAL_RVA;
pub(crate) use er_title_flow::IODEV_INFLIGHT_10_OFFSET;
pub(crate) use er_title_flow::IODEV_REQHANDLE_18_OFFSET;
pub(crate) use er_title_flow::IODEV_REQHANDLE_20_OFFSET;
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
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
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
/// CORRECTION (2026-08-01): this is `SaveLoad2::SLSystemImpl*`, NOT an FD4 IO worker
/// manager -- its lazy initializer `FUN_14240dee0` opens with
/// `*param_1 = SaveLoad2::SLSystemImpl::vftable`. The name is kept for its call sites.
///
/// UNRESOLVED, and it matters: the doc above reads `+0x19` as "the worker no-accept/
/// shutdown byte", while `experiments/own_stepper/bootstrap_drive.rs` reads the SAME field
/// on the SAME object as "sysimpl built+ready (`sysimpl+0x19 != 0`)". Those are opposite
/// polarities. Tracked in bd; do not build new logic on either reading until it is settled.
pub(crate) const FD4_IO_WORKER_MGR_RVA: usize = RuntimeGlobalRva::SaveLoad2SlSystemImpl as usize;
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
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const INPUTMGR_PENDING_13C_OFFSET: usize = 0x13c;
pub(crate) use er_title_flow::TITLE_ACCEPT_LATCH_RVA;
pub(crate) use er_title_flow::MOVIE_SKIP_FLAG_CLEAR;
/// Render-thread liveness probe logging cadence (in render frames).
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
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
// Foreground-force constants REMOVED (user directive 2026-07-16): the product must not patch
// CS::CSWindowImp::IsGameInForeground (it made the game grab the OS cursor on world-entry). See
// bootstrap.rs / profile_select_flow.rs.
pub(crate) use er_title_flow::MsgBoxRva;

/// `FUN_1407b0cf0` -- vtable slot 14 of every MessageBoxDialog-family vtable. Whole body is
/// `return 1 < *(i32*)(this+0x25e8)`, i.e. "this box has MORE THAN ONE button" (a Yes/No
/// confirm rather than a single-OK notice). It is NOT a finished/decided poll: `+0x25e8` is
/// the BUTTON COUNT the ctor writes at construction (see `MSGBOX_BUTTON_COUNT_25E8_OFFSET`).
/// The old name (`MSGBOX_FINISHED_GETTER_RVA`) was wrong and is what made the save-flow poll
/// resolve a freshly built two-button confirm on its first frame (2026-07-28).
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const MSGBOX_MULTI_CHOICE_GETTER_RVA: u32 = MsgBoxRva::MultiChoiceGetter as u32;
pub(crate) use er_title_flow::MSGBOX_DIALOG_VTABLE_RVA;
/// `CS::MessageBoxDialog::Update` (`FUN_140927d30`), vtable slot 2. STRUCTURAL IDENTITY: all
/// five MessageBoxDialog-family vtables in `eldenring-deobf.bin` carry this exact function in
/// slot 2 -- base `CS::MessageBoxDialog` (rva 0x2b03550), `CS::SaveRetryDialog` (0x2aaabf8),
/// and the subclasses at 0x2ae5ae0 / 0x2b06780 / 0x2b220d0 (byte-read from the deobf image
/// 2026-07-28). Comparing `vtable[2]` therefore recognises EVERY legitimate subclass -- and a
/// wrapper that swaps the vtable after the builder runs -- while still rejecting a freed or
/// reused object, which a single hard-coded vtable equality cannot do.
pub(crate) const MSGBOX_DIALOG_UPDATE_RVA: usize = MsgBoxRva::Update as usize;
/// Index of the `Update` entry in a MessageBoxDialog vtable (slot 2 == byte offset 0x10).
pub(crate) const MSGBOX_DIALOG_VTABLE_UPDATE_SLOT: usize = 2;

pub(crate) use er_title_flow::MsgBoxDialogLayout;

/// `dialog+0x25e0` -- the DEFAULT/INITIAL CURSOR INDEX, written once by the ctor
/// (`FUN_1409275b0`: `*(u32*)(param_1+0x4bc) = param_5`, and `0x4bc*8 == 0x25e0`) from the
/// MenuJob config the builder's `finalize` filled in. `OnDecide` (`FUN_140927ba0`) reads it
/// and either moves the list cursor there (`FUN_140738d40(dialog+0xa38, idx, 0)`) or, when it
/// is -1, takes the cancel arm. It is NOT the button the user chose: it never changes after
/// construction, so for a `[Yes, No]` + `default_last` confirm it reads 1 ("No") from the very
/// first frame the box exists.
pub(crate) const MSGBOX_DEFAULT_CURSOR_25E0_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, default_cursor_index);
/// `dialog+0x25e8` -- the BUTTON COUNT, written by the ctor as
/// `(*(i64*)(cfg+0x38) - *(i64*)(cfg+0x30)) / 0x210` (the size of the button-descriptor
/// vector). A Yes/No confirm therefore reads 2 here at construction; a single-OK notice reads
/// 1. Never a state machine.
pub(crate) const MSGBOX_BUTTON_COUNT_25E8_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, button_count);
/// First button index -- what the deprecated startup auto-accept pokes into the default-cursor
/// field so `OnDecide` dispatches button 0 instead of taking the cancel arm.
pub(crate) const MSGBOX_FIRST_BUTTON_INDEX: i32 = false as i32;
/// The button count that makes `MSGBOX_MULTI_CHOICE_GETTER_RVA` (`1 < count`) return true.
/// The deprecated auto-accept writes this over the real count so the title flow's modal poll
/// treats the box as resolved -- a deliberate corruption of a real field, kept only because
/// that historical path depends on it.
pub(crate) const MSGBOX_BUTTON_COUNT_MULTI_CHOICE: i32 = 2;
/// `dialog+0x1e8` -- the dialog's own `MenuJobResult` (state i32, subcode i32 at +0x1ec).
/// THIS is where a pressed button's answer lands. RE (1.16.2, 2026-07-28):
///   * `add_yes` (`FUN_1407b1c70`) attaches `MenuJobResult::SetResult(Success=2, 0)` to its
///     button; `add_no` (`FUN_1407b1900`) attaches `SetResult(Failed=3, 0)`.
///   * pressing a button runs the OK handler `FUN_14078e030` -> `FUN_14078ef20`, which reads
///     that button's `MenuJobResult` out of the 0x210-byte command struct at `+0x180` and
///     hands it to the lambda `FUN_14078ee20`, whose whole body is:
///     `if (*(u8*)(dialog+0x127c)) dialog->vtable[0x60](dialog, result);`
///     `else                       *(MenuJobResult*)(dialog+0x1e8) = result;`
///     -- i.e. either EMIT the result (which sets the `+0x3b0` latch) or store it here.
///   * the dialog's own `Update` (`FUN_140927d30`) reads `+0x1e8` and only auto-cancels
///     (`FUN_1407ac890` -> emit `Failed`) when it is still non-terminal.
pub(crate) const MSGBOX_JOB_RESULT_STATE_1E8_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, job_result_state);
pub(crate) const MSGBOX_JOB_RESULT_SUBCODE_1EC_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, job_result_subcode);
/// `MenuJobResult` state values, read straight out of the callers' immediates:
/// `MenuJobResult::SetResult` (0x1407a91e0) is `[rcx]=edx; [rcx+4]=r8d`, `add_yes` passes
/// `edx=2`, `add_no` passes `edx=3`, and `ShouldContinue` (0x1407a9200) is `cmp [rcx],1;
/// seta al` -- so state <= 1 means "no answer yet" and anything above is terminal.
pub(crate) const MENU_JOB_RESULT_STATE_CONTINUE_MAX: i32 = 1;
pub(crate) const MENU_JOB_RESULT_STATE_SUCCESS: i32 = 2;
pub(crate) const MENU_JOB_RESULT_STATE_FAILED: i32 = 3;
/// "No source has produced a state" sentinel for the save-flow poll. Deliberately inside the
/// `Continue` band so it can never be mistaken for a terminal answer.
pub(crate) const MENU_JOB_RESULT_STATE_NONE: i32 = 0;
/// `CS::MenuJob::EmitResult` -- vtable slot 12 (`+0x60`) of every MessageBoxDialog-family
/// vtable, `void f(rcx=this, rdx=MenuJobResult by value, r8, r9)`. Guards on the `+0x3b0`
/// latch (`cmpb $0x0,0x3b0(%rcx)`), picks the confirm/cancel sound by
/// `MenuJobResult::IsSuccess`, hands the result to the parent, then SETS `+0x3b0`. It is the
/// single choke point every answer passes through -- a pressed Yes, a pressed No, and the
/// cancel/auto-close path (`FUN_1407ac890`, which emits `Failed`) -- which is why the
/// save-flow observes it instead of guessing from dialog fields.
pub(crate) const MENU_JOB_EMIT_RESULT_RVA: u32 = 0x746e80;
/// Prologue of `MENU_JOB_EMIT_RESULT_RVA` (`mov %rdx,0x10(%rsp); push %rbx; sub $0x80,%rsp`),
/// byte-verified against `eldenring-deobf.bin` 2026-07-28.
pub(crate) const MENU_JOB_EMIT_RESULT_SIG: &[u8] =
    &[0x48, 0x89, 0x54, 0x24, 0x10, 0x53, 0x48, 0x81, 0xec, 0x80, 0x00, 0x00, 0x00];
pub(crate) static MENU_JOB_EMIT_RESULT_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) const MENU_JOB_EMIT_RESULT_NOT_INSTALLED: usize = 0;
pub(crate) const MENU_JOB_EMIT_RESULT_INSTALLED_YES: usize = 1;
pub(crate) use er_telemetry::counters::MENU_JOB_EMIT_RESULT_INSTALLED;
/// SaveRetryDialog fade gate the OK-handler (0x78e030) reads: it commits/closes only when
/// fade_current (+0x1278) <= fade_target (+0x2300). Writing fade_current = fade_target bits makes it
/// commit on the first frame (no fade-in animation = no visible flash) instead of ~20 frames.
pub(crate) const MSGBOX_FADE_CURRENT_1278_OFFSET: usize = 0x1278;
pub(crate) const MSGBOX_FADE_TARGET_2300_OFFSET: usize = 0x2300;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MSGBOX_FINISHED_TRUE: u8 = true as u8;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MSGBOX_FINISHED_FALSE: u8 = false as u8;
pub(crate) const AUTO_ACCEPT_LOG_INTERVAL: usize = 30;
/// Original finished-poll getter trampoline (0 until the hook installs).
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static MSGBOX_FINISHED_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::AUTO_ACCEPT_INSTALLED;
pub(crate) const AUTO_ACCEPT_NOT_INSTALLED: usize = 0;
pub(crate) const AUTO_ACCEPT_INSTALLED_YES: usize = 1;
/// Set once when the local player first exists in-world; gates the auto-accept OFF so in-game
/// MessageBoxDialogs (which need real choices) are never force-accepted.
pub(crate) use er_telemetry::counters::IN_WORLD_REACHED;
pub(crate) use er_telemetry::counters::LOADGAME_BUILDER_LAST_NATIVE_SLOT;
pub(crate) use er_telemetry::counters::LOADGAME_BUILDER_SLOT_OVERRIDES;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const IN_WORLD_NOT_REACHED: usize = 0;
pub(crate) use er_title_flow::IN_WORLD_REACHED_YES;
/// The fresh_deser load epoch whose world is genuinely LIVE (play_time advanced past that epoch's
/// baseline), published by the play-time-live oracle. The boot-view compositor uses this as a
/// PER-EPOCH world-reached signal to stop its per-frame GPU readback promptly after an own-menu switch
/// (load2+) becomes playable -- `IN_WORLD_REACHED` above is a stale latch that never re-arms during a
/// switch, so it cannot stop the readback in-world (bd
/// fps-killer-rootcaused-per-frame-gpu-readback-boot-view-not-stopping-inworld-load2). `usize::MAX` = none.
pub(crate) use er_telemetry::counters::BOOT_VIEW_EPOCH_WORLD_LIVE;
/// FPS BAIL for own-menu reloads whose handoff signals never fire: when load2 stalls at the finalize
/// (frozen), it builds no NEW loadscreen table and its play_time never advances, so NEITHER
/// `loading_handoff` NOR `world_handoff` becomes true and the per-frame GPU readback runs forever
/// (~20fps). The loading bar itself DOES fill (present=True/mms18 = world resident) so a permille + time
/// bail reliably stops the readback regardless of the handoff signals. Per-epoch composite clock.
pub(crate) use er_telemetry::counters::BOOT_VIEW_COMPOSITE_EPOCH;
pub(crate) use er_telemetry::counters::BOOT_VIEW_COMPOSITE_FIRST_MS;
// BOOT_VIEW_EPOCH_BAIL_PERMILLE removed 2026-07-31 (er-effects-rs-drb7) along with the FPS bail's
// permille arm: "the bar reads ~full" is a progress reading, not the freeze predicate the bail
// needs, and it fired ~1.3s into every healthy switch. BOOT_VIEW_EPOCH_COMPOSITE_CAP_MS is now the
// bail's only trigger.
/// Hard cap (ms) on how long the composite may run for one own-menu reload epoch before the FPS bail
/// force-stops it, so the GPU readback can never tank FPS indefinitely even if permille stalls.
pub(crate) const BOOT_VIEW_EPOCH_COMPOSITE_CAP_MS: u64 = 20_000;
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
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
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
pub(crate) use er_telemetry::counters::POLICY_TOS_TITLE_HOOK_INSTALLED;
pub(crate) const POLICY_TOS_TITLE_HOOK_NOT_INSTALLED: usize = 0;
pub(crate) const POLICY_TOS_TITLE_HOOK_INSTALLED_YES: usize = 1;
pub(crate) static POLICY_TOS_TITLE_TOTAL_BUILDS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Count of TosMultiLangDialog builds our wrapper skipped (zero-input ToS-modal
/// suppression). Non-zero only when `policy_tos_suppress_enabled()` is on; the
/// suppressed build returns null, mimicking the wrapper's native allocation-failure
/// path so the unnecessary startup ToS modal is never constructed.
pub(crate) use er_telemetry::counters::POLICY_TOS_TITLE_SUPPRESSED_BUILDS;
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
pub(crate) use er_telemetry::counters::SERVER_STATUS_HOOK_INSTALLED;
pub(crate) const SERVER_STATUS_HOOK_NOT_INSTALLED: usize = 0;
pub(crate) const SERVER_STATUS_HOOK_INSTALLED_YES: usize = 1;
pub(crate) static SERVER_STATUS_TOTAL_SEEN: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static SERVER_STATUS_LAST_STATE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static SERVER_STATUS_LAST_TEXT_ID: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static START_SERVER_STATUS_HOOK: Once = Once::new();
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
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
pub(crate) use er_telemetry::counters::SCENE_OBJ_PROXY_CTOR_ORIG;
/// The host MenuWindow* latched from the SceneObjProxy ctor (incoming rdx) at title build. 0 until
/// the title builds. Updated on every VALID (vtable-checked) call. Read by
/// `locate_live_loadgame_node` (SeqCst); fail-closed when still 0.
pub(crate) use er_telemetry::counters::LATCHED_MENU_WINDOW;
/// One-shot install guard for the MenuWindow-latch factory hook (mirrors AUTO_ACCEPT_INSTALLED).
pub(crate) use er_telemetry::counters::MENU_WINDOW_LATCH_INSTALLED;
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
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA: usize = 0x93bb41;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SYSTEM_QUIT_COMPONENT_INDEX_EXPECTED_GAME_END: u8 = 0x0e;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SYSTEM_QUIT_COMPONENT_INDEX_REPLACEMENT_MULTI_SLOT: u8 = 0x02;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SYSTEM_QUIT_COMPONENT_INDEX_PATCH_LEN: usize = 1;
/// Existing native line-help text reused as the visible label/help for the cloned quick-load row.
/// `GR_LineHelp[406000] == "Select profile to load"` in the local FMG dump.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SYSTEM_QUIT_LOAD_LINEHELP_ID: u32 = 406000;
/// Live/deobf `GetGR_LineHelp(MenuString*, int)` (dump `0x140760880` -> live `0x140760790`).
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
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
/// Native save-only routines: `SaveRequest_Profile(bool)` and `RequestSave(bool)`. Distinct from
/// `FUN_14067a490`, which requests save AND sets return-title teardown state.
///
/// Bool PINNED via the 1.16.2 Ghidra decompile (2026-07-28): the parameter is `throttled`.
/// `RequestSave(true)` runs a 60-second throttle against `GameMan+0xb98` (last game-save
/// DLDateTime, +60 window) with an early `return` that sets NOTHING; `false` skips the throttle
/// and always sets `+0xb73` (plus `+0xb72` iff `saveSlot != -1`), gated only on the suppression
/// global. `SaveRequest_Profile` has the same shape (gate `FUN_14080d570` first, throttle vs
/// `+0xb88` `SetSeconds(0x3c)`).
pub(crate) const SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA: u32 = 0x67a420;
pub(crate) const SYSTEM_QUIT_REQUEST_SAVE_RVA: u32 = 0x67a520;
/// The game's OWN retractions of the two save-request flags: `FUN_140678740` clears
/// `GameMan+0xb72` and `FUN_140678710` clears `GameMan+0xb73`. Each is a whole function of
/// three instructions -- load the GameMan singleton from `0x143d69918`, store 0 into the
/// flag byte, return -- so calling them is exactly the game's own retract semantics with
/// no offset of ours in the loop.
///
/// They exist because a refused save lane touches NOTHING: the flags stay latched, the
/// per-frame dispatcher `FUN_140afb880` re-enters the refusing lane every frame, and each
/// entry serializes the whole character into a 0x280000 buffer and throws it away. Measured
/// on a run whose submit builder was refusing: 27,824 declines with 0 serializer failures
/// over 854 s -- ~33 full character serializations per second, indefinitely, on the game
/// thread. Retracting a request that provably cannot be serviced is not a dropped save; it
/// is the end of a spin.
pub(crate) const SAVE_REQUEST_RETRACT_B72_RVA: u32 = 0x678740;
pub(crate) const SAVE_REQUEST_RETRACT_B73_RVA: u32 = 0x678710;
/// Whole-body bytes of the two retractions, byte-verified in `eldenring-deobf.bin` at the
/// same VA (1.16.2, shift 0). `48 8B 05 <disp32>` is the RIP-relative GameMan load whose
/// target resolves to `0x143d69918`; `C6 80 <off32> 00` is the flag store, and the offset
/// immediates (`0xb72` / `0xb73`) are visible in the signature itself. Verified before the
/// call: if the bytes ever differ, the address means something else in that build and the
/// retraction is skipped rather than fired blind at unknown code.
pub(crate) const SAVE_REQUEST_RETRACT_B72_SIG: &[u8] = &[
    0x48, 0x8B, 0x05, 0xD1, 0x11, 0x6F, 0x03, 0xC6, 0x80, 0x72, 0x0B, 0x00, 0x00, 0x00, 0xC3,
];
pub(crate) const SAVE_REQUEST_RETRACT_B73_SIG: &[u8] = &[
    0x48, 0x8B, 0x05, 0x01, 0x12, 0x6F, 0x03, 0xC6, 0x80, 0x73, 0x0B, 0x00, 0x00, 0x00, 0xC3,
];
// ---- SAVE-FLOW state machine (save-game-flow WP1, 2026-07-28; reshaped 2026-07-31) ----
// The Save Game row is CLOSE-THEN-FIRE: a commit is staged (stage 6), the proven close sequence
// runs, and only with menus closed + RAM gates green does the tick arm the one-shot
// er-save-suppress bypass and fire the FORCED (throttle-skipping) request pair.
//
// The row press goes STRAIGHT TO THE DESTINATION LIST (stage 3). It asks nothing first: the only
// question in the flow is the overwrite confirm (stage 4), and it is asked about a file the user
// has already pointed at. Full stage map lives on `er_telemetry::counters::SAVE_FLOW_STAGE`.
//
// STAGE IDS 1 AND 2 ARE RETIRED, NOT REUSED. They were the two up-front confirms. Leaving the gap
// keeps every existing log line and `oracle_save_flow_stage` value in the archive meaning what it
// meant when it was written; renumbering 3..8 down would silently redefine them. A live 1 or 2 is
// now state corruption, and `save_flow_tick`'s catch-all arm resets loudly on it.
pub(crate) const SAVE_FLOW_STAGE_IDLE: usize = 0;
/// The destination browser owns the screen -- opening, being browsed, or tearing down after a
/// destination was chosen (`SAVE_DEST_COMMIT_PENDING`). The Save Game row press enters HERE.
pub(crate) const SAVE_FLOW_STAGE_DEST_BROWSE: usize = 3;
/// The "Are you sure you want to overwrite this file?" confirm is up over the destination
/// browser (default No). The only confirm in the flow.
pub(crate) const SAVE_FLOW_STAGE_OVERWRITE_CONFIRM: usize = 4;
/// WP2: the user declined (or a recipe failure aborted); the close sequence is running and
/// NOTHING will be written.
pub(crate) const SAVE_FLOW_STAGE_CLOSING_ABORT: usize = 5;
pub(crate) const SAVE_FLOW_STAGE_CLOSING_COMMIT: usize = 6;
pub(crate) const SAVE_FLOW_STAGE_FIRE_GATE_WAIT: usize = 7;
pub(crate) const SAVE_FLOW_STAGE_COMMIT_WAIT: usize = 8;
/// Stage-7 fire-gate timeout (~10 s at 60 game-task ticks/s): if the RAM gates never go
/// green the flow aborts WITHOUT firing (the bypass token is only armed at the green edge,
/// so nothing is left armed).
pub(crate) const SAVE_FLOW_FIRE_GATE_TIMEOUT_TICKS: usize = 600;
/// Stage-8 commit watchdog (~15 s): a fired request whose enqueue/terminal status never
/// arrives expires the still-pending bypass token so it can never leak onto a later
/// native save.
pub(crate) const SAVE_BYPASS_WATCHDOG_TICKS: usize = 900;
/// `CSMenuMan+0x80` -> the save-gate sub-object read by `FUN_14080d660` /
/// `SaveRequest_Profile`'s gate `FUN_14080d570`: the gate fails PERMANENTLY for the session
/// while the `+0x290` byte or `+0x298` qword is latched nonzero (same fields the switch-2
/// save-freeze diagnostic `system_quit_log_save_gates` reads inline).
pub(crate) const CS_MENU_MAN_SAVE_GATE_SUB_80_OFFSET: usize = 0x80;
pub(crate) const CS_MENU_MAN_SAVE_GATE_LATCH_290_OFFSET: usize = 0x290;
pub(crate) const CS_MENU_MAN_SAVE_GATE_LATCH_298_OFFSET: usize = 0x298;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BYPASS_ALLOWED_AT_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_COMMIT_COMPLETE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_COMMIT_VERIFY_FAIL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_DIALOG;
pub(crate) use er_telemetry::counters::SAVE_FLOW_ROW_PRESS_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_DISPATCH_CALLS_AT_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_DISPATCH_DECLINES_AT_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_COMMIT_JOB_START_TICK;
pub(crate) use er_telemetry::counters::SAVE_FLOW_COMMIT_WATCHDOG_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_ENQUEUE_MISSING_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_SAVE_JOB_STARTS_AT_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_SERIALIZE_CALLS_AT_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_SERIALIZE_FAILURES_AT_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_SUBMITS_SWALLOWED_AT_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_B72_BEFORE_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_B73_BEFORE_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_FLAG_UNREAD;
pub(crate) use er_telemetry::counters::SAVE_FLOW_REQUEST_RETRACTIONS;
pub(crate) use er_telemetry::counters::SAVE_FLOW_RETRACT_DECLINED;
/// Game-task ticks stage 8 waits for the fired save request to actually REACH the writer (an SL
/// save enqueue arriving at the suppressor) before declaring the fire failed. ~3 s at 60 ticks/s,
/// the same budget the confirm-box build and destination-browser open timeouts use.
///
/// This exists because the full `SAVE_BYPASS_WATCHDOG_TICKS` (900) is the wrong bound for a fire
/// that never dispatched: the Save Game row is gated on the flow being IDLE, so a dead stage 8
/// froze every subsequent press for ~15-30 s (user-reported 2026-07-28). A write that IS in
/// flight still gets the full watchdog -- only the never-enqueued case bails early.
pub(crate) const SAVE_FLOW_ENQUEUE_GRACE_TICKS: usize = 180;
pub(crate) use er_telemetry::counters::SAVE_FLOW_GATE_LATCH_BLOCKED_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_STAGE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_STAGE_TICKS;
// ---- SAVE-FLOW confirm box (save-game-flow WP2, 2026-07-28; one box since 2026-07-31) ----
// The Save Game row does not commit on press and does not ask on press: it opens the destination
// list. The flow's ONE confirm ("Are you sure you want to overwrite this file?", default No) is
// built through the GAME's own `CS::MessageBoxBuilder` recipe (RVAs below) and submitted to a
// MenuJob queue, so it is localized, skinned and input-routed exactly like the native quit
// confirm. See `save_flow_boxes.rs` for the recipe.
pub(crate) use er_telemetry::counters::SAVE_FLOW_ABORT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_BUILD_TIMEOUT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_DIALOG;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_EXPECTED;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_EMIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_EMIT_DIALOG;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_EMIT_STATE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_IDENTITY_LOST_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_NO_COUNTS;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_OPEN_COUNTS;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_RESULT_BASELINE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_UNDECIDABLE_COUNTS;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_YES_COUNTS;
pub(crate) use er_telemetry::counters::SAVE_FLOW_RECIPE_UNAVAILABLE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_SUBMIT_BOX_PENDING;
/// Game-task ticks a submitted confirm box may go without its `CS::MessageBoxDialog` build
/// reaching the builder hook (~3 s at 60 ticks/s). Exceeding it means the recipe produced no
/// visible box, so the flow aborts back to the world instead of waiting on a box that will
/// never appear. There is deliberately NO timeout on the user's DECISION.
pub(crate) const SAVE_FLOW_BOX_BUILD_TIMEOUT_TICKS: usize = 180;
pub(crate) use er_telemetry::counters::SAVE_FLOW_BOX_HOST_DIALOG;
// ---- SAVE-DESTINATION browser (save-game-flow WP3, 2026-07-28) ----
// The Save Game row press opens the shipping `05_010` picker REPURPOSED as a save-destination
// chooser (`[ new ]` is the initial selection, below the always-first drive row when present), and
// the commit writes there instead of the loaded save by
// diverting the native writer's single container write-open. See `save_dest_commit.rs`. A pick
// that resolves back to the LOADED save is recognised as such and routed to the sanctioned
// in-place overwrite -- with the up-front "Overwrite your loaded save?" box gone, that is the ONLY
// way a user overwrites their own save, so the filesystem-identity check is load-bearing.
/// 1 = an OS Save-As chose an existing file and the Box3 overwrite confirm is owed. Consumed by
/// the save-flow tick, which owns every `SAVE_FLOW_STAGE` transition on the OS path.
pub(crate) use er_telemetry::counters::SAVE_DEST_CONFIRM_PENDING;
pub(crate) use er_telemetry::counters::SAVE_DEST_OVERWRITE_UNCONFIRMABLE_COUNT;
/// Game-task ticks the save flow waits for the destination browser to actually appear after the
/// open was staged for the menu pump (~3 s at 60 ticks/s, same budget as a confirm-box build).
/// Exceeding it aborts back to the world with nothing written.
pub(crate) const SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS: usize = 180;
// ---- DESTINATION-COMMIT SAFETY ORACLES (2026-07-29) ----
// Each one names a decision the commit refused to guess at, a wait it had to take, or a fact it
// could not establish. They exist because the previous shape of this flow could destroy the loaded
// save while its log read "restored pre-fire snapshot ok=true": a decision it got wrong had no
// name, so no run could report it.
pub(crate) use er_telemetry::counters::SAVE_DEST_DISARM_DEFERRED;
pub(crate) use er_telemetry::counters::SAVE_DEST_DISARM_UNPROVEN;
pub(crate) use er_telemetry::counters::SAVE_DEST_FOREIGN_OPEN_PASSED;
pub(crate) use er_telemetry::counters::SAVE_DEST_IDENTITY_UNKNOWN_ABORT;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_STAT_UNREADABLE;
pub(crate) use er_telemetry::counters::SAVE_DEST_NO_WRITER_OBSERVER_ABORT;
pub(crate) use er_telemetry::counters::SAVE_DEST_RESTORE_FAILED;
pub(crate) use er_telemetry::counters::SAVE_DEST_RESTORE_SUPPRESSED;
pub(crate) use er_telemetry::counters::SAVE_DEST_SELF_REDIRECT_BLOCKED;
pub(crate) use er_telemetry::counters::SAVE_FLOW_DEGRADED_COMPLETE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_DEGRADED_FIRE;
pub(crate) use er_telemetry::counters::SAVE_FLOW_DEGRADED_UNOBSERVED_COUNT;
pub(crate) use er_telemetry::counters::SAVE_FLOW_SAVE_JOB_COMPLETIONS_AT_FIRE;
/// Extra game-task ticks past `SAVE_BYPASS_WATCHDOG_TICKS` that a destination commit will hold its
/// redirect window open when the bypass token was CONSUMED but no save-job body ever started
/// (~60 s at 60 ticks/s).
///
/// The window cannot be closed on the watchdog alone: an enqueue that the worker has not picked up
/// yet is a write that has not happened, and disarming in front of it sends the native writer's
/// per-block opens to the loaded save. Waiting is the safe side, so the extension exists purely so
/// a permanently stalled SL queue cannot disable the Save Game row for the rest of the session --
/// and reaching it is a named failure (`oracle_save_dest_disarm_unproven`), never a quiet timeout.
/// A writer that is genuinely INSIDE a body is never cut off by this; that case waits it out.
pub(crate) const SAVE_DEST_TEARDOWN_UNPROVEN_EXTRA_TICKS: usize = 3600;
/// `PropertyEditDialog` pointer stored at `action_object + 0x8` -- how every System>Quit row
/// action reaches its owning dialog.
pub(crate) const SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET: usize = 0x8;
/// `CS::MessageBoxBuilder` recipe, byte-verified against `eldenring-deobf.bin` on 2026-07-28
/// and lifted verbatim from the native Yes/No confirm wrapper `FUN_1407b73d0` (whose own
/// disassembly is the source for the call order and argument registers). Every address is
/// re-checked against its prologue at first use; a mismatch disarms the whole chain rather
/// than calling into a drifted build.
///
/// `ctor(rcx=builder, rdx=ctx, r8=prompt MenuString*, r9=&mode_i32, [rsp+0x28]=0u8)`
pub(crate) const SYSTEM_QUIT_MSGBOX_BUILDER_CTOR_RVA: u32 = 0x7af730;
pub(crate) const SYSTEM_QUIT_MSGBOX_BUILDER_CTOR_SIG: &[u8] =
    &[0x40, 0x55, 0x56, 0x57, 0x48, 0x81, 0xec, 0x80, 0x00, 0x00, 0x00];
/// `add_yes(rcx=builder, rdx=&SaveFlowYesButtonDesc) -> builder` (localized Yes label).
pub(crate) const SYSTEM_QUIT_MSGBOX_ADD_YES_RVA: u32 = 0x7b1c70;
pub(crate) const SYSTEM_QUIT_MSGBOX_ADD_YES_SIG: &[u8] =
    &[0x4c, 0x8b, 0xdc, 0x57, 0x48, 0x81, 0xec, 0x90, 0x00, 0x00, 0x00];
/// `add_no(rcx=builder) -> builder` (localized No/Cancel label; builds its own descriptor).
pub(crate) const SYSTEM_QUIT_MSGBOX_ADD_NO_RVA: u32 = 0x7b1900;
pub(crate) const SYSTEM_QUIT_MSGBOX_ADD_NO_SIG: &[u8] =
    &[0x40, 0x57, 0x48, 0x81, 0xec, 0xa0, 0x00, 0x00, 0x00];
/// `default_last(rcx=builder) -> builder`; whole body is
/// `*(i32*)(builder+0x28) = *(i32*)(builder+0x10f0) - 1`, i.e. the default choice is the
/// LAST button added. That is why add order encodes the default.
pub(crate) const SYSTEM_QUIT_MSGBOX_DEFAULT_LAST_RVA: u32 = 0x7b1b60;
pub(crate) const SYSTEM_QUIT_MSGBOX_DEFAULT_LAST_SIG: &[u8] = &[
    0x8b, 0x81, 0xf0, 0x10, 0x00, 0x00, 0xff, 0xc8, 0x89, 0x41, 0x28, 0x48, 0x8b, 0xc1, 0xc3,
];
/// `finalize(rcx=builder, rdx=&job_slot, r8b=0) -> &job_slot`: writes the built MenuJob
/// reference into the caller's slot.
pub(crate) const SYSTEM_QUIT_MSGBOX_FINALIZE_RVA: u32 = 0x7b10f0;
pub(crate) const SYSTEM_QUIT_MSGBOX_FINALIZE_SIG: &[u8] = &[
    0x4c, 0x8b, 0xdc, 0x56, 0x57, 0x41, 0x56, 0x48, 0x81, 0xec, 0x30, 0x01, 0x00, 0x00,
];
/// `dtor(rcx=builder)`: tears down the stack builder once the job is built.
pub(crate) const SYSTEM_QUIT_MSGBOX_DTOR_RVA: u32 = 0x7b0140;
pub(crate) const SYSTEM_QUIT_MSGBOX_DTOR_SIG: &[u8] =
    &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x30];
/// Stack footprint of `CS::MessageBoxBuilder` (`sub $0x11b8,%rsp` frame in `FUN_1407b73d0`
/// hands `lea 0x60(%rsp)` to the ctor and `finalize` reads up to `builder+0x1138`).
pub(crate) const MSGBOX_BUILDER_SIZE: usize = 0x1140;
/// Builder mode dword every native Yes/No confirm passes (`movl $0x17`). Only ONE dword is
/// read from the pointer (`mov (%rdx),%eax` in the sub-ctor `FUN_14078c950`).
pub(crate) const MSGBOX_BUILDER_MODE_CONFIRM: i32 = 0x17;
/// Builder trailing byte argument (5th stack arg) every native confirm passes.
pub(crate) const MSGBOX_BUILDER_CTOR_TRAILING_ARG: u8 = 0;
/// Buttons added so far (`builder+0x10f0`), incremented by each adder.
pub(crate) const MSGBOX_BUILDER_BUTTON_COUNT_OFF: usize = 0x10f0;
/// Default/initial cursor index (`builder+0x28`), written by `default_last`.
pub(crate) const MSGBOX_BUILDER_DEFAULT_IDX_OFF: usize = 0x28;
/// `PropertyEditDialog+0x10` is the MenuJob queue our confirm boxes submit to, and
/// `+0x50` the owning MenuWindow list passed as the builder's context -- the same two
/// derivations `system_quit_open_profile_load_dialog` /
/// `system_quit_submit_direct_return_title_chain` already make from the System dialog.
pub(crate) const SYSTEM_QUIT_DIALOG_MENU_JOB_QUEUE_10_OFFSET: usize = 0x10;
pub(crate) const SYSTEM_QUIT_DIALOG_MENU_WINDOW_LIST_50_OFFSET: usize = 0x50;
/// `CS::MenuString` is 0x38 bytes: `MenuHelpLabelComponent` stores its second MenuString at
/// `MENU_HELP_LABEL_HELP_OFFSET`.
pub(crate) const MENU_STRING_SIZE: usize = MENU_HELP_LABEL_HELP_OFFSET;
/// One-shot spawn guard for the boot-time er-save-suppress install thread (bootstrap.rs).
pub(crate) static START_SAVE_SUPPRESS: Once = Once::new();
/// One-shot spawn guard for the boot-time CORE file-ops install thread (CreateFileW in every save
/// mode; the save-destination commit rides that detour).
pub(crate) static START_SAVE_FILE_OPS_CORE: Once = Once::new();
/// One-shot spawn guard for the OBSERVERS-ONLY save-lane attribution thread, used when save
/// suppression is left at its product default of off. The observers call their originals and only
/// count, so binding them changes no save behaviour -- but without them
/// `oracle_save_dispatch_last_decline_reason` reads `unsampled`, which is the field that names why a
/// save was refused and therefore why `GameMan+0xb72`/`+0xb73` stay latched after a reload.
pub(crate) static START_SAVE_OBSERVERS: Once = Once::new();
/// `MenuHelpLabelComponent` contains two `MenuString` objects: visible label at +0, help at +0x38.
pub(crate) const MENU_HELP_LABEL_HELP_OFFSET: usize = 0x38;
pub(crate) const MENU_HELP_LABEL_SIZE: usize = 0x70;
/// Live/deobf `MenuHelpLabelComponent::~MenuHelpLabelComponent` (dump `0x140742d90`).
pub(crate) const MENU_HELP_LABEL_DTOR_RVA: u32 = 0x742c90;
/// Quit Game / return-to-title action std::function-like vtable used by the native Quit Game builder.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
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
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SYSTEM_QUIT_NOOP_ACTION_SENTINEL: usize = 0x4552_5351_4e4f_4f50;
/// `PropertyEditDialog.properties.items`: 0x1260 + BasicViewItemList.items(+8).
pub(crate) const PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET: usize = 0x1268;
/// `PropertyEditDialog.properties.items.count`: 0x1260 + BasicViewItemList.items(+8) +
/// DLFixedVector<EditProperty>.count(+0x888). Pure diagnostic read only.
pub(crate) const PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET: usize = 0x1af0;
pub(crate) const EDIT_PROPERTY_SIZE: usize = 0x88;
pub(crate) const EDIT_PROPERTY_CONTROLLER_OFFSET: usize = 0x78;
/// `CS::EditProperty.label` (a `CS::MenuHelpLabelComponent`, 0x70 bytes) whose FIRST field is the
/// `MenuString`'s raw UTF-16 pointer -- `CS::MenuString::MenuString` stores the pointer it is handed,
/// so a row built from this DLL's static label arrays is identifiable by pointer equality, and every
/// row is identifiable by its text. This is the only per-row identity in the Quit dialog that the
/// engine does not alias or share (1.16.2 `EditProperty`: super_MenuViewItem +0, label +8,
/// propertyController +0x78, size 0x88).
pub(crate) const EDIT_PROPERTY_LABEL_OFFSET: usize = 0x8;
/// In `PropertyNewButtonController`, the action `std::function`'s `_Getter()` slot.
///
/// NOT an object identity. `CS::PropertyNewButtonController` is a 0x300-byte allocation
/// (`HeapAlloc(0x300, 8, ...)` in 1.16.2 `FUN_14086a950`) whose ctor `FUN_14086a2a0`
/// copy-constructs the caller's action `std::function` into `this + 0x70` (`param_1 + 0xe`) and
/// stores the resulting getter pointer here (`param_1[0x15]`). MSVC keeps that getter at
/// `storage + 0x38`, and for a small (inline) callable it points at the storage itself -- so this
/// field always reads back `controller + 0x70`. Comparing it is comparing the controller, and a
/// controller is NOT a row: the patched 4-row Quit tab dispatches all four visible buttons through
/// only the two NATIVE row controllers. Use `system_quit_resolve_row_now` for row identity.
pub(crate) const PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET: usize = 0xa8;
/// `CS::CSEzMenuViewerPad` predicates that `PropertyNewButtonController`'s should-invoke predicate
/// (`FUN_140974b00`, deobf 0x974b00) itself calls to classify the dispatched event. The first
/// short-circuits the predicate with NO positional test (pad/keyboard confirm); the second is the one
/// whose result the native code then hit-tests against the row's display object (mouse click).
pub(crate) const MENU_VIEWER_PAD_CONFIRM_PRESSED_RVA: u32 = 0x758a10;
pub(crate) const MENU_VIEWER_PAD_MOUSE_CLICKED_RVA: u32 = 0x758a70;
/// `CS::GridControl::SetItemCount(this, count)` (1.16.2 `FUN_140738dc0`; byte-verified against
/// `eldenring-deobf.bin`: `48 89 5c 24 08 57 48 83 ec 20 8b fa 48 8b d9 85 d2 75 0d`).
///
/// The list widget's item count is NOT a plain field to poke. This setter writes `this + 0xd0` AND
/// recomputes the scroll/page row count on the embedded scroll control at `this + 0x1a8`
/// (`FUN_14074dad0(this + 0x1a8, (count - 1 + cols) / cols)`), which is what the native rebuild
/// (`FUN_140975040`) calls. Writing the raw field leaves the scroll control describing the old,
/// shorter list.
pub(crate) const GRID_CONTROL_SET_ITEM_COUNT_RVA: u32 = 0x738dc0;
/// The `CS::GridControl` (0x7c8 bytes, vtable dump `0x142a913b8`) embedded in every
/// `GenericListSelectDialog` at `+0xa38`. Its geometry fields, measured once at dialog construction
/// by `GridControl::MeasureGridFromMovie` (vtable `+0x18`, `FUN_140737c60`) from which
/// `Item_<row>_<col>` components the movie actually contains:
///   `+0xd0` item count, `+0xd4` cursor, `+0xd8` COLUMNS, `+0xdc` ROWS.
/// `GridControl::Update` (`FUN_1407392f0`) enables up/down only at `rows >= 2` and left/right only at
/// `cols != 1 || rows < 2`, and the mouse hit test (`FUN_140736c90`) walks exactly `cols * rows`
/// cells -- so these two numbers are the whole navigation and hover model of the dialog.
pub(crate) const DIALOG_GRID_CONTROL_A38_OFFSET: usize = 0xa38;
pub(crate) const GRID_CONTROL_COLS_D8_OFFSET: usize = 0xd8;
pub(crate) const GRID_CONTROL_ROWS_DC_OFFSET: usize = 0xdc;
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
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_DUPLICATE_INSTALLED;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_NOOP_ACTION_INSTALLED;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED;
pub(crate) use er_telemetry::counters::PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GAME_TEXT_INSTALLED;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GAME_CONFIRM_INSTALLED;
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
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_DUPLICATE_COUNT;
/// Native first Quit-tab row action object (label is replaced to Save Game by our text hook). Captured
/// from the row table immediately after the native first AddCancelButton call returns.
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_NATIVE_SAVE_GAME_ACTION_LAST_OBJECT;
/// Native second Quit-tab row action object (Return to Desktop). The patched 4-slot GameEnd GFx can
/// still dispatch this native object for the lower visual buttons; the action hook disambiguates those
/// by the live dialog cursor so row 2/3 become Load Character / Load Character from File instead of showing
/// the native desktop confirmation.
pub(crate) static SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_ACTION_LAST_OBJECT: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_NOOP_SELECTION_COUNT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GAME_TEXT_SUBSTITUTION_COUNT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GAME_CONFIRM_COUNT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GAME_CLOSE_COUNT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_WINDOW;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES;
/// Recorded cloned action implementation object for the quick-load row; only this action is routed.
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT;
/// Recorded `PropertyNewButtonController` for the quick-load row. This is the authoritative click
/// dispatch identity when the GFx/native bridge bypasses the action-object thunk.
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT;
/// Recorded cloned action implementation object for the save-folder row; only this action opens the
/// env-provided save directory.
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT;
/// Recorded `PropertyNewButtonController` for the save-folder row. This is the authoritative click
/// dispatch identity when the GFx/native bridge bypasses the action-object thunk.
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_OPEN_SAVE_DIR_SUCCESS_COUNT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT;
// ---- System->Quit ROW IDENTITY table + resolution oracles (see system_quit_row_identity.rs) ----
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_CONTROLLER_LAST_OBJECT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_ROW_INDEX_LOAD_PROFILE_PLUS1;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_ROW_INDEX_LOAD_SAVE_PROFILES_PLUS1;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_ROW_INDEX_RETURN_DESKTOP_PLUS1;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_ROW_INDEX_SAVE_GAME_PLUS1;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_ROW_TABLE_DIALOG;
pub(crate) use er_telemetry::counters::{
    SYSTEM_QUIT_ACTION_ALIAS_FALSE_QUIT_CLAIMS, SYSTEM_QUIT_QUIT_AUTHORIZED_COUNT,
    SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT, SYSTEM_QUIT_ROW_AMBIGUOUS_COUNT,
    SYSTEM_QUIT_ROW_LAST_AMBIGUITY, SYSTEM_QUIT_ROW_LAST_CURSOR_LABEL_KIND,
    SYSTEM_QUIT_ROW_LAST_CURSOR_PLUS1, SYSTEM_QUIT_ROW_LAST_DISCRIMINATOR,
    SYSTEM_QUIT_GRID_COLS, SYSTEM_QUIT_GRID_ITEM_COUNT, SYSTEM_QUIT_GRID_NAVIGABLE_CELLS,
    SYSTEM_QUIT_GRID_ROWS, SYSTEM_QUIT_ROW_LAST_INPUT_KIND, SYSTEM_QUIT_ROW_LAST_RESOLVED_ROW,
    SYSTEM_QUIT_ROW_REFUSED_DISAGREEMENT_COUNT, SYSTEM_QUIT_ROW_RESOLVED_BY_CURSOR_ROW_COUNT,
    SYSTEM_QUIT_ROW_RESOLVE_COUNT,
};
/// Legacy fallback latch for older confirmation-based Save Game routing. The product Save Game row
/// now requests save + closes menus directly and clears this latch so it never reaches the native
/// Quit Game / return-title action.
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG;
/// Stable qword slot passed to the native `05_010_ProfileSelect` wrapper. The wrapper writes the
/// MenuWindowJob pointer here and captures this slot for its later ProfileLoadDialog factory call.
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT;
/// Live/deobf native `05_010_ProfileSelect` wrapper (`FUN_14081f7e0` dump -> live `0x14081f6f0`).
pub(crate) const PROFILE_SELECT_WRAPPER_RVA: u32 = 0x81f6f0;
pub(crate) use er_title_flow::MENU_JOB_SUBMIT_RVA;
pub(crate) use er_title_flow::MENU_JOB_QUEUE_READY_RVA;
/// Live/deobf native `CS::MenuJob::ChainMenuJobs` (`0x1407a7ca0` dump -> live `0x1407a7bb0`).
/// ABI: `rcx=&first_job_slot, rdx=&out_job_slot, r8=&second_job_slot`; it builds a native
/// FixOrderJobSequence so the existing menu/job pump owns both jobs rather than a private manual pump.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const MENU_JOB_CHAIN_MENU_JOBS_RVA: u32 = 0x7a7bb0;
/// Live/deobf native ProfileSelect LoadJob builder (`FUN_140826600` dump -> live `0x140826510`).
/// ABI: `rcx=&out_job_slot, rdx=dialog+0x50/list, r8d=profile_id, r9=*(dialog+0x1cc8)`.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
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
pub(crate) use er_telemetry::counters::SCALEFORM_HANDLER_TRACE_INSTALLED;
/// Live handler-object addresses (ctor'd, not yet dtor'd). Linear-scanned Vec -- volume is a few
/// dozen menu handlers, not a hot per-frame path. Capped so a genuine leak can't grow it unbounded.
pub(crate) static SCALEFORM_HANDLER_LIVE: std::sync::Mutex<Vec<usize>> =
    std::sync::Mutex::new(Vec::new());
pub(crate) const SCALEFORM_HANDLER_LIVE_CAP: usize = 8192;
/// Oracles: total ctors/dtors seen, double-frees detected+skipped, and the last skipped object +
/// its container/parent for correlation with the switch timeline.
pub(crate) use er_telemetry::counters::SCALEFORM_HANDLER_CTORS;
pub(crate) use er_telemetry::counters::SCALEFORM_HANDLER_DTORS;
pub(crate) use er_telemetry::counters::SCALEFORM_HANDLER_DOUBLE_FREES;
pub(crate) use er_telemetry::counters::SCALEFORM_HANDLER_LAST_DOUBLE_FREE_OBJ;

/// ~MenuWindowJob DOOMED-WINDOW GUARD (er-effects-rs-j74t, return-to-title crash rva 0x7ada87 AND
/// 0x7adb28). Root of the whack-a-mole: `CS::MenuWindowJob::~MenuWindowJob` (deobf 0x1407ac720) calls
/// the finalize (deobf 0x1407ada40), whose `if (owningMenuWindow != 0)` block virtual-CALLS the window
/// (`vfptr[3]`) to fetch an input-event descriptor, indexes the event table by the descriptor's first
/// i32, and CONSUMES the entry as a refcounted CSScaleformLoadResult. During return-to-title the title
/// window is doomed: `vfptr[3]` leaves the descriptor index UNINITIALISED (stack garbage, e.g.
/// 0x7FFF0000/0x51EBA95), so the table pointer is wildly OOB. Patching individual dereferences just
/// moved the crash (getter deref 0x7ada87 -> second virtual call 0x7adb28 with a nulled vtable), and
/// the entry is consumed as a GFx object so there is no safe sentinel entry. The only correct fix is to
/// keep the finalize's whole window block from running. All observed crashes had `menu_id == 0xffff`
/// (the unmapped state) and reproduce the descriptor index via `vfptr[3]`. At the destructor we
/// reproduce that exact call: if owningMenuWindow's vtable is not in the game module (freed+reused), or
/// its `vfptr[3]` yields an out-of-range index (doomed), we null `owningMenuWindow` so the finalize
/// skips the block entirely (and correctly does NOT unref a dead window). Gated to `menu_id == 0xffff`
/// so healthy mapped windows are untouched (byte-identical, no extra virtual call). The dtor forwards
/// rdx/r8/r9 to the finalize untouched, so all four argument registers are forwarded verbatim.
///
/// IDENTITY LAYER (2026-07-29, closes the 2026-07-23 false-negative at rva 0x7ada7c): the state
/// heuristic above forwards whenever `menu_id != 0xffff` -- but a freed-and-REUSED window can carry an
/// in-module vtable and arbitrary non-0xffff bytes at +0x180, so the native finalize then virtual-
/// calls the reused object and crashes at its FIRST `vfptr[3]` call (rva 0x7ada7c; observed
/// 2026-07-23 with the guard armed). No memory-state probe can identify that case. What CAN identify
/// it is OWNERSHIP: the only producer of stale jobs is our own title-cover masquerade, which latches
/// every job it preserves (`title_native_menu_visual_begin_title_hook` /
/// `title_pab_information_visual_hook`). Those latches now also record the job pointer in
/// `MASQUERADE_PRESERVED_JOBS`; at `~MenuWindowJob` a preserved job gets the STRICT lifetime
/// predicate: forward natively only when the window is verifiably still in the state the finalize's
/// contract requires (in-module vtable AND `menu_id < 0x47` mapped, i.e. the native deregistration is
/// valid), probe `vfptr[3]` for the `0xffff` never/de-registered state, and DETACH (native
/// vector-remove `FUN_140733d70` + null `job+0x130`) on every unverifiable state (unreadable window,
/// out-of-module vtable, unreadable or garbage `menu_id`, garbage descriptor index). The 1.16.2
/// finalize (0x1407ada40) and dtor both gate every window access on `owningMenuWindow != 0`, so the
/// detached state is native-tolerated by construction. Non-preserved (native-owned) jobs keep the
/// legacy heuristic byte-identically.
pub(crate) const MENU_WINDOW_JOB_DTOR_RVA: usize = 0x7ac720;

/// `CS::MenuWindowJob` FINALIZE (deobf 0x1407ada40) -- the function whose `if (owningMenuWindow != 0)`
/// block virtual-calls `owningMenuWindow->vfptr[3]`, faulting at rva 0x7ada7c when that window is
/// freed/reused.
///
/// It has FIVE callers and `MENU_WINDOW_JOB_DTOR_RVA` (0x7ac720) is only ONE of them; the others are
/// three sites inside `MenuWindowJob::Run` (0x7ad3fb / 0x7ad54d / 0x7ad66a) and 0x7bdee0. The
/// destructor guard therefore cannot see the crash observed on the profile switch, which arrives via
/// `Run` (proven twice 2026-07-30: agent + user runs, identical AV signature). Hooking the finalize
/// itself covers every caller with one detour, and 0x7ada40 has no other detour -- unlike
/// `MenuWindowJob::Run` (0x7ad1c0), where two detours already collided (MinHook allows one per
/// address; see bd system-quit-menuwindowjob-run-dead-hook-rootcause-2026-07-15).
pub(crate) const MENU_WINDOW_JOB_FINALIZE_RVA: usize = 0x7ada40;
/// `MsbFileCap` load-complete callback -- THE SOLE WRITER of `msbResCap` (`cap+0x90`), 1.16.2 dump
/// `FUN_14021bbf0`. Byte-verified against `eldenring-deobf.bin` at the same VA (shift 0 on 1.16.2):
/// `48 8b c4 56 57 41 56 48 81 ec 80 00 00 00`, with the first rip-relative operand only at +0x1e,
/// so the prologue is safely detourable.
///
/// It writes `msbResCap` ONLY when the cap's content is non-null, and returns normally otherwise --
/// leaving `(loadState=4, msbResCap=0)`, which wedges `WorldBlockRes` case 2 forever. Tracing it
/// separates "fired with null content" (empty read) from "never fired" (cache hit, no enqueue); no
/// passive read can, because both end in identical cap state.
pub(crate) const MSB_FILECAP_PARSE_CALLBACK_RVA: usize = 0x21bbf0;
/// How many SUCCESSFUL parses to log before rate-limiting. Null-result parses are always logged.
pub(crate) const MSB_PARSE_TRACE_VERBOSE_CALLS: usize = 24;
/// `CS::MoveMapListStep::STEP_LoadListWait` -- the ONLY live path that refills the DLC virtual roots
/// (it calls `FUN_140e05fb0(GLOBAL_CSDlc, true)` -> `CSDlcImp::AddVirtualFileRoots`). Proven to be
/// the fix site by bd `PROVEN-reload-softlock-is-blanked-dlc-virtual-root-mapstudio-dlc2-empty`.
///
/// Prologue is `40 53 48 83 ec 20 48 8b 81 c0 02 00 00` (`push rbx; sub rsp,0x20; mov rax,[rcx+0x2c0]`)
/// -- no rip-relative operand anywhere near the patch site, and the deobf bytes match the 1.16.2 dump
/// exactly, so a 5-byte detour relocates cleanly. `rcx` is the `MoveMapListStep` this-pointer.
pub(crate) const STEP_LOADLIST_WAIT_RVA: usize = 0x00af_1800;
/// Gate A operand: `MoveMapListStep::loadList`. The step proceeds when this is NULL **or** the int at
/// `*loadList` is 2 or 3 (`sub eax,2; cmp eax,1; ja bail`).
pub(crate) const MOVEMAPLISTSTEP_LOADLIST_2C0_OFFSET: usize = 0x2c0;
/// Gate B operand: must be 0 for the step to proceed (`cmp qword [rcx+0xb8],0; jnz bail`).
pub(crate) const MOVEMAPLISTSTEP_GATE_B8_OFFSET: usize = 0xb8;
/// `STEP_LoadListWait` runs every frame, so the trace logs only on VERDICT CHANGE plus this many
/// opening entries -- enough to capture the load-1 baseline without burying the reload.
pub(crate) const LOADLIST_WAIT_TRACE_VERBOSE_CALLS: usize = 6;
/// `FUN_140e06490(CSDlcImp*, bool)` -- BLANKS the 13 `*_dlc2` virtual roots to `L""` and clears the
/// DLC ownership flags. Sole code caller is the title start-game flow `FUN_1409b24e0`.
pub(crate) const DLC_ROOTS_BLANK_RVA: usize = 0x00e0_6490;
/// `FUN_140e05fb0(CSDlcImp*, bool)` -- the REFILL: re-queries Steam DLC ownership and calls
/// `CSDlcImp::AddVirtualFileRoots`. Hooked at this shared entry rather than at either caller,
/// because a measured run showed `STEP_LoadListWait` never executes at all.
pub(crate) const DLC_ROOTS_REFILL_RVA: usize = er_game_base::rva::DLC_ROOTS_REFILL_RVA;
/// `FUN_140836f30` -- the `Do` of the MenuFunctorJob that eventually reaches the refill (vtable
/// 0x142acb638). One level above `FUN_140e05fb0`, so it separates "job never enqueued" from "job ran
/// and diverged inside". Prologue `48 89 54 24 10 53 48 83 ec 30`, no rip-relative in the window.
pub(crate) const DLC_ROOTS_JOB_RVA: usize = 0x0083_6f30;
/// `GLOBAL_CSDlc` -- the `CSDlcImp` singleton. Grounded from `FUN_1408371e0`'s own load:
/// `mov 0x354f9ed(%rip),%rcx  # 0x143d86bd8`.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const CSDLC_SINGLETON_RVA: usize = er_game_base::rva::CSDLC_SINGLETON_RVA;
/// The DLIO alias every failing `m28` read resolves through.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const DLC_ROOT_ALIAS_NAME: &str = "mapstudio_dlc2";

/// How many NULL-RESULT parses also carry a DLIO virtual-root dump. The null path fires ~13x/second
/// during the stall and the root walk is a vector scan, so only the first few need it -- the roots
/// do not change once the block is wedged, and the load-1 baseline comes from the verbose successes.
pub(crate) const MSB_PARSE_TRACE_ROOTS_ON_NULL_RESULTS: usize = 4;
pub(crate) const MENU_WINDOW_JOB_OWNING_WINDOW_OFFSET: usize = 0x130;
/// The window's cached menu id (`field246_0x180`). `0xffff` is the unmapped sentinel and the state
/// every observed crash was in; the finalize's second getter is itself gated on it.
pub(crate) const MENU_WINDOW_MENU_ID_OFFSET: usize = 0x180;
pub(crate) const MENU_WINDOW_MENU_ID_UNMAPPED_SENTINEL: u16 = 0xffff;
/// `vfptr[3]` slot (the finalize's `call *0x18(vtable)`): the window's get-input-descriptor method.
pub(crate) const MENU_WINDOW_INPUT_DESC_VTABLE_SLOT: usize = 0x18;
/// Plausible upper bound on a real menu event index. Real indices are tiny (the CSMenuMan flag array
/// `field106_0x90` holds 0x47 entries); observed garbage indices were 0x7FFF0000 / ~85M. Anything
/// `< 0` or `>=` this is unmistakably OOB, so this never rejects a valid index.
pub(crate) const MENU_WINDOW_EVENT_INDEX_SANE_MAX: i32 = 0x1000;
/// Scratch out-buffer size for the reproduced `vfptr[3]` call (descriptors are small; oversized).
pub(crate) const MENU_WINDOW_INPUT_DESC_SCRATCH_LEN: usize = 0x200;
/// Generous game-module span (>= the ~0x5e0_0000 image) bounding a plausible in-module vtable; heap
/// vtables sit far below the game base, so this cleanly separates freed-reused from live.
pub(crate) const GAME_MODULE_VTABLE_SPAN: usize = 0x0800_0000;
/// The job's push-target `DLFixedVector*` (`field2_0x50`, a pointer at job+0x50 -- verified by the
/// finalize's `mov rcx,[rbx+0x50]`). `MenuWindowJob::Run` pushes owningMenuWindow into this vector,
/// and the finalize is supposed to REMOVE it via `FUN_140733e70(field2_0x50, window)` -- but that
/// call sits AFTER the crashing getter, so on a doomed window the removal never runs and the window
/// dangles in the title-step's active-window vector that `STEP_MenuJobWait` walks (the SECOND crash,
/// rva 0x733f80). We perform this removal ourselves in the doomed branch.
pub(crate) const MENU_WINDOW_JOB_PUSH_TARGET_50_OFFSET: usize = 0x50;
/// `FUN_140733e70` (deobf/live 0x140733d70): DLFixedVector pointer-remove-and-compact. ABI
/// `fn(rcx = vector, rdx = window)`; it only searches/removes the pointer and decrements the count at
/// vector+0x48 -- it NEVER dereferences the window's vtable, so it is safe to call on a doomed window.
pub(crate) const MENU_WINDOW_LIST_REMOVE_RVA: usize = 0x733d70;
/// The DLFixedVector count field (`vector+0x48`) + a sane upper bound. We validate the count is
/// readable and in `(0, MAX]` before calling the native removal so a corrupt push-target pointer
/// cannot drive the (non-SEH) native search loop off into unmapped memory.
pub(crate) const MENU_WINDOW_LIST_COUNT_48_OFFSET: usize = 0x48;
pub(crate) const MENU_WINDOW_LIST_SANE_MAX_COUNT: i32 = 64;
pub(crate) static MENU_WINDOW_JOB_DTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_DTOR_TRACE_INSTALLED;
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_DTOR_DOOMED_GUARDS;
pub(crate) use er_telemetry::counters::{
    MENU_WINDOW_JOB_FINALIZE_GUARDS, MENU_WINDOW_JOB_FINALIZE_INSTALLED,
    MENU_WINDOW_JOB_FINALIZE_LAST_WINDOW, MENU_WINDOW_JOB_FINALIZE_ORIG,
};
pub(crate) use er_telemetry::counters::{
    MSB_PARSE_TRACE_CALLS, MSB_PARSE_TRACE_INSTALLED, MSB_PARSE_TRACE_NULL_RESULTS,
    MSB_PARSE_TRACE_ORIG,
};
pub(crate) use er_telemetry::counters::{
    LOADLIST_WAIT_TRACE_CALLS, LOADLIST_WAIT_TRACE_INSTALLED, LOADLIST_WAIT_TRACE_LAST_VERDICT,
    LOADLIST_WAIT_TRACE_ORIG, LOADLIST_WAIT_TRACE_REACHED_STATUS_GATE,
};
pub(crate) use er_telemetry::counters::{
    DLC_ROOTS_BLANK_CALLS, DLC_ROOTS_BLANK_ORIG, DLC_ROOTS_REFILL_CALLS, DLC_ROOTS_REFILL_ORIG,
    DLC_ROOTS_TRACE_INSTALLED,
};
pub(crate) use er_telemetry::counters::{DLC_ROOTS_JOB_CALLS, DLC_ROOTS_JOB_ORIG};
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_DTOR_LIST_REMOVALS;
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_DTOR_LAST_GUARDED_WINDOW;
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_DTOR_LAST_GUARDED_INDEX;
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_DTOR_PRESERVED_STALE_DETACHES;
/// Upper bound (exclusive) of a MAPPED menu id: the 1.16.2 finalize's own flag-clear guard is
/// `if (*menu_id < 0x47) GLOBAL_CSMenuMan->field99_0x90[*menu_id] = 0;` and the sibling
/// `MenuWindowJob::Run` bounds the same index identically, so `< 0x47` is the game's own definition
/// of "registered in the CSMenuMan window-flag table".
pub(crate) const MENU_WINDOW_MAPPED_MENU_ID_MAX: u16 = 0x47;
/// Identity set of `MenuWindowJob*` values preserved by OUR title-cover masquerade (er-effects-rs-
/// j74t identity layer; see `MENU_WINDOW_JOB_DTOR_RVA`). The part-a latches insert
/// (`masquerade_preserved_job_note`); `menu_window_job_dtor_hook` removes at destruction
/// (`masquerade_preserved_job_take`), so the set self-cleans and 4 slots comfortably cover the at
/// most two preserved jobs (05_000_Title + 05_020_TitleInformation) live per title build. On
/// overflow the job simply falls back to the legacy state heuristic (logged).
pub(crate) const MASQUERADE_PRESERVED_JOB_SLOTS: usize = 4;
pub(crate) static MASQUERADE_PRESERVED_JOBS: [AtomicUsize; MASQUERADE_PRESERVED_JOB_SLOTS] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// QUIT-TO-DESKTOP CLEAN KILL (user directive 2026-07-08): the native quit saves the character then
/// tears the world down and rebuilds the title -- slow, and with our flow the rebuilt title's
/// `CSMenuProfModelRend` looks up the `MenuOffscrRendParam` param table, which the teardown has
/// unloaded, so the game `DLPanic`s (`MenuOffscrRendParam.cpp:0x23`, `GetParamResCap(..., MenuOffscr-
/// RendParam, 0) == NULL`). We turn that exact condition into a fast CLEAN exit: hook the offscreen-
/// render param lookup (inner `LookupMenuOffscrRendParam`, deobf 0x140d3ed90), and when the param
/// TABLE is absent -- which only happens on a quit teardown (it stays resident through loads, proven
/// by the successful repeated loads) -- `ExitProcess(0)` instead of the DLPanic. The native quit has
/// already issued the character save before the rebuild, so this is save-then-kill; the native
/// confirm dialog is untouched (the teardown only runs after Yes). Grounded by the inner lookup's own
/// disasm: repo ptr `0x143d81ee8`, `GetParamResCap` `0x140d4cc50`, `MenuOffscrRendParam` type `0x4e`.
pub(crate) const MENU_OFFSCR_REND_PARAM_LOOKUP_RVA: usize = 0xd3ed90;
pub(crate) const SOLO_PARAM_REPOSITORY_PTR_RVA: usize = 0x3d81ee8;
pub(crate) const GET_PARAM_RESCAP_RVA: usize = 0xd4cc50;
pub(crate) const MENU_OFFSCR_REND_PARAM_TYPE: u32 = 0x4e;
pub(crate) static MENU_OFFSCR_REND_PARAM_LOOKUP_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::MENU_OFFSCR_REND_PARAM_LOOKUP_INSTALLED;
pub(crate) use er_telemetry::counters::QUIT_TO_DESKTOP_CLEAN_KILLS;

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
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
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
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_SAMPLE_COUNT;
/// Last sample: whether the `WindowList` container resolved (0/1).
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_LAST_WINDOWLIST_RESOLVED;
/// Last sample: whether the `WindowList` container's DisplayInfo.Visible was set (0/1).
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_LAST_WINDOWLIST_VISIBLE;
/// Last sample: bitmask (bit N = pane N of `OPTIONSETTING_PANE_NAMES`) of panes that resolved.
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_LAST_RESOLVED_MASK;
/// Last sample: bitmask of panes whose DisplayInfo.Visible was set.
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_LAST_VISIBLE_MASK;
/// Last sample: the `WindowList` child's raw dataType (for gate diagnosis).
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_LAST_DATATYPE;
/// Count of vcalls skipped fail-closed because objectInterface/vtable/getfn were not game-image-live.
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_GUARD_SKIPS;
/// Last sample: whether the composite sub-dialog job slot was bound (0/1).
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_COMPOSITE_BOUND;
/// Count of samples where the blank-pane signature fired (`WindowList` resolved but NOT visible).
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_BLANK_DETECTED_COUNT;
/// The REAL row-pane signal: the current tab dialog (`*(composite+0xb8)`) and the DisplayInfo.Visible of
/// its embedded pane proxy at `dialog+0x1200` -- the object the game's own tab-select SetVisibles. The 8
/// named WindowList children are always Visible=0 and are NOT the signal (they made blank_detected fire
/// before the user could even reproduce). `actively_shown` = CSMenuMan flag bit 0x4 (drawn this frame).
pub(crate) use er_telemetry::counters::OPTIONSETTING_CURRENT_DIALOG;
pub(crate) use er_telemetry::counters::OPTIONSETTING_CURRENT_PANE_VISIBLE;
pub(crate) use er_telemetry::counters::OPTIONSETTING_CURRENT_PANE_DATATYPE;
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVELY_SHOWN;
pub(crate) use er_telemetry::counters::OPTIONSETTING_LAST_FLAG;
/// Latch: the current pane was seen VISIBLE at least once (a healthy Game Options open). The teardown
/// oracle `..._REAL_BLANK_DETECTED_COUNT` only fires AFTER this latch, so a boot/preload state (pane
/// never yet shown) can never be mistaken for the bug -- the bug is healthy(visible)->blank(hidden).
pub(crate) use er_telemetry::counters::OPTIONSETTING_CURRENT_PANE_EVER_VISIBLE;
/// Run-stopping oracle: healthy pane was seen, THEN the actively-shown current pane went hidden.
pub(crate) use er_telemetry::counters::OPTIONSETTING_REAL_BLANK_DETECTED_COUNT;
/// The selected tab index the user is on (`*(*(window+0x1870+0x10)+0xd4)`) at the last sample, and the
/// cache slot the current pane dialog matches -- to identify WHICH tab is blank (e.g. the Quit/Exit tab
/// where our injected Load-Profile rows live vs the Game tab).
pub(crate) use er_telemetry::counters::OPTIONSETTING_CURRENT_TAB;
/// The System/OptionSetting Quit tab index. The custom Load Character / Load Character from File rows are
/// children of this tab, so Back from their ProfileSelect child must restore this tab as the parent.
pub(crate) const OPTIONSETTING_QUIT_TAB_INDEX: usize = 8;
pub(crate) use er_telemetry::counters::OPTIONSETTING_CURRENT_TAB_AT_BLANK;
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_REAPPLY_COUNT: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_TAB: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_OLD_CURRENT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_SELECTED: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_COUNT;
pub(crate) static SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_LAST_SELECTED: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Count of times the fix forced the actively-shown current tab's pane back visible (via SetVisible on
/// dialog+0x1200 -- the same proxy/call the game's own tab-select uses). Nonzero = the blank was caught
/// and corrected; the pane draws again.
pub(crate) use er_telemetry::counters::OPTIONSETTING_PANE_FIX_APPLIED;
/// Active OptionSetting row-table sampler: read-only row/action classification for the currently
/// visible tab dialog. This is the product-proof oracle for the Game Options/Quit contamination class:
/// tab 0 must not contain cloned quick-load/open-profile actions; Quit tab should contain them once the
/// feature is injected.
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVE_ROW_SAMPLE_COUNT;
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVE_ROW_DIALOG;
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVE_ROW_TAB;
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVE_ROW_COUNT;
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVE_ROW_CLONED_MASK;
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVE_ROW_NATIVE_SAVE_MASK;
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVE_ROW_ACTION_HASH;
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVE_ROW_LABEL_HASH;
pub(crate) use er_telemetry::counters::OPTIONSETTING_ACTIVE_ROW_QUIT_LABEL_MASK;
pub(crate) use er_telemetry::counters::OPTIONSETTING_GAME_OPTIONS_CLONED_ROW_HITS;
pub(crate) use er_telemetry::counters::OPTIONSETTING_GAME_OPTIONS_QUIT_LABEL_HITS;
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
pub(crate) use er_telemetry::counters::GX_RESERVE_CMD_QUEUE_SLOT_INSTALLED;
/// Cumulative occupancy high-water, per-switch high-water (reset by `sq_repro_begin_switch`), the
/// observed capacity, and total reserve calls.
pub(crate) use er_telemetry::counters::GX_CMD_QUEUE_MAX_FILL;
pub(crate) use er_telemetry::counters::GX_CMD_QUEUE_SWITCH_MAX_FILL;
pub(crate) use er_telemetry::counters::GX_CMD_QUEUE_CAP_SEEN;
pub(crate) use er_telemetry::counters::GX_CMD_QUEUE_SUBMITS;
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
pub(crate) use er_telemetry::counters::GX_CMD_QUEUE_HIST_DROPPED;
/// Near-full evidence: hits with count >= cap - margin, and a log throttle so the dump lands BEFORE
/// the crash frame without spamming (one line per 64 near-full reserves).
pub(crate) const GX_CMD_QUEUE_NEARFULL_MARGIN: usize = 24;
pub(crate) const GX_CMD_QUEUE_NEARFULL_LOG_EVERY: usize = 64;
pub(crate) use er_telemetry::counters::GX_CMD_QUEUE_NEARFULL_HITS;
/// BUCKET-TABLE instrument (names the RETAINER class the producer histogram cannot: run 10d proved
/// the drain pump FUN_141b3bdc0 dominates reserves by RESUBMITTING its context list each frame, so
/// the leak is list membership). The pump's context (its param_1; latched by a thin entry hook at
/// deobf 0x1b3bda0, dump 0x141b3bdc0, shift-verified) holds a 109-bucket table of per-frame queue
/// slot ranges: begin i32 at ctx+0x30+idx*0x18, end i32 at ctx+0x34+idx*0x18 (from the pump's
/// bucket-locate loop, bound 0x6d). Nonzero widths per bucket, diffed across switches, name which
/// bucket's submissions grow toward the 192 cap.
pub(crate) const GX_CMD_PUMP_RVA: usize = 0x1b3bda0;
pub(crate) static GX_CMD_PUMP_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::GX_CMD_PUMP_INSTALLED;
pub(crate) use er_telemetry::counters::GX_CMD_PUMP_CTX;
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
pub(crate) use er_telemetry::counters::GX_CMD_QUEUE_PEAK_LAST_LOGGED;
/// COMMAND-BYTE ARENA fill (user-reported render corruption during switch #3's return-title window,
/// 2026-07-03): `reserve_command_queue_slot` allocates command BYTES from a bump arena at
/// queue+0x40 (FUN_141c48e80: alloc counter at arena+0x14, limit at +0x20, cursor at +0x28;
/// remaining = limit - align_up(cursor_lo); on remaining < request it takes a refill/wrap path
/// FUN_141c48f50). If that wraps while earlier commands are unconsumed, live command bytes are
/// overwritten -> garbled draws WITHOUT a crash -- the sub-critical sibling of the 0x1aeaf05
/// slot-table overflow. Track remaining low-water (cumulative + per-switch) to correlate.
pub(crate) const GX_CMD_QUEUE_ARENA_OFFSET: usize = 0x40;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const GX_CMD_ARENA_ALLOC_COUNT_OFFSET: usize = 0x14;
pub(crate) const GX_CMD_ARENA_LIMIT_OFFSET: usize = 0x20;
pub(crate) const GX_CMD_ARENA_CURSOR_OFFSET: usize = 0x28;
/// Low-water sentinel: usize::MAX until the first sample lands.
pub(crate) use er_telemetry::counters::GX_CMD_ARENA_MIN_REMAINING;
pub(crate) use er_telemetry::counters::GX_CMD_ARENA_SWITCH_MIN_REMAINING;
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
pub(crate) const DELAY_DELETE_MAN_SINGLETON_PTR_RVA: usize = er_title_flow::CS_DELAY_DELETE_MAN_GLOBAL_RVA;
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
pub(crate) use er_telemetry::counters::PROFILE_SPARE_ORPHAN;
/// Count of leaked spared renderers reclaimed via the native delete path (repeated-switch GX fix).
pub(crate) use er_telemetry::counters::PROFILE_SPARE_ORPHANS_DELETED;
