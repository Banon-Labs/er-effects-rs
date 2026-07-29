// Save-to-a-chosen-destination commit state (save-game-flow WP3).
//
// The System->Quit "Save Game" flow can end on a destination the user browsed to instead of the
// loaded save. Nothing about the native save is re-implemented: the game's own writer is diverted
// at the `CreateFileW` funnel every FromSoft file open uses.
//
// # What the native writer actually does (1.16.2 decompile, corrected 2026-07-28)
//
// The save job body `FUN_14240fd70` formats the container path (`L"%s\\%s%s"`) and then picks ONE
// OF TWO write paths from `FUN_142413230`, which mounts the container ALREADY ON DISK at that path
// and checks whether every block the request supplies still fits its existing entry:
//
//   * no usable container / a block outgrew its entry -> `FUN_142413860`, the FULL REBUILD: it
//     rebuilds the whole image in memory and emits it as one `WriteBytes` from offset 0.
//   * every block fits (the steady state for a save over an existing container) ->
//     `FUN_1424142e0`, the PER-BLOCK IN-PLACE WRITER, called once per supplied block:
//     `OpenFile` -> `Seek(entry.dataOffset)` -> `WriteBytes(block)` -> optionally
//     `Seek(entryHeaderOffset)` + `WriteBytes(0x20)` -> `Seek(0, END)` -> `CloseStream`.
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
// Box2 answered "Yes" (and a browsed pick that resolves back to the loaded save) means the native
// writer rewrites the live container in place -- correct, sanctioned, and until now completely
// unnamed in telemetry, which is how run 4's 20:43:51 live-file rewrite read as an anonymous
// mutation. `save_dest_arm_live_overwrite` records that intent BEFORE the fire and verifies it
// afterwards, so every save this flow performs names the file it is about to rewrite.

/// 1 while the scoped write-open redirect is armed. Read by the `CreateFileW` detour BEFORE it
/// touches any lock, so an unarmed process pays one relaxed-ordering atomic load per open.
pub(crate) use er_telemetry::counters::SAVE_DEST_REDIRECT_ARMED;
pub(crate) use er_telemetry::counters::SAVE_DEST_REDIRECT_HITS;
/// Flow latches: the menu-pump open request, and the "a destination is chosen, commit once the
/// picker has torn down" hand-off from the picker's activation hook to the save-flow tick.
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_PENDING;
pub(crate) use er_telemetry::counters::SAVE_DEST_OPEN_PICKER_PENDING;
/// Destination oracles.
pub(crate) use er_telemetry::counters::SAVE_DEST_CANCEL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_FAIL;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_BAK_MUTATED;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_OVERWRITE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_PICKER_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_SEED_FAIL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_SEEDED_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_EXISTING_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_NEW_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_STRUCTURE_OK;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_WRITTEN_OK;

/// BND4 container magic: the first four bytes of every ER save the game writes.
const SAVE_DEST_BND4_MAGIC: [u8; 4] = *b"BND4";
/// `CreateFileW` desired-access bits that make an open a WRITE open (`GENERIC_WRITE`,
/// `FILE_WRITE_DATA`). Read-opens (the RMW base read) must pass through untouched.
const SAVE_DEST_WRITE_ACCESS_MASK: u32 = 0x4000_0000 | 0x2;
/// Save-container extensions: a destination whose leaf is the live save's counterpart twin
/// (`ER0000.co2` vs `ER0000.sl2`) is still the same open, whichever side rewrote the path first.
const SAVE_DEST_SEAMLESS_EXTENSION: &str = "co2";
const SAVE_DEST_VANILLA_EXTENSION: &str = "sl2";

/// The chosen destination for the save currently being committed. `None` = the loaded save is the
/// target (the plain overwrite path), which needs no redirect at all.
static SAVE_DEST_TARGET_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Live save + destination bookkeeping for one in-flight commit.
struct SaveDestRedirect {
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
    /// `.sl2`/`.co2` counterpart twin.
    accepted_leaves: Vec<Vec<u16>>,
}

static SAVE_DEST_REDIRECT: Mutex<Option<SaveDestRedirect>> = Mutex::new(None);

/// A commit whose destination IS the loaded save: Box2 answered "Yes", or a browsed pick that
/// resolves back to the loaded save. Nothing is redirected -- the native writer rewrites the live
/// container in place, which is exactly what the user asked for. Recorded anyway so the rewrite is
/// NAMED before it happens and scored after it, instead of surfacing later as an unattributed
/// change to the user's save file.
struct SaveDestLiveOverwrite {
    live_path: PathBuf,
    before: Option<(u64, u128)>,
    bak_before: Option<(u64, u128)>,
    reason: &'static str,
}

static SAVE_DEST_LIVE_OVERWRITE: Mutex<Option<SaveDestLiveOverwrite>> = Mutex::new(None);

/// Outcome of scoring one commit's file(s). Returned so the flow's FINAL log line can state the
/// file result rather than announcing the game's SL status and being contradicted a line later.
pub(crate) struct SaveDestVerdict {
    pub(crate) ok: bool,
    pub(crate) summary: String,
}

fn save_dest_target_lock() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    SAVE_DEST_TARGET_PATH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn save_dest_redirect_lock() -> std::sync::MutexGuard<'static, Option<SaveDestRedirect>> {
    SAVE_DEST_REDIRECT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn save_dest_live_overwrite_lock() -> std::sync::MutexGuard<'static, Option<SaveDestLiveOverwrite>> {
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
fn save_dest_wide_leaf_lower(path: &[u16]) -> Option<Vec<u16>> {
    let start = path
        .iter()
        .rposition(|&c| c == b'\\' as u16 || c == b'/' as u16)
        .map_or(0, |idx| idx + 1);
    let leaf = path.get(start..)?;
    if leaf.is_empty() {
        return None;
    }
    Some(leaf.iter().copied().map(save_dest_ascii_lower).collect())
}

fn save_dest_ascii_lower(c: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&c) {
        c + 0x20
    } else {
        c
    }
}

/// The live save's leaf plus its counterpart-extension twin, ASCII-lowercased UTF-16.
fn save_dest_accepted_leaves(live_path: &Path) -> Vec<Vec<u16>> {
    let Some(leaf) = live_path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let mut names = vec![leaf.to_ascii_lowercase()];
    if let Some((stem, extension)) = leaf.rsplit_once('.') {
        let twin = if extension.eq_ignore_ascii_case(SAVE_DEST_SEAMLESS_EXTENSION) {
            Some(SAVE_DEST_VANILLA_EXTENSION)
        } else if extension.eq_ignore_ascii_case(SAVE_DEST_VANILLA_EXTENSION) {
            Some(SAVE_DEST_SEAMLESS_EXTENSION)
        } else {
            None
        };
        if let Some(twin) = twin {
            names.push(format!("{}.{twin}", stem.to_ascii_lowercase()));
        }
    }
    names
        .iter()
        .map(|name| name.encode_utf16().collect())
        .collect()
}

fn save_dest_file_stamp(path: &Path) -> Option<(u64, u128)> {
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
fn save_dest_bak_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

/// End offset of the last BND4 entry, i.e. the length a STRUCTURALLY COMPLETE container must have.
///
/// This is the check that would have caught run 4's garbage file for what it was: the sparse file
/// the in-place writer left had no `BND4` header at all, and even a container that parses is only
/// complete when its own index accounts for every byte up to EOF.
fn save_dest_container_end(bytes: &[u8]) -> Option<usize> {
    let entries = er_save_loader::bnd4::parse_entries(bytes).ok()?;
    if entries.is_empty() {
        return None;
    }
    let mut end = 0_usize;
    for entry in &entries {
        end = end.max(entry.data_offset.checked_add(entry.entry_size)?);
    }
    Some(end)
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
    if let Err(err) = fs::write(target_path, &live_bytes) {
        SAVE_DEST_SEED_FAIL_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: ARM FAILED -- could not seed destination '{}' with the live container ({} bytes): {err}; NOT firing, because a save that cannot land must not be reported as one",
            target_path.display(),
            live_bytes.len()
        ));
        return false;
    }
    SAVE_DEST_SEEDED_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_DEST_REDIRECT_HITS.store(0, Ordering::SeqCst);
    let live_bak_before = save_dest_file_stamp(&save_dest_bak_path(live_path));
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
    });
    SAVE_DEST_REDIRECT_ARMED.store(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-dest: redirect ARMED live='{}' (len={live_len}) -> target='{}' (existing={target_existed}); destination SEEDED with a byte copy of the live container, save-container write-opens now land on it, reads pass through",
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
    access & SAVE_DEST_WRITE_ACCESS_MASK != 0
}

/// Destination path for a `CreateFileW` write-open of `path`, or `None` to pass it through.
///
/// NEVER logs while holding the redirect lock: the debug log itself opens a file, which re-enters
/// this detour on the same thread, and a second lock acquisition would deadlock the save worker.
pub(crate) fn save_dest_redirect_for_open(path: &[u16], access: u32) -> Option<Vec<u16>> {
    if !save_dest_redirect_armed() || !save_dest_is_write_access(access) {
        return None;
    }
    let leaf = save_dest_wide_leaf_lower(path)?;
    let guard = save_dest_redirect_lock();
    let state = guard.as_ref()?;
    if !state
        .accepted_leaves
        .iter()
        .any(|accepted| accepted.as_slice() == leaf.as_slice())
    {
        return None;
    }
    Some(state.target_w.clone())
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
pub(crate) fn save_dest_verify_and_disarm(reason: &str) -> Option<SaveDestVerdict> {
    if let Some(state) = save_dest_redirect_lock().take() {
        SAVE_DEST_REDIRECT_ARMED.store(0, Ordering::SeqCst);
        return Some(save_dest_verify_destination(&state, reason));
    }
    SAVE_DEST_REDIRECT_ARMED.store(0, Ordering::SeqCst);
    let state = save_dest_live_overwrite_lock().take()?;
    Some(save_dest_verify_live_overwrite(&state, reason))
}

fn save_dest_verify_destination(state: &SaveDestRedirect, reason: &str) -> SaveDestVerdict {
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
    let live_after = save_dest_file_stamp(&state.live_path);
    let live_mutated =
        live_after.is_none_or(|(len, ns)| len != state.live_len || ns != state.live_modified_ns);
    if live_mutated {
        SAVE_DEST_LIVE_FILE_MUTATED.store(1, Ordering::SeqCst);
        let restored = fs::write(&state.live_path, &state.live_bytes).is_ok();
        append_autoload_debug(format_args!(
            "save-dest: LIVE SAVE MUTATED during a destination commit reason={reason} live='{}' -- the redirect leaked; restored pre-fire snapshot ok={restored} ({} bytes)",
            state.live_path.display(),
            state.live_bytes.len()
        ));
    }
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
        summary,
    }
}

fn save_dest_verify_live_overwrite(state: &SaveDestLiveOverwrite, reason: &str) -> SaveDestVerdict {
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

/// True when `target` IS the loaded save: the commit is then the plain overwrite path and no
/// redirect is armed.
///
/// Both sides go through the SAME Windows-form transform the redirect hands to `CreateFileW`
/// (`Z:`-prefixing a Linux-form path, separator normalization), so a live path in one form and a
/// browsed target in the other still compare equal. Getting this wrong would arm a redirect from
/// the live save onto itself and then score the resulting write as a "live file mutated" failure,
/// restoring the pre-save snapshot over the save the user just made.
pub(crate) fn save_dest_target_is_live(target: &Path, live: &Path) -> bool {
    let normalize = |path: &Path| -> Option<String> {
        let wide = system_quit_path_for_windows(path.to_str()?);
        let end = wide.iter().position(|unit| *unit == 0).unwrap_or(wide.len());
        String::from_utf16(&wide[..end])
            .ok()
            .map(|text| text.to_ascii_lowercase())
    };
    match (normalize(target), normalize(live)) {
        (Some(target), Some(live)) => target == live,
        _ => false,
    }
}

#[cfg(test)]
mod save_dest_commit_tests {
    use super::*;

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
}
