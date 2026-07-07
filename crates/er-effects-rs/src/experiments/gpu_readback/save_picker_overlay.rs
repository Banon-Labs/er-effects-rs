// DLL-drawn startup save-file picker overlay.
//
// At a no-save boot the game's menu assets are NOT ready to draw an in-game menu, and the whole
// title is deliberately suppressed (black), so the save picker cannot be a native game menu. Like
// the old OS file dialog, this picker is drawn and driven by the DLL itself -- on the SAME D3D12
// Present-composite layer as the boot loading bar (`boot_progress.rs`), with input read directly
// from the OS (independent of the game's frozen/blocked input path). The boot stays held before
// the save-check/continue (the `SetState(4/5)` deny in `title_tick_cover.rs`), so every
// save-dependent thread waits; picking a file installs the redirect, releases the hold, and the
// normal autoload resumes.
//
// This shares the `boot_progress` module namespace (both are `include!`d into `gpu_readback.rs`),
// so it uses the boot glyph/rasterize helpers directly and the picker model via the crate
// re-exports. `GetModuleHandleA`/`PCSTR` are already in module scope (resource_readback.rs); only
// `GetProcAddress` needs importing here.

use windows::Win32::System::LibraryLoader::GetProcAddress;

/// 1 once the startup overlay picker has opened its model for this pending no-save boot. Distinct
/// from `SAVE_PICKER_MODE_ACTIVE` (the in-world System>Quit native-window picker).
pub(crate) static SAVE_PICKER_OVERLAY_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Previous frame's pressed-action bitmask (edge detection; see `PickerAction`).
static SAVE_PICKER_OVERLAY_PREV_ACTIONS: AtomicUsize = AtomicUsize::new(0);
/// Cached `user32!GetAsyncKeyState` / `xinput!XInputGetState` resolutions (0 = unresolved, !0 = tried-and-absent).
static GET_ASYNC_KEY_STATE_PROC: AtomicUsize = AtomicUsize::new(0);
static XINPUT_GET_STATE_PROC: AtomicUsize = AtomicUsize::new(0);
/// Telemetry oracles.
pub(crate) static SAVE_PICKER_OVERLAY_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_OVERLAY_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_OVERLAY_INPUT_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_OVERLAY_PICK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_OVERLAY_PICK_REJECT_COUNT: AtomicUsize = AtomicUsize::new(0);

const PROC_ABSENT: usize = usize::MAX;

// Edge-triggered logical actions (one step per press).
const PICKER_ACT_UP: usize = 1 << 0;
const PICKER_ACT_DOWN: usize = 1 << 1;
const PICKER_ACT_LEFT: usize = 1 << 2;
const PICKER_ACT_RIGHT: usize = 1 << 3;
const PICKER_ACT_SELECT: usize = 1 << 4;
const PICKER_ACT_BACK: usize = 1 << 5;

// Virtual-key codes (win32).
const VK_BACK: i32 = 0x08;
const VK_RETURN: i32 = 0x0d;
const VK_LEFT: i32 = 0x25;
const VK_UP: i32 = 0x26;
const VK_RIGHT: i32 = 0x27;
const VK_DOWN: i32 = 0x28;

// XInput gamepad button bits.
const XINPUT_DPAD_UP: u16 = 0x0001;
const XINPUT_DPAD_DOWN: u16 = 0x0002;
const XINPUT_DPAD_LEFT: u16 = 0x0004;
const XINPUT_DPAD_RIGHT: u16 = 0x0008;
const XINPUT_A: u16 = 0x1000;
const XINPUT_B: u16 = 0x2000;

/// True while the DLL-drawn startup picker owns the screen (draw + input). Gated on the same
/// missing-save-pending latch that holds the boot.
pub(crate) fn save_picker_overlay_active() -> bool {
    SAVE_PICKER_OVERLAY_ARMED.load(Ordering::SeqCst) != 0 && missing_save_selection_pending()
}

type GetAsyncKeyStateFn = unsafe extern "system" fn(i32) -> i16;
type XInputGetStateFn = unsafe extern "system" fn(u32, *mut XInputStateRaw) -> u32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputGamepadRaw {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    thumb_lx: i16,
    thumb_ly: i16,
    thumb_rx: i16,
    thumb_ry: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputStateRaw {
    packet: u32,
    gamepad: XInputGamepadRaw,
}

fn resolve_get_async_key_state() -> Option<GetAsyncKeyStateFn> {
    let cached = GET_ASYNC_KEY_STATE_PROC.load(Ordering::SeqCst);
    if cached == PROC_ABSENT {
        return None;
    }
    if cached != 0 {
        return Some(unsafe { std::mem::transmute::<usize, GetAsyncKeyStateFn>(cached) });
    }
    let addr = unsafe { GetModuleHandleA(PCSTR(b"user32.dll\0".as_ptr())) }
        .ok()
        .and_then(|m| unsafe { GetProcAddress(m, PCSTR(b"GetAsyncKeyState\0".as_ptr())) })
        .map(|p| p as usize);
    match addr {
        Some(a) if a != 0 => {
            GET_ASYNC_KEY_STATE_PROC.store(a, Ordering::SeqCst);
            Some(unsafe { std::mem::transmute::<usize, GetAsyncKeyStateFn>(a) })
        }
        _ => {
            GET_ASYNC_KEY_STATE_PROC.store(PROC_ABSENT, Ordering::SeqCst);
            None
        }
    }
}

fn resolve_xinput_get_state() -> Option<XInputGetStateFn> {
    let cached = XINPUT_GET_STATE_PROC.load(Ordering::SeqCst);
    if cached == PROC_ABSENT {
        return None;
    }
    if cached != 0 {
        return Some(unsafe { std::mem::transmute::<usize, XInputGetStateFn>(cached) });
    }
    // The game loads XInput for its own gamepad support, so GetModuleHandleA resolves it without a
    // LoadLibrary; if absent (keyboard-only session), gamepad nav is simply unavailable.
    for dll in [
        b"xinput1_4.dll\0".as_slice(),
        b"xinput1_3.dll\0",
        b"xinput9_1_0.dll\0",
    ] {
        let Ok(module) = (unsafe { GetModuleHandleA(PCSTR(dll.as_ptr())) }) else {
            continue;
        };
        if let Some(proc) = unsafe { GetProcAddress(module, PCSTR(b"XInputGetState\0".as_ptr())) } {
            let a = proc as usize;
            XINPUT_GET_STATE_PROC.store(a, Ordering::SeqCst);
            return Some(unsafe { std::mem::transmute::<usize, XInputGetStateFn>(a) });
        }
    }
    XINPUT_GET_STATE_PROC.store(PROC_ABSENT, Ordering::SeqCst);
    None
}

/// Sample keyboard + gamepad into the logical-action bitmask (which actions are currently held).
fn save_picker_sample_actions() -> usize {
    let mut held = 0usize;
    if let Some(gaks) = resolve_get_async_key_state() {
        // High bit set => key is currently down.
        let down = |vk: i32| (unsafe { gaks(vk) } as u16 & 0x8000) != 0;
        if down(VK_UP) {
            held |= PICKER_ACT_UP;
        }
        if down(VK_DOWN) {
            held |= PICKER_ACT_DOWN;
        }
        if down(VK_LEFT) {
            held |= PICKER_ACT_LEFT;
        }
        if down(VK_RIGHT) {
            held |= PICKER_ACT_RIGHT;
        }
        if down(VK_RETURN) {
            held |= PICKER_ACT_SELECT;
        }
        if down(VK_BACK) {
            held |= PICKER_ACT_BACK;
        }
    }
    if let Some(xinput) = resolve_xinput_get_state() {
        let mut st = XInputStateRaw::default();
        // Only controller 0; ERROR_SUCCESS(0) == connected.
        if unsafe { xinput(0, &mut st) } == 0 {
            let b = st.gamepad.buttons;
            if b & XINPUT_DPAD_UP != 0 {
                held |= PICKER_ACT_UP;
            }
            if b & XINPUT_DPAD_DOWN != 0 {
                held |= PICKER_ACT_DOWN;
            }
            if b & XINPUT_DPAD_LEFT != 0 {
                held |= PICKER_ACT_LEFT;
            }
            if b & XINPUT_DPAD_RIGHT != 0 {
                held |= PICKER_ACT_RIGHT;
            }
            if b & XINPUT_A != 0 {
                held |= PICKER_ACT_SELECT;
            }
            if b & XINPUT_B != 0 {
                held |= PICKER_ACT_BACK;
            }
        }
    }
    held
}

/// Open the picker model for the pending no-save boot if not already armed. Idempotent.
fn save_picker_overlay_arm_if_pending() {
    if !missing_save_selection_pending() || SAVE_PICKER_OVERLAY_ARMED.load(Ordering::SeqCst) != 0 {
        return;
    }
    let extension = if save_picker_seamless_mode_after_settle("startup-overlay-picker") {
        "co2"
    } else {
        "sl2"
    };
    let start_dir = save_picker_title_start_dir();
    let model = crate::experiments::save_picker::SavePickerModel::open(&start_dir, extension);
    *crate::experiments::save_picker::active_save_picker_lock() = Some(model);
    SAVE_PICKER_OVERLAY_ARMED.store(1, Ordering::SeqCst);
    SAVE_PICKER_OVERLAY_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OVERLAY_PREV_ACTIONS.store(0, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker-overlay: opened DLL-drawn startup picker dir='{}' ext=.{extension}",
        start_dir.display()
    ));
}

/// Disarm the overlay (pick completed / no longer pending): drop the model and reset edge state.
fn save_picker_overlay_disarm(reason: &str) {
    if SAVE_PICKER_OVERLAY_ARMED.swap(0, Ordering::SeqCst) == 0 {
        return;
    }
    // The startup overlay and the in-world System>Quit picker are mutually exclusive (startup
    // resolves before the world is reachable), so the overlay owns the shared model slot.
    *crate::experiments::save_picker::active_save_picker_lock() = None;
    SAVE_PICKER_OVERLAY_PREV_ACTIONS.store(0, Ordering::SeqCst);
    append_autoload_debug(format_args!("save-picker-overlay: disarmed ({reason})"));
}

/// Per-frame input drive for the startup overlay picker. Reads OS keyboard/gamepad directly
/// (independent of the game's blocked input), edge-detects, and drives the model. Call from the
/// game task tick. No-op unless the overlay is active.
pub(crate) fn save_picker_overlay_input_tick() {
    save_picker_overlay_arm_if_pending();
    if !save_picker_overlay_active() {
        // No longer pending -> the pick released the hold; drop the model.
        save_picker_overlay_disarm("not-pending");
        return;
    }
    let held = save_picker_sample_actions();
    let prev = SAVE_PICKER_OVERLAY_PREV_ACTIONS.swap(held, Ordering::SeqCst);
    let pressed = held & !prev; // rising edges only
    if pressed == 0 {
        return;
    }
    SAVE_PICKER_OVERLAY_INPUT_HITS.fetch_add(1, Ordering::SeqCst);

    // Resolve the activation (if any) under the lock, then act on it OUTSIDE the lock so the
    // completion path (which re-locks) never deadlocks.
    enum Act {
        None,
        Picked(std::path::PathBuf),
    }
    let act = {
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_mut() else {
            return;
        };
        if pressed & PICKER_ACT_UP != 0 {
            model.move_cursor(false);
        }
        if pressed & PICKER_ACT_DOWN != 0 {
            model.move_cursor(true);
        }
        if pressed & PICKER_ACT_LEFT != 0 {
            model.cycle_page(false);
        }
        if pressed & PICKER_ACT_RIGHT != 0 {
            model.cycle_page(true);
        }
        if pressed & PICKER_ACT_BACK != 0 {
            model.go_up();
        }
        if pressed & PICKER_ACT_SELECT != 0 {
            match model.activate_cursor() {
                crate::experiments::save_picker::PickerActivation::PickedFile(path) => {
                    Act::Picked(path)
                }
                _ => Act::None,
            }
        } else {
            Act::None
        }
    };

    if let Act::Picked(path) = act {
        if crate::experiments::complete_missing_save_selection_from_picker(&path) {
            SAVE_PICKER_OVERLAY_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-overlay: picked '{}' -- redirect active, releasing the save-check hold",
                path.display()
            ));
            save_picker_overlay_disarm("picked");
        } else {
            // Invalid container: stay in the picker so the user can choose another file.
            SAVE_PICKER_OVERLAY_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }
}

// ---- Rendering ----

// Overlay palette (reuses the boot bar's understated language; dark panel, off-white highlight).
const PICKER_RGB_BG: [u8; 3] = [8, 8, 9];
const PICKER_RGB_TITLE: [u8; 3] = [214, 208, 190];
const PICKER_RGB_DIM: [u8; 3] = [120, 117, 108];
const PICKER_RGB_ROW: [u8; 3] = [176, 172, 160];
const PICKER_RGB_SEL_BAR: [u8; 3] = [58, 54, 44];
const PICKER_RGB_SEL_TEXT: [u8; 3] = [238, 232, 214];
const PICKER_RGB_RULE: [u8; 3] = [40, 38, 33];

/// Truncate `text` so it fits within `max_px` at the boot font's scale (drops the tail; keeps a
/// trailing marker when clipped).
fn picker_fit_text(text: &str, max_px: usize) -> String {
    let adv = BOOT_VIEW_GLYPH_ADV * BOOT_VIEW_TEXT_SCALE;
    let max_chars = (max_px / adv).max(1);
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('>');
    out
}

/// Rasterize the full-frame file browser into `buf` (w*h RGBA8). Reads the live model; safe to
/// call from the render thread (pure read + CPU raster). Returns false if there is no model.
pub(crate) fn rasterize_save_picker_overlay(buf: &mut [u8], w: usize, h: usize) -> bool {
    let guard = crate::experiments::save_picker::active_save_picker_lock();
    let Some(model) = guard.as_ref() else {
        return false;
    };

    boot_fill_rect(buf, w, h, 0, 0, w, h, PICKER_RGB_BG);

    let scale = BOOT_VIEW_TEXT_SCALE;
    let line_h = BOOT_VIEW_GLYPH_H * scale;
    let row_step = line_h + line_h / 2; // 1.5 line spacing between rows
    let margin_x = (w / 12).max(24);
    let content_w = w.saturating_sub(margin_x * 2);
    let mut y = (h / 8).max(40);

    // Title.
    boot_draw_text_rgb(buf, w, h, margin_x, y, "SELECT SAVE FILE", PICKER_RGB_TITLE);
    y += line_h + line_h / 2;

    // Current directory (dimmed, fit to width) + extension hint.
    let dir_str = model.current_dir().display().to_string();
    let dir_line = picker_fit_text(&dir_str, content_w);
    boot_draw_text_rgb(buf, w, h, margin_x, y, &dir_line, PICKER_RGB_DIM);
    y += line_h;
    let mode_line = format!(
        "SHOWING *.{}   PAGE {}/{}",
        model.extension().to_ascii_uppercase(),
        model.page() + 1,
        model.page_count()
    );
    boot_draw_text_rgb(buf, w, h, margin_x, y, &mode_line, PICKER_RGB_DIM);
    y += line_h;
    // Divider rule.
    boot_fill_rect(buf, w, h, margin_x, y, content_w, scale.max(1), PICKER_RGB_RULE);
    y += line_h;

    // Rows.
    let cursor = model.cursor();
    for row in 0..crate::experiments::save_picker::PICKER_ROW_COUNT {
        let label = model.row_label_ascii(row);
        if label.is_empty() {
            continue;
        }
        let selected = row == cursor;
        if selected {
            boot_fill_rect(
                buf,
                w,
                h,
                margin_x.saturating_sub(scale * 4),
                y.saturating_sub(scale * 2),
                content_w + scale * 8,
                line_h + scale * 4,
                PICKER_RGB_SEL_BAR,
            );
        }
        let (color, prefix) = if selected {
            (PICKER_RGB_SEL_TEXT, "> ")
        } else {
            (PICKER_RGB_ROW, "  ")
        };
        let text = picker_fit_text(&format!("{prefix}{label}"), content_w);
        boot_draw_text_rgb(buf, w, h, margin_x, y, &text, color);
        y += row_step;
        if y + line_h >= h.saturating_sub(margin_x) {
            break;
        }
    }

    // Footer hint.
    let footer_y = h.saturating_sub((h / 10).max(40));
    boot_fill_rect(buf, w, h, margin_x, footer_y.saturating_sub(line_h), content_w, scale.max(1), PICKER_RGB_RULE);
    boot_draw_text_rgb(
        buf,
        w,
        h,
        margin_x,
        footer_y,
        "ARROWS/DPAD MOVE   ENTER/A SELECT   BKSP/B UP   L/R PAGE",
        PICKER_RGB_DIM,
    );
    true
}
