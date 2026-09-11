//! The native `CS::SoftwareKeyboard` job, and the two text fields this repo opens with it.
//!
//! Moved out of `er-quickload`'s `quit_menu/save_picker_path_editor.rs`. Two surfaces drive one
//! mechanism -- the save picker's CurrentPath editor and the System>Quit **Load Build from URL**
//! link field -- and they are kept apart by [`KeyboardPurpose`] rather than by a flag, because each
//! owns its own job slot, outcome slot, window latch and Scaleform cache key. The picker half still
//! reaches its browse surface through the `QuitMenuHost` seam; the link-field half needs no product
//! at all, which is what lets a standalone shell open it.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use er_game_base::mem::{
    game_module_base, game_rva, game_rva_for_hook, safe_read_i32, safe_read_u16, safe_read_usize,
};
use er_telemetry_core::counters::*;

use crate::host::{
    append_autoload_debug, reset_path_editor_caret_latch, save_picker_stage_row_records,
};
use crate::scaleform_proxy::reset_build_url_field_latches;

/// A null pointer, named. Same value as the product's `TITLE_OWNER_SCAN_START_ADDRESS`.
const TITLE_OWNER_SCAN_START_ADDRESS: usize = usize::MIN;
/// A trampoline slot that has never been written.
const HOOK_ORIGINAL_UNSET: usize = 0;
const GAME_HEAP_ALLOC_RVA: usize = er_game_base::rva::GAME_HEAP_ALLOC_RVA;
const MENU_JOB_SUBMIT_RVA: u32 = er_title_flow::MENU_JOB_SUBMIT_RVA;
const MENU_JOB_QUEUE_READY_RVA: u32 = er_title_flow::MENU_JOB_QUEUE_READY_RVA;
const SYSTEM_QUIT_DIALOG_MENU_JOB_QUEUE_10_OFFSET: usize =
    er_title_flow::SYSTEM_QUIT_DIALOG_MENU_JOB_QUEUE_10_OFFSET;
const SYSTEM_QUIT_DIALOG_MENU_WINDOW_LIST_50_OFFSET: usize =
    er_title_flow::SYSTEM_QUIT_DIALOG_MENU_WINDOW_LIST_50_OFFSET;

/// Resolve an RVA and confirm the live bytes still start with the prologue this address was
/// verified against. Same fail-closed shape as the product's `save_flow_verify_rva`: a mismatch
/// means the running image is not the build these addresses came from, so the call is refused.
/// `mask` is the generated companion of `expected` (`<NAME>_MASK`): 0xff compares exactly, 0x00 is
/// a RIP-relative displacement, which re-encodes on every game build.
fn verify_rva(rva: u32, expected: &[u8], mask: &[u8], name: &str) -> Option<usize> {
    let address = match game_rva(rva) {
        Ok(address) => address,
        Err(err) => {
            append_autoload_debug(format_args!(
                "software-keyboard: cannot resolve {name} rva 0x{rva:x}: {err}"
            ));
            return None;
        }
    };
    let mut actual = [0_u8; 32];
    let window = &mut actual[..expected.len().min(32)];
    if !unsafe { er_game_base::mem::read_bytes(address, window) } {
        append_autoload_debug(format_args!(
            "software-keyboard: {name} @0x{address:x}: prologue unreadable"
        ));
        return None;
    }
    if !er_game_base::prologue::matches_masked(window, expected, mask) {
        append_autoload_debug(format_args!(
            "software-keyboard: {name} @0x{address:x}: prologue mismatch; refusing to call it"
        ));
        return None;
    }
    Some(address)
}

/// [`verify_rva`] for an address about to be detoured: same verification, but what comes back is
/// the unresolved `base + rva`, because the hook API must own the single resolve that decides where
/// the five bytes land.
fn verify_rva_for_hook(rva: u32, expected: &[u8], mask: &[u8], name: &str) -> Option<usize> {
    verify_rva(rva, expected, mask, name)?;
    game_rva_for_hook(rva).ok()
}

/// Register one detour on the `er-hook` union exactly once.
///
/// The union rather than a bare `MhHook`: both of these addresses are also reachable by the product
/// DLL, and two MinHook instances on one prologue overwrite each other's trampolines with nothing
/// logged.
fn mh_install_hook_once(
    flag: &AtomicUsize,
    not_installed: usize,
    installed_yes: usize,
    addr: usize,
    handler: *mut c_void,
    orig: &'static AtomicUsize,
    name: &str,
) -> bool {
    if flag
        .compare_exchange(
            not_installed,
            installed_yes,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return flag.load(Ordering::SeqCst) == installed_yes;
    }
    let handler_fn: er_hook::UnionFn =
        unsafe { std::mem::transmute::<*mut c_void, er_hook::UnionFn>(handler) };
    match unsafe { er_hook::register_union_hook(addr, handler_fn, orig) } {
        Ok(()) => {
            append_autoload_debug(format_args!(
                "software-keyboard: {name} registered on union 0x{addr:x}"
            ));
            true
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "software-keyboard: register_union_hook {name} failed: {status:?}"
            ));
            flag.store(not_installed, Ordering::SeqCst);
            false
        }
    }
}

const SOFTWARE_KEYBOARD_JOB_SIZE: usize = 0x1a8;
const SOFTWARE_KEYBOARD_VALIDATOR_SIZE: usize = 0x70;
const SOFTWARE_KEYBOARD_JOB_CTOR_RVA: u32 = 0x81be30;
const SOFTWARE_KEYBOARD_RESULT_GATE_RVA: u32 = 0x81d3d0;
const SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_RVA: u32 = 0x81d220;
const FD4_TIME_VTABLE_RVA: usize = 0x29c8e58;
const FD4_TIME_FLOAT_VTABLE_RVA: usize = 0x29c8e48;
const SOFTWARE_KEYBOARD_VALIDATOR_INIT_RVA: u32 = 0xe70920;
const SOFTWARE_KEYBOARD_VALIDATOR_DTOR_RVA: u32 = 0xe70960;
const SOFTWARE_KEYBOARD_ENTER_NAME_RVA: u32 = 0xe70c00;
const SOFTWARE_KEYBOARD_SET_INITIAL_RVA: u32 = 0xe709f0;
const SOFTWARE_KEYBOARD_SET_MAX_RVA: u32 = 0x2416ee0;

// The `*_SIG` prologue for each RVA above -- plus `GAME_HEAP_ALLOC_SIG` for the allocator thunk
// at `GAME_HEAP_ALLOC_RVA` -- is assembled from named instructions by this crate's `build.rs`,
// which also compares them against `eldenring-deobf.bin` when a copy is present.
include!(concat!(
    env!("OUT_DIR"),
    "/generated_software_keyboard_prologues.rs"
));
const GLOBAL_MENU_HEAP_ALLOCATOR_RVA: usize = er_game_base::rva::GLOBAL_MENU_HEAP_ALLOCATOR_RVA;

const SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET: usize = 0xd8;
const SOFTWARE_KEYBOARD_CONTROLLER_RESULT_78_OFFSET: usize = 0x78;
const SOFTWARE_KEYBOARD_CONTROLLER_TEXT_80_OFFSET: usize = 0x80;
const DLSTRING_DATA_08_OFFSET: usize = 0x08;
const DLSTRING_LENGTH_18_OFFSET: usize = 0x18;
const DLSTRING_CAPACITY_20_OFFSET: usize = 0x20;
const SOFTWARE_KEYBOARD_VALIDATOR_MAX_60_OFFSET: usize = 0x60;
const SOFTWARE_KEYBOARD_VALIDATOR_FLAGS_68_OFFSET: usize = 0x68;
const SOFTWARE_KEYBOARD_VALIDATOR_MAX_6C_OFFSET: usize = 0x6c;
const SOFTWARE_KEYBOARD_MAX_PATH_UNITS: usize = 1024;
const MENU_JOB_REFCOUNT_08_OFFSET: usize = 0x08;
const MENU_JOB_STATE_CONTINUE: i32 = 1;
const MENU_JOB_STATE_SUCCESS: i32 = 2;
const MENU_JOB_STATE_FAILED: i32 = 3;
const PATH_EDITOR_WINDOW_STALE_PROFILE_TICKS: usize = 3;

/// Distinct Scaleform cache key for the path editor. The file-open hook redirects this miss to the
/// canonical 02_990 bytes and derives an inline movie without mutating the game's shared native
/// text-input resource.
pub const TEXT_INPUT_RESOURCE_NAME: &str = "02_990_TextInput_PathEditor";

/// One movie, two cache keys, two DERIVATIONS.
///
/// The build-url field used to pass the path editor's key, which meant it also got the path
/// editor's derived movie -- and that derivation alpha-zeroes the movie's backing plate and both
/// frame placements, because over ProfileSelect the picker's own `CurrentPath` button supplies the
/// frame. Nothing on the Quit tab supplies one, so the link field rendered as a bare text run in
/// the top-left corner of the screen (user report 2026-08-23). Scaleform caches by this string, so
/// a second key is what buys the Quit tab its own bytes; the file-open hook redirects both keys to
/// the same canonical 02_990 payload and derives from it separately.
pub const BUILD_URL_TEXT_INPUT_RESOURCE_NAME: &str = "02_990_TextInput_BuildUrl";

/// NUL-terminated UTF-16 copy of an ASCII resource name, for the native
/// `CS::SoftwareKeyboardConfig`. Const-evaluated, so a name that would not fit its buffer is a
/// compile error rather than a silently unterminated string handed to the engine.
const fn utf16_resource<const N: usize>(name: &str) -> [u16; N] {
    let bytes = name.as_bytes();
    assert!(bytes.len() < N, "resource name needs room for its NUL");
    let mut out = [0u16; N];
    let mut index = 0;
    while index < bytes.len() {
        out[index] = bytes[index] as u16;
        index += 1;
    }
    out
}

// `static`, not `const`, and that distinction is load-bearing. The job constructor copies the
// `SoftwareKeyboardConfig` struct by value into the job (`MOVUPS XMM0,[RSI]; MOVUPS
// [R14+0x150],XMM0` at `0x14081bec2`) -- pointer included -- and the movie is not loaded then. It is
// loaded later, from the job's own first step: `FUN_14081cd70` reads the copied config back out of
// `job+0x150` and takes `config.resource` as the `CSScaleformLoadInfo::filename` it hands to
// `FUN_1407fb050`. That dereference happens long after `submit_software_keyboard` has returned, so
// the bytes must outlive it. A `const` is inlined as a temporary at each use site and `.as_ptr()` on
// one has no lifetime past its statement unless the compiler happens to promote it -- a guarantee
// this must not depend on when the failure mode is the keyboard loading a movie named out of reused
// stack.
static TEXT_INPUT_RESOURCE: [u16; 28] = utf16_resource(TEXT_INPUT_RESOURCE_NAME);
static BUILD_URL_TEXT_INPUT_RESOURCE: [u16; 26] =
    utf16_resource(BUILD_URL_TEXT_INPUT_RESOURCE_NAME);

#[repr(C)]
struct SoftwareKeyboardConfig {
    max_units: u32,
    mode: u8,
    padding: [u8; 3],
    resource: *const u16,
}

struct SoftwareKeyboardRecipe {
    ctor: usize,
    validator_init: usize,
    validator_dtor: usize,
    enter_name: usize,
    set_initial: usize,
    set_max: usize,
    heap_alloc: usize,
    queue_ready: usize,
    submit: usize,
}

#[derive(Debug)]
enum PathEditorOutcome {
    Accepted(String),
    Cancelled,
    TextUnreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathEditorSubmit {
    Submitted,
    RetryWhenQueueReady,
    Rejected,
}

static SOFTWARE_KEYBOARD_RECIPE: OnceLock<Option<SoftwareKeyboardRecipe>> = OnceLock::new();
static SOFTWARE_KEYBOARD_RESULT_GATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_WINDOW: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_OUTCOME: OnceLock<Mutex<Option<PathEditorOutcome>>> =
    OnceLock::new();

// ---------------------------------------------------------------------------------------------
// Two editors, one pair of DETOURS.
//
// The Quit tab's "Load Build from URL" row needs the same native `CS::SoftwareKeyboard` this file
// already drives for save paths. It must not install its own hooks on 0x81d3d0 / 0x81d220: two
// MinHook detours on one prologue overwrite each other's trampolines, which is the exact corruption
// `scripts/me3-dll-conflicts.toml` exists to keep out of a profile -- and it would be worse here,
// inside one DLL, where no profile check could see it.
//
// So the detours stay single and gain an owner. Each purpose has its own active-job slot and its
// own outcome mailbox; a dispatched job belongs to whichever slot holds it, and a job in neither is
// forwarded untouched (the game opens this keyboard for character names too).
// ---------------------------------------------------------------------------------------------

/// Which editor a live `SoftwareKeyboardJob` belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyboardPurpose {
    /// The save picker's folder path field.
    SavePath,
    /// The System>Quit "Load Build from URL" row's link field.
    BuildUrl,
}

static BUILD_URL_EDITOR_ACTIVE_JOB: AtomicUsize = AtomicUsize::new(0);

/// Menu-pump passes since the DLL loaded, counted by [`build_url_menu_pump_tick`].
///
/// Not `PROFILE_SELECT_WINDOW_RUN_TICKS`, which the first version of this watchdog used and which
/// made it inert: that counter is incremented only by the `05_010_ProfileSelect` branch of the run
/// post-hook, so on the System>Quit tab -- where the link field actually lives -- it never advances
/// at all. `now - last` stayed 0 forever and no latch was ever judged abandoned (measured
/// `dll:f9d11870`, 2026-08-23: four opens, zero releases). A watchdog is only as good as the clock
/// it reads, and this one now reads a clock that ticks where the field is.
static BUILD_URL_MENU_PUMP_TICKS: AtomicUsize = AtomicUsize::new(0);

/// The [`BUILD_URL_MENU_PUMP_TICKS`] value when the link field's window was last seen running.
///
/// A closed 02_990 window is not reported terminal to us -- it simply stops being run, so the
/// live->terminal transition an earlier fix watched for never arrives. Absence is the only signal
/// the closed field emits, and this stamp is what makes absence measurable.
static BUILD_URL_EDITOR_WINDOW_LAST_TICK: AtomicUsize = AtomicUsize::new(0);

/// Advance the link field's clock. Called once per menu-pump pass, unconditionally, because the
/// pump is the one thing that runs whether or not the field's own window still does.
pub fn build_url_menu_pump_tick() -> usize {
    BUILD_URL_MENU_PUMP_TICKS.fetch_add(1, Ordering::SeqCst) + 1
}
static BUILD_URL_EDITOR_OUTCOME: OnceLock<Mutex<Option<PathEditorOutcome>>> = OnceLock::new();
/// The link field's own live 02_990 MenuWindow, kept apart from the picker's for the same reason
/// their movies are: the picker's stale-window watchdog would otherwise see this window, decide its
/// own job had gone quiet, and cancel it.
static BUILD_URL_EDITOR_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Which keyboard job was latched when [`BUILD_URL_EDITOR_WINDOW`] adopted its window.
///
/// The pairing is the point. Without it the close path took whatever job the slot held, which is
/// not necessarily the job the closing window belonged to -- and in run `br-20260911-151334-56a8`
/// it was not: at `+106723ms` a job was submitted and, in the same millisecond, reported closed and
/// cancelled, before its window had run a single frame. What closed was the previous field's
/// window, going terminal one frame late; the job it cancelled was the one just submitted. The
/// second occurrence is starker still -- `window=0x0 closed with job=0x1c5a4cb80` -- a null window
/// cancelling a field the player had only just opened.
static BUILD_URL_EDITOR_WINDOW_JOB: AtomicUsize = AtomicUsize::new(0);

/// The window pointer whose field has already closed, and which must not be adopted again.
///
/// A closed 02_990 window keeps being run live for a further ~200ms before it stops, and during
/// that tail it was re-adopted as though it were a fresh field. Run br-20260911-152507-b8e5: the
/// old window `0x28dcf080` was still being positioned at `+51122ms`, the player pressed the row at
/// `+51736ms`, and one millisecond later that same old window reported terminal and cancelled the
/// job that had just been submitted. The window the new field actually got was `0x28dcc080`, and
/// the game asked for its movie at `+51765ms` -- after the cancel.
///
/// Cleared by [`build_url_note_movie_served`], because the game acquiring the 02_990 resource is
/// its own statement that a new field is being built. Keyed on the pointer, so an allocator that
/// hands the next field the same address is covered by that clear rather than blocked forever.
static BUILD_URL_CLOSED_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// The last state value seen for the link field's window, so only changes are logged.
static BUILD_URL_EDITOR_WINDOW_LAST_STATE: core::sync::atomic::AtomicIsize =
    core::sync::atomic::AtomicIsize::new(-999);

/// The game has acquired the link field's 02_990 movie: a new field is being built, so the
/// previous field's window is no longer the thing to refuse.
pub fn build_url_note_movie_served() {
    BUILD_URL_CLOSED_WINDOW.store(0, Ordering::SeqCst);
}

/// Note the link field's 02_990 MenuWindow state. `true` while it is a live transform target -- the
/// caller may position it; `false` once the window is terminal and its SceneObjProxy teardown has
/// begun, after which writing a transform through that proxy is a use-after-free.
pub fn build_url_note_editor_window_state(window: usize, state: i32) -> bool {
    // Log every state change for this window. Two fixes were built on a guess about which state
    // arrives and when, and both were wrong, because no line in the log carried the number. It is
    // one line per transition, not per frame, so a field that is simply up stays silent.
    let previous =
        BUILD_URL_EDITOR_WINDOW_LAST_STATE.swap(state as usize as isize, Ordering::SeqCst);
    if previous != state as usize as isize {
        append_autoload_debug(format_args!(
            "system-quit-build-url: 02_990 window=0x{window:x} state {previous} -> {state} (live={})",
            text_input_02_990_window_is_live(state)
        ));
    }
    if text_input_02_990_window_is_live(state) {
        if BUILD_URL_CLOSED_WINDOW.load(Ordering::SeqCst) == window
            && BUILD_URL_EDITOR_WINDOW.load(Ordering::SeqCst) == 0
        {
            // Its field is over; it is being run out, not up. Adopting it here is what let a dying
            // window cancel the field that replaced it, and positioning it writes a transform
            // nobody will see.
            return false;
        }
        BUILD_URL_EDITOR_WINDOW_LAST_TICK.store(
            BUILD_URL_MENU_PUMP_TICKS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        if BUILD_URL_EDITOR_WINDOW.swap(window, Ordering::SeqCst) == 0 {
            // Pair this window with the job that is latched right now, so its close can only ever
            // release that job and never a later one.
            BUILD_URL_EDITOR_WINDOW_JOB.store(
                keyboard_active_job_slot(KeyboardPurpose::BuildUrl).load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            // A fresh field. The window pointer is recycled across opens, so this 0 -> window
            // transition is the only per-open signal there is.
            //
            // This used to re-arm the save picker's path editor instead of the link field's own
            // latches -- a neighbouring editor that loads the same 02_990 movie, and in a
            // standalone shell with no host a no-op. So the link field's caret pass ran once per
            // process and every field after the first opened with the caret at index 0, which
            // makes typing prepend to the prefilled link.
            reset_build_url_field_latches();
        }
        return true;
    }
    if window != 0 && BUILD_URL_EDITOR_WINDOW.load(Ordering::SeqCst) == window {
        BUILD_URL_EDITOR_WINDOW.store(0, Ordering::SeqCst);
        BUILD_URL_CLOSED_WINDOW.store(window, Ordering::SeqCst);
        let paired = BUILD_URL_EDITOR_WINDOW_JOB.swap(0, Ordering::SeqCst);
        let latched = keyboard_active_job_slot(KeyboardPurpose::BuildUrl).load(Ordering::SeqCst);
        if paired != latched || paired == 0 {
            // This close does not belong to the latched job, so it releases nothing.
            //
            // `paired == 0` is the case that made the first attempt at this guard useless, caught
            // in run br-20260911-152041-9dff: a window whose field has already closed keeps being
            // run live for a few more frames, gets re-adopted at a moment when no job is latched,
            // and pairs with 0. The first version treated 0 as "unknown, go ahead and release" and
            // so still cancelled the next field 2ms after it opened -- `link field requested` at
            // `+66928ms`, `closed with job=0x9a6d60c0 ... releasing it` at `+66930ms`. A window
            // adopted while nothing was latched cannot own a job, which makes 0 a positive answer
            // rather than a missing one.
            if paired != 0 || latched != 0 {
                append_autoload_debug(format_args!(
                    "system-quit-build-url: window=0x{window:x} closed carrying job=0x{paired:x} \
                     while job=0x{latched:x} is latched -- not this window's field; releasing nothing"
                ));
            }
            return false;
        }
        // The window going terminal is the only reliable "THE FIELD CLOSED" signal we get.
        //
        // The back action was supposed to arrive at the `0x81d3d0` result gate, which would record
        // `Cancelled` and clear the active-job slot. It does not: a live session opened three link
        // fields, the player closed each with B, and that detour fired zero times (runtime log
        // `dll:8dca09bb`, 2026-08-23). Neither did the `0x81d220` terminal callback. Those two RVAs
        // are on the `SoftwareKeyboardJob` path, and this field is served by the SCALEFORM 02_990
        // fallback instead -- so a cancel there never reaches them, the job slot stays set forever,
        // and the row refuses every future press with "editor already active". The row is dead for
        // the rest of the session.
        //
        // The window is the participant that actually knows. It runs while the field is up and goes
        // terminal when it closes -- the game's own verdict, read from the state it hands the run
        // post-hook every frame, not a timeout and not an inference from silence. Releasing here
        // covers the cancel and any other close that skips those detours; an accept still reaches
        // the terminal callback first and leaves nothing for this to do.
        release_build_url_keyboard_on_window_close(window);
    }
    false
}

/// Release a link-field keyboard whose window has closed without either detour firing.
///
/// Deposits `Cancelled` only if no outcome is already waiting: an accept records its text from the
/// terminal callback and its window goes terminal immediately afterwards, so overwriting here would
/// turn every accepted link into a cancel.
/// How many menu-pump passes the link field may go unseen before its latch is treated as debris.
///
/// The pump runs once per menu frame, so this is the game's own cadence rather than wall-clock: a
/// field that is genuinely up is seen on every pass and can never reach the threshold, while a
/// closed one stops being seen immediately. Small enough that the row is pressable again well
/// inside the time it takes a player to move the cursor back to it.
const BUILD_URL_WINDOW_UNSEEN_TICK_LIMIT: usize = 8;

/// Has the link field's window stopped being run while its keyboard is still latched?
///
/// This is the case the terminal-state release cannot see. A MenuWindow that closes is not reported
/// to us as terminal -- it stops being pumped at all, so the only evidence of the close is that the
/// window never appears again. Comparing the last-seen stamp against the live counter turns that
/// absence into a verdict.
pub fn build_url_keyboard_latch_is_abandoned() -> bool {
    if keyboard_active_job_slot(KeyboardPurpose::BuildUrl).load(Ordering::SeqCst) == 0 {
        return false;
    }
    let last = BUILD_URL_EDITOR_WINDOW_LAST_TICK.load(Ordering::SeqCst);
    if last == 0 {
        return false;
    }
    let now = BUILD_URL_MENU_PUMP_TICKS.load(Ordering::SeqCst);
    now.saturating_sub(last) > BUILD_URL_WINDOW_UNSEEN_TICK_LIMIT
}

/// Release an abandoned link-field latch, reported so the next occurrence is legible.
pub fn release_abandoned_build_url_keyboard() {
    let window = BUILD_URL_EDITOR_WINDOW.swap(0, Ordering::SeqCst);
    BUILD_URL_EDITOR_WINDOW_LAST_TICK.store(0, Ordering::SeqCst);
    release_build_url_keyboard_on_window_close(window);
}

fn release_build_url_keyboard_on_window_close(window: usize) {
    let job = keyboard_active_job_slot(KeyboardPurpose::BuildUrl).swap(0, Ordering::SeqCst);
    if job == 0 {
        return;
    }
    // The latch is released; Ownership is not. This job still carries the empty `std::function` we
    // handed the engine, so the detours must keep claiming it until it actually finishes.
    remember_released_keyboard_job(job, KeyboardPurpose::BuildUrl);
    let mut slot = keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if slot.is_none() {
        *slot = Some(PathEditorOutcome::Cancelled);
    }
    drop(slot);
    append_autoload_debug(format_args!(
        "system-quit-build-url: link field window=0x{window:x} closed with job=0x{job:x} still \
         latched -- neither the cancel gate nor the terminal callback fired; releasing it so the \
         row is pressable again"
    ));
}

fn keyboard_active_job_slot(purpose: KeyboardPurpose) -> &'static AtomicUsize {
    match purpose {
        KeyboardPurpose::SavePath => &SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB,
        KeyboardPurpose::BuildUrl => &BUILD_URL_EDITOR_ACTIVE_JOB,
    }
}

fn keyboard_outcome_slot(purpose: KeyboardPurpose) -> &'static Mutex<Option<PathEditorOutcome>> {
    match purpose {
        KeyboardPurpose::SavePath => path_editor_outcome(),
        KeyboardPurpose::BuildUrl => BUILD_URL_EDITOR_OUTCOME.get_or_init(|| Mutex::new(None)),
    }
}

/// Which purpose owns this dispatched job, if any. A job owned by neither is the game's own use of
/// the keyboard and must be forwarded untouched.
fn keyboard_owner_of(job: usize) -> Option<KeyboardPurpose> {
    if job == 0 {
        return None;
    }
    if let Some(purpose) = [KeyboardPurpose::SavePath, KeyboardPurpose::BuildUrl]
        .into_iter()
        .find(|purpose| keyboard_active_job_slot(*purpose).load(Ordering::SeqCst) == job)
    {
        return Some(purpose);
    }
    // A job whose latch we already released is still ours, and the detours must still claim it.
    //
    // This is what crashed the game (`dll:a71aa552`, 2026-08-23, `0xe06d7363` ->
    // `ThrowBadFunctionCallException` from inside `FUN_14081d220+0xf8`, then
    // `NtTerminateProcess(0xc0000005)`). The job is constructed with an intentionally empty
    // `std::function` as its completion callback, which is only safe because the `0x81d220` detour
    // recognises the job and never lets the native side invoke it. The abandoned-latch watchdog
    // cleared the active-job slot while the job was still alive, so when that job later terminated
    // `keyboard_owner_of` no longer recognised it, the detour forwarded to the original, and the
    // engine called an empty `std::function` -- `std::bad_function_call`, straight through the
    // game's stack.
    //
    // Ownership therefore outlives the latch: releasing the latch is about the row being pressable
    // again, and says nothing about who is responsible for the job. Only the job actually finishing
    // ends that responsibility.
    RELEASED_KEYBOARD_JOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .find_map(|(released, purpose)| (*released == job).then_some(*purpose))
}

/// Jobs whose latch was released early but which are still alive, and still ours to intercept.
///
/// Bounded: a released job is dropped as soon as its detour fires, and the list is cleared whenever
/// an editor resets. The cap is a backstop so a pathological session cannot grow it without limit --
/// dropping the oldest is right, because the newest released job is the one most likely still alive.
static RELEASED_KEYBOARD_JOBS: Mutex<Vec<(usize, KeyboardPurpose)>> = Mutex::new(Vec::new());

/// Most recently released jobs kept claimable at once.
const RELEASED_KEYBOARD_JOB_LIMIT: usize = 8;

/// Remember a job whose latch was released while the job itself may still be running.
fn remember_released_keyboard_job(job: usize, purpose: KeyboardPurpose) {
    if job == 0 {
        return;
    }
    let mut jobs = RELEASED_KEYBOARD_JOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if jobs.iter().any(|(known, _)| *known == job) {
        return;
    }
    if jobs.len() >= RELEASED_KEYBOARD_JOB_LIMIT {
        jobs.remove(0);
    }
    jobs.push((job, purpose));
}

/// Forget a released job once its detour has fired and the native side is done with it.
fn forget_released_keyboard_job(job: usize) {
    RELEASED_KEYBOARD_JOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|(known, _)| *known != job);
}

/// Log tag per purpose, so one debug log distinguishes the two editors.
fn keyboard_tag(purpose: KeyboardPurpose) -> &'static str {
    match purpose {
        KeyboardPurpose::SavePath => "save-picker-path",
        KeyboardPurpose::BuildUrl => "system-quit-build-url",
    }
}

/// Scaleform cache key per purpose. Two keys, so each editor gets its own derived movie and its
/// own placement -- see [`BUILD_URL_TEXT_INPUT_RESOURCE_NAME`].
fn keyboard_resource(purpose: KeyboardPurpose) -> *const u16 {
    match purpose {
        KeyboardPurpose::SavePath => TEXT_INPUT_RESOURCE.as_ptr(),
        KeyboardPurpose::BuildUrl => BUILD_URL_TEXT_INPUT_RESOURCE.as_ptr(),
    }
}

pub fn save_picker_path_editor_active() -> bool {
    SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst) != 0
        || SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.load(Ordering::SeqCst) != 0
}

/// Called only from the owned 02_990 MenuWindowJob::Run post-hook. A terminal MenuWindow result
/// means its SceneObjProxy teardown has begun: never write another transform through that proxy.
///
/// Shared with the build-url field, which loads the same movie and needs the same answer -- one
/// rule, so the two editors cannot drift into disagreeing about when a window is safe to touch.
pub fn text_input_02_990_window_is_live(state: i32) -> bool {
    // A newly-created MenuWindow begins at zero before its first controller update. Continue is 1;
    // only Success/Failed are terminal. Treating zero as terminal cancelled every editor during its
    // construction frame and queued a ProfileSelect rebuild against half-bound child components.
    state == 0 || state == MENU_JOB_STATE_CONTINUE
}

pub fn save_picker_note_path_editor_window_state(window: usize, state: i32) -> bool {
    if text_input_02_990_window_is_live(state) {
        let previous_window = SAVE_PICKER_PATH_EDITOR_WINDOW.swap(window, Ordering::SeqCst);
        SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK.store(
            er_telemetry_core::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        if previous_window == 0 {
            // A fresh editor: re-arm the end-caret. This transition is the only per-open signal --
            // the window pointer itself gets recycled across opens.
            reset_path_editor_caret_latch();
            let dialog = SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.load(Ordering::SeqCst);
            if dialog != 0
                && SAVE_PICKER_REBUILD_PENDING_DIALOG
                    .compare_exchange(0, dialog, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                append_autoload_debug(format_args!(
                    "save-picker-path: 02_990 became visible window=0x{window:x}; queued ProfileSelect row rebuild to hide read-only CurrentPath"
                ));
            }
        }
        return true;
    }
    if SAVE_PICKER_PATH_EDITOR_WINDOW.load(Ordering::SeqCst) == window {
        SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
    }
    let active = SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst);
    if active != 0
        && SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB
            .compare_exchange(active, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        *path_editor_outcome()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PathEditorOutcome::Cancelled);
        append_autoload_debug(format_args!(
            "save-picker-path: 02_990 MenuWindow became terminal state={state} window=0x{window:x}; released job=0x{active:x} as cancelled before proxy teardown"
        ));
    }
    false
}

fn path_editor_outcome() -> &'static Mutex<Option<PathEditorOutcome>> {
    SAVE_PICKER_PATH_EDITOR_OUTCOME.get_or_init(|| Mutex::new(None))
}

/// Drop every pointer/queued result owned by a ProfileSelect path-editor session. Called only after
/// the native ProfileSelect MenuWindow finalizer has run; retaining any of these values lets a later
/// `Load Character from File` reuse a dead dialog/job from the prior menu generation.
pub fn save_picker_reset_path_editor_state() {
    SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(0, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.store(0, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.store(0, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK.store(0, Ordering::SeqCst);
    *path_editor_outcome()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

fn software_keyboard_recipe() -> Option<&'static SoftwareKeyboardRecipe> {
    SOFTWARE_KEYBOARD_RECIPE
        .get_or_init(|| {
            Some(SoftwareKeyboardRecipe {
                ctor: verify_rva(
                    SOFTWARE_KEYBOARD_JOB_CTOR_RVA,
                    SOFTWARE_KEYBOARD_JOB_CTOR_SIG,
                    SOFTWARE_KEYBOARD_JOB_CTOR_SIG_MASK,
                    "SoftwareKeyboardJob ctor",
                )?,
                validator_init: verify_rva(
                    SOFTWARE_KEYBOARD_VALIDATOR_INIT_RVA,
                    SOFTWARE_KEYBOARD_VALIDATOR_INIT_SIG,
                    SOFTWARE_KEYBOARD_VALIDATOR_INIT_SIG_MASK,
                    "SoftwareKeyboard validator init",
                )?,
                validator_dtor: verify_rva(
                    SOFTWARE_KEYBOARD_VALIDATOR_DTOR_RVA,
                    SOFTWARE_KEYBOARD_VALIDATOR_DTOR_SIG,
                    SOFTWARE_KEYBOARD_VALIDATOR_DTOR_SIG_MASK,
                    "SoftwareKeyboard validator dtor",
                )?,
                enter_name: verify_rva(
                    SOFTWARE_KEYBOARD_ENTER_NAME_RVA,
                    SOFTWARE_KEYBOARD_ENTER_NAME_SIG,
                    SOFTWARE_KEYBOARD_ENTER_NAME_SIG_MASK,
                    "SoftwareKeyboard EnterName preset",
                )?,
                set_initial: verify_rva(
                    SOFTWARE_KEYBOARD_SET_INITIAL_RVA,
                    SOFTWARE_KEYBOARD_SET_INITIAL_SIG,
                    SOFTWARE_KEYBOARD_SET_INITIAL_SIG_MASK,
                    "SoftwareKeyboard initial text setter",
                )?,
                set_max: verify_rva(
                    SOFTWARE_KEYBOARD_SET_MAX_RVA,
                    SOFTWARE_KEYBOARD_SET_MAX_SIG,
                    SOFTWARE_KEYBOARD_SET_MAX_SIG_MASK,
                    "SoftwareKeyboard max-length setter",
                )?,
                heap_alloc: verify_rva(
                    GAME_HEAP_ALLOC_RVA as u32,
                    GAME_HEAP_ALLOC_SIG,
                    GAME_HEAP_ALLOC_SIG_MASK,
                    "game heap allocator",
                )?,
                queue_ready: game_rva(MENU_JOB_QUEUE_READY_RVA).ok()?,
                submit: game_rva(MENU_JOB_SUBMIT_RVA).ok()?,
            })
        })
        .as_ref()
}

fn install_software_keyboard_result_hooks() -> bool {
    if SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED.load(Ordering::SeqCst) == 1
        && SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_INSTALLED.load(Ordering::SeqCst) == 1
    {
        // The live prologues are now MinHook jumps, so signature verification is valid only before
        // first installation. Reuse the already-verified hooks on every later editor open.
        return true;
    }
    let Some(address) = verify_rva_for_hook(
        SOFTWARE_KEYBOARD_RESULT_GATE_RVA,
        SOFTWARE_KEYBOARD_RESULT_GATE_SIG,
        SOFTWARE_KEYBOARD_RESULT_GATE_SIG_MASK,
        "SoftwareKeyboard accepted/cancel gate",
    ) else {
        return false;
    };
    mh_install_hook_once(
        &SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED,
        0,
        1,
        address,
        software_keyboard_result_gate_hook as *mut c_void,
        &SOFTWARE_KEYBOARD_RESULT_GATE_ORIG,
        "SoftwareKeyboard path cancel gate",
    );
    let Some(terminal) = verify_rva_for_hook(
        SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_RVA,
        SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_SIG,
        SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_SIG_MASK,
        "SoftwareKeyboard terminal callback",
    ) else {
        return false;
    };
    mh_install_hook_once(
        &SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_INSTALLED,
        0,
        1,
        terminal,
        software_keyboard_terminal_callback_hook as *mut c_void,
        &SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_ORIG,
        "SoftwareKeyboard path terminal callback",
    );
    SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED.load(Ordering::SeqCst) == 1
        && SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_INSTALLED.load(Ordering::SeqCst) == 1
}

unsafe fn software_keyboard_result_state(job: usize) -> Option<i32> {
    let controller = unsafe { safe_read_usize(job + SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET) }?;
    if controller == 0 || controller == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    unsafe { safe_read_i32(controller + SOFTWARE_KEYBOARD_CONTROLLER_RESULT_78_OFFSET) }
}

// The 02_990 controller substitutes some punctuation on the way out, and the PLACEHOLDERS are
// circled numbers.
//
// What comes back from `controller + 0x80` is not always what was typed. The backslash case was
// found first (a `Z:\...` path returned with U+3254 where each `\` had been) and decoded as a
// one-off. The second case cost a user-visible bug: a build link pasted into the System>Quit field
// came back with U+2473 where its `?` had been, so `validate_build_url` said "that link has no ?b=
// build id" and the field refused a link that was correct (runtime-captured 2026-08-23,
// `dll:6808d66f`, on `https://er-build-planner.nyasu.business/?b=bc2a932db14675`).
//
// The two placeholders name themselves once looked up: U+2473 is circled number twenty and U+3254
// is circled number twenty-four. The controller is emitting an index into some table of substituted
// characters, drawn as a circled numeral -- `?` is #20 and `\` is #24. Two points do not give the
// rest of that table, and it is not a contiguous u16 array anywhere in the image (both constants
// were searched for; their only co-location is inside high-entropy data).
//
// So this table holds what has been measured, and anything else non-ASCII is reported rather than
// guessed at -- see `native_field_text_sentinel_report`. The next unknown placeholder arrives in the
// log as its own code point and becomes a one-line addition here with evidence behind it, instead
// of another silent "that link is invalid".
const NATIVE_FIELD_SENTINELS: [(char, char); 2] = [('\u{2473}', '?'), ('\u{3254}', '\\')];

/// Decode every measured transport placeholder back to the character the player actually typed.
///
/// Applied to both editors now. Scoping it to paths was the mistake that shipped the bug: a URL has
/// no backslashes, so the build-url field skipped the decode entirely -- and then lost its `?`.
fn decode_native_field_text(text: String) -> String {
    let mut out = text;
    for (sentinel, decoded) in NATIVE_FIELD_SENTINELS {
        if out.contains(sentinel) {
            out = out.replace(sentinel, decoded.encode_utf8(&mut [0_u8; 4]));
        }
    }
    out
}

/// Any non-ASCII left after decoding, as `U+XXXX` codes, or `None` when the text is clean.
///
/// A planner link and a Windows path are both pure ASCII, so anything left here is a placeholder
/// this DLL has not learned yet. Naming the code point turns the next occurrence into a fix rather
/// than an investigation.
fn native_field_text_sentinel_report(text: &str) -> Option<String> {
    let unknown: Vec<String> = text
        .chars()
        .filter(|c| !c.is_ascii())
        .map(|c| format!("U+{:04X}", c as u32))
        .collect();
    (!unknown.is_empty()).then(|| unknown.join(" "))
}

unsafe fn software_keyboard_text(job: usize) -> Option<String> {
    let controller = unsafe { safe_read_usize(job + SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET) }?;
    if controller == 0 || controller == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let text = controller + SOFTWARE_KEYBOARD_CONTROLLER_TEXT_80_OFFSET;
    let length = unsafe { safe_read_usize(text + DLSTRING_LENGTH_18_OFFSET) }?;
    let capacity = unsafe { safe_read_usize(text + DLSTRING_CAPACITY_20_OFFSET) }?;
    if length > SOFTWARE_KEYBOARD_MAX_PATH_UNITS {
        return None;
    }
    let data = if capacity > 7 {
        unsafe { safe_read_usize(text + DLSTRING_DATA_08_OFFSET) }?
    } else {
        text + DLSTRING_DATA_08_OFFSET
    };
    if data == 0 || data == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let mut units = Vec::with_capacity(length);
    for index in 0..length {
        units.push(unsafe { safe_read_u16(data + index * 2) }?);
    }
    String::from_utf16(&units).ok()
}

unsafe extern "system" fn software_keyboard_result_gate_hook(
    job: usize,
    result: usize,
    time: usize,
    d: usize,
) -> usize {
    let original_addr = SOFTWARE_KEYBOARD_RESULT_GATE_ORIG.load(Ordering::SeqCst);
    if original_addr == HOOK_ORIGINAL_UNSET {
        return result;
    }
    // Through the union's own shape: this rides `register_union_hook`, so the slot may hold the
    // next handler on the address rather than the game trampoline.
    let original: er_hook::UnionFn = unsafe { std::mem::transmute(original_addr) };
    let Some(purpose) = keyboard_owner_of(job) else {
        return unsafe { original(job, result, time, d) };
    };

    // Preserve the native accepted-state and intermediate cleanup chain. The owned terminal d220
    // detour below replaces only the callback leaf that our intentionally-empty std::function cannot
    // satisfy. Cancellation still terminates here and never reaches d220.
    let ret = unsafe { original(job, result, time, d) };
    let result_state = unsafe { safe_read_i32(result) }.unwrap_or(0);
    if result_state == MENU_JOB_STATE_FAILED {
        // The back action lands here. `FUN_14081d3d0` reads the controller's result code
        // (`+0x78`) once the keyboard has closed and reports Failed for anything but 2, so a
        // cancel is a native verdict rather than something inferred from absence. Recording it as
        // `Cancelled` is what lets the build-url editor tell "the player backed out" apart from
        // "the player accepted something invalid" -- only the second re-opens.
        *keyboard_outcome_slot(purpose)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PathEditorOutcome::Cancelled);
        keyboard_active_job_slot(purpose).store(0, Ordering::SeqCst);
        forget_released_keyboard_job(job);
        append_autoload_debug(format_args!(
            "{}: native SoftwareKeyboard cancelled job=0x{job:x}; nothing is applied",
            keyboard_tag(purpose)
        ));
    }
    ret
}

unsafe extern "system" fn software_keyboard_terminal_callback_hook(
    job: usize,
    result: usize,
    time: usize,
    d: usize,
) -> usize {
    let original_addr = SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_ORIG.load(Ordering::SeqCst);
    if original_addr == HOOK_ORIGINAL_UNSET {
        return result;
    }
    let Some(purpose) = keyboard_owner_of(job) else {
        // Through the union's own shape -- see the sibling gate hook above.
        let original: er_hook::UnionFn = unsafe { std::mem::transmute(original_addr) };
        return unsafe { original(job, result, time, d) };
    };

    let outcome = match unsafe { software_keyboard_text(job) } {
        Some(raw_text) => {
            // Both purposes decode. The first version only decoded for paths, on the reasoning
            // that "a URL has no backslashes" -- true, and beside the point: the controller also
            // substitutes `?`, so the build-url field silently lost the one character that makes a
            // link importable.
            let text = decode_native_field_text(raw_text.clone());
            if let Some(unknown) = native_field_text_sentinel_report(&text) {
                append_autoload_debug(format_args!(
                    "{}: the field returned placeholder(s) this DLL cannot decode: {unknown}. They are circled numerals standing in for punctuation; add them to NATIVE_FIELD_SENTINELS.",
                    keyboard_tag(purpose)
                ));
            }
            append_autoload_debug(format_args!(
                "{}: native editor accepted raw={raw_text:?} text={text:?} utf16_units={}",
                keyboard_tag(purpose),
                text.encode_utf16().count()
            ));
            PathEditorOutcome::Accepted(text)
        }
        None => PathEditorOutcome::TextUnreadable,
    };
    *keyboard_outcome_slot(purpose)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outcome);
    keyboard_active_job_slot(purpose).store(0, Ordering::SeqCst);
    // The job is finished, so our responsibility for its empty callback ends here and the pointer
    // must not linger in the released list, where a recycled address would be claimed by mistake.
    forget_released_keyboard_job(job);

    // Exact native d220 tail after callback return: SetResult(Success, 0), then restore FD4Time's
    // two vtables in order. This skips only the null job+0x1a0 callback invocation.
    if result != 0 && result != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe {
            *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
            *((result + 4) as *mut i32) = 0;
        }
    }
    if time != 0
        && time != TITLE_OWNER_SCAN_START_ADDRESS
        && let Ok(base) = game_module_base()
    {
        unsafe {
            *(time as *mut usize) =
                er_game_base::mem::game_data_addr(base, FD4_TIME_VTABLE_RVA, "FD4_TIME_VTABLE_RVA");
            *(time as *mut usize) = er_game_base::mem::game_data_addr(
                base,
                FD4_TIME_FLOAT_VTABLE_RVA,
                "FD4_TIME_FLOAT_VTABLE_RVA",
            );
        }
    }
    append_autoload_debug(format_args!(
        "{}: native SoftwareKeyboard terminal accepted job=0x{job:x}; captured the exact UTF-16 text and skipped the empty callback",
        keyboard_tag(purpose)
    ));
    result
}

/// Submit the build-url editor's keyboard. Called only from the build-url editor's own pump, which
/// runs in the same menu-pump context the path editor's does.
///
/// # Safety
///
/// Menu-pump context, `dialog` a live System>Quit `PropertyEditDialog`.
pub unsafe fn submit_build_url_keyboard(dialog: usize, initial: &[u16]) -> bool {
    matches!(
        unsafe { submit_software_keyboard(KeyboardPurpose::BuildUrl, dialog, initial) },
        PathEditorSubmit::Submitted
    )
}

/// Is the build-url keyboard up right now?
pub fn build_url_keyboard_active() -> bool {
    keyboard_active_job_slot(KeyboardPurpose::BuildUrl).load(Ordering::SeqCst) != 0
}

/// What the player did with the build-url keyboard, taken exactly once.
pub enum BuildUrlKeyboardOutcome {
    /// Accept, with the exact text the field held.
    Accepted(String),
    /// The back action. Nothing is applied.
    Cancelled,
    /// Accept, but the text could not be read out of the controller.
    TextUnreadable,
}

/// Take the build-url editor's pending outcome, if the native job has produced one.
pub fn take_build_url_keyboard_outcome() -> Option<BuildUrlKeyboardOutcome> {
    let taken = keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()?;
    Some(match taken {
        PathEditorOutcome::Accepted(text) => BuildUrlKeyboardOutcome::Accepted(text),
        PathEditorOutcome::Cancelled => BuildUrlKeyboardOutcome::Cancelled,
        PathEditorOutcome::TextUnreadable => BuildUrlKeyboardOutcome::TextUnreadable,
    })
}

/// Forget every build-url keyboard pointer. Called when the Quit dialog goes away, so a later press
/// cannot resolve against a dead job.
pub fn reset_build_url_keyboard_state() {
    keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(0, Ordering::SeqCst);
    BUILD_URL_EDITOR_WINDOW.store(0, Ordering::SeqCst);
    *keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

pub fn save_picker_request_path_editor(dialog: usize) {
    if dialog != 0 && dialog != TITLE_OWNER_SCAN_START_ADDRESS {
        let cleared_stale_warning = er_save_picker_core::model::active_save_picker_lock()
            .as_mut()
            .is_some_and(er_save_picker_core::SavePickerModel::begin_path_edit);
        SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-path: activation requested dialog=0x{dialog:x} cleared_stale_warning={cleared_stale_warning}"
        ));
    }
}

unsafe fn submit_path_editor(dialog: usize) -> PathEditorSubmit {
    let initial = {
        let guard = er_save_picker_core::model::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return PathEditorSubmit::Rejected;
        };
        let Some(path) = model.current_dir().to_str() else {
            return PathEditorSubmit::Rejected;
        };
        path.encode_utf16()
            .chain(core::iter::once(0))
            .collect::<Vec<_>>()
    };
    let submitted =
        unsafe { submit_software_keyboard(KeyboardPurpose::SavePath, dialog, &initial) };
    if submitted == PathEditorSubmit::Submitted {
        SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.store(dialog, Ordering::SeqCst);
        SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
        SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK.store(
            er_telemetry_core::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
    }
    submitted
}

/// Build and submit one native `CS::SoftwareKeyboardJob` on `dialog`'s own MenuJobQueue, prefilled
/// with `initial` (NUL-terminated UTF-16).
///
/// Owner-agnostic on purpose: the save picker and the System>Quit build-url row differ only in what
/// they prefill and what they do with the answer, and duplicating this would mean a second
/// allocation/ctor/submit sequence to keep in step with the native one -- and, worse, a second pair
/// of detours on the same two prologues.
///
/// # which arguments the native side keeps, and for how long
///
/// Every pointer below is handed to code that outlives this call, so "does it copy?" is settled
/// here from the 1.16.2 dump rather than re-litigated each time the field misbehaves. It has been
/// blamed for a stale field twice, and it was not the cause either time.
///
/// * `initial` is copied three times over, and none of the copies keeps the pointer.
///   `enter_name` (`0xe70c00`) and `set_initial` (`0xe709f0`) both run it through
///   `DLTX::DLString::CopyFromU16Array` into a stack `DLString` and then assign that into the
///   validator with `0x142416ef0` (a `DLString::substr(dst, src, 0, -1)`). The ctor takes it a third
///   time as its 5th argument and calls `DLTX::DLString<wchar_t>::FromU16Array(job+0xe8, initial,
///   GetMenuHeapAllocator())` -- which measures the string, allocates on the menu heap and copies.
///   The live log is the independent confirmation: a field left open for 36 seconds returned
///   exactly the 43 code units it was opened with.
/// * `validator` is deep-copied into `job+0x60` by `0x1407f3eb0`, which runs `DLString::Copy` over
///   both of its strings. That is why destroying the stack validator immediately after the ctor is
///   correct and not a use-after-free.
/// * `config` is copied by value into `job+0x150` (16 bytes, one `MOVUPS` pair). The struct's
///   `resource` pointer is copied with it and dereferenced much later -- see
///   [`TEXT_INPUT_RESOURCE`], which is a `static` for exactly that reason.
/// * `empty_callback` is consumed during the ctor: it reads slot 7 (the MSVC `std::function`
///   pointer), and ours is zeroed, so nothing is cloned and `job+0x1a0` stays null. That null is
///   what the terminal-callback detour exists to stand in for -- the native leaf would
///   `ThrowBadFunctionCallException` on it.
///
/// # Safety
///
/// Menu-pump context, with `dialog` a live `PropertyEditDialog`/`GenericListSelectDialog`. Calls
/// byte-verified native functions and allocates from the game's own menu heap.
unsafe fn submit_software_keyboard(
    purpose: KeyboardPurpose,
    dialog: usize,
    initial: &[u16],
) -> PathEditorSubmit {
    if keyboard_active_job_slot(purpose).load(Ordering::SeqCst) != 0 {
        return PathEditorSubmit::RetryWhenQueueReady;
    }
    // One keyboard at a time across both purposes. The queue is per dialog, but the native
    // keyboard is a single on-screen surface driven by one controller, so a second job submitted
    // while the first is up would leave two owners waiting on one answer.
    let other_editor_busy = [KeyboardPurpose::SavePath, KeyboardPurpose::BuildUrl]
        .into_iter()
        .any(|other| {
            other != purpose && keyboard_active_job_slot(other).load(Ordering::SeqCst) != 0
        });
    if other_editor_busy {
        return PathEditorSubmit::RetryWhenQueueReady;
    }
    let Some(recipe) = software_keyboard_recipe() else {
        append_autoload_debug(format_args!(
            "save-picker-path: native SoftwareKeyboard recipe unavailable; refusing unsafe call"
        ));
        return PathEditorSubmit::Rejected;
    };
    if !install_software_keyboard_result_hooks() {
        append_autoload_debug(format_args!(
            "save-picker-path: result hooks unavailable; refusing a job whose terminal callback cannot be captured safely"
        ));
        return PathEditorSubmit::Rejected;
    }

    let queue = dialog + SYSTEM_QUIT_DIALOG_MENU_JOB_QUEUE_10_OFFSET;
    let queue_ready: unsafe extern "system" fn(usize) -> u8 =
        unsafe { std::mem::transmute(recipe.queue_ready) };
    if unsafe { queue_ready(queue) } == 0 {
        return PathEditorSubmit::RetryWhenQueueReady;
    }
    let mut validator = [0_u64; SOFTWARE_KEYBOARD_VALIDATOR_SIZE / 8];
    let validator_ptr = validator.as_mut_ptr() as usize;
    let validator_init: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(recipe.validator_init) };
    let enter_name: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(recipe.enter_name) };
    let set_initial: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(recipe.set_initial) };
    let set_max: unsafe extern "system" fn(usize, i32) =
        unsafe { std::mem::transmute(recipe.set_max) };
    let validator_dtor: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(recipe.validator_dtor) };
    unsafe {
        validator_init(validator_ptr);
        enter_name(validator_ptr, initial.as_ptr() as usize);
        set_max(validator_ptr, SOFTWARE_KEYBOARD_MAX_PATH_UNITS as i32);
        *((validator_ptr + SOFTWARE_KEYBOARD_VALIDATOR_MAX_6C_OFFSET) as *mut u32) =
            SOFTWARE_KEYBOARD_MAX_PATH_UNITS as u32;
        let flags = (validator_ptr + SOFTWARE_KEYBOARD_VALIDATOR_FLAGS_68_OFFSET) as *mut u32;
        *flags &= !2;
        set_initial(validator_ptr, initial.as_ptr() as usize);
    }
    debug_assert_eq!(
        unsafe { safe_read_i32(validator_ptr + SOFTWARE_KEYBOARD_VALIDATOR_MAX_60_OFFSET) },
        Some(SOFTWARE_KEYBOARD_MAX_PATH_UNITS as i32)
    );

    let Ok(base) = game_module_base() else {
        unsafe { validator_dtor(validator_ptr) };
        return PathEditorSubmit::Rejected;
    };
    let allocator = match unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GLOBAL_MENU_HEAP_ALLOCATOR_RVA,
            "GLOBAL_MENU_HEAP_ALLOCATOR_RVA",
        ))
    } {
        Some(allocator) if allocator != 0 && allocator != TITLE_OWNER_SCAN_START_ADDRESS => {
            allocator
        }
        _ => {
            unsafe { validator_dtor(validator_ptr) };
            return PathEditorSubmit::Rejected;
        }
    };
    let heap_alloc: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(recipe.heap_alloc) };
    let memory = unsafe { heap_alloc(SOFTWARE_KEYBOARD_JOB_SIZE, 8, allocator) };
    if memory == 0 || memory == TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { validator_dtor(validator_ptr) };
        return PathEditorSubmit::Rejected;
    }

    let config = SoftwareKeyboardConfig {
        max_units: SOFTWARE_KEYBOARD_MAX_PATH_UNITS as u32,
        mode: 1,
        padding: [0; 3],
        resource: keyboard_resource(purpose),
    };
    let empty_callback = [0_usize; 8];
    let ctor: unsafe extern "system" fn(usize, usize, usize, usize, usize, u8, usize) -> usize =
        unsafe { std::mem::transmute(recipe.ctor) };
    let job = unsafe {
        ctor(
            memory,
            dialog + SYSTEM_QUIT_DIALOG_MENU_WINDOW_LIST_50_OFFSET,
            validator_ptr,
            (&raw const config) as usize,
            initial.as_ptr() as usize,
            1,
            empty_callback.as_ptr() as usize,
        )
    };
    unsafe { validator_dtor(validator_ptr) };
    if job == 0 || job == TITLE_OWNER_SCAN_START_ADDRESS {
        return PathEditorSubmit::Rejected;
    }

    unsafe {
        let refcount = (job + MENU_JOB_REFCOUNT_08_OFFSET) as *mut std::sync::atomic::AtomicI32;
        (*refcount).fetch_add(1, Ordering::SeqCst);
    }
    keyboard_active_job_slot(purpose).store(job, Ordering::SeqCst);
    *keyboard_outcome_slot(purpose)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let mut job_slot = job;
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(recipe.submit) };
    unsafe { submit(queue, (&raw mut job_slot) as usize) };
    append_autoload_debug(format_args!(
        "{}: submitted native SoftwareKeyboardJob=0x{job:x} dialog=0x{dialog:x} queue=0x{queue:x} initial_units={} max_units={SOFTWARE_KEYBOARD_MAX_PATH_UNITS}",
        keyboard_tag(purpose),
        initial.len().saturating_sub(1)
    ));
    PathEditorSubmit::Submitted
}

fn apply_path_editor_outcome(dialog: usize, outcome: PathEditorOutcome) {
    let mut guard = er_save_picker_core::model::active_save_picker_lock();
    let Some(model) = guard.as_mut() else {
        return;
    };
    match outcome {
        PathEditorOutcome::Accepted(path) => match model.set_current_dir_from_text(&path) {
            Ok(changed) => append_autoload_debug(format_args!(
                "save-picker-path: committed accepted directory changed={changed} exact='{}'",
                model.current_dir().display()
            )),
            Err(reason) => {
                model.set_status_message(reason.status_message());
                // Keep what was typed on the control, marked invalid, so it can be corrected in
                // place. Ordering matters: `set_status_message` does not clear the rejected text,
                // while a later valid entry refreshes the listing and drops it automatically.
                model.set_rejected_path_text(&path);
                append_autoload_debug(format_args!(
                    "save-picker-path: rejected accepted text reason={reason:?}; keeping '{path}' on the control as invalid; directory remains '{}'",
                    model.current_dir().display()
                ));
            }
        },
        PathEditorOutcome::Cancelled => {
            append_autoload_debug(format_args!(
                "save-picker-path: cancel consumed; directory remains '{}'",
                model.current_dir().display()
            ));
        }
        PathEditorOutcome::TextUnreadable => {
            model.set_status_message(er_save_picker_core::PickerStatusMessage::new(
                "PATH TEXT UNREADABLE",
                "The native editor returned invalid UTF-16; the folder was not changed.",
            ));
        }
    }
    if unsafe { save_picker_stage_row_records(model) } {
        SAVE_PICKER_REBUILD_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
    }
}

/// Menu-pump-owned submit/result bridge. The native text editor and its job queue are never touched
/// from FrameBegin or the recurring game task.
///
/// # Safety
///
/// Menu-pump context only. It submits to and drains the native job queue, which is not serialised
/// against FrameBegin or the recurring game task.
pub unsafe fn save_picker_menu_pump_path_editor() {
    let active_before_watchdog = SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst);
    let editor_window = SAVE_PICKER_PATH_EDITOR_WINDOW.load(Ordering::SeqCst);
    if active_before_watchdog != 0 && editor_window != 0 {
        let last = SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK.load(Ordering::SeqCst);
        let now =
            er_telemetry_core::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst);
        if now.saturating_sub(last) >= PATH_EDITOR_WINDOW_STALE_PROFILE_TICKS
            && SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB
                .compare_exchange(
                    active_before_watchdog,
                    0,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        {
            SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
            *path_editor_outcome()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(PathEditorOutcome::Cancelled);
            append_autoload_debug(format_args!(
                "save-picker-path: 02_990 MenuWindow stopped running for {} ProfileSelect ticks; released stale job=0x{active_before_watchdog:x} window=0x{editor_window:x} as cancelled before reading freed controller state",
                now.saturating_sub(last)
            ));
        }
    }

    let active_job = SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst);
    if active_job != 0
        && unsafe { software_keyboard_result_state(active_job) } == Some(MENU_JOB_STATE_FAILED)
        && SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB
            .compare_exchange(active_job, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        *path_editor_outcome()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PathEditorOutcome::Cancelled);
        append_autoload_debug(format_args!(
            "save-picker-path: observed native SoftwareKeyboard failed/cancelled state for job=0x{active_job:x}; released editor ownership"
        ));
    }

    let outcome = path_editor_outcome()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(outcome) = outcome {
        let dialog = SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.swap(0, Ordering::SeqCst);
        SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
        if dialog != 0 {
            apply_path_editor_outcome(dialog, outcome);
        }
    }

    if SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst) != 0 {
        return;
    }
    let dialog = SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.load(Ordering::SeqCst);
    if dialog == 0 {
        return;
    }
    match unsafe { submit_path_editor(dialog) } {
        PathEditorSubmit::Submitted => {
            SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(0, Ordering::SeqCst);
            let guard = er_save_picker_core::model::active_save_picker_lock();
            if let Some(model) = guard.as_ref()
                && unsafe { save_picker_stage_row_records(model) }
            {
                SAVE_PICKER_REBUILD_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
            }
        }
        PathEditorSubmit::RetryWhenQueueReady => {}
        PathEditorSubmit::Rejected => {
            SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(0, Ordering::SeqCst);
            let mut guard = er_save_picker_core::model::active_save_picker_lock();
            if let Some(model) = guard.as_mut() {
                model.set_status_message(er_save_picker_core::PickerStatusMessage::new(
                    "PATH EDITOR UNAVAILABLE",
                    "The native text editor could not be opened; the current folder was not changed.",
                ));
                if unsafe { save_picker_stage_row_records(model) } {
                    SAVE_PICKER_REBUILD_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared process STATICS, raced by parallel test threads.
    ///
    /// The BuildUrl/SavePath keyboard-purpose slots below (`keyboard_active_job_slot`,
    /// `keyboard_outcome_slot`, `BUILD_URL_EDITOR_WINDOW`, `BUILD_URL_EDITOR_WINDOW_LAST_TICK`,
    /// `BUILD_URL_MENU_PUMP_TICKS`, `RELEASED_KEYBOARD_JOBS`) are process-wide `static`s with no
    /// synchronization of their own, and `cargo test` runs every test in this module on its own
    /// thread by default. Left unguarded, one test's setup races another's assertion on the same
    /// static: measured by running the full suite repeatedly (`cargo xwin test --lib`) on the base
    /// commit -- the failure always has the shape "112 passed; 1 failed", but which of the tests
    /// below fails changes from run to run
    /// (`a_window_close_after_an_accept_does_not_clobber_the_accepted_text` one run,
    /// `a_link_field_window_closing_releases_a_latch_no_detour_cleared` the next), always landing on
    /// a test that touches this state. The production code is not at fault -- the named failing test
    /// passes deterministically when run alone. Every test that touches this shared state takes this
    /// lock first, so their setup/act/assert runs atomically with respect to one another; tests that
    /// do not touch it are unaffected and keep running in parallel.
    static KEYBOARD_STATE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn native_keyboard_config_matches_the_static_constructor_copy() {
        assert_eq!(core::mem::size_of::<SoftwareKeyboardConfig>(), 0x10);
        assert_eq!(SOFTWARE_KEYBOARD_VALIDATOR_SIZE, 0x70);
        assert_eq!(SOFTWARE_KEYBOARD_JOB_SIZE, 0x1a8);
        assert_eq!(
            String::from_utf16(&TEXT_INPUT_RESOURCE[..TEXT_INPUT_RESOURCE.len() - 1]).unwrap(),
            "02_990_TextInput_PathEditor"
        );
    }

    /// The two cache keys must not collide, and each must be NUL-terminated.
    ///
    /// Sharing one key is what shipped the unstyled link field: Scaleform handed the Quit tab the
    /// save picker's derived movie, whose chrome is deliberately hidden. If these two strings ever
    /// become equal again the symptom returns silently -- the field still opens, it just has no box
    /// around it.
    #[test]
    fn each_editor_asks_scaleform_for_its_own_movie() {
        assert_ne!(TEXT_INPUT_RESOURCE_NAME, BUILD_URL_TEXT_INPUT_RESOURCE_NAME);
        assert_ne!(
            keyboard_resource(KeyboardPurpose::SavePath),
            keyboard_resource(KeyboardPurpose::BuildUrl)
        );
        for (name, units) in [
            (TEXT_INPUT_RESOURCE_NAME, &TEXT_INPUT_RESOURCE[..]),
            (
                BUILD_URL_TEXT_INPUT_RESOURCE_NAME,
                &BUILD_URL_TEXT_INPUT_RESOURCE[..],
            ),
        ] {
            assert_eq!(units.len(), name.len() + 1, "one trailing NUL, no padding");
            assert_eq!(*units.last().unwrap(), 0, "the engine reads to the NUL");
            assert_eq!(String::from_utf16(&units[..units.len() - 1]).unwrap(), name);
        }
    }

    #[test]
    fn native_keyboard_separator_sentinel_decodes_to_a_windows_backslash() {
        assert_eq!(
            decode_native_field_text("Z:㉔home㉔banon㉔saves".to_owned()),
            r"Z:\home\banon\saves"
        );
    }

    /// The exact bug, as the game produced it. Captured 2026-08-23 from `dll:6808d66f`: a correct
    /// build link was typed into the System>Quit field and came back with U+2473 where its `?` had
    /// been, so the gate reported "that link has no ?b= build id" and refused it eight times.
    ///
    /// The decode must restore the `?` and the restored text must satisfy the same validator the
    /// field runs -- decoding it into something the gate still rejects would fix nothing.
    #[test]
    fn the_question_mark_sentinel_decodes_and_the_link_then_validates() {
        let from_the_field = "https://er-build-planner.nyasu.business/\u{2473}b=bc2a932db14675";
        let decoded = decode_native_field_text(from_the_field.to_owned());
        assert_eq!(
            decoded,
            "https://er-build-planner.nyasu.business/?b=bc2a932db14675"
        );
        assert_eq!(
            er_build_import_core::validate_build_url(&decoded),
            Ok("bc2a932db14675"),
            "decoding must produce a link the gate accepts"
        );
        assert!(er_build_import_core::validate_build_url(from_the_field).is_err());
    }

    /// Both placeholders in one string, ASCII left alone. A link and a path never legitimately
    /// contain non-ASCII, which is what makes "anything else non-ASCII" a reliable signal.
    #[test]
    fn decoding_is_confined_to_the_measured_placeholders() {
        assert_eq!(
            decode_native_field_text("a\u{2473}b\u{3254}c".to_owned()),
            "a?b\\c"
        );
        let clean = "https://p/?b=abc123";
        assert_eq!(decode_native_field_text(clean.to_owned()), clean);
        assert_eq!(native_field_text_sentinel_report(clean), None);
    }

    /// An unknown placeholder must be named, not swallowed. That is what makes the next one cost a
    /// line of code instead of an investigation.
    #[test]
    fn an_undecoded_placeholder_is_reported_by_code_point() {
        let decoded = decode_native_field_text("https://p/?b=abc\u{2460}123".to_owned());
        assert_eq!(
            native_field_text_sentinel_report(&decoded).as_deref(),
            Some("U+2460")
        );
        let decoded = decode_native_field_text("x\u{2473}y\u{3254}z".to_owned());
        assert_eq!(native_field_text_sentinel_report(&decoded), None);
    }

    /// The detour pair is shared by two editors and by the game itself, so the one thing that must
    /// never break is that a dispatched job is attributed to exactly the purpose that submitted it.
    /// A job owned by neither is the game's own keyboard (character naming) and must stay untouched.
    #[test]
    fn terminal_capture_is_scoped_to_the_owning_purpose() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for purpose in [KeyboardPurpose::SavePath, KeyboardPurpose::BuildUrl] {
            keyboard_active_job_slot(purpose).store(0, Ordering::SeqCst);
        }
        assert_eq!(keyboard_owner_of(0x1234), None, "no owner claims it yet");
        assert_eq!(keyboard_owner_of(0), None, "job 0 is never owned");

        keyboard_active_job_slot(KeyboardPurpose::SavePath).store(0x1234, Ordering::SeqCst);
        assert_eq!(keyboard_owner_of(0x1234), Some(KeyboardPurpose::SavePath));
        assert_eq!(keyboard_owner_of(0x5678), None, "a foreign job is unowned");

        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(0x5678, Ordering::SeqCst);
        assert_eq!(keyboard_owner_of(0x5678), Some(KeyboardPurpose::BuildUrl));
        assert_eq!(
            keyboard_owner_of(0x1234),
            Some(KeyboardPurpose::SavePath),
            "the two purposes must not shadow each other"
        );
        // Separate mailboxes: one editor's outcome can never be read by the other.
        assert!(!std::ptr::eq(
            keyboard_outcome_slot(KeyboardPurpose::SavePath),
            keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
        ));

        for purpose in [KeyboardPurpose::SavePath, KeyboardPurpose::BuildUrl] {
            keyboard_active_job_slot(purpose).store(0, Ordering::SeqCst);
        }
        assert_eq!(SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET, 0xd8);
        assert_eq!(SOFTWARE_KEYBOARD_CONTROLLER_TEXT_80_OFFSET, 0x80);
        assert_eq!(FD4_TIME_VTABLE_RVA, 0x29c8e58);
        assert_eq!(FD4_TIME_FLOAT_VTABLE_RVA, 0x29c8e48);
    }

    #[test]
    fn terminal_window_states_are_never_transform_targets() {
        assert!(text_input_02_990_window_is_live(0));
        assert!(text_input_02_990_window_is_live(MENU_JOB_STATE_CONTINUE));
        assert!(!text_input_02_990_window_is_live(MENU_JOB_STATE_SUCCESS));
        assert!(!text_input_02_990_window_is_live(MENU_JOB_STATE_FAILED));
    }

    #[test]
    fn path_limit_exceeds_the_native_name_presets_without_becoming_unbounded() {
        const {
            assert!(SOFTWARE_KEYBOARD_MAX_PATH_UNITS > 16);
            assert!(SOFTWARE_KEYBOARD_MAX_PATH_UNITS <= 1024);
        }
    }
    /// The second link field of a session must re-arm its own latches, not a neighbour's.
    ///
    /// `reset_build_url_field_latches` is keyed to the 0 -> window transition because the allocator
    /// hands back the same window pointer across opens. Until 2026-09-11 that transition called the
    /// save picker's reset instead, so the link field's caret pass and placement counter ran once
    /// per process: field two opened with the caret at index 0 and typing prepended to the
    /// prefilled link. The placement attempt counter is the observable half of the same latch, so
    /// it is what this asserts.
    #[test]
    fn a_second_link_field_re_arms_the_caret_and_placement_latches() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let window = 0xa0a0_0000;
        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(0, Ordering::SeqCst);
        BUILD_URL_EDITOR_WINDOW.store(0, Ordering::SeqCst);
        crate::scaleform_proxy::reset_build_url_field_latches();

        assert!(build_url_note_editor_window_state(
            window,
            MENU_JOB_STATE_CONTINUE
        ));
        // Stand in for the placement passes a live field takes every frame it is up.
        crate::scaleform_proxy::note_build_url_window_position_attempt_for_test();
        crate::scaleform_proxy::note_build_url_window_position_attempt_for_test();
        assert_eq!(
            crate::scaleform_proxy::build_url_window_position_counts().0,
            2,
            "the open field must be counting its own placement passes"
        );

        assert!(!build_url_note_editor_window_state(
            window,
            MENU_JOB_STATE_FAILED
        ));
        // The closed window is refused until the game asks for the movie again, which is what
        // says a new field is being built. Without this the pointer is still the previous
        // field's, and adopting it is what let a dying window cancel its successor.
        assert!(
            !build_url_note_editor_window_state(window, MENU_JOB_STATE_CONTINUE),
            "a window that has closed is not a fresh field just because it runs again"
        );
        build_url_note_movie_served();
        assert!(build_url_note_editor_window_state(
            window,
            MENU_JOB_STATE_CONTINUE
        ));

        assert_eq!(
            crate::scaleform_proxy::build_url_window_position_counts().0,
            0,
            "a fresh field starts its latches over, or its caret pass never runs again"
        );
    }

    /// A window that outlives its own field must not cancel the field that replaces it.
    ///
    /// Run br-20260911-152041-9dff: `link field requested` at `+66928ms`, and at `+66930ms` --
    /// two milliseconds later, before the new field's window had run once -- the previous field's
    /// window went terminal and released the job that had just been latched. The player had opened
    /// a second link field and it was cancelled out from under them.
    #[test]
    fn a_stale_window_closing_does_not_cancel_the_field_that_replaced_it() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let window = 0xb0b0_0000;
        let fresh_job = 0x7777_0000;

        // The lingering window: still reported live, but nothing is latched, so it owns no job.
        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(0, Ordering::SeqCst);
        BUILD_URL_EDITOR_WINDOW.store(0, Ordering::SeqCst);
        BUILD_URL_EDITOR_WINDOW_JOB.store(0, Ordering::SeqCst);
        assert!(build_url_note_editor_window_state(
            window,
            MENU_JOB_STATE_CONTINUE
        ));
        assert_eq!(
            BUILD_URL_EDITOR_WINDOW_JOB.load(Ordering::SeqCst),
            0,
            "a window adopted while nothing was latched cannot own a job"
        );

        // The player presses the row again: a new field, a new job.
        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(fresh_job, Ordering::SeqCst);
        *keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;

        // ...and only now does the old window go terminal.
        assert!(!build_url_note_editor_window_state(
            window,
            MENU_JOB_STATE_FAILED
        ));

        assert_eq!(
            keyboard_active_job_slot(KeyboardPurpose::BuildUrl).load(Ordering::SeqCst),
            fresh_job,
            "the new field's job must survive the old window's close"
        );
        assert!(
            keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_none(),
            "no outcome may be deposited for a field the player is still looking at"
        );
        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(0, Ordering::SeqCst);
    }

    /// Pressing B closed the field and killed the row for the rest of the session.
    ///
    /// Live session `dll:8dca09bb`, 2026-08-23: three link fields opened, each closed with the back
    /// action, and the `0x81d3d0` cancel gate fired zero times -- so the active-job slot stayed set
    /// and every later press was refused with "editor already active". The window going terminal is
    /// the signal that actually arrives, so it has to be the one that releases the latch.
    #[test]
    fn a_link_field_window_closing_releases_a_latch_no_detour_cleared() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let job = 0x4242_0000;
        let window = 0x8080_0000;
        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(job, Ordering::SeqCst);
        *keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        // Through adoption rather than by storing the window directly. The close path releases
        // only the job a window was adopted with, so a fixture that skips the live frame is
        // testing a state the game never reaches.
        BUILD_URL_EDITOR_WINDOW.store(0, Ordering::SeqCst);
        BUILD_URL_EDITOR_WINDOW_JOB.store(0, Ordering::SeqCst);
        assert!(build_url_note_editor_window_state(
            window,
            MENU_JOB_STATE_CONTINUE
        ));

        assert!(!build_url_note_editor_window_state(
            window,
            MENU_JOB_STATE_FAILED
        ));

        assert_eq!(
            keyboard_active_job_slot(KeyboardPurpose::BuildUrl).load(Ordering::SeqCst),
            0,
            "the job slot must clear, or the row refuses every future press"
        );
        assert!(
            matches!(
                keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take(),
                Some(PathEditorOutcome::Cancelled)
            ),
            "a close with nothing accepted is a cancel"
        );
    }

    /// ...but an accept must survive it. The terminal callback records the text, and the window goes
    /// terminal a frame or two later; overwriting that with `Cancelled` would drop every link the
    /// player successfully entered.
    #[test]
    fn a_window_close_after_an_accept_does_not_clobber_the_accepted_text() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let window = 0x9090_0000;
        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(0x1111_0000, Ordering::SeqCst);
        *keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PathEditorOutcome::Accepted(
            "https://example/?b=abc".to_owned(),
        ));
        BUILD_URL_EDITOR_WINDOW.store(window, Ordering::SeqCst);

        assert!(!build_url_note_editor_window_state(
            window,
            MENU_JOB_STATE_FAILED
        ));

        let outcome = keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        assert!(
            matches!(outcome, Some(PathEditorOutcome::Accepted(text)) if text.contains("?b=abc")),
            "the accepted text must outlive its window"
        );
    }
    /// A closed 02_990 window is never reported terminal -- It just stops being run.
    ///
    /// `dll:9caf1a27`, 2026-08-23: four link fields opened and closed with B, and the
    /// live->terminal release fired zero times, because the transition never reaches us. Absence is
    /// the only evidence a closed field leaves, so the latch has to be released from the window
    /// going unseen, measured in the game's own window-run ticks.
    #[test]
    fn a_link_field_window_that_stops_running_abandons_its_latch() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(0x7777_0000, Ordering::SeqCst);
        // The counter starts at 0 off the game thread, and every case here reasons about ticks
        // before `now`; give it a base so those subtractions describe a real elapsed window instead
        // of underflowing.
        let now = 1_000;
        BUILD_URL_MENU_PUMP_TICKS.store(now, Ordering::SeqCst);

        // Seen this very tick: a field that is genuinely up must never be judged abandoned.
        BUILD_URL_EDITOR_WINDOW_LAST_TICK.store(now, Ordering::SeqCst);
        assert!(!build_url_keyboard_latch_is_abandoned());

        // Seen exactly at the limit is still within tolerance.
        BUILD_URL_EDITOR_WINDOW_LAST_TICK
            .store(now - BUILD_URL_WINDOW_UNSEEN_TICK_LIMIT, Ordering::SeqCst);
        assert!(!build_url_keyboard_latch_is_abandoned());

        // Past it, the window has stopped running and the row must become pressable again.
        BUILD_URL_EDITOR_WINDOW_LAST_TICK.store(
            now - BUILD_URL_WINDOW_UNSEEN_TICK_LIMIT - 1,
            Ordering::SeqCst,
        );
        assert!(build_url_keyboard_latch_is_abandoned());

        release_abandoned_build_url_keyboard();
        assert_eq!(
            keyboard_active_job_slot(KeyboardPurpose::BuildUrl).load(Ordering::SeqCst),
            0
        );
        let _ = keyboard_outcome_slot(KeyboardPurpose::BuildUrl)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }

    /// With no keyboard latched there is nothing to abandon -- the watchdog must stay silent rather
    /// than firing every frame the menu is open.
    #[test]
    fn an_idle_link_field_is_never_abandoned() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(0, Ordering::SeqCst);
        BUILD_URL_EDITOR_WINDOW_LAST_TICK.store(1, Ordering::SeqCst);
        assert!(!build_url_keyboard_latch_is_abandoned());
    }
    /// The watchdog is only as good as the clock it reads.
    ///
    /// Its first version read `PROFILE_SELECT_WINDOW_RUN_TICKS`, which is incremented only by the
    /// `05_010_ProfileSelect` branch of the run post-hook. The link field lives on the System>Quit
    /// tab, where that window never runs, so the counter was frozen, `now - last` was always 0, and
    /// no latch could ever be judged abandoned -- an inert watchdog that shipped and tested green
    /// (`dll:f9d11870`, 2026-08-23: four opens, zero releases). This pins the clock to the pump.
    #[test]
    fn the_abandon_clock_advances_on_the_pump_not_on_profile_select() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let before = BUILD_URL_MENU_PUMP_TICKS.load(Ordering::SeqCst);
        let after = build_url_menu_pump_tick();
        assert_eq!(
            after,
            before + 1,
            "each pump pass must advance the clock the watchdog reads"
        );
        assert!(
            build_url_menu_pump_tick() > after,
            "and keep advancing, or an abandoned latch is never noticed"
        );
    }
    /// Releasing the latch must not DISOWN the job -- That crashed the game.
    ///
    /// `dll:a71aa552`, 2026-08-23: `0xe06d7363` -> `ThrowBadFunctionCallException` inside
    /// `FUN_14081d220+0xf8`, then `NtTerminateProcess(0xc0000005)`. The job carries an
    /// intentionally empty `std::function`, safe only because our detour claims the job and never
    /// lets the engine invoke it. The abandoned-latch watchdog cleared the active-job slot while
    /// the job was still alive, `keyboard_owner_of` stopped recognising it, the detour forwarded,
    /// and the engine called an empty function.
    #[test]
    fn a_released_job_is_still_ours_to_intercept() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let job = 0x5150_0000;
        RELEASED_KEYBOARD_JOBS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        keyboard_active_job_slot(KeyboardPurpose::BuildUrl).store(0, Ordering::SeqCst);
        assert!(
            keyboard_owner_of(job).is_none(),
            "a job nobody created is not ours"
        );

        remember_released_keyboard_job(job, KeyboardPurpose::BuildUrl);
        assert_eq!(
            keyboard_owner_of(job),
            Some(KeyboardPurpose::BuildUrl),
            "the detour MUST still claim it, or the engine reaches the empty std::function"
        );

        forget_released_keyboard_job(job);
        assert!(
            keyboard_owner_of(job).is_none(),
            "once the job has finished, a recycled address must not be claimed by mistake"
        );
    }

    /// The released list is a safety net, not a leak: it cannot grow without bound, and it keeps
    /// the newest entries, which are the ones most likely still alive.
    #[test]
    fn the_released_job_list_is_bounded_and_keeps_the_newest() {
        let _guard = KEYBOARD_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        RELEASED_KEYBOARD_JOBS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        for index in 1..=(RELEASED_KEYBOARD_JOB_LIMIT + 4) {
            remember_released_keyboard_job(0x1000 * index, KeyboardPurpose::BuildUrl);
        }
        let jobs = RELEASED_KEYBOARD_JOBS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(jobs.len(), RELEASED_KEYBOARD_JOB_LIMIT);
        assert_eq!(
            jobs.last().map(|(job, _)| *job),
            Some(0x1000 * (RELEASED_KEYBOARD_JOB_LIMIT + 4))
        );
        assert!(
            !jobs.iter().any(|(job, _)| *job == 0x1000),
            "the oldest entry is the one dropped"
        );
    }
}
