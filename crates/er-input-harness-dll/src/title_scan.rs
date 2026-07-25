//! TITLE-OWNER SCAN -- passive read of the TITLE screen's state machine, ported from the product's
//! `find_title_owner_by_vtable` (experiments/title/profile_select_flow.rs). It lets the harness gate the
//! title phases (PRESS ANY BUTTON parked vs. Continue/Load menu built) on real RAM semaphores instead of
//! guessing, per bd TITLE-CONTINUE-is-accept-byte-not-keystate (owner+0x48 state, dialog+0xa40).
//!
//! The title owner is a heap object whose vtable is `base + TITLE_OWNER_VTABLE_RVA` and whose per-
//! instance state-dispatch table (at `owner+0x10`) is `base + INNER_TITLE_STATE_TABLE_RVA`. To find it we
//! walk this process's own address space with `VirtualQuery`, read each committed/readable region in
//! 64KB chunks via `ReadProcessMemory` (a chunk freed by the booting game returns FALSE instead of
//! faulting), and scan for a pointer-slot equal to the vtable whose `+0x10` slot equals the state table.
//!
//! All reads are fault-safe (`ReadProcessMemory` on the current-process pseudo handle, same idiom as
//! `win32::read_usize`/`read_u8`), so a not-yet-initialized/garbage pointer can never fault the game
//! thread. Discovery is incremental and bounded per frame so a not-found state can still emit phase
//! heartbeats instead of blocking the game thread. Game-thread only.

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery,
};

use crate::log::harness_log;
use crate::win32::{read_u8, read_usize};

// --- RVAs / offsets off the game image base (0x140000000), bd TITLE-CONTINUE-is-accept-byte-not-keystate ---
/// Title-owner vtable RVA -- `[owner+0x00]` equals `base + this`.
const TITLE_OWNER_VTABLE_RVA: usize = 0x2b63bb0;
/// Per-instance state-dispatch table RVA -- `[owner+0x10]` equals `base + this` (the discriminator that
/// rejects stray `.data` matches on the vtable value alone).
const INNER_TITLE_STATE_TABLE_RVA: usize = 0x3d71580;
/// Live title state (`owner+0x48`, i32): 10 = MenuJobWait (PAB + menu), 11 = Finish, 6 = in-world.
const TITLE_OWNER_STATE_OFFSET: usize = 0x48;
/// State-table pointer slot within the owner (`owner+0x10`).
const TITLE_OWNER_INSTANCE_TABLE_OFFSET: usize = 0x10;
/// TitleTopDialog holder (`owner+0xe0` -> dialog).
const TITLE_OWNER_DIALOG_E0_OFFSET: usize = 0xe0;
/// TitleTopDialog vtable RVA -- `[dialog+0x00]` equals `base + this`.
const TITLETOP_DIALOG_VTABLE_RVA: usize = 0x2b26468;
/// TitleTopDialog discriminator (`dialog+0xa40`, u8): 0 = PRESS ANY BUTTON parked / 1 = Continue-Load
/// menu built (both are outer state 10 -- this byte is THE difference).
const TITLETOP_DIALOG_A40_OFFSET: usize = 0xa40;

// --- scan tuning ---
/// One `ReadProcessMemory` per 64KB keeps an individual memory read bounded.
const SCAN_CHUNK: usize = 0x10000;
/// Limit title-owner discovery work per game frame so startup never stalls inside one full address-space
/// walk. The external watcher can then observe phase heartbeats instead of killing a silently blocked
/// game thread.
const SCAN_CHUNKS_PER_CALL: usize = 8;
/// Upper scan bound (above 64-bit user address space; `VirtualQuery` fails out before this in practice).
const SCAN_MAX: usize = 1usize << 47;
/// Lowest plausible heap/image pointer -- filters null and small sentinels out of pointer walks.
const HEAP_LO: usize = 0x10000;

static CACHED_OWNER: AtomicUsize = AtomicUsize::new(0);
static SCAN_CURSOR: AtomicUsize = AtomicUsize::new(0);
static OWNER_LOGGED: AtomicBool = AtomicBool::new(false);

/// The current-process pseudo handle (`-1`) for `ReadProcessMemory`.
fn cur_proc() -> HANDLE {
    HANDLE((-1isize) as *mut c_void)
}

/// Scan a single 64KB chunk (already read into `buf`) for a candidate title owner. Returns the owner
/// address when a pointer-slot equals the vtable AND that candidate's `+0x10` slot equals the state
/// table. Fault-safe: `ReadProcessMemory` never faults, and the `+0x10` cross-check uses `read_usize`.
fn scan_chunk(
    want_vtable: usize,
    want_table: usize,
    addr: usize,
    len: usize,
    buf: &mut [u8],
) -> Option<usize> {
    let mut read: usize = 0;
    let ok = unsafe {
        ReadProcessMemory(
            cur_proc(),
            addr as *const c_void,
            buf.as_mut_ptr().cast(),
            len,
            Some(&mut read),
        )
    };
    if ok.is_err() {
        return None;
    }
    let usable = read.min(len) & !(core::mem::size_of::<usize>() - 1);
    let mut i = 0usize;
    while i + core::mem::size_of::<usize>() <= usable {
        let val = usize::from_ne_bytes(buf[i..i + core::mem::size_of::<usize>()].try_into().ok()?);
        if val == want_vtable {
            let candidate = addr + i;
            if unsafe { read_usize(candidate + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }
                == Some(want_table)
            {
                return Some(candidate);
            }
        }
        i += core::mem::size_of::<usize>();
    }
    None
}

/// Incremental memory walk for the title owner. Each call scans at most `SCAN_CHUNKS_PER_CALL`
/// 64KB chunks, then stores a cursor for the next frame. This preserves the same fault-safe evidence
/// path as the previous full scan but avoids blocking the game thread for seconds during boot.
fn scan_for_owner(base: usize) -> Option<usize> {
    let want_vtable = base.checked_add(TITLE_OWNER_VTABLE_RVA)?;
    let want_table = base.checked_add(INNER_TITLE_STATE_TABLE_RVA)?;
    let mut buf = [0u8; SCAN_CHUNK];
    let mut address = SCAN_CURSOR
        .load(Ordering::SeqCst)
        .min(SCAN_MAX.saturating_sub(1));
    let mut chunks_scanned = 0usize;

    while address < SCAN_MAX && chunks_scanned < SCAN_CHUNKS_PER_CALL {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let queried = unsafe {
            VirtualQuery(
                Some(address as *const c_void),
                &mut info,
                core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried == 0 {
            SCAN_CURSOR.store(0, Ordering::SeqCst);
            return None;
        }
        let region_base = info.BaseAddress as usize;
        let size = info.RegionSize;
        let next_region = region_base.saturating_add(size);
        let protect = info.Protect.0;
        let readable = info.State.0 == MEM_COMMIT.0
            && protect & PAGE_NOACCESS.0 == 0
            && protect & PAGE_GUARD.0 == 0;
        if readable && size >= TITLE_OWNER_STATE_OFFSET + core::mem::size_of::<i32>() {
            let region_off = address.saturating_sub(region_base).min(size);
            let chunk = (size - region_off).min(SCAN_CHUNK);
            let chunk_base = region_base + region_off;
            if let Some(hit) = scan_chunk(want_vtable, want_table, chunk_base, chunk, &mut buf) {
                SCAN_CURSOR.store(0, Ordering::SeqCst);
                return Some(hit);
            }
            chunks_scanned += 1;
            let next_chunk = chunk_base.saturating_add(chunk);
            address = if next_chunk < next_region {
                next_chunk
            } else {
                next_region
            };
        } else {
            address = next_region;
        }
        if address <= region_base {
            SCAN_CURSOR.store(0, Ordering::SeqCst);
            return None;
        }
    }

    if address >= SCAN_MAX {
        address = 0;
    }
    SCAN_CURSOR.store(address, Ordering::SeqCst);
    None
}

/// Resolve the title owner: return the cached pointer if `[ptr+0] == base+vtable` still holds;
/// otherwise advance the bounded incremental scan. Caches and logs once on capture.
pub fn find_title_owner(base: usize) -> Option<usize> {
    let cached = CACHED_OWNER.load(Ordering::SeqCst);
    if cached != 0 {
        if unsafe { read_usize(cached) } == Some(base + TITLE_OWNER_VTABLE_RVA) {
            return Some(cached);
        }
        // The owner was freed / vtable no longer matches -> invalidate and resume scanning.
        CACHED_OWNER.store(0, Ordering::SeqCst);
    }

    let owner = scan_for_owner(base)?;
    CACHED_OWNER.store(owner, Ordering::SeqCst);
    if !OWNER_LOGGED.swap(true, Ordering::SeqCst) {
        harness_log!(
            "title_scan: captured title owner 0x{owner:x} (vtable base+0x{TITLE_OWNER_VTABLE_RVA:x}, state-table base+0x{INNER_TITLE_STATE_TABLE_RVA:x}, state={})",
            unsafe { read_usize(owner + TITLE_OWNER_STATE_OFFSET) }
                .map_or(-1, |v| (v & 0xffff_ffff) as u32 as i32)
        );
    }
    Some(owner)
}

/// Live title state (`owner+0x48`, i32), or `-1` if the owner is not resolved.
pub fn title_state(base: usize) -> i32 {
    let Some(owner) = find_title_owner(base) else {
        return -1;
    };
    unsafe { read_usize(owner + TITLE_OWNER_STATE_OFFSET) }
        .map_or(-1, |v| (v & 0xffff_ffff) as u32 as i32)
}

/// The TitleTopDialog pointer (`owner+0xe0`), validated by its vtable, or `None`.
pub fn title_dialog(base: usize) -> Option<usize> {
    let owner = find_title_owner(base)?;
    let dialog =
        (unsafe { read_usize(owner + TITLE_OWNER_DIALOG_E0_OFFSET) }).filter(|p| *p >= HEAP_LO)?;
    let vtable = unsafe { read_usize(dialog) }?;
    (vtable == base + TITLETOP_DIALOG_VTABLE_RVA).then_some(dialog)
}

/// TitleTopDialog discriminator byte (`dialog+0xa40`): 0 = PAB parked, 1 = menu up; `-1` if no dialog.
pub fn title_dialog_a40(base: usize) -> i32 {
    match title_dialog(base) {
        Some(dialog) => {
            (unsafe { read_u8(dialog + TITLETOP_DIALOG_A40_OFFSET) }).map_or(-1, i32::from)
        }
        None => -1,
    }
}

/// PRESS ANY BUTTON ready and the menu NOT yet opened: state == 10 && dialog a40 == 0.
pub fn title_pab_parked(base: usize) -> bool {
    title_state(base) == 10 && title_dialog_a40(base) == 0
}

/// The Continue/Load menu has been built (dialog a40 == 1).
pub fn title_menu_up(base: usize) -> bool {
    title_dialog_a40(base) == 1
}
