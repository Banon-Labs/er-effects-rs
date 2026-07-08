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
/// Diagnostics for the "inputs eaten during load" report: total input polls the dedicated thread ran
/// (proves the thread is alive and at cadence, independent of the ~4 fps Present redraw), and polls
/// where ANY navigation key/button was down (proves the background thread can actually READ OS input
/// under Wine/Proton -- if this stays ~0 while the user mashes, a background thread cannot see the
/// keys and input must move back to a pumped thread).
pub(crate) static SAVE_PICKER_OVERLAY_POLL_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_OVERLAY_HELD_POLLS: AtomicUsize = AtomicUsize::new(0);

/// Overlay stage: 0 = browsing files, 1 = choosing a character (save slot) from the picked file.
static SAVE_PICKER_STAGE_CHARS: AtomicUsize = AtomicUsize::new(0);
/// Highlighted row in the character sub-picker.
static SAVE_PICKER_CHAR_CURSOR: AtomicUsize = AtomicUsize::new(0);
/// The autoload slot the character sub-picker chose (`usize::MAX` = none yet). The product-core
/// callsite reads this as the load target when no slot is configured.
pub(crate) static MISSING_SAVE_PICKER_SELECTED_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// The picked save awaiting a character selection: its path and the active character slots parsed
/// from its bytes.
struct PendingSave {
    path: std::path::PathBuf,
    slots: Vec<crate::experiments::SaveSlotInfo>,
}
static SAVE_PICKER_PENDING_SAVE: Mutex<Option<PendingSave>> = Mutex::new(None);

fn pending_save_lock() -> std::sync::MutexGuard<'static, Option<PendingSave>> {
    SAVE_PICKER_PENDING_SAVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A character pick awaiting completion `(save path, slot)`. Set by the Present-hook input on the
/// render thread; consumed by [`save_picker_overlay_process_completion`] on the game-task thread so
/// the redirect activation + MinHook install runs off the render thread.
#[allow(clippy::type_complexity)]
static SAVE_PICKER_COMPLETE_REQUEST: Mutex<Option<(std::path::PathBuf, usize)>> = Mutex::new(None);

fn save_picker_complete_request_lock()
-> std::sync::MutexGuard<'static, Option<(std::path::PathBuf, usize)>> {
    SAVE_PICKER_COMPLETE_REQUEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Consume a pending character pick and complete it: activate the save redirect + install the
/// redirect hooks + release the save-check hold, so the autoload loads the chosen character. Call
/// from the game-task thread (safe for MinHook, and alive at pick time before loading starts).
/// No-op when no pick is pending.
pub(crate) fn save_picker_overlay_process_completion() {
    let request = save_picker_complete_request_lock().take();
    let Some((path, slot)) = request else {
        return;
    };
    if crate::experiments::complete_missing_save_selection_from_picker(&path) {
        SAVE_PICKER_OVERLAY_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-overlay: completed pick '{}' slot {slot} -- redirect active, releasing the save-check hold to autoload that character",
            path.display()
        ));
        save_picker_overlay_disarm("picked");
    } else {
        // Validation failed at commit -- back to the file browser.
        MISSING_SAVE_PICKER_SELECTED_SLOT.store(usize::MAX, Ordering::SeqCst);
        SAVE_PICKER_OVERLAY_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
        *pending_save_lock() = None;
        SAVE_PICKER_STAGE_CHARS.store(0, Ordering::SeqCst);
    }
}

/// The character sub-picker's chosen autoload slot, if one has been picked this session.
pub(crate) fn missing_save_picker_selected_slot() -> Option<i32> {
    let v = MISSING_SAVE_PICKER_SELECTED_SLOT.load(Ordering::SeqCst);
    (v != usize::MAX).then_some(v as i32)
}

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

/// Sample keyboard + gamepad. Returns `(held_now, pressed_this_poll)`.
///
/// Keyboard "pressed" uses the LOW bit of `GetAsyncKeyState` ("pressed since our previous call"), so
/// a press is caught even when it happened AND was released between two of the slow (~4 fps)
/// boot-frame polls -- polling only the high bit drops those, which is why deliberate navigation felt
/// eaten. Gamepad has no such bit, so it edge-detects the button state vs the previous poll.
///
/// MUST be called on the game's render thread (the Present hook). `GetAsyncKeyState` does not report
/// the user's keys from a background thread under Wine/Proton -- measured: a dedicated poll thread ran
/// 1089 polls yet saw only 5 key-downs while the user mashed, and completed 0 picks.
fn save_picker_sample() -> (usize, usize) {
    let mut held = 0usize;
    let mut pressed = 0usize;
    if let Some(gaks) = resolve_get_async_key_state() {
        let mut probe = |vk: i32, act: usize| {
            let state = unsafe { gaks(vk) } as u16;
            if state & 0x8000 != 0 {
                held |= act; // currently down
            }
            if state & 0x0001 != 0 {
                pressed |= act; // pressed since our previous poll
            }
        };
        probe(VK_UP, PICKER_ACT_UP);
        probe(VK_DOWN, PICKER_ACT_DOWN);
        probe(VK_LEFT, PICKER_ACT_LEFT);
        probe(VK_RIGHT, PICKER_ACT_RIGHT);
        probe(VK_RETURN, PICKER_ACT_SELECT);
        probe(VK_BACK, PICKER_ACT_BACK);
    }
    if let Some(xinput) = resolve_xinput_get_state() {
        let mut st = XInputStateRaw::default();
        // Only controller 0; ERROR_SUCCESS(0) == connected.
        if unsafe { xinput(0, &mut st) } == 0 {
            let b = st.gamepad.buttons;
            let mut gamepad = 0usize;
            if b & XINPUT_DPAD_UP != 0 {
                gamepad |= PICKER_ACT_UP;
            }
            if b & XINPUT_DPAD_DOWN != 0 {
                gamepad |= PICKER_ACT_DOWN;
            }
            if b & XINPUT_DPAD_LEFT != 0 {
                gamepad |= PICKER_ACT_LEFT;
            }
            if b & XINPUT_DPAD_RIGHT != 0 {
                gamepad |= PICKER_ACT_RIGHT;
            }
            if b & XINPUT_A != 0 {
                gamepad |= PICKER_ACT_SELECT;
            }
            if b & XINPUT_B != 0 {
                gamepad |= PICKER_ACT_BACK;
            }
            held |= gamepad;
            let prev = SAVE_PICKER_OVERLAY_PREV_ACTIONS.swap(gamepad, Ordering::SeqCst);
            pressed |= gamepad & !prev; // rising edges only
        }
    }
    (held, pressed)
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
    // Reset the character sub-picker stage (the chosen slot in MISSING_SAVE_PICKER_SELECTED_SLOT is
    // intentionally left set -- the autoload callsite still needs it to load the picked character).
    SAVE_PICKER_STAGE_CHARS.store(0, Ordering::SeqCst);
    SAVE_PICKER_CHAR_CURSOR.store(0, Ordering::SeqCst);
    *pending_save_lock() = None;
    append_autoload_debug(format_args!("save-picker-overlay: disarmed ({reason})"));
}

/// One input poll for the startup overlay picker. Reads OS keyboard/gamepad directly (independent of
/// the game's blocked input) and captures presses. MUST run on the game's render thread -- it is
/// driven from the D3D12 Present hook, which is the only thread that can read `GetAsyncKeyState`
/// under Wine/Proton. Present starves to ~4 fps while the boot streams assets, so a press could fall
/// between two polls; [`save_picker_sample`] uses the GetAsyncKeyState "pressed-since-last-call" bit
/// so those presses are still caught (that dropping was the "inputs eaten" symptom). Navigation is
/// applied here (pure Mutex state); the one-shot pick COMPLETION (redirect + MinHook install) is
/// deferred to [`save_picker_overlay_process_completion`] on the game-task thread. No-op unless the
/// overlay is active.
pub(crate) fn save_picker_overlay_input_tick() {
    save_picker_overlay_arm_if_pending();
    if !save_picker_overlay_active() {
        // No longer pending -> the pick released the hold; drop the model.
        save_picker_overlay_disarm("not-pending");
        return;
    }
    SAVE_PICKER_OVERLAY_POLL_COUNT.fetch_add(1, Ordering::SeqCst);
    let (held, pressed) = save_picker_sample();
    if held != 0 {
        SAVE_PICKER_OVERLAY_HELD_POLLS.fetch_add(1, Ordering::SeqCst);
    }
    if pressed == 0 {
        return;
    }
    SAVE_PICKER_OVERLAY_INPUT_HITS.fetch_add(1, Ordering::SeqCst);

    if SAVE_PICKER_STAGE_CHARS.load(Ordering::SeqCst) != 0 {
        save_picker_character_stage_input(pressed);
    } else {
        save_picker_file_stage_input(pressed);
    }
}


/// File-browser stage input: navigate/drive/page, and on picking a save file, parse its character
/// slots and switch to the character sub-picker (the redirect + load are deferred until a
/// character is chosen).
fn save_picker_file_stage_input(pressed: usize) {
    let picked = {
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
        // Left/right cycle the DRIVE when the highlight is on the top drive-selector row, else
        // page through the current listing.
        if pressed & PICKER_ACT_LEFT != 0 {
            if model.cursor_on_drive_selector() {
                model.cycle_drive(false);
            } else {
                model.cycle_page(false);
            }
        }
        if pressed & PICKER_ACT_RIGHT != 0 {
            if model.cursor_on_drive_selector() {
                model.cycle_drive(true);
            } else {
                model.cycle_page(true);
            }
        }
        if pressed & PICKER_ACT_BACK != 0 {
            model.go_up();
        }
        if pressed & PICKER_ACT_SELECT != 0 {
            match model.activate_cursor() {
                crate::experiments::save_picker::PickerActivation::PickedFile(path) => Some(path),
                _ => None,
            }
        } else {
            None
        }
    };

    let Some(path) = picked else {
        return;
    };
    // Parse the picked save's active character slots (from its own bytes -- no dependency on the
    // game having built its ProfileSummary yet).
    let slots = std::fs::read(&path)
        .ok()
        .map(|bytes| crate::experiments::parse_save_character_slots(&bytes))
        .unwrap_or_default();
    if slots.is_empty() {
        // Not a readable save / no characters -- stay in the file browser.
        SAVE_PICKER_OVERLAY_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-overlay: '{}' has no readable character slots; staying in file browser",
            path.display()
        ));
        return;
    }
    append_autoload_debug(format_args!(
        "save-picker-overlay: selected save '{}' -- {} character slots; opening character sub-picker",
        path.display(),
        slots.len()
    ));
    *pending_save_lock() = Some(PendingSave { path, slots });
    SAVE_PICKER_CHAR_CURSOR.store(0, Ordering::SeqCst);
    SAVE_PICKER_STAGE_CHARS.store(1, Ordering::SeqCst);
}

/// Character sub-picker input: up/down move, back returns to the file browser, select commits the
/// chosen slot -- record it for the autoload, activate the redirect, and release the save-check
/// hold.
fn save_picker_character_stage_input(pressed: usize) {
    // Resolve the chosen slot + path under the lock, act (redirect/complete) outside it.
    enum Act {
        None,
        Back,
        Pick(std::path::PathBuf, usize),
    }
    let act = {
        let guard = pending_save_lock();
        let Some(pending) = guard.as_ref() else {
            // No pending save -> fall back to the file browser.
            SAVE_PICKER_STAGE_CHARS.store(0, Ordering::SeqCst);
            return;
        };
        let n = pending.slots.len().max(1);
        let mut cursor = SAVE_PICKER_CHAR_CURSOR.load(Ordering::SeqCst).min(n - 1);
        if pressed & PICKER_ACT_UP != 0 {
            cursor = (cursor + n - 1) % n;
        }
        if pressed & PICKER_ACT_DOWN != 0 {
            cursor = (cursor + 1) % n;
        }
        SAVE_PICKER_CHAR_CURSOR.store(cursor, Ordering::SeqCst);
        if pressed & PICKER_ACT_BACK != 0 {
            Act::Back
        } else if pressed & PICKER_ACT_SELECT != 0 {
            Act::Pick(pending.path.clone(), pending.slots[cursor].slot)
        } else {
            Act::None
        }
    };
    match act {
        Act::None => {}
        Act::Back => {
            *pending_save_lock() = None;
            SAVE_PICKER_STAGE_CHARS.store(0, Ordering::SeqCst);
        }
        Act::Pick(path, slot) => {
            // Defer the actual redirect activation + MinHook install to the game-task thread (via
            // this request): it runs the risky install off the render thread, and the game task is
            // alive at pick time (the boot is still HELD -- loading only starts once the pick
            // releases the hold). Record the chosen slot now so the character list stays selected.
            MISSING_SAVE_PICKER_SELECTED_SLOT.store(slot, Ordering::SeqCst);
            *save_picker_complete_request_lock() = Some((path, slot));
            append_autoload_debug(format_args!(
                "save-picker-overlay: character slot {slot} chosen; completion requested (game-task thread)"
            ));
        }
    }
}

// ---- Rendering ----

// Overlay palette (reuses the boot bar's understated language; dark panel, off-white highlight).
const PICKER_RGB_PANEL: [u8; 3] = [12, 12, 14];
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

/// Draw the file browser onto an EXISTING full-frame buffer (`w*h` RGBA8) that already holds the
/// boot loading bar at the bottom. The picker occupies a bounded panel in the upper region so the
/// game's own loading-bar language (bottom strip) stays visible underneath -- the picker is
/// composited WITH the bar, not in place of it. Reads the live model; render-thread safe (pure
/// read + CPU raster). Returns false if there is no model.
pub(crate) fn overlay_save_picker_onto(buf: &mut [u8], w: usize, h: usize) -> bool {
    let scale = BOOT_VIEW_TEXT_SCALE;
    let line_h = BOOT_VIEW_GLYPH_H * scale;
    let row_step = line_h + line_h / 2; // 1.5 line spacing between rows
    let margin_x = (w / 10).max(24);
    let content_w = w.saturating_sub(margin_x * 2);

    // Bounded panel: leave the bottom ~18% for the boot bar (drawn by the caller). A subtly
    // lifted-from-black fill reads as a panel over the suppressed black title.
    let panel_top = (h / 12).max(24);
    let panel_bottom = h * 82 / 100;
    let panel_h = panel_bottom.saturating_sub(panel_top);
    boot_fill_rect(buf, w, h, margin_x, panel_top, content_w, panel_h, PICKER_RGB_PANEL);

    // Stage two: choose which character (save slot) in the picked file to load.
    if SAVE_PICKER_STAGE_CHARS.load(Ordering::SeqCst) != 0 {
        return overlay_character_stage_onto(
            buf,
            w,
            h,
            margin_x,
            content_w,
            panel_top,
            panel_bottom,
        );
    }

    let guard = crate::experiments::save_picker::active_save_picker_lock();
    let Some(model) = guard.as_ref() else {
        return false;
    };

    let mut y = panel_top + line_h;

    // Title.
    boot_draw_text_rgb(buf, w, h, margin_x + scale * 4, y, "SELECT SAVE FILE", PICKER_RGB_TITLE);
    y += line_h + line_h / 2;

    // Location line (dimmed, fit to width): the current directory path.
    let loc_line = picker_fit_text(&model.location_label(), content_w.saturating_sub(scale * 8));
    boot_draw_text_rgb(buf, w, h, margin_x + scale * 4, y, &loc_line, PICKER_RGB_DIM);
    y += line_h;
    let mode_line = format!(
        "SHOWING *.{}   PAGE {}/{}",
        model.extension().to_ascii_uppercase(),
        model.page() + 1,
        model.page_count()
    );
    boot_draw_text_rgb(buf, w, h, margin_x + scale * 4, y, &mode_line, PICKER_RGB_DIM);
    y += line_h;
    // Divider rule.
    boot_fill_rect(buf, w, h, margin_x + scale * 4, y, content_w.saturating_sub(scale * 8), scale.max(1), PICKER_RGB_RULE);
    y += line_h;

    // Rows.
    let rows_bottom = panel_bottom.saturating_sub(line_h * 2);
    let cursor = model.cursor();
    for row in 0..crate::experiments::save_picker::PICKER_ROW_COUNT {
        let label = model.row_label_ascii(row);
        if label.is_empty() {
            continue;
        }
        if y + line_h >= rows_bottom {
            break;
        }
        let selected = row == cursor;
        if selected {
            boot_fill_rect(
                buf,
                w,
                h,
                margin_x + scale * 2,
                y.saturating_sub(scale * 2),
                content_w.saturating_sub(scale * 4),
                line_h + scale * 4,
                PICKER_RGB_SEL_BAR,
            );
        }
        let (color, prefix) = if selected {
            (PICKER_RGB_SEL_TEXT, "> ")
        } else {
            (PICKER_RGB_ROW, "  ")
        };
        let text = picker_fit_text(&format!("{prefix}{label}"), content_w.saturating_sub(scale * 8));
        boot_draw_text_rgb(buf, w, h, margin_x + scale * 6, y, &text, color);
        y += row_step;
    }

    // Footer hint inside the panel (above the bottom bar).
    let footer_y = panel_bottom.saturating_sub(line_h);
    boot_draw_text_rgb(
        buf,
        w,
        h,
        margin_x + scale * 4,
        footer_y,
        "UP/DN MOVE  L/R DRIVE (TOP ROW) OR PAGE  ENTER/A OPEN  BKSP/B UP",
        PICKER_RGB_DIM,
    );
    true
}

/// Draw the character sub-picker (stage two): the picked save's active characters, one per row.
fn overlay_character_stage_onto(
    buf: &mut [u8],
    w: usize,
    h: usize,
    margin_x: usize,
    content_w: usize,
    panel_top: usize,
    panel_bottom: usize,
) -> bool {
    let scale = BOOT_VIEW_TEXT_SCALE;
    let line_h = BOOT_VIEW_GLYPH_H * scale;
    let row_step = line_h + line_h / 2;
    let guard = pending_save_lock();
    let Some(pending) = guard.as_ref() else {
        return false;
    };

    let mut y = panel_top + line_h;
    boot_draw_text_rgb(buf, w, h, margin_x + scale * 4, y, "SELECT CHARACTER", PICKER_RGB_TITLE);
    y += line_h + line_h / 2;

    let name = pending
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_ascii_uppercase();
    let file_line = picker_fit_text(&name, content_w.saturating_sub(scale * 8));
    boot_draw_text_rgb(buf, w, h, margin_x + scale * 4, y, &file_line, PICKER_RGB_DIM);
    y += line_h;
    boot_fill_rect(
        buf,
        w,
        h,
        margin_x + scale * 4,
        y,
        content_w.saturating_sub(scale * 8),
        scale.max(1),
        PICKER_RGB_RULE,
    );
    y += line_h;

    let cursor = SAVE_PICKER_CHAR_CURSOR
        .load(Ordering::SeqCst)
        .min(pending.slots.len().saturating_sub(1));
    let rows_bottom = panel_bottom.saturating_sub(line_h * 2);
    for (i, info) in pending.slots.iter().enumerate() {
        if y + line_h >= rows_bottom {
            break;
        }
        let selected = i == cursor;
        if selected {
            boot_fill_rect(
                buf,
                w,
                h,
                margin_x + scale * 2,
                y.saturating_sub(scale * 2),
                content_w.saturating_sub(scale * 4),
                line_h + scale * 4,
                PICKER_RGB_SEL_BAR,
            );
        }
        let (color, prefix) = if selected {
            (PICKER_RGB_SEL_TEXT, "> ")
        } else {
            (PICKER_RGB_ROW, "  ")
        };
        let label = format!(
            "{prefix}SLOT {}   {}   LV {}",
            info.slot,
            info.name.to_ascii_uppercase(),
            info.level
        );
        let text = picker_fit_text(&label, content_w.saturating_sub(scale * 8));
        boot_draw_text_rgb(buf, w, h, margin_x + scale * 6, y, &text, color);
        y += row_step;
    }

    let footer_y = panel_bottom.saturating_sub(line_h);
    boot_draw_text_rgb(
        buf,
        w,
        h,
        margin_x + scale * 4,
        footer_y,
        "UP/DN MOVE  ENTER/A LOAD CHARACTER  BKSP/B BACK TO FILES",
        PICKER_RGB_DIM,
    );
    true
}
