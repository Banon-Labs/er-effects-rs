
/// Read-only, save-safe save-data snapshot for the parked-title disambiguation
/// (goal step 2): confirm GameDataMan (`game_data_man_ptr_or_null()`) and its `CS::ProfileSummary`
/// container (`+SLOT_MANAGER_CONTAINER_OFFSET`) are built cold, read the per-slot
/// active bytes the char-mount gate (`0x67b200`) checks via `byte[profile+slot+8]`,
/// and read the save-mgr deserialize-ready handle (`[mgr+0xdf0]`, the gate fast-path).
/// Every access is a fault-tolerant `ReadProcessMemory` -- no game-state mutation.
pub(crate) fn write_save_data_snapshot_telemetry(body: &mut String) {
    /// Null pointer sentinel for the chased singleton reads.
    const NULL_POINTER_VALUE: usize = 0;
    /// ProfileSummary per-slot active-byte array base (getter reads `byte[profile+slot+8]`).
    const PROFILE_SLOT_ACTIVE_ARRAY_OFFSET: usize = core::mem::size_of::<usize>();
    /// Save-mgr deserialize-ready handle (gate `0x67b200` fast-path `[mgr+0xdf0]`).
    const GAME_MAN_DESERIALIZE_READY_DF0_OFFSET: usize =
        core::mem::offset_of!(GameManSaveSnapshotLayout, deserialize_ready);

    let Ok(base) = crate::experiments::game_module_base() else {
        body.push_str("  \"save_snapshot_available\": false,\n");
        return;
    };

    let game_data_man = crate::game_data_man_ptr_or_null();
    let profile_summary = if game_data_man == NULL_POINTER_VALUE {
        NULL_POINTER_VALUE
    } else {
        unsafe {
            crate::experiments::safe_read_usize(
                game_data_man + crate::SLOT_MANAGER_CONTAINER_OFFSET,
            )
        }
        .unwrap_or(NULL_POINTER_VALUE)
    };
    let slot_active_bytes = if profile_summary == NULL_POINTER_VALUE {
        None
    } else {
        unsafe {
            crate::experiments::safe_read_usize(profile_summary + PROFILE_SLOT_ACTIVE_ARRAY_OFFSET)
        }
    };
    let save_mgr = crate::game_man_ptr_or_null();
    let deserialize_ready = if save_mgr == NULL_POINTER_VALUE {
        None
    } else {
        unsafe {
            crate::experiments::safe_read_usize(save_mgr + GAME_MAN_DESERIALIZE_READY_DF0_OFFSET)
        }
    };

    // FD4 async-IO DRAIN subsystem (B step-3 lever check, read-only). The cold save-IO read
    // never drains because the queue-processing worker threads live in the global thread POOL
    // [0x144853048], NOT in the worker MANAGER. If the pool is NULL cold, cold-building it
    // (0x14240afe0) is the untested save-safe lever; if non-null cold, the read fails elsewhere.
    // CORRECTION (autoresearch 2026-06-18): the "stream task" read is actually
    // upstream's `runtime_heap_allocator` (DLAllocator) -- always non-null, so the
    // `fd4_stream_task_present` signal is meaningless. Resolve it through fromsoftware-rs.
    const FD4_IO_POOL_RVA: usize = RuntimeGlobalRva::Fd4IoPool as usize;
    // Kept under the local name for its call sites; the object is SaveLoad2::SLSystemImpl,
    // not an FD4 IO worker manager (corrected 2026-08-01).
    const FD4_IO_WORKER_MANAGER_RVA: usize = RuntimeGlobalRva::SaveLoad2SlSystemImpl as usize;
    const IO_DEVICE_SINGLETON_RVA: usize = RuntimeGlobalRva::IoDeviceSingleton as usize;
    const IO_DEVICE_INFLIGHT_10_OFFSET: usize =
        core::mem::offset_of!(IoDeviceSnapshotLayout, inflight);
    const IO_DEVICE_REQHANDLE_20_OFFSET: usize =
        core::mem::offset_of!(IoDeviceSnapshotLayout, request_handle);
    let io_pool = unsafe { crate::experiments::safe_read_usize(base + FD4_IO_POOL_RVA) }
        .unwrap_or(NULL_POINTER_VALUE);
    let io_worker_manager =
        unsafe { crate::experiments::safe_read_usize(base + FD4_IO_WORKER_MANAGER_RVA) }
            .unwrap_or(NULL_POINTER_VALUE);
    let stream_task = crate::runtime_heap_allocator_ptr_or_null();
    let io_device = unsafe { crate::experiments::safe_read_usize(base + IO_DEVICE_SINGLETON_RVA) }
        .unwrap_or(NULL_POINTER_VALUE);
    let io_inflight = if io_device == NULL_POINTER_VALUE {
        None
    } else {
        unsafe { crate::experiments::safe_read_usize(io_device + IO_DEVICE_INFLIGHT_10_OFFSET) }
    };
    let io_reqhandle = if io_device == NULL_POINTER_VALUE {
        None
    } else {
        unsafe { crate::experiments::safe_read_usize(io_device + IO_DEVICE_REQHANDLE_20_OFFSET) }
    };

    body.push_str("  \"save_snapshot_available\": true,\n");
    body.push_str(&format!(
        "  \"fd4_io_pool_present\": {},\n",
        io_pool != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"fd4_io_worker_manager_present\": {},\n",
        io_worker_manager != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"fd4_stream_task_present\": {},\n",
        stream_task != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"io_device_present\": {},\n",
        io_device != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"io_device_inflight_10\": {},\n",
        io_inflight.map_or_else(|| "null".to_owned(), |value| format!("\"{value:#x}\""))
    ));
    body.push_str(&format!(
        "  \"io_device_reqhandle_20\": {},\n",
        io_reqhandle.map_or_else(|| "null".to_owned(), |value| format!("\"{value:#x}\""))
    ));
    body.push_str(&format!(
        "  \"game_data_man_present\": {},\n",
        game_data_man != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"profile_summary_present\": {},\n",
        profile_summary != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"profile_slot_active_bytes_qword\": {},\n",
        slot_active_bytes.map_or_else(|| "null".to_owned(), |value| format!("\"{value:#x}\""))
    ));
    body.push_str(&format!(
        "  \"game_save_deserialize_ready_df0\": {},\n",
        deserialize_ready.map_or_else(|| "null".to_owned(), |value| format!("\"{value:#x}\""))
    ));
    // Corrupted-save SEMAPHORE: the GR_System_Message id (0 = none) the game fetched for a "save data
    // is corrupted" dialog -- our RAM-read detector for that popup (the gold save was read but rejected
    // on validate/write). See CORRUPTED_SAVE_MSG_IDS.
    body.push_str(&format!(
        "  \"oracle_corrupted_save_seen_id\": {},\n  \"oracle_corrupted_save_load_failed_seen_id\": {},\n  \"oracle_corrupted_save_seen_count\": {},\n  \"oracle_corrupted_save_seen_caller_rva\": \"{:#x}\",\n",
        crate::experiments::CORRUPTED_SAVE_SEEN_ID.load(Ordering::SeqCst),
        crate::experiments::CORRUPTED_SAVE_LOAD_FAILED_SEEN_ID.load(Ordering::SeqCst),
        crate::experiments::CORRUPTED_SAVE_SEEN_COUNT.load(Ordering::SeqCst),
        crate::experiments::CORRUPTED_SAVE_SEEN_CALLER_RVA.load(Ordering::SeqCst)
    ));
    // PRIVACY-POLICY SEMAPHORE (privacy-policy-gated-on-character-presence-CONFIRMED-2026-06-23):
    // this is a pre-render character/profile-summary gate, not evidence that a ToS/policy renderer was
    // reached. The Bandai-Namco PRIVACY POLICY boot screen appears iff the active ProfileSummary exists
    // but reports ZERO active slots (`slot_active_bytes == 0`, no character). When a gold/native-profile
    // load is expected (not telemetry-only), `true` means the profile summary was not populated before
    // the title gate, so the native menu / Continue / ProfileSelect renderer path will not be reached.
    // On a real loaded profile this is false (at least one active slot -> policy skipped). Do not fix a
    // true value by pressing E/OK or by suppressing the policy UI; satisfy the underlying native profile
    // read/summary-population precondition so the gate is false before row/portrait rendering.
    let privacy_policy_gate = profile_summary != NULL_POINTER_VALUE
        && slot_active_bytes == Some(0)
        && !crate::experiments::save_override_telemetry_only();
    body.push_str(&format!(
        "  \"oracle_privacy_policy_gate\": {privacy_policy_gate},\n"
    ));
    // SPLASH-SKIP SEMAPHORE (splash-skip-correctness): the only failure mode of the BeginLogo logo
    // skip is the je->jg branch flip at base+SPLASH_SKIP_RVA not being live (never applied, or
    // reverted by Arxan / another mod). So read that .text byte directly each telemetry frame:
    //   jg (0x7f) = patch LIVE -> STEP_BeginLogo falls through past the ESRB/illegal-copy logo build
    //               (the logos are skipped, the title advances SetState(2)->(3) without them);
    //   je (0x74) = UNPATCHED -> splash will play;
    //   anything else = corrupted/reverted -> splash-skip is BROKEN.
    // apply_splash_skip runs at DLL attach (before the title runs state 2), so by the time telemetry
    // writes (at the title/menu) a live jg means the skip already executed this boot. This is the
    // in-process detector that was MISSING for "are we correctly skipping the splash screens".
    if let Ok(base) = crate::experiments::game_module_base() {
        let splash_byte =
            unsafe { crate::experiments::safe_read_u8(base + crate::SPLASH_SKIP_RVA) }.unwrap_or(0);
        body.push_str(&format!(
            "  \"oracle_splash_skip_armed\": {},\n  \"oracle_splash_skip_patch_byte\": \"{:#x}\",\n",
            splash_byte == crate::SPLASH_SKIP_REPLACEMENT_JG,
            splash_byte
        ));
    }
    // AUDIO SEMAPHORE: actual Wwise PostEvent submissions. This catches audible-only regressions
    // (for example startup/title-logo music) that can block the later title/load flow without a useful
    // screenshot oracle. The hook is observe-only and forwards every event unchanged.
    body.push_str(&format!(
        "  \"oracle_sound_post_event_hook_installed\": {},\n  \"oracle_sound_post_event_hits\": {},\n  \"oracle_sound_post_event_muted_hits\": {},\n  \"oracle_sound_post_event_forwarded_hits\": {},\n  \"oracle_sound_post_event_first_id\": {},\n  \"oracle_sound_post_event_last_id\": {},\n  \"oracle_sound_post_event_first_muted_id\": {},\n  \"oracle_sound_post_event_last_muted_id\": {},\n  \"oracle_sound_post_event_last_playing_id\": {},\n  \"oracle_sound_post_event_last_game_object\": \"{:#x}\",\n  \"oracle_sound_post_event_last_flags\": \"{:#x}\",\n  \"oracle_sound_post_event_last_caller_rva\": \"{:#x}\",\n",
        crate::SOUND_POST_EVENT_CORE_INSTALLED.load(Ordering::SeqCst) != 0,
        crate::SOUND_POST_EVENT_HITS.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_MUTED_HITS.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_FORWARDED_HITS.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_FIRST_ID.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_LAST_ID.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_FIRST_MUTED_ID.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_LAST_MUTED_ID.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_LAST_PLAYING_ID.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_LAST_GAME_OBJECT.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_LAST_FLAGS.load(Ordering::SeqCst),
        crate::SOUND_POST_EVENT_LAST_CALLER_RVA.load(Ordering::SeqCst)
    ));
    // oracle_continue_ready_stage / _scan_node_hits / _dialog_vt REMOVED 2026-06-24: they were the
    // diagnostic for the native_continue Continue-node scan (CONTINUE_READY_STAGE/SCAN_NODE_HITS/
    // DIALOG_VT_SEEN), which was ripped out as dead code -- the scan never found the node and the
    // zero-input load fires via pab-advance + title-accept-byte instead.
}

pub(crate) fn telemetry_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_TELEMETRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-telemetry.json"))
}

pub(crate) fn write_policy_oracle_snapshot(reason: &str) {
    let path = telemetry_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let seamless_loaded = seamless_coop_loaded();
    let policy_total_builds = POLICY_TOS_TITLE_TOTAL_BUILDS.load(Ordering::SeqCst);
    let policy_any_seen = policy_total_builds != MENU_TRACE_UNSEEN_SEQ;
    let server_status_total_seen = SERVER_STATUS_TOTAL_SEEN.load(Ordering::SeqCst);
    let server_status_any_seen = server_status_total_seen != MENU_TRACE_UNSEEN_SEQ;
    let body = format!(
        "{{\n  \"player_available\": false,\n  \"player_seen\": false,\n  \"runtime_mode\": \"{}\",\n  \"seamless_coop_loaded\": {},\n  \"telemetry_source\": \"policy_oracle_snapshot\",\n  \"telemetry_snapshot_reason\": \"{}\",\n  \"simulated_button_presses_total\": 0,\n  \"oracle_policy_window_total_builds\": {},\n  \"oracle_policy_window_any_seen\": {},\n  \"oracle_policy_window_ptr\": {},\n  \"oracle_policy_window_vtable\": {},\n  \"oracle_policy_window_stack_arg0\": {},\n  \"oracle_policy_window_backing_flag_ptr\": {},\n  \"oracle_policy_window_stored_backing_flag_ptr\": {},\n  \"oracle_policy_window_backing_flag_value\": {},\n  \"oracle_policy_window_requested_flag_value\": {},\n  \"oracle_policy_window_caller_rva\": {},\n  \"oracle_policy_ctor_wrapper_hits\": {},\n  \"oracle_policy_ctor_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_wrapper_hits\": {},\n  \"oracle_policy_selector_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_ctor_hits\": {},\n  \"oracle_policy_selector_ctor_requested_flag_value\": {},\n  \"oracle_policy_selector_ctor_caller_rva\": {},\n  \"oracle_policy_status_predicate_hits\": {},\n  \"oracle_policy_status_predicate_caller_rva\": {},\n  \"oracle_policy_flag_setter_hits\": {},\n  \"oracle_policy_flag_setter_caller_rva\": {},\n  \"oracle_server_status_total_seen\": {},\n  \"oracle_server_status_any_seen\": {},\n  \"oracle_server_status_state\": {},\n  \"oracle_server_status_text_id\": {}\n}}\n",
        if seamless_loaded {
            RUNTIME_MODE_SEAMLESS
        } else {
            RUNTIME_MODE_VANILLA_OR_UNKNOWN
        },
        seamless_loaded,
        json_escape(reason),
        policy_total_builds,
        policy_any_seen,
        POLICY_TOS_TITLE_LAST_THIS.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_VTABLE.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_STACK_ARG0.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_WRAPPER_HITS.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_WRAPPER_HITS.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_CTOR_HITS.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_STATUS_HITS.load(Ordering::SeqCst),
        POLICY_TOS_STATUS_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_FLAG_SETTER_HITS.load(Ordering::SeqCst),
        POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA.load(Ordering::SeqCst),
        server_status_total_seen,
        server_status_any_seen,
        SERVER_STATUS_LAST_STATE.load(Ordering::SeqCst),
        SERVER_STATUS_LAST_TEXT_ID.load(Ordering::SeqCst)
    );
    let tmp_path = path.with_extension("json.tmp");
    if fs::write(&tmp_path, body).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
    write_bootstrap_event(BOOTSTRAP_EVENT_POLICY_TELEMETRY_SNAPSHOT, reason);
}

pub(crate) fn command_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_COMMAND_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-command.txt"))
}

pub(crate) fn json_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            character if character.is_control() => format!("\\u{:04x}", character as u32)
                .chars()
                .collect::<Vec<_>>(),
            character => vec![character],
        })
        .collect()
}

// ENV-GATE RATIONALE: ER_EFFECTS_CRASH_LOG_PATH is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn crash_log_path() -> PathBuf {
    std::env::var("ER_EFFECTS_CRASH_LOG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // CANONICAL name `er-effects-crash-log.txt` -- the SAME file the crash-logger enable
            // sentinel (crash_logger_enabled) and the probe's per-run truncation use. The prior
            // default `er-effects-crash.log` silently diverged from those, so the probe never
            // cleared the real crash log (it accumulated across runs) and readers checked the wrong
            // file (observed 2026-06-22, cost a debug cycle). bd log-output-paths-consolidation.
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-crash-log.txt")
        })
}

/// Monotonic process-attach epoch for self-describing DLL logs. Lazily set on the FIRST log call
/// (close to DLL_PROCESS_ATTACH in practice), so every emitted line carries `[+<elapsed_ms>ms] `
/// measured from that common start -- making ordering and gaps obvious in raw logs without needing
/// the bash launch T0. Mirrors the `TIMELINE_EPOCH` pattern; `Instant` is QPC-backed and works under
/// wine. Kept lock-light: one short lock that returns a u128, never held across the file write.
static PROCESS_LOG_EPOCH: Mutex<Option<Instant>> = Mutex::new(None);

/// Elapsed milliseconds since the process-log epoch (lazily anchored on first call). Cheap: a single
/// short-lived lock, poison-tolerant, no file IO under the lock. `pub(crate)` so the input-trace
/// JSONL stamps its rows on the SAME clock as the `[+Nms]` debug-log prefixes (cross-correlation).
pub(crate) fn process_log_elapsed_ms() -> u128 {
    let mut guard = match PROCESS_LOG_EPOCH.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let epoch = guard.get_or_insert_with(Instant::now);
    epoch.elapsed().as_millis()
}

/// Local wall-clock stamp `YYYY-MM-DD HH:MM:SS:cc` (cc = centiseconds, i.e. ms/10). Absolute time so
/// a line is unambiguous across the accumulated log -- the `[+Nms]` epoch resets every process and
/// cannot tell two runs apart.
#[cfg(windows)]
fn wall_clock_stamp() -> String {
    let mut st = SystemTimeMin::default();
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}:{:02}",
        st.w_year,
        st.w_month,
        st.w_day,
        st.w_hour,
        st.w_minute,
        st.w_second,
        st.w_milliseconds / 10
    )
}

#[cfg(not(windows))]
fn wall_clock_stamp() -> String {
    "0000-00-00 00:00:00:00".to_owned()
}

/// md5 (hex) of the DLL's own on-disk image, so a log names the EXACT build that wrote it (matches
/// the `md5sum` reported for the built DLL). Computed once from `GetModuleFileNameW(SELF_DLL_BASE)`;
/// only a successful result is cached, so a call before the self-base is recorded transiently returns
/// `"unknown"` and the next call retries.
#[cfg(windows)]
fn compute_dll_md5() -> Option<String> {
    use std::os::windows::ffi::OsStringExt;
    let base = SELF_DLL_BASE.load(Ordering::SeqCst);
    if base == 0 || base == NULL_MODULE_BASE {
        return None;
    }
    let mut buf = [0u16; 1024];
    let len = unsafe { GetModuleFileNameW(base as isize, buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 || len as usize >= buf.len() {
        return None;
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(&buf[..len as usize]));
    let bytes = std::fs::read(&path).ok()?;
    let digest = er_save_loader::bnd4::md5_digest(&bytes);
    let mut hex = String::with_capacity(32);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    Some(hex)
}

#[cfg(not(windows))]
fn compute_dll_md5() -> Option<String> {
    None
}

/// Full DLL md5 hex, cached on first success (`"unknown"` until the self-base is recorded).
fn dll_md5_hex() -> &'static str {
    static DLL_MD5: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(cached) = DLL_MD5.get() {
        return cached;
    }
    match compute_dll_md5() {
        Some(hex) => {
            let _ = DLL_MD5.set(hex);
            DLL_MD5.get().map(String::as_str).unwrap_or("unknown")
        }
        None => "unknown",
    }
}

/// Short per-line DLL tag (first 8 hex of the md5) so any single copied line is attributable to a
/// build without carrying the full 32-char digest on every line (the header carries the full md5).
fn dll_md5_short() -> &'static str {
    let full = dll_md5_hex();
    full.get(..8).unwrap_or(full)
}

/// Common line prefix: `[+<elapsed>ms] <wall-clock> dll:<short>`. `[+Nms]` stays first so existing
/// `^\[\+(\d+)ms\]`-anchored readers keep working; the wall-clock + build tag follow.
fn log_line_prefix() -> String {
    format!(
        "[+{}ms] {} dll:{}",
        process_log_elapsed_ms(),
        wall_clock_stamp(),
        dll_md5_short()
    )
}

/// One-time self-describing header written the first time a given log file is opened this run: the
/// full DLL md5 + path + wall-clock, so the build and start time are unambiguous even when many runs
/// accumulate in the same file. `resolved_path` is the ABSOLUTE path this handle actually opened, so
/// a log found on disk states where it came from and no reader has to guess the process CWD.
fn write_log_header(file: &mut std::fs::File, resolved_path: &std::path::Path) {
    use std::io::Write;
    let _ = writeln!(
        file,
        "===== er-effects log opened {} dll_md5={} (per-line tag `dll:{}`) path={}; [+Nms] = elapsed since this process's first log line =====",
        wall_clock_stamp(),
        dll_md5_hex(),
        dll_md5_short(),
        resolved_path.display()
    );
}

/// Fresh per process, like `append_autoload_debug`: the first line of a run truncates the file
/// (the previous run's survives one generation as `.log.prev`), later lines append. A crash log
/// that accumulated across runs would sit crashes from builds that no longer exist next to the
/// one under test, with only the header line to tell them apart.
pub(crate) fn append_crash_log(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    static HEADER: std::sync::Once = std::sync::Once::new();
    let prefix = log_line_prefix();
    let path = crash_log_path();
    if let Some(mut file) = er_game_base::log::open_fresh_run_append(&path) {
        HEADER.call_once(|| write_log_header(&mut file, &path));
        let _ = writeln!(file, "{prefix} {args}");
    }
}

/// Loading-screen portrait capture check, run at CAPTURE time (every time a portrait RGBA is about to
/// be stored), so a transient wrong-source frame -- our neutral texture flashing in right after Continue
/// (Bug B), or a small head from the current deliberate low-resolution experiment -- cannot slip between
/// the coarse telemetry writes. Records the capture dims + neutral-color fraction, latches the two
/// once-seen bug versions (semaphores), and RETURNS whether this capture is fit to PUBLISH.
///
/// Returns `false` (do NOT publish; hold the previous frame / the loading background) only when the
/// capture is our neutral texture. Small captures are still published: the current 56x56 native-source
/// experiment intentionally relies on scaling a tiny real head up to the full backbuffer. Cheap: a
/// strided sample.
pub(crate) fn note_ls_portrait_capture(w: u32, h: u32, px: &[u8]) -> bool {
    let texels = (w as usize) * (h as usize);
    if texels == 0 || px.len() < texels * 4 {
        return false;
    }
    let [nr, ng, nb, _] = STATS_PANEL_BG_RGBA;
    let tol: i32 = 8;
    let stride = (texels / 2000).max(1);
    let (mut sampled, mut neutral) = (0usize, 0usize);
    let mut i = 0usize;
    while i < texels {
        let b = i * 4;
        let (r, g, bl) = (px[b] as i32, px[b + 1] as i32, px[b + 2] as i32);
        if (r - nr as i32).abs() <= tol
            && (g - ng as i32).abs() <= tol
            && (bl - nb as i32).abs() <= tol
        {
            neutral += 1;
        }
        sampled += 1;
        i += stride;
    }
    let neutral_pct = (neutral * 100).checked_div(sampled).unwrap_or(0);
    LS_PORTRAIT_LAST_W.store(w as usize, Ordering::SeqCst);
    LS_PORTRAIT_LAST_H.store(h as usize, Ordering::SeqCst);
    LS_PORTRAIT_LAST_NEUTRAL_PCT.store(neutral_pct, Ordering::SeqCst);
    // Use the version this capture will carry (bumped by the caller right after the store); reading it
    // here is close enough for a first-seen stamp.
    let version = LOADING_BG_PORTRAIT_RGBA_VERSION
        .load(Ordering::SeqCst)
        .max(1);
    let is_neutral = neutral_pct >= 90;
    let is_small = w.max(h) <= LS_PORTRAIT_SMALL_MAX_SIDE;
    if is_neutral {
        let _ = LS_PORTRAIT_NEUTRAL_LEAK_SEEN_VERSION.compare_exchange(
            0,
            version,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    } else if is_small {
        let _ = LS_PORTRAIT_TOO_SMALL_SEEN_VERSION.compare_exchange(
            0,
            version,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    // Publishable unless it is our NEUTRAL texture (Bug B) -- that must never reach the loading screen.
    // We deliberately do NOT reject the too-small case: the current experiment intentionally renders a
    // tiny 56x56 native portrait and scales it up to test whether quality is related to choppiness.
    // `is_small` still latches its semaphore for monitoring. Rejected frames are counted so a monitor can
    // see the neutral-texture gate working.
    let _ = is_small;
    let publishable = !is_neutral;
    if !publishable {
        LS_PORTRAIT_REJECTED_PUBLISHES.fetch_add(1, Ordering::SeqCst);
        // ATTRIBUTION (er-effects-rs-k979). A bare reject count cannot distinguish the gate doing
        // its job from the pipeline breaking, so a proof gating on "zero rejects" failed healthy
        // runs. Stamp WHY (the neutral share that tripped it) and WHEN (this capture's version),
        // and split on whether anything has ever published cleanly: before the first clean publish
        // this is warm-up -- the offscreen RT is still the blank background and refusing it is
        // correct -- while after it means the pipeline began emitting blanks mid-window.
        er_telemetry::counters::LS_PORTRAIT_REJECT_LAST_VERSION.store(version, Ordering::SeqCst);
        er_telemetry::counters::LS_PORTRAIT_REJECT_LAST_NEUTRAL_PCT
            .store(neutral_pct, Ordering::SeqCst);
        let warmup = reject_is_warmup(
            LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
            er_telemetry::counters::LS_PORTRAIT_REJECT_PUBLISH_BASELINE.load(Ordering::SeqCst),
        );
        if warmup {
            er_telemetry::counters::LS_PORTRAIT_REJECTS_BEFORE_WINDOW_PUBLISH
                .fetch_add(1, Ordering::SeqCst);
        } else {
            er_telemetry::counters::LS_PORTRAIT_REJECTS_AFTER_WINDOW_PUBLISH
                .fetch_add(1, Ordering::SeqCst);
        }
    }
    publishable
}

/// Is a rejected capture pipeline WARM-UP rather than a fault? (er-effects-rs-k979)
///
/// Split on whether THIS WINDOW has published cleanly yet. Before its first clean publish the
/// offscreen RT is still the blank background, so a >=90%-neutral frame is expected and refusing it
/// is the gate working -- measured in run slot-portrait-proof-20260731-130803, where the neutral
/// frame was capture version 1, 2 of 1542 were refused, and all 1540 publishes were clean. After a
/// clean publish the same refusal means the pipeline started emitting blanks mid-window, which is a
/// real defect and the thing a proof should fail on.
///
/// The baseline is essential, not decoration: `LOADING_BG_PORTRAIT_RGBA_VERSION` is cumulative for
/// the whole PROCESS (that 3-window run ended at 1540 and never reset), so comparing it against 0
/// would mark every window after the first as "already published" and misfile its warm-up reject as
/// a fault -- reintroducing the very failure this change removes, one window later.
///
/// Pure so the distinction is unit-testable: the live event is intermittent and did not recur on
/// the validation run, so waiting for it would leave the classification unproven.
fn reject_is_warmup(published_rgba_version: usize, window_baseline: usize) -> bool {
    published_rgba_version <= window_baseline
}

#[cfg(test)]
mod portrait_reject_attribution_tests {
    use super::reject_is_warmup;

    #[test]
    fn a_reject_before_this_window_published_is_warmup() {
        assert!(reject_is_warmup(0, 0));
    }

    #[test]
    fn a_reject_after_this_window_published_is_not_warmup() {
        // The defect this split exists to surface: the window published fine, then began
        // producing blank frames.
        assert!(!reject_is_warmup(1, 0));
        assert!(!reject_is_warmup(1540, 1400));
    }

    #[test]
    fn a_later_windows_warmup_is_still_warmup() {
        // The regression this baseline exists to prevent. Window 2 opens at cumulative version
        // 1400; a reject before it publishes anything is warm-up, NOT a fault, even though the
        // process-wide counter is long past zero.
        assert!(reject_is_warmup(1400, 1400));
    }
}

/// DEFAULT-OFF marker gate for the `append_autoload_debug` firehose (Phase B decoupled diagnostics,
/// bd decoupled-diagnostics-architecture-buildplan-2026-07-24). Env vars do NOT cross me3/Proton, so
/// the enable is a game-dir marker file `er-effects-autoload-debug.txt` checked via `.exists()` and
/// cached once. This is a PURELY DIAGNOSTIC logging toggle -- it changes NO game behavior, only whether
/// the passive debug-log lines are written -- so the armed-vs-disarmed A/B baseline pays zero per-frame
/// log-file cost in both arms. Registered in `.auto/marker_file_gate_baseline.json` diagnostic_gates.
fn autoload_debug_log_enabled() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        game_directory_path()
            .map(|dir| dir.join("er-effects-autoload-debug.txt").exists())
            .unwrap_or(false)
    })
}

/// Bare file name of the autoload debug log, used both for the resolved path and for the
/// last-resort relative fallback.
const AUTOLOAD_DEBUG_LOG_FILE_NAME: &str = "er-effects-autoload-debug.log";

/// Deterministic location for the autoload debug log.
///
/// The old default was the RELATIVE `er-effects-autoload-debug.log`, so under me3/Proton the trace
/// landed in whatever the process CWD happened to be -- measured, that was the APPDATA SAVE
/// directory, nowhere near the game dir. A diagnosis run whose log cannot be found is a wasted user
/// press, so the default now resolves next to the marker file that ENABLES the log
/// (`game_directory_path()`, the same directory `autoload_debug_log_enabled` probes). The explicit
/// `ER_EFFECTS_AUTOLOAD_DEBUG_PATH` override still wins, and the bare relative name survives only as
/// a last resort for the case where the game directory cannot be resolved at all.
fn autoload_debug_log_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("ER_EFFECTS_AUTOLOAD_DEBUG_PATH") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    game_directory_path()
        .map(|dir| dir.join(AUTOLOAD_DEBUG_LOG_FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(AUTOLOAD_DEBUG_LOG_FILE_NAME))
}

/// Nested `append_autoload_debug` calls the re-entrancy guard refused. Non-zero proves the
/// recursion described on [`AutoloadDebugReentryGuard`] is live in this process -- the logger's own
/// file open came back through the logger -- and that the guard, not luck, is what stopped it.
static AUTOLOAD_DEBUG_REENTRANT_DROPS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

std::thread_local! {
    /// True while THIS thread is somewhere inside `append_autoload_debug`.
    static AUTOLOAD_DEBUG_IN_PROGRESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks a thread as being inside `append_autoload_debug` for as long as the guard is alive.
///
/// # Why a logger of all things needs a re-entrancy guard
///
/// Both file operations `append_autoload_debug` performs reach the OS through
/// `kernel32!CreateFileW`: the marker gate's `.exists()` probe (`std::fs::metadata` opens the path
/// with `FILE_FLAG_BACKUP_SEMANTICS` before it will answer), and the log handle's own
/// `OpenOptions::open`. `install_save_file_core_hooks` detours that export in EVERY save mode, and
/// the detour LOGS -- its very first call, and every save-like path -- so the logger's own open
/// arrives straight back in the logger ON THE SAME THREAD.
///
/// Neither primitive the outer call holds at that moment is re-entrant:
///
///   * the marker gate is a `OnceLock` whose initializer is what performs the `.exists()` probe, and
///     re-entering `get_or_init` from inside its own initializer never returns;
///   * the log handle lives behind a `std::sync::Mutex`, and a second `lock()` on one thread never
///     returns either.
///
/// The thread that hits this is the one installing the hook, during DLL attach, and every other
/// thread that logs afterwards queues behind it -- so the symptom is the game hanging at boot having
/// written nothing. Nested lines are DROPPED and counted: a line describing the opening of the log
/// is worth nothing, and the outer line it interrupted is still written normally.
struct AutoloadDebugReentryGuard;

impl AutoloadDebugReentryGuard {
    /// `None` when this thread is already inside `append_autoload_debug`, or when the thread-local
    /// flag cannot be reached at all (thread teardown). An unanswerable "am I nested?" counts as
    /// nested: dropping a diagnostic line costs nothing, and guessing wrong hangs the game.
    fn enter() -> Option<Self> {
        let entered = AUTOLOAD_DEBUG_IN_PROGRESS
            .try_with(|in_progress| !in_progress.replace(true))
            .unwrap_or(false);
        if entered {
            Some(Self)
        } else {
            AUTOLOAD_DEBUG_REENTRANT_DROPS.fetch_add(1, Ordering::SeqCst);
            None
        }
    }
}

impl Drop for AutoloadDebugReentryGuard {
    fn drop(&mut self) {
        let _ = AUTOLOAD_DEBUG_IN_PROGRESS.try_with(|in_progress| in_progress.set(false));
    }
}

// ENV-GATE RATIONALE: ER_EFFECTS_AUTOLOAD_DEBUG_PATH is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    // PHASE B DECOUPLED DIAGNOSTICS: this per-frame firehose is DEFAULT-OFF. Return before ANY file I/O
    // unless the `er-effects-autoload-debug.txt` marker is present, so the armed-vs-disarmed A/B baseline
    // has a ZERO-LOG cost in both arms (no per-frame log-file-I/O confound). Cached; no game behavior.
    // RE-ENTRANCY, checked BEFORE the marker gate: the gate's own `.exists()` probe is a
    // `CreateFileW` and therefore re-enters the save-destination detour, so the recursion is
    // reachable before the gate has even decided whether logging is on. See
    // `AutoloadDebugReentryGuard`.
    let Some(_not_nested) = AutoloadDebugReentryGuard::enter() else {
        return;
    };
    if !autoload_debug_log_enabled() {
        return;
    }
    use std::io::Write;
    // FPS FIX (bd fps-fix-not-confirmed-new-suspect-perframe-debug-logging): the old path did a full file
    // OPEN + write + CLOSE on EVERY call (3 syscalls/line). The DLL logs heavily during loads/transitions
    // (per-frame WORLDRES-GETTER phase changes, oracles, etc.), so that per-call open/close tanked the
    // framerate exactly when the user sees it. Keep ONE persistent handle: open+truncate+header once, then
    // only writeln thereafter -- no per-call open/close. Same output, a fraction of the syscalls.
    static LOG: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
    // Serializes the ONE truncating open so `LOG` is never held across file I/O and two threads can
    // never both truncate the file. Taken only after `LOG` was found empty.
    static LOG_OPEN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let prefix = log_line_prefix();
    let write_through_open_handle = || -> bool {
        let mut guard = LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.as_mut() {
            Some(file) => {
                let _ = writeln!(file, "{prefix} {args}");
                true
            }
            None => false,
        }
    };
    if write_through_open_handle() {
        return;
    }
    // NO FILE I/O UNDER `LOG` -- the open below re-enters the `CreateFileW` detour, which takes
    // locks of its own (the save-destination redirect lock, during an armed commit), so holding
    // `LOG` across it invites the reverse lock order from any thread that logs while holding one of
    // them. The guard above stops the same-thread recursion; keeping the open outside `LOG` stops
    // that inversion.
    let _opening = LOG_OPEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Another thread may have opened the log while this one waited for the gate.
    if write_through_open_handle() {
        return;
    }
    // TRUNCATE ONCE per process so each run starts a CLEAN log (matches the trace DLL's reset-on-attach).
    let path = autoload_debug_log_path();
    // Keep the PREVIOUS run one generation as `.log.prev` instead of destroying it -- this is the
    // most-read log in the repo and the truncation below is otherwise final. Safe under `LOG_OPEN`:
    // `begin_fresh_run` never holds its own registry lock across file I/O (so there is no reverse
    // order against `LOG_OPEN`), and the re-entrancy guard held by this thread makes the rename's
    // trip through the `CreateFileW` detour come straight back out of this function.
    er_game_base::log::begin_fresh_run(&path);
    let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
    else {
        // Deliberately not latched: a directory that is not writable yet may be later, and the next
        // line retries. Nothing is published, so no reader sees a half-opened log.
        return;
    };
    write_log_header(&mut file, &path);
    let _ = writeln!(file, "{prefix} {args}");
    *LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(file);
}

#[cfg(test)]
mod autoload_debug_log_tests {
    use super::*;

    /// The drop counter and the thread-local flag are process-global, so these tests take turns.
    static AUTOLOAD_DEBUG_GUARD_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serialize() -> std::sync::MutexGuard<'static, ()> {
        AUTOLOAD_DEBUG_GUARD_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn nested_drops() -> usize {
        AUTOLOAD_DEBUG_REENTRANT_DROPS.load(Ordering::SeqCst)
    }

    /// One entry per thread at a time, and the flag must clear when the guard dies -- a guard that
    /// leaked its flag would silence every later line on that thread instead of deadlocking, which
    /// is a quieter version of the same bug.
    #[test]
    fn a_second_entry_on_the_same_thread_is_refused_until_the_first_is_dropped() {
        let _serialized = serialize();
        let before = nested_drops();
        let outer = AutoloadDebugReentryGuard::enter().expect("a fresh thread is not nested");
        assert!(AutoloadDebugReentryGuard::enter().is_none());
        assert_eq!(nested_drops(), before + 1);
        drop(outer);
        assert!(AutoloadDebugReentryGuard::enter().is_some());
        assert_eq!(nested_drops(), before + 1);
    }

    /// The guard must be per THREAD: the game logs from the task thread, the save worker and the
    /// hook installer at once, and a process-wide flag would drop most of the log.
    #[test]
    fn the_guard_is_per_thread_not_per_process() {
        let _serialized = serialize();
        let outer = AutoloadDebugReentryGuard::enter().expect("a fresh thread is not nested");
        let other_thread_entered =
            std::thread::spawn(|| AutoloadDebugReentryGuard::enter().is_some())
                .join()
                .expect("thread joins");
        assert!(other_thread_entered);
        drop(outer);
    }

    /// The real logger's FIRST action is the guard, so a line arriving from inside itself -- which is
    /// what the `CreateFileW` detour does when the log's own open re-enters it -- returns without
    /// reaching the marker gate's `OnceLock` or the log `Mutex`. Before this, that call re-locked
    /// both and hung the calling thread.
    #[test]
    fn append_autoload_debug_drops_a_line_that_arrives_from_inside_itself() {
        let _serialized = serialize();
        let before = nested_drops();
        let outer = AutoloadDebugReentryGuard::enter().expect("stand in for the outer log call");
        append_autoload_debug(format_args!("nested line from inside the logger"));
        assert_eq!(nested_drops(), before + 1);
        drop(outer);
    }
}

/// Wall-clock epoch for the load-timeline markers. Lazily set on the FIRST `timeline_event`
/// call (which is T0 by construction -- the first frame the title is parked at state 10),
/// so every subsequent `ms=` is measured from that common start. `Instant` is QPC-backed on
/// the windows target and works under wine, so no new FFI is needed.
static TIMELINE_EPOCH: Mutex<Option<Instant>> = Mutex::new(None);

/// Emit a frame-stamped load-timeline marker so one parser handles BOTH a native-menu load
/// (observe mode) and a DLL-driven load (own-stepper). Format (greppable, single regex):
///   `EVENT <name> frame=<n> ms=<elapsed-from-T0> <fields>`
/// `frame` is the monotonic per-frame `game_task_ticks`; `ms` is wall-clock from the first
/// event. Edge-triggering (fire each marker once) is the caller's responsibility.
pub(crate) fn timeline_event(name: &str, frame: u64, fields: std::fmt::Arguments<'_>) {
    let ms = {
        let mut guard = match TIMELINE_EPOCH.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let epoch = guard.get_or_insert_with(Instant::now);
        epoch.elapsed().as_millis()
    };
    append_autoload_debug(format_args!("EVENT {name} frame={frame} ms={ms} {fields}"));
}

// ENV-GATE RATIONALE: ER_EFFECTS_TRACE_CONTINUE_PATH is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn continue_trace_log_path() -> PathBuf {
    std::env::var("ER_EFFECTS_TRACE_CONTINUE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-continue-trace.log")
        })
}

pub(crate) fn game_directory_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
}

/// Fresh per process: one Continue trace per run. Two runs' traces in one file cannot be told
/// apart by a reader counting transitions, which is the whole use of this file.
pub(crate) fn append_continue_trace(args: std::fmt::Arguments<'_>) {
    er_game_base::log::append_line(&continue_trace_log_path(), args);
}
