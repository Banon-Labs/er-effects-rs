//! Tier A: has this process got a real, sized, top-level window yet?
//!
//! # Why an overlay has to ask
//!
//! hudhook's D3D12 pipeline is built on the first `Present` it intercepts, and building it starts
//! with `GetClientRect(swap_chain.GetDesc().OutputWindow).unwrap()`. Install the Present hook
//! before ELDEN RING has its window and the first Present you intercept can carry an
//! `OutputWindow` that is not a live window -- `GetClientRect` fails with `0x80070578`
//! ERROR_INVALID_WINDOW_HANDLE, the unwrap panics, and the panic unwinds out of an
//! `extern "system"` callback, which is an abort.
//!
//! Measured, 2026-08-29, twice, in different modules and therefore not a property of either:
//! `er_build_watermark.dll` died that way 229 ms after the first backbuffer draw; with that shell
//! excluded, `er_net_effects.dll` -- the next module to win the overlay-host claim -- died the
//! same way at +2060 ms. A third run with the same binaries did not die and reached a mapped
//! 3072x1712 window, which is what makes it a race against window creation rather than a
//! systematic failure, and what makes waiting the fix.
//!
//! # What it does not claim
//!
//! A window existing is not the game being ready, and this deliberately says nothing about the
//! swapchain, the device, or whether anything has been drawn. It answers exactly one question --
//! is there a visible top-level window belonging to this process with a non-empty client area --
//! because that is the precondition the `GetClientRect` above actually needs.

#[cfg(windows)]
use core::ffi::c_void;

/// `RECT`: four LONGs, and the only Win32 struct this module needs.
#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

// user32 is not linked by default the way kernel32 is, so the window calls need the attribute or
// the DLLs fail at link with `undefined symbol: EnumWindows`.
#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(isize, isize) -> i32, param: isize) -> i32;
    fn GetWindowThreadProcessId(window: isize, process_id: *mut u32) -> u32;
    fn IsWindowVisible(window: isize) -> i32;
    fn GetClientRect(window: isize, rect: *mut Rect) -> i32;
}

#[cfg(windows)]
unsafe extern "system" {
    fn GetCurrentProcessId() -> u32;
}

/// What [`enum_callback`] is filling in: the first qualifying window it finds.
#[cfg(windows)]
struct Search {
    process_id: u32,
    found: isize,
}

/// `EnumWindows` callback. Returns 0 to stop enumerating once a window qualifies.
///
/// # Safety
///
/// Called by `EnumWindows` with the `param` handed to it, which is always a `&mut Search` that
/// outlives the call below.
#[cfg(windows)]
unsafe extern "system" fn enum_callback(window: isize, param: isize) -> i32 {
    const CONTINUE_ENUM: i32 = 1;
    const STOP_ENUM: i32 = 0;
    let search = unsafe { &mut *(param as *mut Search) };
    let mut owner = 0_u32;
    unsafe { GetWindowThreadProcessId(window, &raw mut owner) };
    if owner != search.process_id || unsafe { IsWindowVisible(window) } == 0 {
        return CONTINUE_ENUM;
    }
    let mut rect = Rect::default();
    // A zero-sized client area is a window that exists but cannot be rendered into, which is the
    // same problem in a costume: hudhook would set imgui's display size to 0x0.
    if unsafe { GetClientRect(window, &raw mut rect) } == 0
        || rect.right - rect.left <= 0
        || rect.bottom - rect.top <= 0
    {
        return CONTINUE_ENUM;
    }
    search.found = window;
    STOP_ENUM
}

/// A visible, non-empty top-level window belonging to this process, if there is one.
#[cfg(windows)]
pub fn process_window() -> Option<isize> {
    let mut search = Search {
        process_id: unsafe { GetCurrentProcessId() },
        found: 0,
    };
    let param = (&raw mut search) as *mut c_void as isize;
    unsafe { EnumWindows(enum_callback, param) };
    (search.found != 0).then_some(search.found)
}

/// Block until this process has such a window, or give up.
///
/// Returns `true` if one appeared. The wait is bounded by [`crate::wait::poll_until`] and spins in
/// user space, so a caller that waits for a window that never comes costs the process nothing --
/// the whole reason that helper exists.
#[cfg(windows)]
pub fn wait_for_process_window() -> bool {
    crate::wait::poll_until(process_window).is_some()
}

/// Host builds have no windows and no game.
#[cfg(not(windows))]
pub fn wait_for_process_window() -> bool {
    false
}
