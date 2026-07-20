
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
    const FD4_IO_WORKER_MANAGER_RVA: usize = RuntimeGlobalRva::Fd4IoWorkerManager as usize;
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
    let msgbox_total_builds = MSGBOX_TOTAL_BUILDS.load(Ordering::SeqCst);
    let msgbox_any_seen = msgbox_total_builds != MENU_TRACE_UNSEEN_SEQ;
    let server_status_total_seen = SERVER_STATUS_TOTAL_SEEN.load(Ordering::SeqCst);
    let server_status_any_seen = server_status_total_seen != MENU_TRACE_UNSEEN_SEQ;
    let body = format!(
        "{{\n  \"player_available\": false,\n  \"player_seen\": false,\n  \"runtime_mode\": \"{}\",\n  \"seamless_coop_loaded\": {},\n  \"telemetry_source\": \"policy_oracle_snapshot\",\n  \"telemetry_snapshot_reason\": \"{}\",\n  \"simulated_button_presses_total\": 0,\n  \"oracle_msgbox_total_builds\": {},\n  \"oracle_msgbox_any_seen\": {},\n  \"oracle_msgbox_builder_args\": [{}, {}, {}, {}],\n  \"oracle_policy_window_total_builds\": {},\n  \"oracle_policy_window_any_seen\": {},\n  \"oracle_policy_window_ptr\": {},\n  \"oracle_policy_window_vtable\": {},\n  \"oracle_policy_window_stack_arg0\": {},\n  \"oracle_policy_window_backing_flag_ptr\": {},\n  \"oracle_policy_window_stored_backing_flag_ptr\": {},\n  \"oracle_policy_window_backing_flag_value\": {},\n  \"oracle_policy_window_requested_flag_value\": {},\n  \"oracle_policy_window_caller_rva\": {},\n  \"oracle_policy_ctor_wrapper_hits\": {},\n  \"oracle_policy_ctor_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_wrapper_hits\": {},\n  \"oracle_policy_selector_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_ctor_hits\": {},\n  \"oracle_policy_selector_ctor_requested_flag_value\": {},\n  \"oracle_policy_selector_ctor_caller_rva\": {},\n  \"oracle_policy_status_predicate_hits\": {},\n  \"oracle_policy_status_predicate_caller_rva\": {},\n  \"oracle_policy_flag_setter_hits\": {},\n  \"oracle_policy_flag_setter_caller_rva\": {},\n  \"oracle_server_status_total_seen\": {},\n  \"oracle_server_status_any_seen\": {},\n  \"oracle_server_status_state\": {},\n  \"oracle_server_status_text_id\": {}\n}}\n",
        if seamless_loaded {
            RUNTIME_MODE_SEAMLESS
        } else {
            RUNTIME_MODE_VANILLA_OR_UNKNOWN
        },
        seamless_loaded,
        json_escape(reason),
        msgbox_total_builds,
        msgbox_any_seen,
        MSGBOX_LAST_ARG_RCX.load(Ordering::SeqCst),
        MSGBOX_LAST_ARG_RDX.load(Ordering::SeqCst),
        MSGBOX_LAST_ARG_R8.load(Ordering::SeqCst),
        MSGBOX_LAST_ARG_R9.load(Ordering::SeqCst),
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
/// accumulate in the same file.
fn write_log_header(file: &mut std::fs::File) {
    use std::io::Write;
    let _ = writeln!(
        file,
        "===== er-effects log opened {} dll_md5={} (per-line tag `dll:{}`); [+Nms] = elapsed since this process's first log line =====",
        wall_clock_stamp(),
        dll_md5_hex(),
        dll_md5_short()
    );
}

pub(crate) fn append_crash_log(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    static HEADER: std::sync::Once = std::sync::Once::new();
    let prefix = log_line_prefix();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(crash_log_path())
    {
        HEADER.call_once(|| write_log_header(&mut file));
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
    let neutral_pct = if sampled > 0 {
        neutral * 100 / sampled
    } else {
        0
    };
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
    }
    publishable
}

// ENV-GATE RATIONALE: ER_EFFECTS_AUTOLOAD_DEBUG_PATH is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    // FPS FIX (bd fps-fix-not-confirmed-new-suspect-perframe-debug-logging): the old path did a full file
    // OPEN + write + CLOSE on EVERY call (3 syscalls/line). The DLL logs heavily during loads/transitions
    // (per-frame WORLDRES-GETTER phase changes, oracles, etc.), so that per-call open/close tanked the
    // framerate exactly when the user sees it. Keep ONE persistent handle: open+truncate+header once, then
    // only writeln thereafter -- no per-call open/close. Same output, a fraction of the syscalls.
    static LOG: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
    let prefix = log_line_prefix();
    let mut guard = match LOG.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.is_none() {
        // TRUNCATE ONCE per process so each run starts a CLEAN log (matches the trace DLL's reset-on-attach).
        let path = std::env::var("ER_EFFECTS_AUTOLOAD_DEBUG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("er-effects-autoload-debug.log"));
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
        {
            write_log_header(&mut file);
            *guard = Some(file);
        }
    }
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "{prefix} {args}");
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

pub(crate) fn trace_continue_default_path() -> PathBuf {
    game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-trace-continue.txt")
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

pub(crate) fn append_continue_trace(args: std::fmt::Arguments<'_>) {
    use std::io::Write;

    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(continue_trace_log_path())
    {
        let _ = writeln!(file, "{args}");
    }
}
