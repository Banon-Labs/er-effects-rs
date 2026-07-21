//! Can-move readiness probe (2026-07-18, user-directed): PROVE that input actually moves the
//! character, not just that it is render-ready. "render-ready" says the character can be SEEN;
//! this says input MOVES it. `play_time` advancing is necessary but not sufficient (it ticks during
//! the freeze), so movement is proven by a havok-POSITION delta under a KNOWN injected forward stick,
//! sustained for `MOVE_PROBE_REQUIRED_FRAMES` (60) consecutive frames per load -- a real walk, not a
//! one-frame twitch. Runs on the game thread (safe to drive input); the XInput hook stamps the stick
//! when `MOVE_PROBE_ACTIVE`.
//!
//! Per load epoch (fresh_deser_count) the probe resets, then each render-ready frame it injects the
//! forward stick and counts consecutive frames whose horizontal displacement clears the threshold.
//! A static/frozen character repeats its position exactly (delta ~0), so it never accumulates; a
//! walking character clears 60 frames quickly and latches `CAN_MOVE_CONFIRMED`.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::constants::{
    CAN_MOVE_CONFIRMED, DID_MOVE_FRAMES, HARNESS_MOVE_VERDICT, MOVE_PROBE_ACTIVE, MOVE_PROBE_EPOCH,
    MOVE_PROBE_MOVED_FRAMES, MOVE_PROBE_PER_FRAME_THRESHOLD, SUPPLIED_MOVEMENT_INPUT_FRAMES,
    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT,
};

/// DLUID (input-device manager) singleton RVA + its input-accept-while-unfocused flag offset. Holding
/// `[DLUID+0x88d]=1` every probe frame makes ER apply the injected pad stick even while the window is
/// UNFOCUSED (bd breakthrough-pad-boundary-injection-moves-char-needs-focus). Tied DIRECTLY to the
/// probe here -- NOT the `er-effects-stay-active.txt` marker, which the samechar-3x run script sweeps,
/// so the injected stick was being discarded while ER was unfocused (bd
/// canmove-contaminated-user-moved-harness-never-supplied). Fault-safe (null/low-ptr guarded).
const DLUID_SINGLETON_RVA: u32 = 0x485dc18;
const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize = 0x88d;
const HEAP_LO: usize = 0x1_0000;

fn hold_input_active() {
    let Ok(slot) = crate::game_rva(DLUID_SINGLETON_RVA) else {
        return;
    };
    // The singleton SLOT is module memory (always mapped); read the DLUID heap pointer from it.
    let dluid = unsafe { std::ptr::read_volatile(slot as *const usize) };
    if dluid < HEAP_LO {
        return; // singleton not yet constructed
    }
    // SAFETY: dluid is a live heap object once non-null; +0x88d is a byte the game itself writes.
    unsafe { std::ptr::write_volatile((dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) as *mut u8, 1u8) };
}
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use crate::telemetry::append_autoload_debug;

/// FD4PadDevice poll (deobf `0x141f6bad0`, RE `er-movement-input-stick-boundary-2026-07-18`): the
/// per-device, per-frame function where XInput / DirectInput / ScePad all deposit the device's
/// normalized analog-stick into `this`, BELOW the OS/Steam-Input layer and BEFORE locomotion reads it.
/// Our synthetic `XInputGetState(0)` never moved the character because Steam Input routes the pad
/// through ScePad/DirectInput, not the raw xinput DLL. Hooking here and writing the left-stick injects a
/// controller stick deflection at the game's OWN input boundary -- run through the full deadzone ->
/// mapping -> locomotion chain, identical to any real pad, robust to Steam Input. This injects INPUT
/// (a stick push), NOT the locomotion output, so it faithfully tests "does input move the character".
const FD4_PAD_DEVICE_POLL_RVA: u32 = 0x1f6bad0;
const PAD_STICK_LX_OFFSET: usize = 0x89c; // f32 in [-1.0, 1.0]
const PAD_STICK_LY_OFFSET: usize = 0x8a0; // f32 in [-1.0, 1.0]; +1.0 = full forward
static ORIG_PAD_POLL: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn pad_poll_hook(this: usize, a: usize, b: usize, c: usize) -> usize {
    let orig = ORIG_PAD_POLL.load(Ordering::SeqCst);
    let ret = if orig != 0 {
        let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(this, a, b, c) }
    } else {
        0
    };
    // After the poll filled the stick from the real source, overwrite with FULL FORWARD while probing.
    // Every device is overwritten; the priority moderator's active device is the one that moves the char.
    if this != 0 && MOVE_PROBE_ACTIVE.load(Ordering::SeqCst) {
        unsafe {
            *((this + PAD_STICK_LX_OFFSET) as *mut f32) = 0.0;
            *((this + PAD_STICK_LY_OFFSET) as *mut f32) = 1.0;
        }
        // SUPPLIED_MOVEMENT_INPUT: we actually wrote the forward stick into a live pad device this
        // frame (distinct from whether it MOVED the character -- see DID_MOVE).
        SUPPLIED_MOVEMENT_INPUT_FRAMES.fetch_add(1, Ordering::Relaxed);
    }
    ret
}

/// FD4PadManager singleton RVA (GLOBAL_FD4PadManager, dump 0x14485dc20 == DLUID+0x8). Its `padDevices`
/// is a `DLFixedVector<FD4PadDevice*,4>`: inline entries at +0x18, count at +0x40. Each `FD4PadDevice`
/// holds the concrete `DLUserInputDevice` at +0x8, which carries the normalized stick at +0x89c/+0x8a0.
/// bd er-movement-input-stick-boundary-2026-07-18.
const FD4_PAD_MANAGER_RVA: u32 = 0x485dc20;
const PAD_MGR_DEVICES_OFFSET: usize = 0x18;
const PAD_MGR_DEVICE_COUNT_OFFSET: usize = 0x40;
const FD4PADDEVICE_CONCRETE_OFFSET: usize = 0x8;

/// Write full-forward LY (neutral LX) to the CONCRETE device of EVERY registered pad device, not just
/// the one the poll hook fires for. Two reasons: (1) the player's active device (phantom XInput vs real
/// ScePad/DInput) is unknown, so cover all up-to-4; (2) this dereferences `FD4PadDevice+0x8` to reach
/// the concrete device explicitly -- if the poll hook's `this` is the FD4PadDevice (not the concrete
/// device), `this+0x8a0` is 8 bytes off the real stick and never moves the char; this writes the
/// definitely-correct `concrete+0x8a0`. Every deref is low-pointer guarded. Called only while injecting.
unsafe fn inject_all_pad_devices() {
    let Ok(mgr_ptr) = crate::game_rva(FD4_PAD_MANAGER_RVA) else {
        return;
    };
    let mgr = unsafe { *(mgr_ptr as *const usize) };
    if mgr < 0x10000 {
        return;
    }
    let count = (unsafe { *((mgr + PAD_MGR_DEVICE_COUNT_OFFSET) as *const u32) } as usize).min(4);
    for i in 0..count {
        let dev = unsafe { *((mgr + PAD_MGR_DEVICES_OFFSET + i * 8) as *const usize) };
        if dev < 0x10000 {
            continue;
        }
        let concrete = unsafe { *((dev + FD4PADDEVICE_CONCRETE_OFFSET) as *const usize) };
        if concrete < 0x10000 {
            continue;
        }
        unsafe {
            *((concrete + PAD_STICK_LX_OFFSET) as *mut f32) = 0.0;
            *((concrete + PAD_STICK_LY_OFFSET) as *mut f32) = 1.0;
        }
    }
}

/// `Game.Debug::IsEnableControlOnDisactiveWindow` (deobf `0x140e53220`, RE `AUTONOMOUS-FOCUS-FIX-...`):
/// returns false in retail. Its result is cached to `CSPadStep+0xba` every frame; when the ER window
/// is UNFOCUSED and that byte is 0, `CSPadStep::STEP_Update` runs the pad-manager on the "inactive"
/// path that latches a flag which makes the locomotion consumer DISCARD our injected stick (menus still
/// work via the separate DLUID+0x88d gate, but gameplay movement does not). Forcing this to return 1
/// makes the unfocused path byte-identical to the focused one, so the injected pad stick reaches
/// locomotion WITHOUT the window being active -- the missing half of an autonomous, focus-free proof.
const IS_ENABLE_CONTROL_ON_DISACTIVE_RVA: u32 = 0xe53220;

unsafe extern "system" fn is_enable_control_on_disactive_hook(
    _a: usize,
    _b: usize,
    _c: usize,
    _d: usize,
) -> usize {
    1
}

/// Install the "enable control on inactive window" override once (proof runs only). We never call the
/// original (it just returns a debug bool), so no trampoline is retained.
fn install_focus_override_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        match unsafe { MH_Initialize() } {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            status => {
                append_autoload_debug(format_args!(
                    "can-move: focus-override MH_Initialize failed: {status:?}"
                ));
                return;
            }
        }
        let Ok(addr) = crate::game_rva(IS_ENABLE_CONTROL_ON_DISACTIVE_RVA) else {
            append_autoload_debug(format_args!("can-move: focus-override game_rva failed"));
            return;
        };
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                is_enable_control_on_disactive_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                if unsafe { hook.queue_enable() }.is_ok()
                    && matches!(unsafe { MH_ApplyQueued() }, MH_STATUS::MH_OK)
                {
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "can-move: focus-override installed at 0x{addr:x} (IsEnableControlOnDisactiveWindow->1: gameplay input applies while unfocused)"
                    ));
                } else {
                    append_autoload_debug(format_args!("can-move: focus-override enable failed"));
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "can-move: focus-override MhHook::new failed: {status:?}"
            )),
        }
    });
}

/// Install the pad-poll hook once (only when the movement proof is authorized).
fn install_pad_poll_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        match unsafe { MH_Initialize() } {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            status => {
                append_autoload_debug(format_args!(
                    "can-move: pad-poll MH_Initialize failed: {status:?}"
                ));
                return;
            }
        }
        let Ok(addr) = crate::game_rva(FD4_PAD_DEVICE_POLL_RVA) else {
            append_autoload_debug(format_args!("can-move: pad-poll game_rva failed"));
            return;
        };
        match unsafe { MhHook::new(addr as *mut c_void, pad_poll_hook as *mut c_void) } {
            Ok(hook) => {
                ORIG_PAD_POLL.store(hook.trampoline() as usize, Ordering::SeqCst);
                if unsafe { hook.queue_enable() }.is_ok()
                    && matches!(unsafe { MH_ApplyQueued() }, MH_STATUS::MH_OK)
                {
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "can-move: pad-poll hook installed at 0x{addr:x} (faithful stick injection boundary)"
                    ));
                } else {
                    append_autoload_debug(format_args!("can-move: pad-poll enable failed"));
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "can-move: pad-poll MhHook::new failed: {status:?}"
            )),
        }
    });
}

/// Previous frame's world position while a probe is active (game thread only touches this).
static PREV_POS: Mutex<Option<(f32, f32, f32)>> = Mutex::new(None);

fn lock_prev() -> std::sync::MutexGuard<'static, Option<(f32, f32, f32)>> {
    PREV_POS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Drive one frame of the can-move probe. Proves HARNESS-driven movement with USER contamination
/// EXCLUDED (user 2026-07-20). It alternates INJECT-ON windows (write the forward stick + hold
/// input-active so it applies unfocused) with INJECT-OFF windows (release the stick), and requires the
/// char to move WHILE WE inject AND to stop in the OFF tail when we release. A user moving the char
/// shows movement during OFF windows -> read as CONTAMINATED, never proof. Sets HARNESS_MOVE_VERDICT
/// (0 pending / 1 proven / 2 disproven / 3 contaminated) so the watcher tears down the instant the
/// answer is known -- no waiting for an fps/stall window (bd
/// collect-decisive-info-teardown-immediately, canmove-contaminated-user-moved-harness-never-supplied).
pub(crate) fn tick(pos: (f32, f32, f32)) {
    // INJECT-ON / INJECT-OFF window sizes. OFF_TAIL = the last N OFF frames, measured after the char
    // has decelerated, so residual momentum just after releasing the stick isn't miscounted as movement.
    const ON_FRAMES: usize = 30;
    const OFF_FRAMES: usize = 20;
    const CYCLE: usize = ON_FRAMES + OFF_FRAMES;
    const OFF_TAIL: usize = 8;

    // PROOF-ONLY: runs only when the input-harness DLL is present (prove_movement_enabled =
    // GetModuleHandle check, not a marker/env gate); never fires in a normal user session.
    static PROOF_GATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
    static PHASE_FRAME: AtomicUsize = AtomicUsize::new(0);
    static ON_TOTAL: AtomicUsize = AtomicUsize::new(0);
    static ON_MOVED: AtomicUsize = AtomicUsize::new(0);
    static OFF_TAIL_TOTAL: AtomicUsize = AtomicUsize::new(0);
    static OFF_TAIL_MOVED: AtomicUsize = AtomicUsize::new(0);

    let gate = PROOF_GATE.load(Ordering::Relaxed);
    let enabled = if gate == 0 {
        let on = crate::experiments::prove_movement_enabled();
        PROOF_GATE.store(if on { 1 } else { 2 }, Ordering::Relaxed);
        on
    } else {
        gate == 1
    };
    if !enabled {
        return;
    }
    install_pad_poll_hook();
    install_focus_override_hook();

    let epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
    // New load epoch -> reset the probe (each load must re-prove HARNESS movement on its own).
    if MOVE_PROBE_EPOCH.swap(epoch, Ordering::SeqCst) != epoch {
        CAN_MOVE_CONFIRMED.store(false, Ordering::SeqCst);
        HARNESS_MOVE_VERDICT.store(0, Ordering::SeqCst);
        MOVE_PROBE_MOVED_FRAMES.store(0, Ordering::SeqCst);
        DID_MOVE_FRAMES.store(0, Ordering::Relaxed);
        SUPPLIED_MOVEMENT_INPUT_FRAMES.store(0, Ordering::Relaxed);
        PHASE_FRAME.store(0, Ordering::Relaxed);
        ON_TOTAL.store(0, Ordering::Relaxed);
        ON_MOVED.store(0, Ordering::Relaxed);
        OFF_TAIL_TOTAL.store(0, Ordering::Relaxed);
        OFF_TAIL_MOVED.store(0, Ordering::Relaxed);
        MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
        *lock_prev() = None;
    }

    // Verdict already reached for this load -> stop injecting.
    if HARNESS_MOVE_VERDICT.load(Ordering::SeqCst) != 0 {
        MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
        return;
    }

    // Hold ER's input-accept flag EVERY frame so the injected stick applies while the window is
    // unfocused (the fix for the discarded 400 injected frames). Never forces foreground.
    hold_input_active();

    let pf = PHASE_FRAME.load(Ordering::Relaxed);
    let is_on = pf < ON_FRAMES;
    // pad_poll_hook overwrites the stick to full-forward ONLY while MOVE_PROBE_ACTIVE. During OFF we
    // leave it false so the real (neutral, unless a user pushes) stick flows through -> the OFF tail
    // measures movement we are NOT causing.
    MOVE_PROBE_ACTIVE.store(is_on, Ordering::SeqCst);
    // Force ER genuinely focused ONCE at the START of each injection burst (user 2026-07-20: "just when
    // you need to move", not constantly). Gameplay locomotion applies the injected stick only when the
    // window is truly active; kb+mouse are disabled as game inputs so this brief grab is uncontaminated.
    if is_on && pf == 0 && crate::experiments::probe_foreground_enabled() {
        crate::experiments::sq_repro_force_foreground_now();
    }
    // Also write full-forward to EVERY registered pad device's CONCRETE pointer -- covers the case where
    // the poll hook's `this` is the FD4PadDevice (so `this+0x8a0` is 8 bytes off the real stick) or the
    // player reads a device the poll hook did not fire for this frame.
    if is_on {
        unsafe { inject_all_pad_devices() };
    }

    let mut prev = lock_prev();
    if let Some((px, _py, pz)) = *prev {
        let dx = pos.0 - px;
        let dz = pos.2 - pz;
        let moved = (dx * dx + dz * dz).sqrt() >= MOVE_PROBE_PER_FRAME_THRESHOLD;
        if is_on {
            ON_TOTAL.fetch_add(1, Ordering::Relaxed);
            if moved {
                ON_MOVED.fetch_add(1, Ordering::Relaxed);
                DID_MOVE_FRAMES.fetch_add(1, Ordering::Relaxed);
                MOVE_PROBE_MOVED_FRAMES.fetch_add(1, Ordering::SeqCst);
            }
        } else if pf >= CYCLE - OFF_TAIL {
            OFF_TAIL_TOTAL.fetch_add(1, Ordering::Relaxed);
            if moved {
                OFF_TAIL_MOVED.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Latch the first clear verdict from cumulative counters.
        let ot = ON_TOTAL.load(Ordering::Relaxed);
        let om = ON_MOVED.load(Ordering::Relaxed);
        let ft = OFF_TAIL_TOTAL.load(Ordering::Relaxed);
        let fm = OFF_TAIL_MOVED.load(Ordering::Relaxed);
        let verdict = if ft >= OFF_TAIL && fm * 100 > 40 * ft {
            3 // CONTAMINATED: char moves while we are NOT injecting -> external input present
        } else if ot >= 40 && om * 100 >= 70 * ot && (ft == 0 || fm * 100 <= 15 * ft) {
            1 // PROVEN: moved under our stick, still (mostly) in the OFF tail when released
        } else if ot >= 90 && om * 100 <= 10 * ot {
            2 // DISPROVEN: many ON frames injected, char barely moved -> injection ineffective
        } else {
            0
        };
        if verdict != 0 {
            HARNESS_MOVE_VERDICT.store(verdict, Ordering::SeqCst);
            if verdict == 1 {
                CAN_MOVE_CONFIRMED.store(true, Ordering::SeqCst);
            }
            MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
            let label = match verdict {
                1 => "PROVEN(harness moved char)",
                2 => "DISPROVEN(injection ineffective)",
                _ => "CONTAMINATED(external input)",
            };
            append_autoload_debug(format_args!(
                "can-move: HARNESS_MOVE_VERDICT={verdict} {label} epoch={epoch} on_moved={om}/{ot} off_tail_moved={fm}/{ft}"
            ));
        }
    }
    PHASE_FRAME.store((pf + 1) % CYCLE, Ordering::Relaxed);
    *prev = Some(pos);
}
