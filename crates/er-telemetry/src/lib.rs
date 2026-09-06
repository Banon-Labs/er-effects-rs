//! er-telemetry: a thin standalone telemetry cdylib.
//!
//! Modeled on er-reload-trace's shape: a `DllMain` that, on
//! `DLL_PROCESS_ATTACH`, spawns an install thread which waits for the game's
//! task manager and registers a game-thread `FrameBegin` recurring tick. The
//! tick runs ONLY er-telemetry-core's read-side oracles (game-RAM/PE reads that need
//! no product hooks) and writes `er-telemetry-standalone.json`.
//!
//! Runnable alone (telemetry-only me3 profile) or alongside the product DLL as an
//! additional `[[natives]]` entry. All reusable logic lives in the er-telemetry-core
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

/// `report_panics_to`'s sink. This shell has no logger of its own -- everything reusable lives in
/// `er-telemetry-core` -- so the panic line goes next to the executable under this DLL's own name,
/// which is where an investigator already looks for a shell's output.
#[cfg(windows)]
fn panic_log_sink(args: core::fmt::Arguments<'_>) {
    er_game_base::log::append_line(
        &er_game_base::log::game_directory_path()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("er-telemetry.log"),
        args,
    );
}

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
    er_telemetry_core::counters::WINRECONFIG_CREATE_WINDOW_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry_core::counters::WINRECONFIG_CREATE_WINDOW_ORIG
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
    er_telemetry_core::counters::WINRECONFIG_SET_WINDOW_POS_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    er_telemetry_core::counters::WINRECONFIG_LAST_SET_POS_SIZE
        .store(pack_size(cx, cy), std::sync::atomic::Ordering::SeqCst);
    er_telemetry_core::counters::WINRECONFIG_LAST_SET_POS_FLAGS
        .store(flags as usize, std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry_core::counters::WINRECONFIG_SET_WINDOW_POS_ORIG
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
    er_telemetry_core::counters::WINRECONFIG_SET_WINDOW_LONG_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry_core::counters::WINRECONFIG_SET_WINDOW_LONG_ORIG
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
    er_telemetry_core::counters::WINRECONFIG_MOVE_WINDOW_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    er_telemetry_core::counters::WINRECONFIG_LAST_MOVE_SIZE
        .store(pack_size(w, h), std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry_core::counters::WINRECONFIG_MOVE_WINDOW_ORIG
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
    er_telemetry_core::counters::WINRECONFIG_CHANGE_DISPLAY_CALLS
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if devmode != 0 {
        let (pels_w, pels_h) = unsafe {
            (
                *((devmode + DEVMODEW_PELS_WIDTH_OFFSET) as *const u32),
                *((devmode + DEVMODEW_PELS_HEIGHT_OFFSET) as *const u32),
            )
        };
        er_telemetry_core::counters::WINRECONFIG_LAST_CHANGE_DISPLAY_SIZE.store(
            pack_u32_size(pels_w, pels_h),
            std::sync::atomic::Ordering::SeqCst,
        );
    }
    er_telemetry_core::counters::WINRECONFIG_LAST_CHANGE_DISPLAY_FLAGS
        .store(flags as usize, std::sync::atomic::Ordering::SeqCst);
    let orig = er_telemetry_core::counters::WINRECONFIG_CHANGE_DISPLAY_ORIG
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
            er_telemetry_core::counters::WINRECONFIG_EARLY_APPLY_RESULT.store(8, Ordering::SeqCst);
            er_telemetry_core::counters::WINRECONFIG_EARLY_APPLY_MS
                .store(status as usize, Ordering::SeqCst);
            return;
        }
    }

    let targets: [(&[u8], *mut c_void, &std::sync::atomic::AtomicUsize); 5] = [
        (
            b"CreateWindowExW\0",
            telemetry_create_window_hook as *mut c_void,
            &er_telemetry_core::counters::WINRECONFIG_CREATE_WINDOW_ORIG,
        ),
        (
            b"SetWindowPos\0",
            telemetry_set_window_pos_hook as *mut c_void,
            &er_telemetry_core::counters::WINRECONFIG_SET_WINDOW_POS_ORIG,
        ),
        (
            b"SetWindowLongPtrW\0",
            telemetry_set_window_long_hook as *mut c_void,
            &er_telemetry_core::counters::WINRECONFIG_SET_WINDOW_LONG_ORIG,
        ),
        (
            b"MoveWindow\0",
            telemetry_move_window_hook as *mut c_void,
            &er_telemetry_core::counters::WINRECONFIG_MOVE_WINDOW_ORIG,
        ),
        (
            b"ChangeDisplaySettingsExW\0",
            telemetry_change_display_hook as *mut c_void,
            &er_telemetry_core::counters::WINRECONFIG_CHANGE_DISPLAY_ORIG,
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
    er_telemetry_core::counters::WINRECONFIG_EARLY_APPLY_RESULT.store(7, Ordering::SeqCst);
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
        // FIRST, before anything that can panic. A panic in a cdylib crosses an
        // `extern "system"` boundary and becomes an ABORT, which does not dispatch to a
        // vectored handler -- so no crash record is written at all and the process simply
        // vanishes. Enforced by `scripts/check-panic-reporter-installed.py`.
        er_game_base::panic_report::report_panics_to("er-telemetry", panic_log_sink);

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
                    // BOUNDED (2026-08-29). This loop is the one that hung a boot: on 1.17 the
                    // singleton did not appear, and this thread plus er-invasion-path's twin
                    // saturated the wineserver -- 19,348 CPU ticks here against the game's 104,
                    // fifty-nine game threads asleep, no window. er_game_base::wait spins in user
                    // space between attempts and GIVES UP, so a missing singleton leaves this
                    // shell inert instead of taking the process with it.
                    let Some(task) =
                        er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
                    else {
                        return;
                    };
                    task.run_recurring(
                        |_data: &FD4TaskData| {
                            // Standalone oracles + telemetry-owned diagnostic hook counters; no EffectsState.
                            er_telemetry_core::standalone_tick();
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
pub extern "C" fn er_telemetry_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
