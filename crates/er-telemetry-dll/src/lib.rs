//! er-telemetry-dll: a thin standalone telemetry cdylib.
//!
//! Modeled on er-reload-trace-dll's shape: a `DllMain` that, on
//! `DLL_PROCESS_ATTACH`, spawns an install thread which waits for the game's
//! task manager and registers a game-thread `FrameBegin` recurring tick. The
//! tick runs ONLY er-telemetry's read-side oracles (game-RAM/PE reads that need
//! no product hooks) and writes `er-telemetry-standalone.json`.
//!
//! Runnable alone (telemetry-only me3 profile) or alongside the product DLL as an
//! additional `[[natives]]` entry. All reusable logic lives in the er-telemetry
//! LIB; this crate is only the DllMain + task-registration shell.

#[cfg(windows)]
use std::{ffi::c_void, sync::Once};

#[cfg(windows)]
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp},
    fd4::FD4TaskData,
};
#[cfg(windows)]
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};
#[cfg(windows)]
use windows::{
    Win32::{
        Foundation::HINSTANCE,
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            SystemServices::DLL_PROCESS_ATTACH,
        },
    },
    core::PCSTR,
};

const DLL_MAIN_SUCCESS: i32 = 1;

#[cfg(windows)]
static START: Once = Once::new();
#[cfg(windows)]
static WINRECONFIG_START: Once = Once::new();

#[cfg(windows)]
const DEVMODEW_PELS_WIDTH_OFFSET: usize = 0xAC;
#[cfg(windows)]
const DEVMODEW_PELS_HEIGHT_OFFSET: usize = 0xB0;

#[cfg(windows)]
fn pack_size(w: i32, h: i32) -> usize {
    ((w as u32 as usize) << 32) | h as u32 as usize
}

#[cfg(windows)]
fn pack_u32_size(w: u32, h: u32) -> usize {
    ((w as usize) << 32) | h as usize
}

#[cfg(windows)]
fn proc_addr(module: &[u8], proc: &[u8]) -> Result<*mut c_void, String> {
    let module = unsafe { GetModuleHandleA(PCSTR(module.as_ptr())) }
        .map_err(|error| format!("GetModuleHandleA failed: {error}"))?;
    let proc = unsafe { GetProcAddress(module, PCSTR(proc.as_ptr())) }
        .ok_or_else(|| "GetProcAddress returned null".to_owned())?;
    Ok(proc as *mut c_void)
}

#[cfg(windows)]
type CreateWindowExWFn = unsafe extern "system" fn(
    u32,
    usize,
    usize,
    u32,
    i32,
    i32,
    i32,
    i32,
    usize,
    usize,
    usize,
    usize,
) -> usize;

#[cfg(windows)]
unsafe extern "system" fn telemetry_create_window_hook(
    exstyle: u32,
    class: usize,
    name: usize,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: usize,
    menu: usize,
    instance: usize,
    param: usize,
) -> usize {
    er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_ORIG
        .load(std::sync::atomic::Ordering::SeqCst);
    let f: CreateWindowExWFn = unsafe { std::mem::transmute(orig) };
    unsafe {
        f(
            exstyle, class, name, style, x, y, w, h, parent, menu, instance, param,
        )
    }
}

#[cfg(windows)]
type SetWindowPosFn = unsafe extern "system" fn(usize, usize, i32, i32, i32, i32, u32) -> i32;

#[cfg(windows)]
unsafe extern "system" fn telemetry_set_window_pos_hook(
    hwnd: usize,
    insert_after: usize,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> i32 {
    let _ = (insert_after, x, y);
    er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    er_telemetry::counters::WINRECONFIG_LAST_SET_POS_SIZE
        .store(pack_size(cx, cy), std::sync::atomic::Ordering::SeqCst);
    er_telemetry::counters::WINRECONFIG_LAST_SET_POS_FLAGS
        .store(flags as usize, std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_ORIG
        .load(std::sync::atomic::Ordering::SeqCst);
    let f: SetWindowPosFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(hwnd, insert_after, x, y, cx, cy, flags) }
}

#[cfg(windows)]
type SetWindowLongPtrWFn = unsafe extern "system" fn(usize, i32, isize) -> isize;

#[cfg(windows)]
unsafe extern "system" fn telemetry_set_window_long_hook(
    hwnd: usize,
    index: i32,
    value: isize,
) -> isize {
    er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_ORIG
        .load(std::sync::atomic::Ordering::SeqCst);
    let f: SetWindowLongPtrWFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(hwnd, index, value) }
}

#[cfg(windows)]
type MoveWindowFn = unsafe extern "system" fn(usize, i32, i32, i32, i32, i32) -> i32;

#[cfg(windows)]
unsafe extern "system" fn telemetry_move_window_hook(
    hwnd: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    repaint: i32,
) -> i32 {
    let _ = (x, y);
    er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    er_telemetry::counters::WINRECONFIG_LAST_MOVE_SIZE
        .store(pack_size(w, h), std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_ORIG
        .load(std::sync::atomic::Ordering::SeqCst);
    let f: MoveWindowFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(hwnd, x, y, w, h, repaint) }
}

#[cfg(windows)]
type ChangeDisplaySettingsExWFn = unsafe extern "system" fn(usize, usize, usize, u32, usize) -> i32;

#[cfg(windows)]
unsafe extern "system" fn telemetry_change_display_hook(
    devname: usize,
    devmode: usize,
    hwnd: usize,
    flags: u32,
    param: usize,
) -> i32 {
    er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if devmode != 0 {
        let (pels_w, pels_h) = unsafe {
            (
                *((devmode + DEVMODEW_PELS_WIDTH_OFFSET) as *const u32),
                *((devmode + DEVMODEW_PELS_HEIGHT_OFFSET) as *const u32),
            )
        };
        er_telemetry::counters::WINRECONFIG_LAST_CHANGE_DISPLAY_SIZE.store(
            pack_u32_size(pels_w, pels_h),
            std::sync::atomic::Ordering::SeqCst,
        );
    }
    er_telemetry::counters::WINRECONFIG_LAST_CHANGE_DISPLAY_FLAGS
        .store(flags as usize, std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_ORIG
        .load(std::sync::atomic::Ordering::SeqCst);
    let f: ChangeDisplaySettingsExWFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(devname, devmode, hwnd, flags, param) }
}

#[cfg(windows)]
fn install_window_reconfig_hooks() {
    use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
    use std::sync::atomic::Ordering;

    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RESULT.store(8, Ordering::SeqCst);
            er_telemetry::counters::WINRECONFIG_EARLY_APPLY_MS
                .store(status as usize, Ordering::SeqCst);
            return;
        }
    }

    let targets: [(&[u8], *mut c_void, &std::sync::atomic::AtomicUsize); 5] = [
        (
            b"CreateWindowExW\0",
            telemetry_create_window_hook as *mut c_void,
            &er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_ORIG,
        ),
        (
            b"SetWindowPos\0",
            telemetry_set_window_pos_hook as *mut c_void,
            &er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_ORIG,
        ),
        (
            b"SetWindowLongPtrW\0",
            telemetry_set_window_long_hook as *mut c_void,
            &er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_ORIG,
        ),
        (
            b"MoveWindow\0",
            telemetry_move_window_hook as *mut c_void,
            &er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_ORIG,
        ),
        (
            b"ChangeDisplaySettingsExW\0",
            telemetry_change_display_hook as *mut c_void,
            &er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_ORIG,
        ),
    ];
    let mut hooks = Vec::new();
    for (proc, hook_impl, orig_slot) in targets {
        let Ok(target) = proc_addr(b"user32.dll\0", proc) else {
            continue;
        };
        let Ok(hook) = (unsafe { MhHook::new(target, hook_impl) }) else {
            continue;
        };
        orig_slot.store(hook.trampoline() as usize, Ordering::SeqCst);
        if unsafe { hook.queue_enable() }.is_ok() {
            hooks.push(hook);
        }
    }
    let _ = unsafe { MH_ApplyQueued() };
    er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RESULT.store(7, Ordering::SeqCst);
    std::mem::forget(hooks);
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    _module: HINSTANCE,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        WINRECONFIG_START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-telemetry-winreconfig-hooks".into())
                .spawn(install_window_reconfig_hooks);
        });
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-telemetry-standalone".into())
                .spawn(|| {
                    // Wait for the game's task manager, then register a game-thread
                    // per-frame tick (same pattern as the product's wait_for_task_instance).
                    let task = loop {
                        match unsafe { CSTaskImp::instance() } {
                            Ok(t) => break t,
                            // No sleep (banned by scripts/check-no-timeouts.py): yield to the
                            // game threads and re-poll -- the exact wait the product uses in
                            // wait_for_task_instance (bootstrap.rs).
                            Err(_) => std::thread::yield_now(),
                        }
                    };
                    task.run_recurring(
                        |_data: &FD4TaskData| {
                            // Standalone oracles + telemetry-owned diagnostic hook counters; no EffectsState.
                            er_telemetry::standalone_tick();
                        },
                        CSTaskGroupIndex::FrameBegin,
                    );
                });
        });
    }
    DLL_MAIN_SUCCESS
}

// Non-windows: keep the crate buildable for host tooling / workspace resolution.
// A cdylib with no DllMain is valid; the game entry only exists on windows.
#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_telemetry_dll_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
