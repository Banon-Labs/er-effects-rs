//! Bottom-up census of every call site that touches save data on disk.
//!
//! The top-down reverse engineering of ELDEN RING's save machinery answers "where
//! does the game *decide* to save". This module answers the complementary and much
//! harder-to-fake question: "where do save bytes *actually* reach the filesystem".
//! It hooks the Win32 file APIs, filters to save-relevant paths, and records the
//! captured stack of each write as game-module RVAs.
//!
//! Why that matters for the objective: a save-disable feature is only as good as
//! its coverage, and coverage cannot be established by reading disassembly alone --
//! a missed trigger looks exactly like a trigger that does not exist. Once the
//! game-side interception lands, any non-zero count here is a save path we failed
//! to enumerate, which makes [`escaped_write_sites`] the completeness oracle rather
//! than an assumption.
//!
//! Everything here is observation-only. This module never suppresses a write.

use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use er_game_base::mem::module_text_range;

/// Longest wide path we will read out of a caller's buffer. Save paths are far
/// shorter; the bound exists so a malformed/non-terminated pointer cannot walk us
/// off the end of a page.
const MAX_PATH_UNITS: usize = 1024;
/// Stack frames captured per save write. Deep enough to walk out of the CRT/file
/// device wrappers and reach the FromSoft code that requested the write.
const CAPTURED_FRAMES: usize = 16;
/// Distinct call-site stacks retained. The census is expected to converge on a
/// handful; the cap bounds memory if a path turns out to be far noisier.
const MAX_RECORDED_SITES: usize = 64;
/// Open save handles tracked at once (the game keeps very few).
const MAX_TRACKED_HANDLES: usize = 32;

/// Path fragments that mark a file as save data. Matched case-insensitively against
/// the full path. `.sl2`/`.co2` cover the character containers and their `.bak`
/// companions (which are `ER0000.sl2.bak`, so the fragment still matches).
const SAVE_PATH_FRAGMENTS: [&str; 3] = [".sl2", ".co2", "er0000"];

static CREATE_SAVE_OPENS: AtomicU64 = AtomicU64::new(0);
static CREATE_SAVE_WRITE_OPENS: AtomicU64 = AtomicU64::new(0);
static WRITE_CALLS: AtomicU64 = AtomicU64::new(0);
static WRITE_BYTES: AtomicU64 = AtomicU64::new(0);
static SET_END_OF_FILE_CALLS: AtomicU64 = AtomicU64::new(0);
static MOVE_CALLS: AtomicU64 = AtomicU64::new(0);
static COPY_CALLS: AtomicU64 = AtomicU64::new(0);
/// Bytes moved by copies. Counted separately from `WRITE_BYTES` but summed into the
/// reported total, so a byte-for-byte reconciliation against an offline save diff sees
/// copies as the writes they effectively are.
static COPY_BYTES: AtomicU64 = AtomicU64::new(0);
static DELETE_CALLS: AtomicU64 = AtomicU64::new(0);
static SITES_DROPPED: AtomicU64 = AtomicU64::new(0);
static HANDLES_DROPPED: AtomicU64 = AtomicU64::new(0);

// Reentrancy guard. Our own logging and telemetry writes go through the very APIs
// we hook; without this, recording a save write could recurse into the recorder.
//
// A plain comment, not a doc comment: `///` on a macro invocation attaches to nothing
// and rustc reports it as an unused doc comment, so the explanation was invisible in
// both the docs and (under this repo's `-Awarnings`) the build output.
thread_local! {
    static IN_WITNESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Report that the census has gone blind.
///
/// Exists so the blindness call sites read the same on both targets. Inlining
/// `#[cfg(windows)] if ...` at each site left the `dropped` binding unused on host
/// builds -- a warning nobody saw, because this repo builds with `-Awarnings`.
/// On host there is no game directory to log into, so the message is dropped; the
/// counter still moves and `check-save-suppression.py` still gates on it.
fn log_blind(args: std::fmt::Arguments<'_>) {
    #[cfg(windows)]
    crate::log_message(args);
    #[cfg(not(windows))]
    let _ = args;
}

/// One distinct call site that reached save data on disk.
#[derive(Clone)]
pub(crate) struct WriteSite {
    /// Which API the site was observed through (`WriteFile`, `MoveFileExW`, ...).
    pub(crate) api: &'static str,
    /// Captured return addresses that fall inside the game module, as RVAs. These
    /// are directly comparable to the addresses produced by static RE, so a census
    /// entry can be matched against the reverse-engineered save call graph.
    pub(crate) game_rvas: Vec<usize>,
    /// Frames outside the game's `.text` (CRT, kernel32/ntdll, Wine internals),
    /// kept as ABSOLUTE addresses. Retaining them rather than just counting them is
    /// what makes an attribution failure diagnosable: if `game_rvas` is empty, these
    /// say whether the writer was genuinely another module or whether the range test
    /// was wrong.
    pub(crate) foreign_frames: Vec<usize>,
    pub(crate) hits: u64,
    pub(crate) bytes: u64,
    /// The save path this site was first seen writing.
    pub(crate) path: String,
}

struct Census {
    sites: Vec<WriteSite>,
    handles: Vec<(usize, String)>,
}

fn census() -> &'static Mutex<Census> {
    static CENSUS: OnceLock<Mutex<Census>> = OnceLock::new();
    CENSUS.get_or_init(|| {
        Mutex::new(Census {
            sites: Vec::new(),
            handles: Vec::new(),
        })
    })
}

static GAME_TEXT_RANGE: OnceLock<Option<(usize, usize)>> = OnceLock::new();
static GAME_BASE: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn set_game_base(base: usize) {
    GAME_BASE.store(base, Ordering::SeqCst);
}

fn game_text_range() -> Option<(usize, usize)> {
    *GAME_TEXT_RANGE.get_or_init(module_text_range)
}

/// `(game_base, text_start, text_size)` as the census resolved them.
///
/// Published in telemetry so an empty `game_rvas` can be diagnosed offline: a zero
/// base or absent text range means attribution was broken, which is a very different
/// finding from "the writer really was another module".
pub(crate) fn attribution_context() -> (usize, usize, usize) {
    let (start, size) = game_text_range().unwrap_or((0, 0));
    (GAME_BASE.load(Ordering::SeqCst), start, size)
}

/// Read a NUL-terminated UTF-16 path, bounded, and lowercase it for matching.
///
/// # Safety
/// `ptr` must be a valid wide string pointer or null.
pub(crate) unsafe fn read_wide_path(ptr: *const u16) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let mut units = Vec::with_capacity(64);
    for index in 0..MAX_PATH_UNITS {
        // Bounded, and every read is of a unit we have already proven the previous
        // unit was non-NUL for; a caller handing us a non-terminated buffer costs us
        // MAX_PATH_UNITS reads rather than an unbounded walk.
        let unit = unsafe { ptr.add(index).read() };
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    // Lossy is deliberate here: the string is only ever used for substring
    // classification and for diagnostics, never to reopen the file. A lone surrogate
    // in a path must not cause the write to vanish from the census.
    Some(String::from_utf16_lossy(&units))
}

pub(crate) fn is_save_path(path: &str) -> bool {
    let mut buffer = [0_u8; MAX_PATH_UNITS];
    let len = ascii_fold_into(path.chars().map(|c| c as u32), &mut buffer);
    buffer_is_save_path(&buffer[..len])
}

/// ASCII-lowercase `chars` into `buffer`, returning the used length. Non-ASCII
/// units become `?`: only the ASCII fragments in [`SAVE_PATH_FRAGMENTS`] are ever
/// matched, so a non-ASCII directory component cannot change the verdict.
fn ascii_fold_into(chars: impl Iterator<Item = u32>, buffer: &mut [u8; MAX_PATH_UNITS]) -> usize {
    let mut len = 0;
    for unit in chars {
        if len == buffer.len() {
            break;
        }
        buffer[len] = if unit < 128 {
            (unit as u8).to_ascii_lowercase()
        } else {
            b'?'
        };
        len += 1;
    }
    len
}

fn buffer_is_save_path(folded: &[u8]) -> bool {
    SAVE_PATH_FRAGMENTS.iter().any(|fragment| {
        folded
            .windows(fragment.len())
            .any(|window| window == fragment.as_bytes())
    })
}

/// Classify a wide path without allocating.
///
/// `CreateFileW` is called constantly while the game streams assets, so the common
/// case (not a save path) must not heap-allocate. Only a match pays for a `String`.
///
/// # Safety
/// `ptr` must be a valid wide string pointer or null.
unsafe fn wide_is_save_path(ptr: *const u16) -> bool {
    if ptr.is_null() {
        return false;
    }
    let mut buffer = [0_u8; MAX_PATH_UNITS];
    let mut len = 0;
    for index in 0..MAX_PATH_UNITS {
        let unit = unsafe { ptr.add(index).read() };
        if unit == 0 {
            break;
        }
        buffer[len] = if unit < 128 {
            (unit as u8).to_ascii_lowercase()
        } else {
            b'?'
        };
        len += 1;
    }
    buffer_is_save_path(&buffer[..len])
}

/// Split captured return addresses into game-module RVAs and everything else.
///
/// Pure and unit-tested on purpose. The first census run silently produced empty
/// RVA lists because this classification treated `module_text_range`'s second tuple
/// element as an END address when it is a SIZE -- every frame then failed the range
/// test and was written off as foreign, which looks identical to "the writer is not
/// game code". Attribution failing silently is the worst outcome for a census, so
/// the arithmetic now lives somewhere a test can pin it.
fn classify_frames(
    frames: &[usize],
    game_base: usize,
    text: Option<(usize, usize)>,
) -> (Vec<usize>, Vec<usize>) {
    let mut rvas = Vec::new();
    let mut foreign = Vec::new();
    for &address in frames {
        let in_text = text
            .is_some_and(|(start, size)| address >= start && address < start.saturating_add(size));
        if in_text && game_base != 0 && address >= game_base {
            rvas.push(address - game_base);
        } else {
            foreign.push(address);
        }
    }
    (rvas, foreign)
}

/// Capture the current call stack.
///
/// Host builds have no game and no `RtlCaptureStackBackTrace`; they return an empty
/// stack so the classification and census bookkeeping stay unit-testable off Windows.
/// Only the capture itself is platform-specific.
#[cfg(not(windows))]
fn capture_site_rvas() -> (Vec<usize>, Vec<usize>) {
    (Vec::new(), Vec::new())
}

#[cfg(windows)]
fn capture_site_rvas() -> (Vec<usize>, Vec<usize>) {
    let mut frames = [core::ptr::null_mut::<core::ffi::c_void>(); CAPTURED_FRAMES];
    // Skip 2: this helper and the detour that called it.
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            2,
            CAPTURED_FRAMES as u32,
            frames.as_mut_ptr(),
            core::ptr::null_mut(),
        )
    };
    let addresses: Vec<usize> = frames
        .iter()
        .take(captured as usize)
        .map(|frame| *frame as usize)
        .collect();
    classify_frames(
        &addresses,
        GAME_BASE.load(Ordering::SeqCst),
        game_text_range(),
    )
}

/// Record one save-data touch. Deduplicates by captured call stack so repeated
/// writes from the same code path collapse into a single census entry with a count.
fn record(api: &'static str, path: &str, bytes: u64) {
    let (game_rvas, foreign_frames) = capture_site_rvas();
    let Ok(mut census) = census().lock() else {
        return;
    };
    if let Some(existing) = census
        .sites
        .iter_mut()
        .find(|site| site.api == api && site.game_rvas == game_rvas)
    {
        existing.hits = existing.hits.saturating_add(1);
        existing.bytes = existing.bytes.saturating_add(bytes);
        return;
    }
    if census.sites.len() >= MAX_RECORDED_SITES {
        // The escape oracle just went blind: from here on, an escaped write can happen
        // and `escaped_write_sites` will not grow. Unthrottled and unconditional --
        // a detector that stops detecting must never do so quietly.
        let dropped = SITES_DROPPED.fetch_add(1, Ordering::SeqCst) + 1;
        if dropped == 1 {
            log_blind(format_args!(
                "census: BLIND -- site table full at {MAX_RECORDED_SITES}; escaped writes \
                 from here on will NOT be recorded (sites_dropped={dropped})"
            ));
        }
        return;
    }
    census.sites.push(WriteSite {
        api,
        game_rvas,
        foreign_frames,
        hits: 1,
        bytes,
        path: path.to_owned(),
    });
    drop(census);
    // A newly discovered call site is the only interesting event, so publish here
    // rather than on a timer. Callers are already inside the reentrancy guard, so
    // the telemetry write's own file I/O cannot recurse back into the census.
    // Host builds skip it: there is no game directory to publish into and a unit
    // test must not drop a telemetry file in the working tree -- which is also why
    // the log line below is gated, since `record` is called directly from host tests.
    #[cfg(windows)]
    {
        // The loudest thing this DLL can detect: a write reached save data through a
        // path suppression did not cover. Bounded by `MAX_RECORDED_SITES` and deduped
        // by (api, game_rvas), so it cannot stream.
        crate::log_message(format_args!(
            "census: ESCAPED WRITE -- {api} reached {path} ({bytes} bytes); \
             suppression did not cover this path"
        ));
        crate::telemetry::write_snapshot();
    }
}

/// Mirror of `census.handles.len()`, readable without taking the lock.
///
/// `WriteFile` and `CloseHandle` run on every asset stream and every temp file the
/// game touches. Taking a mutex on each would put this DLL in the hot path of all
/// file I/O; this lets the overwhelmingly common "no save file is open" case cost a
/// single relaxed atomic load.
static TRACKED_HANDLE_COUNT: AtomicUsize = AtomicUsize::new(0);

fn any_tracked_handles() -> bool {
    TRACKED_HANDLE_COUNT.load(Ordering::Relaxed) != 0
}

fn track_handle(handle: usize, path: &str) {
    let Ok(mut census) = census().lock() else {
        return;
    };
    if census.handles.len() >= MAX_TRACKED_HANDLES {
        // An untracked save handle makes its writes invisible to BOTH
        // `escaped_write_sites` and `total_bytes_to_disk` -- the census under-reports
        // and reads clean. Same rule as a full site table: say so once, loudly.
        let dropped = HANDLES_DROPPED.fetch_add(1, Ordering::SeqCst) + 1;
        if dropped == 1 {
            log_blind(format_args!(
                "census: BLIND -- handle table full at {MAX_TRACKED_HANDLES}; writes \
                 through untracked save handles will NOT be counted \
                 (handles_dropped={dropped})"
            ));
        }
        return;
    }
    census.handles.push((handle, path.to_owned()));
    TRACKED_HANDLE_COUNT.store(census.handles.len(), Ordering::Relaxed);
}

fn handle_path(handle: usize) -> Option<String> {
    let census = census().lock().ok()?;
    census
        .handles
        .iter()
        .find(|(tracked, _)| *tracked == handle)
        .map(|(_, path)| path.clone())
}

fn forget_handle(handle: usize) {
    if let Ok(mut census) = census().lock() {
        census.handles.retain(|(tracked, _)| *tracked != handle);
        TRACKED_HANDLE_COUNT.store(census.handles.len(), Ordering::Relaxed);
    }
}

/// Run `body` under the census reentrancy guard, for callers outside the observation
/// paths.
///
/// The suppression detours are not observers, so they reach the telemetry writer with
/// the guard clear. Routing them through here means the writer's own file I/O
/// short-circuits the detours' record paths for every caller, instead of relying on the
/// save-path filter to make it merely harmless.
///
/// Returns `None` without running `body` if this thread is already inside the guard --
/// callers that are already observers (see `record`) must NOT use this, or their
/// snapshot would be silently skipped.
#[cfg(windows)]
pub(crate) fn with_guard<T>(body: impl FnOnce() -> T) -> Option<T> {
    guarded(body)
}

/// Run `body` unless we are already inside the witness on this thread.
fn guarded<T>(body: impl FnOnce() -> T) -> Option<T> {
    IN_WITNESS.with(|flag| {
        if flag.get() {
            return None;
        }
        flag.set(true);
        let result = body();
        flag.set(false);
        Some(result)
    })
}

// ---------------------------------------------------------------------------
// Observation entry points, called from the API detours in `hooks`.
// ---------------------------------------------------------------------------

/// # Safety
/// `path` must be a valid wide string pointer or null.
pub(crate) unsafe fn note_create_file(path: *const u16, desired_access: u32, handle: usize) {
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_WRITE_DATA: u32 = 0x0000_0002;
    const FILE_APPEND_DATA: u32 = 0x0000_0004;
    const INVALID_HANDLE_VALUE: usize = usize::MAX;

    // Cheap rejection first: no allocation and no lock unless the path is save data.
    if !unsafe { wide_is_save_path(path) } {
        return;
    }
    guarded(|| {
        let Some(path) = (unsafe { read_wide_path(path) }) else {
            return;
        };
        CREATE_SAVE_OPENS.fetch_add(1, Ordering::SeqCst);
        let writable = desired_access & (GENERIC_WRITE | FILE_WRITE_DATA | FILE_APPEND_DATA) != 0;
        if !writable || handle == INVALID_HANDLE_VALUE || handle == 0 {
            return;
        }
        CREATE_SAVE_WRITE_OPENS.fetch_add(1, Ordering::SeqCst);
        track_handle(handle, &path);
    });
}

pub(crate) fn note_write_file(handle: usize, bytes: u64) {
    if !any_tracked_handles() {
        return;
    }
    guarded(|| {
        let Some(path) = handle_path(handle) else {
            return;
        };
        WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
        WRITE_BYTES.fetch_add(bytes, Ordering::SeqCst);
        record("WriteFile", &path, bytes);
    });
}

pub(crate) fn note_set_end_of_file(handle: usize) {
    if !any_tracked_handles() {
        return;
    }
    guarded(|| {
        let Some(path) = handle_path(handle) else {
            return;
        };
        SET_END_OF_FILE_CALLS.fetch_add(1, Ordering::SeqCst);
        record("SetEndOfFile", &path, 0);
    });
}

pub(crate) fn note_close_handle(handle: usize) {
    if !any_tracked_handles() {
        return;
    }
    guarded(|| forget_handle(handle));
}

/// # Safety
/// Both pointers must be valid wide string pointers or null.
pub(crate) unsafe fn note_move_file(existing: *const u16, new: *const u16) {
    guarded(|| {
        let from = unsafe { read_wide_path(existing) }.unwrap_or_default();
        let to = unsafe { read_wide_path(new) }.unwrap_or_default();
        if !is_save_path(&from) && !is_save_path(&to) {
            return;
        }
        MOVE_CALLS.fetch_add(1, Ordering::SeqCst);
        record("MoveFileExW", &format!("{from} -> {to}"), 0);
    });
}

/// A copy whose source or destination is save data.
///
/// This is how the game builds `ER0000.sl2.bak`, and it is the route that escaped the
/// original hook set: bytes reach disk without a single `WriteFile`. `bytes` is charged
/// from the source file's size so the census byte total can be reconciled against an
/// offline diff -- an escape that shows up as zero bytes would be invisible to that check.
///
/// # Safety
/// Both pointers must be valid wide string pointers or null.
pub(crate) unsafe fn note_copy_file(existing: *const u16, new: *const u16) {
    guarded(|| {
        let from = unsafe { read_wide_path(existing) }.unwrap_or_default();
        let to = unsafe { read_wide_path(new) }.unwrap_or_default();
        if !is_save_path(&from) && !is_save_path(&to) {
            return;
        }
        COPY_CALLS.fetch_add(1, Ordering::SeqCst);
        let bytes = std::fs::metadata(&from).map(|m| m.len()).unwrap_or(0);
        COPY_BYTES.fetch_add(bytes, Ordering::SeqCst);
        record("CopyFileW", &format!("{from} -> {to}"), bytes);
    });
}

/// # Safety
/// Both pointers must be valid wide string pointers or null.
pub(crate) unsafe fn note_move_file_plain(existing: *const u16, new: *const u16) {
    guarded(|| {
        let from = unsafe { read_wide_path(existing) }.unwrap_or_default();
        let to = unsafe { read_wide_path(new) }.unwrap_or_default();
        if !is_save_path(&from) && !is_save_path(&to) {
            return;
        }
        MOVE_CALLS.fetch_add(1, Ordering::SeqCst);
        record("MoveFileW", &format!("{from} -> {to}"), 0);
    });
}

/// # Safety
/// `path` must be a valid wide string pointer or null.
pub(crate) unsafe fn note_delete_file(path: *const u16) {
    guarded(|| {
        let Some(path) = (unsafe { read_wide_path(path) }) else {
            return;
        };
        if !is_save_path(&path) {
            return;
        }
        DELETE_CALLS.fetch_add(1, Ordering::SeqCst);
        record("DeleteFileW", &path, 0);
    });
}

// ---------------------------------------------------------------------------
// Readouts for telemetry.
// ---------------------------------------------------------------------------

pub(crate) fn snapshot_sites() -> Vec<WriteSite> {
    census()
        .lock()
        .map(|census| census.sites.clone())
        .unwrap_or_default()
}

/// The completeness oracle. Every distinct call site that reached save data on
/// disk. Once game-side interception is installed this must be empty; a non-empty
/// list names exactly the save path that was missed, with RVAs to resolve it.
pub(crate) fn escaped_write_sites() -> Vec<WriteSite> {
    snapshot_sites()
        .into_iter()
        .filter(|site| site.bytes > 0 || site.api != "WriteFile")
        .collect()
}

pub(crate) fn counters() -> [(&'static str, u64); 12] {
    [
        (
            "create_save_opens",
            CREATE_SAVE_OPENS.load(Ordering::SeqCst),
        ),
        ("copy_calls", COPY_CALLS.load(Ordering::SeqCst)),
        ("copy_bytes", COPY_BYTES.load(Ordering::SeqCst)),
        // The figure to reconcile against an offline byte diff: everything that reached
        // disk, however it got there. Reporting write_bytes alone under-counted a real run
        // by 3014688 bytes because the backup was produced by CopyFileW.
        (
            "total_bytes_to_disk",
            WRITE_BYTES.load(Ordering::SeqCst) + COPY_BYTES.load(Ordering::SeqCst),
        ),
        (
            "create_save_write_opens",
            CREATE_SAVE_WRITE_OPENS.load(Ordering::SeqCst),
        ),
        ("write_calls", WRITE_CALLS.load(Ordering::SeqCst)),
        ("write_bytes", WRITE_BYTES.load(Ordering::SeqCst)),
        (
            "set_end_of_file_calls",
            SET_END_OF_FILE_CALLS.load(Ordering::SeqCst),
        ),
        ("move_calls", MOVE_CALLS.load(Ordering::SeqCst)),
        ("delete_calls", DELETE_CALLS.load(Ordering::SeqCst)),
        ("sites_dropped", SITES_DROPPED.load(Ordering::SeqCst)),
        ("handles_dropped", HANDLES_DROPPED.load(Ordering::SeqCst)),
    ]
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        back_trace: *mut *mut core::ffi::c_void,
        back_trace_hash: *mut u32,
    ) -> u16;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_save_containers_and_their_companions() {
        assert!(is_save_path(
            r"C:\users\steamuser\AppData\Roaming\EldenRing\7656\ER0000.sl2"
        ));
        assert!(is_save_path(r"...\ER0000.sl2.bak"));
        assert!(is_save_path(r"...\ER0000.co2"));
        // Case must not matter: the game builds these paths itself and casing is
        // not something we get to rely on.
        assert!(is_save_path(r"...\er0000.SL2"));
    }

    #[test]
    fn ignores_unrelated_game_files() {
        assert!(!is_save_path(r"C:\ELDEN RING\Game\regulation.bin"));
        assert!(!is_save_path(r"C:\ELDEN RING\Game\GraphicsConfig.xml"));
        // This DLL's own outputs must never be mistaken for save data, or the
        // census would observe itself.
        assert!(!is_save_path(r"C:\ELDEN RING\Game\er-save-disable.log"));
        assert!(!is_save_path(
            r"C:\ELDEN RING\Game\er-save-disable-telemetry.json"
        ));
    }

    /// The census is process-global, and host builds capture no stack frames, so
    /// tests must key on a unique API label rather than on site counts -- otherwise
    /// they would race each other under the parallel test harness.
    fn site_for(api: &str) -> Option<WriteSite> {
        snapshot_sites().into_iter().find(|site| site.api == api)
    }

    #[test]
    fn repeated_writes_from_one_call_site_collapse_into_one_entry() {
        // The census enumerates distinct call SITES. A save writes its ~2.5MB
        // container in many chunks from the same code path; if those did not
        // collapse, the site list would overflow its cap and lose real sites.
        record("test:ChunkedWrite", r"...\ER0000.sl2", 1024);
        record("test:ChunkedWrite", r"...\ER0000.sl2", 2048);
        let site = site_for("test:ChunkedWrite").expect("recorded site");
        assert_eq!(site.hits, 2, "identical call stacks must merge");
        assert_eq!(site.bytes, 3072, "byte counts accumulate across hits");
    }

    #[test]
    fn distinct_apis_are_recorded_separately() {
        // A rename-swap save leaves no WriteFile on the final path, so collapsing
        // APIs together would hide the mechanism that actually moved the bytes.
        record("test:Rename", r"...\ER0000.sl2.tmp -> ...\ER0000.sl2", 0);
        record("test:Unlink", r"...\ER0000.sl2.bak", 0);
        assert!(site_for("test:Rename").is_some());
        assert!(site_for("test:Unlink").is_some());
    }

    // ELDEN RING's real layout: image base 0x140000000, .text starting one page in
    // and spanning tens of megabytes. `module_text_range` reports (start, SIZE).
    const GAME_BASE: usize = 0x1_4000_0000;
    const TEXT_START: usize = 0x1_4000_1000;
    const TEXT_SIZE: usize = 0x0300_0000;

    #[test]
    fn game_frames_become_rvas_and_foreign_frames_are_kept() {
        // The regression: reading the tuple's second element as an END address makes
        // `address < 0x3000000` false for every real frame, so the whole stack is
        // written off as foreign and the census reports "no game frames captured"
        // for writes that came straight out of game code.
        let in_game = TEXT_START + 0x0067_a050;
        let ntdll = 0x7fff_1234_5678;
        let (rvas, foreign) =
            classify_frames(&[in_game, ntdll], GAME_BASE, Some((TEXT_START, TEXT_SIZE)));
        assert_eq!(
            rvas,
            vec![in_game - GAME_BASE],
            "a frame inside .text must be attributed to the game"
        );
        assert_eq!(
            foreign,
            vec![ntdll],
            "foreign frames are retained, not counted"
        );
    }

    #[test]
    fn addresses_past_the_end_of_text_are_not_attributed_to_the_game() {
        let past_end = TEXT_START + TEXT_SIZE;
        let (rvas, foreign) =
            classify_frames(&[past_end], GAME_BASE, Some((TEXT_START, TEXT_SIZE)));
        assert!(
            rvas.is_empty(),
            "the range is half-open; the end address is outside"
        );
        assert_eq!(foreign, vec![past_end]);
    }

    #[test]
    fn missing_text_range_attributes_nothing_rather_than_guessing() {
        let address = TEXT_START + 0x1000;
        let (rvas, foreign) = classify_frames(&[address], GAME_BASE, None);
        assert!(rvas.is_empty());
        assert_eq!(foreign, vec![address]);
    }

    #[test]
    fn zero_byte_non_write_apis_still_count_as_escaped() {
        // A save that only renames a pre-staged temp file moves no bytes through
        // WriteFile. If the escape oracle filtered on bytes alone it would call
        // that run clean, which is the exact failure this guards against.
        record("test:EscapeRename", r"...\ER0000.sl2", 0);
        assert!(
            escaped_write_sites()
                .iter()
                .any(|site| site.api == "test:EscapeRename"),
            "a zero-byte rename must still register as an escaped save write"
        );
    }
}
