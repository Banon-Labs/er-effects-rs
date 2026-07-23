//! DIRECT in-process input injection -- the VERIFIED ER input lever (user-confirmed 2026-07-19; the
//! SendInput/XInput/window-focus path was a DEAD END). No OS input is synthesized: the game's own
//! input memory is written on the game thread each frame.
//!
//! Two writes, both ported verbatim from the product with their exact reverse-engineered addresses:
//!
//!  1. MENU EVENTS -- the front-end/menu reads a KEYSTATE BITMAP at `inputmgr+0x90+eventId`, edge-
//!     triggered (`&1`). `inputmgr = *(base + 0x3d6b7b0)` (CSMenuMan / SelectBot input manager). Tap
//!     an event by OR-ing bit0 into `inputmgr+0x90+eventId`; the bitmap is re-polled every frame, so
//!     assert for a couple frames then gap for a clean single edge (no auto-repeat). Verified event
//!     ids (RE 2026-06-17, `frontend-menu-input-injection-ids-2026`): vertical-move = 0x00 AND 0x45
//!     (inject both; only Down advances, Up saturates), Confirm/OK = 0x3d. Mirrors the product's
//!     `menu_input_probe` (crates/er-effects-rs/src/experiments/continue_load/product_continue.rs).
//!
//!  2. STAY-ACTIVE (unfocused input) -- ER clears `[DLUID+0x88d]` every frame it is not
//!     `GetActiveWindow`; re-setting it to 1 lets the injected input apply while the window is
//!     UNFOCUSED (bd `breakthrough-pad-boundary-injection-moves-char-needs-focus`). `DLUID =
//!     *(base + 0x485dc18)` (input-device manager). This is why the direct path needs no window focus.
//!
//! Both writes are guarded by a fault-safe readability probe first, so a not-yet-initialized singleton
//! pointer can never fault the game thread.

use crate::log::harness_log;
use crate::win32::{read_u8, read_usize};

/// `inputmgr`/CSMenuMan singleton RVA (`SELECTBOT_INPUT_MANAGER_GLOBAL_RVA` /
/// `GLOBAL_CSMENUMAN_RVA` in the product constant tree).
const INPUT_MANAGER_GLOBAL_RVA: usize = 0x3d6b7b0;
/// Keystate bitmap base within the input manager (`INPUTMGR_BITMAP_90_OFFSET`).
const INPUTMGR_BITMAP_90_OFFSET: usize = 0x90;
/// Edge bit written per event (`MENU_EVENT_PRESSED_BIT`).
const MENU_EVENT_PRESSED_BIT: u8 = 1;

/// DLUID (input-device manager) singleton RVA (`RuntimeGlobalRva::DluidInputManager`).
const DLUID_SINGLETON_RVA: usize = 0x485dc18;
/// Input-active flag offset within DLUID (`DLUID_INPUT_ACTIVE_FLAG_OFFSET`).
const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize = 0x88d;

const HEAP_LO: usize = 0x10000;

/// Verified front-end/menu event ids (see module doc). No reversed id exists for the OptionSetting
/// tab-switch -- that is mouse-only on native and is the known self-drive gap.
#[derive(Clone, Copy)]
pub enum MenuEvent {
    /// Vertical move DOWN (id 0x00) and UP (id 0x45) -- verified vertical-move ids. Injected singly now
    /// (directional), so nav can stop on a middle row instead of saturating an extreme.
    MoveDown,
    MoveUp,
    /// OptionSetting tab-switch: LEFT/prev tab (id 0x30) and RIGHT/next tab (id 0x31). RE 2026-07-22
    /// (bd MENU-GAPS-CLOSED): GridControl pager FUN_1407392f0 -> tab handler FUN_14093b760.
    TabLeft,
    TabRight,
    Confirm,
    /// Modal-dialog OK/accept (id 0x01). Consumed ONLY by the dialog builder FUN_140e9a920 while a modal
    /// CS dialog (connection-error / offline-notice / save-data / ToS popup) is up; a no-op otherwise.
    /// Tapped EVERY frame to generally accept the 0-N boot-flow popups that block Continue. bd
    /// HARNESS-must-tap-dialog-OK-0x01-every-frame-2026-07-22.
    PopupAccept,
}

impl MenuEvent {
    const fn id(self) -> usize {
        match self {
            MenuEvent::MoveDown => 0x00,
            MenuEvent::MoveUp => 0x45,
            MenuEvent::TabLeft => 0x30,
            MenuEvent::TabRight => 0x31,
            MenuEvent::Confirm => 0x3d,
            MenuEvent::PopupAccept => 0x01,
        }
    }
}

/// CSMenuManImp.popupMenu (+0x80) and its request-open-IngameTop flag (+0x121). RE 2026-07-22.
const CS_MENU_MAN_POPUP_MENU_80_OFFSET: usize = 0x80;
const POPUP_MENU_REQUEST_OPEN_INGAME_TOP_121_OFFSET: usize = 0x121;
/// The in-world menu-open guard id: opening IngameTop is only honored while `inputmgr+0x90+0x1c & 1 == 0`
/// (the raw-pad Options press is read elsewhere; this is the equivalent of the native open fn's guard).
const MENU_OPEN_GUARD_EVENT_ID: usize = 0x1c;

/// Request the in-world pause/System menu (02_000_IngameTop) to open, the equivalent one-shot effect of
/// the native open fn (deobf 0x7ede50): set `popupMenu+0x121 = 1`. Fault-safe; game-thread only. Returns
/// true once the request was written. Gated on the same guard the native fn uses so it is a no-op when a
/// menu is already up. bd MENU-GAPS-CLOSED-tabswitch...pausemenu-open-2026-07-22.
pub fn request_open_ingame_menu(input_manager_ptr: usize) -> bool {
    let guard = input_manager_ptr + INPUTMGR_BITMAP_90_OFFSET + MENU_OPEN_GUARD_EVENT_ID;
    if unsafe { read_u8(guard) }.is_none_or(|g| g & 1 != 0) {
        return false;
    }
    let Some(popup) = (unsafe { read_usize(input_manager_ptr + CS_MENU_MAN_POPUP_MENU_80_OFFSET) })
        .filter(|p| *p >= HEAP_LO)
    else {
        return false;
    };
    let req = popup + POPUP_MENU_REQUEST_OPEN_INGAME_TOP_121_OFFSET;
    if unsafe { read_u8(req) }.is_none() {
        return false;
    }
    // SAFETY: confirmed-readable byte in the live CSPopupMenu; +0x121 is the request-open-IngameTop
    // flag CSPopupMenu::Update consumes next frame (RE 2026-07-22).
    unsafe {
        *(req as *mut u8) = 1;
    }
    true
}

/// Resolve the dereferenced input-manager pointer, or `None` before it is initialized.
pub fn input_manager(base: usize) -> Option<usize> {
    unsafe { read_usize(base + INPUT_MANAGER_GLOBAL_RVA) }.filter(|p| *p >= HEAP_LO)
}

// --- NATIVE EquipTop open (bd er-effects-rs-pe98, RE 2026-07-23) ---
// The pause list opens submenus exclusively through MenuJob FACTORIES + CSPopupMenu job submit;
// there is NO request byte for Equipment (the +0x121/+0x122 request family covers only
// IngameTop/WorldMap). Equipment row = st_pauseMenuClickHandlerInfoList[0], factory dump
// 0x140801cb0 (builds the 02_010_EquipTop union job from popup+0x10 alone); submit wrapper dump
// 0x1407ee2e0 is the SAME one the proven +0x121 IngameTop request path uses (core
// CSPopupMenu::StartTopMenuJob dump 0x1407f0c40 pushes the current top job to popup+0xD0 so Back
// pops natively, and bumps the job serial at popup+0x168). All deobf VAs ground-truthed
// content-unique via scripts/dump-deobf-shift.py.

/// `FUN_140801cb0` deobf: Equipment pause-row MenuJob factory
/// `(DLReferencePointer<CS::MenuJob>* out, ComponentStack* popup+0x10) -> out`.
const EQUIP_TOP_JOB_FACTORY_RVA: usize = 0x801bc0;
/// `FUN_1407ee2e0` deobf: popup top-job submit wrapper `(popup, refptr* out, u64* serial_out,
/// refptr* job)` -- the exact call shape of the +0x121 IngameTop open path.
const POPUP_SUBMIT_TOP_JOB_RVA: usize = 0x7ee1f0;
/// CSPopupMenu.componentStack used by every pause-row factory.
const POPUP_COMPONENT_STACK_10_OFFSET: usize = 0x10;
/// CSPopupMenu top-job submit serial; increments per StartTopMenuJob -- a clean open semaphore.
const POPUP_JOB_SERIAL_168_OFFSET: usize = 0x168;

fn popup_menu(input_manager_ptr: usize) -> Option<usize> {
    unsafe { read_usize(input_manager_ptr + CS_MENU_MAN_POPUP_MENU_80_OFFSET) }
        .filter(|p| *p >= HEAP_LO)
}

/// Read the CSPopupMenu job-submit serial (popup+0x168), or 0 when unresolvable.
pub fn popup_job_serial(input_manager_ptr: usize) -> u64 {
    popup_menu(input_manager_ptr)
        .and_then(|popup| unsafe { read_usize(popup + POPUP_JOB_SERIAL_168_OFFSET) })
        .unwrap_or(0) as u64
}

/// NATIVE Equipment-menu open: build the EquipTop union job with the game's own pause-row factory
/// and submit it through the native CSPopupMenu top-job path (native enqueue + native pump
/// ownership; no Scaleform input). Faithful nesting requires the pause menu (IngameTop) to already
/// be the top job -- call only after `pause_menu_open()`. Game thread only. Returns true once the
/// job was built and submitted.
pub fn native_open_equip_menu(base: usize, input_manager_ptr: usize) -> bool {
    type EquipJobFactoryFn = unsafe extern "system" fn(*mut [usize; 2], usize) -> *mut [usize; 2];
    type SubmitTopJobFn =
        unsafe extern "system" fn(usize, *mut [usize; 2], *mut u64, *mut [usize; 2]);

    let Some(popup) = popup_menu(input_manager_ptr) else {
        return false;
    };
    let factory: EquipJobFactoryFn =
        unsafe { std::mem::transmute(base + EQUIP_TOP_JOB_FACTORY_RVA) };
    let submit: SubmitTopJobFn = unsafe { std::mem::transmute(base + POPUP_SUBMIT_TOP_JOB_RVA) };

    let mut job: [usize; 2] = [0; 2];
    // SAFETY: the factory constructs a DLReferencePointer<MenuJob> into raw 16-byte out storage
    // from popup+0x10, exactly as every native pause-row confirm does (RE 2026-07-23).
    unsafe { factory(&mut job, popup + POPUP_COMPONENT_STACK_10_OFFSET) };
    if job[0] < HEAP_LO {
        return false;
    }
    let mut out: [usize; 2] = [0; 2];
    let mut serial: u64 = 0;
    // SAFETY: same call shape as the native +0x121 IngameTop open (popup, &out, &serial, &job);
    // the core pushes the current top job to popup+0xD0 so Back pops natively. The job refptr's
    // one retained reference is intentionally left alive (the menu owns the job's lifetime).
    unsafe { submit(popup, &mut out, &mut serial, &mut job) };
    true
}

/// Tap one menu event into the keystate bitmap (edge OR). Fault-safe: only writes once the target
/// byte is confirmed readable. Must be called on the game thread (from the per-frame drive hook) so
/// the write lands in the same frame the game re-polls the bitmap.
pub fn tap_menu_event(input_manager_ptr: usize, event: MenuEvent) {
    let addr = input_manager_ptr + INPUTMGR_BITMAP_90_OFFSET + event.id();
    if unsafe { read_u8(addr) }.is_none() {
        return;
    }
    // SAFETY: `addr` is a confirmed-readable byte inside the live input manager; OR-ing the edge bit
    // is exactly what the native input producer does at 0x1407ad509.
    unsafe {
        *(addr as *mut u8) |= MENU_EVENT_PRESSED_BIT;
    }
}

/// Re-set `[DLUID+0x88d] = 1` so injected input applies while the ER window is UNFOCUSED. Fault-safe;
/// call every frame from the drive hook (ER clears it each unfocused frame). Returns true once the
/// flag was written at least once (for logging).
pub fn keep_input_active(base: usize) -> bool {
    let Some(dluid) = (unsafe { read_usize(base + DLUID_SINGLETON_RVA) }).filter(|p| *p >= HEAP_LO)
    else {
        return false;
    };
    let flag = dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET;
    if unsafe { read_u8(flag) }.is_none() {
        return false;
    }
    // SAFETY: confirmed-readable flag byte inside the live DLUID singleton.
    unsafe {
        *(flag as *mut u8) = 1;
    }
    true
}

/// Title global accept byte RVA (`TITLE_GLOBAL_ACCEPT_BYTE_RVA` in the product constant tree). PRESS
/// ANY BUTTON is read on the raw-pad layer, NOT the keystate bitmap; the game's own
/// `TitleTopDialog::update` accept-gate advances the parked press-any-button title when this byte is 1
/// (bd title-global-accept-byte-144589bdc-zeroinput-advance). This is the decoded accept flag, not an
/// OS input event -- the harness sets it to blow through PRESS ANY BUTTON and open the title menu.
const TITLE_GLOBAL_ACCEPT_BYTE_RVA: usize = 0x4589bdc;

/// Set the title global accept byte = 1 to advance the parked PRESS ANY BUTTON title into its menu.
/// Fault-safe; game-thread only. Returns true once written. A no-op effect once past the title.
pub fn advance_press_any_button(base: usize) -> bool {
    let addr = base + TITLE_GLOBAL_ACCEPT_BYTE_RVA;
    if unsafe { read_u8(addr) }.is_none() {
        return false;
    }
    // SAFETY: confirmed-readable byte in the mapped game image; this is the product's own accept-byte
    // write (product_autoload_gates.rs: `*(base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) = 1`).
    unsafe {
        *(addr as *mut u8) = 1;
    }
    true
}

/// Log the resolved singletons once, for the evidence trail.
pub fn log_resolution(base: usize) {
    harness_log!(
        "input-inject: base=0x{base:x} input_manager=0x{:x} dluid_present={} (direct keystate-bitmap + DLUID stay-active channel; no SendInput/XInput)",
        input_manager(base).unwrap_or(0),
        (unsafe { read_usize(base + DLUID_SINGLETON_RVA) })
            .filter(|p| *p >= HEAP_LO)
            .is_some() as u8
    );
}
