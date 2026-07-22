#![allow(non_snake_case)]

use std::{
    ffi::c_void,
    fmt,
    fs::{File, OpenOptions},
    io::Write,
    ptr::null_mut,
    sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering},
    sync::{Mutex, OnceLock},
};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;
const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;
const LOG_PATH: &str = "er-reload-trace.log";

const MH_OK: i32 = 0;
const MH_ERROR_ALREADY_INITIALIZED: i32 = 1;
const MH_ERROR_ENABLED: i32 = 5;

const GAME_MAN_SINGLETON_RVA: usize = 0x3d69918;
const GAME_DATA_MAN_GLOBAL_RVA: usize = 0x3d5df38;
const MOUNTED_ARCHIVE_REGISTRY_RVA: usize = 0x448464a8;

// MoveMapStep finalize advancer FUN_140afa7c0 (dump) -> deobf 0x140afa6d0 -> rva 0xafa6d0 (content-
// unique, scripts/dump-deobf-shift.py). param_1 (rcx) = MoveMapStep; field25_0x12a = the finalize
// sub-state (0..9). cVar10 (the ending request it computes) is written to menuData+0x5e; its rt5d input
// is menuData+0x5d. Hooking this proves whether the advancer is even TICKED for load2 and what it reads.
const MOVEMAPSTEP_FINALIZE_12A_OFFSET: usize = 0x12a;
/// Child teardown FUN_140eb54e0 (dump) -> deobf 0x140eb54c0 / rva 0xeb54c0. STEP_MoveMap_Update calls
/// it to tear down the MoveMapStep child (whose EzChildStepBase = MoveMapStep + 0x108). Hooking it +
/// logging the child_base-0x108 MoveMapStep state(+0x48)/field25(+0x12a) shows whether load2's
/// MoveMapStep child (state==18) is torn down at field25<9 (teardown mechanism) or never appears here
/// (never re-scheduled). rva 0xafa6d0 = the advancer; the mms= pointer in each log distinguishes loads.
const CHILD_TEARDOWN_RVA: usize = 0xeb54c0;
const MOVEMAPSTEP_CHILD_EZSTEP_OFFSET: usize = 0x108;
const MOVEMAPSTEP_STATE_48_OFFSET: usize = 0x48;
const CS_MENU_MAN_GLOBAL_RVA: usize = 0x3d6b7b0;
const CS_MENU_MAN_MENU_DATA_OFFSET: usize = 0x8;
const MENU_DATA_RT5D_OFFSET: usize = 0x5d;
const MENU_DATA_ENDING_5E_OFFSET: usize = 0x5e;

// Case-7 (7->8) save-drain gate flags read by the finalize advancer (1.16.2 FUN_140afa6d0):
// ShouldSave() reads GameMan->saveRequested (0xb72); FUN_140679370() reads GameMan+0xb73 (gated by
// bc4!=3). The suppressed in-world quit-save leaves these set, so !ShouldSave()/!FUN_140679370() fail
// and load2 parks at field25=7 even after rt5d unblocks case 0. The rt5d drive clears them natively.
const GAME_MAN_SAVE_REQUESTED_B72_OFFSET: usize = 0xb72;
const GAME_MAN_FIELD_B73_OFFSET: usize = 0xb73;
const GAME_MAN_REQUESTED_SLOT_B78_OFFSET: usize = 0xb78;
const GAME_MAN_LOAD_PHASE_B80_OFFSET: usize = 0xb80;
const GAME_MAN_SAVE_SLOT_AC0_OFFSET: usize = 0xac0;
const GAME_MAN_CURRENT_MAP_C30_OFFSET: usize = 0xc30;
const GAME_MAN_RESIDENT_DEVICE_DF0_OFFSET: usize = 0xdf0;
// LOAD-SUBMIT gate fields (bd load-submit-67dc00-gate-offsets-to-instrument-pin-load2-divergence).
// combined_load_67b940 -> submit 0x14067dc00 bails (0x14067e12f) unless these GameMan[0x143d69918]
// flags are clear/set. Logging them at the finalize-advancer heartbeat (which fires for load2 in the
// stuck window) pins WHICH gate is the sole load2 divergence vs load1, without Ghidra and without
// forcing state. cb1/cb2/bca/b5e are byte flags; the global at rva 0x3d68078 must be non-null.
const GAME_MAN_SUBMIT_GATE_CB1_OFFSET: usize = 0xcb1;
const GAME_MAN_SUBMIT_GATE_CB2_OFFSET: usize = 0xcb2;
const GAME_MAN_SUBMIT_GATE_BCA_OFFSET: usize = 0xbca;
const GAME_MAN_SUBMIT_GATE_B5E_OFFSET: usize = 0xb5e;
const SUBMIT_GLOBAL_PTR_3D68078_RVA: usize = 0x3d68078;
const GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET: usize = 0x08;

const HOOK_ORIGINAL_UNSET: usize = 0;

type TraceHookFn = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;

/// C-ABI shape of the product DLL's `er_effects_union_register` export (crates/er-effects-rs/src/mh.rs).
/// (target_addr, handler, *mut orig_slot) -> 0 ok / -1 null-slot / positive MH_STATUS on failure.
/// `TraceHookFn` is exactly the product's `UnionFn`, so our detours plug into its union unchanged.
type UnionRegisterFn = unsafe extern "system" fn(usize, TraceHookFn, *mut usize) -> i32;

/// The product DLL's module base name as me3 loads it (matched by base name, not full path).
const PRODUCT_DLL_NAME: &[u8] = b"er_effects_rs.dll\0";
const UNION_REGISTER_EXPORT: &[u8] = b"er_effects_union_register\0";
/// Bounded wait for the product DLL to map + export the registrar (both natives load together under
/// me3; this only covers install-thread ordering). ~3s at 50ms cadence.
const UNION_RESOLVE_TRIES: u32 = 60;
const UNION_RESOLVE_SLEEP_MS: u32 = 50;

/// Addresses the PRODUCT DLL owns with a BARE `MhHook` (not its union) in the sq-repro reload mode
/// this trace runs alongside: 0x67b200 = SYSTEM_QUIT_REQUEST_LOAD_SLOT, 0x67b290 =
/// SYSTEM_QUIT_INWORLD_LOAD (the reload's picked-slot deserialize proof). Routing OUR observer through
/// the product union would create the dispatcher on that address first if our install thread wins the
/// race, making the product's later `MhHook::new` return ALREADY_CREATED and silently dropping the
/// product's CRITICAL reload hook. So in the unioned run we SKIP these two -- the product's own
/// menu-trace union hooks + its inworld-load debug line already log the same deserialize events.
/// (Standalone trace runs, with no product DLL present, still install them via our own MinHook.)
const UNION_SKIP_RVAS: &[usize] = &[0x67b200, 0x67b290];

static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);
/// Finalize-advancer instrumentation: total calls + last-seen field25_0x12a (so a run logs on every
/// sub-state change + a periodic heartbeat, instead of per-frame spam).
static FIN_ADVANCER_CALLS: AtomicU64 = AtomicU64::new(0);
static LAST_FIN12A: AtomicI32 = AtomicI32::new(-2);
/// rt5d DIAGNOSTIC DRIVE (bd DECISIVE-load2-divergence-is-rt5d-menudata5d): load1's finalize naturally
/// gets menuData+0x5d(rt5d)=1 and walks field25 0..9; load2's stays 0 and parks at field25=0 forever.
/// Once a SINGLE MoveMapStep has been stuck at field25=0 for RT5D_DRIVE_THRESHOLD consecutive advancer
/// calls (load1 flips at ~call#133, so this only ever fires for a genuinely-stuck load2), supply rt5d=1
/// once so the game's OWN finalize completes -- then observe complete(field25->9, movable) vs teardown.
const RT5D_DRIVE_THRESHOLD: u64 = 30;
static RT5D_DRIVE_MMS: AtomicUsize = AtomicUsize::new(0);
static RT5D_DRIVE_ZERO_STREAK: AtomicU64 = AtomicU64::new(0);
static RT5D_DRIVE_DONE_MMS: AtomicUsize = AtomicUsize::new(0);
/// The most recent MoveMapStep whose finalize was seen ADVANCING (field25>=5) -- i.e. a load that
/// completes on its own (load1, or a driven load2). Once set, any DIFFERENT mms stuck at field25=0 is
/// the divergent next load; drive it after only RT5D_DRIVE_THRESHOLD stuck calls (a short run may never
/// reach a large global count). Also lets the same logic catch load3 after load2 completes.
static COMPLETION_SEEN_MMS: AtomicUsize = AtomicUsize::new(0);
static ORIG_FINALIZE_ADVANCER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Child-done query FUN_140eb5550 (deobf 0x140eb5530 / rva 0xeb5530): STEP_MoveMap_Update calls it and,
/// if it returns nonzero (done), tears the MoveMapStep child down. HYPOTHESIS: it returns done
/// PREMATURELY for load2 (child field25 still 0) -> premature teardown -> advancer stops. Log-only: logs
/// the return on change + heartbeat to confirm load2 returns done while load1 returns not-done.
static ORIG_CHILD_DONE_QUERY: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static LAST_CHILD_DONE: AtomicI32 = AtomicI32::new(-2);
static CHILD_DONE_CALLS: AtomicU64 = AtomicU64::new(0);
/// Child-teardown instrumentation (FUN_140eb54e0): every teardown logs the child + its MoveMapStep
/// state/field25, so load2's MoveMapStep child teardown (if any) is visible.
static ORIG_CHILD_TEARDOWN: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static CHILD_TEARDOWN_CALLS: AtomicU64 = AtomicU64::new(0);

static ORIG_MENU_CONTINUE_WRAPPER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_MENU_NEW_OR_LOAD_WRAPPER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_MENU_OTHER_LOAD_WRAPPER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_NATIVE_SUBMIT: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_RESULT_EVENT_HANDLER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_RESULT_ACTION_BUILDER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_RESULT_EVENT_WRAPPER_BUILDER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_TASK_ENQUEUE: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_SET_SAVE_SLOT: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_SAVE_REQUEST_PROFILE: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_REQUEST_SAVE: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_CURRENT_SLOT_LOAD: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_CONTINUE_LOAD: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_COMBINED_LOAD: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_MAP_LOAD: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_SAVE_LOAD_STATE_INIT: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_B80_PREVIEW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_TITLE_CONFIRM: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_REQUEST_LOAD_SLOT: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_REQUEST_PROFILE_READ: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_B80_POLL: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_SLOT_DESER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_DISPATCHER2: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_DOSAVE_STUFF: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_MAP_REQUEST_DO: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_MAP_WORK: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_CAP_SETSTATE: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_CAP_LOAD_ACTIVATE: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_CAP_LOAD_ACTIVATE2: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_CAP_BUILDER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_CAP_SELECTOR_TICK: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_CAP_MENU_DESER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_CAP_DIALOG_FACTORY: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_MENU_WINDOW_JOB_CTOR: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_MENU_WINDOW_JOB_NATIVE_CTOR_B: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_MENU_WINDOW_JOB_IDLE_CTOR: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static ORIG_TITLE_NATIVE_READY: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);

struct HookSpec {
    name: &'static str,
    rva: usize,
    detour: TraceHookFn,
    original: &'static AtomicUsize,
}

// Raw MinHook FFI comes from the shared `er-hook` crate (single cc-compile). Its externs return the
// `MH_STATUS` enum; this crate historically compared/logged the status as `i32`, so each call site
// casts `as i32` -- MH_STATUS is `#[repr(C)]` with the same values, so the integers are unchanged.
use er_hook::{MH_CreateHook, MH_EnableHook, MH_Initialize};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
    fn ReadProcessMemory(
        process: isize,
        base: *const c_void,
        buffer: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
}

fn open_log_file() -> Option<Mutex<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_PATH)
        .ok()
        .map(Mutex::new)
}

fn reset_log_file() {
    let _ = File::create(LOG_PATH);
}

fn log_line(args: fmt::Arguments<'_>) {
    let Some(lock) = LOG_FILE.get_or_init(open_log_file) else {
        return;
    };
    let Ok(mut file) = lock.lock() else {
        return;
    };
    let tick = unsafe { GetTickCount64() };
    let seq = EVENT_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = writeln!(file, "[{seq:06} +{tick}ms] {args}");
}

fn game_base() -> Option<usize> {
    let base = unsafe { GetModuleHandleA(std::ptr::null()) } as usize;
    (base != 0).then_some(base)
}

unsafe fn read_usize(addr: usize) -> Option<usize> {
    let mut value = 0usize;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut usize as *mut c_void,
            std::mem::size_of::<usize>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<usize>()).then_some(value)
}

unsafe fn read_i32(addr: usize) -> Option<i32> {
    let mut value = 0i32;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut i32 as *mut c_void,
            std::mem::size_of::<i32>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<i32>()).then_some(value)
}

unsafe fn read_u8(addr: usize) -> Option<u8> {
    let mut value = 0u8;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut u8 as *mut c_void,
            1,
            &mut read,
        )
    };
    (ok != 0 && read == 1).then_some(value)
}

/// Dump a window of qwords from the MoveMapStep header (own process, guarded reads). Why: load2's
/// MoveMapStep Update (FUN_140aff640) stops being ticked by the FD4 scheduler after ~6 ticks while
/// load1 ticks it ~145x to completion (bd load2-real-blocker-movemapstep-child-advancer-tick-never-runs).
/// The advancer fires 1:1 with that Update, so dumping the mms header at each tick lets a run diff
/// load1 (keeps ticking) vs load2 (drops out) and reveal which header field (step-machine state /
/// active flag / scheduler link) changes when ticking stops. Offsets +0x00..+0x58, plus the child
/// ezstep pointer region around +0x108. Read-only.
fn mms_header_window(a: usize) -> String {
    let mut s = String::from("mmshdr[");
    let mut off = 0usize;
    while off <= 0x58 {
        match unsafe { read_usize(a + off) } {
            Some(v) => s.push_str(&format!("+{off:x}=0x{v:x} ")),
            None => s.push_str(&format!("+{off:x}=? ")),
        }
        off += 8;
    }
    // child ezstep base region (mms+0x108) + the finalize substate byte (+0x12a) neighbourhood
    for off in [0x100usize, 0x108, 0x110, 0x118, 0x128] {
        match unsafe { read_usize(a + off) } {
            Some(v) => s.push_str(&format!("+{off:x}=0x{v:x} ")),
            None => s.push_str(&format!("+{off:x}=? ")),
        }
    }
    s.push(']');
    s
}

/// Custom detour for the MoveMapStep finalize advancer (0xafa6d0). Logs field25_0x12a before/after the
/// native call plus menuData 0x5d(rt5d)/0x5e(cVar10 out) -- ONLY on a sub-state change or every 600th
/// call (heartbeat), so a FROZEN load2 (field25 stuck at 0) is visible without per-frame flooding while
/// a healthy walk 0->9 logs every transition. rcx (`a`) = MoveMapStep.
unsafe extern "system" fn hook_finalize_advancer(a: usize, b: usize, c: usize, d: usize) -> usize {
    let calls = FIN_ADVANCER_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let fin_before = unsafe { read_u8(a + MOVEMAPSTEP_FINALIZE_12A_OFFSET) }.map_or(-1, i32::from);
    let menu = game_base().and_then(|base| unsafe { read_usize(base + CS_MENU_MAN_GLOBAL_RVA) });
    let menu_data = menu.and_then(|m| unsafe { read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) });
    // NOTE: the rt5d/save-flag DRIVE was REMOVED (bd CORRECTION-rt5d-drive-tears-down-load2). Driving
    // menuData+0x5d=1 (+ clearing saveRequested/0xb73) DID complete load2's finalize 0..9, but that
    // TORE THE PLAYER DOWN (post-completion: present=False, havok=None, mms=-1 at ~60fps = a player-less
    // world) -- load2's player is not movable at fin=0 when the finalize runs, unlike load1. So the
    // finalize-drive is a proven DEAD END; this hook is log-only again so traces show the natural load2.
    // The old drive statics/consts are retained for reference but intentionally unused.
    let _ = (
        &RT5D_DRIVE_MMS,
        &RT5D_DRIVE_ZERO_STREAK,
        &RT5D_DRIVE_DONE_MMS,
        &COMPLETION_SEEN_MMS,
    );
    let ret = unsafe { call_original(&ORIG_FINALIZE_ADVANCER, a, b, c, d) };
    let fin_after = unsafe { read_u8(a + MOVEMAPSTEP_FINALIZE_12A_OFFSET) }.map_or(-1, i32::from);
    let m5d = menu_data
        .and_then(|md| unsafe { read_u8(md + MENU_DATA_RT5D_OFFSET) })
        .map_or(-1, i32::from);
    let m5e = menu_data
        .and_then(|md| unsafe { read_u8(md + MENU_DATA_ENDING_5E_OFFSET) })
        .map_or(-1, i32::from);
    let last = LAST_FIN12A.swap(fin_after, Ordering::SeqCst);
    // Log EVERY advancer tick with the mms header window. load2's Update ticks only ~6x before the FD4
    // scheduler drops it (load1 ~145x), so the per-tick header lets a run diff which mms field flips
    // when load2 stops ticking. Volume is bounded (a few hundred lines/run) -- acceptable for a
    // diagnostic (bd load2-real-blocker-movemapstep-child-advancer-tick-never-runs).
    let _ = (fin_before, last);
    log_line(format_args!(
        "finalize_advancer_afa6d0 call#{calls} mms=0x{a:x} field25_12a {fin_before}->{fin_after} menuData_5d={m5d} 5e={m5e} {} {}",
        mms_header_window(a),
        snapshot()
    ));
    ret
}

static LOADLIST_INIT_CALLS: AtomicU64 = AtomicU64::new(0);
static ORIG_LOADLIST_INIT: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// worldloadlistlistVirtualPath = InGameStep+0x108, a DlFixedString<wchar_t,128> (inline): +0x00 union
/// (pointer when capacity>7, else in_place), +0x08 size(wchars), +0x10 capacity.
const INGAMESTEP_WORLDLOADLIST_VPATH_108_OFFSET: usize = 0x108;

/// Log-only detour for STEP_MoveMap_LoadlistInit (rva 0xaec480 / dump 0x140aec570). Its gate is
/// worldloadlistlistVirtualPath.size != 0 -- when 0 it SKIPS building the loadlist -> no block-res ->
/// WorldResWait hangs -> load2 finalize stuck. Logs the DlFixedString (size/cap/ptr + ASCII path
/// preview) BEFORE the original so a load1-vs-load2 diff shows load1's map:\WorldMsbList\... path and
/// load2's EMPTY path -- the root of the reload stall (bd fix-point-confirmed-stepmovemap-loadlistinit).
unsafe extern "system" fn hook_loadlist_init(a: usize, b: usize, c: usize, d: usize) -> usize {
    let n = LOADLIST_INIT_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let dfs = a.wrapping_add(INGAMESTEP_WORLDLOADLIST_VPATH_108_OFFSET);
    let size = unsafe { read_usize(dfs + 0x08) }.unwrap_or(usize::MAX);
    let cap = unsafe { read_usize(dfs + 0x10) }.unwrap_or(usize::MAX);
    let str_base = if cap != usize::MAX && cap > 7 {
        unsafe { read_usize(dfs) }.unwrap_or(0)
    } else {
        dfs
    };
    let mut preview = String::new();
    if str_base != 0 && size != usize::MAX && size <= 256 {
        for i in 0..size.min(120) {
            // ASCII path chars sit in the low byte of each UTF-16LE unit.
            match unsafe { read_u8(str_base + i * 2) } {
                Some(w) if (0x20..0x7f).contains(&w) => preview.push(w as char),
                _ => preview.push('.'),
            }
        }
    }
    log_line(format_args!(
        "loadlist_init_aec480 call#{n} InGameStep=0x{a:x} worldloadlist_size={size} cap={cap} strptr=0x{str_base:x} path='{preview}' {}",
        snapshot()
    ));
    unsafe { call_original(&ORIG_LOADLIST_INIT, a, b, c, d) }
}

/// Log-only detour for the child-done query FUN_140eb5550 (rva 0xeb5530). Logs its return (done flag)
/// on change + a 600-call heartbeat. If load2 returns done=1 while load1 returns done=0 during the
/// mms18 freeze, the premature-teardown chain is confirmed.
unsafe extern "system" fn hook_child_done_query(a: usize, b: usize, c: usize, d: usize) -> usize {
    let calls = CHILD_DONE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let ret = unsafe { call_original(&ORIG_CHILD_DONE_QUERY, a, b, c, d) };
    let done = (ret & 0xff) as i32;
    let last = LAST_CHILD_DONE.swap(done, Ordering::SeqCst);
    if last != done || calls % 600 == 0 {
        log_line(format_args!(
            "child_done_query_eb5530 call#{calls} rcx=0x{a:x} returned done={done} {}",
            snapshot()
        ));
    }
    ret
}

/// Log-only detour for the child teardown FUN_140eb54e0 (rva 0xeb54c0). Logs every teardown with the
/// child_base-0x108 MoveMapStep state/field25 so load2's MoveMapStep child teardown (state==18) is
/// visible -- or its absence proves the child is never re-scheduled rather than torn down. mms= ptr
/// distinguishes load1 vs load2.
unsafe extern "system" fn hook_child_teardown(a: usize, b: usize, c: usize, d: usize) -> usize {
    let n = CHILD_TEARDOWN_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let mms = a.wrapping_sub(MOVEMAPSTEP_CHILD_EZSTEP_OFFSET);
    let st = unsafe { read_i32(mms + MOVEMAPSTEP_STATE_48_OFFSET) }.unwrap_or(-999);
    let fin = unsafe { read_u8(mms + MOVEMAPSTEP_FINALIZE_12A_OFFSET) }.map_or(-1, i32::from);
    log_line(format_args!(
        "child_teardown_eb54c0 call#{n} child_base=0x{a:x} mms=0x{mms:x} state={st} field25={fin} {}",
        snapshot()
    ));
    unsafe { call_original(&ORIG_CHILD_TEARDOWN, a, b, c, d) }
}

fn snapshot() -> String {
    let Some(base) = game_base() else {
        return "base=<unresolved>".to_owned();
    };
    let gm = unsafe { read_usize(base + GAME_MAN_SINGLETON_RVA) }.unwrap_or(0);
    let gdm = unsafe { read_usize(base + GAME_DATA_MAN_GLOBAL_RVA) }.unwrap_or(0);
    let mounted = unsafe { read_usize(base + MOUNTED_ARCHIVE_REGISTRY_RVA) }.unwrap_or(0);

    let b78 = unsafe { read_i32(gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) };
    let b80 = unsafe { read_i32(gm + GAME_MAN_LOAD_PHASE_B80_OFFSET) };
    let ac0 = unsafe { read_i32(gm + GAME_MAN_SAVE_SLOT_AC0_OFFSET) };
    let c30 = unsafe { read_i32(gm + GAME_MAN_CURRENT_MAP_C30_OFFSET) };
    let df0 = unsafe { read_usize(gm + GAME_MAN_RESIDENT_DEVICE_DF0_OFFSET) }.unwrap_or(0);
    let pgd = unsafe { read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(0);

    // Load-submit gate fields (see the *_SUBMIT_GATE_* consts): diff load1 vs load2 to find the gate
    // that keeps load2's combined_load submit bailing so the world load never completes.
    let g_cb1 = unsafe { read_u8(gm + GAME_MAN_SUBMIT_GATE_CB1_OFFSET) };
    let g_cb2 = unsafe { read_u8(gm + GAME_MAN_SUBMIT_GATE_CB2_OFFSET) };
    let g_bca = unsafe { read_u8(gm + GAME_MAN_SUBMIT_GATE_BCA_OFFSET) };
    let g_b5e = unsafe { read_u8(gm + GAME_MAN_SUBMIT_GATE_B5E_OFFSET) };
    let g_glob = unsafe { read_usize(base + SUBMIT_GLOBAL_PTR_3D68078_RVA) }.unwrap_or(0);

    format!(
        "base=0x{base:x} gm=0x{gm:x} b78={} b80={} ac0={} c30={} df0=0x{df0:x} gdm=0x{gdm:x} pgd=0x{pgd:x} mounted_registry=0x{mounted:x} submit[cb1={} cb2={} bca={} b5e={} glob=0x{g_glob:x}]",
        fmt_i32(b78),
        fmt_i32(b80),
        fmt_i32(ac0),
        fmt_c30(c30),
        fmt_u8(g_cb1),
        fmt_u8(g_cb2),
        fmt_u8(g_bca),
        fmt_u8(g_b5e),
    )
}

fn fmt_i32(value: Option<i32>) -> String {
    value.map_or_else(|| "<unreadable>".to_owned(), |value| value.to_string())
}

fn fmt_c30(value: Option<i32>) -> String {
    value.map_or_else(|| "<unreadable>".to_owned(), |value| format!("0x{value:x}"))
}

fn fmt_u8(value: Option<u8>) -> String {
    value.map_or_else(|| "<unreadable>".to_owned(), |value| value.to_string())
}

unsafe fn call_original(
    original: &'static AtomicUsize,
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: TraceHookFn = unsafe { std::mem::transmute(original) };
    unsafe { original(a, b, c, d) }
}

unsafe fn trace_hook(
    name: &'static str,
    original: &'static AtomicUsize,
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    log_line(format_args!(
        "{name} ENTER rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x} {}",
        snapshot()
    ));
    let ret = unsafe { call_original(original, a, b, c, d) };
    log_line(format_args!(
        "{name} LEAVE ret=0x{ret:x} rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x} {}",
        snapshot()
    ));
    ret
}

macro_rules! define_trace_hook {
    ($fn_name:ident, $original:ident, $label:literal) => {
        unsafe extern "system" fn $fn_name(a: usize, b: usize, c: usize, d: usize) -> usize {
            unsafe { trace_hook($label, &$original, a, b, c, d) }
        }
    };
}

define_trace_hook!(
    hook_menu_continue_wrapper,
    ORIG_MENU_CONTINUE_WRAPPER,
    "menu_continue_wrapper_82bac0"
);
define_trace_hook!(
    hook_menu_new_or_load_wrapper,
    ORIG_MENU_NEW_OR_LOAD_WRAPPER,
    "menu_new_or_load_wrapper_82ba80"
);
define_trace_hook!(
    hook_menu_other_load_wrapper,
    ORIG_MENU_OTHER_LOAD_WRAPPER,
    "menu_other_load_wrapper_82bb00"
);
define_trace_hook!(
    hook_native_submit,
    ORIG_NATIVE_SUBMIT,
    "native_submit_7ac890"
);
define_trace_hook!(
    hook_result_event_handler,
    ORIG_RESULT_EVENT_HANDLER,
    "result_event_handler_746e80"
);
define_trace_hook!(
    hook_result_action_builder,
    ORIG_RESULT_ACTION_BUILDER,
    "result_action_builder_746a00"
);
define_trace_hook!(
    hook_result_event_wrapper_builder,
    ORIG_RESULT_EVENT_WRAPPER_BUILDER,
    "result_event_wrapper_builder_744a60"
);
define_trace_hook!(hook_task_enqueue, ORIG_TASK_ENQUEUE, "task_enqueue_7a7b60");
define_trace_hook!(
    hook_set_save_slot,
    ORIG_SET_SAVE_SLOT,
    "set_save_slot_67a810"
);
define_trace_hook!(
    hook_save_request_profile,
    ORIG_SAVE_REQUEST_PROFILE,
    "save_request_profile_67a420"
);
define_trace_hook!(hook_request_save, ORIG_REQUEST_SAVE, "request_save_67a520");
define_trace_hook!(
    hook_current_slot_load,
    ORIG_CURRENT_SLOT_LOAD,
    "current_slot_load_67b570"
);
define_trace_hook!(
    hook_continue_load,
    ORIG_CONTINUE_LOAD,
    "continue_load_67b750"
);
define_trace_hook!(
    hook_combined_load,
    ORIG_COMBINED_LOAD,
    "combined_load_67b940"
);
define_trace_hook!(hook_map_load, ORIG_MAP_LOAD, "map_load_67bc10");
define_trace_hook!(
    hook_save_load_state_init,
    ORIG_SAVE_LOAD_STATE_INIT,
    "save_load_state_init_67b030"
);
define_trace_hook!(hook_b80_preview, ORIG_B80_PREVIEW, "b80_preview_67b4e0");
define_trace_hook!(
    hook_title_confirm,
    ORIG_TITLE_CONFIRM,
    "title_confirm_b0e180"
);
define_trace_hook!(
    hook_request_load_slot,
    ORIG_REQUEST_LOAD_SLOT,
    "request_load_slot_67b200"
);
define_trace_hook!(
    hook_request_profile_read,
    ORIG_REQUEST_PROFILE_READ,
    "request_profile_read_67b1a0"
);
define_trace_hook!(hook_b80_poll, ORIG_B80_POLL, "b80_poll_679180");
define_trace_hook!(hook_slot_deser, ORIG_SLOT_DESER, "slot_deser_67b290");
define_trace_hook!(
    hook_dispatcher2,
    ORIG_DISPATCHER2,
    "movemap_dispatcher2_afb880"
);
define_trace_hook!(
    hook_dosave_stuff,
    ORIG_DOSAVE_STUFF,
    "movemap_do_save_stuff_afbad0"
);
define_trace_hook!(
    hook_map_request_do,
    ORIG_MAP_REQUEST_DO,
    "map_request_do_836f30"
);
define_trace_hook!(hook_map_work, ORIG_MAP_WORK, "map_work_82faf0");
define_trace_hook!(hook_cap_setstate, ORIG_CAP_SETSTATE, "cap_setstate_b0d960");
define_trace_hook!(
    hook_cap_load_activate,
    ORIG_CAP_LOAD_ACTIVATE,
    "cap_load_activate_9a4670"
);
define_trace_hook!(
    hook_cap_load_activate2,
    ORIG_CAP_LOAD_ACTIVATE2,
    "cap_load_activate2_9ac760"
);
define_trace_hook!(hook_cap_builder, ORIG_CAP_BUILDER, "cap_builder_826510");
define_trace_hook!(
    hook_cap_selector_tick,
    ORIG_CAP_SELECTOR_TICK,
    "cap_selector_tick_826d50"
);
define_trace_hook!(
    hook_cap_menu_deser,
    ORIG_CAP_MENU_DESER,
    "cap_menu_deser_82c240"
);
define_trace_hook!(
    hook_cap_dialog_factory,
    ORIG_CAP_DIALOG_FACTORY,
    "cap_dialog_factory_81ead0"
);
define_trace_hook!(
    hook_menu_window_job_ctor,
    ORIG_MENU_WINDOW_JOB_CTOR,
    "menu_window_job_ctor_7ac8c0"
);
define_trace_hook!(
    hook_menu_window_job_native_ctor_b,
    ORIG_MENU_WINDOW_JOB_NATIVE_CTOR_B,
    "menu_window_job_native_ctor_b_7acb00"
);
define_trace_hook!(
    hook_menu_window_job_idle_ctor,
    ORIG_MENU_WINDOW_JOB_IDLE_CTOR,
    "menu_window_job_idle_ctor_7acf80"
);
define_trace_hook!(
    hook_title_native_ready,
    ORIG_TITLE_NATIVE_READY,
    "title_native_ready_733150"
);

static HOOKS: &[HookSpec] = &[
    HookSpec {
        name: "menu_continue_wrapper_82bac0",
        rva: 0x82bac0,
        detour: hook_menu_continue_wrapper,
        original: &ORIG_MENU_CONTINUE_WRAPPER,
    },
    HookSpec {
        name: "menu_new_or_load_wrapper_82ba80",
        rva: 0x82ba80,
        detour: hook_menu_new_or_load_wrapper,
        original: &ORIG_MENU_NEW_OR_LOAD_WRAPPER,
    },
    HookSpec {
        name: "menu_other_load_wrapper_82bb00",
        rva: 0x82bb00,
        detour: hook_menu_other_load_wrapper,
        original: &ORIG_MENU_OTHER_LOAD_WRAPPER,
    },
    HookSpec {
        name: "native_submit_7ac890",
        rva: 0x7ac890,
        detour: hook_native_submit,
        original: &ORIG_NATIVE_SUBMIT,
    },
    HookSpec {
        name: "result_event_handler_746e80",
        rva: 0x746e80,
        detour: hook_result_event_handler,
        original: &ORIG_RESULT_EVENT_HANDLER,
    },
    HookSpec {
        name: "result_action_builder_746a00",
        rva: 0x746a00,
        detour: hook_result_action_builder,
        original: &ORIG_RESULT_ACTION_BUILDER,
    },
    HookSpec {
        name: "result_event_wrapper_builder_744a60",
        rva: 0x744a60,
        detour: hook_result_event_wrapper_builder,
        original: &ORIG_RESULT_EVENT_WRAPPER_BUILDER,
    },
    HookSpec {
        name: "task_enqueue_7a7b60",
        rva: 0x7a7b60,
        detour: hook_task_enqueue,
        original: &ORIG_TASK_ENQUEUE,
    },
    HookSpec {
        name: "set_save_slot_67a810",
        rva: 0x67a810,
        detour: hook_set_save_slot,
        original: &ORIG_SET_SAVE_SLOT,
    },
    HookSpec {
        name: "save_request_profile_67a420",
        rva: 0x67a420,
        detour: hook_save_request_profile,
        original: &ORIG_SAVE_REQUEST_PROFILE,
    },
    HookSpec {
        name: "request_save_67a520",
        rva: 0x67a520,
        detour: hook_request_save,
        original: &ORIG_REQUEST_SAVE,
    },
    HookSpec {
        name: "current_slot_load_67b570",
        rva: 0x67b570,
        detour: hook_current_slot_load,
        original: &ORIG_CURRENT_SLOT_LOAD,
    },
    HookSpec {
        name: "continue_load_67b750",
        rva: 0x67b750,
        detour: hook_continue_load,
        original: &ORIG_CONTINUE_LOAD,
    },
    HookSpec {
        name: "combined_load_67b940",
        rva: 0x67b940,
        detour: hook_combined_load,
        original: &ORIG_COMBINED_LOAD,
    },
    HookSpec {
        name: "map_load_67bc10",
        rva: 0x67bc10,
        detour: hook_map_load,
        original: &ORIG_MAP_LOAD,
    },
    HookSpec {
        name: "save_load_state_init_67b030",
        rva: 0x67b030,
        detour: hook_save_load_state_init,
        original: &ORIG_SAVE_LOAD_STATE_INIT,
    },
    HookSpec {
        name: "b80_preview_67b4e0",
        rva: 0x67b4e0,
        detour: hook_b80_preview,
        original: &ORIG_B80_PREVIEW,
    },
    HookSpec {
        name: "title_confirm_b0e180",
        rva: 0xb0e180,
        detour: hook_title_confirm,
        original: &ORIG_TITLE_CONFIRM,
    },
    HookSpec {
        name: "request_load_slot_67b200",
        rva: 0x67b200,
        detour: hook_request_load_slot,
        original: &ORIG_REQUEST_LOAD_SLOT,
    },
    HookSpec {
        name: "request_profile_read_67b1a0",
        rva: 0x67b1a0,
        detour: hook_request_profile_read,
        original: &ORIG_REQUEST_PROFILE_READ,
    },
    HookSpec {
        name: "b80_poll_679180",
        rva: 0x679180,
        detour: hook_b80_poll,
        original: &ORIG_B80_POLL,
    },
    HookSpec {
        name: "slot_deser_67b290",
        rva: 0x67b290,
        detour: hook_slot_deser,
        original: &ORIG_SLOT_DESER,
    },
    HookSpec {
        name: "movemap_dispatcher2_afb880",
        rva: 0xafb880,
        detour: hook_dispatcher2,
        original: &ORIG_DISPATCHER2,
    },
    HookSpec {
        name: "movemap_do_save_stuff_afbad0",
        rva: 0xafbad0,
        detour: hook_dosave_stuff,
        original: &ORIG_DOSAVE_STUFF,
    },
    HookSpec {
        name: "map_request_do_836f30",
        rva: 0x836f30,
        detour: hook_map_request_do,
        original: &ORIG_MAP_REQUEST_DO,
    },
    HookSpec {
        name: "map_work_82faf0",
        rva: 0x82faf0,
        detour: hook_map_work,
        original: &ORIG_MAP_WORK,
    },
    HookSpec {
        name: "cap_setstate_b0d960",
        rva: 0xb0d960,
        detour: hook_cap_setstate,
        original: &ORIG_CAP_SETSTATE,
    },
    HookSpec {
        name: "cap_load_activate_9a4670",
        rva: 0x9a4670,
        detour: hook_cap_load_activate,
        original: &ORIG_CAP_LOAD_ACTIVATE,
    },
    HookSpec {
        name: "cap_load_activate2_9ac760",
        rva: 0x9ac760,
        detour: hook_cap_load_activate2,
        original: &ORIG_CAP_LOAD_ACTIVATE2,
    },
    HookSpec {
        name: "cap_builder_826510",
        rva: 0x826510,
        detour: hook_cap_builder,
        original: &ORIG_CAP_BUILDER,
    },
    HookSpec {
        name: "cap_selector_tick_826d50",
        rva: 0x826d50,
        detour: hook_cap_selector_tick,
        original: &ORIG_CAP_SELECTOR_TICK,
    },
    HookSpec {
        name: "cap_menu_deser_82c240",
        rva: 0x82c240,
        detour: hook_cap_menu_deser,
        original: &ORIG_CAP_MENU_DESER,
    },
    HookSpec {
        name: "cap_dialog_factory_81ead0",
        rva: 0x81ead0,
        detour: hook_cap_dialog_factory,
        original: &ORIG_CAP_DIALOG_FACTORY,
    },
    HookSpec {
        name: "menu_window_job_ctor_7ac8c0",
        rva: 0x7ac8c0,
        detour: hook_menu_window_job_ctor,
        original: &ORIG_MENU_WINDOW_JOB_CTOR,
    },
    HookSpec {
        name: "menu_window_job_native_ctor_b_7acb00",
        rva: 0x7acb00,
        detour: hook_menu_window_job_native_ctor_b,
        original: &ORIG_MENU_WINDOW_JOB_NATIVE_CTOR_B,
    },
    HookSpec {
        name: "menu_window_job_idle_ctor_7acf80",
        rva: 0x7acf80,
        detour: hook_menu_window_job_idle_ctor,
        original: &ORIG_MENU_WINDOW_JOB_IDLE_CTOR,
    },
    HookSpec {
        name: "title_native_ready_733150",
        rva: 0x733150,
        detour: hook_title_native_ready,
        original: &ORIG_TITLE_NATIVE_READY,
    },
    HookSpec {
        name: "finalize_advancer_afa6d0",
        rva: 0xafa6d0,
        detour: hook_finalize_advancer,
        original: &ORIG_FINALIZE_ADVANCER,
    },
    HookSpec {
        name: "loadlist_init_aec480",
        rva: 0xaec480,
        detour: hook_loadlist_init,
        original: &ORIG_LOADLIST_INIT,
    },
    HookSpec {
        name: "child_teardown_eb54c0",
        rva: CHILD_TEARDOWN_RVA,
        detour: hook_child_teardown,
        original: &ORIG_CHILD_TEARDOWN,
    },
    // child_done_query_eb5530 removed: the PRODUCT DLL now owns 0xeb5530 with its override hook
    // (child_done_query_override_detour); a second trace hook here would chain and muddy the override.
];

/// Resolve the product DLL's `er_effects_union_register` export, polling briefly since both natives
/// load together under me3 and thread ordering is not guaranteed. `None` => the product DLL is not in
/// this process (a standalone trace run) or has not exported yet; caller falls back to its own MinHook.
fn resolve_union_register() -> Option<UnionRegisterFn> {
    for _ in 0..UNION_RESOLVE_TRIES {
        let hmod = unsafe { GetModuleHandleA(PRODUCT_DLL_NAME.as_ptr()) };
        if !hmod.is_null() {
            let proc = unsafe { GetProcAddress(hmod, UNION_REGISTER_EXPORT.as_ptr()) };
            if !proc.is_null() {
                // SAFETY: the export's C-ABI shape is fixed by the product DLL; both DLLs live for the
                // process lifetime so the pointer stays valid.
                return Some(unsafe { std::mem::transmute::<*mut c_void, UnionRegisterFn>(proc) });
            }
        }
        unsafe { Sleep(UNION_RESOLVE_SLEEP_MS) };
    }
    None
}

fn install_hooks() {
    reset_log_file();
    log_line(format_args!(
        "er-reload-trace-dll attach: trampoline/log-only build; no input, save redirect, autoload, game task, or game-state writes"
    ));
    let Some(base) = game_base() else {
        log_line(format_args!("install abort: game module base unresolved"));
        return;
    };
    // CROSS-DLL HOOK UNION (2026-07-18, user-directed): when the product DLL (er_effects_rs.dll) is
    // co-loaded, route EVERY trace hook through its `er_effects_union_register` export so a SINGLE
    // MinHook instance owns every address and our observers CHAIN with the product's own detours on
    // shared addresses (0xb0e180 continue-confirm, 0xb0d960 title-SetState, etc.). Two independent
    // MinHook instances patching the same address corrupt each other's trampolines -- the exact race
    // the product's internal union fixes, now spanning DLLs. Standalone trace runs fall back to our
    // own MinHook instance (no product DLL => no shared addresses => no corruption).
    if let Some(reg) = resolve_union_register() {
        log_line(format_args!(
            "cross-dll union: resolved product er_effects_union_register -> routing all {} hooks through the product DLL's single MinHook instance",
            HOOKS.len()
        ));
        for spec in HOOKS {
            install_one_union(reg, base, spec);
        }
        log_line(format_args!("install complete (unioned) {}", snapshot()));
        return;
    }
    log_line(format_args!(
        "cross-dll union: product DLL export not present (standalone trace run) -> own MinHook instance"
    ));
    let init_status = unsafe { MH_Initialize() } as i32;
    if init_status != MH_OK && init_status != MH_ERROR_ALREADY_INITIALIZED {
        log_line(format_args!(
            "MinHook initialize failed status={init_status}"
        ));
        return;
    }
    for spec in HOOKS {
        install_one(base, spec);
    }
    log_line(format_args!("install complete {}", snapshot()));
}

/// Register one trace observer through the product DLL's union (single shared MinHook instance).
/// The union stores the trampoline (or next chained handler) into `spec.original`, which our
/// `call_original` already reads -- so chaining is transparent to the detour bodies.
fn install_one_union(reg: UnionRegisterFn, base: usize, spec: &HookSpec) {
    if UNION_SKIP_RVAS.contains(&spec.rva) {
        log_line(format_args!(
            "hook {} rva=0x{:x} SKIPPED in unioned run (product owns a bare MinHook here; unioning would preempt its critical reload hook)",
            spec.name, spec.rva
        ));
        return;
    }
    let target = base + spec.rva;
    let orig_ptr = spec.original.as_ptr();
    let rc = unsafe { reg(target, spec.detour, orig_ptr) };
    if rc == 0 {
        log_line(format_args!(
            "hook {} rva=0x{:x} target=0x{target:x} union-registered (chained via product DLL)",
            spec.name, spec.rva
        ));
    } else {
        log_line(format_args!(
            "hook {} rva=0x{:x} target=0x{target:x} union register FAILED rc={rc}",
            spec.name, spec.rva
        ));
    }
}

fn install_one(base: usize, spec: &HookSpec) {
    let target = base + spec.rva;
    let mut trampoline: *mut c_void = null_mut();
    let create_status = unsafe {
        MH_CreateHook(
            target as *mut c_void,
            spec.detour as *mut c_void,
            &mut trampoline,
        )
    } as i32;
    if create_status != MH_OK {
        log_line(format_args!(
            "hook {} rva=0x{:x} target=0x{target:x} create failed status={create_status}",
            spec.name, spec.rva
        ));
        return;
    }
    spec.original.store(trampoline as usize, Ordering::SeqCst);
    let enable_status = unsafe { MH_EnableHook(target as *mut c_void) } as i32;
    if enable_status != MH_OK && enable_status != MH_ERROR_ENABLED {
        log_line(format_args!(
            "hook {} rva=0x{:x} target=0x{target:x} enable failed status={enable_status}",
            spec.name, spec.rva
        ));
        return;
    }
    log_line(format_args!(
        "hook {} rva=0x{:x} target=0x{target:x} trampoline=0x{:x} installed",
        spec.name, spec.rva, trampoline as usize
    ));
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    _module: *mut c_void,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        let _ = std::thread::Builder::new()
            .name("er-reload-trace-install".to_owned())
            .spawn(install_hooks);
    }
    DLL_MAIN_SUCCESS
}
