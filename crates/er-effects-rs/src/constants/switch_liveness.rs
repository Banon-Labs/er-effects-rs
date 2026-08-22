// ============================================================================================
// IN-PROCESS MENU INPUT DRIVER (verified RE 2026-06-17). The main menu (built by SetState(2)=
// BeginLogo) reads input from the keystate bitmap inputmgr+0x90+eventId (edge-triggered &1).
// Confirm=0x3d, vertical-move=0x0/0x45. The Load-Game item d180 is INPUT-GATED -- it only ticks
// (and so is captured by the leaf/iterator hooks) once the cursor is navigated ONTO it. Main-menu
// order: Continue(0), Load Game=d180(1), so ONE Down from the default reaches Load Game. We inject
// Down taps in-process (NO host input, NO window focus) until d180 is captured, then STAGE 2
// invokes its functor directly -- so we never Confirm a wrong item (no New-Game/save-write risk).
// ============================================================================================
pub(crate) use er_title_flow::INPUTMGR_BITMAP_90_OFFSET;
pub(crate) use er_title_flow::MENU_EVENT_PRESSED_BIT;
pub(crate) use er_title_flow::MenuEventId;

pub(crate) use er_title_flow::MENU_EVENT_CONFIRM_3D;
pub(crate) const MENU_EVENT_MOVE_A_00: usize = MenuEventId::MoveA as usize;
pub(crate) const MENU_EVENT_MOVE_B_45: usize = MenuEventId::MoveB as usize;
/// Menu list cursor (highlighted index) and item count, on the list object (cursor getter
/// 0x140739e20 = `mov eax,[rcx+0xd4]`). Used to LOG the live cursor (diagnostic) while injecting.
#[repr(C)]
#[allow(dead_code)] // Retained RE layout: decoded struct shape, nothing constructs it today.
pub(crate) struct MenuListLayout {
    pub(crate) unknown_000: [u8; 0xd0],
    pub(crate) count: i32,
    pub(crate) cursor: i32,
}

#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const MENU_LIST_CURSOR_D4_OFFSET: usize = core::mem::offset_of!(MenuListLayout, cursor);
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const MENU_LIST_COUNT_D0_OFFSET: usize = core::mem::offset_of!(MenuListLayout, count);
/// Down-tap cadence: assert the move bit for SET frames (edge), then GAP idle frames (so the menu
/// sees a clean single edge + auto-repeat is avoided), one cursor step per cycle.
#[repr(u64)]
#[allow(dead_code)] // Retained RE layout: decoded value vocabulary, no live use today.
pub(crate) enum MenuTapSchedule {
    SetFrames = 2,
    GapFrames = 10,
    MaxTaps = 12,
}

#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MENU_TAP_SET_FRAMES: u64 = MenuTapSchedule::SetFrames as u64;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MENU_TAP_GAP_FRAMES: u64 = MenuTapSchedule::GapFrames as u64;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MENU_TAP_CYCLE_FRAMES: u64 = MENU_TAP_SET_FRAMES + MENU_TAP_GAP_FRAMES;
/// Max Down taps before giving up (menu has 5 items; cap generously). Down saturates at the last
/// item (no wrap), so this also bounds an overshoot.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MENU_NAV_MAX_TAPS: u64 = MenuTapSchedule::MaxTaps as u64;
/// Per-frame counter for the menu-input nav (starts when nav begins, after the modal grace).
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static MENU_NAV_FRAME: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Forced entry-diagnostic counter (log the first few menu_input_drive calls unconditionally,
/// before any early return, so we can see the inputmgr value + capture state).
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static MENU_DRIVE_ENTER_LOG: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MENU_DRIVE_ENTER_LOG_MAX: usize = TraceSampleLimit::Value8 as usize;
