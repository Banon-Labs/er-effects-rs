use std::sync::atomic::Ordering;

// ===========================================================================
// SAVE-REDIRECT DETOUR RE-ENTRANCY GUARD
// ===========================================================================
//
// Every save-redirect detour in this module runs on the CALLER's thread, and several of them do
// real filesystem work as a side effect (`fs::read` of the configured save for the SteamID
// normalize, `fs::read`/`fs::write` for direct-file staging). That work reaches the OS through
// `kernel32!CreateFileW` -- i.e. straight back into the very detour that started it.
//
// Without a guard that is unbounded recursion, not a retry loop:
// `save_redirect_createfilew_hook` -> `normalize_env_save_file_to_active_steam_id_once` ->
// `fs::read(configured .sl2)` -> `CreateFileW` -> the detour again, with the one-shot latch still
// unset because it was only stored AFTER the read returned. Observed live 2026-07-30:
// `SAVE_CREATEFILEW_DIAG_HITS` climbed 3 -> 512 in ~4ms on the game's 1 MiB main thread (~1168
// bytes of stack per frame) and 1024 -> 2048 on a spawned 2 MiB Rust thread -- the 2x ratio IS the
// stack bound. The thread died of guard-page exhaustion mid-descent, which is why nothing was ever
// logged from the error arm and the crash log stayed empty.
//
// Same shape, same fix as `AutoloadDebugReentryGuard` in `telemetry/save_policy_logs.rs`: the guard
// is per-THREAD because the nesting is always a synchronous same-thread call chain, and a
// process-wide flag would wrongly mute a legitimate concurrent open on another thread.
//
// # Why two counters and not one
//
// `ntdll!NtCreateFile` is not a peer of the Win32 detours, it is BENEATH them: kernel32's own
// `CreateFileW` calls it, so the ntdll detour fires once for every Win32 open that already went
// through a detour above it. Counting it in the same depth would make a perfectly healthy open read
// as depth 2, and a healthy normalize-triggering open read as depth 3 -- so a single-counter
// oracle would have to alarm above 3, a threshold that drifts with Wine's internal call shape and
// with how much the logger happens to nest. A semaphore that false-positives gets ignored, which is
// worse than not having one.
//
// So the reported DEPTH counts only the Win32 file detours -- the layer where our own `fs::` calls
// actually land and where runaway recursion happens -- and the ntdll diagnostic contributes a plain
// "I am inside a detour" flag. Both suppress disk I/O; only the first is a depth. That makes the
// alarm exact:
//
//   1 = an open where no detour did its own I/O
//   2 = a detour's own `fs::read`/`fs::write` re-entered once and was passed through (steady state)
//  >2 = a pass-through decision was lost and this bug class is back

use er_telemetry::counters::{
    SAVE_REDIRECT_DETOUR_MAX_DEPTH, SAVE_REDIRECT_DETOUR_REENTRANT_PASSTHROUGHS,
};

/// Depth value used when the thread-local cannot be reached at all (thread teardown). An
/// unanswerable "am I nested?" counts as nested: refusing one observation costs a diagnostic,
/// guessing wrong costs the process.
const SAVE_DETOUR_DEPTH_UNKNOWN: usize = usize::MAX;

std::thread_local! {
    /// How many WIN32 save-redirect file detours THIS thread is currently inside. This is the
    /// number the max-depth oracle reports; see the module comment for why ntdll is not in it.
    static SAVE_DETOUR_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// True while THIS thread is inside the ntdll `NtCreateFile` diagnostic detour.
    static SAVE_NTCREATE_DETOUR_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// RAII depth token for a WIN32 save-redirect file detour (`CreateFileW`, `CopyFileW`,
/// `GetFileAttributes(Ex)W`, `FindFirstFileW`). Take one at the TOP of the detour body and hold it
/// for the whole call; `is_reentrant()` then says whether this entry is nested inside any
/// save-redirect detour on the same thread.
///
/// A re-entrant entry must degrade to a pure pass-through: call the original API with the caller's
/// own arguments and skip every side effect (redirect decision, observation, staging, normalize,
/// diagnostics). Our own nested I/O addresses real paths we computed ourselves, so it wants the
/// unmodified API and none of the bookkeeping.
pub struct SaveDetourDepth {
    depth: usize,
    reentrant: bool,
}

impl SaveDetourDepth {
    pub fn enter() -> Self {
        // Read BEFORE incrementing: an ntdll detour already active above us makes this entry
        // nested even though it is the first Win32 one on the stack.
        let below_a_detour = !save_detour_disk_io_allowed();
        let depth = SAVE_DETOUR_DEPTH
            .try_with(|cell| {
                let depth = cell.get().saturating_add(1);
                cell.set(depth);
                depth
            })
            .unwrap_or(SAVE_DETOUR_DEPTH_UNKNOWN);
        if depth != SAVE_DETOUR_DEPTH_UNKNOWN {
            SAVE_REDIRECT_DETOUR_MAX_DEPTH.fetch_max(depth, Ordering::SeqCst);
        }
        let reentrant = below_a_detour || depth > 1;
        if reentrant {
            SAVE_REDIRECT_DETOUR_REENTRANT_PASSTHROUGHS.fetch_add(1, Ordering::SeqCst);
        }
        Self { depth, reentrant }
    }

    /// True when this detour entry is nested inside another save-redirect detour on this thread.
    pub fn is_reentrant(&self) -> bool {
        self.reentrant
    }
}

impl Drop for SaveDetourDepth {
    fn drop(&mut self) {
        if self.depth == SAVE_DETOUR_DEPTH_UNKNOWN {
            return;
        }
        let _ = SAVE_DETOUR_DEPTH.try_with(|cell| cell.set(cell.get().saturating_sub(1)));
    }
}

/// RAII token for the ntdll `NtCreateFile` diagnostic detour. Suppresses disk I/O exactly like the
/// Win32 token but adds no depth, because it is the layer beneath them rather than a peer.
pub struct SaveNtCreateDetourGuard {
    reentrant: bool,
    previously_active: bool,
}

impl SaveNtCreateDetourGuard {
    pub fn enter() -> Self {
        let reentrant = !save_detour_disk_io_allowed();
        let previously_active = SAVE_NTCREATE_DETOUR_ACTIVE
            .try_with(|cell| cell.replace(true))
            .unwrap_or(true);
        Self {
            reentrant,
            previously_active,
        }
    }

    /// True when this entry is the ntdll leg of an open a detour above already handled (or of our
    /// own I/O), rather than a genuine ntdll-only open that bypassed Win32.
    pub fn is_reentrant(&self) -> bool {
        self.reentrant
    }
}

impl Drop for SaveNtCreateDetourGuard {
    fn drop(&mut self) {
        if self.previously_active {
            return;
        }
        let _ = SAVE_NTCREATE_DETOUR_ACTIVE.try_with(|cell| cell.set(false));
    }
}

/// False while this thread is inside ANY save-redirect detour, Win32 or ntdll.
///
/// Every helper that touches the disk on behalf of a detour checks this before opening anything.
/// The detour-level tokens above already refuse nested entries, so this is the second line: it
/// keeps the hazard closed for a new caller added to a detour body later.
pub fn save_detour_disk_io_allowed() -> bool {
    let win32_clear = SAVE_DETOUR_DEPTH
        .try_with(|cell| cell.get() == 0)
        .unwrap_or(false);
    let ntdll_clear = SAVE_NTCREATE_DETOUR_ACTIVE
        .try_with(|cell| !cell.get())
        .unwrap_or(false);
    win32_clear && ntdll_clear
}

#[cfg(test)]
mod save_detour_reentry_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// Models the production loop exactly: a detour body whose own file I/O re-enters the same
    /// detour. The recursion is unbounded unless the nested entry short-circuits, so deleting the
    /// `is_reentrant()` early return makes this blow its bound (and then the stack) instead of
    /// passing -- which is precisely what shipped until 2026-07-30.
    #[test]
    fn reentrant_detour_entry_does_not_recurse() {
        fn detour(entries: &std::cell::Cell<usize>) {
            let depth = SaveDetourDepth::enter();
            entries.set(entries.get() + 1);
            assert!(
                entries.get() <= 2,
                "save-redirect detour recursed: {} entries for one outer call",
                entries.get()
            );
            if depth.is_reentrant() {
                // Pass-through: call the original API and do no side-effect work.
                return;
            }
            // Stands in for `fs::read(configured save)` -> CreateFileW -> this same detour.
            detour(entries);
        }

        let entries = std::cell::Cell::new(0);
        detour(&entries);
        assert_eq!(entries.get(), 2, "expected exactly one nested pass-through");
        assert_eq!(
            SAVE_DETOUR_DEPTH.with(std::cell::Cell::get),
            0,
            "depth token leaked"
        );
        assert!(
            SAVE_REDIRECT_DETOUR_MAX_DEPTH.load(Ordering::SeqCst) >= 2,
            "max-depth oracle did not record the nested entry"
        );
    }

    /// The ntdll leg fires under every Win32 open, so counting it as depth would put a HEALTHY
    /// open at 2 and a healthy normalize-triggering open at 3 -- and the `> 2` alarm would fire on
    /// a working game. It must suppress disk I/O without adding depth.
    #[test]
    fn the_ntdll_leg_suppresses_io_without_inflating_the_depth() {
        let win32 = SaveDetourDepth::enter();
        assert!(!win32.is_reentrant());
        let ntdll = SaveNtCreateDetourGuard::enter();
        assert!(
            ntdll.is_reentrant(),
            "the ntdll leg of an already-detoured open is nested"
        );
        assert!(!save_detour_disk_io_allowed());
        assert_eq!(
            SAVE_DETOUR_DEPTH.with(std::cell::Cell::get),
            1,
            "the ntdll leg must not count toward the reported depth"
        );
        drop(ntdll);
        drop(win32);
        assert!(save_detour_disk_io_allowed());
    }

    /// A genuine ntdll-only open (one that bypassed Win32) is NOT nested and does its work -- but
    /// the `fs::read` that work performs re-enters through Win32, and that entry must pass through.
    #[test]
    fn a_win32_entry_under_the_ntdll_leg_is_reentrant() {
        let ntdll = SaveNtCreateDetourGuard::enter();
        assert!(!ntdll.is_reentrant(), "an ntdll-only open is the outermost");
        assert!(!save_detour_disk_io_allowed());
        let win32 = SaveDetourDepth::enter();
        assert!(
            win32.is_reentrant(),
            "our own fs::read under the ntdll detour must pass through"
        );
        drop(win32);
        drop(ntdll);
        assert!(save_detour_disk_io_allowed());
    }

    #[test]
    fn disk_io_is_refused_while_inside_a_detour() {
        assert!(save_detour_disk_io_allowed());
        {
            let _outer = SaveDetourDepth::enter();
            assert!(!save_detour_disk_io_allowed());
            let _inner = SaveDetourDepth::enter();
            assert!(!save_detour_disk_io_allowed());
        }
        assert!(save_detour_disk_io_allowed());
    }
}
