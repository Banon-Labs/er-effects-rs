//! Make a PICKED save's `CS::ProfileSummary` real at the title, so the native Continue row can
//! load it the same way it loads the default save.
//!
//! # Why a picked save had no summary
//!
//! The records the Load-Character rows and `profile_slot_fingerprint` read come from the save
//! container's own `USER_DATA010` table, parsed by `CS::ProfileSummary::Deserialize`
//! (`0x140261f00` -> `0x140261f10`: ten occupancy bytes into `summary+0x8`, then ten records of
//! `0x2a0` from `summary+0x18` via `0x140261cf0`). The game runs that exactly once, inside the boot
//! common-data load (`0x1402570c0`, reached from the save-data `ShowProgressJob`'s delegate).
//!
//! At a BOOT picker that read comes free: the DLL holds the save-data job in `Continue` until the
//! user picks, then passes it through, and the delegate reads the redirected container. A LATE pick
//! -- the boot check accepted a save, the autoload could not load it, and the picker armed
//! afterwards -- has no such job left to run, so the summary keeps describing the save that was
//! rejected, or nothing at all.
//!
//! # What this does instead
//!
//! `MarkProfileIndexAsUsed` (`0x140262250`) is NOT the answer and was checked before this was
//! written: its entire body is `if (slot < 10) { saveSlotsStates[slot] = true; return true; }`. It
//! sets an occupancy FLAG and touches no record field, so a slot it marks still fingerprints empty
//! and the native Continue row still has nothing to load.
//!
//! Rather than re-enter the boot job (an async container read plus a full common-data parse that
//! would also re-deserialize GameSettings, the key config and the net-penalty state at the title),
//! this rebuilds the records directly from the staged container's own bytes, through the SAME
//! writer the System>Quit foreign-save preview already uses -- name, level, play time, rune memory,
//! map, `PlaceName`, `FaceData` and `ChrAsm` per slot, occupancy included. It builds them from each
//! slot's BODY, which is what deserializes into the character.
//!
//! # The records and the bodies do NOT "agree by construction"
//!
//! This doc used to claim they did, on the grounds that both come from the staged container. A
//! container holds two independent descriptions of a slot -- the `USER_DATA010` table the game's
//! own read deserializes, and the body -- and they can disagree; measured 2026-09-03 on
//! `100-Lilbro/ER0000.co2`, 6 of 10 slots did, including the one that was loading (`slot 4 body
//! 'Hero' lvl 7` vs `USER_DATA010 'Vagabond' lvl 9`). Two things follow, and both are implemented
//! below rather than assumed:
//!
//! * a record that fingerprints REAL is not evidence that the summary is CORRECT, so the native
//!   fast path verifies against the body instead of accepting realness as final;
//! * our rewrite is not necessarily the last write. The game's boot ProfileSummary read landed
//!   617ms after ours in run br-20260903-204517-82d2 and replaced every record, and the loading
//!   screen then showed `Vagabond`'s face, name and level under `Hero`'s load. See
//!   [`crate::reassert_policy`] for the bounded drift watch that answers it (bd er-effects-rs-ccud).
//!
//! It deliberately does NOT call the profile-renderer refresh: that AVs when the renderer table has
//! not been built yet (`0x9aa6d4`, observed), and at the boot title it has not.

use core::sync::atomic::Ordering;

use er_game_base::mem::game_module_base;
use er_game_base::profile_summary::PROFILE_SUMMARY_TOTAL_BYTES;

use crate::host::{
    active_save_file_for_system_quit, append_autoload_debug, direct_save_file_source_active,
    load_profile_slot_caches_from_bytes, native_fullread_slot,
};
use crate::live_records::{
    profile_slot_fingerprint, record_identity, system_quit_profile_summary_ptr,
};
use crate::reassert_policy::{
    REASSERT_MAX_REWRITES, ReassertStep, RecordIdentity, name_hash_utf16, reassert_step,
};
use crate::refresh_policy::{RefreshStep, refresh_step};
use crate::save_bytes_records::write_profile_summary_records_from_save_bytes;

/// A null game pointer -- the product's `TITLE_OWNER_SCAN_START_ADDRESS`, which is `usize::MIN`.
const NULL_SUMMARY: usize = usize::MIN;

/// [`PICKED_SUMMARY_REFRESH_STATE`]: the game's own boot save-data read already populated it.
pub const PICKED_SUMMARY_STATE_NATIVE_READ: usize = 1;
/// [`PICKED_SUMMARY_REFRESH_STATE`]: this DLL re-read the staged container at the title.
pub const PICKED_SUMMARY_STATE_REREAD: usize = 2;

pub use er_telemetry_core::counters::PICKED_SUMMARY_BODY_LEVEL;
pub use er_telemetry_core::counters::PICKED_SUMMARY_BODY_NAME_HASH;
pub use er_telemetry_core::counters::PICKED_SUMMARY_REASSERTS;
pub use er_telemetry_core::counters::PICKED_SUMMARY_RECORD_DRIFTS;
pub use er_telemetry_core::counters::PICKED_SUMMARY_REFRESH_ATTEMPTS;
pub use er_telemetry_core::counters::PICKED_SUMMARY_REFRESH_SLOT_MASK;
pub use er_telemetry_core::counters::PICKED_SUMMARY_REFRESH_STATE;
pub use er_telemetry_core::counters::PICKED_SUMMARY_REFRESH_TICKS;
pub use er_telemetry_core::counters::PICKED_SUMMARY_WATCH_ARMED_TICK;

/// Does the slot a direct-file (picked / loose `save_file`) source will load hold a REAL character
/// in the live `CS::ProfileSummary`?
///
/// This is [`profile_slot_fingerprint`] -- level >= 1 and a non-empty name in the RECORD -- and
/// deliberately not `saveSlotsStates[slot]`, which says only that something once marked the slot.
/// The slot is resolved with [`native_fullread_slot`] so the predicate and the full-read fallback
/// can never disagree about which slot is being talked about.
pub fn direct_source_slot_summary_real() -> bool {
    if !direct_save_file_source_active() {
        return false;
    }
    unsafe { profile_slot_fingerprint(native_fullread_slot()).0 }
}

/// Does the slot the BOOT autoload will load fingerprint as a real character in the live
/// `CS::ProfileSummary`?
///
/// [`direct_source_slot_summary_real`] answers this only for a picked/loose source; the default
/// save needs the same answer, because on this build the native Continue row cannot be identified
/// at all (its docall names `CS::BackScreen` and its accept predicate is the global menu-manager
/// busy check), so "is the record real" is what decides whether the verified full-read chain may
/// run instead of waiting for a row that will never arrive.
#[must_use]
pub fn boot_slot_summary_real() -> bool {
    unsafe { profile_slot_fingerprint(native_fullread_slot()).0 }
}

/// Re-read the picked container's `CS::ProfileSummary` records at the title. Idempotent and
/// self-throttling; safe to call from every autoload tick.
///
/// Returns true once the summary describes the picked container -- whether the game's own boot read
/// did it or this did.
pub fn refresh_direct_source_profile_summary() -> bool {
    if !direct_save_file_source_active() {
        return false;
    }
    refresh_active_container_profile_summary()
}

/// The same re-read for the DEFAULT boot container -- the save the game opens when nothing was
/// picked and no loose `save_file` is configured.
///
/// The game fills `CS::ProfileSummary` exactly ONCE per boot, in `CS::ProfileSummary::Deserialize`
/// reached from the save-data `ShowProgressJob`'s delegate. When that read completes with no data
/// the summary stays zeroed for the rest of the boot and nothing retries it: measured run
/// 2026-09-05 20:58:51, the wait step `0x140af2d40` polled `0x140679fd0` four times, got `1` then
/// `0` (`FUN_140e6fe80`'s "completed, result code 0" answer, not the `3` that fills), advanced on
/// the `0` without calling `GetProfileSummary`, and every one of the ten records stayed empty --
/// `oracle_profile_own_summary_rows = 0`, `stats-text: DECLINED slot 0 ... name="" level=0`.
/// The container itself was fine: 28,967,888 bytes of BND4 with all ten characters readable.
///
/// So this is the same repair the picked path already ships, pointed at the same container the
/// runtime opens. It is NOT a second filler racing the native one: `attempt_profile_summary_reread`
/// compares the live record against the container BODY first and skips the rewrite entirely when
/// they agree, so a boot whose native read worked is left untouched, and the drift watch still
/// defends the records if the native read lands afterwards and disagrees.
pub fn refresh_boot_default_profile_summary() -> bool {
    if direct_save_file_source_active() {
        // The picked/loose-file path owns its own entry above, including which container is
        // authoritative for it. Running both would double the attempt budget for one boot.
        return false;
    }
    // SPEND NO BUDGET BEFORE THE TABLE EXISTS. This entry is called from the boot game task, which
    // starts ticking long before `GameDataMan -> ProfileSummary` is allocated -- and every tick
    // through the throttle costs one of the forty attempts whether or not there was anything to
    // read. Eleven seconds of pre-summary boot at 60 fps is ~22 of them. The check is one guarded
    // pointer read, and it makes the first REAL attempt attempt 1.
    if unsafe { system_quit_profile_summary_ptr() } == NULL_SUMMARY {
        return false;
    }
    refresh_active_container_profile_summary()
}

/// Shared body of both entries: throttle, then one re-read attempt, then watch for drift.
fn refresh_active_container_profile_summary() -> bool {
    let state = PICKED_SUMMARY_REFRESH_STATE.load(Ordering::SeqCst);
    if state != 0 {
        // NOT DONE -- WATCHING. The refresh used to return here and never look again, which is
        // what let the game's own boot ProfileSummary read overwrite our body-derived records
        // 617ms later and put another character's face and stats on the loading screen (run
        // br-20260903-204517-82d2, bd er-effects-rs-ccud).
        unsafe { watch_target_record_for_drift() };
        return true;
    }
    // The old fast path accepted `direct_source_slot_summary_real()` as final: a record that
    // fingerprints REAL was taken to mean the game's own read had populated the summary correctly.
    // It means no such thing. `USER_DATA010` -- the table that read deserializes -- and the slot
    // BODY that actually loads are two independent descriptions of a slot, and a container can
    // disagree with itself (measured: 6 of 10 slots). So the container is read either way, and the
    // rewrite is skipped only once the BODY and the record have been compared and agree.
    let tick = PICKED_SUMMARY_REFRESH_TICKS.fetch_add(1, Ordering::SeqCst);
    let attempts = PICKED_SUMMARY_REFRESH_ATTEMPTS.load(Ordering::SeqCst);
    let RefreshStep::Attempt(attempt) = refresh_step(tick, attempts) else {
        return false;
    };
    PICKED_SUMMARY_REFRESH_ATTEMPTS.store(attempt, Ordering::SeqCst);
    unsafe { attempt_profile_summary_reread(attempt) }
}

/// One re-read attempt. Every early return logs why, because a silent failure here is what puts the
/// deserialize back at the title.
unsafe fn attempt_profile_summary_reread(attempt: usize) -> bool {
    const NULL: usize = NULL_SUMMARY;
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!(
            "picked-summary: attempt {attempt} deferred -- game module base unresolved"
        ));
        return false;
    };
    let summary = unsafe { system_quit_profile_summary_ptr() };
    if summary == NULL {
        append_autoload_debug(format_args!(
            "picked-summary: attempt {attempt} deferred -- GameDataMan -> ProfileSummary not allocated yet"
        ));
        return false;
    }
    // The STAGED native save, never the user's source file: the staged copy is what the game's own
    // reads resolve to, so the records this builds and the bodies the game will deserialize come
    // from the same container. `active_save_file_for_system_quit` returns exactly that for a direct
    // source (it never hands back `SAVE_DIRECT_SOURCE_FILE`, which is read-only).
    let Some(path) = active_save_file_for_system_quit() else {
        append_autoload_debug(format_args!(
            "picked-summary: attempt {attempt} deferred -- no staged active save path resolvable yet"
        ));
        return false;
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => {
            append_autoload_debug(format_args!(
                "picked-summary: attempt {attempt} FAILED -- staged container '{}' unreadable: {err}",
                path.display()
            ));
            return false;
        }
    };
    if er_save_loader::bnd4::parse_entries(&bytes).is_err() {
        append_autoload_debug(format_args!(
            "picked-summary: attempt {attempt} FAILED -- staged container '{}' ({} bytes) is not a readable BND4",
            path.display(),
            bytes.len()
        ));
        return false;
    }
    let slot = native_fullread_slot();
    // THE IDENTITY THE BODY GIVES THIS SLOT -- captured before anything is written, and kept, because
    // it is what the drift watch defends for the rest of the boot. The body is the truth about what
    // will load; `USER_DATA010` is only what the game's own summary read believes.
    let body = body_identity(&bytes, slot);
    arm_record_watch(body);
    if let Some(body) = body {
        let live = unsafe { record_identity(slot) };
        if live == body {
            // The game's own boot read already agrees with the body, so there is nothing to correct
            // and the destructive ten-record rewrite is skipped. The caches are still refilled from
            // these bytes -- the rows and the loading-screen stats line read those, not the records.
            let decoded = load_profile_slot_caches_from_bytes(&bytes, "autoload picked save");
            PICKED_SUMMARY_REFRESH_STATE.store(PICKED_SUMMARY_STATE_NATIVE_READ, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "picked-summary: live ProfileSummary AGREES with the container body for target slot {slot} (level={} name_hash=0x{:016x}) -- native Continue row is usable, no rewrite needed; caches={decoded}/10",
                body.level, body.name_hash
            ));
            return true;
        }
        append_autoload_debug(format_args!(
            "picked-summary: attempt {attempt} -- target slot {slot} record (level={} name_hash=0x{:016x}) DISAGREES with the container body (level={} name_hash=0x{:016x}); rewriting the records from the bodies",
            live.level, live.name_hash, body.level, body.name_hash
        ));
    }
    let Some((mask, decoded)) = (unsafe { rewrite_records_from_bytes(base, summary, &bytes) })
    else {
        append_autoload_debug(format_args!(
            "picked-summary: attempt {attempt} FAILED -- staged container '{}' has no readable character slots; the native Continue row stays unusable",
            path.display()
        ));
        return false;
    };
    PICKED_SUMMARY_REFRESH_SLOT_MASK.store(mask, Ordering::SeqCst);
    let (real, map, level, name_len) = unsafe { profile_slot_fingerprint(slot) };
    if !real {
        append_autoload_debug(format_args!(
            "picked-summary: attempt {attempt} rewrote slots mask=0x{mask:x} from '{}' but target slot {slot} still fingerprints empty (map=0x{map:x} level={level} name_len={name_len}) -- the picked slot is not in this container",
            path.display()
        ));
        return false;
    }
    PICKED_SUMMARY_REFRESH_STATE.store(PICKED_SUMMARY_STATE_REREAD, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "picked-summary: *** RE-READ the picked container's ProfileSummary at the title *** path='{}' attempt={attempt} slots=0x{mask:x} caches={decoded}/10; target slot {slot} now map=0x{map:x} level={level} name_len={name_len} -- the picked save takes the NATIVE Continue row from here, so no deserialize runs at the title",
        path.display()
    ));
    true
}

/// The name+level a container's slot BODY gives, or `None` when the slot holds no readable
/// character. This is the same pair `record_identity` reads out of the live record, hashed the same
/// way, so the two are directly comparable.
fn body_identity(bytes: &[u8], slot: i32) -> Option<RecordIdentity> {
    let slot = usize::try_from(slot).ok()?;
    let name = er_save_loader::stats::all_slot_names(bytes)
        .get(slot)?
        .clone()?;
    let level = er_save_loader::stats::all_slot_stats(bytes)
        .get(slot)?
        .as_ref()?
        .level;
    let level = u32::try_from(level).ok()?;
    let identity = RecordIdentity {
        name_hash: name_hash_utf16(&name.encode_utf16().collect::<Vec<u16>>()),
        level,
    };
    identity.is_character().then_some(identity)
}

/// Publish the identity the drift watch defends, and arm the watch at the current tick.
fn arm_record_watch(body: Option<RecordIdentity>) {
    let body = body.unwrap_or_default();
    PICKED_SUMMARY_BODY_NAME_HASH.store(body.name_hash, Ordering::SeqCst);
    PICKED_SUMMARY_BODY_LEVEL.store(body.level as usize, Ordering::SeqCst);
    // +1 so that "armed" is distinguishable from tick 0, which is a real tick.
    PICKED_SUMMARY_WATCH_ARMED_TICK.store(
        PICKED_SUMMARY_REFRESH_TICKS.load(Ordering::SeqCst) + 1,
        Ordering::SeqCst,
    );
}

/// The identity the watch is defending, or `None` while nothing has read a container.
fn watched_body_identity() -> Option<RecordIdentity> {
    let body = RecordIdentity {
        name_hash: PICKED_SUMMARY_BODY_NAME_HASH.load(Ordering::SeqCst),
        level: PICKED_SUMMARY_BODY_LEVEL.load(Ordering::SeqCst) as u32,
    };
    body.is_character().then_some(body)
}

/// Rewrite all ten records from a container's bodies. Returns `(slot mask, caches decoded)`, or
/// `None` when the container yielded no writable slot.
///
/// # Safety
///
/// `summary` must be the LIVE `CS::ProfileSummary` allocation and `base` the running game module
/// base: this zeroes and rewrites ten records through raw pointers. Game thread only.
unsafe fn rewrite_records_from_bytes(
    base: usize,
    summary: usize,
    bytes: &[u8],
) -> Option<(usize, usize)> {
    // Captured BEFORE the rewrite: the writer uses it as a structural template for any slot whose
    // visual blocks it cannot locate in the container.
    let snapshot =
        unsafe { core::slice::from_raw_parts(summary as *const u8, PROFILE_SUMMARY_TOTAL_BYTES) }
            .to_vec();
    let (mask, _stats) =
        unsafe { write_profile_summary_records_from_save_bytes(base, summary, &snapshot, bytes) };
    if mask == 0 {
        return None;
    }
    // The rows and the loading-screen stats line read these caches, not the records, so a rebuilt
    // summary with stale caches shows the picked character's level under the previous save's name.
    let decoded = load_profile_slot_caches_from_bytes(bytes, "autoload picked save");
    Some((mask, decoded))
}

/// Has something overwritten the target slot's record with a different character since the refresh?
///
/// Called on every autoload tick once the refresh has resolved. The check itself is two guarded
/// reads and a hash of at most seventeen UTF-16 units -- cheap enough per frame; the CORRECTION is
/// the expensive part and is capped by `REASSERT_MAX_REWRITES`.
///
/// # Safety
///
/// No precondition: every game read is fault-guarded and the rewrite re-resolves the summary
/// pointer itself. Game thread only, like everything else that writes these records.
unsafe fn watch_target_record_for_drift() {
    let tick = PICKED_SUMMARY_REFRESH_TICKS.fetch_add(1, Ordering::SeqCst);
    let armed = PICKED_SUMMARY_WATCH_ARMED_TICK.load(Ordering::SeqCst);
    if armed == 0 {
        return;
    }
    let slot = native_fullread_slot();
    let live = unsafe { record_identity(slot) };
    let rewrites = PICKED_SUMMARY_REASSERTS.load(Ordering::SeqCst);
    match reassert_step(
        watched_body_identity(),
        live,
        rewrites,
        tick.saturating_sub(armed - 1),
    ) {
        ReassertStep::Hold => {}
        ReassertStep::Exhausted => {
            if PICKED_SUMMARY_REASSERTS.swap(REASSERT_MAX_REWRITES + 1, Ordering::SeqCst)
                <= REASSERT_MAX_REWRITES
            {
                append_autoload_debug(format_args!(
                    "picked-summary: record drift on slot {slot} persists after {REASSERT_MAX_REWRITES} re-asserts -- giving up; the loading-screen portrait and stats will describe record identity (level={} name_hash=0x{:016x}), not the body that loads",
                    live.level, live.name_hash
                ));
            }
        }
        ReassertStep::Rewrite(n) => {
            let drifts = PICKED_SUMMARY_RECORD_DRIFTS.fetch_add(1, Ordering::SeqCst) + 1;
            PICKED_SUMMARY_REASSERTS.store(n, Ordering::SeqCst);
            unsafe { reassert_body_records(slot, live, n, drifts) };
        }
    }
}

/// Re-read the staged container and rewrite the records from its bodies, because something replaced
/// them. Logged loudly every time: a silent correction here would hide the writer that caused it.
///
/// # Safety
///
/// Game thread only; the summary pointer is re-resolved and null-checked here rather than trusted
/// from the earlier refresh, because the allocation can be freed and replaced between them.
unsafe fn reassert_body_records(slot: i32, live: RecordIdentity, n: usize, drifts: usize) {
    let Ok(base) = game_module_base() else {
        return;
    };
    let summary = unsafe { system_quit_profile_summary_ptr() };
    if summary == NULL_SUMMARY {
        return;
    }
    let Some(path) = active_save_file_for_system_quit() else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        append_autoload_debug(format_args!(
            "picked-summary: RE-ASSERT #{n} aborted -- staged container '{}' unreadable",
            path.display()
        ));
        return;
    };
    let Some((mask, decoded)) = (unsafe { rewrite_records_from_bytes(base, summary, &bytes) })
    else {
        append_autoload_debug(format_args!(
            "picked-summary: RE-ASSERT #{n} aborted -- staged container '{}' has no readable character slots",
            path.display()
        ));
        return;
    };
    PICKED_SUMMARY_REFRESH_SLOT_MASK.store(mask, Ordering::SeqCst);
    let now = unsafe { record_identity(slot) };
    append_autoload_debug(format_args!(
        "picked-summary: *** RECORD DRIFT #{drifts} -- RE-ASSERTED the container's bodies *** slot {slot} had been overwritten with level={} name_hash=0x{:016x}; the body says level={} name_hash=0x{:016x}; rewrite #{n} restored slots mask=0x{mask:x} caches={decoded}/10 -> record now level={} name_hash=0x{:016x}. Something else writes these records after we do (the game's own boot ProfileSummary read is the known one) and the loading-screen portrait + stats read whatever is there.",
        live.level,
        live.name_hash,
        watched_body_identity().unwrap_or_default().level,
        watched_body_identity().unwrap_or_default().name_hash,
        now.level,
        now.name_hash
    ));
}
