use super::*;

static PROFILE_05_010_EDITOR_GFX_CACHE: OnceLock<Mutex<Vec<Vec<u8>>>> = OnceLock::new();
static TEXT_INPUT_02_990_RUNTIME_EDITED: OnceLock<Vec<u8>> = OnceLock::new();
static TEXT_INPUT_02_990_RUNTIME_SERVES: AtomicUsize = AtomicUsize::new(0);
static TEXT_INPUT_02_990_RUNTIME_FAILURES: AtomicUsize = AtomicUsize::new(0);
static TEXT_INPUT_02_990_CANONICAL_URL: &[u8] = b"data0:/menu/win/02_990_textinput.gfx\0";

pub(crate) fn install_profile_select_table_diag_hook() {
    if PROFILE_SELECT_TABLE_DIAG_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "profileselect-table-diag: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(PROFILE_RENDERER_REFRESH_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            profile_select_table_diag_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            PROFILE_SELECT_TABLE_DIAG_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "profileselect-table-diag: queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            crate::mh::leak_installed_hook(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "profileselect-table-diag: MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            PROFILE_SELECT_TABLE_DIAG_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "profileselect-table-diag: hooked native profile builder 0x{target:x} (read-only table-state trace)"
            ));
        }
        status => append_autoload_debug(format_args!(
            "profileselect-table-diag: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

pub(crate) fn install_profile_renderer_teardown_spare_hook() {
    if PROFILE_RENDERER_TEARDOWN_HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "loading-portrait: teardown-spare MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(PROFILE_RENDERER_TEARDOWN_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            profile_renderer_teardown_spare_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            PROFILE_RENDERER_TEARDOWN_HOOK_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "loading-portrait: teardown-spare queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            crate::mh::leak_installed_hook(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "loading-portrait: teardown-spare MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            PROFILE_RENDERER_TEARDOWN_HOOK_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "loading-portrait: hooked profile-renderer teardown 0x{target:x} to spare slot0 for the now-loading portrait"
            ));
        }
        status => append_autoload_debug(format_args!(
            "loading-portrait: teardown-spare MH_ApplyQueued failed: {status:?}"
        )),
    }
}

/// Build (once, cached for the process lifetime) the neutral-background TPF003 blob for a stats-panel
/// slot: a solid `STATS_PANEL_BG_RGBA` `STATS_PANEL_TEX_DIM` square, uncompressed legacy-RGBA8 DDS,
/// wrapped in a one-entry TPF whose ENTRY NAME == the slot's `STATS_PANEL_SYSTEX_KEYS` (which becomes
/// the GLOBAL_TexRepository GPU key). Held alive forever so the engine's DEFERRED GPU upload can never
/// read freed bytes (same lifetime discipline the er-tpf cover used). Pure CPU; no native call, no disk.
pub(crate) fn stats_panel_tpf_blob(slot: usize) -> Option<&'static [u8]> {
    static BLOBS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    let blobs = BLOBS.get_or_init(|| {
        (0..STATS_PANEL_SLOT_COUNT)
            .map(|s| {
                let img = er_tpf::DdsImage::solid(
                    STATS_PANEL_TEX_DIM,
                    STATS_PANEL_TEX_DIM,
                    STATS_PANEL_BG_RGBA,
                );
                let dds = img.to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
                er_tpf::Tpf::single_pc(STATS_PANEL_SYSTEX_KEYS[s], dds, 1)
                    .build()
                    .unwrap_or_default()
            })
            .collect()
    });
    match blobs.get(slot) {
        Some(b) if !b.is_empty() => Some(b.as_slice()),
        _ => None,
    }
}

/// Stats-panel product mode: register the neutral-background texture for each ProfileSelect save slot
/// under its unique `STATS_PANEL_SYSTEX_KEYS` via the engine's own in-memory `CS::CreateTpfResCap`
/// factory -- the SAME proven raw-(ptr,len) TPF->GPU path the er-tpf cover and the now-loading forge
/// use. Self-gating + fail-closed: runs on the CSTaskImp game task (post-gfx-init), validates every
/// precondition before the first native call, wraps each call in `catch_unwind`, and only latches a
/// slot's registered bit on a non-null TpfResCap -- so a not-yet-initialized repo (null during boot)
/// simply retries next tick and never crashes. Idempotent per slot via `STATS_PANEL_TEX_REGISTERED_MASK`.
/// The visible-surface redirect is a separate step in the Scaleform bind observer, gated on each slot's
/// registered bit. A texture upload is cheap (no per-frame render), so all 10 slots register with no
/// GX-queue cost -- unlike driving 10 concurrent CSMenuProfModelRend renderers (the 0x1aeaf05 crash).
pub(crate) unsafe fn maybe_register_stats_panel_textures(base: usize) {
    if !stats_panel_enabled() {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_BASE_UNRESOLVED, Ordering::SeqCst);
        return;
    }
    let all: usize = (1 << STATS_PANEL_SLOT_COUNT) - 1;
    if STATS_PANEL_TEX_REGISTERED_MASK.load(Ordering::SeqCst) & all == all {
        return; // every slot already registered
    }
    // Both repos non-null == graphics/repos initialized. Bail (retry next tick) if not ready yet; do
    // NOT consume any register attempt, so boot-time nulls never burn a slot.
    let tpf_repo = unsafe { safe_read_usize(base + GLOBAL_TPF_REPOSITORY_RVA) }.unwrap_or(0);
    if tpf_repo == 0 || tpf_repo == null {
        STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_TPF_REPO_NULL, Ordering::SeqCst);
        return;
    }
    let tex_repo = unsafe { safe_read_usize(base + GLOBAL_TEX_REPOSITORY_RVA) }.unwrap_or(0);
    if tex_repo == 0 || tex_repo == null {
        STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_TEX_REPO_NULL, Ordering::SeqCst);
        return;
    }
    let create_rescap: unsafe extern "system" fn(
        usize,
        *const u16,
        *const u8,
        u64,
        u8,
        u32,
    ) -> usize = unsafe { std::mem::transmute(base + CREATE_TPF_RESCAP_RVA) };
    for (slot, systex_key) in STATS_PANEL_SYSTEX_KEYS
        .iter()
        .enumerate()
        .take(STATS_PANEL_SLOT_COUNT)
    {
        if STATS_PANEL_TEX_REGISTERED_MASK.load(Ordering::SeqCst) & (1 << slot) != 0 {
            continue;
        }
        let Some(tpf_bytes) = stats_panel_tpf_blob(slot) else {
            STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_BLOB_EMPTY, Ordering::SeqCst);
            continue;
        };
        let name_z: Vec<u16> = systex_key
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();
        STATS_PANEL_TEX_REGISTER_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
        let ptr = tpf_bytes.as_ptr();
        let len = tpf_bytes.len() as u64;
        let container = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            create_rescap(tpf_repo, name_z.as_ptr(), ptr, len, 0, 0)
        }));
        match container {
            Ok(c) if c != 0 && c != null => {
                STATS_PANEL_TEX_REGISTERED_MASK.fetch_or(1 << slot, Ordering::SeqCst);
                // Clear the stale boot-time retry marker (repos were null before gfx came up, which set
                // TPF_REPO_NULL); a real register succeeded, so the oracle should read NONE.
                STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_NONE, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "stats-panel: registered neutral bg for slot {slot} key='{}' rescap=0x{c:x} (mask=0x{:x})",
                    STATS_PANEL_SYSTEX_KEYS[slot],
                    STATS_PANEL_TEX_REGISTERED_MASK.load(Ordering::SeqCst)
                ));
            }
            Ok(_) => {
                STATS_PANEL_TEX_REGISTER_FAILURES.fetch_add(1, Ordering::SeqCst);
                STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_RESCAP_NULL, Ordering::SeqCst);
            }
            Err(_) => {
                STATS_PANEL_TEX_REGISTER_FAILURES.fetch_add(1, Ordering::SeqCst);
                STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_PANIC, Ordering::SeqCst);
            }
        }
    }
}

/// Parse the trailing 2-digit slot index (`00`..`09`) from a `systex_menu_profileNN` target DLString.
/// Returns `Some(0..=9)` only for a target that actually looks like the profile SYSTEX key, else `None`
/// (so we never redirect the status-face / kick-face / decorative binds).
pub(crate) unsafe fn systex_profile_target_slot(target_ptr: usize) -> Option<usize> {
    let mut buf = [0u8; 96];
    let n = unsafe { copy_ascii_preview(target_ptr, &mut buf) };
    if n < 2 {
        return None;
    }
    let s = &buf[..n];
    // Lowercase compare against the known prefix so casing never matters.
    let mut lower = [0u8; 96];
    for (i, b) in s.iter().enumerate() {
        lower[i] = b.to_ascii_lowercase();
    }
    let lower = &lower[..n];
    if !lower
        .windows(b"systex_menu_profile".len())
        .any(|w| w == b"systex_menu_profile")
    {
        return None;
    }
    let d1 = s[n - 2];
    let d0 = s[n - 1];
    if !d1.is_ascii_digit() || !d0.is_ascii_digit() {
        return None;
    }
    let slot = ((d1 - b'0') as usize) * 10 + (d0 - b'0') as usize;
    if slot < STATS_PANEL_SLOT_COUNT {
        Some(slot)
    } else {
        None
    }
}

pub(crate) unsafe extern "system" fn title_menu_resource_acquire_observer_hook(
    this: usize,
    load_params: usize,
    param3: u8,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let filename_ptr = if load_params != 0 && load_params != null {
        unsafe { safe_read_usize(load_params + 0x8) }.unwrap_or(null)
    } else {
        null
    };
    let hit = TITLE_MENU_RESOURCE_ACQUIRE_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let caller_rva = trace_first_game_caller_rva();
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_THIS.store(this, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_LOAD_PARAMS.store(load_params, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_FILENAME_PTR.store(filename_ptr, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_PARAM3.store(param3 as usize, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    let is_title_logo = unsafe { wide_ascii_contains_ci(filename_ptr, b"05_001_title_logo") }
        || unsafe { wide_ascii_contains_ci(filename_ptr, b"05_001_title") };

    let orig = TITLE_MENU_RESOURCE_ACQUIRE_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u8) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(this, load_params, param3) }
    } else {
        null
    };
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_RET.store(ret, Ordering::SeqCst);

    if is_title_logo {
        let logo_hit = TITLE_MENU_RESOURCE_ACQUIRE_LOGO_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let mut name = [0u8; 128];
        let name_len = unsafe { copy_wide_ascii_preview(filename_ptr, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: AcquireMenuResource title-logo hit={logo_hit} total={hit} this=0x{this:x} load_params=0x{load_params:x} filename_ptr=0x{filename_ptr:x} filename='{name}' param3={param3} ret=0x{ret:x} caller_rva=0x{caller_rva:x}; observe-only"
        ));
    } else if hit <= 24 {
        let mut name = [0u8; 96];
        let name_len = unsafe { copy_wide_ascii_preview(filename_ptr, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: AcquireMenuResource sample total={hit} filename='{name}' ret=0x{ret:x} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}

/// Product-default 05_000_title strip WITHOUT embedded bytes (er-effects-rs-h7x). `file` is what
/// the native FileOpener just returned for `data0:/menu/05_000_title.gfx`; per the rescap static
/// RE (`FUN_140ce8320`, bd `native-memoryfile-wrapper-expects-gfx-rescap-2026-06-28`) that is a
/// Scaleform MemoryFile whose data/len fields point at the vanilla movie payload owned by
/// `GLOBAL_GfxRepository` (the file object never frees the payload -- the proven synthetic
/// construct path already relied on that). Derive the stripped movie from that payload with
/// `er_gfx::title_05_000::strip` (all-or-nothing content-addressed edits, output verified against
/// the validated-asset fingerprint for the known vanilla input), cache it for the process
/// lifetime, and swap the native file's data/len/cursor onto the cached buffer. ANY failure
/// leaves the native file untouched and returns it as-is: fail-closed to the vanilla title UI,
/// never a crash, never a half-stripped movie.
pub(crate) unsafe fn title_05_000_swap_to_stripped(base: usize, file: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if file == 0 || file == null || file == HOOK_ORIGINAL_UNSET {
        return false;
    }
    let fail = |reason: core::fmt::Arguments<'_>| {
        TITLE_05_000_RUNTIME_STRIP_FAILURES.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "title-resource-observer: 05_000 runtime strip FAIL-CLOSED (serving native vanilla): {reason}"
        ));
        false
    };
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + SCALEFORM_MEMORY_FILE_VTABLE_RVA {
        return fail(format_args!(
            "unexpected file vtable 0x{vtable:x} (want MemoryFile 0x{:x})",
            base + SCALEFORM_MEMORY_FILE_VTABLE_RVA
        ));
    }
    let stripped = match TITLE_05_000_RUNTIME_STRIPPED.get() {
        Some(cached) => cached,
        None => {
            let data =
                unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len =
                unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || data == null || !(64..=0x0100_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            let len = len as usize;
            // Probe both ends through the guarded reader before the bulk copy; the payload is one
            // contiguous repository allocation, so readable ends imply a readable middle.
            let magic_ok = unsafe { safe_read_u8(data) } == Some(b'G')
                && unsafe { safe_read_u8(data + 1) } == Some(b'F')
                && unsafe { safe_read_u8(data + 2) } == Some(b'X')
                && unsafe { safe_read_u8(data + len - 1) }.is_some();
            if !magic_ok {
                return fail(format_args!(
                    "payload at 0x{data:x} len={len} is unreadable or not GFX-magic"
                ));
            }
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
            TITLE_05_000_RUNTIME_STRIP_INPUT_LEN.store(len, Ordering::SeqCst);
            let known = er_gfx::title_05_000::is_known_vanilla(vanilla);
            TITLE_05_000_RUNTIME_STRIP_INPUT_CLASS
                .store(if known { 1 } else { 2 }, Ordering::SeqCst);
            match er_gfx::title_05_000::strip(vanilla) {
                Ok(out) => {
                    TITLE_05_000_RUNTIME_STRIP_OUTPUT_LEN.store(out.len(), Ordering::SeqCst);
                    let validated = out.len() == er_gfx::title_05_000::STRIPPED_LEN
                        && er_gfx::title_05_000::fnv1a64(&out)
                            == er_gfx::title_05_000::STRIPPED_FNV1A64;
                    TITLE_05_000_RUNTIME_STRIP_OUTPUT_VALIDATED
                        .store(if validated { 1 } else { 2 }, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-resource-observer: 05_000 runtime strip derived in={len} out={} known_vanilla={known} out_fnv=0x{:016x}",
                        out.len(),
                        er_gfx::title_05_000::fnv1a64(&out)
                    ));
                    TITLE_05_000_RUNTIME_STRIPPED.get_or_init(|| out)
                }
                Err(err) => {
                    return fail(format_args!("in={len} known_vanilla={known}: {err}"));
                }
            }
        }
    };
    unsafe {
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            stripped.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            stripped.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
    }
    TITLE_05_000_RUNTIME_STRIP_SERVES.fetch_add(1, Ordering::SeqCst);
    // Keep the established product-strip oracles counting regardless of mechanism (the
    // construct-from-embedded path incremented both of these).
    TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS.fetch_add(1, Ordering::SeqCst);
    TITLE_SCALEFORM_05_000_MEMORY_GFX_REPLACEMENTS.fetch_add(1, Ordering::SeqCst);
    TITLE_SCALEFORM_MEMORY_GFX_LAST_FILE.store(file, Ordering::SeqCst);
    true
}

fn profile_05_010_editor_hot_gfx() -> Result<Option<(usize, usize, u64)>, String> {
    let Some(editor_dir) = std::env::var_os("ER_PROFILE_05_010_EDITOR_DIR") else {
        return Ok(None);
    };
    let editor_dir = std::path::PathBuf::from(editor_dir);
    let Some(pi_local_dir) = editor_dir.parent() else {
        return Ok(None);
    };
    let path = pi_local_dir.join("profile-05-010-manual-layout.gfx");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if !(64..=0x0100_0000).contains(&bytes.len())
        || bytes.first() != Some(&b'G')
        || bytes.get(1) != Some(&b'F')
        || bytes.get(2) != Some(&b'X')
    {
        return Err(format!(
            "{} is not a plausible GFX payload (len={})",
            path.display(),
            bytes.len()
        ));
    }
    let fnv = er_gfx::title_05_000::fnv1a64(&bytes);
    let cache = PROFILE_05_010_EDITOR_GFX_CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let mut cache = cache
        .lock()
        .map_err(|_| "profile editor hot GFX cache poisoned".to_owned())?;
    if let Some(existing) = cache.iter().find(|existing| {
        existing.len() == bytes.len() && er_gfx::title_05_000::fnv1a64(existing) == fnv
    }) {
        return Ok(Some((existing.as_ptr() as usize, existing.len(), fnv)));
    }
    append_autoload_debug(format_args!(
        "stats-panel: 05_010 editor hot GFX cached {} len={} fnv=0x{fnv:016x}",
        path.display(),
        bytes.len()
    ));
    cache.push(bytes);
    let cached = cache
        .last()
        .expect("just-pushed 05_010 editor hot GFX cache entry");
    Ok(Some((cached.as_ptr() as usize, cached.len(), fnv)))
}

/// Stats-panel 05_010_ProfileSelect runtime edit (mirrors `title_05_000_swap_to_stripped`): derive
/// the stats-panel movie (face box removed, `ErStats` field added, left column reflowed -- see
/// `er_gfx::title_05_010`) from the native MemoryFile's own vanilla payload, cache it for the
/// process lifetime, and swap the native file's data/len/cursor onto the cached buffer. In editor
/// mode (`ER_PROFILE_05_010_EDITOR_DIR`), prefer the rebuilt `target/pi-local/profile-05-010-manual-layout.gfx`
/// file and cache each version for the process lifetime, so rebuild-only controls hot-reload on the
/// next ProfileSelect movie open without rebuilding/reloading the DLL. ANY failure leaves the native
/// file untouched and returns it as-is: fail-closed to the vanilla/ProfileSelect rows, never a crash,
/// never a half-edited movie.
pub(crate) unsafe fn profile_05_010_swap_to_edited(base: usize, file: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if file == 0 || file == null || file == HOOK_ORIGINAL_UNSET {
        return false;
    }
    let fail = |reason: core::fmt::Arguments<'_>| {
        PROFILE_05_010_RUNTIME_EDIT_FAILURES.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "stats-panel: 05_010 runtime edit FAIL-CLOSED (serving native vanilla): {reason}"
        ));
        false
    };
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + SCALEFORM_MEMORY_FILE_VTABLE_RVA {
        return fail(format_args!(
            "unexpected file vtable 0x{vtable:x} (want MemoryFile 0x{:x})",
            base + SCALEFORM_MEMORY_FILE_VTABLE_RVA
        ));
    }
    match profile_05_010_editor_hot_gfx() {
        Ok(Some((ptr, len, fnv))) => {
            unsafe {
                core::ptr::write(
                    (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
                    ptr,
                );
                core::ptr::write(
                    (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
                    len as u32,
                );
                core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
            }
            PROFILE_05_010_RUNTIME_EDIT_OUTPUT_LEN.store(len, Ordering::SeqCst);
            PROFILE_05_010_RUNTIME_EDIT_OUTPUT_VALIDATED.store(3, Ordering::SeqCst);
            PROFILE_05_010_RUNTIME_EDIT_SERVES.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "stats-panel: 05_010 editor hot GFX served len={len} fnv=0x{fnv:016x}"
            ));
            return true;
        }
        Ok(None) => {}
        Err(err) => {
            return fail(format_args!("editor hot GFX unavailable: {err}"));
        }
    }
    let edited = match PROFILE_05_010_RUNTIME_EDITED.get() {
        Some(cached) => cached,
        None => {
            let data =
                unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len =
                unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || data == null || !(64..=0x0100_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            let len = len as usize;
            let magic_ok = unsafe { safe_read_u8(data) } == Some(b'G')
                && unsafe { safe_read_u8(data + 1) } == Some(b'F')
                && unsafe { safe_read_u8(data + 2) } == Some(b'X')
                && unsafe { safe_read_u8(data + len - 1) }.is_some();
            if !magic_ok {
                return fail(format_args!(
                    "payload at 0x{data:x} len={len} is unreadable or not GFX-magic"
                ));
            }
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
            PROFILE_05_010_RUNTIME_EDIT_INPUT_LEN.store(len, Ordering::SeqCst);
            let known = er_gfx::title_05_010::is_known_vanilla(vanilla);
            PROFILE_05_010_RUNTIME_EDIT_INPUT_CLASS
                .store(if known { 1 } else { 2 }, Ordering::SeqCst);
            match er_gfx::title_05_010::stats_panel(vanilla) {
                Ok(out) => {
                    PROFILE_05_010_RUNTIME_EDIT_OUTPUT_LEN.store(out.len(), Ordering::SeqCst);
                    let validated = out.len() == er_gfx::title_05_010::EDITED_LEN
                        && er_gfx::title_05_000::fnv1a64(&out)
                            == er_gfx::title_05_010::EDITED_FNV1A64;
                    PROFILE_05_010_RUNTIME_EDIT_OUTPUT_VALIDATED
                        .store(if validated { 1 } else { 2 }, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "stats-panel: 05_010 runtime edit derived in={len} out={} known_vanilla={known} out_fnv=0x{:016x}",
                        out.len(),
                        er_gfx::title_05_000::fnv1a64(&out)
                    ));
                    PROFILE_05_010_RUNTIME_EDITED.get_or_init(|| out)
                }
                Err(err) => {
                    return fail(format_args!("in={len} known_vanilla={known}: {err}"));
                }
            }
        }
    };
    unsafe {
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            edited.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            edited.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
    }
    PROFILE_05_010_RUNTIME_EDIT_SERVES.fetch_add(1, Ordering::SeqCst);
    true
}

/// Inline `02_990_textinput` over the save picker's CurrentPath field. Gated by the active native
/// path-editor job at the file-open observer, so ordinary game text inputs retain vanilla geometry.
pub(crate) unsafe fn text_input_02_990_swap_to_inline(base: usize, file: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let fail = |reason: core::fmt::Arguments<'_>| {
        TEXT_INPUT_02_990_RUNTIME_FAILURES.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-path: 02_990 inline GFX edit FAIL-CLOSED: {reason}"
        ));
        false
    };
    if file == 0 || file == null || file == HOOK_ORIGINAL_UNSET {
        return fail(format_args!("invalid MemoryFile 0x{file:x}"));
    }
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + SCALEFORM_MEMORY_FILE_VTABLE_RVA {
        return fail(format_args!("unexpected MemoryFile vtable 0x{vtable:x}"));
    }
    let edited = match TEXT_INPUT_02_990_RUNTIME_EDITED.get() {
        Some(edited) => edited,
        None => {
            let data =
                unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len =
                unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || data == null || !(64..=0x0010_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len as usize) };
            match er_gfx::text_input_02_990::inline_current_path_editor(vanilla) {
                Ok(edited) => {
                    append_autoload_debug(format_args!(
                        "save-picker-path: derived inline 02_990 GFX in={} out={} fnv=0x{:016x}",
                        vanilla.len(),
                        edited.len(),
                        er_gfx::title_05_000::fnv1a64(&edited)
                    ));
                    TEXT_INPUT_02_990_RUNTIME_EDITED.get_or_init(|| edited)
                }
                Err(error) => return fail(format_args!("{error}")),
            }
        }
    };
    unsafe {
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            edited.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            edited.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
    }
    TEXT_INPUT_02_990_RUNTIME_SERVES.fetch_add(1, Ordering::SeqCst);
    true
}

/// Four-button `02_040_optionsetting` runtime edit for System->Quit. This mirrors the 05_000/05_010
/// MemoryFile swap path, but deliberately has no env/file-backed diagnostic input: the product must not
/// ship or depend on an external GFx. The derived movie is built from the game's own vanilla payload and
/// cached for process lifetime so the native MemoryFile's data pointer remains valid.
pub(crate) unsafe fn options_02_040_quit4_swap_to_edited(base: usize, file: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if file == 0 || file == null || file == HOOK_ORIGINAL_UNSET {
        return false;
    }
    let fail = |reason: core::fmt::Arguments<'_>| {
        OPTIONS_02_040_QUIT4_RUNTIME_FAILURES.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-gfx: 02_040 quit4 runtime edit FAIL-CLOSED (serving native vanilla): {reason}"
        ));
        false
    };
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + SCALEFORM_MEMORY_FILE_VTABLE_RVA {
        return fail(format_args!(
            "unexpected file vtable 0x{vtable:x} (want MemoryFile 0x{:x})",
            base + SCALEFORM_MEMORY_FILE_VTABLE_RVA
        ));
    }
    let edited = match OPTIONS_02_040_QUIT4_RUNTIME_EDITED.get() {
        Some(cached) => cached,
        None => {
            let data =
                unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len =
                unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || data == null || !(64..=0x0100_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            let len = len as usize;
            let magic_ok = unsafe { safe_read_u8(data) } == Some(b'G')
                && unsafe { safe_read_u8(data + 1) } == Some(b'F')
                && unsafe { safe_read_u8(data + 2) } == Some(b'X')
                && unsafe { safe_read_u8(data + len - 1) }.is_some();
            if !magic_ok {
                return fail(format_args!(
                    "payload at 0x{data:x} len={len} is unreadable or not GFX-magic"
                ));
            }
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
            let known = er_gfx::options_02_040::is_known_vanilla_win(vanilla);
            match er_gfx::options_02_040::quit4(vanilla) {
                Ok(out) => {
                    let out_fnv = er_gfx::title_05_000::fnv1a64(&out);
                    append_autoload_debug(format_args!(
                        "system-quit-gfx: 02_040 quit4 runtime edit derived in={len} out={} known_vanilla={known} out_fnv=0x{out_fnv:016x}",
                        out.len()
                    ));
                    OPTIONS_02_040_QUIT4_RUNTIME_EDITED.get_or_init(|| out)
                }
                Err(err) => {
                    return fail(format_args!("in={len} known_vanilla={known}: {err}"));
                }
            }
        }
    };
    unsafe {
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            edited.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            edited.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
    }
    OPTIONS_02_040_QUIT4_RUNTIME_SERVES.fetch_add(1, Ordering::SeqCst);
    true
}

pub(crate) unsafe extern "system" fn title_scaleform_file_open_observer_hook(
    loader: usize,
    url: usize,
    flags: u32,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let hit = TITLE_SCALEFORM_FILE_OPEN_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let caller_rva = trace_first_game_caller_rva();
    TITLE_SCALEFORM_FILE_OPEN_LAST_LOADER.store(loader, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_URL_PTR.store(url, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_FLAGS.store(flags as usize, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    let is_title_logo = unsafe { bounded_ascii_contains(url, b"05_001_title_logo") }
        || unsafe { bounded_ascii_contains(url, b"05_001_title") };
    let is_title_05_000 = unsafe { bounded_ascii_contains(url, b"05_000_title") };
    let is_profile_05_010 = unsafe { bounded_ascii_contains(url, b"05_010_profileselect") };
    let is_options_02_040 = unsafe { bounded_ascii_contains(url, b"02_040_optionsetting") };
    let is_path_editor_02_990 =
        unsafe { bounded_ascii_contains(url, b"02_990_textinput_patheditor") }
            || unsafe { bounded_ascii_contains(url, b"02_990_TextInput_PathEditor") };

    let base = game_module_base().unwrap_or(null);
    let mut memory_replacement = false;
    // Label only. Every synthetic/embedded MemoryFile substitution is gone: the title, ProfileSelect
    // and OptionSetting movies are all derived IN PLACE from the game's own vanilla payload below, so
    // the DLL never constructs a Scaleform MemoryFile of its own.
    let memory_label = if is_title_logo {
        "05_001_title_logo"
    } else if is_title_05_000 {
        "05_000_title"
    } else if is_profile_05_010 {
        "05_010_profileselect"
    } else if is_options_02_040 {
        "02_040_optionsetting"
    } else if is_path_editor_02_990 {
        "02_990_textinput_patheditor"
    } else {
        ""
    };
    let orig = TITLE_SCALEFORM_FILE_OPEN_ORIG.load(Ordering::SeqCst);
    let ret = if base != null {
        if orig != null && orig != HOOK_ORIGINAL_UNSET {
            let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
                unsafe { std::mem::transmute(orig) };
            // A custom cache key forces a fresh Scaleform load. Redirect only that key's file-open
            // to the canonical native movie; the game's shared 02_990 cache entry stays untouched.
            let open_url = if is_path_editor_02_990 {
                TEXT_INPUT_02_990_CANONICAL_URL.as_ptr() as usize
            } else {
                url
            };
            let native = unsafe { f(loader, open_url, flags) };
            // Product-default runtime strip (er-effects-rs-h7x): derive the stripped title
            // movie from the native file's own vanilla payload and swap it in place. On any
            // failure the untouched native file is returned (vanilla title UI, fail-closed).
            if is_title_05_000 && TITLE_05_000_RUNTIME_STRIP_ARMED.load(Ordering::SeqCst) != 0 {
                memory_replacement = unsafe { title_05_000_swap_to_stripped(base, native) };
            }
            // Stats-panel 05_010 edit: same in-place derive-and-swap, same fail-closed shape.
            if is_profile_05_010 && PROFILE_05_010_RUNTIME_EDIT_ARMED.load(Ordering::SeqCst) != 0 {
                memory_replacement = unsafe { profile_05_010_swap_to_edited(base, native) };
            }
            // System->Quit four-button GFx edit: product-default, no external asset dependency.
            if is_options_02_040 {
                memory_replacement = unsafe { options_02_040_quit4_swap_to_edited(base, native) };
            }
            if is_path_editor_02_990 {
                memory_replacement = unsafe { text_input_02_990_swap_to_inline(base, native) };
            }
            native
        } else {
            null
        }
    } else if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(loader, url, flags) }
    } else {
        null
    };
    let ret_vtable = if ret != null && ret != HOOK_ORIGINAL_UNSET {
        unsafe { safe_read_usize(ret) }.unwrap_or(null)
    } else {
        null
    };
    TITLE_SCALEFORM_FILE_OPEN_LAST_RET.store(ret, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_RET_VTABLE.store(ret_vtable, Ordering::SeqCst);

    // Capture the game's menu font (font:/<locale>/font.gfx) for our loading-screen stats text (read-only
    // copy of the file's own GFX payload; er-effects-rs-jsm). Observe-only, one-shot.
    if base != null
        && (unsafe { bounded_ascii_contains(url, b"font.gfx") }
            || unsafe { bounded_ascii_contains(url, b"font.swf") })
    {
        unsafe { capture_menu_font_gfx(base, ret) };
    }

    if is_title_logo
        || is_title_05_000
        || is_profile_05_010
        || is_options_02_040
        || is_path_editor_02_990
    {
        let logo_hit = if is_title_logo {
            TITLE_SCALEFORM_FILE_OPEN_LOGO_HITS.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            0
        };
        let mut name = [0u8; 128];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform file-open title-memory label={memory_label} logo_hit={logo_hit} total={hit} loader=0x{loader:x} url=0x{url:x} '{name}' redirected_to_canonical_02_990={is_path_editor_02_990} flags=0x{flags:x} ret=0x{ret:x} ret_vtable=0x{ret_vtable:x} caller_rva=0x{caller_rva:x} memory_replacement={memory_replacement}"
        ));
    } else if hit <= 24 {
        let mut name = [0u8; 96];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform file-open sample total={hit} url='{name}' flags=0x{flags:x} ret=0x{ret:x} ret_vtable=0x{ret_vtable:x} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}

pub(crate) unsafe extern "system" fn title_scaleform_resource_ctor_observer_hook(
    out_resource: usize,
    loader_data: usize,
    file_type: u32,
    url: usize,
    file_obj: usize,
    external_flag: u8,
    heap_arg: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let hit = TITLE_SCALEFORM_RESOURCE_CTOR_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let caller_rva = trace_first_game_caller_rva();
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_OUT.store(out_resource, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_URL_PTR.store(url, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_FILE.store(file_obj, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    let is_title_logo = unsafe { bounded_ascii_contains(url, b"05_001_title_logo") }
        || unsafe { bounded_ascii_contains(url, b"05_001_title") };

    let orig = TITLE_SCALEFORM_RESOURCE_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u32, usize, usize, u8, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe {
            f(
                out_resource,
                loader_data,
                file_type,
                url,
                file_obj,
                external_flag,
                heap_arg,
            )
        }
    } else {
        null
    };
    let movie_data = if ret != null && ret != HOOK_ORIGINAL_UNSET {
        unsafe { safe_read_usize(ret + 0x40) }.unwrap_or(null)
    } else {
        null
    };
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_RET.store(ret, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_MOVIE_DATA.store(movie_data, Ordering::SeqCst);

    if is_title_logo {
        let logo_hit = TITLE_SCALEFORM_RESOURCE_CTOR_LOGO_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let mut name = [0u8; 128];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform resource-ctor title-logo hit={logo_hit} total={hit} out=0x{out_resource:x} url=0x{url:x} '{name}' file=0x{file_obj:x} file_type={file_type} external_flag={external_flag} ret=0x{ret:x} movie_data=0x{movie_data:x} caller_rva=0x{caller_rva:x}; observe-only"
        ));
    } else if hit <= 24 {
        let mut name = [0u8; 96];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform resource-ctor sample total={hit} url='{name}' file=0x{file_obj:x} file_type={file_type} ret=0x{ret:x} movie_data=0x{movie_data:x} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}
