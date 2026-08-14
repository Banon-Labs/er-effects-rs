use crate::compat::*;
use er_game_base::fnv1a::fnv1a64;

// DLC VIRTUAL ROOT SELF-HEAL -- the profile-switch reload softlock fix.
//
// PROVEN CAUSE (bd `FORK-RESOLVED-refill-job-never-enqueued-on-reload-fix-is-gated-self-heal-2026-07-30`):
// the title start-game flow re-registers the 13 `*_dlc2` DLIO aliases with an EMPTY root on EVERY
// pass (`FUN_140e06490`), but the job that repopulates them (`FUN_140e05fb0` via the MenuFunctorJob
// whose `_Do_call` is `FUN_140836f30`) is enqueued only on boot. Measured, one run:
//     BLANK #1 (boot) -> JOB #1 -> REFILL #1 -> roots populated
//     BLANK #2 (reload) -> no JOB #2, no REFILL #2 -> roots EMPTY
// A `mapstudio_dlc2:/m28_*.msb` read then resolves against an empty root and returns 0 bytes, the
// sole `msbResCap` writer short-circuits on null content, and `WorldBlockRes` case 2 waits forever.
//
// WHY THIS CALLS A NATIVE FUNCTION INSTEAD OF ENQUEUEING THE NATIVE JOB: the enqueue chain is a
// closed loop -- `FUN_140836f30` is the `_Do_call` of a functor that `FUN_14082faf0` builds, and
// `FUN_14082faf0` is reachable only from that same loop. Every link is DATA-dispatched with no
// static call site, so there is no enqueue owner to invoke (independently reproduces the July
// finding "dynamically-built std::function, no static registration"). `FUN_140e05fb0` is
// self-contained -- it re-queries Steam DLC ownership through `dlcPlatform` vtable[8]/[0xb]/[0xc],
// rewrites every root 0..0x31 via `AddVirtualFileRoots`, and sets the `+0x45` valid latch -- so
// calling it is not a private per-frame pump of a MenuJob queue.
//
// GATE: the observed POPULATED -> EMPTY transition, NOT a timer and NOT `CSDlcImp+0x45` alone.
//   * `+0x45` is legitimately 0 during early boot before the game's own first refill, so gating on
//     it would fire before Steam DLC ownership is established.
//   * That earliness is not cosmetic: `AddVirtualFileRoots` silently substitutes `L"system:/"` when
//     `CS_DLC_IsEnableDlcFileMount` is false or ownership is unresolved. A too-early call would
//     "succeed", set `+0x45 = 1`, and leave DLC content resolving to the WRONG root -- a silent
//     corruption strictly worse than a visible softlock.
// Requiring a prior good observation means we only ever act on a root the game itself had already
// populated correctly, and only after it was destroyed.
//
// PROOF OBLIGATION: "we called the refill" is NOT success. The heal verifies the resulting string
// equals the expected `map_dlc2:/mapstudio` -- `L"system:/"` is non-empty and wrong, so a
// non-empty check would pass the exact failure this gate exists to prevent.

/// Locate the `mapstudio_dlc2` entry in `DLFileDeviceManager::virtualRoots` and return its address.
/// Cached: the vector is stable once built, and this runs every frame.
pub unsafe fn dlc_root_entry_addr(base: usize) -> Option<usize> {
    let cached = DLC_ROOT_ENTRY_ADDR.load(Ordering::SeqCst);
    if cached > 0x10000 {
        // RE-VALIDATE, do not trust the cache blindly. The alias table GROWS (measured: the title's
        // registration takes it from 87 to 100 entries), and a growing vector reallocates -- which
        // would leave this pointer dangling in a per-frame game-thread path. Confirming the name
        // still reads back is one short string read and turns a potential wild write into a
        // re-resolve.
        if unsafe { dlstring_wide_ascii(cached) } == DLC_ROOT_ALIAS_NAME {
            return Some(cached);
        }
        DLC_ROOT_ENTRY_ADDR.store(0, Ordering::SeqCst);
    }
    let manager = unsafe { safe_read_usize(base + DL_FILE_DEVICE_MANAGER_SINGLETON_RVA) }
        .filter(|&v| v > 0x10000)?;
    let roots = manager + DL_FILE_DEVICE_MANAGER_VIRTUAL_ROOTS_48_OFFSET;
    let start =
        unsafe { safe_read_usize(roots + FILE_DEVICE_VIRTUAL_ROOT_VECTOR_START_08_OFFSET) }?;
    let end = unsafe { safe_read_usize(roots + FILE_DEVICE_VIRTUAL_ROOT_VECTOR_END_10_OFFSET) }?;
    if start <= 0x10000 || end <= start {
        return None;
    }
    let count = ((end - start) / FILE_DEVICE_VIRTUAL_ROOT_ENTRY_STRIDE)
        .min(FILE_DEVICE_VIRTUAL_ROOT_MAX_ENTRIES);
    for i in 0..count {
        let entry = start + i * FILE_DEVICE_VIRTUAL_ROOT_ENTRY_STRIDE;
        if unsafe { dlstring_wide_ascii(entry) } == DLC_ROOT_ALIAS_NAME {
            DLC_ROOT_ENTRY_ADDR.store(entry, Ordering::SeqCst);
            return Some(entry);
        }
    }
    None
}

/// Per-frame DLC virtual-root self-heal. Cheap: one cached pointer plus a length read on the common
/// path; the full string read and the native call happen only on the populated -> empty edge.
pub unsafe fn dlc_roots_self_heal_tick() {
    let Some(base) = game_module_base().ok().filter(|&b| b != 0) else {
        return;
    };
    let Some(entry) = (unsafe { dlc_root_entry_addr(base) }) else {
        return;
    };
    let path_str = entry + FILE_DEVICE_VIRTUAL_ROOT_ENTRY_PATH_30_OFFSET;
    let Some(length) = (unsafe { safe_read_usize(path_str + DLSTRING_LENGTH_18_OFFSET) }) else {
        return;
    };

    if length != 0 {
        // Populated. Arm the heal AND record what "good" actually looks like, as a hash of the
        // live string. SELF-CALIBRATING ON PURPOSE: a hard-coded literal transcribed from the
        // decompile was wrong in exactly this spot -- the native stores `map_dlc2:/mapstudio/`
        // (trailing slash) while `AddVirtualRootMapEntry`'s source literal has none, so the heal
        // reported WRONG-ROOT on roots it had restored correctly and the alarm counter became
        // meaningless. Comparing against what the game itself produced cannot drift like that.
        DLC_ROOT_SEEN_POPULATED.store(1, Ordering::SeqCst);
        let good = unsafe { dlstring_wide_ascii(path_str) };
        DLC_ROOT_GOOD_PATH_HASH.store(fnv1a64(good.as_bytes()) as usize, Ordering::SeqCst);
        return;
    }
    if DLC_ROOT_SEEN_POPULATED.load(Ordering::SeqCst) == 0 {
        // Never yet seen good -- this is early boot before the game's own first refill. Acting here
        // is the silent-corruption case described above.
        return;
    }

    let csdlc = match unsafe { safe_read_usize(base + CSDLC_SINGLETON_RVA) } {
        Some(p) if p > 0x10000 => p,
        _ => return,
    };
    let orig = DLC_ROOTS_REFILL_ORIG.load(Ordering::SeqCst);
    let refill: usize = if orig != 0 {
        // Prefer the trampoline when the diagnostic trace is installed, so the heal does not
        // re-enter our own detour (which would double-log and recurse through this same edge).
        orig
    } else {
        match game_rva(DLC_ROOTS_REFILL_RVA as u32) {
            Ok(addr) => addr,
            Err(_) => return,
        }
    };

    let attempt = DLC_ROOT_HEAL_ATTEMPTS.fetch_add(1, Ordering::SeqCst) + 1;
    // Disarm BEFORE calling: if the call fails to populate, the next frame must not retry forever.
    // Re-arming happens naturally the moment a populated root is observed again.
    DLC_ROOT_SEEN_POPULATED.store(0, Ordering::SeqCst);
    let f: unsafe extern "system" fn(usize, u8) = unsafe { core::mem::transmute(refill) };
    unsafe { f(csdlc, 1) };

    // VERIFY THE VALUE, NOT THE CALL. `L"system:/"` is non-empty and wrong, so a non-empty check
    // would pass the silent-corruption case this gate exists to catch. Compare against the hash of
    // the root the GAME populated earlier in this same process.
    let healed = unsafe { dlstring_wide_ascii(path_str) };
    let expected_hash = DLC_ROOT_GOOD_PATH_HASH.load(Ordering::SeqCst);
    let ok = expected_hash != 0 && fnv1a64(healed.as_bytes()) as usize == expected_hash;
    if ok {
        DLC_ROOT_HEAL_OK.fetch_add(1, Ordering::SeqCst);
    } else {
        DLC_ROOT_HEAL_WRONG.fetch_add(1, Ordering::SeqCst);
    }
    append_crash_log(format_args!(
        "dlc-roots-HEAL #{attempt}: {} '{DLC_ROOT_ALIAS_NAME}' -> '{healed}' csdlc=0x{csdlc:x} ok={} wrong={}",
        if ok { "RESTORED" } else { "WRONG-ROOT" },
        DLC_ROOT_HEAL_OK.load(Ordering::SeqCst),
        DLC_ROOT_HEAL_WRONG.load(Ordering::SeqCst)
    ));
}
