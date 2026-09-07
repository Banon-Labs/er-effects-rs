//! Tier A: fault-safe readers for `FD4FileCap`, `DLString<wchar_t>` and the DLIO virtual-root
//! table.
//!
//! These moved down here from `er-title-flow` on 2026-08-25 because a second image now needs
//! them: `er-diag-harness` carries the msb-parse / DLC-root / loadlist-wait traces that used to
//! be compiled into the product DLL, and every one of those traces names a file cap or a virtual
//! root in its log line. Copying the walkers into the harness would have put two literal
//! declarations on one address, which `scripts/check-rva-alias-drift.py` refuses -- and rightly:
//! divergent copies of a struct walk are divergent claims about the layout.
//!
//! They belong at this tier by construction. Every function below is a bounded `safe_read_*`
//! walk: no writes, no native calls, no vtable dispatch, no locks, and no allocation beyond the
//! returned `String`. They run on the game thread during a stall, so a fault or an unbounded
//! walk would be worse than the stall they are measuring. `er-title-flow` re-exports the whole
//! module, so its own call sites and the product's `constants/gaitem_restore.rs` re-export chain
//! are unchanged.

use crate::mem::{safe_read_u8, safe_read_usize};

/// Smallest address treated as a plausible heap/image pointer. Anything at or below it is a
/// null, a tagged sentinel or a small integer that landed in a pointer field.
const PTR_SANITY_MIN: usize = 0x10000;

/// FD4FileCap load status; `0x04` == load complete. Paired with a non-null bytes pointer.
pub const FD4_FILECAP_STATUS_88_OFFSET: usize = 0x88;

/// `MsbFileCap::msbResCap` -- the PARSED MSB resource, not the raw file buffer. `FD4FileCap` is
/// exactly 0x90 bytes, so this is the first field of the `MsbFileCap` subclass.
///
/// It has exactly one assignment site (1.16.2 static RE, bd
/// `msbrescap-single-assignment-site-and-null-content-shortcircuit-2026-07-30`): the load-complete
/// callback `FUN_14021bbf0`, which does
/// `content = AcquireContent(cap); if (content != 0 && header_ok) { msbResCap = MsbRepository::
/// GetOrCreate(name, content, size); } ReleaseContent(cap);`
/// -- and returns normally when `content` is null. `loadState` is already `4` by then, nothing
/// errors and nothing retries, so `(loadState=4, msbResCap=0)` is a reachable silent terminal state.
/// That is precisely the profile-switch reload freeze: WorldBlockRes case 2 advances to phase 3 only
/// on `cap+0x90 != 0`, and its only other escape is also closed, so the block spins at phase 2 with
/// no timeout.
///
/// `0` and `0xDEADBEEF` mean different things and the distinction is the key discriminator:
/// `MsbFileCap::MsbFileCap` (0x14021b880) inits it to `0`, while `~MsbFileCap` (0x14021b940)
/// releases through the repository and then stores `0xDEADBEEF`. So `0` == never PARSED, never
/// "freed after use".
pub const FD4_FILECAP_BYTES_90_OFFSET: usize = 0x90;

/// `FD4ResCapHolderItem::resourceString` is an `FD4BasicHashString` at `cap+0x08`, whose
/// `DLString<wchar_t>` starts at `cap+0x10`: union (inline `wchar[8]` or pointer) at `+0x08`,
/// `length` at `+0x18`, `capacity` at `+0x20`. `capacity > 7` means the union holds a pointer.
/// Reading it names which msb the stalled cap is, which separates "wrong file requested" from
/// "right file, empty read".
pub const FD4_FILECAP_NAME_UNION_18_OFFSET: usize = 0x18;

pub const FD4_FILECAP_NAME_LENGTH_28_OFFSET: usize = 0x28;

pub const FD4_FILECAP_NAME_CAPACITY_30_OFFSET: usize = 0x30;

/// Inline-vs-pointer threshold for `DLString<wchar_t>` (SSO capacity).
pub const DLSTRING_INLINE_CAPACITY_MAX: usize = 7;

/// Cap the wide-name read so a garbage `length` cannot walk the probe off a page.
pub const FD4_FILECAP_NAME_MAX_CHARS: usize = 96;

/// `FD4FileCap::loadProcess` -> `FD4FileLoadProcess::fileLoadProcessor` (`+0x20`) -> the content
/// the load-complete callback gates on. A null anywhere along this chain makes `AcquireContent`
/// return null, which is exactly the short-circuit that leaves `msbResCap` at `0`.
pub const FD4_FILECAP_LOADPROCESS_78_OFFSET: usize = 0x78;

/// `DLIO::DLFileDeviceManager` singleton. `GetFileDeviceManager` (0x141f48b40) is literally
/// `MOV RAX,[0x1448464a8]` plus a null-check branch, so this global is the manager pointer.
///
/// NOTE this corrects bd `step3-census-registry-null-on-load2-mount-skip-confirmed-2026-07-17`,
/// which called the same address "the mounted-archive registry". It is not: a genuinely null
/// manager would break every file read in the process, so that census reading `null` was a
/// deref-depth/timing artifact and the conclusion drawn from it does not follow.
pub const DL_FILE_DEVICE_MANAGER_SINGLETON_RVA: usize =
    crate::rva::DL_FILE_DEVICE_MANAGER_SINGLETON_RVA;

/// `DLFileDeviceManager::virtualRoots` -- a `FileDeviceVirtualRootVector`
/// (`allocator +0x00`, `start +0x08`, `end +0x10`, `capacity +0x18`).
///
/// This is the phase-2 freeze suspect. The stalled caps are named
/// `mapstudio_dlc2:/m28_00_00_00.msb`, and `mapstudio_dlc2` is an entry in this vector, not a data
/// archive. It has a two-phase lifecycle: `FUN_140e06490(CSDlc, true)` -- called only from the title
/// start-game flow `FUN_1409b24e0` -- registers 13 `*_dlc2` aliases with an empty root `L""`, and
/// only `CSDlcImp::AddVirtualFileRoots` (0x140e06b80, reachable solely via `FUN_140e05fb0`, whose
/// callers are `CS::MoveMapListStep::STEP_LoadListWait` and one title-flow function) fills in the
/// real `mapstudio_dlc2 -> "map_dlc2:/mapstudio"`. If the title blanks it and the
/// `STEP_LoadListWait` gate (`loadList == NULL || *loadList in {2,3}`) does not pass on the warm
/// reload, the alias stays empty, the msb read resolves against nothing and returns 0 bytes, and
/// `msbResCap` never gets written. Reading the alias at the stall settles that without any hook.
pub const DL_FILE_DEVICE_MANAGER_VIRTUAL_ROOTS_48_OFFSET: usize = 0x48;

pub const FILE_DEVICE_VIRTUAL_ROOT_VECTOR_START_08_OFFSET: usize = 0x08;

pub const FILE_DEVICE_VIRTUAL_ROOT_VECTOR_END_10_OFFSET: usize = 0x10;

/// `FileDeviceVirtualRootVectorEntry`: `root` (the alias name) and `path` (what it resolves to),
/// both `DLString<wchar_t>` (48 bytes each), so the entry stride is 0x60.
pub const FILE_DEVICE_VIRTUAL_ROOT_ENTRY_STRIDE: usize = 0x60;

pub const FILE_DEVICE_VIRTUAL_ROOT_ENTRY_PATH_30_OFFSET: usize = 0x30;

/// `DLString<wchar_t>` field offsets relative to the string itself (`allocator +0x00`,
/// union `+0x08`, `length +0x18`, `capacity +0x20`). The `FD4_FILECAP_NAME_*` constants above are
/// the same layout pre-added to the cap's `+0x10` string base, which is why their numbers differ.
pub const DLSTRING_UNION_08_OFFSET: usize = 0x08;

pub const DLSTRING_LENGTH_18_OFFSET: usize = 0x18;

pub const DLSTRING_CAPACITY_20_OFFSET: usize = 0x20;

/// Bound the virtual-root walk: the table is a few dozen aliases, so anything past this is a
/// corrupt/mid-teardown vector and the probe must stop rather than walk off a page.
pub const FILE_DEVICE_VIRTUAL_ROOT_MAX_ENTRIES: usize = 256;

/// Alias prefixes worth reporting at the stall: the DLC roots that back `m28`, plus the base-game
/// `mapstudio` for contrast (if the base alias is populated and the dlc2 one is not, that is the
/// answer outright).
pub const VIRTUAL_ROOTS_OF_INTEREST: [&str; 4] =
    ["mapstudio_dlc2", "map_dlc2", "game_dlc2", "mapstudio"];

pub const FD4_FILELOADPROCESS_PROCESSOR_20_OFFSET: usize = 0x20;

/// `FD4FileLoadProcessor`: `content_` at `+0x20`, its byte count at `+0x28`, and the
/// acquire/release refcount at `+0x30` whose `1 -> 0` edge nulls `content_` and frees the buffer.
pub const FD4_FILELOADPROCESSOR_CONTENT_20_OFFSET: usize = 0x20;

pub const FD4_FILELOADPROCESSOR_SIZE_28_OFFSET: usize = 0x28;

pub const FD4_FILELOADPROCESSOR_ACQUIRE_30_OFFSET: usize = 0x30;

/// Walk `FD4FileCap::loadProcess -> FD4FileLoadProcess::fileLoadProcessor` and sample the content
/// state the MSB load-complete callback gates on, returning `(processor, content, size, acquires)`.
///
/// This is the exact chain `FD4FileCap::AcquireContent` (`FUN_1426591c0`) walks: it returns null if
/// either `loadProcess` or `fileLoadProcessor` is null, and otherwise hands back `processor.content_`
/// -- re-fetching it through a vtable call only on the acquire refcount's `0 -> 1` edge, while the
/// matching release nulls `content_` on the `1 -> 0` edge. A null content is what makes
/// `MsbFileCap::msbResCap` stay `0` even at `loadState == 4`, so sampling it says whether the freeze
/// is "no buffer at all" or "buffer present but never parsed". Read-only: no acquire, no refcount
/// touch, no vtable call.
///
/// # Safety
/// `load_process` is an arbitrary integer; every dereference goes through `safe_read_usize`, so a
/// wild or unmapped value yields the `(0, 0, 0, -1)` sentinel rather than a fault.
pub unsafe fn fd4_filecap_content_state(load_process: usize) -> (usize, usize, usize, i64) {
    if load_process <= PTR_SANITY_MIN {
        return (0, 0, 0, -1);
    }
    let Some(processor) =
        (unsafe { safe_read_usize(load_process + FD4_FILELOADPROCESS_PROCESSOR_20_OFFSET) })
            .filter(|&v| v > PTR_SANITY_MIN)
    else {
        return (0, 0, 0, -1);
    };
    let content = unsafe { safe_read_usize(processor + FD4_FILELOADPROCESSOR_CONTENT_20_OFFSET) }
        .unwrap_or(0);
    let size =
        unsafe { safe_read_usize(processor + FD4_FILELOADPROCESSOR_SIZE_28_OFFSET) }.unwrap_or(0);
    let acquires = unsafe { safe_read_usize(processor + FD4_FILELOADPROCESSOR_ACQUIRE_30_OFFSET) }
        .map(|v| (v & 0xffff_ffff) as i64)
        .unwrap_or(-1);
    (processor, content, size, acquires)
}

/// Read an `FD4ResCapHolderItem`'s resource name (the msb filename) off a file cap, as ASCII.
///
/// `resourceString` is an `FD4BasicHashString` whose `DLString<wchar_t>` is small-string-optimized:
/// `capacity > 7` means the union at `+0x18` holds a heap pointer, otherwise the characters sit
/// inline in the union itself. Both `length` and the read are clamped so a garbage capacity cannot
/// walk the probe off a page, every character goes through `safe_read_u8`, and non-ASCII collapses
/// to `?` -- this runs on the game thread during a stall, so it must not fault or allocate wildly.
///
/// # Safety
/// `cap` is an arbitrary integer; every dereference is fault-tolerant, so a wild value yields a
/// `<badptr>`/`<trunc>` marker string rather than a fault.
pub unsafe fn fd4_filecap_name(cap: usize) -> String {
    let capacity =
        unsafe { safe_read_usize(cap + FD4_FILECAP_NAME_CAPACITY_30_OFFSET) }.unwrap_or(0);
    let length = unsafe { safe_read_usize(cap + FD4_FILECAP_NAME_LENGTH_28_OFFSET) }.unwrap_or(0);
    let union_addr = cap + FD4_FILECAP_NAME_UNION_18_OFFSET;
    let chars_addr = if capacity > DLSTRING_INLINE_CAPACITY_MAX {
        match unsafe { safe_read_usize(union_addr) }.filter(|&v| v > PTR_SANITY_MIN) {
            Some(ptr) => ptr,
            None => return String::from("<badptr>"),
        }
    } else {
        union_addr
    };
    unsafe { clamped_wide_ascii(chars_addr, length) }
}

/// Read a `DLString<wchar_t>` (given the address of the string itself) as clamped ASCII.
///
/// Same small-string-optimization rule as `fd4_filecap_name`: `capacity > 7` means the union holds
/// a heap pointer, otherwise the characters are inline. Kept separate because that helper takes a
/// cap and bakes in the `+0x10` string base, while virtual-root entries hold bare `DLString`s.
///
/// # Safety
/// `string_base` is an arbitrary integer; every dereference is fault-tolerant.
pub unsafe fn dlstring_wide_ascii(string_base: usize) -> String {
    if string_base <= PTR_SANITY_MIN {
        return String::new();
    }
    let capacity =
        unsafe { safe_read_usize(string_base + DLSTRING_CAPACITY_20_OFFSET) }.unwrap_or(0);
    let length = unsafe { safe_read_usize(string_base + DLSTRING_LENGTH_18_OFFSET) }.unwrap_or(0);
    let union_addr = string_base + DLSTRING_UNION_08_OFFSET;
    let chars_addr = if capacity > DLSTRING_INLINE_CAPACITY_MAX {
        match unsafe { safe_read_usize(union_addr) }.filter(|&v| v > PTR_SANITY_MIN) {
            Some(ptr) => ptr,
            None => return String::from("<badptr>"),
        }
    } else {
        union_addr
    };
    unsafe { clamped_wide_ascii(chars_addr, length) }
}

/// The shared tail of both string readers: `length` UTF-16 units from `chars_addr`, clamped to
/// `FD4_FILECAP_NAME_MAX_CHARS`, byte-at-a-time through `safe_read_u8`, non-ASCII collapsed to `?`.
///
/// Written once rather than twice: the two callers differ only in how they locate `chars_addr`,
/// and a duplicated character loop is a duplicated chance to get the clamp wrong on one side.
///
/// # Safety
/// `chars_addr` is an arbitrary integer; every read is fault-tolerant and the walk is bounded.
unsafe fn clamped_wide_ascii(chars_addr: usize, length: usize) -> String {
    let count = length.min(FD4_FILECAP_NAME_MAX_CHARS);
    let mut out = String::with_capacity(count);
    for i in 0..count {
        let (Some(lo), Some(hi)) = (unsafe { safe_read_u8(chars_addr + i * 2) }, unsafe {
            safe_read_u8(chars_addr + i * 2 + 1)
        }) else {
            out.push_str("<trunc>");
            break;
        };
        let unit = u16::from(lo) | (u16::from(hi) << 8);
        if unit == 0 {
            break;
        }
        out.push(if (0x20..0x7f).contains(&unit) {
            unit as u8 as char
        } else {
            '?'
        });
    }
    out
}

/// Report the DLIO virtual-root aliases that back the stalled `mapstudio_dlc2:/m28_*.msb` reads.
///
/// The phase-2 freeze's file caps resolve through `mapstudio_dlc2:`, which is an alias in
/// `DLFileDeviceManager::virtualRoots`, not a data archive. That alias is registered empty (`L""`)
/// by the title start-game flow and only filled in by `CSDlcImp::AddVirtualFileRoots` behind the
/// `STEP_LoadListWait` gate. So an alias present with an empty path at the stall means the read had
/// nowhere to resolve to -- which is exactly a 0-byte read and a null `msbResCap`. Emitting
/// `mapstudio` alongside it is the control: base-game populated + dlc2 empty is decisive on its own.
///
/// Strictly read-only -- a vector walk with bounded length and per-field `safe_read_*`, no locks and
/// no allocation beyond the returned string, because this runs on the game thread mid-stall.
///
/// # Safety
/// `base` is the game image base; every dereference below it is fault-tolerant and bounded by
/// `FILE_DEVICE_VIRTUAL_ROOT_MAX_ENTRIES`.
pub unsafe fn dlio_virtual_roots_summary(base: usize) -> String {
    if base == 0 {
        return String::from("<nobase>");
    }
    let Some(manager) = (unsafe {
        safe_read_usize(crate::mem::game_data_addr(
            base,
            DL_FILE_DEVICE_MANAGER_SINGLETON_RVA,
            "DL_FILE_DEVICE_MANAGER_SINGLETON_RVA",
        ))
    })
    .filter(|&v| v > PTR_SANITY_MIN) else {
        return String::from("<mgrnull>");
    };
    let roots = manager + DL_FILE_DEVICE_MANAGER_VIRTUAL_ROOTS_48_OFFSET;
    let (Some(start), Some(end)) = (
        unsafe { safe_read_usize(roots + FILE_DEVICE_VIRTUAL_ROOT_VECTOR_START_08_OFFSET) },
        unsafe { safe_read_usize(roots + FILE_DEVICE_VIRTUAL_ROOT_VECTOR_END_10_OFFSET) },
    ) else {
        return String::from("<vecunreadable>");
    };
    if start <= PTR_SANITY_MIN || end <= start {
        return format!("<vecempty start={start:#x} end={end:#x}>");
    }
    let count = ((end - start) / FILE_DEVICE_VIRTUAL_ROOT_ENTRY_STRIDE)
        .min(FILE_DEVICE_VIRTUAL_ROOT_MAX_ENTRIES);
    let mut out = String::new();
    let mut seen = 0usize;
    for i in 0..count {
        let entry = start + i * FILE_DEVICE_VIRTUAL_ROOT_ENTRY_STRIDE;
        let name = unsafe { dlstring_wide_ascii(entry) };
        if !VIRTUAL_ROOTS_OF_INTEREST.iter().any(|w| *w == name) {
            continue;
        }
        seen += 1;
        let path =
            unsafe { dlstring_wide_ascii(entry + FILE_DEVICE_VIRTUAL_ROOT_ENTRY_PATH_30_OFFSET) };
        // An empty path on a present alias is the whole point of this probe -- label it loudly so a
        // log scan cannot mistake it for a formatting artifact.
        let verdict = if path.is_empty() { "EMPTY" } else { "ok" };
        let _ = core::fmt::Write::write_fmt(&mut out, format_args!("{name}='{path}'({verdict}),"));
    }
    format!("total={count}/matched={seen}/{out}")
}
