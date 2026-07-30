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
// Depth is counted even for the nested entry that gets refused, so
// `oracle_save_redirect_createfilew_max_depth` reads the real nesting: 1 in a run where no detour
// ever did its own I/O, 2 when one re-entered once (the expected steady state), and anything above
// 2 means a pass-through decision was lost and this bug class is back.

pub(crate) use er_telemetry::counters::SAVE_REDIRECT_DETOUR_MAX_DEPTH;
pub(crate) use er_telemetry::counters::SAVE_REDIRECT_DETOUR_REENTRANT_PASSTHROUGHS;

/// Depth value used when the thread-local cannot be reached at all (thread teardown). An
/// unanswerable "am I nested?" counts as nested: refusing one observation costs a diagnostic,
/// guessing wrong costs the process.
const SAVE_DETOUR_DEPTH_UNKNOWN: usize = usize::MAX;

std::thread_local! {
    /// How many save-redirect detours THIS thread is currently inside.
    static SAVE_DETOUR_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// RAII depth token for a save-redirect file detour. Take one at the TOP of the detour body and
/// hold it for the whole call; `is_reentrant()` then says whether this entry is nested inside
/// another save-redirect detour on the same thread.
///
/// A re-entrant entry must degrade to a pure pass-through: call the original API with the caller's
/// own arguments and skip every side effect (redirect decision, observation, staging, normalize,
/// diagnostics). Our own nested I/O addresses real paths we computed ourselves, so it wants the
/// unmodified API and none of the bookkeeping.
pub(crate) struct SaveDetourDepth {
    depth: usize,
}

impl SaveDetourDepth {
    pub(crate) fn enter() -> Self {
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
        if depth > 1 {
            SAVE_REDIRECT_DETOUR_REENTRANT_PASSTHROUGHS.fetch_add(1, Ordering::SeqCst);
        }
        Self { depth }
    }

    /// True when this detour entry is nested inside another save-redirect detour on this thread.
    pub(crate) fn is_reentrant(&self) -> bool {
        self.depth > 1
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

/// False while this thread is inside a save-redirect detour.
///
/// Every helper that touches the disk on behalf of a detour checks this before opening anything.
/// The detour-level token above already refuses nested entries, so this is the second line: it
/// keeps the hazard closed for a new caller added to a detour body later, and for the ntdll
/// `NtCreateFile` diagnostic, which keeps its logging at any depth and only needs its disk-touching
/// side effects suppressed.
pub(crate) fn save_detour_disk_io_allowed() -> bool {
    SAVE_DETOUR_DEPTH
        .try_with(|cell| cell.get() == 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod save_detour_reentry_tests {
    use super::*;

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

    /// The production one-shot must refuse to touch the disk from inside a detour. With no
    /// configured save file it normally reaches the "no configured save file" one-shot log; held
    /// inside a depth token it must bail before even resolving the path, so that latch stays 0.
    #[test]
    fn normalize_one_shot_bails_before_resolving_a_path_inside_a_detour() {
        if configured_save_file().is_some() {
            // A save file is configured in this environment, so the one-shot would take the
            // read-the-file arm and the latch this test reads is not the observable. Skip rather
            // than assert against a different code path.
            return;
        }
        let prior_steam_id = OBSERVED_ACTIVE_STEAM_ID64.load(Ordering::SeqCst);
        let prior_done = SAVE_STEAM_ID_ENV_NORMALIZE_DONE.load(Ordering::SeqCst);
        let prior_logged = SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.load(Ordering::SeqCst);
        // The preconditions the one-shot needs before it does anything at all.
        OBSERVED_ACTIVE_STEAM_ID64.store(76_561_197_960_265_729, Ordering::SeqCst);
        SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(0, Ordering::SeqCst);
        SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.store(0, Ordering::SeqCst);

        {
            let _inside_detour = SaveDetourDepth::enter();
            normalize_env_save_file_to_active_steam_id_once(0, "unit-test-reentrant");
            assert_eq!(
                SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.load(Ordering::SeqCst),
                0,
                "the one-shot ran its body from inside a save-redirect detour"
            );
        }
        normalize_env_save_file_to_active_steam_id_once(0, "unit-test-outside-detour");
        assert_eq!(
            SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.load(Ordering::SeqCst),
            1,
            "the one-shot did not run its body outside a detour"
        );

        OBSERVED_ACTIVE_STEAM_ID64.store(prior_steam_id, Ordering::SeqCst);
        SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(prior_done, Ordering::SeqCst);
        SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.store(prior_logged, Ordering::SeqCst);
    }
}
