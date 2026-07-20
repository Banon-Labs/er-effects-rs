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
    CAN_MOVE_CONFIRMED, DID_MOVE_FRAMES, MOVE_PROBE_ACTIVE, MOVE_PROBE_EPOCH,
    MOVE_PROBE_MOVED_FRAMES, MOVE_PROBE_PER_FRAME_THRESHOLD, MOVE_PROBE_REQUIRED_FRAMES,
    SUPPLIED_MOVEMENT_INPUT_FRAMES, SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT,
};
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

/// Drive one frame of the can-move probe. Called every frame the local player is present (in-world);
/// `pos` is the player's havok position this frame. It does NOT gate on `render_ready`/`draw_group`:
/// live telemetry proved those read FALSE even for a visibly-rendered, user-controllable load (the
/// only field that distinguished playable from frozen was havok MOVEMENT), so movement itself is the
/// oracle. It injects a forward stick and counts consecutive frames of real displacement; a frozen
/// character (static position) never accumulates, a controllable one clears 60 frames and latches.
pub(crate) fn tick(pos: (f32, f32, f32)) {
    // PROOF-ONLY: the probe drives real forward input, so it must NOT fire in a normal user session
    // (it would fight the player). It runs only when the autonomous movement-proof harness stages the
    // control file `er-effects-prove-movement.txt` next to the game exe. Cached after the first read
    // (0=unknown, 1=on, 2=off); the harness writes the file before launch so it is present in-world.
    static PROOF_GATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
    let gate = PROOF_GATE.load(Ordering::Relaxed);
    let enabled = if gate == 0 {
        // DECOUPLED TOGGLE (2026-07-19): the movement-proof forward-input drive runs when the
        // input-harness DLL is present (prove_movement_enabled() = harness_dll_present(), a
        // GetModuleHandle check, not a marker/env gate). It must never fire in a normal user
        // session (no harness DLL loaded), so it stays off there. bd
        // three-semaphores-can-move-did-move-supplied-input-2026-07-19.
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
    // New load epoch -> reset the probe (each load must re-prove movement on its own).
    if MOVE_PROBE_EPOCH.swap(epoch, Ordering::SeqCst) != epoch {
        CAN_MOVE_CONFIRMED.store(false, Ordering::SeqCst);
        MOVE_PROBE_MOVED_FRAMES.store(0, Ordering::SeqCst);
        DID_MOVE_FRAMES.store(0, Ordering::Relaxed);
        SUPPLIED_MOVEMENT_INPUT_FRAMES.store(0, Ordering::Relaxed);
        MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
        *lock_prev() = None;
    }

    // Already proven for this load -> stop injecting.
    if CAN_MOVE_CONFIRMED.load(Ordering::SeqCst) {
        MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
        return;
    }

    // In-world: arm the injection (the pad-poll hook writes a full-forward stick into the active pad
    // device this frame) and measure this frame's horizontal displacement. During a load/menu/frozen
    // state the character does not move under the injected stick, so the consecutive counter never
    // accumulates -- no false positive, no dependence on the broken render oracle.
    MOVE_PROBE_ACTIVE.store(true, Ordering::SeqCst);
    // Gameplay input only applies while ER is focused; for an unattended proof, force ER foreground
    // (throttled ~1x/sec so it doesn't churn focus every frame). OFF unless the proof harness opts in.
    if crate::experiments::probe_foreground_enabled() {
        static FG_TICK: AtomicUsize = AtomicUsize::new(0);
        if FG_TICK.fetch_add(1, Ordering::Relaxed) % 30 == 0 {
            crate::experiments::sq_repro_force_foreground_now();
        }
    }
    let mut prev = lock_prev();
    if let Some((px, _py, pz)) = *prev {
        let dx = pos.0 - px;
        let dz = pos.2 - pz;
        let horiz = (dx * dx + dz * dz).sqrt();
        if horiz >= MOVE_PROBE_PER_FRAME_THRESHOLD {
            // DID_MOVE: real displacement observed while supplying input (cumulative, never reset per
            // frame) -- proves the injected input actually moved the character.
            DID_MOVE_FRAMES.fetch_add(1, Ordering::Relaxed);
            let moved = MOVE_PROBE_MOVED_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
            if moved >= MOVE_PROBE_REQUIRED_FRAMES {
                CAN_MOVE_CONFIRMED.store(true, Ordering::SeqCst);
                MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "can-move: PROVEN for load epoch {epoch} -- {moved} consecutive frames of injected-forward movement (last frame horiz={horiz:.4})"
                ));
            }
        } else {
            // Not moving this frame -> the run must be CONSECUTIVE, so reset.
            MOVE_PROBE_MOVED_FRAMES.store(0, Ordering::SeqCst);
        }
    }
    *prev = Some(pos);
}
