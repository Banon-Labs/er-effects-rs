use super::*;

#[derive(Clone, Debug)]
pub(crate) struct PreparedForeignProfilePreview {
    summary_bytes: Vec<u8>,
    mask: usize,
    preview_stats: Vec<Vec<u16>>,
    effects: [PreparedProfileSummaryRecordEffects; TITLE_PROFILE_SLOT_COUNT],
}

pub(crate) fn prepare_foreign_profile_summary_preview_with(
    active_slots: [bool; TITLE_PROFILE_SLOT_COUNT],
    summary_snapshot: &[u8],
    mut prepare_slot: impl FnMut(
        usize,
        usize,
        Option<&[u8]>,
    ) -> Option<(PreparedProfileSummaryRecordEffects, Vec<u16>)>,
) -> Option<PreparedForeignProfilePreview> {
    if summary_snapshot.len() != PROFILE_SUMMARY_TOTAL_BYTES {
        return None;
    }
    let mut summary_bytes = summary_snapshot.to_vec();
    for slot in 0..TITLE_PROFILE_SLOT_COUNT {
        let start = PROFILE_SUMMARY_RECORD_BASE + slot * PROFILE_SUMMARY_RECORD_STRIDE;
        summary_bytes[start..start + PROFILE_SUMMARY_RECORD_STRIDE].fill(0);
        summary_bytes[PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot] = 0;
    }
    let fallback_slot = (0..TITLE_PROFILE_SLOT_COUNT)
        .find(|slot| summary_snapshot[PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + *slot] != 0);
    let mut mask = 0usize;
    let mut preview_stats = vec![Vec::new(); TITLE_PROFILE_SLOT_COUNT];
    let mut effects = [PreparedProfileSummaryRecordEffects::default(); TITLE_PROFILE_SLOT_COUNT];
    for slot in 0..TITLE_PROFILE_SLOT_COUNT {
        if !active_slots[slot] {
            continue;
        }
        let fallback_src_slot = if summary_snapshot[PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot] != 0
        {
            Some(slot)
        } else {
            fallback_slot
        };
        let fallback = fallback_src_slot.and_then(|src_slot| {
            let start = PROFILE_SUMMARY_RECORD_BASE + src_slot * PROFILE_SUMMARY_RECORD_STRIDE;
            summary_snapshot.get(start..start + PROFILE_SUMMARY_RECORD_STRIDE)
        });
        let prepared_ptr = summary_bytes.as_mut_ptr() as usize;
        let Some((slot_effects, stats)) = prepare_slot(slot, prepared_ptr, fallback) else {
            continue;
        };
        effects[slot] = slot_effects;
        preview_stats[slot] = stats;
        mask |= 1usize << slot;
    }
    (mask != 0).then_some(PreparedForeignProfilePreview {
        summary_bytes,
        mask,
        preview_stats,
        effects,
    })
}

pub(crate) fn commit_prepared_foreign_profile_preview_with(
    state: &std::sync::Mutex<SystemQuitSaveSwapState>,
    identity: &SystemQuitSaveSwapArmIdentity,
    summary: usize,
    summary_snapshot: Vec<u8>,
    prepared: &PreparedForeignProfilePreview,
    apply_summary: impl FnOnce(&[u8]),
) -> bool {
    let mut st = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !st.armed
        || st.committed
        || st.arm_generation != identity.generation
        || st.path != identity.path
        || st.original_hash != identity.original_hash
    {
        return false;
    }
    apply_summary(&prepared.summary_bytes);
    if st.summary_snapshot.is_empty() || st.summary_ptr != summary {
        st.summary_ptr = summary;
        st.summary_snapshot = summary_snapshot;
    }
    st.summary_mutated_generation = identity.generation;
    st.candidate_slot_mask = prepared.mask;
    st.candidate_stats_utf16 = prepared.preview_stats.clone();
    st.preview_applied = true;
    st.rejected_candidate_hash = 0;
    st.rejected_candidate_len = 0;
    st.rejected_candidate_modified_ns = 0;
    st.rejected_candidate_reason = SAVE_SWAP_REJECTION_NONE;
    st.restored_candidate_generation = 0;
    st.restored_candidate_hash = 0;
    st.restored_candidate_len = 0;
    st.restored_candidate_modified_ns = 0;
    #[cfg(test)]
    if system_quit_save_swap_poll_test_active() {
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    true
}
pub(crate) unsafe fn system_quit_apply_foreign_profile_summary_preview(
    base: usize,
    bytes: &[u8],
    identity: &SystemQuitSaveSwapArmIdentity,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let summary = unsafe { system_quit_profile_summary_ptr() };
    if summary == null {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: cannot preview replacement save -- live ProfileSummary unavailable"
        ));
        return 0;
    }
    let summary_snapshot = {
        let st = system_quit_save_swap_lock();
        if st.arm_generation != identity.generation || !st.armed || st.committed {
            return 0;
        }
        if st.summary_ptr == summary && !st.summary_snapshot.is_empty() {
            st.summary_snapshot.clone()
        } else {
            unsafe {
                core::slice::from_raw_parts(summary as *const u8, PROFILE_SUMMARY_TOTAL_BYTES)
                    .to_vec()
            }
        }
    };
    let Ok(active_slots) = er_save_loader::bnd4::active_slots(bytes) else {
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: replacement preview refused -- active-slot bitmap unreadable"
        ));
        return 0;
    };
    let Some(prepared) = prepare_foreign_profile_summary_preview_with(
        active_slots,
        &summary_snapshot,
        |slot, prepared_summary, fallback| {
            #[cfg(test)]
            if system_quit_save_swap_poll_test_active() {
                SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT.fetch_add(1, Ordering::SeqCst);
                unsafe {
                    *((prepared_summary
                        + PROFILE_SUMMARY_RECORD_BASE
                        + slot * PROFILE_SUMMARY_RECORD_STRIDE) as *mut u8) = 0xa0 | slot as u8;
                    *((prepared_summary + PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot) as *mut u8) =
                        1;
                }
                return Some((
                    PreparedProfileSummaryRecordEffects {
                        face_hash: 0x1000 + slot,
                        place_name_unsourced: false,
                    },
                    vec![slot as u16],
                ));
            }
            let body = er_save_loader::bnd4::slot_body(bytes, slot).ok()?;
            let slot_body = SerializedSaveSlot::new(body);
            let pgd = slot_body.player_game_data()?;
            let saved_map = slot_body.saved_map()?;
            let playtime_ticks = slot_body.in_game_timer_ticks(pgd).unwrap_or(0);
            let place_name_id = er_save_loader::profile_summary::slot_place_name_id(bytes, slot);
            let face_bytes = slot_body.face_data_buffer_bytes(pgd);
            let chr_asm_image = slot_body.runtime_chr_asm_image(pgd);
            let effects = unsafe {
                pgd.write_profile_summary_record(
                    base,
                    prepared_summary,
                    slot,
                    saved_map,
                    place_name_id,
                    playtime_ticks,
                    fallback,
                    face_bytes,
                    chr_asm_image.as_ref(),
                )
            }?;
            append_autoload_debug(format_args!(
                "system-quit-load-save-profiles: prepared preview slot {slot} playtime_ticks={playtime_ticks}"
            ));
            Some((effects, pgd.stats_text_utf16().unwrap_or_default()))
        },
    ) else {
        return 0;
    };
    let prepared_caches =
        crate::experiments::startup_hooks::loading_cover::prepare_profile_slot_caches_from_bytes(
            bytes,
        );
    if !commit_prepared_foreign_profile_preview_with(
        system_quit_save_swap_state(),
        identity,
        summary,
        summary_snapshot,
        &prepared,
        |prepared_bytes| unsafe {
            core::ptr::copy_nonoverlapping(
                prepared_bytes.as_ptr(),
                summary as *mut u8,
                prepared_bytes.len(),
            );
        },
    ) {
        return 0;
    }
    let mut unsourced_mask = 0usize;
    for slot in 0..TITLE_PROFILE_SLOT_COUNT {
        PROFILE_PREVIEW_FACE_HASH[slot].store(prepared.effects[slot].face_hash, Ordering::SeqCst);
        if prepared.effects[slot].place_name_unsourced && prepared.mask & (1usize << slot) != 0 {
            unsourced_mask |= 1usize << slot;
        }
    }
    PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.store(unsourced_mask, Ordering::SeqCst);
    let decoded =
        crate::experiments::startup_hooks::loading_cover::apply_prepared_profile_slot_caches(
            prepared_caches,
            "picker-previewed save",
        );
    let reloads = PROFILE_SLOT_CACHE_PREVIEW_RELOADS.fetch_add(1, Ordering::SeqCst) + 1;
    append_autoload_debug(format_args!(
        "system-quit-save-swap: committed value-owned ProfileSummary preview mask=0x{:x}; per-slot caches reloaded ({decoded}/10 slots, reloads={reloads})",
        prepared.mask
    ));
    PROFILE_STATS_PREVIEW_ROW_CURSOR.store(0, Ordering::SeqCst);
    if !system_quit_save_swap_poll_test_active() {
        let refresh: unsafe extern "system" fn() =
            unsafe { std::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
        unsafe { refresh() };
    }
    prepared.mask
}

pub(crate) fn system_quit_save_swap_identity_matches(
    st: &SystemQuitSaveSwapState,
    identity: &SystemQuitSaveSwapArmIdentity,
) -> bool {
    st.arm_generation == identity.generation
        && st.path == identity.path
        && st.original_hash == identity.original_hash
}

pub(crate) fn system_quit_save_swap_take_exact_with(
    state: &std::sync::Mutex<SystemQuitSaveSwapState>,
    identity: &SystemQuitSaveSwapArmIdentity,
) -> Option<SystemQuitSaveSwapState> {
    let mut guard = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !guard.armed || !system_quit_save_swap_identity_matches(&guard, identity) {
        return None;
    }
    let next_generation = guard.next_generation;
    let mut retired = std::mem::take(&mut *guard);
    guard.next_generation = next_generation;
    retired.next_generation = next_generation;
    Some(retired)
}

pub(crate) fn system_quit_save_swap_poll_eligible(st: &SystemQuitSaveSwapState) -> bool {
    st.armed && !st.committed && !st.path.is_empty() && st.arm_generation != 0
}

pub(crate) fn system_quit_save_swap_candidate_rejected(
    st: &SystemQuitSaveSwapState,
    hash: u64,
    len: u64,
    modified_ns: u128,
) -> bool {
    let _ = modified_ns;
    hash != 0 && st.rejected_candidate_hash == hash && st.rejected_candidate_len == len
}

pub(crate) fn system_quit_save_swap_restore_summary_if_mutated_exact(
    st: &mut SystemQuitSaveSwapState,
    identity: &SystemQuitSaveSwapArmIdentity,
) -> bool {
    if !system_quit_save_swap_identity_matches(st, identity)
        || st.summary_mutated_generation != identity.generation
        || st.summary_ptr < 0x10000
        || st.summary_snapshot.is_empty()
    {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            st.summary_snapshot.as_ptr(),
            st.summary_ptr as *mut u8,
            st.summary_snapshot.len(),
        );
    }
    st.summary_mutated_generation = 0;
    st.preview_applied = false;
    st.candidate_slot_mask = 0;
    st.candidate_stats_utf16.clear();
    true
}

fn system_quit_save_swap_retire_exact_restored(
    state: &std::sync::Mutex<SystemQuitSaveSwapState>,
    identity: &SystemQuitSaveSwapArmIdentity,
) -> Option<SystemQuitSaveSwapState> {
    let mut guard = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !system_quit_save_swap_identity_matches(&guard, identity)
        || guard.file_mutated_generation != 0
        || guard.summary_mutated_generation != 0
    {
        return None;
    }
    let next_generation = guard.next_generation;
    let mut retired = std::mem::take(&mut *guard);
    guard.next_generation = next_generation;
    retired.next_generation = next_generation;
    Some(retired)
}

pub(crate) fn system_quit_save_swap_write_original_with(
    st: &SystemQuitSaveSwapState,
    reason: &str,
    write: impl FnOnce(&str, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    if st.path.is_empty() || st.original_bytes.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "save-swap original path/bytes missing",
        ));
    }
    write(&st.path, &st.original_bytes).map_err(|err| {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: FAILED to restore active save file for {reason} path='{}': {err}",
            st.path
        ));
        err
    })?;
    append_autoload_debug(format_args!(
        "system-quit-save-swap: restored active save file for {reason} path='{}' len={} hash=0x{:016x}",
        st.path,
        st.original_bytes.len(),
        st.original_hash
    ));
    Ok(())
}

pub(crate) fn system_quit_save_swap_restore_if_mutated_with(
    st: &mut SystemQuitSaveSwapState,
    identity: &SystemQuitSaveSwapArmIdentity,
    reason: &str,
    write: impl FnOnce(&str, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    if !system_quit_save_swap_identity_matches(st, identity)
        || st.file_mutated_generation == 0
        || st.file_mutated_generation != identity.generation
    {
        return Ok(false);
    }
    system_quit_save_swap_write_original_with(st, reason, write)?;
    st.file_mutated_generation = 0;
    Ok(true)
}

#[derive(Clone, Copy)]
pub(crate) enum ObservedCandidateRecovery {
    RestoreRequired,
    AlreadyRestoredExact,
}

fn system_quit_save_swap_poll_write_original(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(test)]
    if SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_FAIL_NEXT_RESTORE.swap(false, Ordering::SeqCst) {
        return Err(std::io::Error::other(
            "injected production-poll original restore failure",
        ));
    }
    fs::write(path, bytes)
}

fn system_quit_save_swap_restore_observed_candidate_exact(
    identity: &SystemQuitSaveSwapArmIdentity,
    raw_hash: u64,
    len: u64,
    modified_ns: u128,
) -> bool {
    let mut st = system_quit_save_swap_lock();
    if !system_quit_save_swap_identity_matches(&st, identity)
        || !system_quit_save_swap_poll_eligible(&st)
        || (st.file_mutated_generation != 0 && st.file_mutated_generation != identity.generation)
    {
        return false;
    }
    // The active file was replaced externally, but this exact arm now owns recovery of that
    // observed mutation. Publish ownership before the fallible write and clear it only after A is
    // back on disk through the same conditional accounting used by abort/normal restore.
    st.file_mutated_generation = identity.generation;
    if system_quit_save_swap_restore_if_mutated_with(
        &mut st,
        identity,
        "externally-observed-candidate",
        system_quit_save_swap_poll_write_original,
    )
    .is_err()
    {
        st.armed = false;
        SYSTEM_QUIT_SAVE_SWAP_POLL_RESTORE_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-save-swap: FAILED exact recovery for externally observed candidate generation={} path='{}' len={len} hash=0x{raw_hash:016x}; mutation evidence retained and poll disabled for normal-restore retry",
            identity.generation, identity.path
        ));
        return false;
    }
    st.restored_candidate_generation = identity.generation;
    st.restored_candidate_hash = raw_hash;
    st.restored_candidate_len = len;
    st.restored_candidate_modified_ns = modified_ns;
    true
}

pub(crate) fn system_quit_save_swap_reject_observed_candidate_exact(
    identity: &SystemQuitSaveSwapArmIdentity,
    raw_hash: u64,
    len: u64,
    modified_ns: u128,
    reason: usize,
    recovery: ObservedCandidateRecovery,
) -> bool {
    if matches!(recovery, ObservedCandidateRecovery::RestoreRequired)
        && !system_quit_save_swap_restore_observed_candidate_exact(
            identity,
            raw_hash,
            len,
            modified_ns,
        )
    {
        return false;
    }
    let mut st = system_quit_save_swap_lock();
    if !system_quit_save_swap_identity_matches(&st, identity)
        || !system_quit_save_swap_poll_eligible(&st)
        || st.file_mutated_generation != 0
        || st.restored_candidate_generation != identity.generation
        || st.restored_candidate_hash != raw_hash
        || st.restored_candidate_len != len
        || st.restored_candidate_modified_ns != modified_ns
    {
        return false;
    }
    st.rejected_candidate_hash = raw_hash;
    st.rejected_candidate_len = len;
    st.rejected_candidate_modified_ns = modified_ns;
    st.rejected_candidate_reason = reason;
    SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_COUNT.fetch_add(1, Ordering::SeqCst);
    SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_LAST_REASON.store(reason, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-save-swap: rejected externally observed candidate generation={} path='{}' len={len} hash=0x{raw_hash:016x} modified_ns={modified_ns} reason={reason}; original A restored and identical bytes are poll-inert",
        identity.generation, identity.path
    ));
    true
}

/// Is a FOREIGN save's summary currently on screen (previewed, not yet committed)?
///
/// The row presentation needs this to answer one question correctly: whose name belongs on slot 0.
/// The transient current-player row is built with slot index 0 (`FUN_1408753f0` ->
/// `FUN_1408759e0(summary, 0, &name, pgd->level)`), so slot 0 normally prefers the LIVE character's
/// name. While a foreign save is previewed, slot 0 is that save's slot 0 instead, and preferring the
/// live name puts the loaded character's name on another save's character -- observed 2026-08-07 as
/// "Maddened Bean, RL 100" where RL 100, the attributes and the location were all angrE's.
pub(crate) fn system_quit_foreign_preview_active() -> bool {
    let st = system_quit_save_swap_lock();
    st.preview_applied && !st.committed
}

pub(crate) unsafe fn system_quit_save_swap_abort_exact(
    identity: &SystemQuitSaveSwapArmIdentity,
    reason: &str,
) -> bool {
    let _operation = system_quit_save_swap_operation_lock();
    let summary_restored = {
        let mut st = system_quit_save_swap_lock();
        if !system_quit_save_swap_identity_matches(&st, identity) {
            append_autoload_debug(format_args!(
                "system-quit-save-swap: stale abort ignored generation={} path='{}' reason={reason}",
                identity.generation, identity.path
            ));
            return false;
        }
        system_quit_save_swap_restore_summary_if_mutated_exact(&mut st, identity)
    };
    if summary_restored {
        if !system_quit_save_swap_poll_test_active()
            && let Ok(base) = game_module_base()
        {
            let refresh: unsafe extern "system" fn() =
                unsafe { std::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
            unsafe { refresh() };
        }
        crate::experiments::startup_hooks::loading_cover::invalidate_profile_slot_caches(reason);
        for slot in 0..TITLE_PROFILE_SLOT_COUNT {
            PROFILE_PREVIEW_FACE_HASH[slot].store(0, Ordering::SeqCst);
        }
        PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.store(0, Ordering::SeqCst);
        clear_picker_presentation();
    }
    let file_restored = {
        let mut st = system_quit_save_swap_lock();
        if !system_quit_save_swap_identity_matches(&st, identity) {
            return false;
        }
        match system_quit_save_swap_restore_if_mutated_with(
            &mut st,
            identity,
            reason,
            |path, bytes| fs::write(path, bytes),
        ) {
            Ok(restored) => restored,
            Err(_) => {
                // Keep exact path/original bytes/mutation generation as retry evidence, but make the
                // failed generation poll-inert. A later normal restore retries the write.
                st.armed = false;
                append_autoload_debug(format_args!(
                    "system-quit-save-swap: exact abort retained generation={} restore evidence after write failure; poll disabled until retry",
                    identity.generation
                ));
                return false;
            }
        }
    };
    let retired =
        system_quit_save_swap_retire_exact_restored(system_quit_save_swap_state(), identity)
            .is_some();
    append_autoload_debug(format_args!(
        "system-quit-save-swap: aborted exact arm generation={} path='{}' reason={reason} summary_restored={summary_restored} file_restored={file_restored} retired={retired}",
        identity.generation, identity.path
    ));
    retired
}

pub(crate) struct SystemQuitSaveSwapArmGuard {
    identity: Option<SystemQuitSaveSwapArmIdentity>,
}

impl SystemQuitSaveSwapArmGuard {
    pub(crate) fn commit(mut self) -> SystemQuitSaveSwapArmIdentity {
        self.identity
            .take()
            .expect("save-swap arm guard commits at most once")
    }
}

impl Drop for SystemQuitSaveSwapArmGuard {
    fn drop(&mut self) {
        if let Some(identity) = self.identity.take() {
            unsafe { system_quit_save_swap_abort_exact(&identity, "initial-picker-open-abort") };
        }
    }
}

pub(crate) fn system_quit_save_swap_arm_original_transaction(
    path: &str,
) -> Option<SystemQuitSaveSwapArmGuard> {
    system_quit_save_swap_arm_original_identity(path).map(|identity| SystemQuitSaveSwapArmGuard {
        identity: Some(identity),
    })
}

pub(crate) unsafe fn system_quit_save_swap_restore_profile_summary(reason: &str) {
    let _operation = system_quit_save_swap_operation_lock();
    let identity = {
        let st = system_quit_save_swap_lock();
        if st.committed || st.arm_generation == 0 {
            return;
        }
        SystemQuitSaveSwapArmIdentity {
            generation: st.arm_generation,
            path: st.path.clone(),
            original_hash: st.original_hash,
        }
    };
    let summary_restored = {
        let mut st = system_quit_save_swap_lock();
        system_quit_save_swap_restore_summary_if_mutated_exact(&mut st, &identity)
    };
    if summary_restored {
        if !system_quit_save_swap_poll_test_active()
            && let Ok(base) = game_module_base()
        {
            let refresh: unsafe extern "system" fn() =
                unsafe { std::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
            unsafe { refresh() };
        }
        crate::experiments::startup_hooks::loading_cover::invalidate_profile_slot_caches(reason);
        for slot in 0..TITLE_PROFILE_SLOT_COUNT {
            PROFILE_PREVIEW_FACE_HASH[slot].store(0, Ordering::SeqCst);
        }
        PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.store(0, Ordering::SeqCst);
        clear_picker_presentation();
    }
    let file_result = {
        let mut st = system_quit_save_swap_lock();
        if !system_quit_save_swap_identity_matches(&st, &identity) {
            return;
        }
        system_quit_save_swap_restore_if_mutated_with(&mut st, &identity, reason, |path, bytes| {
            fs::write(path, bytes)
        })
    };
    if file_result.is_err() {
        let mut st = system_quit_save_swap_lock();
        if system_quit_save_swap_identity_matches(&st, &identity) {
            st.armed = false;
        }
        return;
    }
    let _ = system_quit_save_swap_retire_exact_restored(system_quit_save_swap_state(), &identity);
}

pub(crate) unsafe fn system_quit_save_swap_poll_preview(base: usize) {
    let tick = SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.fetch_add(1, Ordering::SeqCst);
    if tick % SYSTEM_QUIT_SAVE_SWAP_POLL_INTERVAL_TICKS != 0 {
        return;
    }
    // Abort/restore/arm serialize with the entire poll, not just its first state read. Therefore an
    // abort that retires a generation cannot be followed by a poll that already cloned its path.
    let _operation = system_quit_save_swap_operation_lock();
    let (
        identity,
        original_len,
        original_modified_ns,
        preview_applied,
        rejected_hash,
        rejected_len,
        rejected_modified_ns,
        rejected_reason,
    ) = {
        let st = system_quit_save_swap_lock();
        if !system_quit_save_swap_poll_eligible(&st) {
            return;
        }
        (
            SystemQuitSaveSwapArmIdentity {
                generation: st.arm_generation,
                path: st.path.clone(),
                original_hash: st.original_hash,
            },
            st.original_len,
            st.original_modified_ns,
            st.preview_applied,
            st.rejected_candidate_hash,
            st.rejected_candidate_len,
            st.rejected_candidate_modified_ns,
            st.rejected_candidate_reason,
        )
    };
    let path = identity.path.clone();
    let original_hash = identity.original_hash;
    if preview_applied {
        return;
    }
    let Some((len, modified_ns)) = system_quit_file_stamp(&path) else {
        return;
    };
    if len == original_len && modified_ns == original_modified_ns {
        return;
    }
    let Ok(mut bytes) = fs::read(&path) else {
        return;
    };
    let raw_hash = system_quit_hash_bytes(&bytes);
    if raw_hash == original_hash {
        return;
    }
    if system_quit_save_swap_candidate_rejected(
        &SystemQuitSaveSwapState {
            rejected_candidate_hash: rejected_hash,
            rejected_candidate_len: rejected_len,
            rejected_candidate_modified_ns: rejected_modified_ns,
            ..SystemQuitSaveSwapState::default()
        },
        raw_hash,
        len,
        modified_ns,
    ) {
        SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_SUPPRESSED_COUNT.fetch_add(1, Ordering::SeqCst);
        let _ = system_quit_save_swap_reject_observed_candidate_exact(
            &identity,
            raw_hash,
            len,
            modified_ns,
            rejected_reason,
            ObservedCandidateRecovery::RestoreRequired,
        );
        return;
    }
    SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
    if er_save_loader::bnd4::parse_entries(&bytes).is_err() {
        SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        let _ = system_quit_save_swap_reject_observed_candidate_exact(
            &identity,
            raw_hash,
            len,
            modified_ns,
            SAVE_SWAP_REJECTION_PARSE_FAILURE,
            ObservedCandidateRecovery::RestoreRequired,
        );
        return;
    }
    if !system_quit_save_swap_poll_test_active() {
        normalize_save_bytes_to_active_steam_id(base, &mut bytes, "system-quit-polled-candidate");
    }
    let hash = system_quit_hash_bytes(&bytes);
    if !system_quit_save_swap_restore_observed_candidate_exact(
        &identity,
        raw_hash,
        len,
        modified_ns,
    ) {
        return;
    }
    let mask =
        unsafe { system_quit_apply_foreign_profile_summary_preview(base, &bytes, &identity) };
    if mask == 0 {
        SYSTEM_QUIT_SAVE_SWAP_POLL_ZERO_SLOT_COUNT.fetch_add(1, Ordering::SeqCst);
        let _ = system_quit_save_swap_reject_observed_candidate_exact(
            &identity,
            raw_hash,
            len,
            modified_ns,
            SAVE_SWAP_REJECTION_ZERO_READABLE_SLOTS,
            ObservedCandidateRecovery::AlreadyRestoredExact,
        );
        return;
    }
    let mut st = system_quit_save_swap_lock();
    if !system_quit_save_swap_identity_matches(&st, &identity) {
        return;
    }
    st.candidate_bytes = bytes;
    st.candidate_hash = hash;
    append_autoload_debug(format_args!(
        "system-quit-save-swap: applied FOREIGN ProfileSummary preview from replacement path='{path}' len={len} hash=0x{hash:016x} slot_mask=0x{mask:x}; active save file restored until the user selects a foreign slot"
    ));
}

pub(crate) unsafe fn system_quit_save_swap_prepare_selected_slot(slot: i32) -> Result<bool, ()> {
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: prepare selected slot skipped -- out-of-range slot={slot}"
        ));
        return Ok(false);
    }
    let mut st = system_quit_save_swap_lock();
    if !st.preview_applied || st.committed {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: prepare selected slot skipped slot={slot} preview_applied={} committed={} armed={} path_set={} candidate_len={} mask=0x{:x}",
            st.preview_applied,
            st.committed,
            st.armed,
            !st.path.is_empty(),
            st.candidate_bytes.len(),
            st.candidate_slot_mask
        ));
        return Ok(false);
    }
    let bit = 1usize << slot as usize;
    if st.candidate_slot_mask & bit == 0 {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: refusing ProfileSelect activation for slot {slot}; foreign preview active but slot bit is absent mask=0x{:x}",
            st.candidate_slot_mask
        ));
        return Err(());
    }
    if st.path.is_empty() || st.candidate_bytes.is_empty() {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: refusing ProfileSelect activation for slot {slot}; foreign preview state incomplete path_set={} candidate_len={} mask=0x{:x}",
            !st.path.is_empty(),
            st.candidate_bytes.len(),
            st.candidate_slot_mask
        ));
        return Err(());
    }
    match system_quit_save_swap_write_candidate_with(&mut st, |path, bytes| {
        write_save_bytes_for_overwrite(path, bytes)
    }) {
        Ok(()) => {
            st.committed = true;
            st.recommitted = false;
            st.armed = false;
            append_autoload_debug(format_args!(
                "system-quit-save-swap: committed foreign save before slot activation path='{}' slot={slot} len={} hash=0x{:016x}; fresh deserialize will read this file",
                st.path,
                st.candidate_bytes.len(),
                st.candidate_hash
            ));
            Ok(true)
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "system-quit-save-swap: FAILED to commit foreign save for slot {slot} path='{}': {err}; blocking activation to avoid loading stale/original bytes",
                st.path
            ));
            Err(())
        }
    }
}

pub(crate) fn make_save_file_writable_for_overwrite(path: &str) {
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
}

pub(crate) fn write_save_bytes_for_overwrite(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    make_save_file_writable_for_overwrite(path);
    fs::write(path, bytes)
}

pub(crate) fn system_quit_save_swap_write_candidate_with(
    st: &mut SystemQuitSaveSwapState,
    write: impl FnOnce(&str, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    if st.arm_generation == 0 || st.path.is_empty() || st.candidate_bytes.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "save-swap candidate write lacks exact generation/path/bytes",
        ));
    }
    write(&st.path, &st.candidate_bytes)?;
    st.file_mutated_generation = st.arm_generation;
    Ok(())
}

/// Re-commit the foreign candidate bytes AFTER the game's return-title save completes (bc4 terminal).
/// The activation-time commit is CLOBBERED by that save whenever the picked slot shares the ACTIVE
/// character's slot index: the return-title chain sets saveRequested and the game re-writes the active
/// slot (+ profile summary) into the active file ~400ms after our write (gm-snap: bc4 1 -> save_state=1
/// -> bc4 terminal), so a same-slot switch fresh-deserialized the ORIGINAL character (user-reported
/// 2026-07-06, run seamless-save-smoke-20260706-144801: two same-slot-0 picks both reloaded the
/// resident character; FACE-IDENTITY MISMATCH #1 confirmed it at RAM level before the pixels did).
/// Different-slot switches always survived because the clobber only rewrites the active slot's
/// USER_DATA entry. By bc4-terminal the save write has finished, and the fresh deserialize is still
/// seconds away at the clean title, so a second write of the pristine candidate bytes wins. Nothing
/// meaningful is lost: System-Quit already saved the old character into their own file before
/// ProfileSelect opened; the return-title re-save was landing in the WRONG (foreign) file anyway.
/// Idempotent per switch via the `recommitted` latch (the terminal block can re-enter when the final
/// functor submit defers).
pub(crate) fn system_quit_save_swap_recommit_after_return_title_save() {
    let mut st = system_quit_save_swap_lock();
    if !st.committed || st.recommitted || st.path.is_empty() || st.candidate_bytes.is_empty() {
        return;
    }
    match system_quit_save_swap_write_candidate_with(&mut st, |path, bytes| {
        write_save_bytes_for_overwrite(path, bytes)
    }) {
        Ok(()) => {
            st.recommitted = true;
            append_autoload_debug(format_args!(
                "system-quit-save-swap: RE-committed foreign save after return-title save (bc4 terminal) path='{}' len={} hash=0x{:016x}; the game's return-title save had re-written the ACTIVE slot over the activation-time commit",
                st.path,
                st.candidate_bytes.len(),
                st.candidate_hash
            ));
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "system-quit-save-swap: FAILED to re-commit foreign save after return-title save path='{}': {err}; a same-slot switch will fresh-deserialize the clobbered ACTIVE slot",
                st.path
            ));
        }
    }
}

/// The game-owned save file a Load-Save-Profiles pick has COMMITTED foreign character bytes into this
/// switch, or `None` when no runtime foreign pick is active (normal boot / config-only autoload).
///
/// When the human-driven "Load Save Profiles" path activates a foreign slot,
/// `system_quit_save_swap_prepare_selected_slot` overwrites the ACTIVE `%APPDATA%/EldenRing/<steamid>/
/// ER0000.{sl2,co2}` file (`st.path` -- the game-owned default, NEVER the read-only picked source or
/// the configured `save_file`) with the picked slot's candidate bytes and sets `committed = true`. The
/// own-load feed uses this to read the COMMITTED file instead of the configured `save_file` for that
/// pick's load (drive.rs `own_load_read_sl2_bytes`): a runtime pick overrides the config default for
/// exactly one load. Returns `None` unless the commit actually landed AND the path/candidate are still
/// present, so a normal boot autoload (no pick) still reads the configured `save_file` unchanged.
pub(crate) fn system_quit_committed_foreign_save_path() -> Option<String> {
    let st = system_quit_save_swap_lock();
    if st.committed && !st.path.is_empty() && !st.candidate_bytes.is_empty() {
        Some(st.path.clone())
    } else {
        None
    }
}

/// Patch the target slot's profile offscreen RT size BEFORE any post-Continue profile renderer is
/// constructed. The constructor snapshots this table; patching after `PROFILE_TABLE_BUILDER_RVA` runs is
/// too late and produces the 256x256 loading-screen portrait (Bug A). Returns true only when the target
/// slot is known and its row is confirmed at the configured target size.
///
/// TARGET SLOT, NOT LOADED SLOT (2026-07-30, deterministic different-slot no-portrait root cause).
/// During a System->Quit->Load-Profile switch the confirmed loaded slot still names the OLD character,
/// whose row was already patched at boot -- so this function silently early-returned true while the
/// NEWLY-selected slot's row stayed native 128 (x2 supersample = the observed 256x256 capture, run
/// 20260730-202840: kick #2/#3 renderers for slot 1 both built 256 while `portrait-res` only logged
/// slot 2 at boot and slot 1 too late at +41246ms, after both builds). Every switch-window capture was
/// then small -> pixelated when published, rejected (no portrait at all) once the small-capture gate
/// landed. Resolve the row for the SELECTED slot as soon as the switch names it, so every later build
/// (our loading-owned rebuild AND the native mid-window TitleTopDialog rebuild) constructs the target
/// RT at full size.
pub(crate) unsafe fn patch_profile_offscreen_size_for_loaded_slot(base: usize) -> bool {
    if !portrait_real_pixels_enabled() {
        return true;
    }
    let target = if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
    {
        portrait_target_slot()
    } else {
        let Some(loaded) = portrait_loaded_slot_confirmed() else {
            return false;
        };
        loaded
    };
    unsafe { patch_profile_offscreen_size_for_slot(base, target) }
}

/// Row-level worker for [`patch_profile_offscreen_size_for_loaded_slot`]; also called eagerly from the
/// switch retarget (`portrait_retarget_and_rearm_for_switch`) the moment the selected slot is known.
/// Idempotent per slot via `PROFILE_SIZE_PATCHED`.
pub(crate) unsafe fn patch_profile_offscreen_size_for_slot(base: usize, target: i32) -> bool {
    if !portrait_real_pixels_enabled() {
        return true;
    }
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&target) {
        return false;
    }
    let bit = 1usize << (target as usize);
    if PROFILE_SIZE_PATCHED.load(Ordering::SeqCst) & bit != 0 {
        return true;
    }
    let table = base + PROFILE_OFFSCREEN_SIZE_TABLE_RVA;
    let row = table + target as usize * PROFILE_OFFSCREEN_SIZE_TABLE_STRIDE;
    let cur = unsafe { safe_read_usize(row) }.unwrap_or(0);
    let patched = if cur == PROFILE_OFFSCREEN_SIZE_TARGET {
        true
    } else if cur == PROFILE_OFFSCREEN_SIZE_INIT {
        unsafe {
            core::ptr::write_volatile(row as *mut u64, PROFILE_OFFSCREEN_SIZE_TARGET as u64);
            core::ptr::write_volatile(
                (row + PROFILE_OFFSCREEN_SIZE_SUPERSAMPLE_FLAG_OFFSET) as *mut u8,
                0,
            );
        }
        true
    } else {
        false
    };
    if patched {
        PROFILE_SIZE_PATCHED.fetch_or(bit, Ordering::SeqCst);
    }
    let target_w = PROFILE_OFFSCREEN_SIZE_TARGET & 0xffff_ffff;
    let target_h = (PROFILE_OFFSCREEN_SIZE_TARGET >> 32) & 0xffff_ffff;
    append_autoload_debug(format_args!(
        "portrait-res: pre-builder target slot {target} row=0x{cur:x} patched={} -> base {target_w}x{target_h}, native supersample off (expected RT {target_w}x{target_h}); other slots left native 128",
        if patched { 1 } else { 0 }
    ));
    patched
}

pub(crate) unsafe fn force_profile_render_tick(base: usize, _slot: i32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    // RE-ENGAGE on every loading screen (subsequent-character-load fix): pause the build pipeline ONLY
    // during active gameplay, not permanently after the first world -- so a System Quit character switch's
    // loading screen re-builds + re-captures the NEW character's portrait.
    // NATIVE PORTRAIT (2026-07-15): but keep running while the native NOW-LOADING screen is actively
    // rendering, even after PlayerIns resolves -- IN_WORLD_REACHED flips ~1.7s early on a fast load, so
    // portrait_pipeline_idle_in_gameplay went true mid-load and this tick returned before building the table
    // (run32: force_profile_render_tick never reached maybe_build). The model must build + render DURING the
    // loading screen we own, so gate the idle return on the native loading screen being gone.
    if unsafe { portrait_pipeline_idle_in_gameplay(base) } && !native_loading_screen_active() {
        return;
    }
    let valid = |p: usize| p != 0 && p != null;
    // POST-CONTINUE PORTRAIT: before the table-ready guard below (which would early-return on the
    // torn-down post-Continue table), repopulate the table during now-loading so the rest of this tick
    // (mark+refresh feed) and the draw/oracle run on the loading screen.
    unsafe { maybe_build_profile_table_for_loading(base) };
    // Build the player-stats text bitmap (game menu font) once the stats + font are readable, for the
    // loading cover to composite alongside the portrait in place of the native tips.
    if portrait_overlay_enabled() {
        unsafe { maybe_build_stats_text() };
    }
    // Product source ownership: the pre-Continue/ProfileSelect renderer is not our loading portrait
    // source. Ignore it completely (no kick, no spare candidate, no bake-capture/dump) until the
    // loading-screen-owned table has been built by maybe_build_profile_table_for_loading above.
    if PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) == 0 {
        return;
    }
    // ProfileSummary = GameDataMan -> slot-manager container.
    let gdm = game_data_man_ptr_or_null();
    if !valid(gdm) {
        return;
    }
    let summary = unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(0);
    if !valid(summary) {
        return;
    }
    // SLOT->NAME dump, once per run (er-effects-rs-hi2 attribution): the anomaly hypothesis is
    // character-specific (Patches' boot/menu-path lifecycle differs on reload), so per-window
    // anomalies must be joinable to WHICH character each retarget slot holds -- readable here from
    // the ProfileSummary records the pipeline already uses.
    if PROFILE_SLOT_NAMES_DUMPED.load(Ordering::SeqCst) == 0 {
        // Only consume the one-shot once at least one REAL name is readable: this runs before the
        // boot ProfileSummary save read (~+16s), and latching on the pre-read table logged ten
        // "(empty)" slots (run 2026-07-03 ~21:14). Keep retrying until the records are populated.
        let mut names: Vec<String> = Vec::with_capacity(TITLE_PROFILE_SLOT_COUNT);
        let mut any_real = false;
        for s in 0..TITLE_PROFILE_SLOT_COUNT {
            let rec = summary + PROFILE_SUMMARY_RECORD_BASE + s * PROFILE_SUMMARY_RECORD_STRIDE;
            let (units, len) = unsafe { read_utf16_name_units(rec) };
            let name = if utf16_name_empty_like(&units, len) {
                "(empty)".to_owned()
            } else {
                any_real = true;
                String::from_utf16_lossy(&units[..len])
            };
            names.push(format!("{s}={name}"));
        }
        if any_real {
            PROFILE_SLOT_NAMES_DUMPED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!("profile-slot-names: {}", names.join(" ")));
        }
    }
    // GUARD (crash fix): only call refresh once the renderer table is LIVE -- it is populated at
    // TitleTopDialog ctor (main menu), NOT at early title. Calling refresh before the table exists
    // AVs inside refresh (observed crash rva 0x9aa6d4 = refresh+0x54 at +8939ms). Require slot-0's
    // table entry to be a valid CSMenuProfModelRend before marking/refreshing.
    let probe = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    if !valid(probe)
        || unsafe { safe_read_usize(probe) }.unwrap_or(0)
            != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return;
    }
    // IMMEDIATE BUILD KICK (regression fix -- goal issue 1, grounded in the 06-29 vs 06-30 capture diff):
    // the 240-tick / feed cadence below can fire BEFORE the native boot ProfileSummary read makes the
    // autoload target slot real (~+17s). When it does, the mark loop marks 0 real slots, refresh requests
    // nothing, the renderer's +0x754 "load-requested" latch stays 0, and the model never builds in the
    // brief now-loading window -> nothing to capture (06-30 runs: req754=0 req755=0 model=0x0). 06-29 runs
    // that captured a portrait marked the slot WHILE refresh ran (req755=1 -> model=0x<nonzero>); the
    // all-slots-mark removal (correctly gated on a real fingerprint to avoid contaminating empty slots'
    // saveSlotsStates) lost that build-request for slot 0 because the cadence no longer coincides with the
    // moment the slot goes real. So here, edge-triggered: the instant a slot's fingerprint is real AND its
    // renderer's +0x754 is still 0, mark + refresh it immediately (off-cadence) and open the feed window to
    // drive the async build to completion. Idempotent -- once +0x754 latches to 1 this no-ops, so no churn.
    // Only marks REAL slots (post-read), identical to the cadence loop's gate, so it can't pre-empt the read.
    // ONLY THE TARGET SLOT (2026-07-30): during System->Quit->Load Profile, `GameMan.save_slot`
    // still names the old resident character when the loading-cover portrait must build the newly
    // selected incoming row. The 19:54 softlock repro proved the drift: target slot=1 but this path
    // kicked "LOADED slot 2", so the first other-slot load displayed no matching profile render. Use the
    // selected switch target when present, otherwise fall back to the confirmed loaded slot for boot/load1.
    let target_slot = if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
    {
        portrait_target_slot()
    } else {
        let Some(loaded) = portrait_loaded_slot_confirmed() else {
            return;
        };
        loaded
    };
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&target_slot) {
        return;
    }
    // FAIL-FAST SEMAPHORE: only compare against the live loaded character once a LOAD HAS ACTUALLY
    // COMPLETED. The old gate was `portrait_loaded_slot_confirmed() == Some(target_slot)` -- i.e.
    // it asked ac0 (`GameMan.save_slot`) whether the target slot was resident. That gate is
    // defeated by our OWN write: `own_load/loaders.rs` calls the native `SetSaveSlot(picked)`
    // (Ghidra 0x14067a810 -- a pure field store with no load semantics) before submitting, ~5s
    // before the deserialize. Three milliseconds later ac0 already equalled the target, the gate
    // opened, and the semaphore compared our incoming record against the character still resident
    // from the PREVIOUS session. Gate on the deserialize instead:
    //   * switch: the picked slot's fresh deserialize completed;
    //   * boot:   c30 is a real saved map (not the m10 new-game default) AND the native slot
    //             request register (`GameMan+0xb78`) is back at its no-request sentinel, i.e. no
    //             load is still in flight.
    let deserialize_completed = if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
    {
        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 1
    } else {
        let gm = game_man_ptr_or_null();
        gm != TITLE_OWNER_SCAN_START_ADDRESS
            && unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }
                .is_some_and(|c30| c30 != FULLREAD_C30_M10_DEFAULT && c30 != 0)
            && unsafe { safe_read_i32(gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) }
                .is_some_and(|b78| b78 < 0)
    };
    if deserialize_completed {
        unsafe { portrait_render_slot_semaphore(base, target_slot) };
    }
    // ARMOR-RESOLUTION oracle (bd er-effects-rs-91l5 Layer 1). Every tick, read the LIVE stage-0
    // ChrAsm of the renderer this tick is driving and publish the four EquipParamProtector rows the
    // model build will actually request. It reads the SAME `target_slot` the kick and the bake capture
    // use, so it can never score a renderer other than the displayed one. Read-only, fault-guarded,
    // and it re-resolves the pool pointer itself on every call.
    unsafe { portrait_equip_oracle_sample(base, summary, target_slot) };
    {
        let mark: unsafe extern "system" fn(usize, i32) -> u8 =
            unsafe { core::mem::transmute(base + PROFILE_MARK_SLOT_USED_RVA) };
        let mut kicked = 0u32;
        let mut kicked_mask = 0u32;
        for s in 0..10i32 {
            // ONE SLOT (GX-overflow revert): immediate-kick only the target (see cadence loop).
            if s != target_slot {
                continue;
            }
            if !unsafe { profile_slot_fingerprint(s).0 } {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if !valid(r)
                || unsafe { safe_read_usize(r) }.unwrap_or(0)
                    != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                continue;
            }
            // +0x754 = the refresh's "load-requested" idempotency latch. 0 = the async model build was
            // never kicked for this slot -> kick it now. Non-zero -> already requested, skip.
            if unsafe { safe_read_u8(r + 0x754) }.unwrap_or(0xff) != 0 {
                continue;
            }
            let _ = unsafe { mark(summary, s) };
            // PER-SLOT kick replica (not the engine's GLOBAL refresh, which would kick EVERY marked
            // slot and build all the save's characters mid-load -> the cross-slot portrait swap).
            if unsafe { kick_target_profile_slot(base, summary, r, s) } {
                kicked += 1;
                kicked_mask |= 1 << s;
            }
        }
        if kicked > 0 {
            // Drive the freshly-requested build to completion + keep it latched through the loading screen.
            PROFILE_LOADSCREEN_FEED_TICKS
                .store(PROFILE_LOADSCREEN_FEED_WINDOW_TICKS, Ordering::SeqCst);
            if PROFILE_REAL_SLOT_KICK_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                append_autoload_debug(format_args!(
                    "force-profile-render: IMMEDIATE build kick -- {kicked} real slot(s) (mask=0x{kicked_mask:x}) became available with req754=0; marked + per-slot kicked off-cadence + opened feed window (summary=0x{summary:x})"
                ));
            }
        }
    }
    // MODEL BUILD: every ~240 ticks, mark all 10 profile slots used + call the refresh that kicks the
    // async character-model build. refresh is IDEMPOTENT per slot via the +0x754 "load-requested" latch,
    // so by default this builds each model ONCE and then leaves it -- the model stays LIVE every frame,
    // which is what the realtime look-at draw needs (an invalid/rebuilding pose-holder fails the draw).
    //
    // DESTRUCTIVE REBUILD (default OFF, `portrait_force_rebuild_enabled`): clear each build latch
    // (+0x754/+0x755) + reset the look-at slot cache to force a FRESH build. The churn leaves models
    // not-live most of the time (~88% draw failures -> flicker), so it is opt-in: flip it on briefly to
    // re-capture the post-FaceData face (an early build before LOAD GAME loads FaceData = default head),
    // then off. See `portrait_force_rebuild_enabled` and bd portrait-lookat-realtime-drawphase-design.
    let counter = PROFILE_FORCE_TICK_COUNTER.fetch_add(1, Ordering::SeqCst);
    // Post-Continue feed window: while it is open, run the (idempotent) mark+refresh every 8 ticks so the
    // freshly-built renderers' async model build is driven to completion and stays latched -- the once-per-
    // 240 baseline is too sparse for the brief now-loading window. Outside the window keep the 240 cadence.
    let feed_window = PROFILE_LOADSCREEN_FEED_TICKS.load(Ordering::SeqCst) > 0;
    if feed_window {
        PROFILE_LOADSCREEN_FEED_TICKS.fetch_sub(1, Ordering::SeqCst);
    }
    if counter % 240 == 0 || (feed_window && counter % 8 == 0) {
        let log_this = counter % 240 == 0; // throttle the in-window feed log to once per 240
        let force_rebuild = portrait_force_rebuild_enabled();
        let mark: unsafe extern "system" fn(usize, i32) -> u8 =
            unsafe { core::mem::transmute(base + PROFILE_MARK_SLOT_USED_RVA) };
        let mut marked = 0u32;
        for s in 0..10i32 {
            // ONE SLOT (GX-overflow revert, user 2026-07-03): build ONLY the autoload target. Rendering
            // every saved slot overran the 192-slot GX command queue (0x1aeaf05 null-slot-write crash) --
            // 10 concurrent live renderers' draw tasks, independent of RT size (target-only 1024 didn't
            // help). All-slots menu portraits will be handled at the GFX/surface layer (remove the
            // DummyProfileFace surface) instead of driving all 10 renderers.
            if s != target_slot {
                continue;
            }
            // Real-character gate (per the native boot ProfileSummary read: level>=1 + non-empty name).
            // Never mark before the read populates the slot (can't pre-empt it / contaminate saveSlotsStates).
            if !unsafe { profile_slot_fingerprint(s).0 } {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            let r_valid = valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
            if force_rebuild && r_valid {
                unsafe {
                    core::ptr::write_volatile((r + 0x754) as *mut u8, 0);
                    core::ptr::write_volatile((r + 0x755) as *mut u8, 0);
                }
            }
            let _ = unsafe { mark(summary, s) };
            // PER-SLOT kick replica in place of the engine's GLOBAL refresh: the global form kicked
            // every marked slot (all the save's characters) -- the cross-slot portrait swap source.
            // Idempotent via the +0x754/+0x755 gate inside, so the feed cadence just re-tries until
            // the record is real and then no-ops.
            if r_valid {
                let _ = unsafe { kick_target_profile_slot(base, summary, r, s) };
            }
            marked += 1;
        }
        // TRIPWIRE oracle: count non-target renderers holding a live model during our feed window.
        // Expected 0 with one-slot render -- any foreign live model is the swap-bug precondition.
        let mut foreign = 0usize;
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            if s == target_slot {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
                && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0)
                    != 0
            {
                foreign += 1;
            }
        }
        PROFILE_FOREIGN_MODELS_MAX.fetch_max(foreign, Ordering::SeqCst);
        if log_this {
            append_autoload_debug(format_args!(
                "force-profile-render: build cycle (counter={counter}) force_rebuild={force_rebuild} feed_window={feed_window} -- marked {marked} real slot(s) + per-slot kicked (summary=0x{summary:x} foreign_models={foreign})"
            ));
        }
        // Only when we forced a fresh build: drop the cached look-at indices/base so they re-resolve and
        // re-latch the idle base from the fresh skeleton. Without a forced rebuild the model (and its
        // skeleton) persist, so KEEP the cache -> the look-at keeps driving every frame with no re-resolve gap.
        if force_rebuild {
            match PROFILE_LOOKAT_SLOTS.lock() {
                Ok(mut g) => *g = [None; 10],
                Err(p) => *p.into_inner() = [None; 10],
            }
            match PROFILE_CAM_FACE_YAW.lock() {
                Ok(mut g) => *g = [None; 10],
                Err(p) => *p.into_inner() = [None; 10],
            }
            PROFILE_CAM_FACE_YAW_LATCHED_MASK.store(0, Ordering::SeqCst);
            // The models are being rebuilt -> the cached PoseHolder pointers are about to go stale. Drop
            // them so they re-resolve against the fresh skeletons (and re-latch a clean base) before the
            // sticky-keep path above starts driving them again.
            for h in PROFILE_LOOKAT_HOLDERS.iter() {
                h.store(0, Ordering::SeqCst);
            }
        }
    }
    // ~80 ticks AFTER each rebuild kick, reset the dump mask so the freshly-rebuilt models (not the
    // stale pre-clear model_ins) get re-dumped. Each cycle's dumps overwrite the per-slot files.
    if counter % 240 == 80 {
        PROFILE_SLOT_DUMP_MASK.store(0, Ordering::SeqCst);
    }
    // CAMERA LEVER: every tick, override each live renderer's orbit camera with our custom viewport.
    // Re-applied so a refresh that re-runs the engine camera setup can't win; the dump loop below then
    // captures the custom-framed RT. Gated under the same `portrait_real_pixels` diagnostic as the dump.
    if portrait_real_pixels_enabled() {
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                unsafe { apply_profile_camera_override(base, r, s) };
            }
        }
    }
    // Cursor/head tracking is intentionally retired. Keep the loading portrait renderer alive and
    // refreshed, but do not rotate character bones toward the mouse. Still pre-record the target
    // renderer as the teardown spare once its model is built so the loading portrait survives Continue.
    for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
        let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
        if valid(r)
            && unsafe { safe_read_usize(r) }.unwrap_or(0)
                == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
        {
            let target = portrait_target_slot();
            if s == target
                && PROFILE_SPARE_CANDIDATE.load(Ordering::SeqCst) == 0
                && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                    .map(|m| valid(m))
                    .unwrap_or(false)
            {
                PROFILE_SPARE_CANDIDATE.store(r, Ordering::SeqCst);
                let model =
                    unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
                PROFILE_SPARE_CANDIDATE_MODEL.store(model, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "loading-portrait: pre-recorded spare candidate renderer=0x{r:x} slot={s} model_ins=0x{model:x} (loading-screen-owned renderer)"
                ));
            }
        }
    }
    // Per-slot: once a slot's model (+0x778) has built, readback its COLOR offscreen RT and dump to
    // portrait-capture-slot{N}.bin ONCE (tracked via PROFILE_SLOT_DUMP_MASK). Inspect the 10 dumps
    // offline and match to the known disk characters to map renderer-slot -> character.
    if portrait_real_pixels_enabled() {
        for s in 0..10i32 {
            let bit = 1usize << s;
            if PROFILE_SLOT_DUMP_MASK.load(Ordering::SeqCst) & bit != 0 {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if !valid(r)
                || unsafe { safe_read_usize(r) }.unwrap_or(0)
                    != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                continue;
            }
            let model =
                unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
            if !valid(model) {
                continue;
            }
            let off = unsafe {
                safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
            }
            .unwrap_or(0);
            if !valid(off) {
                continue;
            }
            // LIGHTING residency oracle: envObj = renderer+0x760; *(envObj) is the registered IBL
            // env-region id, non-zero ONLY if the GILM env map was resident when the IBL built.
            let env_obj =
                unsafe { safe_read_usize(r + PROFILE_RENDERER_ENV_REGION_OFFSET) }.unwrap_or(0);
            let ibl_region = if valid(env_obj) {
                unsafe { safe_read_usize(env_obj) }.unwrap_or(0)
            } else {
                0
            };
            if let Some((w, h, px)) = unsafe { readback_offscreen_rgba8(off) } {
                let nb = portrait_center_nonblack(w, h, &px);
                let checker = portrait_looks_like_checker(w, h, &px);
                // BAKE SOURCE: store the TARGET slot's menu portrait into LOADING_BG_PORTRAIT_RGBA so the
                // now-loading forge bakes IT into the static TPF (the proven decode-time display path) AND the
                // present-overlay composite (gated on PROFILE_BAKE_RGBA_CAPTURED) displays it. ONLY latch on a
                // REAL FACE: nonblack alone false-passes the magenta/white checker (an unrendered RT or our
                // cover placeholder) -- latching that is exactly what put a center checker square on screen and
                // made oracle_..._gx_nonblack a false success. Requiring !checker means we keep re-checking each
                // dump cycle and latch only once a real shaded head has actually rendered into the offscreen
                // (which needs the render-thread offscreen drive -- see portrait_render_drive). One-shot via swap.
                // NO SILENT slot-0 FALLBACK (bd er-effects-rs-91zb step 3). This used to read
                // `portrait_loaded_slot()`, which collapses "no source names a slot" to 0 via
                // `unwrap_or(0)` -- so with nothing confirmed, slot 0's head was published as
                // though it were the loaded character. Publish NOTHING instead: a missing portrait
                // is a visible, diagnosable absence; a confidently wrong one is not.
                if portrait_loaded_slot_confirmed() == Some(s)
                    && nb
                    && !checker
                    && PROFILE_BAKE_RGBA_CAPTURED.swap(1, Ordering::SeqCst) == 0
                {
                    let _ = ibl_region;
                    dump_portrait_rgba(110, w, h, &px);
                    // Readiness gate: hold back neutral/too-small transient captures (Bug A/B). On
                    // rejection, un-consume the one-shot (the swap fired in the condition above) so a
                    // later full-size head still bake-captures.
                    if note_ls_portrait_capture(w, h, &px) {
                        if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
                            *g = Some((w, h, px.clone()));
                        }
                        // Identity tag rides with every bridge write (bd er-effects-rs-dpf6 Phase 1).
                        // Game thread: hash slot `s`'s summary record directly.
                        LS_PORTRAIT_PUBLISHED_SLOT.store((s + 1) as usize, Ordering::SeqCst);
                        LS_PORTRAIT_PUBLISHED_NAME_HASH
                            .store(unsafe { portrait_slot_name_hash(s) }, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "loading-portrait: BAKE-CAPTURED real menu portrait slot={s} dims={w}x{h} ibl_region=0x{ibl_region:x} -> LOADING_BG_PORTRAIT_RGBA (forge will bake it)"
                        ));
                    } else {
                        PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
                    }
                }
                dump_portrait_rgba(s, w, h, &px);
                PROFILE_SLOT_DUMP_MASK.fetch_or(bit, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "profile-slot-dump: slot={s} renderer=0x{r:x} model=0x{model:x} dims={w}x{h} nonblack={} env_obj=0x{env_obj:x} ibl_region=0x{ibl_region:x}",
                    nb as u8
                ));
            }
        }
    }
}

/// Hook on the CSMenuProfModelRend teardown-all (`FUN_1409b2f00`). One-shot: before the original
/// runs, save slot-0's renderer and null its table entry so the original's null-guarded delete
/// enqueue skips it -- sparing the loaded character's portrait renderer from the Continue teardown so
/// we can keep rendering it into the now-loading screen. The original then tears down slots 1-9.
pub(crate) unsafe extern "system" fn profile_renderer_teardown_spare_hook() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    // TEARDOWN FENCE (freeze relaxation, er-effects-rs-l1x): raise the fence BEFORE any
    // delete-enqueue below (both the orphan reclaim and the native table teardown in original()),
    // then wait out a render-thread pump caught mid-drive. The pump is one model update+draw
    // (sub-ms), so the 10ms cap is generous; a timeout is counted, not fatal -- worst case equals
    // the OLD per-frame TOCTOU exposure for exactly one frame instead of every frame. The fence is
    // lowered at the end of this hook, after the native teardown returns.
    PROFILE_RENDERER_TEARDOWN_FENCE.store(1, Ordering::SeqCst);
    if PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) {
        PROFILE_TEARDOWN_FENCE_WAITS.fetch_add(1, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(10);
        while PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) {
            if std::time::Instant::now() > deadline {
                PROFILE_TEARDOWN_FENCE_TIMEOUTS.fetch_add(1, Ordering::SeqCst);
                break;
            }
            std::thread::yield_now();
        }
    }
    // REPEATED-SWITCH GX OVERFLOW FIX (0x1aeaf05, ~switch #4): destroy the PRIOR window's spared
    // renderer now, on the game thread, before sparing this switch's renderer. The load-complete
    // reset (render thread) moved it into PROFILE_SPARE_ORPHAN instead of dropping it; the spare
    // excluded it from the native delete (nulled its table slot), so without this it stayed alive
    // with its ResMan offscreen draw task filling the 192-slot GX command queue every frame,
    // accumulating +1 leaked renderer per switch. delay_delete_enqueue_renderer is the exact native
    // delete path (vtable-guarded), run here on the correct thread.
    let orphan = PROFILE_SPARE_ORPHAN.swap(0, Ordering::SeqCst);
    if orphan != 0 {
        let deleted = unsafe { delay_delete_enqueue_renderer(orphan) };
        // Ownership ledger: discharge our responsibility for the spared renderer (paired with the
        // ownership_take at the spare site). Released whether or not the enqueue took -- either we
        // handed it to delay-delete or it was already stale/gone; either way it is no longer ours.
        ownership_release(OwnedClass::SparedRenderer);
        append_autoload_debug(format_args!(
            "loading-portrait: reclaimed prior spared renderer 0x{orphan:x} via CSDelayDeleteMan enqueued={deleted} (repeated-switch GX command-queue leak fix)"
        ));
    }
    // If the native title/menu code tries to run the teardown-all AGAIN after we have already rebuilt the
    // loading-screen-owned portrait table, do NOT let it delete the live animated source mid-load. This is
    // the exact failure exposed by the leading-gap fix: early build at LoadingScreen start, model animates,
    // then a later stale native teardown clears DAT_143d6d8d0 and the overlay keeps displaying the last
    // snapshot frozen (r_bad climbs, drive/display diverges). The builder's own internal teardown still runs
    // normally because PROFILE_LOADSCREEN_TABLE_OWNED is set only after the builder returns.
    if PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst) == 0
    {
        append_autoload_debug(format_args!(
            "loading-portrait: skipped stale native profile-table teardown during active LoadingScreen -- keeping loading-owned renderer alive so the portrait keeps animating"
        ));
        PROFILE_RENDERER_TEARDOWN_FENCE.store(0, Ordering::SeqCst);
        return;
    }
    // Gate on the live-portrait overlay feature OR product autoload -- the native-continue path does NOT set
    // PRODUCT_AUTOLOAD_ARMED, so gating on product_autoload alone never spared anything there.
    if LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) == 0
        && (product_autoload_enabled() || portrait_overlay_enabled())
        // DISABLE-ON-RELOAD FALLBACK (user 2026-07-23): do NOT spare the portrait renderer on a
        // System->Quit->Load SWITCH reload. A spared renderer whose GX resource goes stale across the
        // reload crashed load2 near completion (null native GX resource wrapper -> FUN_141e90290 rcx=0x20
        // AV; spared[model_ok=0]; lookat off_resource_bad climbing 68->128). Skipping the spare lets the
        // native teardown free it with the world -- no stale spared renderer, so the per-frame profile-draw
        // never runs against a dead resource. LOAD1/first-load is UNAFFECTED (switch_reload_active()==false
        // there), so the loading-portrait still shows on the initial load, just not on reloads. This is the
        // user-chosen fallback ahead of the full Root A teardown fix (unregister the ResMan draw task +
        // free per reload). bd rootB-fd4io-fix-works-load2-resubmits-but-exposes-rootA-spared-renderer-crash-2026-07-23.
        && !crate::experiments::gating::switch_reload_active()
    {
        if let Ok(base) = game_module_base() {
            // The slot we render (er-effects-rs-j3r): the newly-selected character on a switch
            // (SELECTED_SLOT), else the loaded slot (ac0). portrait_target_slot() is what makes the
            // loading portrait show the character just picked, not the one still resident.
            let slot = portrait_target_slot();
            // Prefer the PRE-RECORDED candidate (captured at the menu on a model-built frame -- robust to
            // the menu's model_ins cycling). Find its table slot and protect it. Fall back to reading
            // table[slot] + a model-built guard if no candidate was recorded.
            let candidate = PROFILE_SPARE_CANDIDATE.load(Ordering::SeqCst);
            let target_te = portrait_renderer_table_entry(base, slot);
            // Honor the pre-recorded candidate ONLY if it still sits in the TARGET slot. A candidate
            // captured for the old character before a switch confirm must not be spared over the
            // newly-selected one -- in that case fall back to table[target] (its model is built, the
            // menu rendered all 10 slots). Prevents the loading portrait showing the prior character.
            let candidate_in_target =
                valid(candidate) && unsafe { safe_read_usize(target_te) }.unwrap_or(0) == candidate;
            let (renderer, table, spared_slot) = if candidate_in_target {
                (candidate, target_te, slot)
            } else {
                let r = unsafe { safe_read_usize(target_te) }.unwrap_or(0);
                let model_built = valid(r)
                    && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .map(|m| valid(m))
                        .unwrap_or(false);
                (if model_built { r } else { 0 }, target_te, slot)
            };
            if valid(renderer)
                && unsafe { safe_read_usize(renderer) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                LOADING_BG_PORTRAIT_SPARED_RENDERER.store(renderer, Ordering::SeqCst);
                PROFILE_RENDERER_SPARE_HITS.fetch_add(1, Ordering::SeqCst);
                // Ownership ledger: we just excluded this renderer from the native delete, so WE own
                // its destruction now. Paired with the ownership_release on the drain path below.
                ownership_take(OwnedClass::SparedRenderer);
                // Null the table entry so the original's null-guarded delete-enqueue skips it.
                if table != 0 {
                    unsafe { (table as *mut usize).write_volatile(0) };
                }
                // Re-latch the look-at base from the post-Continue model (a different model instance).
                if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&spared_slot) {
                    let mut guard = match PROFILE_LOOKAT_SLOTS.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    if let Some(s) = guard[spared_slot as usize].as_mut() {
                        s.base_latched = false;
                    }
                }
                let model_at_spare =
                    unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .unwrap_or(0);
                append_autoload_debug(format_args!(
                    "loading-portrait: SPARED slot{spared_slot} renderer=0x{renderer:x} (candidate=0x{candidate:x}) model_ins=0x{model_at_spare:x} from teardown -- drive look-at + render it post-Continue"
                ));
            }
        }
    }
    let orig = PROFILE_RENDERER_TEARDOWN_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
        unsafe { f() };
    }
    // Native teardown done -- the table entries are delete-enqueued/nulled, so the next pump
    // invocation's per-frame table re-read + vtable probe fails closed until the new window's
    // rebuild. Safe to let the drive back in.
    PROFILE_RENDERER_TEARDOWN_FENCE.store(0, Ordering::SeqCst);
}

/// Diagnostic + REPAIR detour on the native profile-portrait builder (`FUN_1409aa7d0` =
/// `PROFILE_RENDERER_REFRESH_RVA`). The builder derefs `table[slot]+0x754` with NO null check for
/// every slot whose profile record exists (Ghidra: `FUN_140261c30(summary,slot) != 0` gates the
/// walk, the entry itself is never checked), and its 10-slot table setup is called from exactly ONE
/// native site -- the TitleTopDialog constructor -- so our cloned in-world ProfileSelect reopens run
/// it against whatever the last teardown left; the 3rd in-session open found the table fully empty
/// and AV'd at `[null+0x754]` (er-effects-rs-j3r). Three layers, all fault-guarded + catch_unwind:
///   1. DIAG: log the full table once per distinct degraded (mask, caller) pattern.
///   2. REPAIR: a FULLY-empty table (the proven crash state) is rebuilt via the engine's own no-arg
///      setup (`PROFILE_TABLE_BUILDER_RVA`; its internal teardown is a no-op on an all-null table),
///      satisfying the native invariant exactly as the TitleTopDialog ctor would. Gated on
///      `PROFILE_TABLE_WAS_POPULATED` (engine/ResMan up -- the setup AVs at boot title) and on
///      fully-empty ONLY: a MIXED table is the intentional teardown-spare state during Continue
///      loading and must not be rebuilt over.
///   3. GUARD: if any slot is still null/invalid after the (possible) repair, SKIP chaining the
///      original this call (fail-soft; the per-frame builder retries) instead of letting the native
///      walk AV.
pub(crate) unsafe extern "system" fn profile_select_table_diag_hook() {
    let chain = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(base) = game_module_base() else {
            return true;
        };
        let null = TITLE_OWNER_SCAN_START_ADDRESS;
        let scan_table = |ptrs: &mut [usize; TITLE_PROFILE_SLOT_COUNT]| -> (u32, u32) {
            let mut null_mask = 0u32;
            let mut valid_mask = 0u32;
            for s in 0..TITLE_PROFILE_SLOT_COUNT {
                let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s as i32)) }
                    .unwrap_or(0);
                ptrs[s] = r;
                let is_valid = r != 0
                    && r != null
                    && unsafe { safe_read_usize(r) }.unwrap_or(0)
                        == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
                if is_valid {
                    valid_mask |= 1 << s;
                } else {
                    null_mask |= 1 << s;
                }
            }
            (valid_mask, null_mask)
        };
        let mut ptrs = [0usize; TITLE_PROFILE_SLOT_COUNT];
        let (valid_mask, mut null_mask) = scan_table(&mut ptrs);
        // Degraded = ANY slot lost its renderer while the builder is about to run. A HEALTHY table
        // is all 10 valid (native setup allocs all 10 unconditionally); any null is the crash-prone
        // state, INCLUDING all-null (the fully-empty table that caused the 3rd-open crash -- the
        // earlier "mixed only" check missed it). Log per distinct (mask, caller) so it never spams.
        let degraded = null_mask != 0;
        let caller_rva = crate::crashlog::trace_first_game_caller_rva();
        let key =
            ((caller_rva & 0xffffff) << 20) | ((valid_mask as usize) << 10) | null_mask as usize;
        if degraded && PROFILE_SELECT_TABLE_DIAG_LAST.swap(key, Ordering::SeqCst) != key {
            append_crash_log(format_args!(
                "PROFILESELECT-TABLE-DIAG: degraded profile-renderer table before native builder (er-effects-rs-j3r) caller_rva=0x{caller_rva:x} valid_mask=0x{valid_mask:x} null_mask=0x{null_mask:x} entries=[0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x}]",
                ptrs[0], ptrs[1], ptrs[2], ptrs[3], ptrs[4], ptrs[5], ptrs[6], ptrs[7], ptrs[8],
                ptrs[9]
            ));
        } else if !degraded {
            PROFILE_SELECT_TABLE_DIAG_LAST.store(0, Ordering::SeqCst);
            // A fully-valid table at builder entry proves the engine built renderers successfully --
            // the same "engine/ResMan up" evidence the loading-screen path latches; latching it here
            // too arms the repair even when the loading-portrait feature is disabled.
            PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
        }
        if null_mask == PROFILE_TABLE_ALL_SLOTS_MASK
            && PROFILE_TABLE_WAS_POPULATED.load(Ordering::SeqCst) != 0
        {
            let build: unsafe extern "system" fn() =
                unsafe { core::mem::transmute(base + PROFILE_TABLE_BUILDER_RVA) };
            unsafe { build() };
            let n = PROFILE_SELECT_TABLE_REPAIR_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            let (revalid_mask, renull_mask) = scan_table(&mut ptrs);
            null_mask = renull_mask;
            append_crash_log(format_args!(
                "PROFILESELECT-TABLE-REPAIR #{n}: fully-empty renderer table at native builder entry -> re-ran native table setup 0x{:x}; post-repair valid_mask=0x{revalid_mask:x} null_mask=0x{renull_mask:x} (er-effects-rs-j3r)",
                base + PROFILE_TABLE_BUILDER_RVA
            ));
            append_autoload_debug(format_args!(
                "profileselect-table-repair #{n}: rebuilt empty 10-slot renderer table via native setup before the native builder walked it; post-repair valid_mask=0x{revalid_mask:x} (er-effects-rs-j3r)"
            ));
        }
        if null_mask != 0 {
            let n = PROFILE_SELECT_TABLE_GUARD_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            let skip_key = ((caller_rva & 0xffffff) << 10) | null_mask as usize;
            if PROFILE_SELECT_TABLE_GUARD_SKIP_LAST.swap(skip_key, Ordering::SeqCst) != skip_key {
                append_crash_log(format_args!(
                    "PROFILESELECT-TABLE-GUARD SKIP #{n}: null/invalid renderer slots remain (null_mask=0x{null_mask:x}) -- skipping the native builder this call so it cannot AV at [null+0x754] (er-effects-rs-j3r)"
                ));
            }
            return false;
        }
        true
    }))
    // A panicked diagnostic keeps the pre-hook behavior: chain the original.
    .unwrap_or(true);
    if !chain {
        return;
    }
    let orig = PROFILE_SELECT_TABLE_DIAG_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return;
    }
    let f: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
    unsafe { f() };
}
