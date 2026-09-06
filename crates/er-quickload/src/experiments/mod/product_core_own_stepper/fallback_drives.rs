macro_rules! own_stepper_idx10_fallbacks {
    ($owner:ident, $framectx:ident, $n:ident, $base:ident, $phase:ident, $gm:ident, $c30:ident, $b80:ident, $want_slot:ident, $pass_through:ident) => {{
        // `format_args!` implicit captures resolve at this macro's definition site.
        let owner = $owner;
        let n = $n;
        let gm = $gm;
        let c30 = $c30;
        let b80 = $b80;
        let want_slot = $want_slot;
    // OBSERVE-ONLY NATIVE-CONTINUE mode (PATH B, gated OFF by default). Same precedence/structure as
    // native_load: it does NOT force the title machine -- the native boot advances naturally via
    // pass-through, and once the live menu is rendered + settled we fire the native Continue
    // (load-most-recent) node's run exactly once, then keep observing so the golden oracle +
    // world-stream telemetry are written as the FULL native load (parse+stream+spawn) runs. Pure
    // read-only until the one-shot fire; NO SetState forcing.
    if native_continue_enabled() {
        // native_continue's Continue-node scan/fire was DEAD CODE: the continue-scan never found the
        // node (found_continue_node=0x0 every frame). The zero-input load actually fires via
        // pab-advance + title-accept-byte natural menu-open (verified 2026-06-24,
        // autoload-zero-input-world-reached-validated). Keep only the save-not-loaded watchdog (aborts
        // if the gold never loads) + per-frame world-stream telemetry (mms_state/block_count/
        // io_inflight/player_present), then pass through so the NATIVE title machine advances untouched.
        unsafe { save_load_watchdog() };
        unsafe { own_load_stream_telemetry($base, $gm, $owner, n) };
        $pass_through(false);
        return;
    }
    // `read_global_ptr` rather than `*(($base + RVA) as *const usize)`: the raw form was BOTH
    // unresolved and unguarded. Every `.data` global moved between 1.16.2 and 1.17 -- this one
    // among them -- so the raw read would have returned whatever now occupies the old slot, and it
    // dereferenced an unvalidated address to do it. `read_global_ptr` resolves for the running
    // build and reads fault-tolerantly, answering 0 for a refusal, an unmapped page, or a
    // genuinely null global -- all three of which this closure already treats as "no IO device".
    let read_iodev = || {
        let iodev = er_game_base::mem::read_global_ptr($base, IODEV_GLOBAL_RVA, "IODEV_GLOBAL_RVA");
        if iodev != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe {
                (
                    *((iodev + IODEV_INFLIGHT_10_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_18_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_20_OFFSET) as *const usize),
                )
            }
        } else {
            (
                TITLE_OWNER_SCAN_START_ADDRESS,
                TITLE_OWNER_SCAN_START_ADDRESS,
                TITLE_OWNER_SCAN_START_ADDRESS,
            )
        }
    };
    // DECISIVE save-data experiment (gated OFF by default; SAVE-SAFE). Register the stream worker,
    // then drive the cold $b80 save-IO mount (preview -> poll to b80==3 -> deserialize) so 0x67b290
    // mounts the real char to memory -- NO SetState, NO save write. Bypasses the menu drive while
    // active; pass-through keeps the title ticking so the scheduler ticks the registered worker.
    // SAVE-SAFE verify-only OWN-LOAD buffer-feed probe (gated OFF by default; one-shot). Takes
    // precedence over cold_char_mount: hooks 0x67b100 to feed our sliced .sl2 slot body, calls the
    // native parser 0x67b290(slot), and reads back $c30 + the char fingerprint. NO SetState5, NO
    // save write. Bypasses the menu drive while active; pass-through keeps the title ticking.
    if own_load_enabled() && unsafe { title_boot_ready($owner, $base) } {
        unsafe { own_load_drive($base, $gm, $owner, $want_slot, n) };
        // Per-frame world-stream stall telemetry (pure reads). own_load_drive's one-shot $phase
        // machine fast-forwards to PHASE_DONE after the verify/continue fires, so this runs EVERY
        // own_load frame -- including all the post-continue_confirm/SetState5 loading-screen frames
        // -- and publishes the deepest world-load pump values so a probe log shows whether the
        // stream advances or is frozen at WorldResWait. Gated to the own_load path: never in play.
        unsafe { own_load_stream_telemetry($base, $gm, $owner, n) };
        $pass_through(false);
        return;
    }
    if cold_char_mount_enabled() && unsafe { title_boot_ready($owner, $base) } {
        unsafe { cold_char_mount_drive($base, $gm, $want_slot, n) };
        $pass_through(false);
        return;
    }
    if $phase == OWN_STEPPER_PHASE_MENU {
        // Drive only when the title $owner/scheduler/session/dialog are semantically ready.
        // $want_slot == -1 is the "most-recent" intent (resolved from the dialog's natural
        // highlight at PHASE_S2_ACTIVATE), NOT a "do nothing" signal.
        if !unsafe { title_boot_ready($owner, $base) } {
            if n % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: waiting for title_boot_ready before menu drive #{n} owner=0x{owner:x}"
                ));
            }
            $pass_through(false);
            return;
        }
        if let StartupModalBlockingState::Blocking {
            dialog,
            vtable,
            closing_latch,
        } = startup_modal_blocking_state()
        {
            if n % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: startup_modal_blocking_state=Blocking dialog=0x{dialog:x} vt=0x{vtable:x} closing_latch={closing_latch} before menu drive #{n}"
                ));
            }
            $pass_through(false);
            return;
        }
        // NO-WRITE CHECKPOINT. Path A (b78-route) is RUNTIME-FALSIFIED
        // (pathA-b78-route-falsified-$b80-stuck-latch-gate-2026): disp2 0x140afb880's b78-route
        // is gated by the title-accept latch [0x143d856a0] (SET by load time -> disp2 bails to
        // cleanup every frame), so GameMan+0xb80 never leaves 0 and the native PlayGame
        // defaults to a NEW-GAME null character (which autosaved over the live slot in the
        // Seamless run). Every hand-driven $b80 lever (cold slot-int primitives, b72 lever,
        // b78-route) hits the SAME wall: $b80 reaches 3 ONLY when the native MoveMapListStep
        // async job pumps the menu deserialize 0x14082c240; FD4 stream-worker registration
        // alone does NOT advance $b80 (0x140af1b40 registers the same task 0x144842d40 under the
        // same key 0x59682f01 as the in-game 0x140b0a980 milestone lever-c already tried with
        // $b80 still 0). So idx10 NO LONGER SetState(5)s -- it stays at the title (NO save
        // write) pending the Path B menu-drive (drive the selector-$owner step 0x140826d50 /
        // native Load-Game menu entry so the native async job mounts c30=real before PlayGame).
        // STAGE 1 (NO-WRITE layout verification + zero-input main-menu build). The parked
        // press-any-button title is the FIRST state 10 and has NOT run BeginTitle, so
        // $owner+0x138 holds only intro items, not Continue/Load. (1) Walk the bare tree and
        // log it to VERIFY the live FD4 SBO pointer-vector layout against the static RE
        // (the captured recipe pointers were suspiciously low -- verify before any invoke).
        // (2) Build the main menu zero-input via SetState($owner, 3=BeginTitle): BeginTitle
        // needs no session and writes NO save (it is a menu-UI build), so this is save-safe;
        // it is exactly what the native press does after BeginLogo. The next frames run
        // BeginTitle (populating Continue/Load into $owner+0x138) then return to state 10,
        // where PHASE_MENU_BUILD walks + identifies the Load-Game leaf. Stage 2 (invoke its
        // +0xa8 functor -> drive the dialog -> native mount) follows once this confirms the
        // live layout + item. Every hand-driven $b80 lever is dead (the menu async job is the
        // only thing that mounts $c30 before PlayGame); this is the Path B menu-drive.
        // T0: the common timeline start -- the title is parked at state 10 and we begin the
        // DLL drive. The first timeline_event sets the wall-clock epoch (so all later ms= are
        // measured from here); a native-baseline observe run sets T0 the same way.
        timeline_event(
            "T0",
            n,
            format_args!("owner=0x{owner:x} state10 slot={want_slot} c30=0x{c30:x}"),
        );
        // A PASSIVE mode used to sit here: it skipped forcing the menu and handed off to
        // PHASE_MENU_BUILD to wait for the USER to open Load Game. Its gate,
        // `own_stepper_passive_enabled()`, only ever returned a literal `false`, so idx10 has
        // always forced the menu below. Branch and gate are both deleted, with the other
        // unreachable load-mechanism experiments (input probe, inject-nav, direct build).
        let (bare, bare_tree) = if live_dialog_enabled() {
            (None, None)
        } else {
            (
                unsafe { diagnostic_menu_walk($owner, $base, "bare", true) },
                unsafe {
                    diagnostic_job_tree_walk(
                        $owner,
                        $base,
                        TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                        "bare-tree",
                        true,
                    )
                },
            )
        };
        // STAGE 1c: build the FULL main menu by replicating the engine's OWN press path.
        // The parked press-any-button screen is the FIRST state 10; the native press handler
        // 0x140b0b6b0 issues SetState($owner,2)=BeginLogo, after which the native pump advances
        // 2->3->10 and builds the Continue / Load-Game(d180) / New-Game items into the CSMenu
        // registry at $owner+0xe0. The registry update 0x1409aac10 then ticks EVERY registered
        // entry each frame, so our menu-item Update hook (functor_chain_hits_factory) will
        // capture d180. SetState(3)=BeginTitle ALONE (skipping BeginLogo) only built the
        // BackScreen (runtime: only c000 ticked), so we drive the full sequence. BeginLogo(2)
        // hard-asserts session singleton 0x144588e98 at entry -- read it live; SetState(2) only
        // when non-null, else fall back to SetState(3). Save-safe either way: BeginLogo/BeginTitle
        // are menu-UI builds with NO save write (only SetState(5)/PlayGame writes).
        let session = unsafe { safe_read_usize(er_game_base::mem::game_data_addr($base, SESSION_SINGLETON_144588E98_RVA, "SESSION_SINGLETON_144588E98_RVA")) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let target_state = if session != TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_STEP_BEGIN_LOGO
        } else {
            TITLE_STEP_BEGIN_TITLE
        };
        // CRITICAL: STEP_BeginLogo builds the main-menu list (Continue/Load d180/...) into
        // $owner+0xe0 via 0x14081f180 ONLY when [$owner+0xb8]==0; if set it short-circuits to
        // SetState(3) and skips the build (bd mainmenu-item-builder-into-iterator-tree-2026) --
        // which is why our prior SetState(2) only produced the 3 title-composition items. Clear
        // the gate so BeginLogo runs the full build (zero-input, menu-UI only -> save-safe).
        let beginlogo_gate =
            unsafe { safe_read_usize($owner + TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if target_state == TITLE_STEP_BEGIN_LOGO {
            unsafe {
                *(($owner + TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET) as *mut u32) =
                    TITLE_OWNER_BEGINLOGO_GATE_CLEAR;
            }
        }
        let set_state: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(er_game_base::mem::game_data_addr($base, TITLE_SET_STATE_RVA, "TITLE_SET_STATE_RVA")) };
        unsafe { set_state($owner, target_state) };
        own_stepper_enter_menu_build_phase();
        append_autoload_debug(format_args!(
            "own_stepper: STAGE1c bare-walk done (load_game_138=0x{:x} load_game_tree=0x{:x}) session(0x144588e98)=0x{session:x} beginlogo_gate(0xb8)=0x{beginlogo_gate:x} -> SetState({target_state}) [{}] to build the FULL main menu zero-input (#{n}) slot={want_slot} gm=0x{gm:x} c30=0x{c30:x} b80={b80}",
            bare.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
            bare_tree.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
            if target_state == TITLE_STEP_BEGIN_LOGO {
                "BeginLogo 2->3->10 full menu"
            } else {
                "BeginTitle fallback (session null)"
            }
        ));
        // Suppress unused warnings for consts/statics retained from the falsified cold
        // slot-int drive, synthetic-dispatcher, b78-route, and Continue-shim work.
        let _ = (
            CONTINUE_CONFIRM_RVA,
            B80_FULL_LOAD_INITIATOR_RVA,
            OWN_STEPPER_PHASE_MOUNT,
            OWN_STEPPER_PHASE_DRIVE,
            OWN_STEPPER_PHASE_CONTINUE,
            B80_DISPATCHER1_RVA,
            B80_DISPATCHER2_RVA,
            SYNTH_MMS_SKIP_APPLY_12A_OFFSET,
            SYNTH_MMS_DESER_SLOT_12C_OFFSET,
            SYNTH_MMS_SKIP_APPLY_ON,
            OWN_STEPPER_DRIVE_MAX,
            OWN_STEPPER_SHIM_OWNER_IDX,
            OWN_STEPPER_MOUNT_POLL_MAX,
            OWN_STEPPER_B80_RESIDENT,
            OWN_STEPPER_B80_PREVIEW_LANE,
            OWN_STEPPER_B80_IDLE,
            B80_POLL_RVA,
            B80_POLL_ARG_ZERO,
            B80_LANE1_DRIVER_RVA,
            B80_LOAD_SAVE_DATA_INITIATOR_RVA,
            DESERIALIZE_SLOT_RVA,
            BLANK_SAVE_CONTAINER_REQUEST_RVA,
            WORLD_WORKER_BUILD_RVA,
            crate::runtime_heap_allocator_ptr_or_null as fn() -> usize,
            WORLD_WORKER_BUILD_STATE,
            SYNTHETIC_STEP_STATE_OFFSET,
            FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
            GAME_MAN_REQUESTED_SLOT_B78_OFFSET,
            GAME_MAN_ARM_FLAG_B72_OFFSET,
            TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET,
            TITLE_OWNER_PLAY_GAME_SLOT_OFFSET,
            DEFAULT_PLAY_GAME_MAP,
            TITLE_STEP_PLAY_GAME,
            &raw const OWN_STEPPER_SHIM,
            &raw const SYNTH_MMS_OWNER,
            &raw mut OWN_STEPPER_WORKER_THIS,
            &OWN_STEPPER_DRIVE_CALLS,
            &OWN_STEPPER_MOUNT_POLLS,
        );
        let _ = read_iodev;
        $pass_through(false);
        return;
    }
    if $phase == OWN_STEPPER_PHASE_MENU_BUILD {
        let waits =
            OWN_STEPPER_MENU_BUILD_WAITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
        let menu_elapsed_ms = own_stepper_menu_build_elapsed_ms();
        let menu_build_timed_out = own_stepper_menu_build_timed_out();
        if let StartupModalBlockingState::Blocking {
            dialog,
            vtable,
            closing_latch,
        } = startup_modal_blocking_state()
        {
            if waits % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: PHASE_MENU_BUILD startup modal still blocking dialog=0x{dialog:x} vt=0x{vtable:x} closing_latch={closing_latch} -- polling modal lifecycle, not a grace counter"
                ));
            }
            $pass_through(false);
            return;
        }
        // ZERO-INPUT d180 LOCATE (replaces the old simulated-input cursor nav, which wrote the
        // keystate bitmap inputmgr+0x90 to move the cursor onto Load-Game -- that is synthesized
        // input and VIOLATES the No-Compromises zero-input standard). SetState(2)->3->10 builds the
        // main-menu job tree; the Load-Game item d180 (a MenuWindowJob whose +0xa8 functor's
        // _Do_call chains to dialog_factory 0x14081ead0) is constructed into the tree at BUILD time,
        // so a pure-read recursive walk can surface it WITHOUT the pump ticking it and WITHOUT any
        // input. A user-driven capture (2026-06-17) pinned d180's functor object = {_Func_impl
        // vtable 0x142ac3ea8, captured $owner+0x138}; the factory reads [capture+8]=$owner+0x138 as
        // the dialog $owner. We walk the candidate holder roots and, on the first functor->factory
        // hit, latch the item into MENU_LOAD_GAME_ITEM so STAGE 2 drives the load. (The
        // cap_menu_item_update hook also sets it if d180 ever ticks; whichever fires first wins.)
        // Throttled; pure reads -> save-safe.
        const D180_ROOT_E0: usize = 0xe0;
        const D180_ROOT_130: usize = 0x130;
        const D180_ROOT_138: usize = 0x138;
        // d180's +0xa8 functor object = {_Func_impl vtable $base+0x2ac3ea8, capture[+8]=$owner+0x138}
        // (user-driven capture 2026-06-17) -- a strong fingerprint corroborating the functor->factory
        // classification.
        const MENU_ITEM_LOADGAME_FUNCTOR_VTABLE_RVA: usize =
            ProfileLoadMenuRva::MenuLoadGameFunctorVtable as usize;
        // The `!own_stepper_passive_enabled() && !input_probe_enabled() &&` terms that opened this
        // condition were both permanently `false`, so both negations were permanently `true`. Both
        // gates are deleted.
        if !live_dialog_enabled()
            && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
            && unsafe { title_scheduler_ready($owner, $base) }
        {
            // Walk the candidate roots; on the first functor->dialog_factory hit (= the Load-Game
            // item d180), validate its fingerprint and LATCH it into MENU_LOAD_GAME_ITEM. STAGE 2
            // then drives it via the NATIVE MenuWindowJob::Update 0x1407ad1c0 (which wires the ctx
            // item+0x10 from the descriptor item+0x58 before firing the functor -> NO synthetic
            // ctx, NO save write). The cap_menu_item_update hook also sets it if d180 ever ticks;
            // whichever fires first wins. Throttled; pure reads here (save-safe).
            const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
            const ITEM_CTX_10: usize = 0x10;
            const ITEM_RESULT_130: usize = 0x130;
            let verbose = OWN_STEPPER_TITLETOP_DUMPS
                .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
                < OWN_STEPPER_TITLETOP_DUMP_CAP;
            let roots = [D180_ROOT_E0, D180_ROOT_130, D180_ROOT_138];
            for &root in roots.iter() {
                if let Some(item) =
                    unsafe { diagnostic_job_tree_walk($owner, $base, root, "d180-locate", verbose) }
                {
                    let null = TITLE_OWNER_SCAN_START_ADDRESS;
                    let functor =
                        unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }.unwrap_or(null);
                    let fvt = if functor != null {
                        unsafe { safe_read_usize(functor) }.unwrap_or(null)
                    } else {
                        null
                    };
                    let fcap = if functor != null {
                        unsafe { safe_read_usize(functor + core::mem::size_of::<usize>()) }
                            .unwrap_or(null)
                    } else {
                        null
                    };
                    let ctx10 = unsafe { safe_read_usize(item + ITEM_CTX_10) }.unwrap_or(null);
                    let res130 = unsafe { safe_read_usize(item + ITEM_RESULT_130) }.unwrap_or(null);
                    MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "own_stepper: ZERO-INPUT d180 LOCATED item=0x{item:x} via $owner+0x{root:x} functor=0x{functor:x} fvt=0x{fvt:x}(want $base+0x{:x}) fcap=0x{fcap:x}(want $owner+0x138=0x{:x}) ctx10=0x{ctx10:x} result130=0x{res130:x} -- latched, STAGE2 will native-Update it",
                        MENU_ITEM_LOADGAME_FUNCTOR_VTABLE_RVA,
                        $owner.wrapping_add(D180_ROOT_138)
                    ));
                    break;
                }
            }
        }
        // STAGE 1d: open the main menu zero-input. SetState(2)->3->10 built the TitleTopDialog at
        // $owner+0xe0 (vt 0x142b26468). The dialog's native update 0x1409aac10 (ticked every frame
        // by $pass_through -> STEP_MenuJobWait) runs the intro FadeIn animation, transitions
        // FadeIn->Loop on anim-complete (NOT input), and on its NON-INPUT Loop-ready path
        // (0x1409aade8) calls the open-menu registrar 0x1409b24e0 ITSELF, which set_state's the
        // SM [dialog+0xa60] to "TextFadeOut" and registers Continue/Load(d180)/New-Game. So the
        // PRIMARY path is to do NOTHING and let the native update self-open the menu.
        //
        // The prior force-call was harmful (bd titletopdialog-loop-ready-gate-2026): firing the
        // registrar on bare flags>=2 fired from the FadeIn node (wrong state) AND set the latch
        // [dialog+0xa40]=1, which PERMANENTLY blocks the native non-input path (it needs latch==0).
        // So here we (a) READ-ONLY probe the live state by NAME via the game's own is_in_state
        // (FadeIn/Loop/TextFadeOut) + the latch, logging it; and (b) only as a FALLBACK self-fire
        // the registrar on the CORRECT gate -- is_in_state(Loop)==true && latch==0 -- which is
        // exactly the native path's own precondition (zero input, NO save write). If the native
        // path fires first (latch->1 in Loop) we simply observe the menu open.
        const MENU_JOB_HOLDER_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
        if MENU_ENTRIES_SEEN.load(Ordering::SeqCst) == MENU_ENTRIES_SEEN_NO {
            let dialog = unsafe { safe_read_usize($owner + MENU_JOB_HOLDER_E0) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let dialog_vt = if dialog != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(dialog) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            };
            // Only call into the dialog's FD4 state machine once $owner+0xe0 IS the TitleTopDialog.
            //
            // ZERO IS NOT A MATCH, and this comparison used to treat it as one. Both sides collapse
            // to 0 on failure: `safe_read_usize(dialog)` falls back to
            // `TITLE_OWNER_SCAN_START_ADDRESS`, which is literally `usize::MIN`, and
            // `game_data_addr` answers 0 when the running build has no verified mapping for the
            // RVA. So an unreadable dialog paired with a REFUSED vtable satisfied `0 == 0` and the
            // native calls below fired on an object nothing had identified. It never bit only
            // because TITLE_TOP_DIALOG_VTABLE_RVA happens to be mapped on 1.17
            // (0x2b26468 -> 0x2b294e8) -- guarded by luck, not by design.
            let title_top_dialog_vt = er_game_base::mem::game_data_addr($base, TITLE_TOP_DIALOG_VTABLE_RVA, "TITLE_TOP_DIALOG_VTABLE_RVA");
            if title_top_dialog_vt != TITLE_OWNER_SCAN_START_ADDRESS
                && dialog_vt == title_top_dialog_vt
            {
                // is_in_state receiver = the ADDRESS dialog+0xa60 (the embedded SM sub-object), per
                // the registrar's `add rcx,0xa60; call`. is_in_state(sm, desc) -> bool reads the
                // live state by name (no hand pointer-chase). Read-only / no side effects.
                let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
                // WAS BROKEN ON 1.17 UNTIL 2026-08-30: this was `transmute($base + RVA)`, a raw
                // `base + RVA` that never called `game_data_addr`/`game_rva`, so the 1.16.2 -> 1.17
                // translation table was never consulted and the refusal path could never fire. The
                // function MOVED -- docs/recon/rva-map-1162-to-1170.needed-verified.tsv maps
                // 0x749b20 -> 0x74a970 (IDENTICAL-WHOLE, 28 insns, both .pdata entries agree) --
                // and byte-checking both images confirms it: 1.16.2 @0x749b20 and 1.17 @0x74a970
                // are the same prologue `48 89 54 24 10 53 48 83 ec 20`, while 1.17 @0x749b20 is
                // `03 48 8b cb ff 10 90 eb 18`, MID-INSTRUCTION and not a function entry. The
                // vtable gate above does NOT save it, because that RVA resolves fine and the gate
                // therefore PASSES. Never `transmute` a raw `base + RVA` call target.
                //
                // The DESCRIPTOR arguments needed the same treatment for a different reason: they
                // are pointers the native function DEREFERENCES, and `game_data_addr` hands back 0
                // on a refusal, so a refused descriptor used to be passed to the game as a null
                // state-name pointer. Both halves now refuse instead, and the resolved address is
                // printed in the probe line below so `is_in_state=0x0` reads as a refusal rather
                // than as "the state machine is in none of the three states".
                let is_in_state_addr = er_game_base::mem::game_data_addr($base, TITLE_TOP_DIALOG_IS_IN_STATE_RVA, "TITLE_TOP_DIALOG_IS_IN_STATE_RVA");
                let in_state = |desc_rva: usize, what: &'static str| -> bool {
                    if is_in_state_addr == TITLE_OWNER_SCAN_START_ADDRESS {
                        return false;
                    }
                    let desc = er_game_base::mem::game_data_addr($base, desc_rva, what);
                    if desc == TITLE_OWNER_SCAN_START_ADDRESS {
                        return false;
                    }
                    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
                        unsafe { std::mem::transmute(is_in_state_addr) };
                    let answer = unsafe { is_in_state(sm, desc) };
                    answer != OWN_STEPPER_FALSE
                };
                let in_fadein = in_state(TITLE_STATE_DESC_FADEIN_RVA, "TITLE_STATE_DESC_FADEIN_RVA");
                let in_loop = in_state(TITLE_STATE_DESC_LOOP_RVA, "TITLE_STATE_DESC_LOOP_RVA");
                let in_textfadeout = in_state(
                    TITLE_STATE_DESC_TEXTFADEOUT_RVA,
                    "TITLE_STATE_DESC_TEXTFADEOUT_RVA",
                );
                let latch =
                    unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
                        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
                        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if waits % STAGE1D_RETRY_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                    append_autoload_debug(format_args!(
                        "own_stepper: STAGE1d probe dialog=0x{dialog:x} sm=0x{sm:x} is_in_state=0x{is_in_state_addr:x} fadein={in_fadein} loop={in_loop} textfadeout={in_textfadeout} latch={latch} waits={waits} (self-fire open-menu on Loop+latch-clear; is_in_state=0x0 means the address was REFUSED for this build, not that the SM is in no state)"
                    ));
                }
                // SELF-FIRE the open-menu registrar on the CORRECT gate (the native path's own
                // precondition: settled in Loop + latch clear). RUNTIME-PROVEN NECESSARY
                // (headless-load 2026-06-17): with the modal suppressed (online-disable), the
                // TitleTopDialog SM sits in Loop forever -- the Loop-ready predicate needs the
                // accept byte (input), which never comes headless (latch=0 for 3000 waits). So the
                // "native self-opens" assumption is FALSE for a clean offline boot; we must fire
                // 0x1409b24e0 ourselves (the zero-input-menu-open milestone proved this opens the
                // menu). Default ON now (no flag) since headless cannot rely on a button press;
                // gated to the correct state (in_loop, NOT FadeIn) + once + latch-clear so it can
                // neither corrupt the SM (titletopdialog-fadein-gate) nor double-fire.
                // `game_data_addr` answers 0 for a refusal, which is a safe address to fail a READ
                // at and a fatal one to jump to -- so the resolved address is bound and checked
                // before it becomes a function pointer, per the rule stated on `mem.rs`'s own
                // `game_data_addr`. It resolves on 1.17 today; the check is what keeps that a fact
                // rather than an assumption.
                let open_menu_addr = er_game_base::mem::game_data_addr($base, TITLE_TOP_DIALOG_OPEN_MENU_RVA, "TITLE_TOP_DIALOG_OPEN_MENU_RVA");
                // WHY `in_loop` IS NOT REQUIRED (2026-09-04). `TitleTopDialog::update` calls this
                // same registrar from TWO sites, and only the first is gated on the "Loop" anim
                // state. Read on 1.16.2 `FUN_1409aac10` (1.17 `FUN_1409abdb0`):
                //
                //   if (anim == "Loop" && dialog->a40 == 0) { ... accept ... OpenMenu(dialog); }
                //   LAB: if (FUN_140e85f50() && dialog->a40 == 0)   { ... OpenMenu(dialog); }
                //
                // The second path needs NO anim state -- only the a40 latch clear. Requiring Loop
                // here was therefore stricter than the game itself, and it is the reason the
                // post-switch title can never be opened: the warm-rebuilt TitleTopDialog does not
                // return to "Loop" (the hazard already noted at title_tick_cover.rs ~2562, where
                // the press-start SceneObjProxy comes back unbound), so the boot fires this once
                // and a switch never can. Measured: the accept byte is written correctly and
                // repeatedly post-switch -- it is translated fine for 1.17
                // (0x144589bdc -> 0x14458dc5c) -- and the title still sits at PRESS BUTTON for
                // 185s, because behind that gate NOTHING reads it (bd er-effects-rs-tkfb).
                //
                // FadeIn/TextFadeOut stay excluded: firing during the fade corrupts the state
                // machine (bd titletopdialog-fadein-gate), and those are exactly the two states
                // the native function bails on before either call site. The a40 latch stays
                // required, which is the one condition BOTH native paths share, and the one-shot
                // still holds -- it re-arms per switch at system_quit_repro_guards.rs:684.
                if !in_fadein
                    && !in_textfadeout
                    && latch == TITLE_OWNER_SCAN_START_ADDRESS
                    && open_menu_addr != TITLE_OWNER_SCAN_START_ADDRESS
                    && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) == OWN_STEPPER_MENU_OPENED_NO
                {
                    let open_menu: unsafe extern "system" fn(usize) =
                        unsafe { std::mem::transmute(open_menu_addr) };
                    unsafe { open_menu(dialog) };
                    OWN_STEPPER_MENU_OPENED.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    // Deterministic timing endpoint: the DLL has driven boot -> modal-skip ->
                    // past press-any-button -> a READY main menu with ZERO input. ms-from-T0 here
                    // is the headless boot-to-menu time (the part vanilla needs >=3 human inputs +
                    // an online-attempt timeout to reach).
                    timeline_event(
                        "T_menu_open",
                        n,
                        format_args!("dialog=0x{dialog:x} waits={waits}"),
                    );
                    append_autoload_debug(format_args!(
                        "own_stepper: STAGE1d self-fire open-menu 0x{open_menu_addr:x}(dialog=0x{dialog:x}) -- not-FadeIn/not-TextFadeOut + latch clear (the gate the native second call site uses; loop={in_loop}) waits={waits}"
                    ));
                }
            }
        }
        // Three post-menu-open experiments used to sit here, each behind a gate that could only
        // return a literal `false`, so none of them ever ran on any build. All three gates have
        // since been deleted too:
        //
        //   * DETERMINISTIC INPUT PROBE (`input_probe_enabled`) -- drove a frame-precise
        //     Down->Confirm through `menu_input_probe` as a measurement oracle;
        //   * INJECT-NAV capture (`inject_nav_enabled`) -- stamped DIK_DOWN into the DInput block
        //     via `InputBlocker::set_injected_key` to self-drive the cursor, DOWN only;
        //   * DIRECT BUILD (`direct_build_enabled`) -- built the ProfileLoadDialog straight from
        //     factory 0x14081ead0 through `own_stepper_direct_build`, bypassing the input-gated
        //     row controller.
        //
        // All three returned early, so deleting them cannot change which path runs: control has
        // always reached the safe read-only default below.
        //
        // SAFE DEFAULT (RTTI-corrected, 2026-06-17). The "title-confirm" menu-drive that used to sit
        // below was built on a MISIDENTIFIED function: 0x14078e1c0 is CommandSelectDialog::Update (an
        // in-game dialog), NOT the TitleTopDialog ($owner+0xe0, RTTI vt 0x142b26468) confirm router, so
        // its cursor [+0xb0c] / rows [+0x1290] offsets do not apply here (bd rtti-correction-...). It
        // was demoted behind legacy_menu_drive_enabled() and is now deleted. A plain own_stepper run
        // reaches the open menu zero-input and STAYS there (no fire, no SetState, save-safe). The real
        // headless Load path is the own-the-stepper / session-activation route, not driving these
        // fake-menu steppers.
        // The `&& !own_stepper_passive_enabled() && !input_probe_enabled() && !inject_nav_enabled()`
        // terms that followed were all permanently-`false` gates (now deleted), so all three
        // negations were permanently `true` and the menu-open check alone decided this branch.
        if OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO {
            if OWN_STEPPER_TITLE_FIRED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
                == TITLE_OWNER_SCAN_START_ADDRESS
            {
                append_autoload_debug(format_args!(
                    "own_stepper: menu open zero-input; disproven title-confirm menu-drive is gated OFF (RTTI-corrected) -- STAY at open menu (NO-WRITE). Set er-quickload-legacy-disproven-menu-drive.txt to revisit the dead path."
                ));
            }
            // 2026-06-18 RECON-ONLY fingerprint scan for the Load-Game entry, run HERE (the open-menu
            // park is where a plain own_stepper run actually lives -- the dump block further down is
            // unreachable behind this early return). Result discarded -> no latch into
            // MENU_LOAD_GAME_ITEM, no STAGE2 advance -> stays NO-WRITE. Dedicated cap/interval so it
            // logs a handful of times across the ~20s post-open window without spamming.
            if OWN_STEPPER_LOADGAME_SCANS.load(Ordering::SeqCst) < OWN_STEPPER_LOADGAME_SCAN_CAP
                && (waits % STAGE1D_RETRY_INTERVAL) == TITLE_OWNER_SCAN_START_ADDRESS as u64
            {
                OWN_STEPPER_LOADGAME_SCANS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                let _ = unsafe { scan_dialog_for_loadgame($owner, $base) };
            }
            if menu_build_timed_out {
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            $pass_through(false);
            return;
        }
        // Wait for the registered entries to tick: the menu-item Update hook + Sequence-iterator
        // hook capture the Load-Game leaf (functor->dialog_factory) as the native pump ticks
        // them. Fallback: our static tree walk. NO SetState here -> stays at the main menu,
        // save-safe. STAGE 2 (invoke the leaf functor) follows once the live item is confirmed.
        // (REFUTED d180-locate path, retained only for the input-probe/inject-nav diagnostic modes.)
        let hooked = MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst);
        // The real Continue/Load-Game rows are TitleTopDialog entries (NOT FD4 jobs). Once the
        // menu is open, sample the dialog's entry vector a few times as it realizes -- save-safe
        // read-only enumeration that identifies the Load-Game/Continue entries for STAGE 2.
        if OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
            && OWN_STEPPER_TITLETOP_DUMPS.load(Ordering::SeqCst) < OWN_STEPPER_TITLETOP_DUMP_CAP
            && (waits % STAGE1D_RETRY_INTERVAL) == TITLE_OWNER_SCAN_START_ADDRESS as u64
        {
            OWN_STEPPER_TITLETOP_DUMPS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            let (tt_load, tt_cont, tt_cursor) = unsafe { dump_titletop_menu_entries($owner, $base) };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE1b titletop-entries load_game=0x{:x} continue=0x{:x} cursor={tt_cursor} (entries are dialog rows, not FD4 jobs)",
                tt_load.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
                tt_cont.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            ));
        }
        // Search BOTH the $owner+0x130 BeginLogo commit target (where the main-menu list with d180
        // actually lands, per the commit fn 0x140b0e530) AND $owner+0xe0 (the dialog holder).
        let found = if hooked != TITLE_OWNER_SCAN_START_ADDRESS {
            Some(hooked)
        } else {
            unsafe {
                diagnostic_job_tree_walk(
                    $owner,
                    $base,
                    TITLE_OWNER_MENU_LIST_130_OFFSET,
                    "list130",
                    false,
                )
            }
            .or_else(|| unsafe {
                diagnostic_job_tree_walk(
                    $owner,
                    $base,
                    TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                    "built-tree",
                    false,
                )
            })
        };
        match found {
            Some(item) => {
                let _ = unsafe { diagnostic_menu_walk($owner, $base, "built-138", true) };
                let _ = unsafe {
                    diagnostic_job_tree_walk(
                        $owner,
                        $base,
                        TITLE_OWNER_MENU_LIST_130_OFFSET,
                        "list130",
                        true,
                    )
                };
                let _ = unsafe {
                    diagnostic_job_tree_walk(
                        $owner,
                        $base,
                        TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                        "built-tree",
                        true,
                    )
                };
                // Ensure MENU_LOAD_GAME_ITEM is set (the item may have come from the static
                // tree walk rather than the leaf/iterator hook) so STAGE 2 reads it.
                if MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS {
                    MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
                }
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE1b LOAD-GAME item identified=0x{item:x} after {waits} waits -- entering STAGE 2 load drive (slot={want_slot}) c30=0x{c30:x} b80={b80}"
                ));
                timeline_event(
                    "T_menu_built",
                    n,
                    format_args!("item=0x{item:x} c30=0x{c30:x}"),
                );
                own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_INVOKE);
            }
            None => {
                // `&& !own_stepper_passive_enabled()` dropped: permanently-`false` gate (now
                // deleted), so the negation was permanently `true` and the timeout alone decided
                // this walk.
                if menu_build_timed_out {
                    let _ = unsafe { diagnostic_menu_walk($owner, $base, "built138-timeout", true) };
                    let _ = unsafe {
                        diagnostic_job_tree_walk(
                            $owner,
                            $base,
                            TITLE_OWNER_MENU_LIST_130_OFFSET,
                            "list130-timeout",
                            true,
                        )
                    };
                    let _ = unsafe {
                        diagnostic_job_tree_walk(
                            $owner,
                            $base,
                            TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                            "built-tree-timeout",
                            true,
                        )
                    };
                    append_autoload_debug(format_args!(
                        "own_stepper: STAGE1b menu-build TIMEOUT after {waits} polls/{menu_elapsed_ms}ms -- Load-Game item not found; staying at title (NO-WRITE)"
                    ));
                    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
                }
            }
        }
        $pass_through(false);
        return;
    }
    if $phase == OWN_STEPPER_PHASE_S2_INVOKE
        || $phase == OWN_STEPPER_PHASE_S2_ACTIVATE
        || $phase == OWN_STEPPER_PHASE_S2_MOUNT_POLL
        || $phase == OWN_STEPPER_PHASE_S2_CONFIRM
    {
        // STAGE 2: drive the verified menu load (functor -> dialog -> load_activate -> native
        // pump mounts c30=real+ac0+char -> continue_confirm -> SetState(5)). Pass-through each
        // frame so STEP_MenuJobWait keeps the native menu task ticking the registered selector.
        unsafe { own_stepper_stage2($owner, $base, $gm, $want_slot, n, $framectx) };
        $pass_through(false);
        return;
    }
    // $phase DONE: idx6 watches the native load; idx10 just passes through if re-entered.
    $pass_through(false);
    }};
}

pub(super) use own_stepper_idx10_fallbacks;
