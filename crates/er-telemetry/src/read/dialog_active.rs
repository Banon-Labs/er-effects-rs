//! ORACLE-3 / SEMAPHORE-D: currently-displayed `CS::MessageBoxDialog` /
//! `CS::SaveRetryDialog` modal -- a PASSIVE read that catches a box built at ANY
//! time (including the boot "session not ended / loss of progress" warning that
//! appears before the product hook installs and in telemetry-only mode).
//!
//! WHY THIS EXISTS: historical build-counter telemetry could not prove a visible dialog was
//! currently blocking the product path. This reader inspects live dialog objects directly,
//! installed late (from the recurring game task, gated by online-disable/product
//! autoload). Any dialog built before that install -- or in a run where the product
//! hook never arms (telemetry-only, or the standalone er-telemetry-dll which has no
//! product hooks at all) -- is never counted, so the counter reports 0 even while a
//! box is on screen (the observed FALSE NEGATIVE, bd
//! msgbox-oracle-false-negative-boot-session-dialog-masked-by-cover-user-image-2026-07-24).
//! A currently-displayed dialog is instead detected here by reading LIVE game RAM:
//! a fault-safe, vtable-gated address-space scan (the exact pattern
//! `title_binding::scan_for_owner` uses for the title owner) finds an object whose
//! first qword is the base MessageBoxDialog vtable OR the SaveRetryDialog subclass
//! vtable, then confirms it is genuinely on screen (not tearing down) with the same
//! `closing_latch`/`state` fields the product's own `blocking_modal_present` uses.
//!
//! All reads are passive `safe_read_*` / `ReadProcessMemory` / `VirtualQuery`; NO
//! hooks, NO vtable-fn calls, NO D3D12 -- RenderDoc-safe. Independent marker gate
//! (`er-oracle-dialog-active.on`, DEFAULT OFF); no cross-dependency on the other
//! read/ oracles.

use core::ffi::c_void;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use er_game_base::mem::{safe_read_i32, safe_read_u8, safe_read_usize, vtable_in_game_image};

// --- Dialog vtable RVAs off the game image base (0x140000000). DATA RVAs are
// version-stable between the dump and the live/deobf image (the piecewise .text
// shift does not apply to .rdata vtables), so these live RVAs are used directly. ---
/// `CS::MessageBoxDialog` base vftable RVA. Source: product
/// constants/autoload_state.rs `MsgBoxRva::DialogVtable`; RE-CONFIRMED in the
/// 1.16.2 dump -- the builder `FUN_1409275b0` writes `*obj =
/// CS::MessageBoxDialog::vftable` at this RVA (dump 0x142b03550, xref from
/// 0x140927601/0x140927608).
use er_game_base::rva::MSGBOX_DIALOG_VTABLE_RVA;
/// `CS::SaveRetryDialog` vftable RVA -- a MessageBoxDialog SUBCLASS whose wrapper
/// overrides the base vtable AFTER the builder runs; it is the offline title flow's
/// "save/load failed -- Retry?" boot prompt. Source: product
/// constants/autoload_state.rs `SAVE_RETRY_DIALOG_VTABLE_RVA`; RE-CONFIRMED in the
/// 1.16.2 dump -- referenced by the SaveRetry wrapper `FUN_1407af8b0` (dump
/// 0x142aaabf8), which is also a caller of the base builder `FUN_1409275b0`.
const SAVE_RETRY_DIALOG_VTABLE_RVA: usize = 0x2aaabf8;

// --- MessageBoxDialog instance fields (version-stable struct offsets; dump ==
// live). ---
/// `closing_latch` (u8, `dialog+0x3b0`): set =1 by EmitResult once the dialog
/// begins teardown. 0 == still displayed. Source: product constants/system_quit.rs
/// `MSGBOX_CLOSING_LATCH_3B0_OFFSET` (offset_of MsgBoxDialogLayout.closing_latch);
/// this is exactly the field write_game_module_oracles' `blocking_modal_present`
/// reads.
const MSGBOX_CLOSING_LATCH_3B0_OFFSET: usize = 0x3b0;
/// `button_count` (i32, `dialog+0x25e8`). CORRECTED 2026-07-28: this is NOT a
/// "decided" state. The ctor (`FUN_1409275b0`) writes it as
/// `(*(i64*)(cfg+0x38) - *(i64*)(cfg+0x30)) / 0x210`, the length of the button
/// descriptor vector, so a Yes/No confirm reads 2 here from the frame it is built.
/// The old `state >= 2 == decided` test therefore REJECTED every two-button confirm
/// as "not active" (false negative). Kept as a shape check only: a real dialog has
/// at least one button. Source: product constants/autoload_state.rs
/// `MSGBOX_BUTTON_COUNT_25E8_OFFSET`.
const MSGBOX_BUTTON_COUNT_25E8_OFFSET: usize = 0x25e8;
/// Plausible button-count range for a live dialog (1 = OK notice, 2 = Yes/No, and
/// the widest native command dialogs stay well under this ceiling). Outside it the
/// candidate is garbage that merely happens to hold the vtable value.
const MSGBOX_BUTTON_COUNT_MIN: i32 = 1;
const MSGBOX_BUTTON_COUNT_MAX: i32 = 16;
/// `job_result.state` (i32, `dialog+0x1e8`): the dialog's own `MenuJobResult`. <= 1
/// (`Continue`) == no answer yet; 2 (`Success`) / 3 (`Failed`) == the user's button
/// already resolved it. Source: product constants/autoload_state.rs
/// `MSGBOX_JOB_RESULT_STATE_1E8_OFFSET`.
const MSGBOX_JOB_RESULT_STATE_1E8_OFFSET: usize = 0x1e8;
/// `MenuJobResult::ShouldContinue` (0x1407a9200) is `cmp [rcx],1; seta al`.
const MENU_JOB_RESULT_STATE_CONTINUE_MAX: i32 = 1;

// --- CSMenuMan singleton diagnostics (SUPPORTING evidence only, NOT the
// authoritative msgbox_active signal). The builder sets a build-time "dialog mode"
// flag in the CSMenuMan singleton; teardown-clear semantics are NOT verified, so
// these are logged for corroboration/future RE but never gate the result. ---
/// `CSMenuMan.popupMenu` (`CSMenuMan+0x80`, CSPopupMenu*). Source: 1.16.2 dump
/// CSMenuManImp struct + `showPopupMenu` (`GLOBAL_CSMenuMan->popupMenu`).
const CS_MENU_MAN_POPUP_MENU_80_OFFSET: usize = 0x80;
/// `CSMenuMan.field_0x104` (i32): the builder sets this (to 1 or 0xb) for the
/// non-special dialog layout. Build-time flag; clear semantics unverified.
const CS_MENU_MAN_DIALOG_FLAG_104_OFFSET: usize = 0x104;
/// `CSMenuMan.field_0x340` (i32): the builder sets this =1 for the special (button
/// grid) dialog layout. Build-time flag; clear semantics unverified.
const CS_MENU_MAN_DIALOG_FLAG_340_OFFSET: usize = 0x340;

// --- owner-scan tuning (ported from title_binding.rs / er-input-harness-dll
// title_scan.rs). ---
/// One `ReadProcessMemory` per 64 KiB keeps the address-space walk fast.
const SCAN_CHUNK: usize = 0x10000;
/// Upper scan bound (top of the 47-bit user address space).
const SCAN_MAX: usize = 1usize << 47;
/// Lowest plausible heap/image pointer -- filters null + small sentinels.
const HEAP_LO: usize = 0x10000;
/// Write-ticks to skip between full rescans while NO active dialog is cached. A
/// dialog is transient, so (unlike the persistent title owner) we cannot cache once
/// forever; this bounds the full-scan cost to a few per second while still catching
/// the seconds-long boot warning. When a dialog IS cached it is re-validated
/// cheaply every tick with zero scanning until it tears down.
const SCAN_THROTTLE: u64 = 4;

const MARKER: &str = "er-oracle-dialog-active.on";
const OUT_FILE: &str = "er-oracle-dialog-active.jsonl";

const CURRENT_PROCESS: isize = -1;
const MEM_COMMIT: u32 = 0x1000;
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_GUARD: u32 = 0x100;

static CACHED_DIALOG: AtomicUsize = AtomicUsize::new(0);
static SCAN_COUNTDOWN: AtomicU64 = AtomicU64::new(0);

/// Windows `MEMORY_BASIC_INFORMATION` (x64). Declared with the documented layout so
/// `repr(C)` places `region_size` at +0x18 exactly like the OS struct.
#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryBasicInformation {
    base_address: usize,
    allocation_base: usize,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    mem_type: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualQuery(addr: *const c_void, buf: *mut MemoryBasicInformation, len: usize) -> usize;
    fn ReadProcessMemory(
        process: isize,
        base_address: *const c_void,
        buffer: *mut c_void,
        size: usize,
        bytes_read: *mut usize,
    ) -> i32;
}

/// Marker gate, checked once and cached (0 = unknown, 1 = off, 2 = on).
fn enabled() -> bool {
    static CACHED: AtomicU8 = AtomicU8::new(0);
    match CACHED.load(Ordering::SeqCst) {
        1 => false,
        2 => true,
        _ => {
            let on = super::marker_exists(MARKER);
            CACHED.store(if on { 2 } else { 1 }, Ordering::SeqCst);
            on
        }
    }
}

/// The class name for a matched dialog vtable, or `None` if `vt` is not a dialog
/// vtable. Also the authoritative "this pointer is a live dialog vtable" test.
fn dialog_class(vt: usize, base: usize) -> Option<&'static str> {
    if vt == base + MSGBOX_DIALOG_VTABLE_RVA {
        Some("MessageBoxDialog")
    } else if vt == base + SAVE_RETRY_DIALOG_VTABLE_RVA {
        Some("SaveRetryDialog")
    } else {
        None
    }
}

/// A validated, currently-displayed dialog located at `dialog`. `class` is the
/// resolved vtable class name; `vtable`/`latch`/`state` are the raw evidence.
#[derive(Clone, Copy)]
struct ActiveDialog {
    dialog: usize,
    vtable: usize,
    class: &'static str,
    latch: u8,
    state: i32,
}

/// Fault-safe test: is `obj` a currently-DISPLAYED (blocking, not-tearing-down)
/// MessageBoxDialog/SaveRetryDialog? Mirrors the product's `blocking_modal_present`
/// (vtable match AND `closing_latch != 1`), tightened with `state < decided` so a
/// decided/closing box does not count. Returns the evidence when active.
fn validate_active(obj: usize, base: usize) -> Option<ActiveDialog> {
    if obj < HEAP_LO {
        return None;
    }
    let vt = unsafe { safe_read_usize(obj) }?;
    let class = dialog_class(vt, base)?;
    if !vtable_in_game_image(vt, base) {
        return None;
    }
    // closing_latch is a u8; a live block reads 0/1, a torn-down one reads 1.
    let latch = unsafe { safe_read_u8(obj + MSGBOX_CLOSING_LATCH_3B0_OFFSET) }?;
    if latch != 0 {
        return None;
    }
    // A live, un-answered dialog still carries its `MenuJobResult` at Continue
    // (<= 1); once the user picks a button the emit path leaves Success/Failed
    // there. This is the real "already answered" discriminator.
    let state = unsafe { safe_read_i32(obj + MSGBOX_JOB_RESULT_STATE_1E8_OFFSET) }?;
    if state > MENU_JOB_RESULT_STATE_CONTINUE_MAX {
        return None;
    }
    // Shape check: a real dialog has a plausible button count. This rejects a stray
    // slot that merely holds the vtable value (freed/garbage blocks almost never
    // satisfy every field).
    let buttons = unsafe { safe_read_i32(obj + MSGBOX_BUTTON_COUNT_25E8_OFFSET) }?;
    if !(MSGBOX_BUTTON_COUNT_MIN..=MSGBOX_BUTTON_COUNT_MAX).contains(&buttons) {
        return None;
    }
    Some(ActiveDialog {
        dialog: obj,
        vtable: vt,
        class,
        latch,
        state,
    })
}

/// Scan one already-`ReadProcessMemory`-loaded 64 KiB chunk for a live dialog: a
/// qword equal to either dialog vtable whose object then passes `validate_active`.
/// Fault-safe: the bulk read never faults, and the field cross-checks use
/// `safe_read_*`.
fn scan_chunk(
    want_a: usize,
    want_b: usize,
    base: usize,
    addr: usize,
    len: usize,
    buf: &mut [u8],
) -> Option<ActiveDialog> {
    let mut read: usize = 0;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS,
            addr as *const c_void,
            buf.as_mut_ptr() as *mut c_void,
            len,
            &mut read,
        )
    };
    if ok == 0 {
        return None;
    }
    let word = core::mem::size_of::<usize>();
    let usable = read.min(len) & !(word - 1);
    let mut i = 0usize;
    while i + word <= usable {
        let val = usize::from_ne_bytes(buf[i..i + word].try_into().ok()?);
        if (val == want_a || val == want_b)
            && let Some(d) = validate_active(addr + i, base)
        {
            return Some(d);
        }
        i += word;
    }
    None
}

/// Bounded, fault-safe full address-space walk for a currently-displayed dialog.
/// Only called on the throttle boundary while no active dialog is cached.
fn scan_for_dialog(base: usize) -> Option<ActiveDialog> {
    let want_a = base.checked_add(MSGBOX_DIALOG_VTABLE_RVA)?;
    let want_b = base.checked_add(SAVE_RETRY_DIALOG_VTABLE_RVA)?;
    let mut buf = vec![0u8; SCAN_CHUNK];
    let mut address: usize = 0;
    while address < SCAN_MAX {
        let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
        let queried = unsafe {
            VirtualQuery(
                address as *const c_void,
                &mut info,
                core::mem::size_of::<MemoryBasicInformation>(),
            )
        };
        if queried == 0 {
            break;
        }
        let region_base = info.base_address;
        let size = info.region_size;
        let next = region_base.saturating_add(size);
        let readable = info.state == MEM_COMMIT && info.protect & (PAGE_NOACCESS | PAGE_GUARD) == 0;
        // The object must be large enough to hold the fields we cross-check.
        if readable && size >= MSGBOX_BUTTON_COUNT_25E8_OFFSET + core::mem::size_of::<i32>() {
            let mut region_off = 0usize;
            while region_off < size {
                let chunk = (size - region_off).min(SCAN_CHUNK);
                let chunk_base = region_base + region_off;
                if let Some(hit) = scan_chunk(want_a, want_b, base, chunk_base, chunk, &mut buf) {
                    return Some(hit);
                }
                region_off = region_off.saturating_add(chunk);
            }
        }
        if next <= address {
            break;
        }
        address = next;
    }
    None
}

/// Resolve the currently-displayed dialog: re-validate the cached pointer cheaply
/// first (no scan while a box stays up); else rescan (THROTTLED while none is
/// found). Returns the active dialog and whether a full scan ran this tick.
fn find_active_dialog(base: usize) -> (Option<ActiveDialog>, bool) {
    let cached = CACHED_DIALOG.load(Ordering::SeqCst);
    if cached != 0 {
        if let Some(d) = validate_active(cached, base) {
            return (Some(d), false);
        }
        CACHED_DIALOG.store(0, Ordering::SeqCst);
    }
    let countdown = SCAN_COUNTDOWN.load(Ordering::SeqCst);
    if countdown > 0 {
        SCAN_COUNTDOWN.store(countdown - 1, Ordering::SeqCst);
        return (None, false);
    }
    SCAN_COUNTDOWN.store(SCAN_THROTTLE, Ordering::SeqCst);
    match scan_for_dialog(base) {
        Some(d) => {
            CACHED_DIALOG.store(d.dialog, Ordering::SeqCst);
            (Some(d), true)
        }
        None => (None, true),
    }
}

/// Emit one dialog-active line for this write-tick. No-op when the marker is absent.
pub fn tick(base: usize, epoch: u64, play_time_ms: i64) {
    if base == 0 || !enabled() {
        return;
    }

    let (active, scanned) = find_active_dialog(base);

    // Supporting singleton diagnostics (NOT authoritative): the CSMenuMan build-time
    // dialog flags, for corroboration + future RE. Their teardown-clear semantics
    // are unverified, so they never gate `msgbox_active`.
    let cs_menu_man = unsafe { safe_read_usize(base + er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA) }
        .filter(|p| *p >= HEAP_LO);
    let popup_menu =
        cs_menu_man.and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_POPUP_MENU_80_OFFSET) });
    let flag_104 =
        cs_menu_man.and_then(|m| unsafe { safe_read_i32(m + CS_MENU_MAN_DIALOG_FLAG_104_OFFSET) });
    let flag_340 =
        cs_menu_man.and_then(|m| unsafe { safe_read_i32(m + CS_MENU_MAN_DIALOG_FLAG_340_OFFSET) });

    let msgbox_active = active.is_some();
    let dialog_ptr = active.map(|d| d.dialog);
    let dialog_vtable = active.map(|d| d.vtable);
    let dialog_class = match active {
        Some(d) => format!("\"{}\"", d.class),
        None => "null".to_string(),
    };
    let closing_latch = active.map(|d| d.latch);
    let dialog_state = active.map(|d| d.state);

    let line = format!(
        "{{\"epoch\":{epoch},\
\"play_time_ms\":{play_time_ms},\
\"msgbox_active\":{msgbox_active},\
\"dialog_class_or_vtable\":{dialog_class},\
\"dialog_vtable\":{dialog_vtable},\
\"dialog_ptr\":{dialog_ptr},\
\"closing_latch_3b0\":{closing_latch},\
\"dialog_state_25e8\":{dialog_state},\
\"scanned_this_tick\":{scanned},\
\"cs_menu_man_ptr\":{cs_menu_man},\
\"popup_menu_ptr\":{popup_menu},\
\"menuman_flag_104\":{flag_104},\
\"menuman_flag_340\":{flag_340}}}\n",
        dialog_vtable = super::json_hex_opt(dialog_vtable),
        dialog_ptr = super::json_hex_opt(dialog_ptr),
        closing_latch = super::json_u8_opt(closing_latch),
        dialog_state = super::json_i32_opt(dialog_state),
        cs_menu_man = super::json_hex_opt(cs_menu_man),
        popup_menu = super::json_hex_opt(popup_menu),
        flag_104 = super::json_i32_opt(flag_104),
        flag_340 = super::json_i32_opt(flag_340),
    );
    super::append_line(OUT_FILE, &line);
}
