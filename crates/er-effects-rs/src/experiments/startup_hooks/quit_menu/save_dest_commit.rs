use super::*;

// Save-to-a-chosen-destination commit state (save-game-flow WP3).
//
// The System->Quit "Save Game" flow can end on a destination the user browsed to instead of the
// loaded save. Nothing about the native save is re-implemented: the game's own writer is diverted
// at the `CreateFileW` funnel every FromSoft file open uses.
//
// # What the native writer actually does (1.16.2 decompile, corrected 2026-07-28)
//
// The save job body `FUN_14240fd70` formats the container path (`L"%s\\%s%s"`) and then picks ONE
// OF TWO write paths ITSELF -- the `if`/`else` is its own code. (Attribution corrected 2026-07-29:
// this comment used to say the choice came "from `FUN_142413230`". That function is a THIRD,
// separate call the body makes first, and the branch tests its RESULT CODE, read back out of the
// job via `FUN_14240dbf0`/`FUN_14240d8d0`.) `FUN_142413230` mounts the container ALREADY ON DISK at
// that path and checks whether every block the request supplies still fits its existing entry,
// returning 6 when everything fits and 0 when it does not -- the inverse of the writers' own
// 0 = success convention:
//
//   * probe returns 0: no usable container / a block outgrew its entry -> `FUN_142413860`, the
//     FULL REBUILD: it rebuilds the whole image in memory and emits it as one `WriteBytes` from
//     offset 0.
//   * probe returns 6: every block fits (the steady state for a save over an existing container)
//     -> `FUN_1424142e0`, the PER-BLOCK IN-PLACE WRITER, called once per supplied block:
//     `OpenFile` -> `Seek(entry.dataOffset)` -> `WriteBytes(block)` -> optionally
//     `Seek(entryHeaderOffset)` + `WriteBytes(0x20)` -> `Seek(0, END)` -> `CloseStream`.
//
// Which branch a given save actually took is now measurable rather than inferred: see
// `oracle_save_write_full_rebuild_calls` / `oracle_save_write_in_place_calls`, the passive
// observers in `er-save-suppress`.
//
// The in-place writer NEVER writes the bytes it did not change, and
// `MicrosoftDiskFileOperator::OpenFile` opens write-mode handles with `OPEN_ALWAYS` (`dwCreation
// Disposition = 4`, `0x141fc13f0`) -- it creates a missing file and does NOT truncate. So diverting
// only the write-opens onto an EMPTY destination produced exactly what run 4 measured: a sparse
// file, zero from byte 0, ending at the highest written block (`USER_DATA010`'s end, 26,608,560 of
// the live container's 28,967,888), with no `BND4` magic. Catching the opens was never enough.
//
// # Why this seeds the destination
//
// The branch decision and every entry offset the in-place writer seeks to are read from the LIVE
// container (read-opens pass through untouched), so the destination must already BE that container
// for those offsets to mean anything. The redirect therefore writes a byte-exact copy of the live
// save to the destination BEFORE firing, and the native writer then patches its changed blocks into
// it. Both native paths land correctly on a seeded destination: the in-place writer patches blocks
// at valid offsets, and the full rebuild overwrites from 0 (its `Seek(0,END)`/close sets the length
// either way). Seeding is also the arming gate -- if the copy cannot be written the request is NOT
// fired, because a save that cannot land must never be reported as one.
//
// The window is armed at the fire gate and disarmed at completion (never one-shot: a writer retry
// must not be able to leak onto the live file). Read-opens pass through -- the read side IS the
// "current state" the user asked to write elsewhere -- and so does the native `.bak` `CopyFileW`,
// which is normal save behavior against a file we never write.
//
// Safety net: the live file's bytes/stat are snapshotted before the fire, and completion verifies
// (a) the destination is a STRUCTURALLY COMPLETE `BND4` container of the live save's size whose
// bytes differ from the seed, and (b) the live file did NOT change. A mutated live file is a hard
// failure oracle (`oracle_save_dest_live_file_mutated`): the snapshot is restored over it and the
// failure is logged and published.
//
// # The other destination: the loaded save itself
//
// A browsed pick that resolves back to the loaded save means the native
// writer rewrites the live container in place -- correct, sanctioned, and until now completely
// unnamed in telemetry, which is how run 4's 20:43:51 live-file rewrite read as an anonymous
// mutation. `save_dest_arm_live_overwrite` records that intent BEFORE the fire and verifies it
// afterwards, so every save this flow performs names the file it is about to rewrite.
//
// # The four rules that keep this from eating a save (2026-07-29)
//
// Each replaces a decision that used to be made on evidence too weak to carry it.
//
// 1. **"Is the destination the loaded save?" is a HANDLE question, never a string question.**
//    `save_dest_commit_identity` compares `BY_HANDLE_FILE_INFORMATION` (see
//    `save_dest_identity.rs`), because the same file is reachable as
//    `C:\users\steamuser\...\ER0000.sl2` and as
//    `Z:\...\pfx\drive_c\users\steamuser\...\ER0000.sl2`, and the destination browser really does
//    produce the second spelling. Answering "different" there seeds and redirects the loaded save
//    onto ITSELF: the write lands, the live stamp moves because it IS the destination, the safety
//    net calls that a leak, and it restores the pre-fire snapshot over the save that just
//    succeeded. Identity that cannot be established is `Unknown`, and `Unknown` refuses to fire.
//
// 2. **A write-open is matched by its FULL path.** The leaf alone (`er0000.sl2`) belongs to every
//    Steam account folder, every backup tool, every other mod and our own staged tree, and any of
//    them opening one during the armed window used to have its bytes rerouted into the user's
//    chosen destination. The match is now the normalized full path, its `.sl2`/`.co2` twin, or a
//    directory PROVEN to be the loaded save's by handle identity.
//
// 3. **Nothing is written until the commit is committed to firing, and the seed is all-or-
//    nothing.** The seed is ~29 MB over a file the user may have picked out of their own save
//    collection; `fs::write` truncates first, so a failure part way through leaves that file
//    unloadable -- and the old ordering could still abort AFTER seeding, reporting that the save
//    did not happen having already overwritten the destination. It is now a sibling temp file
//    plus a rename, run after the last abort gate.
//
// 4. **Teardown waits for the writer, and the restore waits for proof.** The in-place writer
//    opens the container once per dirty block, so a window closed on a tick count can close
//    between block k and k+1 and patch the remainder into the live save, after the leak check has
//    already run. Teardown is gated on `er_save_suppress::save_job_writer_idle`. And the restore
//    -- itself a whole-container write over the user's loaded save -- now requires positive
//    evidence of a CONTENT change: an unreadable stat is reported as unreadable, not as mutation.

pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_FAIL;
/// Flow latches: the menu-pump open request, and the "a destination is chosen, commit once the
/// picker has torn down" hand-off from the picker's activation hook to the save-flow tick.
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_PENDING;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_BAK_MUTATED;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_OVERWRITE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_OPEN_PICKER_PENDING;
/// 1 while the scoped write-open redirect is armed. Read by the `CreateFileW` detour BEFORE it
/// touches any lock, so an unarmed process pays one relaxed-ordering atomic load per open.
pub(crate) use er_telemetry::counters::SAVE_DEST_REDIRECT_ARMED;
pub(crate) use er_telemetry::counters::SAVE_DEST_REDIRECT_HITS;
pub(crate) use er_telemetry::counters::SAVE_DEST_SEED_FAIL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_SEEDED_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_STRUCTURE_OK;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_WRITTEN_OK;
// The destination-commit SAFETY oracles this file raises -- every refusal, deferral and
// undecidable fact -- are re-exported with the rest of the save-flow counters in
// `constants::autoload_state`, which is in scope here.

/// BND4 container magic: the first four bytes of every ER save the game writes.
pub(crate) const SAVE_DEST_BND4_MAGIC: [u8; 4] = *b"BND4";
/// `CreateFileW` desired-access bits that make an open a WRITE open (`GENERIC_WRITE`,
/// `FILE_WRITE_DATA`). Read-opens (the RMW base read) must pass through untouched.
#[allow(dead_code)] // Retained: Decoded CreateFileW access bits for the save-destination redirect; kept with the rest of that table.
pub(crate) const SAVE_DEST_WRITE_ACCESS_MASK: u32 = 0x4000_0000 | 0x2;
/// Save-container extensions: a destination whose leaf is the live save's counterpart twin
/// (`ER0000.co2` vs `ER0000.sl2`) is still the same open, whichever side rewrote the path first.
#[allow(dead_code)] // Retained: Save-container extension pair naming the counterpart-twin rule its doc describes.
pub(crate) const SAVE_DEST_SEAMLESS_EXTENSION: &str = "co2";
#[allow(dead_code)] // Retained: Save-container extension pair naming the counterpart-twin rule its doc describes.
pub(crate) const SAVE_DEST_VANILLA_EXTENSION: &str = "sl2";

/// The chosen destination for the save currently being committed. `None` = the loaded save is the
/// target (the plain overwrite path), which needs no redirect at all.
pub(crate) static SAVE_DEST_TARGET_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Live save + destination bookkeeping for one in-flight commit.
pub(crate) struct SaveDestRedirect {
    target_path: PathBuf,
    /// NUL-terminated Windows-form destination handed to the original `CreateFileW`.
    target_w: Vec<u16>,
    /// Did the destination already exist before the seed overwrote it? Reporting only.
    target_existed: bool,
    live_path: PathBuf,
    live_len: u64,
    live_modified_ns: u128,
    /// `(len, modified_ns)` of the live save's `.bak` twin before the fire. The native backup step
    /// (`FUN_142410830`, `CopyFileW` live -> live.bak) is not redirected, so this is how a commit
    /// that was supposed to leave the loaded save's folder alone reports that it did not.
    live_bak_before: Option<(u64, u128)>,
    /// Pre-fire bytes of the LIVE save. Doubles as the destination SEED (written to the target
    /// before the fire so the native in-place writer has a real container to patch) and as the
    /// snapshot restored if the redirect leaks and the loaded save is mutated anyway -- the user
    /// explicitly chose NOT to overwrite it.
    live_bytes: Vec<u8>,
    /// Accepted leaf names (ASCII-lowercased UTF-16): the live save's own leaf plus its
    /// `.sl2`/`.co2` counterpart twin. A cheap PREFILTER only -- matching on it alone is what
    /// let any process's `ER0000.sl2` write-open be rerouted into the user's destination.
    accepted_leaves: Vec<Vec<u16>>,
    /// Normalized full paths a write-open must equal to be the loaded save's container: the live
    /// path and its `.sl2`/`.co2` twin, in every accepted directory.
    accepted_paths: Vec<String>,
    /// Directories that resolve to the loaded save's folder, for the handle-identity fallback
    /// when an incoming path is spelled differently than any accepted path.
    accepted_dirs: Vec<PathBuf>,
}

pub(crate) static SAVE_DEST_REDIRECT: Mutex<Option<SaveDestRedirect>> = Mutex::new(None);

/// A commit whose destination IS the loaded save -- a browsed pick, or `[ new ]` in the loaded
/// save's own folder, that
/// resolves back to the loaded save. Nothing is redirected -- the native writer rewrites the live
/// container in place, which is exactly what the user asked for. Recorded anyway so the rewrite is
/// NAMED before it happens and scored after it, instead of surfacing later as an unattributed
/// change to the user's save file.
pub(crate) struct SaveDestLiveOverwrite {
    live_path: PathBuf,
    before: Option<(u64, u128)>,
    bak_before: Option<(u64, u128)>,
    reason: &'static str,
}

pub(crate) static SAVE_DEST_LIVE_OVERWRITE: Mutex<Option<SaveDestLiveOverwrite>> = Mutex::new(None);

/// Outcome of scoring one commit's file(s). Returned so the flow's FINAL log line can state the
/// file result rather than announcing the game's SL status and being contradicted a line later.
pub(crate) struct SaveDestVerdict {
    pub(crate) ok: bool,
    pub(crate) summary: String,
}

pub(crate) fn save_dest_target_lock() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    SAVE_DEST_TARGET_PATH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn save_dest_redirect_lock() -> std::sync::MutexGuard<'static, Option<SaveDestRedirect>>
{
    SAVE_DEST_REDIRECT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn save_dest_live_overwrite_lock()
-> std::sync::MutexGuard<'static, Option<SaveDestLiveOverwrite>> {
    SAVE_DEST_LIVE_OVERWRITE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Record the destination the user chose (picker activation / Box3 confirm).
pub(crate) fn save_dest_set_target(path: PathBuf, source: &str) {
    append_autoload_debug(format_args!(
        "save-dest: target set '{}' (source={source})",
        path.display()
    ));
    *save_dest_target_lock() = Some(path);
}

pub(crate) fn save_dest_target() -> Option<PathBuf> {
    save_dest_target_lock().clone()
}

/// Drop a chosen destination without ending the flow (Box3 answered No: the browser stays up).
pub(crate) fn save_dest_clear_target(reason: &str) {
    if let Some(previous) = save_dest_target_lock().take() {
        append_autoload_debug(format_args!(
            "save-dest: target cleared '{}' (reason={reason})",
            previous.display()
        ));
    }
}

/// Full teardown of the destination side of a save flow: target, commit/open latches, and any
/// still-armed redirect window. Called whenever the flow returns to IDLE.
pub(crate) fn save_dest_reset(reason: &str) {
    save_dest_clear_target(reason);
    SAVE_DEST_COMMIT_PENDING.store(0, Ordering::SeqCst);
    SAVE_DEST_OPEN_PICKER_PENDING.store(0, Ordering::SeqCst);
    if SAVE_DEST_REDIRECT_ARMED.load(Ordering::SeqCst) != 0 {
        // Should be impossible (the commit stage always verifies+disarms), so it is a failure
        // path: log on occurrence rather than silently dropping an armed redirect window.
        append_autoload_debug(format_args!(
            "save-dest: redirect was STILL ARMED at flow reset (reason={reason}) -- disarming; the destination write was never verified"
        ));
        let _ = save_dest_verify_and_disarm(reason);
    } else if save_dest_live_overwrite_lock().is_some() {
        // Same shape for the overwrite-the-loaded-save commit: a record left behind means the
        // rewrite this flow announced was never scored, so score it now rather than dropping it.
        let _ = save_dest_verify_and_disarm(reason);
    }
}

/// ASCII-lowercase leaf (file name) of a wide Windows path, or `None` when the path ends in a
/// separator / is empty.
pub(crate) fn save_dest_wide_leaf_lower(path: &[u16]) -> Option<Vec<u16>> {
    er_quit_menu::save_dest_commit::save_dest_wide_leaf_lower(path)
}

/// The live save's leaf plus its counterpart-extension twin, ASCII-lowercased UTF-16.
pub(crate) fn save_dest_accepted_leaves(live_path: &Path) -> Vec<Vec<u16>> {
    er_quit_menu::save_dest_commit::save_dest_accepted_leaves(live_path)
}

/// Every directory whose `ER0000.{sl2,co2}` write-open IS the loaded save's.
///
/// Normally exactly one: the live save's own folder. In staged / direct-file mode the loaded save
/// lives under the private staged root while the native writer still opens
/// `...\Roaming\EldenRing\<steamid>\ER0000.sl2` -- the general save redirect rewrites that open a
/// moment after this one declines it -- so that game-side folder counts too. Leaf-only matching
/// used to cover this case by accident; a full-path match has to name it.
pub(crate) fn save_dest_accepted_dirs(live_path: &Path) -> Vec<PathBuf> {
    er_quit_menu::save_dest_commit::save_dest_accepted_dirs_for(
        live_path,
        save_redirect_native_source_dir(),
    )
}

/// Normalized full paths that ARE the loaded save's container: every accepted leaf in every
/// accepted directory.
pub(crate) fn save_dest_accepted_paths(live_path: &Path) -> Vec<String> {
    er_quit_menu::save_dest_commit::save_dest_accepted_paths_for(
        live_path,
        save_redirect_native_source_dir(),
    )
}

pub(crate) fn save_dest_file_stamp(path: &Path) -> Option<(u64, u128)> {
    let meta = fs::metadata(path).ok()?;
    let modified_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    Some((meta.len(), modified_ns))
}

/// The `.bak` twin the native backup step (`FUN_142410830`) copies a saved container to.
pub(crate) fn save_dest_bak_path(path: &Path) -> PathBuf {
    er_quit_menu::save_dest_commit::save_dest_bak_path(path)
}

/// End offset of the last BND4 entry, i.e. the length a STRUCTURALLY COMPLETE container must have.
///
/// This is the check that would have caught run 4's garbage file for what it was: the sparse file
/// the in-place writer left had no `BND4` header at all, and even a container that parses is only
/// complete when its own index accounts for every byte up to EOF.
pub(crate) fn save_dest_container_end(bytes: &[u8]) -> Option<usize> {
    er_quit_menu::save_dest_commit::save_dest_container_end(bytes)
}

/// Record that this commit's destination IS the loaded save, and say so BEFORE the write happens.
///
/// The native writer is left completely alone here -- this is the overwrite the user confirmed.
/// The point is attribution: without this line a rewrite of `ER0000.sl2` (and, through the native
/// `.bak` copy, its backup) is indistinguishable in the log from a suppression leak or a staging
/// copy, which is exactly the ambiguity run 4's 20:43:51 live-file change created.
pub(crate) fn save_dest_arm_live_overwrite(live_path: &Path, reason: &'static str) {
    let before = save_dest_file_stamp(live_path);
    let bak_before = save_dest_file_stamp(&save_dest_bak_path(live_path));
    SAVE_DEST_LIVE_OVERWRITE_COUNT.fetch_add(1, Ordering::SeqCst);
    // THIS commit's verdict starts blank. Without it the previous commit's `target_written_ok` is
    // still 1 while this one is scored, so a failure exports as the success that came before it.
    er_telemetry::counters::save_dest_reset_commit_verdicts();
    save_dest_reset_defer_report();
    append_autoload_debug(format_args!(
        "save-dest: this commit's destination IS THE LOADED SAVE '{}' (reason={reason} len={}) -- the native writer will REWRITE it and copy it over its .bak; this is the sanctioned overwrite the user confirmed, and it is the ONLY way this flow writes the loaded save",
        live_path.display(),
        before.map_or(0, |(len, _)| len)
    ));
    *save_dest_live_overwrite_lock() = Some(SaveDestLiveOverwrite {
        live_path: live_path.to_path_buf(),
        before,
        bak_before,
        reason,
    });
}

/// Arm the scoped write-open redirect for one commit. Snapshots the live save first so a leaked
/// write can be undone, then SEEDS the destination with that snapshot: the native in-place block
/// writer seeks to offsets read from the live container's index, so the destination has to already
/// be that container or the seeks land in empty space (measured run 4: a sparse 26,608,560-byte
/// file with no `BND4` magic).
///
/// Returns false when the live save is unreadable, the destination path is unusable, or the seed
/// cannot be written -- the caller must then abort WITHOUT firing rather than report a save that
/// cannot land. A destination the user confirmed overwriting is left holding the seed if the native
/// write then fails: a complete, loadable container of the current character, which is a far better
/// failure mode than the truncated garbage the un-seeded redirect produced.
pub(crate) fn save_dest_arm_redirect(live_path: &Path, target_path: &Path) -> bool {
    let accepted_leaves = save_dest_accepted_leaves(live_path);
    if accepted_leaves.is_empty() {
        append_autoload_debug(format_args!(
            "save-dest: ARM FAILED -- live save '{}' has no file name to match write-opens against",
            live_path.display()
        ));
        return false;
    }
    // FULL-PATH match set. Without it the window diverts ANY process's write-open of a file
    // merely NAMED `ER0000.sl2` -- another Steam account's folder, a backup tool, another mod --
    // into the destination the user picked for their own character.
    let accepted_paths = save_dest_accepted_paths(live_path);
    if accepted_paths.is_empty() {
        append_autoload_debug(format_args!(
            "save-dest: ARM FAILED -- live save '{}' produced no matchable full path, so a write-open could only be recognized by its file name",
            live_path.display()
        ));
        return false;
    }
    let accepted_dirs = save_dest_accepted_dirs(live_path);
    let Some(target_text) = target_path.to_str() else {
        append_autoload_debug(format_args!(
            "save-dest: ARM FAILED -- destination '{}' is not representable as UTF-8",
            target_path.display()
        ));
        return false;
    };
    let target_w = system_quit_path_for_windows(target_text);
    let Some((live_len, live_modified_ns)) = save_dest_file_stamp(live_path) else {
        append_autoload_debug(format_args!(
            "save-dest: ARM FAILED -- cannot stat the live save '{}'",
            live_path.display()
        ));
        return false;
    };
    let Ok(live_bytes) = fs::read(live_path) else {
        append_autoload_debug(format_args!(
            "save-dest: ARM FAILED -- cannot snapshot the live save '{}' (needed to undo a leaked write)",
            live_path.display()
        ));
        return false;
    };
    let target_existed = save_dest_file_stamp(target_path).is_some();
    // SEED (the fix for run 4): the native writer patches BLOCKS at offsets it read from the live
    // container's index and opens with OPEN_ALWAYS (no truncate), so the destination must be that
    // container before the first write-open. Written from the same buffer that is the live-file
    // safety snapshot, so the seed and the "did it change" baseline can never disagree.
    //
    // ALL-OR-NOTHING. A destination the user picked is very often one of their OWN saves, and a
    // truncate-then-write of ~29 MB that fails half way through leaves it unloadable -- while the
    // very next line of this function reports that the save must not be treated as landed. The
    // sibling-temp-plus-rename in `save_dest_write_atomic` makes every failure here leave the
    // destination byte-for-byte as the user left it.
    if let Err(err) = save_dest_write_atomic(target_path, &live_bytes, "seed") {
        SAVE_DEST_SEED_FAIL_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: ARM FAILED -- could not seed destination '{}' with the live container ({} bytes): {err}; the destination is UNCHANGED and nothing is fired, because a save that cannot land must not be reported as one",
            target_path.display(),
            live_bytes.len()
        ));
        return false;
    }
    SAVE_DEST_SEEDED_COUNT.fetch_add(1, Ordering::SeqCst);
    // THIS commit's verdict starts blank -- the redirect hit count and every 0/1 verdict oracle
    // together, from one list, so the reset can never drift out of step with what is exported as
    // this commit's result.
    er_telemetry::counters::save_dest_reset_commit_verdicts();
    save_dest_reset_defer_report();
    let live_bak_before = save_dest_file_stamp(&save_dest_bak_path(live_path));
    let matched = accepted_paths.join(", ");
    *save_dest_redirect_lock() = Some(SaveDestRedirect {
        target_path: target_path.to_path_buf(),
        target_w,
        target_existed,
        live_path: live_path.to_path_buf(),
        live_len,
        live_modified_ns,
        live_bak_before,
        live_bytes,
        accepted_leaves,
        accepted_paths,
        accepted_dirs,
    });
    SAVE_DEST_REDIRECT_ARMED.store(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-dest: redirect ARMED live='{}' (len={live_len}) -> target='{}' (existing={target_existed}); destination SEEDED with a byte copy of the live container; write-opens of [{matched}] land on it, every other path -- including another folder's save of the same name -- passes through, and reads pass through",
        live_path.display(),
        target_path.display()
    ));
    true
}

pub(crate) fn save_dest_redirect_armed() -> bool {
    SAVE_DEST_REDIRECT_ARMED.load(Ordering::SeqCst) != 0
}

/// True while EITHER commit window is open: a destination redirect, or a recorded
/// overwrite-the-loaded-save. Both mean a write is in flight that stage 8 must wait for and score
/// -- scoring either one at the moment of firing would read the file before the writer touched it.
pub(crate) fn save_dest_commit_window_armed() -> bool {
    save_dest_redirect_armed() || save_dest_live_overwrite_lock().is_some()
}

/// True when `access` is a write open (the only opens the redirect may divert).
pub(crate) fn save_dest_is_write_access(access: u32) -> bool {
    er_quit_menu::save_dest_commit::save_dest_is_write_access(access)
}

/// One "the teardown is waiting for the writer" line per commit, not one per tick.
pub(crate) static SAVE_DEST_DEFER_REPORTED: AtomicUsize = AtomicUsize::new(0);

/// Where the native SL writer is, relative to the commit that armed this window.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveDestWriterState {
    /// A save-job body ran and RETURNED after the fire, and none is executing now. The container
    /// will not be opened again for this commit.
    Settled,
    /// A save-job body is executing right now. The in-place writer (`FUN_1424142e0`) opens the
    /// container ONCE PER DIRTY BLOCK, so the window must stay armed across all of them: closing
    /// it between block k and k+1 sends blocks k+1..N to whatever the unredirected path resolves
    /// to, which is the loaded save the user chose NOT to overwrite -- and it happens after the
    /// leak check has already run, so nothing detects or undoes it.
    InBody,
    /// Nothing has started since the fire. Safe to tear down only when it is also known that
    /// nothing CAN start (the bypass token was revoked, or the submit itself failed).
    NotStarted,
}

/// Read the writer's position from the SL job-body observer's own counters.
pub(crate) fn save_dest_writer_state(completions_at_fire: u64) -> SaveDestWriterState {
    if !er_save_suppress::save_job_writer_idle() {
        return SaveDestWriterState::InBody;
    }
    if er_save_suppress::save_job_completions() > completions_at_fire {
        SaveDestWriterState::Settled
    } else {
        SaveDestWriterState::NotStarted
    }
}

/// Clear the per-commit "waiting for the writer" report latch. Called at every arm.
pub(crate) fn save_dest_reset_defer_report() {
    SAVE_DEST_DEFER_REPORTED.store(0, Ordering::SeqCst);
}

/// May the commit window be torn down right now?
///
/// `false` means a save-job body is executing and the redirect must stay armed until it returns.
/// Counted every time and logged once per commit, so a commit that took an unusually long time to
/// tear down says why rather than looking like a hang.
pub(crate) fn save_dest_teardown_allowed(completions_at_fire: u64, context: &str) -> bool {
    if !save_dest_commit_window_armed() {
        return true;
    }
    if save_dest_writer_state(completions_at_fire) != SaveDestWriterState::InBody {
        return true;
    }
    SAVE_DEST_DISARM_DEFERRED.fetch_add(1, Ordering::SeqCst);
    if SAVE_DEST_DEFER_REPORTED.swap(1, Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "save-dest: teardown ({context}) is WAITING -- the SL writer is inside a save-job body (starts={} completions={}). The in-place writer opens the container once per dirty block, so disarming now would send the remaining blocks to the loaded save",
            er_save_suppress::save_job_starts(),
            er_save_suppress::save_job_completions()
        ));
    }
    false
}

/// Destination path for a `CreateFileW` write-open of `path`, or `None` to pass it through.
///
/// # What may be diverted
///
/// Exactly the loaded save's own container, by FULL path. The leaf test is only a prefilter:
/// `ER0000.sl2` is the name of every Elden Ring save on the machine, and matching on it alone
/// meant that during the armed window ANY process's write-open of any file with that name --
/// another Steam account's folder, a backup tool, Seamless, another mod, our own staged tree --
/// had its bytes rerouted into the destination the user chose for this character.
///
/// A path that clears the leaf prefilter but is not an accepted full path gets one more chance:
/// its parent directory is compared to the loaded save's by HANDLE IDENTITY, which is what
/// recognizes the same folder reached through the other Wine drive spelling. Anything else passes
/// through and is counted.
///
/// NEVER logs while holding the redirect lock, and never performs I/O while holding it either:
/// the debug log and the directory probe both open files, which re-enters this detour on the same
/// thread, and a second lock acquisition would deadlock the save worker. Everything the decision
/// needs is cloned out and the guard dropped first.
pub(crate) fn save_dest_redirect_for_open(path: &[u16], access: u32) -> Option<Vec<u16>> {
    if !save_dest_redirect_armed() || !save_dest_is_write_access(access) {
        return None;
    }
    let leaf = save_dest_wide_leaf_lower(path)?;
    let (target_w, accepted_paths, accepted_dirs) = {
        let guard = save_dest_redirect_lock();
        let state = guard.as_ref()?;
        if !state
            .accepted_leaves
            .iter()
            .any(|accepted| accepted.as_slice() == leaf.as_slice())
        {
            return None;
        }
        (
            state.target_w.clone(),
            state.accepted_paths.clone(),
            state.accepted_dirs.clone(),
        )
    };
    // An undecodable path cannot be shown to be the loaded save, and the rule for "cannot be
    // shown" is to leave it alone.
    let Some(normalized) = save_dest_normalize_wide(path) else {
        SAVE_DEST_FOREIGN_OPEN_PASSED.fetch_add(1, Ordering::SeqCst);
        return None;
    };
    if accepted_paths.contains(&normalized) {
        return Some(target_w);
    }
    // Different spelling, possibly the same folder. This costs one directory open, and only for
    // opens that already carry a save container's name during an armed commit.
    if let Some(parent) = save_dest_normalized_parent(&normalized) {
        let parent_path = Path::new(parent);
        for dir in &accepted_dirs {
            if matches!(
                save_dest_dir_identity(parent_path, dir),
                SaveDestIdentity::SameFile
            ) {
                return Some(target_w);
            }
        }
    }
    SAVE_DEST_FOREIGN_OPEN_PASSED.fetch_add(1, Ordering::SeqCst);
    None
}

/// Record a diverted write-open. First occurrence plus power-of-two milestones. A commit produces
/// ONE open per dirty block on the native in-place path (`FUN_1424142e0`) and one for a full
/// rebuild, so several hits are normal -- ZERO is the anomaly, and the commit verification is what
/// catches that.
pub(crate) fn save_dest_note_redirect_hit(handle_ok: bool) {
    let hits = SAVE_DEST_REDIRECT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 || hits.is_power_of_two() {
        let target = save_dest_redirect_lock()
            .as_ref()
            .map(|state| state.target_path.display().to_string())
            .unwrap_or_else(|| "<disarmed>".to_owned());
        append_autoload_debug(format_args!(
            "save-dest: write-open #{hits} REDIRECTED to '{target}' ok={handle_ok}"
        ));
    }
}

/// Disarm the commit window and score the file(s) it was responsible for. Returns `None` when no
/// window was armed at all, so the caller can tell "nothing to check" from "checked and failed".
///
/// Destination commits: the target must be a STRUCTURALLY COMPLETE `BND4` container (its own index
/// accounts for every byte up to EOF) of the live save's size whose bytes DIFFER from the seed --
/// an unchanged file means the native writer never reached it. The live save must NOT have changed;
/// a mutated live file is the hard failure this whole mechanism exists to prevent, so the pre-fire
/// snapshot is written back over it and the failure is logged.
///
/// Loaded-save overwrites: the live container must still be a complete `BND4` and its stamp must
/// have moved, which is what proves the rewrite the flow announced actually happened.
///
/// HARD INTERLOCK, applied to EVERY caller: while the SL worker is inside a save-job body the
/// window is not taken away from it, and this returns `None` having disarmed nothing. The flow's
/// own stage 8 never reaches that state (it gates on the same signal a step earlier), so this
/// covers the opportunistic resets -- a corrupted stage, a fresh row press -- that would otherwise
/// close the redirect between two of the in-place writer's per-block opens and send the remainder
/// to the loaded save. The IDLE-tick sweep in `save_flow_tick` closes a window left behind here as
/// soon as the writer returns.
pub(crate) fn save_dest_verify_and_disarm(reason: &str) -> Option<SaveDestVerdict> {
    if save_dest_commit_window_armed() && !er_save_suppress::save_job_writer_idle() {
        SAVE_DEST_DISARM_DEFERRED.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: REFUSING to disarm the commit window (reason={reason}) -- the SL writer is inside a save-job body (starts={} completions={}); it will be closed and scored once the writer returns",
            er_save_suppress::save_job_starts(),
            er_save_suppress::save_job_completions()
        ));
        return None;
    }
    if let Some(state) = save_dest_redirect_lock().take() {
        SAVE_DEST_REDIRECT_ARMED.store(0, Ordering::SeqCst);
        return Some(save_dest_verify_destination(&state, reason));
    }
    SAVE_DEST_REDIRECT_ARMED.store(0, Ordering::SeqCst);
    let state = save_dest_live_overwrite_lock().take()?;
    Some(save_dest_verify_live_overwrite(&state, reason))
}

pub(crate) fn save_dest_verify_destination(
    state: &SaveDestRedirect,
    reason: &str,
) -> SaveDestVerdict {
    let hits = SAVE_DEST_REDIRECT_HITS.load(Ordering::SeqCst);
    let target_bytes = fs::read(&state.target_path).unwrap_or_default();
    let magic_ok = target_bytes
        .get(..SAVE_DEST_BND4_MAGIC.len())
        .is_some_and(|magic| magic == SAVE_DEST_BND4_MAGIC);
    let size_ok = target_bytes.len() as u64 == state.live_len;
    let structure_ok = save_dest_container_end(&target_bytes) == Some(target_bytes.len());
    // Compared against the SEED, not against a pre-arm stat: the seed is what the destination held
    // when the native writer opened it, so "identical to the seed" is precisely "the writer wrote
    // nothing here".
    let changed_ok = target_bytes != state.live_bytes;
    let written_ok = hits >= 1 && magic_ok && size_ok && structure_ok && changed_ok;
    let summary = format!(
        "target='{}' (pre-existing={}) hits={hits} bnd4={magic_ok} len_ok={size_ok} structure_ok={structure_ok} changed_from_seed={changed_ok}",
        state.target_path.display(),
        state.target_existed
    );
    if written_ok {
        SAVE_DEST_TARGET_WRITTEN_OK.store(1, Ordering::SeqCst);
        SAVE_DEST_TARGET_STRUCTURE_OK.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: destination VERIFIED reason={reason} {summary}"
        ));
    } else {
        SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: destination NOT VERIFIED reason={reason} {summary}; the user's save did NOT land where they asked"
        ));
    }
    let live_state = save_dest_score_live_file(state, reason);
    let live_mutated = matches!(live_state, SaveDestLiveState::Changed);
    // The native `.bak` copy (`FUN_142410830`) is not redirected, so it is the one remaining way a
    // "save somewhere else" commit can still touch the loaded save's folder. Named rather than
    // scored: it can only ever copy the untouched live container over its own backup.
    let bak_path = save_dest_bak_path(&state.live_path);
    let bak_after = save_dest_file_stamp(&bak_path);
    if bak_after != state.live_bak_before {
        SAVE_DEST_LIVE_BAK_MUTATED.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: the loaded save's .bak twin '{}' moved during a destination commit reason={reason} -- the native backup step ran against the (unmodified) live container; the loaded save itself is unchanged={}",
            bak_path.display(),
            !live_mutated
        ));
    }
    SaveDestVerdict {
        ok: written_ok && !live_mutated,
        summary: format!("{summary} loaded_save={}", live_state.label()),
    }
}

/// What became of the LOADED save across a destination commit. Three states, not two: the old
/// code folded "cannot read it" into "it changed" with `is_none_or`, and a transient stat failure
/// -- a restrictive share mode while the game or the native `.bak` `CopyFileW` holds the file is
/// enough -- then triggered a whole-container overwrite of the user's live save with a snapshot,
/// on no evidence at all that anything had happened to it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveDestLiveState {
    /// Stamp and content match the pre-fire snapshot. The commit kept its promise.
    Untouched,
    /// The stamp moved but the bytes are identical to the snapshot. Nothing was lost, so nothing
    /// needs restoring; a rewrite here would be pure risk.
    StampOnly,
    /// The CONTENT differs from the pre-fire snapshot: the redirect leaked and the loaded save
    /// the user chose not to overwrite was written anyway.
    Changed,
    /// The loaded save could not be read. Not evidence of change, and not evidence of safety.
    Unreadable,
}

impl SaveDestLiveState {
    fn label(self) -> &'static str {
        match self {
            SaveDestLiveState::Untouched => "untouched",
            SaveDestLiveState::StampOnly => "stamp-moved-bytes-identical",
            SaveDestLiveState::Changed => "MUTATED",
            SaveDestLiveState::Unreadable => "UNREADABLE",
        }
    }
}

/// Score the loaded save after a destination commit, and restore it only on proof that it needs
/// restoring.
///
/// The restore is itself a whole-container write over the user's save file, so it is held to the
/// same standard as any other write in this flow:
///
///   * the stamp must have moved AND the bytes must actually differ from the snapshot -- a stamp
///     that moved with identical content is not a lost save;
///   * the destination must not turn out to BE this file (the case that made a successful save
///     get overwritten by its own pre-save snapshot), re-checked here by handle identity rather
///     than trusted from the arm;
///   * and the write goes through the same temp-plus-rename as the seed, so a restore that fails
///     leaves the container the writer produced instead of a truncated one. There is deliberately
///     no in-place fallback: a valid save of the wrong vintage beats an unloadable file.
pub(crate) fn save_dest_score_live_file(
    state: &SaveDestRedirect,
    reason: &str,
) -> SaveDestLiveState {
    let Some((len, modified_ns)) = save_dest_file_stamp(&state.live_path) else {
        SAVE_DEST_LIVE_STAT_UNREADABLE.store(1, Ordering::SeqCst);
        SAVE_DEST_RESTORE_SUPPRESSED.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: the loaded save '{}' could NOT BE READ at verification reason={reason} -- that is not evidence it changed, so nothing is written over it; treat the read-only guarantee for this commit as UNVERIFIED",
            state.live_path.display()
        ));
        return SaveDestLiveState::Unreadable;
    };
    if len == state.live_len && modified_ns == state.live_modified_ns {
        return SaveDestLiveState::Untouched;
    }
    let Ok(live_now) = fs::read(&state.live_path) else {
        SAVE_DEST_LIVE_STAT_UNREADABLE.store(1, Ordering::SeqCst);
        SAVE_DEST_RESTORE_SUPPRESSED.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: the loaded save '{}' stamp moved (len {}->{len}) but its bytes could not be read back reason={reason}; nothing is written over it on an unread comparison",
            state.live_path.display(),
            state.live_len
        ));
        return SaveDestLiveState::Unreadable;
    };
    if live_now == state.live_bytes {
        append_autoload_debug(format_args!(
            "save-dest: the loaded save '{}' has a NEW STAMP but IDENTICAL BYTES reason={reason} -- nothing was lost, so the pre-fire snapshot is NOT written back",
            state.live_path.display()
        ));
        SAVE_DEST_RESTORE_SUPPRESSED.fetch_add(1, Ordering::SeqCst);
        return SaveDestLiveState::StampOnly;
    }
    SAVE_DEST_LIVE_FILE_MUTATED.store(1, Ordering::SeqCst);
    // The per-commit flag above is cleared at the next arm; this count is not, so the worst thing
    // this flow can do to a save cannot be erased by the commit that follows it.
    er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED_TOTAL.fetch_add(1, Ordering::SeqCst);
    // Defence in depth for the self-redirect: if the destination IS this file, its new content is
    // the user's save, and the "leaked write" is the save itself. Writing the snapshot back would
    // destroy exactly what the commit just achieved.
    if matches!(
        save_dest_file_identity(&state.target_path, &state.live_path),
        SaveDestIdentity::SameFile | SaveDestIdentity::Unknown
    ) {
        SAVE_DEST_RESTORE_SUPPRESSED.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: the loaded save '{}' changed, but it cannot be shown to be a DIFFERENT file from the destination '{}' reason={reason} -- the change may be the user's own save landing, so the pre-fire snapshot is NOT written back",
            state.live_path.display(),
            state.target_path.display()
        ));
        return SaveDestLiveState::Changed;
    }
    match save_dest_write_atomic(&state.live_path, &state.live_bytes, "restore") {
        Ok(()) => append_autoload_debug(format_args!(
            "save-dest: LIVE SAVE MUTATED during a destination commit reason={reason} live='{}' -- the redirect leaked; the pre-fire snapshot ({} bytes) has been restored over it",
            state.live_path.display(),
            state.live_bytes.len()
        )),
        Err(err) => {
            SAVE_DEST_RESTORE_FAILED.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-dest: LIVE SAVE MUTATED during a destination commit reason={reason} live='{}' -- the redirect leaked AND the restore FAILED: {err}. The file is left exactly as the native writer produced it (a complete container of a newer state), NOT half-written",
                state.live_path.display()
            ));
        }
    }
    SaveDestLiveState::Changed
}

pub(crate) fn save_dest_verify_live_overwrite(
    state: &SaveDestLiveOverwrite,
    reason: &str,
) -> SaveDestVerdict {
    let after = save_dest_file_stamp(&state.live_path);
    let changed_ok = after.is_some() && after != state.before;
    let bytes = fs::read(&state.live_path).unwrap_or_default();
    let magic_ok = bytes
        .get(..SAVE_DEST_BND4_MAGIC.len())
        .is_some_and(|magic| magic == SAVE_DEST_BND4_MAGIC);
    let structure_ok = save_dest_container_end(&bytes) == Some(bytes.len());
    let bak_after = save_dest_file_stamp(&save_dest_bak_path(&state.live_path));
    let ok = changed_ok && magic_ok && structure_ok;
    let summary = format!(
        "loaded save '{}' rewritten in place (reason={}) bnd4={magic_ok} structure_ok={structure_ok} changed={changed_ok} len={}->{} bak_moved={}",
        state.live_path.display(),
        state.reason,
        state.before.map_or(0, |(len, _)| len),
        after.map_or(0, |(len, _)| len),
        bak_after != state.bak_before
    );
    if ok {
        append_autoload_debug(format_args!(
            "save-dest: LOADED SAVE OVERWRITE VERIFIED reason={reason} {summary}"
        ));
    } else {
        SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: LOADED SAVE OVERWRITE NOT VERIFIED reason={reason} {summary}; the user's save did NOT land"
        ));
    }
    SaveDestVerdict { ok, summary }
}

/// Loaded-save path the destination flow works against (write target of the native save).
pub(crate) fn save_dest_live_save_path() -> Option<PathBuf> {
    match system_quit_env_save_path() {
        Ok(path) => Some(PathBuf::from(path)),
        Err(reason) => {
            append_autoload_debug(format_args!(
                "save-dest: live save path unavailable -- {reason}"
            ));
            None
        }
    }
}

/// Is the browsed `target` the loaded save, a different file, or unprovable?
///
/// The whole destructive failure mode of this flow hangs off this answer. A `target` that IS the
/// loaded save must take the plain overwrite path; arming a redirect from the live save onto
/// ITSELF makes the write land correctly, moves the live file's stamp because it IS the
/// destination, and then trips the leak check into writing the pre-fire snapshot back over the
/// save the user just made.
///
/// A string compare cannot answer it. Under Wine one save file is reachable as
/// `C:\users\steamuser\AppData\Roaming\EldenRing\<id>\ER0000.sl2` AND as
/// `Z:\home\<user>\...\pfx\drive_c\users\steamuser\AppData\Roaming\EldenRing\<id>\ER0000.sl2`,
/// and the destination browser produces the second form whenever its start directory came from a
/// remembered Linux-form path -- so the two spellings meet in exactly the flow that matters. On
/// top of that, no amount of text handling resolves a symlinked folder or a junction.
///
/// So this is a handle-identity question ([`save_dest_file_identity`]), with a third answer.
/// `Unknown` is not "probably different": the caller refuses to fire on it.
pub(crate) fn save_dest_commit_identity(target: &Path, live: &Path) -> SaveDestIdentity {
    save_dest_file_identity(target, live)
}

#[cfg(test)]
mod save_dest_commit_tests {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use er_telemetry::counters::{
        SAVE_DEST_LIVE_BAK_MUTATED, SAVE_DEST_LIVE_FILE_MUTATED, SAVE_DEST_LIVE_STAT_UNREADABLE,
        SAVE_DEST_REDIRECT_HITS, SAVE_DEST_TARGET_STRUCTURE_OK, SAVE_DEST_TARGET_WRITTEN_OK,
    };

    /// Build a minimal, structurally complete BND4 container: header, `names.len()` entry headers
    /// of `entry_len` bytes each, a UTF-16 name table, then the data blobs back to back.
    ///
    /// Deterministic generator, not captured game bytes (repo rule: no game-derived binaries in
    /// tree). It reproduces only the four header/entry fields `parse_entries` reads.
    fn synthetic_container(names: &[&str], entry_len: usize) -> Vec<u8> {
        const HEADER_LEN: usize = 0x40;
        const ENTRY_STRIDE: usize = 0x20;
        let names_at = HEADER_LEN + names.len() * ENTRY_STRIDE;
        let name_bytes: Vec<Vec<u8>> = names
            .iter()
            .map(|name| {
                let mut out: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
                out.extend_from_slice(&[0, 0]);
                out
            })
            .collect();
        let names_len: usize = name_bytes.iter().map(Vec::len).sum();
        let data_at = names_at + names_len;
        let mut out = vec![0_u8; data_at + names.len() * entry_len];
        out[..4].copy_from_slice(&SAVE_DEST_BND4_MAGIC);
        out[0x0c..0x10].copy_from_slice(&(names.len() as i32).to_le_bytes());
        out[0x10..0x18].copy_from_slice(&(HEADER_LEN as i64).to_le_bytes());
        out[0x20..0x28].copy_from_slice(&(ENTRY_STRIDE as i64).to_le_bytes());
        let mut name_cursor = names_at;
        for (index, name) in name_bytes.iter().enumerate() {
            let entry = HEADER_LEN + index * ENTRY_STRIDE;
            out[entry + 0x08..entry + 0x10].copy_from_slice(&(entry_len as i64).to_le_bytes());
            out[entry + 0x10..entry + 0x14]
                .copy_from_slice(&((data_at + index * entry_len) as i32).to_le_bytes());
            out[entry + 0x14..entry + 0x18].copy_from_slice(&(name_cursor as i32).to_le_bytes());
            out[name_cursor..name_cursor + name.len()].copy_from_slice(name);
            name_cursor += name.len();
        }
        out
    }

    #[test]
    fn a_complete_container_ends_exactly_where_its_index_says() {
        let bytes = synthetic_container(&["USER_DATA000", "USER_DATA001", "USER_DATA010"], 0x200);
        assert_eq!(save_dest_container_end(&bytes), Some(bytes.len()));
    }

    /// The exact shape run 4 produced: the native in-place writer seeked past the start of an empty
    /// destination and wrote one block, leaving a sparse file with no header at all. Length alone
    /// cannot catch this -- the structure check must.
    #[test]
    fn a_sparse_fragment_with_no_header_is_not_a_container() {
        let complete = synthetic_container(&["USER_DATA000", "USER_DATA010"], 0x200);
        let mut sparse = vec![0_u8; complete.len()];
        let tail = sparse.len() - 0x200;
        sparse[tail..].copy_from_slice(&complete[tail..]);
        assert_eq!(save_dest_container_end(&sparse), None);
    }

    /// A container whose index describes more bytes than the file holds is incomplete even though
    /// it parses and carries the magic.
    #[test]
    fn a_truncated_container_does_not_account_for_its_own_index() {
        let mut bytes = synthetic_container(&["USER_DATA000", "USER_DATA001"], 0x200);
        let full = bytes.len();
        bytes.truncate(full - 0x100);
        assert_eq!(save_dest_container_end(&bytes), Some(full));
        assert_ne!(save_dest_container_end(&bytes), Some(bytes.len()));
    }

    #[test]
    fn the_bak_twin_is_the_container_path_plus_bak() {
        let live = Path::new(r"C:\users\steamuser\AppData\Roaming\EldenRing\1234\ER0000.sl2");
        assert_eq!(
            save_dest_bak_path(live),
            Path::new(r"C:\users\steamuser\AppData\Roaming\EldenRing\1234\ER0000.sl2.bak")
        );
    }

    /// The redirect must recognize the loaded save's container by its FULL path. `ER0000.sl2` is
    /// the name of every Elden Ring save on the machine; matching the leaf alone diverted any
    /// process's write-open of that name -- another Steam account's folder included -- into the
    /// destination the user picked for this character.
    #[test]
    fn another_accounts_save_of_the_same_name_is_not_an_accepted_path() {
        let live = Path::new(r"C:\users\steamuser\AppData\Roaming\EldenRing\7656\ER0000.sl2");
        let accepted = save_dest_accepted_paths(live);
        let own = r"c:\users\steamuser\appdata\roaming\eldenring\7656\er0000.sl2".to_owned();
        let twin = r"c:\users\steamuser\appdata\roaming\eldenring\7656\er0000.co2".to_owned();
        let other_account =
            r"c:\users\steamuser\appdata\roaming\eldenring\9999\er0000.sl2".to_owned();
        let backup_folder = r"c:\backups\er0000.sl2".to_owned();
        assert!(accepted.contains(&own));
        assert!(accepted.contains(&twin));
        assert!(!accepted.contains(&other_account));
        assert!(!accepted.contains(&backup_folder));
    }

    /// The incoming `CreateFileW` path is matched after the same normalization, so case and
    /// separator differences still hit while a different folder still misses.
    #[test]
    fn an_incoming_write_open_matches_only_through_normalization() {
        let live = Path::new(r"C:\users\steamuser\AppData\Roaming\EldenRing\7656\ER0000.sl2");
        let accepted = save_dest_accepted_paths(live);
        let wide = |text: &str| -> Vec<u16> {
            let mut out: Vec<u16> = text.encode_utf16().collect();
            out.push(0);
            out
        };
        let same_file_other_case = save_dest_normalize_wide(&wide(
            r"C:\Users\SteamUser\AppData\Roaming\EldenRing\7656\ER0000.SL2",
        ))
        .expect("normalizes");
        let different_folder = save_dest_normalize_wide(&wide(
            r"C:\SteamLibrary\ELDEN RING\Game\SeamlessCoop\ER0000.sl2",
        ))
        .expect("normalizes");
        assert!(accepted.contains(&same_file_other_case));
        assert!(!accepted.contains(&different_folder));
    }

    /// Scratch directory for one test, keyed by PROCESS as well as by name.
    ///
    /// The pid matters: this suite wipes the directory on entry, `%TEMP%` is shared by every
    /// process in the wine prefix, and two test binaries running at once (two checkouts, or the
    /// gate run twice over) then delete each other's files mid-test. Measured that way -- a
    /// `rename` onto a target whose parent had just been removed came back
    /// `File not found (os error 2)` and the atomic-write and restore tests failed with nothing
    /// wrong in the code they cover.
    fn save_dest_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "er-save-dest-{name}-p{pid}",
            pid = std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("test dir");
        dir
    }

    /// The seed and the restore both replace a whole ~29 MB save container. A truncate-then-write
    /// that fails part way through leaves the user's file neither old nor new; this one either
    /// lands completely or leaves the previous bytes untouched, and never leaves its staging file
    /// behind for the destination browser to list.
    #[test]
    fn an_atomic_write_replaces_the_file_and_leaves_no_staging_copy() {
        let dir = save_dest_test_dir("atomic-write");
        let target = dir.join("ER0000.sl2");
        fs::write(&target, b"old contents").expect("seed the target");
        save_dest_write_atomic(&target, b"new contents", "seed").expect("atomic write");
        assert_eq!(fs::read(&target).expect("read back"), b"new contents");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("list")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging file left behind: {leftovers:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A destination whose folder has gone away must fail with the destination untouched rather
    /// than after emptying it.
    #[test]
    fn an_atomic_write_that_cannot_stage_leaves_the_destination_alone() {
        let dir = save_dest_test_dir("atomic-write-fail");
        let target = dir.join("missing").join("ER0000.sl2");
        assert!(save_dest_write_atomic(&target, b"payload", "seed").is_err());
        assert!(!target.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    fn save_dest_test_redirect(live: &Path, target: &Path, bytes: Vec<u8>) -> SaveDestRedirect {
        let (live_len, live_modified_ns) = save_dest_file_stamp(live).unwrap_or((0, 0));
        SaveDestRedirect {
            target_path: target.to_path_buf(),
            target_w: Vec::new(),
            target_existed: false,
            live_path: live.to_path_buf(),
            live_len,
            live_modified_ns,
            live_bak_before: None,
            live_bytes: bytes,
            accepted_leaves: Vec::new(),
            accepted_paths: Vec::new(),
            accepted_dirs: Vec::new(),
        }
    }

    /// A loaded save whose stat cannot be read is UNREADABLE, not MUTATED. The old `is_none_or`
    /// folded the two together, so one transient stat failure -- a restrictive share mode while
    /// the game or the native `.bak` copy holds the file is enough -- triggered a blind whole-
    /// container overwrite of the user's live save.
    #[test]
    fn an_unreadable_loaded_save_is_not_restored_over() {
        let dir = save_dest_test_dir("live-unreadable");
        let live = dir.join("ER0000.sl2");
        let target = dir.join("elsewhere.sl2");
        let state = save_dest_test_redirect(&live, &target, b"pre-fire snapshot".to_vec());
        assert!(!live.exists());
        assert!(matches!(
            save_dest_score_live_file(&state, "test"),
            SaveDestLiveState::Unreadable
        ));
        assert!(
            !live.exists(),
            "an unreadable loaded save must not be written to"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A stamp that moved while the bytes stayed identical is not a lost save, so the pre-fire
    /// snapshot must not be written back over it.
    #[test]
    fn a_stamp_that_moved_with_identical_bytes_is_not_a_mutation() {
        let dir = save_dest_test_dir("live-stamp-only");
        let live = dir.join("ER0000.sl2");
        let target = dir.join("elsewhere.sl2");
        fs::write(&live, b"identical").expect("write live");
        let mut state = save_dest_test_redirect(&live, &target, b"identical".to_vec());
        // Force the "stamp moved" branch without changing a byte.
        state.live_modified_ns = state.live_modified_ns.wrapping_add(1);
        assert!(matches!(
            save_dest_score_live_file(&state, "test"),
            SaveDestLiveState::StampOnly
        ));
        assert_eq!(fs::read(&live).expect("read live"), b"identical");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A loaded save whose CONTENT differs from the pre-fire snapshot is the leak this mechanism
    /// exists to catch, and the snapshot goes back over it.
    #[test]
    fn a_genuinely_mutated_loaded_save_is_restored_from_the_snapshot() {
        let dir = save_dest_test_dir("live-mutated");
        let live = dir.join("ER0000.sl2");
        let target = dir.join("elsewhere.sl2");
        fs::write(&live, b"snapshot").expect("write live");
        let mut state = save_dest_test_redirect(&live, &target, b"snapshot".to_vec());
        fs::write(&target, b"destination").expect("write target");
        fs::write(&live, b"leaked write").expect("leak onto live");
        state.live_len = b"snapshot".len() as u64;
        assert!(matches!(
            save_dest_score_live_file(&state, "test"),
            SaveDestLiveState::Changed
        ));
        assert_eq!(fs::read(&live).expect("read live"), b"snapshot");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Arming a commit must WIPE the previous commit's verdict.
    ///
    /// Each verdict oracle is stored only on the branch that observes it, so an unreset flag stays
    /// at 1 for the life of the process: after one verified commit, `target_written_ok` was still
    /// published as 1 while a LATER commit failed, which reports a save that never landed as one
    /// that did. Only the oracles no other test in this crate writes are read back here -- the
    /// tests share one process and `save_dest_score_live_file`'s own tests legitimately raise
    /// `LIVE_FILE_MUTATED` / `LIVE_STAT_UNREADABLE`. Those two are covered by the membership test
    /// below, which nothing concurrent can perturb.
    #[test]
    fn arming_a_commit_clears_the_previous_commits_verdict() {
        SAVE_DEST_TARGET_WRITTEN_OK.store(1, Ordering::SeqCst);
        SAVE_DEST_TARGET_STRUCTURE_OK.store(1, Ordering::SeqCst);
        SAVE_DEST_LIVE_BAK_MUTATED.store(1, Ordering::SeqCst);
        SAVE_DEST_REDIRECT_HITS.store(7, Ordering::SeqCst);
        // The live-overwrite arm needs no file to exist: it stats the path (None is fine), names
        // the intent and records it. Nothing is written.
        save_dest_arm_live_overwrite(
            Path::new(r"C:\users\steamuser\AppData\Roaming\EldenRing\7656\ER0000.sl2"),
            "verdict-reset-test",
        );
        assert_eq!(SAVE_DEST_TARGET_WRITTEN_OK.load(Ordering::SeqCst), 0);
        assert_eq!(SAVE_DEST_TARGET_STRUCTURE_OK.load(Ordering::SeqCst), 0);
        assert_eq!(SAVE_DEST_LIVE_BAK_MUTATED.load(Ordering::SeqCst), 0);
        assert_eq!(SAVE_DEST_REDIRECT_HITS.load(Ordering::SeqCst), 0);
        // Leave no armed window behind for the rest of the process.
        save_dest_live_overwrite_lock().take();
    }

    /// Every 0/1 oracle this file publishes as one commit's verdict has to be IN the arm-time reset
    /// set. A new verdict oracle added without its reset is exactly the original defect again, so
    /// this compares by address rather than by value and cannot be perturbed by a parallel test.
    #[test]
    fn every_per_commit_verdict_oracle_is_reset_at_arm_time() {
        let reset_set = er_telemetry::counters::save_dest_commit_verdict_oracles();
        let address = |oracle: &'static AtomicUsize| oracle as *const AtomicUsize as usize;
        for verdict in [
            &SAVE_DEST_REDIRECT_HITS,
            &SAVE_DEST_TARGET_WRITTEN_OK,
            &SAVE_DEST_TARGET_STRUCTURE_OK,
            &SAVE_DEST_LIVE_FILE_MUTATED,
            &SAVE_DEST_LIVE_BAK_MUTATED,
            &SAVE_DEST_LIVE_STAT_UNREADABLE,
        ] {
            assert!(
                reset_set
                    .iter()
                    .any(|resettable| address(resettable) == address(verdict)),
                "a per-commit verdict oracle is missing from the arm-time reset set"
            );
        }
    }

    /// The cumulative leak count must survive the reset: a leak the snapshot restore then repaired
    /// increments no other process-wide counter, so clearing the per-commit flag without it would
    /// leave the worst outcome this flow can produce with no trace at all.
    #[test]
    fn the_cumulative_leak_count_outlives_the_per_commit_reset() {
        let before =
            er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED_TOTAL.load(Ordering::SeqCst);
        let dir = save_dest_test_dir("leak-total");
        let live = dir.join("ER0000.sl2");
        let target = dir.join("elsewhere.sl2");
        fs::write(&live, b"snapshot").expect("write live");
        let mut state = save_dest_test_redirect(&live, &target, b"snapshot".to_vec());
        fs::write(&target, b"destination").expect("write target");
        fs::write(&live, b"leaked write").expect("leak onto live");
        state.live_len = b"snapshot".len() as u64;
        assert!(matches!(
            save_dest_score_live_file(&state, "test"),
            SaveDestLiveState::Changed
        ));
        assert!(
            er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED_TOTAL.load(Ordering::SeqCst)
                > before
        );
        er_telemetry::counters::save_dest_reset_commit_verdicts();
        assert_eq!(SAVE_DEST_LIVE_FILE_MUTATED.load(Ordering::SeqCst), 0);
        assert!(
            er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED_TOTAL.load(Ordering::SeqCst)
                > before
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
