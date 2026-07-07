
/// Dismiss the captured startup MessageBoxDialog (connection-error / EULA / warning) by calling
/// its verified OnDecide/finalize 0x140927ba0(rcx=dialog) -- the genuine OK handler that
/// dispatches the chosen button (builder-defaulted to OK) and drives the dialog to emit "stop"
/// so the parent MenuWindowJob tears it down. Called each frame pre-in-world from the game task
/// (the menu/game thread, where OnDecide's input-registrar singleton access is valid) UNTIL the
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
    // Drive the dialog Decided + OK + fade-complete BEFORE the OK-handler so (a) the title-flow's
    // modal-build poll ([dialog+0x25e8]>0 at 0x1407b04f5) treats it as resolved and PROCEEDS to the
    // menu, and (b) the OK-handler's fade gate (commit only when fade_current<=fade_target) fires THIS
    // frame -> instant commit/close, no fade-in render = no flash (vs the ~20 OnDecide frames before).
    // The dialog is vtable-validated above (base MessageBoxDialog OR SaveRetryDialog). bd
    // press-any-button-golden-lever-job1e8-readiness-2026-06-23 + offline-title-modal-is-saveretrydialog.
    unsafe {
        *((dialog + MSGBOX_STATE_25E8_OFFSET) as *mut i32) = MSGBOX_STATE_DECIDED;
        *((dialog + MSGBOX_RESULT_BUTTON_25E0_OFFSET) as *mut i32) = MSGBOX_OK_BUTTON;
    }
    if let Some(fade_target_bits) =
        unsafe { safe_read_i32(dialog + MSGBOX_FADE_TARGET_2300_OFFSET) }
    {
        unsafe {
            *((dialog + MSGBOX_FADE_CURRENT_1278_OFFSET) as *mut i32) = fade_target_bits;
        }
    }
    // PROPER OK (NOT force-stop): OnDecide 0x140927ba0 branches on the chosen button [dialog+0x25e0]
    // -- if == -1 it calls 0x14078dfd0 (the CANCEL/notify-closed path, which kicks the title flow
    // BACK to PRESS-ANY-BUTTON); if != -1 it DISPATCHES that button (= press OK -> proceed to the
    // main menu offline). The prior force-stop 0x14078dfd0 was exactly the cancel path, so the game
    // bounced back to press-any-button. Fix: set the chosen button to OK (index 0), then OnDecide.
    // Press OK EVERY FRAME (runtime-confirmed: one-shot only HIGHLIGHTS OK; the modal needs the
    // per-frame re-dispatch to progress its decide animation -> activate -> close -> proceed to
    // the main menu). [dialog+0x25e0]=0 selects OK so OnDecide takes the dispatch (NOT cancel) arm.
    // Call THE REAL OK-BUTTON HANDLER 0x14078e030(rcx=dialog) -- captured from a live OK-press.
    // It reads the dialog cursor, gets the OK callback, and COMMITS (0x14078ef20) which actually
    // CLOSES the dialog and emits its result so the title flow PROCEEDS. This is what a real OK
    // does; OnDecide/field-writes/input-injection all failed to close it. Runs each frame on every
    // captured MessageBoxDialog -> skips ALL of them (connection-error, starting-offline, ...).
    let ok_handler: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + MSGBOX_OK_HANDLER_RVA) };
    unsafe { ok_handler(dialog) };
    let n = DISMISS_WRITE_LOG.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n % AUTO_ACCEPT_LOG_INTERVAL == null {
        append_autoload_debug(format_args!(
            "auto-accept: OK-handler 0x{:x}(MessageBoxDialog 0x{dialog:x}) -- real OK-press to close + proceed #{n}",
            base + MSGBOX_OK_HANDLER_RVA
        ));
    }
    let _ = (
        &LAST_ONDECIDE_DIALOG,
        MSGBOX_RESULT_BUTTON_25E0_OFFSET,
        MSGBOX_OK_BUTTON,
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
    let Ok(builder_addr) = game_rva(MSGBOX_BUILDER_RVA) else {
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
                    std::mem::forget(hook);
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

/// Diagnostic gate (GAME_DIR file `er-effects-grsysmsg-log.txt` or `ER_EFFECTS_GRSYSMSG_LOG=1`):
/// arm the GR_System_Message id-logger so a probe can DEFINITIVELY name which message(s) the
/// menu-open MessageBoxDialogs carry (instead of guessing connection vs save). Reusable tool.
pub(crate) fn grsysmsg_log_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_GRSYSMSG_LOG").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-grsysmsg-log.txt")
            .exists()
}

static GR_SYSMSG_LOG_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static GR_SYSMSG_LOG_ORIG: AtomicUsize = AtomicUsize::new(0);
static GR_SYSMSG_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `CS::GetGR_System_Message` (deobf entry 0x140762e30): `MenuString* (rcx=out, edx=int messageId)`.
/// The dump labels it 0x140762e40 but that is MID-INSTRUCTION (inside `movq $-2,[rsp+0x28]`); the real
/// MSVC prologue (`mov [rsp+8],rcx; push rdi; sub rsp,0x30`) is at 0x140762e30 -- VERIFIED by deobf
/// boundary disasm (prev fn ret+int3 at 0x140762e26/27, then this prologue). Body reads FMG repo
/// [0x143d7d4f8], applies the +0x384 variant, builds the MenuString.
// CORRECTED 2026-06-23 (corrupted-save-re-findings): 0x762e30 is GetTextEmbedImageName (it does
// id += 900, uses a different singleton) -- NOT GetGR_System_Message. The real getter is deobf
// 0x140762d50 (dump 0x140762e40 - 0xf0 region shift): it loads L"GR_System_Message"+L"SM" and calls
// MsgRepository::GetAndFormat with the id in edx. Hooking the WRONG fn is why the 401106 corrupted-
// save id was never seen (oracle stayed 0). This RVA must be the real getter for the semaphore.
const GR_SYSTEM_MESSAGE_RVA: u32 = 0x762d50;
const GR_SYSMSG_LOG_MAX: usize = 64;

/// DIAGNOSTIC detour for GetGR_System_Message 0x140762e40. Once the main menu has opened (skip the
/// boot-time message flood), log the integer message id (the `edx`/`rdx` arg) + first game caller RVA
/// for each call, capped. The id maps 1:1 to GR_System_Message_win64 (e.g. 4101 "Cannot connect to
/// network", 4102 "connection to game server lost", 4190 "network error", 70000 save-data notice,
/// 4191 "Failed to save game"), so the menu-open modals can be named without guessing. Read-only
/// passthrough; never mutates.
/// GR_System_Message ids the game fetches when it builds a "save data is corrupted" dialog (verified
/// from menu.msgbnd GR_System_Message_win64.fmg). 4191/4192/4193/401106 = "Failed to save game --
/// save data is corrupted"; 401721 = "Failed to load save data -- corrupted"; 401107 = "delete
/// corrupted data and create a new save?". Detecting any of these in GetGR_System_Message IS the
/// memory-read semaphore for the corrupted-save popup (privacy-policy/char-presence-CONFIRMED loop).
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
pub(crate) static CORRUPTED_SAVE_SEEN_COUNT: AtomicUsize = AtomicUsize::new(0);

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
    let Ok(addr) = game_rva(GR_SYSTEM_MESSAGE_RVA) else {
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
                    std::mem::forget(hook);
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
const NETWORK_CHECK_JOB_RUN_RVA: u32 = 0x821310;
/// `FD4::FD4TimeTemplate<float>::vftable` (deobf 0x1429c8e48) -- the value Run's common-return path
/// writes to `*(param_3)` in every leaf (RVA read from the deobf disasm of the clean leaf).
const FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA: usize = 0x29c8e48;
/// `MenuJobState::Continue` (the no-modal result), verified from the deobf clean leaf (`lea edx,[r8+1]`).
const MENU_JOB_STATE_CONTINUE: i32 = 1;

static NETWORK_CHECK_SHORTCIRCUIT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// pub(crate): the boot-progress view reads this as its menu-open-era milestone (the shortcircuit
/// fires within ~10ms of the title-accept-byte natural menu-open on the product path).
pub(crate) static NETWORK_CHECK_SHORTCIRCUIT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// THE MILESTONE-3 FIX (zero-input, save-safe). `CS::NetworkCheckJob::Run` is a title-flow MenuJob the
/// TitleTopDialog registrar chains UNCONDITIONALLY at menu-open. Offline, its Steam-holder check
/// (FUN_140cab320: all 3 holders field@0x10==2) and EOS check (FUN_140ddfb90) never pass, so every
/// decision-tree leaf builds a GR_System_Message MessageBoxDialog -- EXCEPT one leaf that does
/// `MenuJobResult::SetResult(Continue)` with no modal (decompile-verified). This detour REPLACES Run
/// with exactly that clean leaf, skipping the entire tree, so ZERO modals are ever enqueued regardless
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
    // Always exit to the no-modal Continue (offline modal suppression). This job is REPLACED either
    // way, so no real check / modal runs -> save-safe + online-safe.
    //
    // REGRESSION FIX (2026-06-30): a prior "PORTRAIT HOLD" held this job in a RUNNING state (>1, so
    // MenuJobResult::ShouldContinue keeps it polling) until the menu portrait was captured. That was
    // self-defeating: holding NetworkCheckJob stalls the title-flow check chain, so the SAVE-data
    // ShowProgressJob (the boot ProfileSummary read) never runs -> the profile stays empty -> the
    // autoload starts a NEW GAME instead of loading the real character (and the stalled flow crashed
    // the world-load). The hold waited on a capture that could not happen until the read it was
    // blocking completed. Runtime-confirmed: with the hold gone, the boot read fires (showprog PASS),
    // the real character loads, and the world reaches `player_present`. The portrait-capture timing is
    // owned DOWNSTREAM by `portrait_render_window` instead, which holds the load COMMIT after menu-open
    // (i.e. AFTER the boot read has populated the slot). bd autoload-regression-lookat-breaks-bootread-2026-06-30.
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
    if let Ok(base) = game_module_base() {
        if r8 > null && unsafe { safe_read_usize(r8) }.is_some() {
            unsafe { *(r8 as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
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

/// Install the NetworkCheckJob::Run short-circuit ONCE (MinHook on 0x140821310), mirroring the
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
    let Ok(addr) = game_rva(NETWORK_CHECK_JOB_RUN_RVA) else {
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
                    std::mem::forget(hook);
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
/// result, r8=FD4Time*)` -- IDENTICAL to NetworkCheckJob::Run.
const SHOW_PROGRESS_JOB_RUN_RVA: u32 = 0x8349c0;
/// `MenuJobState::Success` (=2; Continue=1). Verified from FUN_1407a7340's `SetResult(.,Success,0)`
/// clean leaf (deobf `lea edx,[r8+2]`). A passing check returns Success -> `ShouldContinue` (state>1)
/// true -> ShowProgressJob::Run propagates it -> flow ADVANCES (no modal). Forcing Continue(1) would
/// loop the timed job; Success(2) completes it cleanly.
const MENU_JOB_STATE_SUCCESS: i32 = 2;

static SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SHOW_PROGRESS_SHORTCIRCUIT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Original CS::ShowProgressJob::Run trampoline (MinHook). Needed so the SAVE-data progressType can be
/// PASSED THROUGH to its real delegate -- that delegate IS the boot ProfileSummary read (SLLoadSession
/// -> ER0000.sl2). Blanket-suppressing every type (the prior behavior) killed the save read, leaving
/// an empty profile -> Bandai privacy policy. bd boot-profile-read-STEP_InitMenu-blocked-by-showprogress-shortcircuit-2026-06-23.
static SHOW_PROGRESS_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// ShowProgressJob progressType at [job+0x18] (RE-confirmed). 10 = save-data check/load (MUST run its
/// delegate); 20=network, 30/31=sign-in, 60=login (offline-modal types we still short-circuit).
const SHOW_PROGRESS_TYPE_OFFSET: usize = 0x18;
const SHOW_PROGRESS_SAVE_TYPE: u32 = 10;
static SHOW_PROGRESS_TYPE_LOGGED: AtomicUsize = AtomicUsize::new(0);

/// THE MILESTONE-3 FIX, part 2 (zero-input, save-safe). `CS::ShowProgressJob::Run` (deobf 0x1408349c0)
/// is the SHARED Run for the offline title-flow check steps (save=10/network=20/sign-in=30,31/
/// login=60) the registrar chains at menu-open. Each runs a check delegate (job+0x20, slot +0x10);
/// offline the delegate returns an ERROR result, which ShowProgressJob::Run propagates so the pump
/// enqueues a GR_System_Message MessageBox. The 3 observed menu-open modals all come from these
/// ShowProgressJobs (NOT NetworkCheckJob, which is a separate job already hooked). This detour REPLACES
/// Run with a passing-check exit: result = {state=Success, field1=0} (exactly what FUN_1407a7340's
/// SetResult(Success) clean leaf yields) + the FD4Time vtable, skipping the delegate -> the job
/// completes successfully, the flow advances, and ZERO modals are enqueued. One hook covers all the
/// check steps. Offline-gated (no effect on an online Seamless Co-op check). bd er-effects-rs-0ye.
/// Deterministic clean-title active-save-slot override for the System-Quit->Load-Profile switch.
///
/// The clean-title reload is the game's NATIVE most-recent Continue: the ShowProgressJob save-data
/// delegate (the boot ProfileSummary read) derives+selects the MOST-RECENT save slot and writes it to
/// the active-slot field GameMan+0xac0, and the reload deserializes 0xac0 immediately afterward. On a
/// switch that makes it re-load the ORIGINAL character (proven 2026-07-02: picked slot 4 'Speed Bean'
/// but ac0 re-derived to 5 -> loaded 'Patches'). Repointing ac0 to the picked slot on a per-tick poll
/// LOSES the race -- the derivation and the load happen inside one game-task tick, so the tick-set
/// landed after the load committed. Calling this RIGHT AFTER the delegate (before the load) wins it
/// deterministically. Gated on a torn-down world (local player absent) so it only ever fires at the
/// clean-title reload, never while the old world is live -- where it would misdirect the return-title
/// quit-save to the picked slot. Save-safe: a pure active-slot write, no save-file mutation. See bd
/// system-quit-ac0-fix-insufficient-cleantitle-load-is-native-mostrecent-2026-07-02.
unsafe fn system_quit_repoint_active_slot_at_clean_title(source: &str) {
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
    // CLEAN-title only: an OLD world still up means the return-title quit-save has not run yet, and
    // ac0 selects the slot it writes -- repointing now would corrupt (overwrite) the picked slot.
    if unsafe { PlayerIns::local_player_mut() }.is_ok() {
        return;
    }
    let Ok(base) = game_module_base() else {
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
    let set_save_slot: unsafe extern "system" fn(i32) =
        unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
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
    // progressType ([job+0x18], low 32 bits). 10 = the SAVE-data check/load: its delegate is the boot
    // ProfileSummary read, so it MUST run -- pass it through to the original. Suppressing it (as the
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
        if missing_save_selection_pending() {
            if result > null && unsafe { safe_read_usize(result) }.is_some() {
                unsafe {
                    *(result as *mut i32) = MENU_JOB_STATE_CONTINUE;
                    *((result + 4) as *mut i32) = 0;
                }
            }
            if let Ok(base) = game_module_base() {
                if r8 > null && unsafe { safe_read_usize(r8) }.is_some() {
                    unsafe { *(r8 as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
                }
            }
            if d < 16 || d.is_power_of_two() {
                append_autoload_debug(format_args!(
                    "show-progress: LOOP save-data progressType {SHOW_PROGRESS_SAVE_TYPE} while missing-save picker is pending -- title/save flow paused without suspending Wine dialog threads"
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
            // The delegate above just selected the MOST-RECENT save slot into GameMan+0xac0. On a
            // System-Quit->Load-Profile switch the reload deserializes 0xac0 next, so override it to
            // the PICKED slot here -- after the native derivation, before the load. Deterministic, no
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
    if let Ok(base) = game_module_base() {
        if r8 > null && unsafe { safe_read_usize(r8) }.is_some() {
            unsafe { *(r8 as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
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

/// Install the ShowProgressJob::Run short-circuit ONCE (MinHook on 0x1408349c0). Must arm before
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
    let Ok(addr) = game_rva(SHOW_PROGRESS_JOB_RUN_RVA) else {
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
            // Store the trampoline BEFORE enabling so the SAVE-data progressType can be passed through
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
                    std::mem::forget(hook);
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

/// LATCH detour for the CS::SceneObjProxy ctor 0x14074a700 (rcx=proxy[this], rdx=MenuWindow*,
/// r8/r9 forwarded). Disasm-verified: the ctor does `mov %rdx,%rbx` (0x14074a720) then
/// `mov %rbx,0x20(%rsi)` (0x14074a735) -- so the incoming RDX is the engine-verified MenuWindow it
/// stores at proxy+0x20 (probe-6 proved the OLD TitleTopDialog-factory rdx was a std::function
/// delegate, NOT the MenuWindow). Runtime showed the old MenuWindow/MenuWindowProxy vtable constants
/// are stale for this ctor's engine-provided rdx, but static disassembly still proves the game stores
/// rdx as proxy+0x20. Treat the engine-provided heap-aligned rdx as the trust boundary and OVERWRITE
/// LATCHED_MENU_WINDOW on EVERY valid call (most-recent live host window wins -- the title's host
/// window is latched by the time STAGE2 runs). Then pure passthrough: call the original trampoline
/// with ALL args preserved + return its result, never perturbing the build.
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
    static SCENE_OBJ_PROXY_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);

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

unsafe fn build_profile_select_cover_job(
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
    let cover_builder: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(base + TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA) };
    let cover_ret = unsafe { cover_builder((&raw mut cover_slot) as usize, rdx, r8) };
    let cover_job = cover_slot;
    TITLE_CUSTOM_COVER_PROFILE_SELECT_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_RET.store(cover_ret, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_JOB.store(cover_job, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-b: BUILT non-returned custom cover {TITLE_CUSTOM_COVER_PROFILE_SELECT_NAME} via 0x{:x} from {source} -> ret=0x{cover_ret:x} job=0x{cover_job:x}; dummy={TITLE_CUSTOM_COVER_DUMMY_PROFILE_SYMBOL} target={TITLE_CUSTOM_COVER_SYSTEX_TARGET} renderer={TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS}",
        base + TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA,
    ));
}

unsafe fn build_black_cover_job(base: usize, rdx: usize, caller_rva: usize, source: &str) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == null || base == 0 {
        return;
    }
    if TITLE_CUSTOM_COVER_BLACK_BUILDS.load(Ordering::SeqCst) != 0 {
        return;
    }
    let mut cover_slot = null;
    let cover_builder: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA) };
    let cover_ret = unsafe { cover_builder((&raw mut cover_slot) as usize, rdx) };
    let cover_job = cover_slot;
    TITLE_CUSTOM_COVER_BLACK_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_BLACK_LAST_RET.store(cover_ret, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_BLACK_LAST_JOB.store(cover_job, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_BLACK_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-b: BUILT non-returned custom black cover {TITLE_CUSTOM_COVER_BLACK_NAME} via 0x{:x} from {source} -> ret=0x{cover_ret:x} job=0x{cover_job:x}; will be pumped above native title/PAB jobs",
        base + TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA,
    ));
}

pub(crate) unsafe extern "system" fn title_pab_information_visual_hook(
    out_slot: usize,
    rdx: usize,
    r8: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    let caller_rva = trace_first_game_caller_rva();
    let orig = TITLE_PAB_INFORMATION_VISUAL_ORIG.load(Ordering::SeqCst);
    let mut native_ret = out_slot;
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let native_wrapper: unsafe extern "system" fn(usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        native_ret = unsafe { native_wrapper(out_slot, rdx, r8) };
    }
    let native_job = if out_slot != null {
        unsafe { safe_read_usize(out_slot) }.unwrap_or(null)
    } else {
        null
    };
    let native_window = if native_job != null {
        unsafe { safe_read_usize(native_job + 0x130) }.unwrap_or(null)
    } else {
        null
    };
    TITLE_PAB_INFORMATION_VISUAL_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_PAB_INFORMATION_VISUAL_LAST_JOB.store(native_job, Ordering::SeqCst);
    TITLE_PAB_INFORMATION_VISUAL_LAST_WINDOW.store(native_window, Ordering::SeqCst);
    TITLE_PAB_INFORMATION_VISUAL_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-a: PRESERVED native {TITLE_PAB_INFORMATION_VISUAL_NAME} wrapper 0x{:x}; latched job=0x{native_job:x} window=0x{native_window:x} for PAB cover (out_slot=0x{out_slot:x} rdx=0x{rdx:x} r8=0x{r8:x} caller_rva=0x{caller_rva:x})",
        base + TITLE_NATIVE_MENU_VISUAL_TITLE_INFORMATION_RVA,
    ));
    native_ret
}

/// Detour for BeginTitle's `05_000_Title` visual wrapper (deobf 0x14081f9f0). Static RE shows the
/// wrapper constructs a CSScaleformLoadInfo with filename `05_000_Title` and calls factory
/// 0x1407acbf0 to allocate/return a MenuWindowJob. For the title-cover masquerade we now preserve
/// that native MenuWindowJob and only latch it for the render-only FadeIn suppressor below. This keeps
/// TitleStep, FixOrderJobSequence, native Continue, STEP_PlayGame, and the resident-UI CSMenuMan+0x21
/// gate untouched; the draw bit is cleared later only for this preserved native title window.
pub(crate) unsafe extern "system" fn title_native_menu_visual_begin_title_hook(
    out_slot: usize,
    rdx: usize,
    r8: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    let prev_out = if out_slot != null {
        unsafe { safe_read_usize(out_slot) }.unwrap_or(null)
    } else {
        null
    };
    let caller_rva = trace_first_game_caller_rva();
    TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_OUT_SLOT.store(out_slot, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_PREV_OUT.store(prev_out, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_ARG_RDX.store(rdx, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_ARG_R8.store(r8, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);

    let orig = TITLE_NATIVE_MENU_VISUAL_SUPPRESS_ORIG.load(Ordering::SeqCst);
    let mut native_ret = out_slot;
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let native_wrapper: unsafe extern "system" fn(usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        native_ret = unsafe { native_wrapper(out_slot, rdx, r8) };
    }
    let native_job = if out_slot != null {
        unsafe { safe_read_usize(out_slot) }.unwrap_or(null)
    } else {
        null
    };
    let native_window = if native_job != null {
        unsafe { safe_read_usize(native_job + 0x130) }.unwrap_or(null)
    } else {
        null
    };
    TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB.store(native_job, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW.store(native_window, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-b: independent 01_900_Black build disabled; prior pump proof stalled title flow (no completion epilogue)"
    ));

    append_autoload_debug(format_args!(
        "title-cover-part-a: PRESERVED native {TITLE_NATIVE_MENU_VISUAL_NAME} wrapper 0x{:x}/factory 0x{:x}; latched job=0x{native_job:x} window=0x{native_window:x} for render-only suppression (out_slot=0x{out_slot:x} prev=0x{prev_out:x} rdx=0x{rdx:x} r8=0x{r8:x} caller_rva=0x{caller_rva:x})",
        base + TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA,
        base + TITLE_NATIVE_MENU_VISUAL_FACTORY_RVA,
    ));
    native_ret
}

unsafe fn force_hide_title_logo_surface(
    base: usize,
    logo: usize,
    requested_visible: usize,
    source: &str,
) {
    if base == TITLE_OWNER_SCAN_START_ADDRESS
        || base == 0
        || logo == 0
        || logo == TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let orig = TITLE_LOGO_SET_VISIBLE_ORIG.load(Ordering::SeqCst);
    let set_visible: unsafe extern "system" fn(usize, u8) =
        if orig != 0 && orig != HOOK_ORIGINAL_UNSET {
            unsafe { std::mem::transmute(orig) }
        } else {
            unsafe { std::mem::transmute(base + TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA) }
        };
    unsafe { set_visible(logo, 0) };
    let calls = TITLE_LOGO_GFX_HIDE_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    TITLE_LOGO_GFX_HIDE_LAST_LOGO.store(logo, Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE
        .store(OWN_STEPPER_PHASE.load(Ordering::SeqCst), Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_REQUESTED_VISIBLE.store(requested_visible, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-a: forced {TITLE_LOGO_BACK_VIEW_PARTS_NAME}/{TITLE_LOGO_RESOURCE_NAME} hidden via {source} logo=0x{logo:x} requested_visible={requested_visible} hide_calls={calls}"
    ));
}

pub(crate) unsafe extern "system" fn title_logo_set_visible_force_hidden_hook(
    logo: usize,
    visible: u8,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    unsafe { force_hide_title_logo_surface(base, logo, visible as usize, "SetVisible detour") };
}

pub(crate) unsafe extern "system" fn title_logo_ctor_force_hidden_hook(
    logo: usize,
    resource: usize,
    param_3: usize,
    param_4: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    let orig = TITLE_LOGO_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(logo, resource, param_3, param_4) }
    } else {
        logo
    };
    unsafe { force_hide_title_logo_surface(base, logo, 0, "ctor detour") };
    ret
}

pub(crate) unsafe extern "system" fn title_top_start_login_hide_hook(
    dialog: usize,
    param_2: usize,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    let orig = TITLE_TOP_START_LOGIN_HIDE_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize, usize) =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(dialog, param_2) };
    }
    if base == null || dialog == null || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let logo = dialog + TITLE_LOGO_BACK_VIEW_PARTS_AA8_OFFSET;
    if unsafe { safe_read_usize(logo) }.is_none() {
        return;
    }
    let set_visible: unsafe extern "system" fn(usize, u8) =
        unsafe { std::mem::transmute(base + TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA) };
    unsafe { set_visible(logo, 0) };
    let calls = TITLE_LOGO_GFX_HIDE_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    TITLE_LOGO_GFX_HIDE_LAST_DIALOG.store(dialog, Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_LOGO.store(logo, Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE
        .store(OWN_STEPPER_PHASE.load(Ordering::SeqCst), Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-a: hid {TITLE_LOGO_BACK_VIEW_PARTS_NAME}/{TITLE_LOGO_RESOURCE_NAME} after native TitleTopDialog start-login via 0x{:x} dialog=0x{dialog:x} logo=0x{logo:x} hide_calls={calls}",
        base + TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA,
    ));
}

pub(crate) unsafe extern "system" fn title_custom_cover_menu_window_run_hook(
    job: usize,
    load_params: usize,
    fd4_time: usize,
    menu_man: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = TITLE_CUSTOM_COVER_RUN_ORIG.load(Ordering::SeqCst);
    if orig == null || orig == HOOK_ORIGINAL_UNSET {
        return null;
    }
    let run: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let ret = unsafe { run(job, load_params, fd4_time, menu_man) };
    if TITLE_CUSTOM_COVER_RUN_RECURSION.load(Ordering::SeqCst) != 0 {
        return ret;
    }
    let title_job = TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB.load(Ordering::SeqCst);
    let pab_job = TITLE_PAB_INFORMATION_VISUAL_LAST_JOB.load(Ordering::SeqCst);
    let cover_job = TITLE_CUSTOM_COVER_BLACK_LAST_JOB.load(Ordering::SeqCst);
    let native_job = if job == title_job {
        title_job
    } else if job == pab_job {
        pab_job
    } else {
        null
    };
    if native_job == null
        || cover_job == null
        || cover_job == TITLE_OWNER_SCAN_START_ADDRESS
        || cover_job == native_job
    {
        return ret;
    }
    TITLE_CUSTOM_COVER_RUN_RECURSION.store(1, Ordering::SeqCst);
    let cover_ret = unsafe { run(cover_job, load_params, fd4_time, menu_man) };
    TITLE_CUSTOM_COVER_RUN_RECURSION.store(0, Ordering::SeqCst);
    let cover_window = unsafe { safe_read_usize(cover_job + 0x130) }.unwrap_or(null);
    let calls = TITLE_CUSTOM_COVER_RUN_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    let profile_value = TITLE_PROFILE_FACE_LAST_VALUE.load(Ordering::SeqCst);
    if profile_value != null && profile_value != HOOK_ORIGINAL_UNSET {
        let base = game_module_base().unwrap_or(null);
        if base != null {
            let set_position: unsafe extern "system" fn(usize, f32, f32) -> usize =
                unsafe { std::mem::transmute(base + TITLE_GFX_VALUE_SET_POSITION_RVA) };
            let set_scale: unsafe extern "system" fn(usize, *const f32) -> usize =
                unsafe { std::mem::transmute(base + TITLE_GFX_VALUE_SET_SCALE_RVA) };
            let scale = [3.2f32, 3.2f32];
            append_autoload_debug(format_args!(
                "title-cover-part-b: deferred transform after custom cover value=0x{profile_value:x} calls={calls}"
            ));
            unsafe { set_position(profile_value, 640.0, 360.0) };
            unsafe { set_scale(profile_value, scale.as_ptr()) };
            TITLE_PROFILE_FACE_TRANSFORM_APPLIED.store(1, Ordering::SeqCst);
            TITLE_PROFILE_FACE_OTHER_HIDDEN.store(9, Ordering::SeqCst);
        }
    }
    TITLE_CUSTOM_COVER_RUN_LAST_NATIVE_JOB.store(native_job, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_RUN_LAST_COVER_JOB.store(cover_job, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_RUN_LAST_COVER_WINDOW.store(cover_window, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_RUN_LAST_RET.store(cover_ret, Ordering::SeqCst);
    if calls <= 16 || calls.is_power_of_two() {
        append_autoload_debug(format_args!(
            "title-cover-part-b: ran custom black cover {TITLE_CUSTOM_COVER_BLACK_NAME} job=0x{cover_job:x} alongside native title/PAB job=0x{native_job:x}; ret=0x{cover_ret:x} window=0x{cover_window:x} calls={calls}"
        ));
    }
    ret
}
