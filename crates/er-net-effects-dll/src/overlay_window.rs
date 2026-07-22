use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BACKGROUND_MODE, BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HGDIOBJ,
    InvalidateRect, PAINTSTRUCT, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows, GWLP_USERDATA,
    GetForegroundWindow, GetMessageW, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    HWND_TOPMOST, IsWindowVisible, KillTimer, LWA_ALPHA, MSG, RegisterClassW, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetTimer,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_CREATE,
    WM_DESTROY, WM_NCDESTROY, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

use crate::{effects::effect_selector_text, log::net_effects_log};

const OVERLAY_W: i32 = 980;
const OVERLAY_H: i32 = 92;
const OVERLAY_MARGIN_X: i32 = 28;
const OVERLAY_MARGIN_Y: i32 = 28;
const OVERLAY_TIMER_ID: usize = 1;
const OVERLAY_TIMER_MS: u32 = 66;

static OVERLAY_THREAD_STARTED: AtomicBool = AtomicBool::new(false);
static OVERLAY_HWND: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_RENDER_TEXT: OnceLock<Mutex<String>> = OnceLock::new();

pub(crate) fn start_overlay_window_thread() {
    if OVERLAY_THREAD_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("er-net-effects-overlay".to_owned())
        .spawn(overlay_thread_main);
    if spawned.is_err() {
        OVERLAY_THREAD_STARTED.store(false, Ordering::SeqCst);
        net_effects_log(format_args!(
            "overlay-window: failed to spawn overlay thread"
        ));
    }
}

fn overlay_thread_main() {
    let result = unsafe { overlay_window_loop() };
    if let Err(error) = result {
        OVERLAY_THREAD_STARTED.store(false, Ordering::SeqCst);
        net_effects_log(format_args!("overlay-window: {error}"));
    }
}

unsafe fn overlay_window_loop() -> Result<(), String> {
    let hinstance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| format!("GetModuleHandleW failed: {error:?}"))?;
    let class_name = w!("ErNetEffectsOverlayWindow");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(overlay_wndproc),
        hInstance: HINSTANCE(hinstance.0),
        lpszClassName: class_name,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            class_name,
            w!("er-net-effects overlay"),
            WS_POPUP,
            40,
            40,
            OVERLAY_W,
            OVERLAY_H,
            None,
            None,
            Some(HINSTANCE(hinstance.0)),
            None,
        )
    }
    .map_err(|error| format!("CreateWindowExW failed: {error:?}"))?;

    OVERLAY_HWND.store(hwnd.0 as usize, Ordering::SeqCst);
    let _ = unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 232, LWA_ALPHA) };
    let timer = unsafe { SetTimer(Some(hwnd), OVERLAY_TIMER_ID, OVERLAY_TIMER_MS, None) };
    if timer == 0 {
        return Err("SetTimer failed".to_owned());
    }
    net_effects_log(format_args!(
        "overlay-window: created hwnd=0x{:x}",
        hwnd.0 as usize
    ));

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let _ = unsafe { KillTimer(Some(hwnd), OVERLAY_TIMER_ID) };
    Ok(())
}

unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let _ = lparam.0 as *const CREATESTRUCTW;
            LRESULT(0)
        }
        WM_TIMER => {
            update_overlay_window(hwnd);
            LRESULT(0)
        }
        WM_PAINT => {
            paint_overlay_window(hwnd);
            LRESULT(0)
        }
        WM_DESTROY | WM_NCDESTROY => {
            OVERLAY_HWND.store(0, Ordering::SeqCst);
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn update_overlay_window(hwnd: HWND) {
    let text = effect_selector_text();
    if text.is_empty() {
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        return;
    }

    let mut changed = false;
    if let Ok(mut slot) = OVERLAY_RENDER_TEXT
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
        && *slot != text
    {
        *slot = text;
        changed = true;
    }

    let (x, y) = overlay_position(hwnd).unwrap_or((40, 40));
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            OVERLAY_W,
            OVERLAY_H,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    if changed {
        let _ = unsafe { InvalidateRect(Some(hwnd), None, true) };
    }
}

fn overlay_position(overlay_hwnd: HWND) -> Option<(i32, i32)> {
    let game_hwnd = find_current_process_window(overlay_hwnd).or_else(|| {
        let foreground = unsafe { GetForegroundWindow() };
        (!foreground.0.is_null() && foreground != overlay_hwnd).then_some(foreground)
    })?;
    let mut rect = RECT::default();
    unsafe { GetWindowRect(game_hwnd, &mut rect) }.ok()?;
    Some((
        rect.left.saturating_add(OVERLAY_MARGIN_X),
        rect.top.saturating_add(OVERLAY_MARGIN_Y),
    ))
}

struct WindowSearch {
    process_id: u32,
    overlay_hwnd: HWND,
    found: HWND,
}

fn find_current_process_window(overlay_hwnd: HWND) -> Option<HWND> {
    let mut search = WindowSearch {
        process_id: unsafe { GetCurrentProcessId() },
        overlay_hwnd,
        found: HWND(std::ptr::null_mut()),
    };
    let search_ptr = &mut search as *mut WindowSearch as isize;
    let _ = unsafe { EnumWindows(Some(enum_window_proc), LPARAM(search_ptr)) };
    (!search.found.0.is_null()).then_some(search.found)
}

unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    if lparam.0 == 0 {
        return true.into();
    }
    let search = unsafe { &mut *(lparam.0 as *mut WindowSearch) };
    if hwnd == search.overlay_hwnd || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return true.into();
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == search.process_id {
        search.found = hwnd;
        return false.into();
    }
    true.into()
}

fn paint_overlay_window(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    if hdc.0.is_null() {
        return;
    }

    let background = unsafe { CreateSolidBrush(COLORREF(0x00201810)) };
    let rect = RECT {
        left: 0,
        top: 0,
        right: OVERLAY_W,
        bottom: OVERLAY_H,
    };
    let _ = unsafe { FillRect(hdc, &rect, background) };
    let _ = unsafe { SetBkMode(hdc, BACKGROUND_MODE(TRANSPARENT.0)) };
    let _ = unsafe { SetTextColor(hdc, COLORREF(0x00E2DFD6)) };

    let text = OVERLAY_RENDER_TEXT
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
        .map(|slot| slot.clone())
        .unwrap_or_default();
    let lines = overlay_lines(&text);
    for (index, line) in lines.iter().enumerate() {
        let wide = line.encode_utf16().collect::<Vec<_>>();
        let _ = unsafe { TextOutW(hdc, 18, 16 + (index as i32 * 23), &wide) };
    }

    let _ = unsafe { DeleteObject(HGDIOBJ(background.0)) };
    let _ = unsafe { EndPaint(hwnd, &ps) };
}

fn overlay_lines(text: &str) -> Vec<String> {
    let parts = text.split(" | ").collect::<Vec<_>>();
    if parts.len() >= 4 {
        let mut lines = vec![
            parts[0].to_owned(),
            format!("{} {} {}", parts[1], parts[2], parts[3]),
        ];
        if parts.len() > 4 {
            lines.push(parts[4..].join(" "));
        }
        lines
    } else {
        vec![text.to_owned()]
    }
}
