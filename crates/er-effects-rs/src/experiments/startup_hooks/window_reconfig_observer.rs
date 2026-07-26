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
// native calls -- and the eventual product fix (window at final geometry from creation) has
// its before/after proof. Pure passthrough: nothing is modified, reordered, or suppressed.

/// Trampolines (0 = hook not installed).
pub(crate) use er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_ORIG;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_ORIG;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_ORIG;
pub(crate) use er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_ORIG;
pub(crate) use er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_ORIG;

/// Total call counts (telemetry: the reconfig timeline's RAM counters).
pub(crate) use er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_CALLS;
/// Last SetWindowPos geometry, packed (cx << 32 | cy) and (x << 32 | y as u32) for telemetry.
pub(crate) use er_telemetry::counters::WINRECONFIG_LAST_SET_POS_SIZE;

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
    apply_startup_window_final_geometry();
}

// ---------------------------------------------------------------------------------------------
// EARLY FINAL-GEOMETRY APPLY -- the product fix for the mid-boot black flashes.
//
// Observed (run 195813, exact frame correlation): the game applies its display config at ~+11.4s
// with MoveWindow(resize-in-place to monitor size) + SetWindowPos(FRAMECHANGED), and each
// geometry-CHANGING call costs 2-7 black presented frames while XWayland remaps the surface;
// the two later calls that change nothing produce no flash at all. So: apply the final monitor
// rect ourselves as soon as the game window exists -- BEFORE the first present, while the screen
// is legitimately black -- and the game's own reconfiguration becomes a chain of no-ops.
// The boot pump holds its FIRST self-present until this declares a result, so no pixel can reach
// the screen at pre-final geometry. Config-respecting: WINDOWED mode skips the apply entirely.

/// Result latch: 0 = not finished, 1 = applied, 2 = skipped (WINDOWED), 3 = window never found,
/// 4 = monitor info failed, 5 = config unreadable (skipped), 6 = already at final geometry.
pub(crate) use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RESULT;
/// Attach-relative ms when the early apply finished, and the applied (w<<16|h) pack.
pub(crate) use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_MS;
pub(crate) use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RECT;

const WINRECONFIG_EARLY_APPLY_MAX_MS: u128 = 20_000;
const WINRECONFIG_EARLY_APPLY_POLL_MS: u64 = 20;
const WINRECONFIG_RESULT_APPLIED: usize = 1;
const WINRECONFIG_RESULT_SKIP_WINDOWED: usize = 2;
const WINRECONFIG_RESULT_NO_WINDOW: usize = 3;
const WINRECONFIG_RESULT_NO_MONITOR: usize = 4;
const WINRECONFIG_RESULT_NO_CONFIG: usize = 5;
const WINRECONFIG_RESULT_ALREADY_FINAL: usize = 6;
const CSIDL_APPDATA: i32 = 0x1a;
const SHGFP_TYPE_CURRENT: u32 = 0;
const MAX_PATH_W: usize = 260;

type WinreconfigShGetFolderPathWFn =
    unsafe extern "system" fn(isize, i32, isize, u32, *mut u16) -> i32;

/// Read the game's own GraphicsConfig.xml ScreenMode (UTF-16 XML). Resolves %APPDATA% through
/// SHGetFolderPathW so an active save-redirect (which the game also sees) is honored.
fn winreconfig_screen_mode() -> Option<String> {
    let mut root = [0u16; MAX_PATH_W];
    let resolved = match safe_input_proc(b"shell32.dll\0", b"SHGetFolderPathW\0") {
        Ok(addr) => {
            let f: WinreconfigShGetFolderPathWFn = unsafe { std::mem::transmute(addr) };
            (unsafe { f(0, CSIDL_APPDATA, 0, SHGFP_TYPE_CURRENT, root.as_mut_ptr()) }) == 0
        }
        Err(_) => false,
    };
    let root_string = if resolved {
        let len = root.iter().position(|&u| u == 0).unwrap_or(0);
        String::from_utf16(&root[..len]).ok()?
    } else {
        std::env::var("APPDATA").ok()?
    };
    let path = format!("{root_string}\\EldenRing\\GraphicsConfig.xml");
    let bytes = std::fs::read(&path).ok()?;
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    if units.first() == Some(&0xFEFF) {
        units.remove(0);
    }
    let text = String::from_utf16(&units).ok()?;
    let open = "<ScreenMode>";
    let start = text.find(open)? + open.len();
    let end = text[start..].find("</ScreenMode>")? + start;
    let mode = text[start..end].trim().to_owned();
    append_autoload_debug(format_args!(
        "winreconfig: GraphicsConfig ScreenMode='{mode}' (from {path})"
    ));
    Some(mode)
}

fn winreconfig_finish(result: usize, since_ms: u128, detail: &str) {
    WINRECONFIG_EARLY_APPLY_MS.store(since_ms.min(usize::MAX as u128) as usize, Ordering::SeqCst);
    WINRECONFIG_EARLY_APPLY_RESULT.store(result, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "winreconfig: EARLY-APPLY result={result} at +{since_ms}ms -- {detail}"
    ));
}

fn apply_startup_window_final_geometry() {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, MoveWindow, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER,
        SetWindowPos,
    };

    let start = std::time::Instant::now();
    match winreconfig_screen_mode() {
        None => {
            winreconfig_finish(
                WINRECONFIG_RESULT_NO_CONFIG,
                start.elapsed().as_millis(),
                "GraphicsConfig unreadable; leaving startup geometry to the game",
            );
            return;
        }
        Some(mode) if mode.eq_ignore_ascii_case("WINDOWED") => {
            winreconfig_finish(
                WINRECONFIG_RESULT_SKIP_WINDOWED,
                start.elapsed().as_millis(),
                "ScreenMode=WINDOWED; the game keeps its own window sizing",
            );
            return;
        }
        Some(_) => {}
    }

    // Bounded wait for the game's main window (same pacing primitive as the boot pump: a
    // held-but-never-sent channel; recv_timeout is the sanctioned bounded wait).
    let (_tick_tx, tick_rx) = std::sync::mpsc::channel::<()>();
    let poll = std::time::Duration::from_millis(WINRECONFIG_EARLY_APPLY_POLL_MS);
    let hwnd = loop {
        if let Some(hwnd) = own_window() {
            break hwnd;
        }
        if start.elapsed().as_millis() > WINRECONFIG_EARLY_APPLY_MAX_MS {
            winreconfig_finish(
                WINRECONFIG_RESULT_NO_WINDOW,
                start.elapsed().as_millis(),
                "game window never became visible within budget",
            );
            return;
        }
        let _ = tick_rx.recv_timeout(poll);
    };

    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if monitor.is_invalid() || !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        winreconfig_finish(
            WINRECONFIG_RESULT_NO_MONITOR,
            start.elapsed().as_millis(),
            "MonitorFromWindow/GetMonitorInfoW failed",
        );
        return;
    }
    let target = info.rcMonitor;
    let width = target.right - target.left;
    let height = target.bottom - target.top;
    WINRECONFIG_EARLY_APPLY_RECT.store(
        (((width as u32 as usize) << 16) | (height as u32 as usize & 0xffff)).min(usize::MAX),
        Ordering::SeqCst,
    );

    let mut current = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut current) }.is_ok()
        && current.left == target.left
        && current.top == target.top
        && current.right == target.right
        && current.bottom == target.bottom
    {
        winreconfig_finish(
            WINRECONFIG_RESULT_ALREADY_FINAL,
            start.elapsed().as_millis(),
            "window already at the monitor rect",
        );
        return;
    }

    // Mirror the game's own +11s sequence exactly (resize + FRAMECHANGED reposition), just early.
    // SWP_NOACTIVATE | SWP_NOOWNERZORDER: our EARLY reposition must NOT activate/foreground the game window.
    // Without SWP_NOACTIVATE, Windows activates the target as a side effect of SetWindowPos, so this early
    // apply (which fires up to a few times during boot) yanked focus to the game -- user-reported 2026-07-15
    // "the DLL is changing focus". We only relocate the window; the game's own launch activation is untouched.
    let move_ok = unsafe { MoveWindow(hwnd, target.left, target.top, width, height, true) }.is_ok();
    let pos_ok = unsafe {
        SetWindowPos(
            hwnd,
            None,
            target.left,
            target.top,
            width,
            height,
            SWP_NOZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
    }
    .is_ok();
    winreconfig_finish(
        WINRECONFIG_RESULT_APPLIED,
        start.elapsed().as_millis(),
        &format!(
            "monitor rect ({},{} {width}x{height}) applied early (move_ok={move_ok} pos_ok={pos_ok}); the game's own reconfig should now be a no-op",
            target.left, target.top,
        ),
    );
}
