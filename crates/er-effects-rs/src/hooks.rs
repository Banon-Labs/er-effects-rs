//! hooks module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::input_blocker::InputBlocker;
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::GetCurrentProcessId,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_KEYDOWN,
            WM_KEYUP,
        },
    },
    core::{BOOL, PCSTR},
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, experiments::*, ffi::*, telemetry::*};

pub(crate) fn process_safe_input_request(state: &mut EffectsState) {
    if SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES {
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING
            .fetch_sub(SAFE_INPUT_CONFIRM_FRAME_DECREMENT, Ordering::SeqCst);
    }
    if !state.safe_input.loaded {
        load_safe_input_runtime(&mut state.safe_input);
    }
    if state.safe_input.confirm_count == SAFE_INPUT_NO_CONFIRM_PULSES
        || state.safe_input.pulses_sent >= state.safe_input.confirm_count
    {
        return;
    }
    if DIRECT_INPUT_GET_DEVICE_STATE_ORIG.load(Ordering::SeqCst) == HOOK_ORIGINAL_UNSET
        && state.game_task_ticks < SAFE_INPUT_DIRECT_INPUT_WAIT_TICKS
    {
        state.safe_input.last_status = Some(format!(
            "waiting for DirectInput GetDeviceState hook before confirm pulses tick={}",
            state.game_task_ticks
        ));
        return;
    }
    if state.safe_input.pulses_sent == SAFE_INPUT_FIRST_PULSE_INDEX {
        if state.game_task_ticks < state.safe_input.initial_delay_ticks {
            state.safe_input.last_status = Some(format!(
                "waiting for initial safe-input delay tick={} target={}",
                state.game_task_ticks, state.safe_input.initial_delay_ticks
            ));
            return;
        }
    } else if state
        .game_task_ticks
        .saturating_sub(state.safe_input.last_pulse_tick)
        < state.safe_input.interval_ticks
    {
        return;
    }

    let before_snapshot = menu_trace_snapshot();
    let gate_reason = if requires_post_map_final_confirm_gate(&state.safe_input) {
        if !is_post_map_continuation_gate(before_snapshot) {
            state.safe_input.last_status = Some(format!(
                "waiting for post-map continuation input gate before final confirm tick={} {}",
                state.game_task_ticks,
                before_snapshot.summary()
            ));
            return;
        }
        Some("post_map_continuation")
    } else {
        None
    };

    let pulse_seq = SAFE_INPUT_CONFIRM_PULSE_SEQ
        .fetch_add(SAFE_INPUT_NEXT_PULSE_OFFSET as usize, Ordering::SeqCst)
        + SAFE_INPUT_NEXT_PULSE_OFFSET as usize;
    if let Some(reason) = gate_reason {
        let line = format!(
            "input_gate[{reason}] state-gated input satisfied pulse={}/{} tick={} {} {}",
            state.safe_input.pulses_sent + SAFE_INPUT_NEXT_PULSE_OFFSET,
            state.safe_input.confirm_count,
            state.game_task_ticks,
            before_snapshot.summary(),
            game_man_trace_summary()
        );
        append_autoload_debug(format_args!("{line}"));
        append_continue_trace(format_args!("{line}"));
    }
    append_confirm_probe(
        "before_confirm",
        pulse_seq,
        state.game_task_ticks,
        before_snapshot,
        None,
    );

    match emit_confirm_pulse_to_own_window() {
        Ok(()) => {
            state.safe_input.pulses_sent += SAFE_INPUT_NEXT_PULSE_OFFSET;
            state.safe_input.last_pulse_tick = state.game_task_ticks;
            state.safe_input.last_status = Some(format!(
                "confirm pulse {}/{} via DirectInput/key-state hook + post_message",
                state.safe_input.pulses_sent, state.safe_input.confirm_count
            ));
            append_autoload_debug(format_args!(
                "safe_input_confirm pulse {}/{} tick={} hook_frames={}",
                state.safe_input.pulses_sent,
                state.safe_input.confirm_count,
                state.game_task_ticks,
                SAFE_INPUT_CONFIRM_HOOK_FRAMES
            ));
            let after_snapshot = menu_trace_snapshot();
            append_confirm_probe(
                "after_confirm",
                pulse_seq,
                state.game_task_ticks,
                after_snapshot,
                Some(after_snapshot.advanced_from(before_snapshot)),
            );
        }
        Err(error) => {
            state.safe_input.last_status = Some(error.clone());
            append_autoload_debug(format_args!("safe_input_confirm {error}"));
            let after_snapshot = menu_trace_snapshot();
            append_confirm_probe(
                "after_confirm_error",
                pulse_seq,
                state.game_task_ticks,
                after_snapshot,
                Some(after_snapshot.advanced_from(before_snapshot)),
            );
        }
    }
}

pub(crate) fn requires_post_map_final_confirm_gate(runtime: &SafeInputRuntime) -> bool {
    runtime.confirm_count >= SAFE_INPUT_POST_MAP_MIN_CONFIRM_COUNT
        && runtime.pulses_sent + SAFE_INPUT_NEXT_PULSE_OFFSET == runtime.confirm_count
}

pub(crate) fn is_post_map_continuation_gate(snapshot: MenuTraceSnapshot) -> bool {
    snapshot.seq > MENU_TRACE_UNSEEN_SEQ
        && snapshot.hook_rva == TRACE_MENU_OTHER_LOAD_WRAPPER_RVA as usize
        && snapshot.state_qword == POST_MAP_CONTINUATION_STATE_QWORD
}

pub(crate) fn load_safe_input_runtime(runtime: &mut SafeInputRuntime) {
    runtime.loaded = true;
    runtime.interval_ticks = SAFE_INPUT_DEFAULT_INTERVAL_TICKS;
    runtime.initial_delay_ticks = SAFE_INPUT_INITIAL_DELAY_TICKS;
    runtime.last_pulse_tick = SAFE_INPUT_INITIAL_LAST_PULSE_TICK;

    let path = safe_input_path();
    let Ok(contents) = fs::read_to_string(&path) else {
        runtime.last_status = Some(format!("safe input config not found at {}", path.display()));
        return;
    };

    for line in contents.lines().map(str::trim) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "confirm_count" => {
                runtime.confirm_count = value
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(SAFE_INPUT_NO_CONFIRM_PULSES)
                    .min(SAFE_INPUT_MAX_CONFIRM_PULSES);
            }
            "interval_ticks" => {
                runtime.interval_ticks = value
                    .trim()
                    .parse::<u64>()
                    .unwrap_or(SAFE_INPUT_DEFAULT_INTERVAL_TICKS)
                    .max(GAME_TASK_TICK_INCREMENT);
            }
            "initial_delay_ticks" | "first_pulse_min_tick" => {
                runtime.initial_delay_ticks = value
                    .trim()
                    .parse::<u64>()
                    .unwrap_or(SAFE_INPUT_INITIAL_DELAY_TICKS)
                    .max(SAFE_INPUT_INITIAL_DELAY_TICKS);
            }
            "backend" => {}
            _ => {}
        }
    }
    runtime.hooks_requested = true;
    runtime.last_status = Some(format!(
        "loaded safe input config {} confirm_count={} interval_ticks={} initial_delay_ticks={}",
        path.display(),
        runtime.confirm_count,
        runtime.interval_ticks,
        runtime.initial_delay_ticks
    ));
    append_autoload_debug(format_args!(
        "{}",
        runtime
            .last_status
            .as_deref()
            .unwrap_or("loaded safe input config")
    ));
}

pub(crate) fn safe_input_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_SAFE_INPUT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-safe-input.txt")
        })
}

pub(crate) unsafe extern "system" fn find_own_window_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut window_pid = WINDOW_PID_UNSET;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_pid as *mut u32)) };
    let current_pid = unsafe { GetCurrentProcessId() };
    if window_pid == current_pid && unsafe { IsWindowVisible(hwnd).as_bool() } {
        let output = lparam.0 as *mut HWND;
        if !output.is_null() {
            unsafe { *output = hwnd };
        }
        return BOOL(ENUM_WINDOWS_STOP_NUMERIC);
    }
    BOOL(ENUM_WINDOWS_CONTINUE_NUMERIC)
}

pub(crate) fn own_window() -> Option<HWND> {
    let mut hwnd = HWND::default();
    unsafe {
        let _ = EnumWindows(
            Some(find_own_window_callback),
            LPARAM((&mut hwnd as *mut HWND).cast::<()>() as isize),
        );
    }
    if hwnd.0.is_null() { None } else { Some(hwnd) }
}

/// Total synthesized-input presses the DLL has injected anywhere (DInput device-state fill,
/// GetAsyncKeyState/GetKeyState override, PostMessage confirm pulse). The zero-input autoload
/// path must NEVER trigger any of these, so the proof oracle asserts this counter == 0 for the
/// whole run. Every injection site increments it; it is exported in telemetry as
/// `simulated_button_presses_total`.
pub(crate) static SIMULATED_INPUT_PRESSES_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// One synthesized key press.
pub(crate) const SIMULATED_PRESS_INCREMENT: usize = 1;

/// Record `count` synthesized key presses at an injection site.
pub(crate) fn note_simulated_presses(count: usize) {
    SIMULATED_INPUT_PRESSES_TOTAL.fetch_add(count, Ordering::SeqCst);
}

pub(crate) fn emit_confirm_pulse_to_own_window() -> Result<(), String> {
    SAFE_INPUT_CONFIRM_FRAMES_REMAINING.store(SAFE_INPUT_CONFIRM_HOOK_FRAMES, Ordering::SeqCst);
    let hwnd = own_window().ok_or_else(|| "no visible process window for safe input".to_owned())?;
    for key in [VK_RETURN_KEY, VK_SPACE_KEY] {
        note_simulated_presses(SIMULATED_PRESS_INCREMENT);
        unsafe { PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(key), LPARAM(KEYDOWN_LPARAM)) }
            .map_err(|error| format!("PostMessageW keydown {key:#x} failed: {error}"))?;
        unsafe { PostMessageW(Some(hwnd), WM_KEYUP, WPARAM(key), LPARAM(KEYUP_LPARAM)) }
            .map_err(|error| format!("PostMessageW keyup {key:#x} failed: {error}"))?;
    }
    Ok(())
}

pub(crate) fn safe_input_proc(module: &[u8], proc: &[u8]) -> Result<*mut c_void, String> {
    let module = unsafe { GetModuleHandleA(PCSTR(module.as_ptr())) }
        .map_err(|error| format!("GetModuleHandleA failed: {error}"))?;
    let proc = unsafe { GetProcAddress(module, PCSTR(proc.as_ptr())) }
        .ok_or_else(|| "GetProcAddress returned null".to_owned())?;
    Ok(proc as *mut c_void)
}

pub(crate) unsafe fn create_absolute_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    target: *mut c_void,
    hook_impl: *mut c_void,
    original: &AtomicUsize,
) {
    match unsafe { MhHook::new(target, hook_impl) } {
        Ok(hook) => {
            original.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "safe_input hook {name}: queue_enable failed: {status:?}"
                ));
            } else {
                append_autoload_debug(format_args!(
                    "safe_input hook {name}: target={target:p} trampoline={:p}",
                    hook.trampoline()
                ));
                hooks.push(hook);
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "safe_input hook {name}: create failed at {target:p}: {status:?}"
        )),
    }
}

pub(crate) unsafe fn create_and_apply_single_hook(
    name: &str,
    target: *mut c_void,
    hook_impl: *mut c_void,
    original: &AtomicUsize,
) {
    if original.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET {
        return;
    }
    let mut hooks = Vec::new();
    unsafe { create_absolute_hook(&mut hooks, name, target, hook_impl, original) };
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!("safe_input hook {name} applied")),
        status => append_autoload_debug(format_args!(
            "safe_input hook {name}: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}

pub(crate) fn install_safe_input_hooks() {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!("safe_input MH_Initialize failed: {status:?}"));
            return;
        }
    }

    let mut hooks = Vec::new();
    match safe_input_proc(b"user32.dll\0", b"GetAsyncKeyState\0") {
        Ok(target) => unsafe {
            create_absolute_hook(
                &mut hooks,
                "GetAsyncKeyState",
                target,
                get_async_key_state_hook as *mut c_void,
                &GET_ASYNC_KEY_STATE_ORIG,
            )
        },
        Err(error) => append_autoload_debug(format_args!(
            "safe_input GetAsyncKeyState resolve failed: {error}"
        )),
    }
    match safe_input_proc(b"user32.dll\0", b"GetKeyState\0") {
        Ok(target) => unsafe {
            create_absolute_hook(
                &mut hooks,
                "GetKeyState",
                target,
                get_key_state_hook as *mut c_void,
                &GET_KEY_STATE_ORIG,
            )
        },
        Err(error) => append_autoload_debug(format_args!(
            "safe_input GetKeyState resolve failed: {error}"
        )),
    }
    match safe_input_proc(b"dinput8.dll\0", b"DirectInput8Create\0") {
        Ok(target) => unsafe {
            create_absolute_hook(
                &mut hooks,
                "DirectInput8Create",
                target,
                direct_input8_create_hook as *mut c_void,
                &DIRECT_INPUT8_CREATE_ORIG,
            )
        },
        Err(error) => append_autoload_debug(format_args!(
            "safe_input DirectInput8Create resolve failed: {error}"
        )),
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "safe_input hooks applied count={}",
            hooks.len()
        )),
        status => {
            append_autoload_debug(format_args!("safe_input MH_ApplyQueued failed: {status:?}"))
        }
    }
    std::mem::forget(hooks);
}

pub(crate) fn is_safe_input_confirm_key(vkey: i32) -> bool {
    matches!(vkey as usize, VK_RETURN_KEY | VK_SPACE_KEY)
}

pub(crate) fn safe_input_key_state_override(vkey: i32, original_value: i16) -> i16 {
    if is_safe_input_confirm_key(vkey)
        && SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES
    {
        note_simulated_presses(SIMULATED_PRESS_INCREMENT);
        original_value | i16::MIN
    } else {
        original_value
    }
}

pub(crate) unsafe extern "system" fn get_async_key_state_hook(vkey: i32) -> i16 {
    type GetAsyncKeyState = unsafe extern "system" fn(i32) -> i16;
    let original = GET_ASYNC_KEY_STATE_ORIG.load(Ordering::SeqCst);
    let original_value = if original == HOOK_ORIGINAL_UNSET {
        SAFE_INPUT_KEY_UP_STATE
    } else {
        let original: GetAsyncKeyState = unsafe { std::mem::transmute(original) };
        unsafe { original(vkey) }
    };
    safe_input_key_state_override(vkey, original_value)
}

pub(crate) unsafe extern "system" fn get_key_state_hook(vkey: i32) -> i16 {
    type GetKeyState = unsafe extern "system" fn(i32) -> i16;
    let original = GET_KEY_STATE_ORIG.load(Ordering::SeqCst);
    let original_value = if original == HOOK_ORIGINAL_UNSET {
        SAFE_INPUT_KEY_UP_STATE
    } else {
        let original: GetKeyState = unsafe { std::mem::transmute(original) };
        unsafe { original(vkey) }
    };
    safe_input_key_state_override(vkey, original_value)
}

pub(crate) unsafe fn install_direct_input_create_device_hook(direct_input: *mut c_void) {
    if direct_input.is_null()
        || DIRECT_INPUT_CREATE_DEVICE_ORIG.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET
    {
        return;
    }
    let vtable = unsafe { *(direct_input as *const *const *mut c_void) };
    if vtable.is_null() {
        return;
    }
    let target = unsafe { *vtable.add(DIRECT_INPUT_CREATE_DEVICE_VTBL_INDEX) };
    if target.is_null() {
        return;
    }
    unsafe {
        create_and_apply_single_hook(
            "IDirectInput8::CreateDevice",
            target,
            direct_input_create_device_hook as *mut c_void,
            &DIRECT_INPUT_CREATE_DEVICE_ORIG,
        )
    };
}

pub(crate) unsafe fn install_direct_input_get_state_hook(device: *mut c_void) {
    if device.is_null()
        || DIRECT_INPUT_GET_DEVICE_STATE_ORIG.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET
    {
        return;
    }
    let vtable = unsafe { *(device as *const *const *mut c_void) };
    if vtable.is_null() {
        return;
    }
    let target = unsafe { *vtable.add(DIRECT_INPUT_DEVICE_GET_STATE_VTBL_INDEX) };
    if target.is_null() {
        return;
    }
    unsafe {
        create_and_apply_single_hook(
            "IDirectInputDevice8::GetDeviceState",
            target,
            direct_input_get_device_state_hook as *mut c_void,
            &DIRECT_INPUT_GET_DEVICE_STATE_ORIG,
        )
    };
}

pub(crate) unsafe extern "system" fn direct_input8_create_hook(
    instance: HINSTANCE,
    version: u32,
    riidltf: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    type DirectInput8Create = unsafe extern "system" fn(
        HINSTANCE,
        u32,
        *const c_void,
        *mut *mut c_void,
        *mut c_void,
    ) -> i32;
    let original = DIRECT_INPUT8_CREATE_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return DIRECT_INPUT_FAILURE_HRESULT;
    }
    let original: DirectInput8Create = unsafe { std::mem::transmute(original) };
    let hr = unsafe { original(instance, version, riidltf, out, outer) };
    if hr >= HRESULT_SUCCESS_FLOOR && !out.is_null() {
        let direct_input = unsafe { *out };
        unsafe { install_direct_input_create_device_hook(direct_input) };
    }
    hr
}

pub(crate) unsafe extern "system" fn direct_input_create_device_hook(
    this: *mut c_void,
    guid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    type CreateDevice =
        unsafe extern "system" fn(*mut c_void, *const c_void, *mut *mut c_void, *mut c_void) -> i32;
    let original = DIRECT_INPUT_CREATE_DEVICE_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return DIRECT_INPUT_FAILURE_HRESULT;
    }
    let original: CreateDevice = unsafe { std::mem::transmute(original) };
    let hr = unsafe { original(this, guid, out, outer) };
    if hr >= HRESULT_SUCCESS_FLOOR && !out.is_null() {
        let device = unsafe { *out };
        unsafe { install_direct_input_get_state_hook(device) };
    }
    hr
}

pub(crate) unsafe extern "system" fn direct_input_get_device_state_hook(
    this: *mut c_void,
    data_len: u32,
    data: *mut c_void,
) -> i32 {
    type GetDeviceState = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> i32;
    let original = DIRECT_INPUT_GET_DEVICE_STATE_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return DIRECT_INPUT_FAILURE_HRESULT;
    }
    let original: GetDeviceState = unsafe { std::mem::transmute(original) };
    let hr = unsafe { original(this, data_len, data) };
    if hr >= HRESULT_SUCCESS_FLOOR
        && !data.is_null()
        && SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES
        && data_len as usize > DIK_SPACE
    {
        let state = unsafe { std::slice::from_raw_parts_mut(data as *mut u8, data_len as usize) };
        note_simulated_presses(SIMULATED_PRESS_INCREMENT);
        note_simulated_presses(SIMULATED_PRESS_INCREMENT);
        state[DIK_RETURN] |= DIRECT_INPUT_KEY_DOWN_MASK;
        state[DIK_SPACE] |= DIRECT_INPUT_KEY_DOWN_MASK;
    }
    hr
}
