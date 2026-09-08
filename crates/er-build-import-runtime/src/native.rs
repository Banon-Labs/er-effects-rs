//! Resolving the game functions this crate calls, for the build that is actually running.
//!
//! # Why every call in this crate goes through here
//!
//! Every address in this crate is a 1.16.2 RVA, and the installed game has been 1.17 since
//! 2026-08-27. The old shape was `transmute(module_base + <the RVA>)` under the comment "verified
//! 1.16.2 RVAs within the loaded image" -- a claim that was true when written and became a
//! *stale* claim the day the game patched. There were 38 of them, in four cdylibs including the
//! product DLL, and a call through one is the worst failure mode available: it transfers control
//! into whatever now occupies those bytes, which on a patched build is routinely the middle of an
//! unrelated function -- an execute fault with no unwind information and no exception record
//! naming anything of ours.
//!
//! [`er_game_base::game_build::resolve_game_address`] answers where an address went, or refuses.
//! A refusal costs a feature; using the stale address costs the process. So a refusal here means
//! the caller does not make that call and says so.
//!
//! # Why the log is bounded
//!
//! `resolve_game_address` writes one line per refusal into the shared address log, and this crate
//! adds one line of its own naming the feature that just went inert. That second line is emitted
//! once per name for the life of the process. The bound is not decoration: a session once logged
//! **339,764** refusals of a single address because an unbounded refusal sat inside a recurring
//! task, and the log became unreadable rather than merely long. This crate's callers are one-shot
//! (`tick` claims its phase before it runs), so the natural volume is already small -- the guard
//! is what keeps it small if a future caller is not.
//!
//! # What this is not for
//!
//! An address the running process discovered for itself -- an AOB scan hit, a vtable read, a
//! MinHook trampoline, a captured return address -- is already on the running build and must
//! never be translated. Feeding one to this helper would refuse it (it is not a 1.16.2 source in
//! the table) and cost the feature, or worse, translate it a second time. Nothing in this crate
//! discovers an address that way; every address here is a compile-time 1.16.2 constant.

use std::sync::Mutex;

/// Names already reported inert, so the log line below is emitted once per name.
///
/// A `Vec` rather than a set because it holds at most the ~40 distinct names this crate resolves,
/// and a linear scan of forty `&'static str` pointers is not worth a hash table.
static REPORTED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Where `module_base + rva` lives on the running build, or `None` when nothing knows.
///
/// `what` names the game function, and is what a reader of the log has to go on when a feature
/// goes quiet -- so it should be the engine's own name for it, not the call site's.
pub fn resolve(module_base: usize, rva: usize, what: &'static str) -> Option<usize> {
    match er_game_base::game_build::resolve_game_address(module_base + rva, what) {
        Some(address) => Some(address),
        None => {
            report_once(what);
            None
        }
    }
}

/// Log that `what` has no mapping on this build -- the first time, and only the first time.
///
/// A poisoned lock is treated as "already reported": losing a log line is strictly better than
/// panicking inside a game-loaded DLL to announce that something else went wrong.
fn report_once(what: &'static str) {
    let Ok(mut reported) = REPORTED.lock() else {
        return;
    };
    if reported.contains(&what) {
        return;
    }
    reported.push(what);
    crate::log_line(&format!(
        "[build-import] NATIVE UNAVAILABLE: `{what}` has no verified mapping for the running \
         build, so every call to it is refused. Whatever depends on it is inert for this session; \
         see the ADDRESS REFUSED line in the address log for the address that was asked for."
    ));
}

/// Resolve a whole group of natives, reporting every name that has no mapping rather than the
/// first.
///
/// The equip pass needs ten functions at once. Failing on the first would send a reader to fix one
/// address, rebuild, and discover the next -- so the group answers with the complete list of what
/// is missing, which is what a `docs/recon` row-proposal actually needs.
pub fn resolve_all<const N: usize>(
    module_base: usize,
    wanted: [(usize, &'static str); N],
) -> Result<[usize; N], Vec<&'static str>> {
    let mut out = [0usize; N];
    let mut missing = Vec::new();
    for (index, (rva, what)) in wanted.into_iter().enumerate() {
        match resolve(module_base, rva, what) {
            Some(address) => out[index] = address,
            None => missing.push(what),
        }
    }
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(missing)
    }
}

/// The natives already reported inert on this build, in the order they were first refused.
///
/// Exists so a run that applied nothing can name what was missing instead of printing zeroes.
/// Before 2026-08-30 the importer answered a fully-refused run with
/// `import complete: 0/0 items, 0/0 gear, 0/0 spells, RL0` and a telemetry block reading
/// `failed_count=0, refused_count=0` -- the counters were truthful about what was attempted and
/// silent about the fact that nothing could be, which is the most expensive way to be right.
/// A poisoned lock yields an empty list: the caller's message degrades to "no names available",
/// which is still a failure report.
pub fn inert() -> Vec<&'static str> {
    REPORTED
        .lock()
        .map(|names| names.clone())
        .unwrap_or_default()
}
