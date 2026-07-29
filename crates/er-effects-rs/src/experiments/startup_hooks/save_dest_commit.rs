// Save-to-a-chosen-destination commit state (save-game-flow WP3).
//
// The System->Quit "Save Game" flow can end on a destination the user browsed to instead of the
// loaded save. Nothing about the native save is re-implemented: the game's own BND4 writer
// (`FUN_142413860`) is a READ-MODIFY-WRITE -- it reads back every block the request did not supply
// from the live container, rebuilds the whole image with its own headers + per-entry MD5, and
// emits it as ONE whole-buffer write through the single `CreateFileW` funnel every FromSoft file
// open uses. So diverting exactly that write-open produces a complete, game-authored, MD5-correct
// container at the destination while the loaded save is only ever READ.
//
// The window is armed at the fire gate and disarmed at completion (never one-shot: a writer retry
// must not be able to leak onto the live file). Read-opens pass through -- the read side IS the
// "current state" the user asked to write elsewhere -- and so does the native `.bak` `CopyFileW`,
// which is normal save behavior against a file we never write.
//
// Safety net: the live file's bytes/stat are snapshotted before the fire, and completion verifies
// (a) the destination exists, starts with `BND4`, matches the live container size and changed on
// disk, and (b) the live file did NOT change. A mutated live file is a hard failure oracle
// (`oracle_save_dest_live_file_mutated`): the snapshot is restored over it and the failure is
// logged and published.

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
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED;
pub(crate) use er_telemetry::counters::SAVE_DEST_PICKER_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_EXISTING_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_NEW_COUNT;
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
    /// `(len, modified_ns)` of the destination before the commit; `None` if it did not exist.
    target_before: Option<(u64, u128)>,
    live_path: PathBuf,
    live_len: u64,
    live_modified_ns: u128,
    /// Pre-fire bytes of the LIVE save, restored if the redirect leaks and the loaded save is
    /// mutated anyway -- the user explicitly chose NOT to overwrite it.
    live_bytes: Vec<u8>,
    /// Accepted leaf names (ASCII-lowercased UTF-16): the live save's own leaf plus its
    /// `.sl2`/`.co2` counterpart twin.
    accepted_leaves: Vec<Vec<u16>>,
}

static SAVE_DEST_REDIRECT: Mutex<Option<SaveDestRedirect>> = Mutex::new(None);

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
        save_dest_verify_and_disarm(reason);
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

/// Arm the scoped write-open redirect for one commit. Snapshots the live save first so a leaked
/// write can be undone. Returns false when the live save is unreadable or the destination path is
/// unusable -- the caller must then abort WITHOUT firing rather than save to the wrong file.
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
    let target_before = save_dest_file_stamp(target_path);
    SAVE_DEST_REDIRECT_HITS.store(0, Ordering::SeqCst);
    *save_dest_redirect_lock() = Some(SaveDestRedirect {
        target_path: target_path.to_path_buf(),
        target_w,
        target_before,
        live_path: live_path.to_path_buf(),
        live_len,
        live_modified_ns,
        live_bytes,
        accepted_leaves,
    });
    SAVE_DEST_REDIRECT_ARMED.store(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-dest: redirect ARMED live='{}' (len={live_len}) -> target='{}' (existing={}); save-container write-opens now land on the destination, reads pass through",
        live_path.display(),
        target_path.display(),
        target_before.is_some()
    ));
    true
}

pub(crate) fn save_dest_redirect_armed() -> bool {
    SAVE_DEST_REDIRECT_ARMED.load(Ordering::SeqCst) != 0
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

/// Record a diverted write-open. First occurrence plus power-of-two milestones (a save writes
/// exactly one container, so anything past the first is already an anomaly worth seeing).
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

/// Disarm the redirect and score the commit: the destination must exist, be a BND4 container of
/// the live save's size, and have changed on disk; the live save must NOT have changed.
///
/// A mutated live file is the hard failure this whole mechanism exists to prevent, so the
/// pre-fire snapshot is written back over it and the failure is logged.
pub(crate) fn save_dest_verify_and_disarm(reason: &str) {
    let Some(state) = save_dest_redirect_lock().take() else {
        SAVE_DEST_REDIRECT_ARMED.store(0, Ordering::SeqCst);
        return;
    };
    SAVE_DEST_REDIRECT_ARMED.store(0, Ordering::SeqCst);
    let hits = SAVE_DEST_REDIRECT_HITS.load(Ordering::SeqCst);
    let target_after = save_dest_file_stamp(&state.target_path);
    let magic_ok = match fs::File::open(&state.target_path) {
        Ok(mut file) => {
            let mut magic = [0_u8; SAVE_DEST_BND4_MAGIC.len()];
            std::io::Read::read_exact(&mut file, &mut magic).is_ok() && magic == SAVE_DEST_BND4_MAGIC
        }
        Err(_) => false,
    };
    let size_ok = target_after.is_some_and(|(len, _)| len == state.live_len);
    let changed_ok = match (state.target_before, target_after) {
        (None, Some(_)) => true,
        (Some(before), Some(after)) => before != after,
        _ => false,
    };
    let written_ok = hits >= 1 && magic_ok && size_ok && changed_ok;
    let live_after = save_dest_file_stamp(&state.live_path);
    let live_mutated =
        live_after.is_none_or(|(len, ns)| len != state.live_len || ns != state.live_modified_ns);
    if written_ok {
        SAVE_DEST_TARGET_WRITTEN_OK.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: destination VERIFIED reason={reason} target='{}' hits={hits} len_ok={size_ok} bnd4={magic_ok} changed={changed_ok}",
            state.target_path.display()
        ));
    } else {
        SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest: destination NOT VERIFIED reason={reason} target='{}' hits={hits} bnd4={magic_ok} len_ok={size_ok} changed={changed_ok}; the user's save did NOT land where they asked",
            state.target_path.display()
        ));
    }
    if live_mutated {
        SAVE_DEST_LIVE_FILE_MUTATED.store(1, Ordering::SeqCst);
        let restored = fs::write(&state.live_path, &state.live_bytes).is_ok();
        append_autoload_debug(format_args!(
            "save-dest: LIVE SAVE MUTATED during a destination commit reason={reason} live='{}' -- the redirect leaked; restored pre-fire snapshot ok={restored} ({} bytes)",
            state.live_path.display(),
            state.live_bytes.len()
        ));
    }
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
