use super::*;

static SAVE_SWAP_PRODUCTION_TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProductionPollGlobalSnapshot {
    save_swap: SystemQuitSaveSwapState,
    caches: ProfileSlotCachesTestSnapshot,
    presentation: PickerPresentationTestSnapshot,
    poll_telemetry: [usize; 8],
    face_hashes: [usize; TITLE_PROFILE_SLOT_COUNT],
    place_mask: usize,
    stats_row_cursor: usize,
    test_summary_ptr: usize,
    test_prepare_count: usize,
    test_commit_count: usize,
    test_fail_next_restore: bool,
}

impl ProductionPollGlobalSnapshot {
    fn capture() -> Self {
        let _operation = system_quit_save_swap_operation_lock();
        Self {
            save_swap: system_quit_save_swap_lock().clone(),
            caches: snapshot_profile_slot_caches_for_test(),
            presentation: snapshot_picker_presentation_for_test(),
            poll_telemetry: [
                SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.load(Ordering::SeqCst),
                SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_ATTEMPTS.load(Ordering::SeqCst),
                SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_FAILURE_COUNT.load(Ordering::SeqCst),
                SYSTEM_QUIT_SAVE_SWAP_POLL_ZERO_SLOT_COUNT.load(Ordering::SeqCst),
                SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_COUNT.load(Ordering::SeqCst),
                SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_LAST_REASON.load(Ordering::SeqCst),
                SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_SUPPRESSED_COUNT.load(Ordering::SeqCst),
                SYSTEM_QUIT_SAVE_SWAP_POLL_RESTORE_FAILURE_COUNT.load(Ordering::SeqCst),
            ],
            face_hashes: std::array::from_fn(|slot| {
                PROFILE_PREVIEW_FACE_HASH[slot].load(Ordering::SeqCst)
            }),
            place_mask: PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.load(Ordering::SeqCst),
            stats_row_cursor: PROFILE_STATS_PREVIEW_ROW_CURSOR.load(Ordering::SeqCst),
            test_summary_ptr: SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_SUMMARY_PTR.load(Ordering::SeqCst),
            test_prepare_count: SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT
                .load(Ordering::SeqCst),
            test_commit_count: SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT.load(Ordering::SeqCst),
            test_fail_next_restore: SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_FAIL_NEXT_RESTORE
                .load(Ordering::SeqCst),
        }
    }
}

struct ProductionPollTestCleanup {
    baseline: Option<ProductionPollGlobalSnapshot>,
    summary_ptr: usize,
    summary_bytes: Vec<u8>,
}

impl ProductionPollTestCleanup {
    fn new(baseline: ProductionPollGlobalSnapshot, live_summary: Option<&mut [u8]>) -> Self {
        let (summary_ptr, summary_bytes) = live_summary
            .map(|summary| (summary.as_mut_ptr() as usize, summary.to_vec()))
            .unwrap_or((0, Vec::new()));
        if summary_ptr != 0 {
            SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_SUMMARY_PTR.store(summary_ptr, Ordering::SeqCst);
        }
        Self {
            baseline: Some(baseline),
            summary_ptr,
            summary_bytes,
        }
    }
}

impl Drop for ProductionPollTestCleanup {
    fn drop(&mut self) {
        let Some(baseline) = self.baseline.take() else {
            return;
        };
        let _operation = system_quit_save_swap_operation_lock();
        // The summary target outlives this guard by construction. Restore its bytes before restoring
        // the hook pointer or the state snapshot that may name a different live summary.
        if self.summary_ptr != 0 && !self.summary_bytes.is_empty() {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.summary_bytes.as_ptr(),
                    self.summary_ptr as *mut u8,
                    self.summary_bytes.len(),
                );
            }
        }
        restore_profile_slot_caches_for_test(&baseline.caches);
        restore_picker_presentation_for_test(&baseline.presentation);
        for (slot, value) in baseline.face_hashes.iter().copied().enumerate() {
            PROFILE_PREVIEW_FACE_HASH[slot].store(value, Ordering::SeqCst);
        }
        PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.store(baseline.place_mask, Ordering::SeqCst);
        PROFILE_STATS_PREVIEW_ROW_CURSOR.store(baseline.stats_row_cursor, Ordering::SeqCst);
        let counters = [
            &SYSTEM_QUIT_SAVE_SWAP_POLL_TICK,
            &SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_ATTEMPTS,
            &SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_FAILURE_COUNT,
            &SYSTEM_QUIT_SAVE_SWAP_POLL_ZERO_SLOT_COUNT,
            &SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_COUNT,
            &SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_LAST_REASON,
            &SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_SUPPRESSED_COUNT,
            &SYSTEM_QUIT_SAVE_SWAP_POLL_RESTORE_FAILURE_COUNT,
        ];
        for (counter, value) in counters.into_iter().zip(baseline.poll_telemetry) {
            counter.store(value, Ordering::SeqCst);
        }
        *system_quit_save_swap_lock() = baseline.save_swap;
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_SUMMARY_PTR
            .store(baseline.test_summary_ptr, Ordering::SeqCst);
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT
            .store(baseline.test_prepare_count, Ordering::SeqCst);
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT
            .store(baseline.test_commit_count, Ordering::SeqCst);
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_FAIL_NEXT_RESTORE
            .store(baseline.test_fail_next_restore, Ordering::SeqCst);
    }
}

fn temp_file(name: &str) -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "er-effects-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ))
}

fn corpus_root() -> std::path::PathBuf {
    let root = std::env::var_os("ER_SAVE_CORPUS_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../save-files")
        });
    if root.join("150-Banon/ER0000.sl2").is_file() {
        root
    } else {
        root.join("save-files")
    }
}

fn corpus_save() -> Option<Vec<u8>> {
    std::fs::read(corpus_root().join("150-Banon/ER0000.sl2")).ok()
}

fn changed_corpus_save() -> Option<Vec<u8>> {
    ["90-Bean/ER0000.sl2", "200-BEAST/ER0000.sl2"]
        .into_iter()
        .find_map(|path| std::fs::read(corpus_root().join(path)).ok())
}

fn production_poll_corpus_pair() -> Option<(Vec<u8>, Vec<u8>)> {
    Some((corpus_save()?, changed_corpus_save()?))
}

fn with_zero_active_slots(mut bytes: Vec<u8>) -> Option<Vec<u8>> {
    const MENU_SAVE_LOAD_LEN: usize = 0x150;
    const MENU_SAVE_LOAD_DATA: usize = 0x154;
    let entry = er_save_loader::bnd4::parse_entries(&bytes)
        .ok()?
        .into_iter()
        .find(|entry| entry.name == "USER_DATA010")?;
    let body = entry.data_offset + er_save_loader::bnd4::ENTRY_MD5_LEN;
    let menu_len = u32::from_le_bytes(
        bytes
            .get(body + MENU_SAVE_LOAD_LEN..body + MENU_SAVE_LOAD_LEN + 4)?
            .try_into()
            .ok()?,
    ) as usize;
    let active = body + MENU_SAVE_LOAD_DATA + menu_len;
    bytes
        .get_mut(active..active + TITLE_PROFILE_SLOT_COUNT)?
        .fill(0);
    Some(bytes)
}

#[test]
fn real_file_state_writes_original_only_for_exact_successful_mutation() {
    let path = temp_file("save-swap-write-ownership");
    let original = b"original-A".to_vec();
    std::fs::write(&path, &original).unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().ok();
    let mut state = SystemQuitSaveSwapState::default();
    let identity = system_quit_save_swap_publish_arm(
        &mut state,
        path.to_str().unwrap(),
        original.clone(),
        original.len() as u64,
        1,
    );
    let writes = AtomicUsize::new(0);
    assert_eq!(
        system_quit_save_swap_restore_if_mutated_with(
            &mut state,
            &identity,
            "no-post-arm-mutation",
            |_, _| {
                writes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap(),
        false
    );
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().ok(), before);

    state.candidate_bytes = b"candidate-B".to_vec();
    assert!(
        system_quit_save_swap_write_candidate_with(&mut state, |_, _| {
            Err(std::io::Error::other("candidate write injected"))
        })
        .is_err()
    );
    assert_eq!(state.file_mutated_generation, 0);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    system_quit_save_swap_write_candidate_with(&mut state, |path, bytes| {
        std::fs::write(path, bytes)
    })
    .unwrap();
    assert_eq!(state.file_mutated_generation, identity.generation);
    let injected = system_quit_save_swap_restore_if_mutated_with(
        &mut state,
        &identity,
        "injected-write-failure",
        |_, _| Err(std::io::Error::other("injected")),
    );
    assert!(injected.is_err());
    assert_eq!(state.file_mutated_generation, identity.generation);
    assert_eq!(std::fs::read(&path).unwrap(), b"candidate-B");
    assert!(
        system_quit_save_swap_restore_if_mutated_with(
            &mut state,
            &identity,
            "retry-success",
            |path, bytes| std::fs::write(path, bytes),
        )
        .unwrap()
    );
    assert_eq!(state.file_mutated_generation, 0);
    assert_eq!(std::fs::read(&path).unwrap(), original);

    let newer = system_quit_save_swap_publish_arm(
        &mut state,
        path.to_str().unwrap(),
        b"newer-C".to_vec(),
        7,
        2,
    );
    state.file_mutated_generation = newer.generation;
    let stale_writes = AtomicUsize::new(0);
    assert!(
        !system_quit_save_swap_restore_if_mutated_with(
            &mut state,
            &identity,
            "stale-aba",
            |_, _| {
                stale_writes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap()
    );
    assert_eq!(stale_writes.load(Ordering::SeqCst), 0);
    assert_eq!(state.file_mutated_generation, newer.generation);
    let _ = std::fs::remove_file(path);
}

#[test]
fn production_exact_abort_performs_zero_file_write_without_owned_mutation_and_rejects_stale_aba() {
    let _serial = SAVE_SWAP_PRODUCTION_TEST_SERIAL.lock().unwrap();
    let path = temp_file("production-abort-no-write");
    let original = b"production-original-A".to_vec();
    std::fs::write(&path, &original).unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().ok();
    let baseline = ProductionPollGlobalSnapshot::capture();
    let cleanup = ProductionPollTestCleanup::new(baseline.clone(), None);
    let identity = {
        let mut global = system_quit_save_swap_lock();
        system_quit_save_swap_publish_arm(
            &mut global,
            path.to_str().unwrap(),
            original.clone(),
            original.len() as u64,
            11,
        )
    };
    assert!(unsafe { system_quit_save_swap_abort_exact(&identity, "production-no-mutation-test") });
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().ok(), before);

    let newer = {
        let mut global = system_quit_save_swap_lock();
        system_quit_save_swap_publish_arm(
            &mut global,
            path.to_str().unwrap(),
            b"newer-snapshot".to_vec(),
            14,
            12,
        )
    };
    assert!(!unsafe { system_quit_save_swap_abort_exact(&identity, "production-stale-aba-test") });
    {
        let global = system_quit_save_swap_lock();
        assert_eq!(global.arm_generation, newer.generation);
        assert_eq!(global.original_bytes, b"newer-snapshot");
    }
    drop(cleanup);
    assert_eq!(ProductionPollGlobalSnapshot::capture(), baseline);
    let _ = std::fs::remove_file(path);
}

// Caller holds SAVE_SWAP_PRODUCTION_TEST_SERIAL for this whole scenario.
fn run_production_poll_normal_completion_scenario(file_a: &[u8], valid_candidate: &[u8]) {
    assert_ne!(
        system_quit_hash_bytes(&file_a),
        system_quit_hash_bytes(&valid_candidate),
        "changed corpus candidate must differ from active A"
    );
    assert!(
        er_save_loader::bnd4::active_slots(&valid_candidate)
            .expect("changed corpus BND4")
            .into_iter()
            .any(|active| active),
        "changed corpus candidate needs at least one active slot"
    );
    let zero_candidate =
        with_zero_active_slots(valid_candidate.to_vec()).expect("zero-slot transform");
    assert_eq!(
        er_save_loader::bnd4::active_slots(&zero_candidate).expect("valid zero-slot BND4"),
        [false; TITLE_PROFILE_SLOT_COUNT]
    );

    let baseline = ProductionPollGlobalSnapshot::capture();
    let path = temp_file("production-poll-transaction");
    std::fs::write(&path, &file_a).unwrap();
    let mut live_summary = vec![0x5au8; PROFILE_SUMMARY_TOTAL_BYTES];
    live_summary[PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET] = 1;
    let original_summary = live_summary.clone();
    let cleanup =
        ProductionPollTestCleanup::new(baseline.clone(), Some(live_summary.as_mut_slice()));
    SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT.store(0, Ordering::SeqCst);
    let parse_base = SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_ATTEMPTS.load(Ordering::SeqCst);
    let failure_base = SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_FAILURE_COUNT.load(Ordering::SeqCst);
    let zero_base = SYSTEM_QUIT_SAVE_SWAP_POLL_ZERO_SLOT_COUNT.load(Ordering::SeqCst);
    let suppressed_base =
        SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_SUPPRESSED_COUNT.load(Ordering::SeqCst);
    let restore_failure_base =
        SYSTEM_QUIT_SAVE_SWAP_POLL_RESTORE_FAILURE_COUNT.load(Ordering::SeqCst);
    let cache_reload_base = PROFILE_SLOT_CACHE_PREVIEW_RELOADS.load(Ordering::SeqCst);

    let identity = system_quit_save_swap_arm_original_identity(path.to_str().unwrap())
        .expect("production arm A");
    let invalid_candidate = b"not-a-valid-bnd4-candidate".to_vec();
    std::fs::write(&path, &invalid_candidate).unwrap();
    SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.store(0, Ordering::SeqCst);
    unsafe { system_quit_save_swap_poll_preview(0) };
    assert_eq!(std::fs::read(&path).unwrap(), file_a);
    assert_eq!(live_summary, original_summary);
    assert_eq!(
        PROFILE_SLOT_CACHE_PREVIEW_RELOADS.load(Ordering::SeqCst),
        cache_reload_base
    );
    assert_eq!(
        std::array::from_fn(|slot| PROFILE_PREVIEW_FACE_HASH[slot].load(Ordering::SeqCst)),
        baseline.face_hashes
    );
    assert_eq!(
        PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.load(Ordering::SeqCst),
        baseline.place_mask
    );
    {
        let st = system_quit_save_swap_lock();
        assert!(system_quit_save_swap_identity_matches(&st, &identity));
        assert!(st.armed);
        assert!(!st.preview_applied);
        assert_eq!(st.file_mutated_generation, 0);
        assert_eq!(st.summary_mutated_generation, 0);
        assert_eq!(
            st.rejected_candidate_hash,
            system_quit_hash_bytes(&invalid_candidate)
        );
        assert_eq!(st.rejected_candidate_len, invalid_candidate.len() as u64);
        assert_ne!(st.rejected_candidate_modified_ns, 0);
        assert_eq!(
            st.rejected_candidate_reason,
            SAVE_SWAP_REJECTION_PARSE_FAILURE
        );
    }
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_ATTEMPTS.load(Ordering::SeqCst),
        parse_base + 1
    );
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_FAILURE_COUNT.load(Ordering::SeqCst),
        failure_base + 1
    );

    // Re-observing identical bytes restores A again but does not parse or touch live presentation.
    std::fs::write(&path, &invalid_candidate).unwrap();
    SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.store(0, Ordering::SeqCst);
    unsafe { system_quit_save_swap_poll_preview(0) };
    assert_eq!(std::fs::read(&path).unwrap(), file_a);
    assert_eq!(live_summary, original_summary);
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_PARSE_ATTEMPTS.load(Ordering::SeqCst),
        parse_base + 1
    );
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_REJECTION_SUPPRESSED_COUNT.load(Ordering::SeqCst),
        suppressed_base + 1
    );
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT.load(Ordering::SeqCst),
        0
    );

    std::fs::write(&path, &zero_candidate).unwrap();
    SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.store(0, Ordering::SeqCst);
    unsafe { system_quit_save_swap_poll_preview(0) };
    assert_eq!(std::fs::read(&path).unwrap(), file_a);
    assert_eq!(live_summary, original_summary);
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_ZERO_SLOT_COUNT.load(Ordering::SeqCst),
        zero_base + 1
    );
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        PROFILE_SLOT_CACHE_PREVIEW_RELOADS.load(Ordering::SeqCst),
        cache_reload_base
    );
    assert_eq!(
        std::array::from_fn(|slot| PROFILE_PREVIEW_FACE_HASH[slot].load(Ordering::SeqCst)),
        baseline.face_hashes
    );
    assert_eq!(
        PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.load(Ordering::SeqCst),
        baseline.place_mask
    );
    {
        let st = system_quit_save_swap_lock();
        assert_eq!(
            st.rejected_candidate_hash,
            system_quit_hash_bytes(&zero_candidate)
        );
        assert_eq!(
            st.rejected_candidate_reason,
            SAVE_SWAP_REJECTION_ZERO_READABLE_SLOTS
        );
        assert_eq!(st.summary_mutated_generation, 0);
    }

    std::fs::write(&path, &valid_candidate).unwrap();
    SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.store(0, Ordering::SeqCst);
    unsafe { system_quit_save_swap_poll_preview(0) };
    assert_eq!(std::fs::read(&path).unwrap(), file_a);
    assert_ne!(live_summary, original_summary);
    assert!(SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT.load(Ordering::SeqCst) > 0);
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT.load(Ordering::SeqCst),
        1
    );
    {
        let st = system_quit_save_swap_lock();
        assert!(st.preview_applied);
        assert_eq!(st.summary_mutated_generation, identity.generation);
        assert_eq!(st.file_mutated_generation, 0);
        assert_eq!(st.rejected_candidate_reason, SAVE_SWAP_REJECTION_NONE);
        assert_eq!(st.candidate_bytes, valid_candidate);
    }
    unsafe { system_quit_save_swap_restore_profile_summary("production-poll-normal-restore") };
    assert_eq!(live_summary, original_summary);
    assert_eq!(std::fs::read(&path).unwrap(), file_a);
    {
        let st = system_quit_save_swap_lock();
        assert_eq!(st.arm_generation, 0);
        assert_eq!(st.file_mutated_generation, 0);
        assert_eq!(st.summary_mutated_generation, 0);
    }

    // A poll restore failure retains exact file recovery evidence. Normal restore retries it without
    // publishing a rejection fingerprint or letting a stale generation write.
    let retry_identity = system_quit_save_swap_arm_original_identity(path.to_str().unwrap())
        .expect("production re-arm A for restore retry");
    let retry_invalid = b"different-invalid-bnd4-for-restore-retry".to_vec();
    std::fs::write(&path, &retry_invalid).unwrap();
    SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_FAIL_NEXT_RESTORE.store(true, Ordering::SeqCst);
    SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.store(0, Ordering::SeqCst);
    unsafe { system_quit_save_swap_poll_preview(0) };
    assert_eq!(std::fs::read(&path).unwrap(), retry_invalid);
    {
        let st = system_quit_save_swap_lock();
        assert!(system_quit_save_swap_identity_matches(&st, &retry_identity));
        assert!(!st.armed);
        assert_eq!(st.file_mutated_generation, retry_identity.generation);
        assert_eq!(st.rejected_candidate_reason, SAVE_SWAP_REJECTION_NONE);
        assert_eq!(st.rejected_candidate_hash, 0);
    }
    assert_eq!(
        SYSTEM_QUIT_SAVE_SWAP_POLL_RESTORE_FAILURE_COUNT.load(Ordering::SeqCst),
        restore_failure_base + 1
    );
    unsafe { system_quit_save_swap_restore_profile_summary("production-poll-restore-retry") };
    assert_eq!(std::fs::read(&path).unwrap(), file_a);
    {
        let st = system_quit_save_swap_lock();
        assert_eq!(st.arm_generation, 0);
        assert_eq!(st.file_mutated_generation, 0);
    }

    drop(cleanup);
    assert_eq!(live_summary, original_summary);
    assert_eq!(ProductionPollGlobalSnapshot::capture(), baseline);
    let _ = std::fs::remove_file(path);
}

// Caller holds SAVE_SWAP_PRODUCTION_TEST_SERIAL for this whole scenario.
fn run_production_poll_panic_cleanup_scenario(file_a: &[u8], valid_candidate: &[u8]) {
    let baseline = ProductionPollGlobalSnapshot::capture();
    let path = temp_file("production-poll-panic-isolation");
    std::fs::write(&path, &file_a).unwrap();
    let mut live_summary = vec![0x6bu8; PROFILE_SUMMARY_TOTAL_BYTES];
    live_summary[PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET] = 1;
    let original_summary = live_summary.clone();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _cleanup =
            ProductionPollTestCleanup::new(baseline.clone(), Some(live_summary.as_mut_slice()));
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT.store(0, Ordering::SeqCst);
        SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT.store(0, Ordering::SeqCst);
        let identity = system_quit_save_swap_arm_original_identity(path.to_str().unwrap())
            .expect("production panic test arm A");
        std::fs::write(&path, &valid_candidate).unwrap();
        SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.store(0, Ordering::SeqCst);
        unsafe { system_quit_save_swap_poll_preview(0) };
        assert_eq!(std::fs::read(&path).unwrap(), file_a);
        assert_ne!(live_summary, original_summary);
        assert_eq!(
            SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT.load(Ordering::SeqCst),
            1
        );
        {
            let st = system_quit_save_swap_lock();
            assert!(system_quit_save_swap_identity_matches(&st, &identity));
            assert!(st.preview_applied);
            assert_eq!(st.summary_mutated_generation, identity.generation);
        }
        assert_ne!(
            snapshot_profile_slot_caches_for_test(),
            baseline.caches,
            "valid preview must mutate the cache snapshot before unwind"
        );
        panic!("intentional post-preview panic for RAII isolation proof");
    }));

    assert!(result.is_err());
    assert_eq!(live_summary, original_summary);
    assert_eq!(ProductionPollGlobalSnapshot::capture(), baseline);
    assert_eq!(std::fs::read(&path).unwrap(), file_a);
    let _ = std::fs::remove_file(path);
}

#[test]
fn production_poll_parse_failure_zero_slot_and_valid_candidate_transaction() {
    let _serial = SAVE_SWAP_PRODUCTION_TEST_SERIAL.lock().unwrap();
    let Some((file_a, valid_candidate)) = production_poll_corpus_pair() else {
        eprintln!("save corpus missing; skipping production poll transaction test");
        return;
    };
    run_production_poll_normal_completion_scenario(&file_a, &valid_candidate);
}

#[test]
fn production_poll_cleanup_restores_every_global_after_panic_post_commit() {
    let _serial = SAVE_SWAP_PRODUCTION_TEST_SERIAL.lock().unwrap();
    let Some((file_a, valid_candidate)) = production_poll_corpus_pair() else {
        eprintln!("save corpus missing; skipping production poll panic-isolation test");
        return;
    };
    run_production_poll_panic_cleanup_scenario(&file_a, &valid_candidate);
}

#[test]
fn production_poll_same_process_normal_then_panic_preserves_complete_baseline() {
    let _serial = SAVE_SWAP_PRODUCTION_TEST_SERIAL.lock().unwrap();
    let Some((file_a, valid_candidate)) = production_poll_corpus_pair() else {
        eprintln!("save corpus missing; skipping same-process order test");
        return;
    };
    let process_baseline = ProductionPollGlobalSnapshot::capture();
    run_production_poll_normal_completion_scenario(&file_a, &valid_candidate);
    assert_eq!(
        ProductionPollGlobalSnapshot::capture(),
        process_baseline,
        "normal scenario leaked state before same-process panic scenario"
    );
    run_production_poll_panic_cleanup_scenario(&file_a, &valid_candidate);
    assert_eq!(
        ProductionPollGlobalSnapshot::capture(),
        process_baseline,
        "panic scenario leaked state after same-process normal scenario"
    );
}
