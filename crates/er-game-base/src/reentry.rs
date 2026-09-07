//! One thread-local re-entrancy latch, for the code paths that can be called from inside
//! themselves.
//!
//! # Why this keeps being needed
//!
//! Three separate places in this workspace have now been killed by the same shape, and each one
//! grew its own copy of the same latch before this module existed:
//!
//! | where | the loop |
//! |---|---|
//! | `save_policy_logs::append_autoload_debug` | the logger's own file open re-enters the `CreateFileW` detour, which logs |
//! | `er_save_redirect::reentry` | a detour's own `fs::read` re-enters the detour |
//! | `crashlog::veh_exit_hooks::crash_vectored_handler` | describing a fault faults, and a VEH is re-entered for its own faults |
//!
//! All three are same-thread synchronous recursion, so the latch is thread-local: a process-wide
//! flag would wrongly mute a legitimate concurrent call on another thread, which in the logger's
//! case silently drops most of the log.
//!
//! None of them are bounded by a call budget. A budget is checked once per entry and each entry
//! costs a whole stack frame, so the stack always runs out first -- measured on the VEH at 4704
//! bytes a level against a 1 MiB stack, dead after ~220 of a permitted 256.
//!
//! # The "unanswerable means nested" rule
//!
//! [`ReentryLatch::enter`] returns `None` when the thread-local cannot be reached at all -- during
//! thread teardown, or a fault early enough that TLS is not up. That counts as nested on purpose:
//! refusing one diagnostic costs a line, and guessing the other way costs the process.

use core::cell::Cell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// A thread-local "am I already inside?" latch plus a count of what it refused.
///
/// Declare one per guarded path in a `thread_local!` and pair it with a `static` counter:
///
/// ```
/// use er_game_base::reentry::ReentryLatch;
/// use core::sync::atomic::{AtomicUsize, Ordering};
///
/// static REFUSALS: AtomicUsize = AtomicUsize::new(0);
/// thread_local! {
///     static LATCH: ReentryLatch = const { ReentryLatch::new() };
/// }
///
/// fn guarded(depth: &core::cell::Cell<usize>) {
///     let Some(_token) = ReentryLatch::enter(&LATCH, &REFUSALS) else {
///         return;
///     };
///     depth.set(depth.get() + 1);
///     guarded(depth); // the re-entrant call the latch exists to refuse
/// }
///
/// let depth = core::cell::Cell::new(0);
/// guarded(&depth);
/// assert_eq!(depth.get(), 1);
/// assert_eq!(REFUSALS.load(Ordering::SeqCst), 1);
/// ```
pub struct ReentryLatch {
    inside: Cell<bool>,
}

impl ReentryLatch {
    /// A latch nobody is inside yet.
    pub const fn new() -> Self {
        Self {
            inside: Cell::new(false),
        }
    }

    /// `Some(token)` only for the OUTERMOST entry on this thread; `None` for a nested one, which
    /// also bumps `refusals`. The flag clears when the token is dropped, including on an early
    /// return or an unwind.
    pub fn enter(
        latch: &'static std::thread::LocalKey<Self>,
        refusals: &'static AtomicUsize,
    ) -> Option<ReentryToken> {
        let entered = latch
            .try_with(|latch| !latch.inside.replace(true))
            .unwrap_or(false);
        if entered {
            Some(ReentryToken { latch })
        } else {
            refusals.fetch_add(1, Ordering::SeqCst);
            None
        }
    }
}

impl Default for ReentryLatch {
    fn default() -> Self {
        Self::new()
    }
}

/// Holds a [`ReentryLatch`] closed for as long as it lives.
pub struct ReentryToken {
    latch: &'static std::thread::LocalKey<ReentryLatch>,
}

impl Drop for ReentryToken {
    fn drop(&mut self) {
        let _ = self.latch.try_with(|latch| latch.inside.set(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_REFUSALS: AtomicUsize = AtomicUsize::new(0);
    thread_local! {
        static TEST_LATCH: ReentryLatch = const { ReentryLatch::new() };
    }

    /// The counter and the flag are process-global, so these take turns.
    static TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serialize() -> std::sync::MutexGuard<'static, ()> {
        TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn enter() -> Option<ReentryToken> {
        ReentryLatch::enter(&TEST_LATCH, &TEST_REFUSALS)
    }

    /// One entry per thread at a time, and the flag must clear when the token dies -- a latch that
    /// leaked its flag would silence every later call on that thread, which is the same bug
    /// wearing the opposite costume.
    #[test]
    fn a_second_entry_on_the_same_thread_is_refused_until_the_first_is_dropped() {
        let _serialized = serialize();
        let before = TEST_REFUSALS.load(Ordering::SeqCst);
        let outer = enter().expect("a fresh thread is not nested");
        assert!(enter().is_none());
        assert_eq!(TEST_REFUSALS.load(Ordering::SeqCst), before + 1);
        drop(outer);
        assert!(enter().is_some());
        assert_eq!(TEST_REFUSALS.load(Ordering::SeqCst), before + 1);
    }

    /// Per thread, not per process: two threads faulting or logging at once are two independent
    /// events, and a process-wide flag would drop the second one for no reason.
    #[test]
    fn the_latch_is_per_thread_not_per_process() {
        let _serialized = serialize();
        let outer = enter().expect("a fresh thread is not nested");
        let other_thread_entered = std::thread::spawn(|| enter().is_some())
            .join()
            .expect("thread joins");
        assert!(other_thread_entered);
        drop(outer);
    }

    /// Models the production descent: a body whose own work re-enters it. Without the latch this
    /// recurses until the stack dies, which is exactly what the VEH did on 2026-08-28.
    #[test]
    fn a_body_that_re_enters_itself_runs_once() {
        let _serialized = serialize();
        fn body(entries: &Cell<usize>) {
            let Some(_token) = enter() else {
                return;
            };
            entries.set(entries.get() + 1);
            assert!(
                entries.get() <= 1,
                "the latch let it recurse: {} entries",
                entries.get()
            );
            body(entries);
        }

        let entries = Cell::new(0);
        body(&entries);
        assert_eq!(entries.get(), 1, "exactly one entry survives the latch");
    }
}

/// A re-entrancy latch shared by every module in the process, not just this one.
///
/// # Why the thread-local latch above is not enough for the crash-logger VEH
///
/// [`ReentryLatch`] is declared in a `thread_local!` **per module**. That bounds one module
/// guarding one path. It cannot bound the VEH, because every DLL in this workspace that links a
/// crash logger installs its own vectored handler holding its own copy of the static. A fault
/// raised while module A is describing a fault is therefore still a *first* entry for modules B,
/// C and D. Each of them describes it, each description can fault in turn, and the amplification
/// the per-module latch was added to stop returns multiplied by the number of loggers loaded.
///
/// Measured 2026-09-02, ELDEN RING 1.17, 24 native DLLs: one `0xc000001d` at `game+0x10043`, then
/// 214 identical `0xc0000005` inside ntdll's unwinder, `rsp` marching from `0x10f560` down to
/// `0x13810` -- a megabyte of stack -- until the faulting thread died and the session wedged with
/// its window still up. Four crash logs each recorded the same storm (`er-quickload` 213,
/// `er-net-effects` 214, `er-loading-bar` 64, `er-loading-portrait` 64). That is the per-module
/// latch working exactly as designed, four times over, on four private copies of the flag.
///
/// # What is shared, and what deliberately is not
///
/// The backing store is a named zero-filled section (`CreateFileMappingW` against the pagefile,
/// named with this process's id so two running games never share one), mapped by whichever module
/// asks first and by every module after it -- the same page in all of them.
///
/// What the page holds is a table of the thread ids currently inside a report, not a single
/// process-wide flag. The distinction is the one the module docs above already draw: a
/// process-wide boolean would mute a legitimate concurrent fault on another thread, which in a
/// crash logger silently drops most of the log. Entry is refused only when *this* thread is
/// already reporting somewhere in the process.
///
/// If the section cannot be created or mapped, the table falls back to a private per-module
/// `static`. That is exactly today's behaviour, so a mapping failure is never worse than not
/// having this type at all.
pub mod process_wide {
    use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    /// Threads that may be inside a report at once. A fault storm is not wide, it is deep; this
    /// only has to cover genuine concurrency, and 64 is far past the number of threads that fault
    /// in the same instant.
    pub const SLOTS: usize = 64;

    /// The id no thread has, so a zero-filled page is already an empty table.
    const EMPTY: u32 = 0;

    /// Private fallback, used when the shared section is unavailable. Same shape, module-local
    /// reach -- i.e. the behaviour that shipped before this module existed.
    static FALLBACK: [AtomicU32; SLOTS] = [const { AtomicU32::new(EMPTY) }; SLOTS];

    /// A slot held for as long as this lives.
    pub struct ProcessWideToken {
        slot: &'static AtomicU32,
    }

    impl Drop for ProcessWideToken {
        fn drop(&mut self) {
            self.slot.store(EMPTY, Ordering::SeqCst);
        }
    }

    /// `Some(token)` when this thread is not already inside a report anywhere in the process.
    ///
    /// `None` -- and a bump of `refusals` -- when it is, and also when the table is full. A full
    /// table means 64 threads are reporting at once, which is itself a storm; refusing follows the
    /// module's "unanswerable means nested" rule, because guessing the other way costs the
    /// process.
    pub fn enter(refusals: &'static AtomicUsize, thread_id: u32) -> Option<ProcessWideToken> {
        // A caller that cannot name its thread cannot be told apart from a nested one.
        if thread_id == EMPTY {
            refusals.fetch_add(1, Ordering::SeqCst);
            return None;
        }
        let table = table();
        // Already inside on this thread anywhere in the process: this is the nested entry.
        for slot in table.iter() {
            if slot.load(Ordering::SeqCst) == thread_id {
                refusals.fetch_add(1, Ordering::SeqCst);
                return None;
            }
        }
        for slot in table.iter() {
            if slot
                .compare_exchange(EMPTY, thread_id, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Some(ProcessWideToken { slot });
            }
        }
        refusals.fetch_add(1, Ordering::SeqCst);
        None
    }

    /// How many slots the live table has, and whether it is the shared one. For tests and for the
    /// install-time log line that says whether cross-module coverage is actually in effect.
    pub fn is_shared() -> bool {
        !core::ptr::eq(table().as_ptr(), FALLBACK.as_ptr())
    }

    #[cfg(not(windows))]
    fn table() -> &'static [AtomicU32; SLOTS] {
        &FALLBACK
    }

    #[cfg(windows)]
    fn table() -> &'static [AtomicU32; SLOTS] {
        use core::sync::atomic::AtomicPtr;

        static MAPPED: AtomicPtr<AtomicU32> = AtomicPtr::new(core::ptr::null_mut());
        static RESOLVED: AtomicUsize = AtomicUsize::new(0);

        // Resolved once; every later call is one relaxed load. Racing callers may both map the
        // section, which is harmless: the name makes it the same page either way, and the loser's
        // view leaks one mapping for the life of the process.
        if RESOLVED.load(Ordering::Acquire) == 0 {
            let mapped = map_shared_table();
            MAPPED.store(mapped, Ordering::Release);
            RESOLVED.store(1, Ordering::Release);
        }
        let mapped = MAPPED.load(Ordering::Acquire);
        if mapped.is_null() {
            return &FALLBACK;
        }
        // SAFETY: `map_shared_table` returns either null or a view of at least
        // `SLOTS * size_of::<AtomicU32>()` zeroed bytes, which is a valid `[AtomicU32; SLOTS]`,
        // and the view is never unmapped.
        unsafe { &*(mapped as *const [AtomicU32; SLOTS]) }
    }

    #[cfg(windows)]
    fn map_shared_table() -> *mut core::sync::atomic::AtomicU32 {
        use core::ffi::c_void;

        const INVALID_HANDLE_VALUE: isize = -1;
        const PAGE_READWRITE: u32 = 0x04;
        const FILE_MAP_ALL_ACCESS: u32 = 0xf001f;
        const VIEW_BYTES: usize = 4096;

        unsafe extern "system" {
            fn CreateFileMappingW(
                file: isize,
                attributes: *mut c_void,
                protect: u32,
                size_high: u32,
                size_low: u32,
                name: *const u16,
            ) -> *mut c_void;
            fn MapViewOfFile(
                mapping: *mut c_void,
                access: u32,
                offset_high: u32,
                offset_low: u32,
                bytes: usize,
            ) -> *mut c_void;
            fn GetCurrentProcessId() -> u32;
        }

        // Per-process, not per-session: `Local\` alone would make two running copies of the game
        // share one table, and one game's fault storm would then refuse the other's reports.
        let mut name = [0u16; 64];
        let written = write_section_name(&mut name, unsafe { GetCurrentProcessId() });
        if written == 0 {
            return core::ptr::null_mut();
        }

        // SAFETY: a pagefile-backed section of a fixed size with a NUL-terminated name; both calls
        // report failure by returning null, which is handled.
        let mapping = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                core::ptr::null_mut(),
                PAGE_READWRITE,
                0,
                VIEW_BYTES as u32,
                name.as_ptr(),
            )
        };
        if mapping.is_null() {
            return core::ptr::null_mut();
        }
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, VIEW_BYTES) };
        view as *mut core::sync::atomic::AtomicU32
    }

    /// `Local\er-veh-latch-v1-<pid>` as NUL-terminated UTF-16, without formatting machinery: this
    /// runs on the path that describes a crash, so it allocates nothing.
    ///
    /// Returns the number of units written, or 0 if the buffer is too small to hold the name.
    pub fn write_section_name(out: &mut [u16], pid: u32) -> usize {
        const PREFIX: &[u8] = b"Local\\er-veh-latch-v1-";
        let mut digits = [0u8; 10];
        let mut count = 0usize;
        let mut value = pid;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            count += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        if out.len() < PREFIX.len() + count + 1 {
            return 0;
        }
        let mut written = 0usize;
        for byte in PREFIX {
            out[written] = u16::from(*byte);
            written += 1;
        }
        for index in (0..count).rev() {
            out[written] = u16::from(digits[index]);
            written += 1;
        }
        out[written] = 0;
        written + 1
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        static REFUSALS: AtomicUsize = AtomicUsize::new(0);

        /// The table is process-global, so these take turns.
        static TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

        fn serialize() -> std::sync::MutexGuard<'static, ()> {
            TESTS.lock().unwrap_or_else(|poison| poison.into_inner())
        }

        #[test]
        fn the_second_entry_on_one_thread_is_refused() {
            let _guard = serialize();
            let before = REFUSALS.load(Ordering::SeqCst);
            let outer = enter(&REFUSALS, 7).expect("first entry is admitted");
            assert!(
                enter(&REFUSALS, 7).is_none(),
                "the nested entry on the same thread is refused"
            );
            assert_eq!(REFUSALS.load(Ordering::SeqCst), before + 1);
            drop(outer);
            enter(&REFUSALS, 7).expect("the slot is released with the token");
        }

        /// The distinction the whole design turns on: another thread faulting at the same instant
        /// is a real fault and must still be described.
        #[test]
        fn a_different_thread_is_admitted_concurrently() {
            let _guard = serialize();
            let first = enter(&REFUSALS, 11).expect("thread 11 admitted");
            let second = enter(&REFUSALS, 12).expect("thread 12 admitted while 11 is reporting");
            drop(first);
            drop(second);
        }

        #[test]
        fn a_thread_id_of_zero_is_treated_as_nested() {
            let _guard = serialize();
            let before = REFUSALS.load(Ordering::SeqCst);
            assert!(enter(&REFUSALS, 0).is_none());
            assert_eq!(REFUSALS.load(Ordering::SeqCst), before + 1);
        }

        #[test]
        fn a_full_table_refuses_rather_than_guessing() {
            let _guard = serialize();
            let held: Vec<_> = (1..=SLOTS as u32)
                .map(|id| enter(&REFUSALS, id).expect("distinct threads fill the table"))
                .collect();
            assert!(
                enter(&REFUSALS, SLOTS as u32 + 1).is_none(),
                "no free slot means refuse"
            );
            drop(held);
        }

        #[test]
        fn the_section_name_is_per_process_and_nul_terminated() {
            let mut buf = [0u16; 64];
            let written = write_section_name(&mut buf, 3586501);
            let text: String = buf[..written - 1]
                .iter()
                .map(|u| *u as u8 as char)
                .collect();
            assert_eq!(text, "Local\\er-veh-latch-v1-3586501");
            assert_eq!(buf[written - 1], 0, "the name is NUL-terminated");
        }

        #[test]
        fn a_buffer_too_small_for_the_name_reports_zero() {
            let mut buf = [0u16; 8];
            assert_eq!(write_section_name(&mut buf, 3586501), 0);
        }
    }
}
