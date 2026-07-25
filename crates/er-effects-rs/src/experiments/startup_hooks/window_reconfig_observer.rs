// Observe-only user32 window-reconfiguration hooks (bd er-effects-rs-rzow).
//
// The 60fps boot videos proved the mid-boot black flashes are the game's own startup
// display-mode application: the boot window is created small/windowed and jumps to borderless
// fullscreen at ~+11s through user32 window calls, each of which XWayland/Hyprland services
// with a few black frames in the presented surface (bd boot-video-black-flash-root-cause-
// 2026-07-06). These hooks are the in-process RAM-timeline semaphore for that phenomenon:
// every CreateWindowExW / SetWindowPos / SetWindowLongPtrW / MoveWindow /
// ChangeDisplaySettingsExW call is counted, and the first few are logged with their args and
// the first game caller RVA, so a recorded video's black runs can be attributed to exact
// native calls. Pure passthrough: nothing is modified, reordered, or suppressed.

/// Trampolines (0 = hook not installed).
pub(crate) static WINRECONFIG_CREATE_WINDOW_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WINRECONFIG_SET_WINDOW_POS_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WINRECONFIG_SET_WINDOW_LONG_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WINRECONFIG_MOVE_WINDOW_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WINRECONFIG_CHANGE_DISPLAY_ORIG: AtomicUsize = AtomicUsize::new(0);

/// Total call counts (telemetry: the reconfig timeline's RAM counters).
pub(crate) static WINRECONFIG_CREATE_WINDOW_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WINRECONFIG_SET_WINDOW_POS_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WINRECONFIG_SET_WINDOW_LONG_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WINRECONFIG_MOVE_WINDOW_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WINRECONFIG_CHANGE_DISPLAY_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Last SetWindowPos geometry, packed (cx << 32 | cy) and (x << 32 | y as u32) for telemetry.
pub(crate) static WINRECONFIG_LAST_SET_POS_SIZE: AtomicUsize = AtomicUsize::new(0);

/// Per-hook log cap: the first calls carry the whole startup story; later calls only count.
const WINRECONFIG_LOG_CAP: usize = 48;
/// Class/name pointers below this are ATOM values, not strings (Win32 MAKEINTATOM contract).
const WINRECONFIG_ATOM_LIMIT: usize = 0x1_0000;
/// Bounded UTF-16 read for window/class names.
const WINRECONFIG_NAME_CAP: usize = 64;
/// DEVMODEW fixed ABI offsets (dmPelsWidth / dmPelsHeight); read raw so no Gdi feature is needed.
const DEVMODEW_PELS_WIDTH_OFFSET: usize = 0xAC;
const DEVMODEW_PELS_HEIGHT_OFFSET: usize = 0xB0;

fn winreconfig_name(ptr: usize) -> String {
    if ptr == 0 {
        return "<null>".to_owned();
    }
    if ptr < WINRECONFIG_ATOM_LIMIT {
        return format!("<atom:{ptr:#x}>");
    }
    let mut units: Vec<u16> = Vec::with_capacity(WINRECONFIG_NAME_CAP);
    for i in 0..WINRECONFIG_NAME_CAP {
        let unit = unsafe { *(ptr as *const u16).add(i) };
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16(&units).unwrap_or_else(|_| format!("<utf16-err:{ptr:#x}>"))
}

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

unsafe extern "system" fn winreconfig_create_window_hook(
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
    let count = WINRECONFIG_CREATE_WINDOW_CALLS.fetch_add(1, Ordering::SeqCst);
    let orig = WINRECONFIG_CREATE_WINDOW_ORIG.load(Ordering::SeqCst);
    let f: CreateWindowExWFn = unsafe { std::mem::transmute(orig) };
    let hwnd = unsafe {
        f(
            exstyle, class, name, style, x, y, w, h, parent, menu, instance, param,
        )
    };
    if count < WINRECONFIG_LOG_CAP {
        append_autoload_debug(format_args!(
            "winreconfig: CreateWindowExW #{count} class={} name={} style=0x{style:x} exstyle=0x{exstyle:x} rect=({x},{y} {w}x{h}) parent=0x{parent:x} -> hwnd=0x{hwnd:x} caller_rva=0x{:x}",
            winreconfig_name(class),
            winreconfig_name(name),
            trace_first_game_caller_rva(),
        ));
    }
    hwnd
}

type SetWindowPosFn = unsafe extern "system" fn(usize, usize, i32, i32, i32, i32, u32) -> i32;

unsafe extern "system" fn winreconfig_set_window_pos_hook(
    hwnd: usize,
    insert_after: usize,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> i32 {
    let count = WINRECONFIG_SET_WINDOW_POS_CALLS.fetch_add(1, Ordering::SeqCst);
    WINRECONFIG_LAST_SET_POS_SIZE.store(
        ((cx as u32 as usize) << 32) | cy as u32 as usize,
        Ordering::SeqCst,
    );
    if count < WINRECONFIG_LOG_CAP {
        append_autoload_debug(format_args!(
            "winreconfig: SetWindowPos #{count} hwnd=0x{hwnd:x} after=0x{insert_after:x} rect=({x},{y} {cx}x{cy}) flags=0x{flags:x} caller_rva=0x{:x}",
            trace_first_game_caller_rva(),
        ));
    }
    let orig = WINRECONFIG_SET_WINDOW_POS_ORIG.load(Ordering::SeqCst);
    let f: SetWindowPosFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(hwnd, insert_after, x, y, cx, cy, flags) }
}

type SetWindowLongPtrWFn = unsafe extern "system" fn(usize, i32, isize) -> isize;

unsafe extern "system" fn winreconfig_set_window_long_hook(
    hwnd: usize,
    index: i32,
    value: isize,
) -> isize {
    let count = WINRECONFIG_SET_WINDOW_LONG_CALLS.fetch_add(1, Ordering::SeqCst);
    let orig = WINRECONFIG_SET_WINDOW_LONG_ORIG.load(Ordering::SeqCst);
    let f: SetWindowLongPtrWFn = unsafe { std::mem::transmute(orig) };
    let previous = unsafe { f(hwnd, index, value) };
    if count < WINRECONFIG_LOG_CAP {
        append_autoload_debug(format_args!(
            "winreconfig: SetWindowLongPtrW #{count} hwnd=0x{hwnd:x} index={index} value=0x{value:x} prev=0x{previous:x} caller_rva=0x{:x}",
            trace_first_game_caller_rva(),
        ));
    }
    previous
}

type MoveWindowFn = unsafe extern "system" fn(usize, i32, i32, i32, i32, i32) -> i32;

unsafe extern "system" fn winreconfig_move_window_hook(
    hwnd: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    repaint: i32,
) -> i32 {
    let count = WINRECONFIG_MOVE_WINDOW_CALLS.fetch_add(1, Ordering::SeqCst);
    if count < WINRECONFIG_LOG_CAP {
        append_autoload_debug(format_args!(
            "winreconfig: MoveWindow #{count} hwnd=0x{hwnd:x} rect=({x},{y} {w}x{h}) repaint={repaint} caller_rva=0x{:x}",
            trace_first_game_caller_rva(),
        ));
    }
    let orig = WINRECONFIG_MOVE_WINDOW_ORIG.load(Ordering::SeqCst);
    let f: MoveWindowFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(hwnd, x, y, w, h, repaint) }
}

type ChangeDisplaySettingsExWFn =
    unsafe extern "system" fn(usize, usize, usize, u32, usize) -> i32;

unsafe extern "system" fn winreconfig_change_display_hook(
    devname: usize,
    devmode: usize,
    hwnd: usize,
    flags: u32,
    param: usize,
) -> i32 {
    let count = WINRECONFIG_CHANGE_DISPLAY_CALLS.fetch_add(1, Ordering::SeqCst);
    if count < WINRECONFIG_LOG_CAP {
        let (pels_w, pels_h) = if devmode == 0 {
            (0u32, 0u32)
        } else {
            unsafe {
                (
                    *((devmode + DEVMODEW_PELS_WIDTH_OFFSET) as *const u32),
                    *((devmode + DEVMODEW_PELS_HEIGHT_OFFSET) as *const u32),
                )
            }
        };
        append_autoload_debug(format_args!(
            "winreconfig: ChangeDisplaySettingsExW #{count} dev={} devmode=0x{devmode:x} pels={pels_w}x{pels_h} hwnd=0x{hwnd:x} flags=0x{flags:x} caller_rva=0x{:x}",
            winreconfig_name(devname),
            trace_first_game_caller_rva(),
        ));
    }
    let orig = WINRECONFIG_CHANGE_DISPLAY_ORIG.load(Ordering::SeqCst);
    let f: ChangeDisplaySettingsExWFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(devname, devmode, hwnd, flags, param) }
}

/// Install all observe-only user32 window-reconfiguration hooks. Runs from its own attach
/// thread (same early-attach pattern as the safe-input hooks) so CreateWindowExW is covered
/// before the game builds its startup window.
pub(crate) fn install_window_reconfig_observer_hooks() {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "winreconfig: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    let targets: [(&str, &[u8], *mut c_void, &AtomicUsize); 5] = [
        (
            "CreateWindowExW",
            b"CreateWindowExW\0",
            winreconfig_create_window_hook as *mut c_void,
            &WINRECONFIG_CREATE_WINDOW_ORIG,
        ),
        (
            "SetWindowPos",
            b"SetWindowPos\0",
            winreconfig_set_window_pos_hook as *mut c_void,
            &WINRECONFIG_SET_WINDOW_POS_ORIG,
        ),
        (
            "SetWindowLongPtrW",
            b"SetWindowLongPtrW\0",
            winreconfig_set_window_long_hook as *mut c_void,
            &WINRECONFIG_SET_WINDOW_LONG_ORIG,
        ),
        (
            "MoveWindow",
            b"MoveWindow\0",
            winreconfig_move_window_hook as *mut c_void,
            &WINRECONFIG_MOVE_WINDOW_ORIG,
        ),
        (
            "ChangeDisplaySettingsExW",
            b"ChangeDisplaySettingsExW\0",
            winreconfig_change_display_hook as *mut c_void,
            &WINRECONFIG_CHANGE_DISPLAY_ORIG,
        ),
    ];
    for (name, proc, hook_impl, original) in targets {
        match safe_input_proc(b"user32.dll\0", proc) {
            Ok(target) => unsafe {
                create_absolute_hook(&mut hooks, name, target, hook_impl, original)
            },
            Err(error) => append_autoload_debug(format_args!(
                "winreconfig: {name} resolve failed: {error}"
            )),
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "winreconfig: observer hooks applied count={} (observe-only)",
            hooks.len()
        )),
        status => append_autoload_debug(format_args!(
            "winreconfig: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
    winreconfig_finish(
        WINRECONFIG_RESULT_DISABLED,
        0,
        "startup geometry manipulation disabled; observer hooks remain observe-only",
    );
}

// ---------------------------------------------------------------------------------------------
// STARTUP GEOMETRY APPLY -- DISABLED.
//
// This module now observes native window reconfiguration only. It does not apply a final monitor
// rect, resize, move, focus, pin, float, or otherwise place Elden Ring.

/// Result latch: 0 = not finished, 7 = disabled. Older telemetry consumers may still know the
/// removed historical values 1..6 from the previous early-apply implementation.
pub(crate) static WINRECONFIG_EARLY_APPLY_RESULT: AtomicUsize = AtomicUsize::new(0);
/// Attach-relative ms when the disabled apply latch fired. The rect pack stays 0 because no rect is
/// applied.
pub(crate) static WINRECONFIG_EARLY_APPLY_MS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static WINRECONFIG_EARLY_APPLY_RECT: AtomicUsize = AtomicUsize::new(0);

const WINRECONFIG_RESULT_DISABLED: usize = 7;

fn winreconfig_finish(result: usize, since_ms: u128, detail: &str) {
    WINRECONFIG_EARLY_APPLY_MS.store(since_ms.min(usize::MAX as u128) as usize, Ordering::SeqCst);
    WINRECONFIG_EARLY_APPLY_RESULT.store(result, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "winreconfig: EARLY-APPLY result={result} at +{since_ms}ms -- {detail}"
    ));
}
