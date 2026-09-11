use super::*;

/// Dismiss the captured startup MessageBoxDialog (connection-error / EULA / warning) by calling
/// its verified OnDecide/finalize 0x140927ba0(rcx=dialog) -- the genuine OK handler that
/// dispatches the chosen button (builder-defaulted to OK) and drives the dialog to emit "stop"
/// so the parent MenuWindowJob tears it down. Called each frame pre-in-world from the game task
/// (the menu/game thread, where OnDecide's input-registrar singleton access is valid) until the
/// closing latch [dialog+0x3b0]==1 or the dialog is freed/reused (vtable mismatch) -- both stop
/// the calls, avoiding re-dispatch / UAF. Fault-tolerant reads never AV.
pub(crate) fn force_dismiss_startup_dialog() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog = CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst);
    if dialog == null {
        return;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != null {
            own
        } else {
            game_module_base().unwrap_or(null)
        }
    };
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    if base == null || !is_startup_msgbox_vtable(vt, base) {
        // Dialog consumed/freed/reused -> stop (and let the builder hook re-capture a new one).
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        return;
    }
    // Stop once the dialog has begun teardown (EmitResult set the closing latch) -- calling
    // OnDecide again risks re-dispatch / UAF as the job frees it.
    let closing = unsafe { safe_read_usize(dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET) }
        .map(|v| v & MSGBOX_LATCH_BYTE_MASK)
        .unwrap_or(MSGBOX_CLOSING_YES);
    if closing == MSGBOX_CLOSING_YES {
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        let n = DISMISS_WRITE_LOG.load(Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "auto-accept: MessageBoxDialog 0x{dialog:x} closing (latch+0x3b0=1) after {n} OnDecide calls -- dismissed"
        ));
        return;
    }
    // Drive the dialog Decided + OK + fade-complete before the OK-handler so (a) the title-flow's
    // modal-build poll ([dialog+0x25e8]>0 at 0x1407b04f5) treats it as resolved and proceeds to the
    // menu, and (b) the OK-handler's fade gate (commit only when fade_current<=fade_target) fires this
    // frame -> instant commit/close, no fade-in render = no flash (vs the ~20 OnDecide frames before).
    // The dialog is vtable-validated above (base MessageBoxDialog or SaveRetryDialog). bd
    // press-any-button-golden-lever-job1e8-readiness-2026-06-23 + offline-title-modal-is-saveretrydialog.
    // Field semantics corrected 2026-07-28 (RE of the 1.16.2 ctor `FUN_1409275b0`; the writes
    // themselves are byte-for-byte unchanged so this deprecated path keeps behaving as it did):
    //   * `+0x25e8` is the button count, not a state. Writing 2 makes the multi-choice getter
    //     `1 < count` (0x1407b0cf0) -- which the title flow's modal poll at 0x1407b04f5 reads --
    //     report the box as resolved. It CORRUPTS the real count; acceptable only here, on a
    //     startup notice this path is about to force closed.
    //   * `+0x25e0` is the default cursor index. Writing 0 makes `OnDecide` dispatch button 0
    //     instead of taking its `index == -1` cancel arm.
    unsafe {
        *((dialog + MSGBOX_BUTTON_COUNT_25E8_OFFSET) as *mut i32) =
            MSGBOX_BUTTON_COUNT_MULTI_CHOICE;
        *((dialog + MSGBOX_DEFAULT_CURSOR_25E0_OFFSET) as *mut i32) = MSGBOX_FIRST_BUTTON_INDEX;
    }
    if let Some(fade_target_bits) =
        unsafe { safe_read_i32(dialog + MSGBOX_FADE_TARGET_2300_OFFSET) }
    {
        unsafe {
            *((dialog + MSGBOX_FADE_CURRENT_1278_OFFSET) as *mut i32) = fade_target_bits;
        }
    }
    // Proper OK (not force-stop): OnDecide 0x140927ba0 branches on the chosen button [dialog+0x25e0]
    // -- if == -1 it calls 0x14078dfd0 (the CANCEL/notify-closed path, which kicks the title flow
    // back to press-any-button); if != -1 it dispatches that button (= press OK -> proceed to the
    // main menu offline). The prior force-stop 0x14078dfd0 was exactly the cancel path, so the game
    // bounced back to press-any-button. Fix: set the chosen button to OK (index 0), then OnDecide.
    // Press OK every frame (runtime-confirmed: one-shot only highlights OK; the modal needs the
    // per-frame re-dispatch to progress its decide animation -> activate -> close -> proceed to
    // the main menu). [dialog+0x25e0]=0 selects OK so OnDecide takes the dispatch (not cancel) arm.
    // Call the real OK-button handler 0x14078e030(rcx=dialog) -- captured from a live OK-press.
    // It reads the dialog cursor, gets the OK callback, and commits (0x14078ef20) which actually
    // closes the dialog and emits its result so the title flow proceeds. This is what a real OK
    // does; OnDecide/field-writes/input-injection all failed to close it. Runs each frame on every
    // captured MessageBoxDialog -> skips all of them (connection-error, starting-offline, ...).
    let ok_handler: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(MSGBOX_OK_HANDLER_RVA, "MSGBOX_OK_HANDLER_RVA")
            {
                Some(address) => address,
                None => return,
            },
        )
    };
    unsafe { ok_handler(dialog) };
    let n = DISMISS_WRITE_LOG.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n % AUTO_ACCEPT_LOG_INTERVAL == null {
        append_autoload_debug(format_args!(
            "auto-accept: OK-handler 0x{:x}(MessageBoxDialog 0x{dialog:x}) -- real OK-press to close + proceed #{n}",
            er_game_base::mem::game_data_addr(base, MSGBOX_OK_HANDLER_RVA, "MSGBOX_OK_HANDLER_RVA")
        ));
    }
    let _ = (
        &LAST_ONDECIDE_DIALOG,
        MSGBOX_DEFAULT_CURSOR_25E0_OFFSET,
        MSGBOX_FIRST_BUTTON_INDEX,
        MSGBOX_CONFIRM_LATCH_1BC0_OFFSET,
        MSGBOX_CONFIRM_LATCH_SET,
        MSGBOX_ONDECIDE_RVA,
        INPUTMGR_BITMAP_90_OFFSET,
        MENU_EVENT_CONFIRM_3D,
        MENU_EVENT_PRESSED_BIT,
    );
}

/// Install the startup-popup capture hook once (minhook on the MessageBoxDialog builder
/// 0x1409275b0). The builder hook captures each created MessageBoxDialog into
/// CONNECTION_ERROR_DIALOG; `force_dismiss_startup_dialog` then dismisses it via OnDecide each
/// frame. Idempotent; safe to call every frame from the game task until it succeeds.
pub(crate) fn install_auto_accept_hook() {
    if AUTO_ACCEPT_INSTALLED.load(Ordering::SeqCst) != AUTO_ACCEPT_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "auto-accept: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(builder_addr) = game_rva_for_hook(MSGBOX_BUILDER_RVA) else {
        append_autoload_debug(format_args!("auto-accept: failed to resolve builder rva"));
        return;
    };
    match unsafe {
        MhHook::new(
            builder_addr as *mut c_void,
            msgbox_builder_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            MSGBOX_BUILDER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "auto-accept: queue_enable builder failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    AUTO_ACCEPT_INSTALLED.store(AUTO_ACCEPT_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "auto-accept: hooked MessageBoxDialog builder 0x{builder_addr:x} (capture -> OnDecide dismiss)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "auto-accept: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "auto-accept: MhHook::new builder failed: {status:?}"
        )),
    }
}

/// Diagnostic gate (GAME_DIR file `er-quickload-grsysmsg-log.txt` or `ER_QUICKLOAD_GRSYSMSG_LOG=1`):
/// arm the GR_System_Message id-logger so a probe can definitively name which message(s) the
/// menu-open MessageBoxDialogs carry (instead of guessing connection vs save). Reusable tool.
pub(crate) fn grsysmsg_log_enabled() -> bool {
    matches!(
        std::env::var("ER_QUICKLOAD_GRSYSMSG_LOG").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-quickload-grsysmsg-log.txt")
        .exists()
}

pub(crate) use er_telemetry_core::counters::GR_SYSMSG_LOG_COUNT;
pub(crate) use er_telemetry_core::counters::GR_SYSMSG_LOG_INSTALLED;
pub(crate) use er_telemetry_core::counters::GR_SYSMSG_LOG_ORIG;
/// `CS::GetGR_System_Message` (deobf entry 0x140762e30): `MenuString* (rcx=out, edx=int messageId)`.
/// The dump labels it 0x140762e40 but that is mid-instruction (inside `movq $-2,[rsp+0x28]`); the real
/// MSVC prologue (`mov [rsp+8],rcx; push rdi; sub rsp,0x30`) is at 0x140762e30 -- Verified by deobf
/// boundary disasm (prev fn ret+int3 at 0x140762e26/27, then this prologue). Body reads FMG repo
/// [0x143d7d4f8], applies the +0x384 variant, builds the MenuString.
// Corrected 2026-06-23 (corrupted-save-re-findings): 0x762e30 is GetTextEmbedImageName (it does
// id += 900, uses a different singleton) -- Not GetGR_System_Message. The real getter is deobf
// 0x140762d50 (dump 0x140762e40 - 0xf0 region shift): it loads L"GR_System_Message"+L"SM" and calls
// MsgRepository::GetAndFormat with the id in edx. Hooking the wrong fn is why the 401106 corrupted-
// save id was never seen (oracle stayed 0). This RVA must be the real getter for the semaphore.
pub(crate) const GR_SYSTEM_MESSAGE_RVA: u32 = er_game_base::rva::GR_SYSTEM_MESSAGE_RVA as u32;
pub(crate) const GR_SYSMSG_LOG_MAX: usize = 64;

/// Diagnostic detour for GetGR_System_Message 0x140762e40. Once the main menu has opened (skip the
/// boot-time message flood), log the integer message id (the `edx`/`rdx` arg) + first game caller RVA
/// for each call, capped. The id maps 1:1 to GR_System_Message_win64 (e.g. 4101 "Cannot connect to
/// network", 4102 "connection to game server lost", 4190 "network error", 70000 save-data notice,
/// 4191 "Failed to save game"), so the menu-open modals can be named without guessing. Read-only
/// passthrough; never mutates.
/// GR_System_Message ids the game fetches when it builds a "save data is corrupted" dialog (verified
/// from menu.msgbnd GR_System_Message_win64.fmg). 4191/4192/4193/401106 = "Failed to save game --
/// save data is corrupted"; 401721 = "Failed to load save data -- corrupted"; 401107 = "delete
/// corrupted data and create a new save?". Detecting any of these in GetGR_System_Message is the
/// memory-read semaphore for the corrupted-save popup (privacy-policy/char-presence-confirmed loop).
pub(crate) const CORRUPTED_SAVE_MSG_IDS: &[i32] = &[4191, 4192, 4193, 401106, 401107, 401721];
pub(crate) const CORRUPTED_SAVE_LOAD_FAILED_MSG_IDS: &[i32] = &[401721];
/// The corrupted-save message id last seen (0 = none). Exposed as `oracle_corrupted_save_seen_id`.
pub(crate) static CORRUPTED_SAVE_SEEN_ID: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);
/// More specific load-failure corrupted-save id last seen (currently 401721 only).
pub(crate) static CORRUPTED_SAVE_LOAD_FAILED_SEEN_ID: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);
pub(crate) static CORRUPTED_SAVE_SEEN_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry_core::counters::CORRUPTED_SAVE_SEEN_COUNT;

pub(crate) unsafe extern "system" fn gr_sysmsg_log_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    // Corrupted-save SEMAPHORE: always check (independent of the menu-open-gated logging below) so a
    // load probe records the corrupted-save popup as RAM-read telemetry, not just an on-screen image.
    let msg_id_now = (rdx & 0xffff_ffff) as i32;
    if CORRUPTED_SAVE_MSG_IDS.contains(&msg_id_now) {
        let caller_rva = trace_first_game_caller_rva();
        CORRUPTED_SAVE_SEEN_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
        CORRUPTED_SAVE_SEEN_COUNT.fetch_add(1, Ordering::SeqCst);
        if CORRUPTED_SAVE_LOAD_FAILED_MSG_IDS.contains(&msg_id_now) {
            CORRUPTED_SAVE_LOAD_FAILED_SEEN_ID.store(msg_id_now, Ordering::SeqCst);
        }
        if CORRUPTED_SAVE_SEEN_ID.swap(msg_id_now, Ordering::SeqCst) != msg_id_now {
            let kind = if CORRUPTED_SAVE_LOAD_FAILED_MSG_IDS.contains(&msg_id_now) {
                "load-failed-corrupted"
            } else {
                "save/write-corrupted"
            };
            append_autoload_debug(format_args!(
                "save-override: CORRUPTED-SAVE SEMAPHORE -- kind={kind} GetGR_System_Message id={msg_id_now} caller_rva=0x{caller_rva:x}; native text id says save data is corrupted"
            ));
        }
    }
    if TFC_AUTO_MENU_OPENED.load(Ordering::SeqCst) != 0 {
        let n = GR_SYSMSG_LOG_COUNT.fetch_add(1, Ordering::SeqCst);
        if n < GR_SYSMSG_LOG_MAX {
            let msg_id = (rdx & 0xffff_ffff) as i32;
            let caller_rva = trace_first_game_caller_rva();
            append_autoload_debug(format_args!(
                "grsysmsg #{n}: id={msg_id} caller_rva=0x{caller_rva:x} out=0x{rcx:x}"
            ));
        }
    }
    let orig = GR_SYSMSG_LOG_ORIG.load(Ordering::SeqCst);
    if orig == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(rcx, rdx, r8, r9) }
}

/// Install the GR_System_Message id-logger once (MinHook on 0x140762e40), mirroring the auto-accept
/// builder-hook precedent. Caller-gated by `grsysmsg_log_enabled()`.
pub(crate) fn install_gr_sysmsg_log_hook() {
    if GR_SYSMSG_LOG_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "grsysmsg-log: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva_for_hook(GR_SYSTEM_MESSAGE_RVA) else {
        append_autoload_debug(format_args!("grsysmsg-log: failed to resolve rva"));
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, gr_sysmsg_log_hook as *mut c_void) } {
        Ok(hook) => {
            GR_SYSMSG_LOG_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "grsysmsg-log: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "grsysmsg-log: hooked GetGR_System_Message 0x{addr:x} (log id+caller after menu-open)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "grsysmsg-log: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("grsysmsg-log: MhHook::new failed: {status:?}"))
        }
    }
}

/// CS::NetworkCheckJob::Run RVA (deobf entry 0x140821310). Signature
/// `MenuJobResult*(rcx=job, rdx=MenuJobResult* result, r8=FD4Time*)`. Entry prologue
/// (push rbp/rsi/rdi/r14/r15; lea rbp; sub rsp) is a clean MinHook target (disasm-verified).
pub(crate) const NETWORK_CHECK_JOB_RUN_RVA: u32 = 0x821310;
/// `FD4::FD4TimeTemplate<float>::vftable` (deobf 0x1429c8e48) -- the value Run's common-return path
/// writes to `*(param_3)` in every leaf (RVA read from the deobf disasm of the clean leaf).
pub(crate) const FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA: usize = 0x29c8e48;
/// `MenuJobState::Continue` (the no-modal result), verified from the deobf clean leaf (`lea edx,[r8+1]`).
pub(crate) const MENU_JOB_STATE_CONTINUE: i32 = 1;

/// pub(crate): the boot-progress view reads this as its menu-open-era milestone (the shortcircuit
/// fires within ~10ms of the title-accept-byte natural menu-open on the product path).
pub(crate) use er_telemetry_core::counters::NETWORK_CHECK_SHORTCIRCUIT_COUNT;
pub(crate) use er_telemetry_core::counters::NETWORK_CHECK_SHORTCIRCUIT_INSTALLED;

/// The milestone-3 fix (zero-input, save-safe). `CS::NetworkCheckJob::Run` is a title-flow MenuJob the
/// TitleTopDialog registrar chains unconditionally at menu-open. Offline, its Steam-holder check
/// (FUN_140cab320: all 3 holders field@0x10==2) and EOS check (FUN_140ddfb90) never pass, so every
/// decision-tree leaf builds a GR_System_Message MessageBoxDialog -- Except one leaf that does
/// `MenuJobResult::SetResult(Continue)` with no modal (decompile-verified). This detour replaces Run
/// with exactly that clean leaf, skipping the entire tree, so zero modals are ever enqueued regardless
/// of CSNetMan/CSCheatEOS readiness. The original is never called (its only outputs are the result +
/// the FD4Time vtable, both replicated). No input, no save write; only armed when offline is forced,
/// so it never alters an online (Seamless Co-op) network check. bd er-effects-rs-0ye.
pub(crate) unsafe extern "system" fn network_check_job_run_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let result = rdx;
    // Always exit to the no-modal Continue (offline modal suppression). This job is replaced either
    // way, so no real check / modal runs -> save-safe + online-safe.
    //
    // Regression fix (2026-06-30): a prior "PORTRAIT HOLD" held this job in a running state (>1, so
    // MenuJobResult::ShouldContinue keeps it polling) until the menu portrait was captured. That was
    // self-defeating: holding NetworkCheckJob stalls the title-flow check chain, so the save-data
    // ShowProgressJob (the boot ProfileSummary read) never runs -> the profile stays empty -> the
    // autoload starts a new game instead of loading the real character (and the stalled flow crashed
    // the world-load). The hold waited on a capture that could not happen until the read it was
    // blocking completed. Runtime-confirmed: with the hold gone, the boot read fires (showprog pass),
    // the real character loads, and the world reaches `player_present`. The portrait-capture timing is
    // owned downstream by `portrait_render_window` instead, which holds the load commit after menu-open
    // (i.e. After the boot read has populated the slot). bd autoload-regression-lookat-breaks-bootread-2026-06-30.
    let state = MENU_JOB_STATE_CONTINUE;
    // MenuJobResult::SetResult(result, state, 0): state @ +0 (i32), field1 @ +4 (i32). The native
    // SetResult 0x1407a91e0 only writes these two fields, so replicate inline. Readability-guarded.
    if result > null && unsafe { safe_read_usize(result) }.is_some() {
        unsafe {
            *(result as *mut i32) = state;
            *((result + 4) as *mut i32) = 0;
        }
    }
    // param_3->base._vfptr = FD4::FD4TimeTemplate<float>::vftable (Run's common-return sets this).
    if let Ok(base) = game_module_base()
        && r8 > null
        && unsafe { safe_read_usize(r8) }.is_some()
    {
        // Never store a refusal. `game_data_addr` answers 0 when the running build
        // moved this vftable and nothing verified where to, and a 0 here is not a
        // degraded value -- it is a NULL vptr in an object the engine will later
        // call through. Leaving the field as the native left it is strictly safer.
        let vftable = er_game_base::mem::game_data_addr(
            base,
            FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA,
            "FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA",
        );
        if vftable != null {
            unsafe { *(r8 as *mut usize) = vftable };
        }
    }
    if NETWORK_CHECK_SHORTCIRCUIT_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) == null {
        append_autoload_debug(format_args!(
            "network-check-shortcircuit: forced CS::NetworkCheckJob::Run -> MenuJobResult(Continue) result=0x{rdx:x} fd4time=0x{r8:x} -- no GR_System_Message modal enqueued (offline)"
        ));
    }
    let _ = (rcx, r9);
    result
}

/// Install the NetworkCheckJob::Run short-circuit once (MinHook on 0x140821310), mirroring the
/// auto-accept builder-hook precedent. Must arm before menu-open; caller-gated (offline only).
pub(crate) fn install_network_check_shortcircuit_hook() {
    if NETWORK_CHECK_SHORTCIRCUIT_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "network-check-shortcircuit: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva_for_hook(NETWORK_CHECK_JOB_RUN_RVA) else {
        append_autoload_debug(format_args!(
            "network-check-shortcircuit: failed to resolve rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            network_check_job_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "network-check-shortcircuit: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "network-check-shortcircuit: hooked CS::NetworkCheckJob::Run 0x{addr:x} -- offline modal suppression armed"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "network-check-shortcircuit: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "network-check-shortcircuit: MhHook::new failed: {status:?}"
        )),
    }
}

/// CS::ShowProgressJob::Run RVA (deobf entry 0x1408349c0; dump 0x140834ab0, region shift -0xf0,
/// clean prologue disasm-verified). Signature `MenuJobResult*(rcx=ShowProgressJob, rdx=MenuJobResult*
/// result, r8=FD4Time*)` -- Identical to NetworkCheckJob::Run.
pub(crate) const SHOW_PROGRESS_JOB_RUN_RVA: u32 = 0x8349c0;
/// `MenuJobState::Success` (=2; Continue=1). Verified from FUN_1407a7340's `SetResult(.,Success,0)`
/// clean leaf (deobf `lea edx,[r8+2]`). A passing check returns Success -> `ShouldContinue` (state>1)
/// true -> ShowProgressJob::Run propagates it -> flow advances (no modal). Forcing Continue(1) would
/// loop the timed job; Success(2) completes it cleanly.
pub(crate) const MENU_JOB_STATE_SUCCESS: i32 = 2;

pub(crate) use er_telemetry_core::counters::SHOW_PROGRESS_SHORTCIRCUIT_COUNT;
pub(crate) use er_telemetry_core::counters::SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED;
/// Original CS::ShowProgressJob::Run trampoline (MinHook). Needed so the save-data progressType can be
/// passed through to its real delegate -- that delegate is the boot ProfileSummary read (SLLoadSession
/// -> ER0000.sl2). Blanket-suppressing every type (the prior behavior) killed the save read, leaving
/// an empty profile -> Bandai privacy policy. bd boot-profile-read-STEP_InitMenu-blocked-by-showprogress-shortcircuit-2026-06-23.
pub(crate) static SHOW_PROGRESS_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// ShowProgressJob progressType at [job+0x18] (RE-confirmed). 10 = save-data check/load (must run its
/// delegate); 20=network, 30/31=sign-in, 60=login (offline-modal types we still short-circuit).
pub(crate) const SHOW_PROGRESS_TYPE_OFFSET: usize = 0x18;
pub(crate) const SHOW_PROGRESS_SAVE_TYPE: u32 =
    er_title_flow::boot_hold::SHOW_PROGRESS_SAVE_CHECK_TYPE;
pub(crate) use er_telemetry_core::counters::SHOW_PROGRESS_TYPE_LOGGED;

/// The milestone-3 fix, part 2 (zero-input, save-safe). `CS::ShowProgressJob::Run` (deobf 0x1408349c0)
/// is the shared Run for the offline title-flow check steps (save=10/network=20/sign-in=30,31/
/// login=60) the registrar chains at menu-open. Each runs a check delegate (job+0x20, slot +0x10);
/// offline the delegate returns an error result, which ShowProgressJob::Run propagates so the pump
/// enqueues a GR_System_Message MessageBox. The 3 observed menu-open modals all come from these
/// ShowProgressJobs (not NetworkCheckJob, which is a separate job already hooked). This detour replaces
/// Run with a passing-check exit: result = {state=Success, field1=0} (exactly what FUN_1407a7340's
/// SetResult(Success) clean leaf yields) + the FD4Time vtable, skipping the delegate -> the job
/// completes successfully, the flow advances, and zero modals are enqueued. One hook covers all the
/// check steps. Offline-gated (no effect on an online Seamless Co-op check). bd er-effects-rs-0ye.
/// Deterministic clean-title active-save-slot override for the System-Quit->Load-Profile switch.
///
/// The clean-title reload is the game's native most-recent Continue: the ShowProgressJob save-data
/// delegate (the boot ProfileSummary read) derives+selects the most-recent save slot and writes it to
/// the active-slot field GameMan+0xac0, and the reload deserializes 0xac0 immediately afterward. On a
/// switch that makes it re-load the original character (proven 2026-07-02: picked slot 4 'Speed Bean'
/// but ac0 re-derived to 5 -> loaded 'Patches'). Repointing ac0 to the picked slot on a per-tick poll
/// loses the race -- the derivation and the load happen inside one game-task tick, so the tick-set
/// landed after the load committed. Calling this right after the delegate (before the load) wins it
/// deterministically. Gated on a torn-down world (local player absent) so it only ever fires at the
/// clean-title reload, never while the old world is live -- where it would misdirect the return-title
/// quit-save to the picked slot. Save-safe: a pure active-slot write, no save-file mutation. See bd
/// system-quit-ac0-fix-insufficient-cleantitle-load-is-native-mostrecent-2026-07-02.
pub(crate) unsafe fn system_quit_repoint_active_slot_at_clean_title(source: &str) {
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        < SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
    {
        return;
    }
    let picked = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    if picked == usize::MAX {
        return;
    }
    let picked = picked as i32;
    if picked < 0 {
        return;
    }
    // Clean-title only: an old world still up means the return-title quit-save has not run yet, and
    // ac0 selects the slot it writes -- repointing now would corrupt (overwrite) the picked slot.
    if unsafe { PlayerIns::local_player_mut() }.is_ok() {
        return;
    }
    let Ok(_base) = game_module_base() else {
        return;
    };
    let gm = game_man_ptr_or_null();
    if gm == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let ac0_before = unsafe { safe_read_i32(gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    if ac0_before == picked {
        return;
    }
    let set_save_slot: unsafe extern "system" fn(i32) = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(
                FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
                "FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    unsafe { set_save_slot(picked) };
    let ac0_after = unsafe { safe_read_i32(gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    append_autoload_debug(format_args!(
        "system-quit-quickload: [{source}] DETERMINISTIC clean-title active-slot override ac0 {ac0_before}->{ac0_after} via set_save_slot({picked}) -- applied after the native most-recent derivation, before the reload deserialize, so the reload loads the PICKED slot"
    ));
}

pub(crate) unsafe extern "system" fn show_progress_job_run_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let result = rdx;
    // progressType ([job+0x18], low 32 bits). 10 = the save-data check/load: its delegate is the boot
    // ProfileSummary read, so it must run -- pass it through to the original. Suppressing it (as the
    // prior blanket short-circuit did) leaves the profile empty -> privacy policy, and the save is
    // never read. All other types (network/sign-in/login) still get the Success short-circuit so the
    // offline connection modals stay suppressed.
    let ptype = if rcx > null {
        unsafe { safe_read_usize(rcx + SHOW_PROGRESS_TYPE_OFFSET) }
            .map(|v| (v & 0xffff_ffff) as u32)
    } else {
        None
    };
    let raw10 = if rcx > null {
        unsafe { safe_read_usize(rcx + 0x10) }
    } else {
        None
    };
    let d = SHOW_PROGRESS_TYPE_LOGGED.fetch_add(1, Ordering::SeqCst);
    if d < 16 {
        append_autoload_debug(format_args!(
            "show-progress: progressType[+0x18]={ptype:?} field[+0x10]={raw10:x?} result=0x{rdx:x} (save_type={SHOW_PROGRESS_SAVE_TYPE})"
        ));
    }
    if ptype == Some(SHOW_PROGRESS_SAVE_TYPE) {
        // Missing-save HOLD: while no save has been selected, loop this save-data job with
        // continue every frame. This holds the title-flow FixOrderJobSequence open at the
        // save-check without freezing any thread -- the boot loading bar sticks at its SAVE_CHECK
        // marker, and the DLL-drawn overlay picker (`save_picker_overlay.rs`) composites on top.
        // The game task keeps ticking (so the overlay reads input) and Present keeps firing (so
        // the overlay + bar draw). Making the native title menu input-dead is irrelevant: the
        // picker is a DLL-drawn overlay with its own OS-read input, not a native menu. The pick
        // installs the save redirect and clears `missing_save_selection_pending()`, so the very
        // next frame this job passes through to the delegate with the redirected save present and
        // the boot resumes -- the bar advances past SAVE_CHECK and the overlay disarms.
        if er_title_flow::boot_hold::should_hold_save_check(ptype, missing_save_selection_pending())
        {
            if result > null && unsafe { safe_read_usize(result) }.is_some() {
                unsafe {
                    *(result as *mut i32) = MENU_JOB_STATE_CONTINUE;
                    *((result + 4) as *mut i32) = 0;
                }
            }
            if let Ok(base) = game_module_base()
                && r8 > null
                && unsafe { safe_read_usize(r8) }.is_some()
            {
                // Never store a refusal. `game_data_addr` answers 0 when the running build
                // moved this vftable and nothing verified where to, and a 0 here is not a
                // degraded value -- it is a NULL vptr in an object the engine will later
                // call through. Leaving the field as the native left it is strictly safer.
                let vftable = er_game_base::mem::game_data_addr(
                    base,
                    FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA,
                    "FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA",
                );
                if vftable != null {
                    unsafe { *(r8 as *mut usize) = vftable };
                }
            }
            if d < 16 || d.is_power_of_two() {
                append_autoload_debug(format_args!(
                    "show-progress: HOLD save-data progressType {SHOW_PROGRESS_SAVE_TYPE} (CONTINUE) -- overlay save picker pending; boot bar held at SAVE_CHECK"
                ));
            }
            let _ = (rcx, r9);
            return result;
        }
        let orig = SHOW_PROGRESS_ORIG.load(Ordering::SeqCst);
        if orig != HOOK_ORIGINAL_UNSET {
            if d < 16 {
                append_autoload_debug(format_args!(
                    "show-progress: PASS-THROUGH save-data progressType {SHOW_PROGRESS_SAVE_TYPE} -> original delegate (boot ProfileSummary read fires)"
                ));
            }
            let call: unsafe extern "system" fn(usize, usize, usize, usize) -> usize = unsafe {
                std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize, usize) -> usize,
                >(orig)
            };
            let ret = unsafe { call(rcx, rdx, r8, r9) };
            // The delegate above just selected the most-recent save slot into GameMan+0xac0. On a
            // System-Quit->Load-Profile switch the reload deserializes 0xac0 next, so override it to
            // the picked slot here -- after the native derivation, before the load. Deterministic, no
            // tick-race. No-ops off the switch path / while the old world is up (see the helper).
            unsafe { system_quit_repoint_active_slot_at_clean_title("show-progress-delegate") };
            return ret;
        }
    }
    if result > null && unsafe { safe_read_usize(result) }.is_some() {
        unsafe {
            *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
            *((result + 4) as *mut i32) = 0;
        }
    }
    if let Ok(base) = game_module_base()
        && r8 > null
        && unsafe { safe_read_usize(r8) }.is_some()
    {
        // Never store a refusal. `game_data_addr` answers 0 when the running build
        // moved this vftable and nothing verified where to, and a 0 here is not a
        // degraded value -- it is a NULL vptr in an object the engine will later
        // call through. Leaving the field as the native left it is strictly safer.
        let vftable = er_game_base::mem::game_data_addr(
            base,
            FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA,
            "FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA",
        );
        if vftable != null {
            unsafe { *(r8 as *mut usize) = vftable };
        }
    }
    if SHOW_PROGRESS_SHORTCIRCUIT_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) == null {
        append_autoload_debug(format_args!(
            "show-progress-shortcircuit: forced CS::ShowProgressJob::Run -> MenuJobResult(Success) result=0x{rdx:x} fd4time=0x{r8:x} -- offline title-flow check modal(s) suppressed at the shared chokepoint"
        ));
    }
    let _ = (rcx, r9);
    result
}

/// Install the ShowProgressJob::Run short-circuit once (MinHook on 0x1408349c0). Must arm before
/// menu-open; caller-gated (offline only).
pub(crate) fn install_show_progress_shortcircuit_hook() {
    if SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "show-progress-shortcircuit: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva_for_hook(SHOW_PROGRESS_JOB_RUN_RVA) else {
        append_autoload_debug(format_args!(
            "show-progress-shortcircuit: failed to resolve rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            show_progress_job_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            // Store the trampoline before enabling so the save-data progressType can be passed through
            // to the original delegate (the boot ProfileSummary read).
            SHOW_PROGRESS_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "show-progress-shortcircuit: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "show-progress-shortcircuit: hooked CS::ShowProgressJob::Run 0x{addr:x} -- save-type passthrough + offline-check modal suppression armed"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "show-progress-shortcircuit: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "show-progress-shortcircuit: MhHook::new failed: {status:?}"
        )),
    }
}

// ---- Missing-save picker: hold the title at press-any-button until the pick ----
// The native title auto-opens its menu ~25-38s into a parked press-any-button boot (the online->
// offline sign-in flow timing out chains the menu-open check steps). If that happens before the user
// picks a save, the Continue/Load rows build against an empty ProfileSummary (no save yet) -> disabled
// rows -> no character ever loads and the boot idles forever on a null pump (softlock on a late pick;
// bd er-effects-rs-ns4n follow-up). The save-check ShowProgressJob hold is too late -- it holds the
// bar after menu-open. This detour suppresses `TitleTopDialog::open_menu` while the picker is pending,
// so the menu is only ever built after the pick installs the redirect (save present -> rows enabled),
// where the normal post-pick accept-byte flow opens it fresh -- identical to the working early-pick
// path, regardless of how long the user waits. Self-gates on `missing_save_selection_pending()`, so it
// is a pure pass-through on an early pick and on any run without the missing-save picker armed.
pub(crate) use er_telemetry_core::counters::TITLE_OPEN_MENU_SUPPRESS_INSTALLED;
pub(crate) static TITLE_OPEN_MENU_SUPPRESS_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry_core::counters::TITLE_OPEN_MENU_SUPPRESSED_COUNT;
pub(crate) use er_telemetry_core::counters::{
    TITLE_OPEN_MENU_PASSTHROUGH_AFTER_SUPPRESS_COUNT, TITLE_OPEN_MENU_PASSTHROUGH_COUNT,
};

pub(crate) unsafe extern "system" fn title_open_menu_suppress_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    if er_title_flow::boot_hold::should_suppress_title_open_menu(missing_save_selection_pending()) {
        let n = TITLE_OPEN_MENU_SUPPRESSED_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        // Log the first suppression and then sparsely (power-of-two) so a repro shows the hold firing
        // without flooding the log across the whole pending window.
        if n == 1 || n.is_power_of_two() {
            append_autoload_debug(format_args!(
                "title-open-menu: SUPPRESSED native open_menu #{n} while missing-save picker pending (dialog=0x{rcx:x}) -- menu must build post-pick with the save present"
            ));
        }
        return 0;
    }
    // Log the pass-through too. Only the dropped calls used to be recorded, which made "did the
    // native title ever open its menu again after the hold released" unanswerable from the log --
    // the ambiguity that cost the 2026-08-26 softlock its diagnosis. A pass-through counted after a
    // suppression is the decisive one: it proves the title re-issues `open_menu` on its own, so
    // dropping a request is a deferral rather than a loss.
    let suppressed = TITLE_OPEN_MENU_SUPPRESSED_COUNT.load(Ordering::SeqCst);
    let n = TITLE_OPEN_MENU_PASSTHROUGH_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    // Observe the a40 edge as an event, not by sampling (bd er-effects-rs-1742).
    //
    // `OWN_STEPPER_MENU_OPENED` used to be latched in one place: `product_core_autoload_tick`, which
    // sets it only on a tick that happens to read `dialog+0xa40 == 1`. But a40 is set inside
    // `open_menu` and back to 0 by the follow-up `TitleTopDialog::update` -- the comment beside that
    // latch says so -- so the window can be shorter than one game-task tick at ~28fps. Miss it and
    // the tick re-arms the accept byte instead, which restarts the menu transition, which closes the
    // window again: the title loops forever at press-any-button and the semantic Continue row is
    // never driven. Measured 2026-09-05 on the autoload path: core readiness `ready` with
    // ready_successes climbing past 400, phase pinned at menu, `title_open_menu_passthrough_count=1`
    // (open_menu did run) and `menu_opened_latch=0` -- the edge happened and nobody saw it.
    //
    // This detour cannot miss it. It is the call, so a pass-through is the edge, observed from
    // inside the native frame with no sampling window at all. Only pass-throughs latch: a suppressed
    // call is one we dropped while the missing-save picker is pending, and the menu genuinely has
    // not opened then, so the picker path is untouched.
    if OWN_STEPPER_MENU_OPENED
        .compare_exchange(
            OWN_STEPPER_MENU_OPENED_NO,
            OWN_STEPPER_CALL_INC,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
    {
        append_autoload_debug(format_args!(
            "title-open-menu: LATCHED menu-opened from the native open_menu pass-through (dialog=0x{rcx:x}) -- the a40 edge as an event, so a game-task tick that misses the transient a40 window no longer re-arms the accept byte forever"
        ));
    }
    let after = if suppressed > 0 {
        Some(TITLE_OPEN_MENU_PASSTHROUGH_AFTER_SUPPRESS_COUNT.fetch_add(1, Ordering::SeqCst) + 1)
    } else {
        None
    };
    if n == 1 || n.is_power_of_two() || after == Some(1) {
        append_autoload_debug(format_args!(
            "title-open-menu: PASS-THROUGH native open_menu #{n} (dialog=0x{rcx:x}) suppressed_so_far={suppressed} after_suppress={} -- the native title is building its menu now",
            after.unwrap_or(0)
        ));
    }
    let orig = TITLE_OPEN_MENU_SUPPRESS_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let call: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { call(rcx, rdx, r8, r9) }
}

/// Install the `TitleTopDialog::open_menu` suppression detour once (MinHook on 0x1409b24e0). Must arm
/// before the native auto-menu-open (~+38s). Harmless when no picker is pending (pass-through).
pub(crate) fn install_title_open_menu_suppress_hook() {
    if TITLE_OPEN_MENU_SUPPRESS_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-open-menu-suppress: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!(
            "title-open-menu-suppress: failed to resolve module base"
        ));
        return;
    };
    let addr = base + TITLE_TOP_DIALOG_OPEN_MENU_RVA;
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_open_menu_suppress_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_OPEN_MENU_SUPPRESS_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-open-menu-suppress: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "title-open-menu-suppress: hooked CS::TitleTopDialog::open_menu 0x{addr:x} -- native menu-open held while missing-save picker pending (rows build post-pick with the save present)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-open-menu-suppress: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-open-menu-suppress: MhHook::new failed: {status:?}"
        )),
    }
}

/// Latch detour for the CS::SceneObjProxy ctor 0x14074a700 (rcx=proxy[this], rdx=MenuWindow*,
/// r8/r9 forwarded). Disasm-verified: the ctor does `mov %rdx,%rbx` (0x14074a720) then
/// `mov %rbx,0x20(%rsi)` (0x14074a735) -- so the incoming RDX is the engine-verified MenuWindow it
/// stores at proxy+0x20 (probe-6 proved the old TitleTopDialog-factory rdx was a std::function
/// delegate, not the MenuWindow). Runtime showed the old MenuWindow/MenuWindowProxy vtable constants
/// are stale for this ctor's engine-provided rdx, but static disassembly still proves the game stores
/// rdx as proxy+0x20. Treat the engine-provided heap-aligned rdx as the trust boundary and OVERWRITE
/// LATCHED_MENU_WINDOW on every valid call (most-recent live host window wins -- the title's host
/// window is latched by the time STAGE2 runs). Then pure passthrough: call the original trampoline
/// with all args preserved + return its result, never perturbing the build.
/// bd live-dialog-probe6-factory-fires-returns-dialog-rdx-not-menuwindow-2026.
pub(crate) unsafe extern "system" fn scene_obj_proxy_ctor_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    const CANDIDATE_ALIGNED: usize = 0;
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    const SCENE_OBJ_PROXY_CTOR_LOG_MAX: usize = 32;
    const SCENE_OBJ_PROXY_CTOR_HIT_INC: usize = 1;
    pub(crate) use er_telemetry_core::counters::SCENE_OBJ_PROXY_CTOR_HITS;

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let menu_window = rdx;
    let hit = SCENE_OBJ_PROXY_CTOR_HITS.fetch_add(SCENE_OBJ_PROXY_CTOR_HIT_INC, Ordering::SeqCst);
    let pvt = unsafe { safe_read_usize(menu_window) }.unwrap_or(null);
    if menu_window != null
        && menu_window >= HEAP_LO
        && (menu_window & PTR_ALIGN_MASK) == CANDIDATE_ALIGNED
    {
        LATCHED_MENU_WINDOW.store(menu_window, Ordering::SeqCst);
        if hit < SCENE_OBJ_PROXY_CTOR_LOG_MAX {
            append_autoload_debug(format_args!(
                "menuwindow-latch: 0x14074a700 ACCEPT #{hit} rdx=0x{menu_window:x} first=0x{pvt:x} (engine-stored proxy+0x20 candidate)"
            ));
        }
    } else if hit < SCENE_OBJ_PROXY_CTOR_LOG_MAX {
        append_autoload_debug(format_args!(
            "menuwindow-latch: 0x14074a700 REJECT #{hit} rdx=0x{menu_window:x} first=0x{pvt:x} (not heap-aligned)"
        ));
    }
    let orig = SCENE_OBJ_PROXY_CTOR_ORIG.load(Ordering::SeqCst);
    if orig == null {
        return null;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(rcx, rdx, r8, r9) }
}

#[allow(dead_code)] // Retained: Title-cover part B builder: the native wrapper RVA + telemetry wiring it encodes is the retained result, currently unwired.
pub(crate) unsafe fn build_profile_select_cover_job(
    base: usize,
    rdx: usize,
    r8: usize,
    caller_rva: usize,
    source: &str,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == null || base == 0 {
        return;
    }
    let mut cover_slot = null;
    let cover_builder: unsafe extern "system" fn(usize, usize, usize) -> usize = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(
                TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA,
                "TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    let cover_ret = unsafe { cover_builder((&raw mut cover_slot) as usize, rdx, r8) };
    let cover_job = cover_slot;
    TITLE_CUSTOM_COVER_PROFILE_SELECT_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_RET.store(cover_ret, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_JOB.store(cover_job, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-b: BUILT non-returned custom cover {TITLE_CUSTOM_COVER_PROFILE_SELECT_NAME} via 0x{:x} from {source} -> ret=0x{cover_ret:x} job=0x{cover_job:x}; dummy={TITLE_CUSTOM_COVER_DUMMY_PROFILE_SYMBOL} target={TITLE_CUSTOM_COVER_SYSTEX_TARGET} renderer={TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS}",
        er_game_base::mem::game_data_addr(
            base,
            TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA,
            "TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA"
        ),
    ));
}

#[allow(dead_code)] // Retained: Title-cover part B builder: the native wrapper RVA + telemetry wiring it encodes is the retained result, currently unwired.
pub(crate) unsafe fn build_black_cover_job(
    base: usize,
    rdx: usize,
    caller_rva: usize,
    source: &str,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == null || base == 0 {
        return;
    }
    if TITLE_CUSTOM_COVER_BLACK_BUILDS.load(Ordering::SeqCst) != 0 {
        return;
    }
    let mut cover_slot = null;
    let cover_builder: unsafe extern "system" fn(usize, usize) -> usize = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(
                TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA,
                "TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    let cover_ret = unsafe { cover_builder((&raw mut cover_slot) as usize, rdx) };
    let cover_job = cover_slot;
    TITLE_CUSTOM_COVER_BLACK_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_BLACK_LAST_RET.store(cover_ret, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_BLACK_LAST_JOB.store(cover_job, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_BLACK_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-b: BUILT non-returned custom black cover {TITLE_CUSTOM_COVER_BLACK_NAME} via 0x{:x} from {source} -> ret=0x{cover_ret:x} job=0x{cover_job:x}; will be pumped above native title/PAB jobs",
        er_game_base::mem::game_data_addr(
            base,
            TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA,
            "TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA"
        ),
    ));
}
