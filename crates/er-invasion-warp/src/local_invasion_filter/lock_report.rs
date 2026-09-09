//! Naming the thread that already holds ERSC's session lock.
//!
//! Every ERSC option action opens by locking a `std::mutex` that lives inside the session, and on
//! 2026-09-07 the first rejected match of a run aborted the process there: `_Mtx_lock` returned
//! nonzero and Seamless threw `std::system_error` with errno 36. Nothing in this repo takes that
//! lock, and ERSC holds it across zero calls at all fifteen of its own acquisition sites, so the
//! owner had no name.
//!
//! This module is the reading that gives it one. It logs; it decides nothing, and no caller
//! branches on it. See [`report_lock_preconditions`] for what the abort already proves on its own
//! and which two hypotheses are left for the log line to separate.
//!
//! It sits in its own file rather than in the parent because the parent was one addition away from
//! the hard limit `scripts/check-rust-file-sizes.py` enforces.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::{SeamlessSession, ersc, osm_tag_matches, plausible_session_pointer, state_name};

/// A handler of ours that ERSC or the game can enter, with the thread it was last entered on and
/// how many of its frames are live.
///
/// [`report_lock_preconditions`] can say which thread owns the lock and nothing more, because a
/// thread id is all the abort leaves behind. Each handler stamps itself on entry so the log can
/// turn that number back into a place.
struct HandlerThread {
    what: &'static str,
    /// The thread most recently seen entering this handler. Never cleared, so a handler that has
    /// already returned still says which thread ran it.
    last: AtomicU32,
    /// Live frames of this handler across every thread. Non-zero means one is on a stack now.
    depth: AtomicU32,
}

/// Every handler of ours that can be on a stack when an ERSC action is driven.
///
/// The index constants below are the only way to address a row, so a handler added here needs a
/// name and a matching `const` and cannot be stamped by accident.
static HANDLER_THREADS: [HandlerThread; 5] = [
    HandlerThread {
        what: "ersc show",
        last: AtomicU32::new(0),
        depth: AtomicU32::new(0),
    },
    HandlerThread {
        what: "ersc BuildLobbyKey",
        last: AtomicU32::new(0),
        depth: AtomicU32::new(0),
    },
    HandlerThread {
        what: "SetMultiplayJoinData",
        last: AtomicU32::new(0),
        depth: AtomicU32::new(0),
    },
    HandlerThread {
        what: "game task tick",
        last: AtomicU32::new(0),
        depth: AtomicU32::new(0),
    },
    HandlerThread {
        what: "ersc invade action",
        last: AtomicU32::new(0),
        depth: AtomicU32::new(0),
    },
];

pub(super) const HANDLER_ERSC_SHOW: usize = 0;
pub(super) const HANDLER_ERSC_LOBBY_KEY: usize = 1;
pub(super) const HANDLER_JOIN_DATA: usize = 2;
pub(super) const HANDLER_GAME_TASK: usize = 3;
pub(super) const HANDLER_ERSC_INVADE: usize = 4;

/// Decrements the depth its [`enter_handler`] raised, on every way out of the handler.
pub(super) struct HandlerScope(usize);

impl Drop for HandlerScope {
    fn drop(&mut self) {
        if let Some(entry) = HANDLER_THREADS.get(self.0) {
            entry.depth.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Stamp this thread onto a handler for as long as the returned scope lives.
///
/// One store and one increment per call, and the game task's is one per frame, so this is cheap
/// enough to leave on a hot path.
#[must_use]
pub(super) fn enter_handler(handler: usize) -> HandlerScope {
    if let Some(entry) = HANDLER_THREADS.get(handler) {
        entry.last.store(current_thread_id(), Ordering::SeqCst);
        entry.depth.fetch_add(1, Ordering::SeqCst);
    }
    HandlerScope(handler)
}

// Depth of ERSC callbacks this thread is currently inside.
//
// # Why a counter and not a flag, and why it is thread-local
//
// Re-entering `ersc.dll` from a frame ERSC itself entered means calling its code with whatever
// locks and half-finished state that call left behind. The handlers of ours that ERSC calls
// raise this on entry, and [`inside_ersc_callback`] is what [`ersc_action`] consults before it
// hands out a function pointer, so the refusal sits at the one place every drive site passes
// through rather than at each of them.
//
// A counter rather than a flag because two of the five re-enter each other: `request_lobby_list`
// drives the string-filter vtable slot directly, which lands in our own filter handler, so a flag
// would be cleared by the inner frame while the outer one was still live.
//
// Thread-local rather than a global because ERSC's Steam handlers and the game's own frame task
// run on different threads. A global would let a lobby-list query in flight on one thread decline
// a perfectly safe cancel on another, which is a refusal that costs an unfiltered match for no
// reason at all.
//
// [`ersc_action`]: super::ersc_action
thread_local! {
    static ERSC_CALLBACK_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Lowers the depth [`enter_ersc_callback`] raised, on every way out of the handler.
pub(crate) struct ErscCallbackScope;

impl Drop for ErscCallbackScope {
    fn drop(&mut self) {
        ERSC_CALLBACK_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Mark this thread as executing inside a callback ERSC made, for as long as the scope lives.
#[must_use]
pub(crate) fn enter_ersc_callback() -> ErscCallbackScope {
    ERSC_CALLBACK_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
    ErscCallbackScope
}

/// Whether a frame ERSC entered is live on this thread right now.
pub(crate) fn inside_ersc_callback() -> bool {
    ERSC_CALLBACK_DEPTH.with(|depth| depth.get()) > 0
}

/// Which of our handlers a thread id belongs to, or that it belongs to none of them.
fn handler_for_thread(id: u32) -> &'static str {
    if id == 0 {
        return "no thread";
    }
    if id == MTX_THREAD_ID_UNOWNED {
        return "unowned";
    }
    for entry in &HANDLER_THREADS {
        if entry.last.load(Ordering::SeqCst) == id {
            return entry.what;
        }
    }
    "a thread none of our handlers has run on"
}

/// The handlers of ours that have a live frame right now, rendered for the log.
struct HandlersInFlight;

impl std::fmt::Display for HandlersInFlight {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut written = 0;
        for entry in &HANDLER_THREADS {
            let depth = entry.depth.load(Ordering::SeqCst);
            if depth == 0 {
                continue;
            }
            if written > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{} x{depth}", entry.what)?;
            written += 1;
        }
        if written == 0 {
            formatter.write_str("none")?;
        }
        Ok(())
    }
}

/// The calling thread's id: the value `mtx_do_lock` compares `_Thread_id` against, and therefore
/// the whole question this instrumentation exists to answer.
#[cfg(windows)]
fn current_thread_id() -> u32 {
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    unsafe { GetCurrentThreadId() }
}

/// Host builds never reach a real ERSC action, so there is no thread to name. Zero is not a thread
/// id Windows ever issues, which is what [`handler_for_thread`] reads it as.
#[cfg(not(windows))]
fn current_thread_id() -> u32 {
    0
}

// The `_Mtx_internal_imp_t` field offsets, read out of ersc v2.0.1's own copy of `_Mtx_unlock` at
// `ersc+0xf9830`: it decrements `+0x4c`, and when that reaches zero writes `-1` into `+0x48` and
// releases the `SRWLOCK` at `+0x10`. The base of the object is `session+0x100`, which is where
// `ersc+0x258d0` points `rcx` before its lock call -- and it is derived from the guard offset
// rather than written down twice, since the `ersc` module already records `guard = mutex+0x4c`.
const MTX_TYPE_OFFSET: usize = 0x00;
const MTX_THREAD_ID_OFFSET: usize = 0x48;
const MTX_COUNT_OFFSET: usize = 0x4c;

/// `_Mtx_recursive`. `mtx_do_lock` clears this bit before its fast-path comparison, and a mutex
/// carrying it returns success from a re-lock on the owning thread instead of `_Thrd_busy`.
const MTX_RECURSIVE: u32 = 0x100;
/// `_Mtx_plain`. A `_Type` that reduces to exactly this takes `mtx_do_lock`'s fast path, which
/// ends in `inc _Count; return _Thrd_success` and can never report the lock busy.
const MTX_PLAIN: u32 = 0x01;
/// `_Mtx_try`, the bit MSVC's `std::mutex` constructor adds. It is what puts the object on the
/// slow path, and the slow path holds the only `_Thrd_busy` return in the function.
const MTX_TRY: u32 = 0x02;
/// What `_Mtx_unlock` writes into `_Thread_id` when the last recursion level is released.
const MTX_THREAD_ID_UNOWNED: u32 = 0xffff_ffff;

/// The session states ERSC's own hide-predicate lets its Cancel row through.
///
/// Read out of `ersc+0x26b40`, which is nine instructions: it loads `[this+0x58]` and then
/// `[session+0x150]`, sets one flag from `state >= 0x13`, sets another from `bt 0x23fff, state`,
/// and returns the two combined with `or`. So the row is hidden for every state at or above `0x13`
/// and for every state whose bit is set in `0x23fff`, which is `0x00` through `0x0d` plus `0x11`.
/// The four values left over are these, and the `or` means the bit test cannot rescue a large
/// state that the range check already hid.
///
/// Driving the action while the state is outside this set is driving a row the user could not have
/// clicked, which is worth knowing before reading anything else in the line.
const CANCEL_ROW_VISIBLE_STATES: [u32; 4] = [0x0e, 0x0f, 0x10, 0x12];

/// Renders a fault-tolerant read as hex, or says plainly that the read did not land.
struct Reading<T>(Option<T>);

impl<T: std::fmt::LowerHex> std::fmt::Display for Reading<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(value) => write!(formatter, "{value:#x}"),
            None => formatter.write_str("unreadable"),
        }
    }
}

/// What the mutex header says about who holds the lock, in the words of the hypothesis it settles.
///
/// The ordering matters: the shape checks come before the ownership ones, because a `_Type` that is
/// not a mutex makes `_Thread_id` a coincidence rather than an owner.
fn lock_verdict(
    kind: Option<u32>,
    thread: Option<u32>,
    count: Option<u32>,
    me: u32,
) -> &'static str {
    let (Some(kind), Some(thread), Some(count)) = (kind, thread, count) else {
        return "the mutex header did not read back, so the address this session resolved to is \
                not addressable and no field above is a reading";
    };
    let reduced = kind & !MTX_RECURSIVE;
    if reduced == MTX_PLAIN {
        return "_Type reduces to _Mtx_plain, whose path through mtx_do_lock ends in \
                `inc _Count; return _Thrd_success`, so this object cannot be the one that reported \
                the lock busy";
    }
    if (reduced & MTX_TRY) == 0 {
        return "_Type is not a shape MSVC's mutex constructors write, so this object is not a \
                _Mtx_internal_imp_t at all and the session pointer is wrong";
    }
    if (kind & MTX_RECURSIVE) != 0 {
        return "the mutex is recursive, so a re-lock on the owning thread returns success and the \
                abort has to have come from somewhere other than this lock";
    }
    if thread == me {
        return if count == 0 {
            "_Thread_id is this thread while _Count is zero, which is the half-released state \
             _Mtx_unlock passes through; the reading is torn rather than settled"
        } else {
            "this thread already holds the lock, so the abort is a self-deadlock and the frame \
             that took it is on our own stack"
        };
    }
    if thread == MTX_THREAD_ID_UNOWNED || count == 0 {
        return "the lock is free at this instant, so driving the action from this thread should \
                acquire it rather than fail";
    }
    "another thread holds the lock, and mtx_do_lock blocks on that rather than reporting it busy, \
     so an abort after this reading would mean the owner changed between the read and the call"
}

/// Refuse to drive an ERSC action whose own `_Mtx_lock` would not succeed.
///
/// [`report_lock_preconditions`] reads these same fields and decides nothing. This one decides, and
/// the split is deliberate: the log line has to record the state that killed the process even in
/// the runs where this gate then declines, or the gate would erase its own evidence.
///
/// The three refusals map onto the three ways `mtx_do_lock` ends badly. A header that does not read
/// back means the session address is not addressable, so the action's own first load would fault. A
/// `_Type` that is not a shape MSVC's constructors write means the object is not a mutex, so the
/// pointer identifies something else and every later field is a coincidence. A lock already held is
/// the abort itself if we hold it, and a blocked game thread if somebody else does -- neither is a
/// thing to walk into.
///
/// `_Mtx_plain` and recursive shapes are let through, because `mtx_do_lock` cannot report either of
/// them busy: the plain path ends `inc _Count; return _Thrd_success`, and a recursive re-lock on the
/// owning thread returns success too.
///
/// A refusal costs one unfiltered match. Driving into a wrong object costs the session.
pub(super) fn lock_shape_refusal(session: &SeamlessSession) -> Option<&'static str> {
    let mutex = session.session + session.abi.session_guard_offset - MTX_COUNT_OFFSET;
    let kind =
        unsafe { er_game_base::mem::safe_read_i32(mutex + MTX_TYPE_OFFSET) }.map(|raw| raw as u32);
    let thread = unsafe { er_game_base::mem::safe_read_i32(mutex + MTX_THREAD_ID_OFFSET) }
        .map(|raw| raw as u32);
    let count =
        unsafe { er_game_base::mem::safe_read_i32(mutex + MTX_COUNT_OFFSET) }.map(|raw| raw as u32);
    let (Some(kind), Some(thread), Some(count)) = (kind, thread, count) else {
        return Some(
            "the mutex header at session+0x100 did not read back, so this address is not a live              session and the action's own first load would fault",
        );
    };
    if !mutex_type_is_constructible(kind) {
        return Some(
            "_Type at session+0x100 is not a shape MSVC's mutex constructors write, so this is not              a _Mtx_internal_imp_t and the session pointer identifies something else",
        );
    }
    let reduced = kind & !MTX_RECURSIVE;
    if reduced == MTX_PLAIN || (kind & MTX_RECURSIVE) != 0 {
        return None;
    }
    if count == 0 || thread == MTX_THREAD_ID_UNOWNED {
        return None;
    }
    if thread == current_thread_id() {
        return Some(
            "this thread already holds the session mutex, so the action's _Mtx_lock would return              _Thrd_busy and ERSC would throw errno 36 out of a nounwind boundary",
        );
    }
    Some(
        "another thread holds the session mutex, so the action would block the caller inside          ersc.dll rather than return",
    )
}

/// Whether a `_Type` word is one MSVC's mutex constructors could have written.
///
/// The three accepted shapes are the three [`lock_shape_refusal`] lets past its identity arm:
/// `_Mtx_plain`, anything carrying `_Mtx_recursive`, and anything carrying `_Mtx_try` -- the bit
/// `std::mutex`'s own constructor adds. Everything else means the object is not an
/// `_Mtx_internal_imp_t`, so the pointer it was reached through is not a session.
///
/// Factored out because it now has two callers that must not be able to disagree: the refusal
/// below, which runs when an action is about to be driven, and
/// [`mutex_shape_identifies_a_session`], which runs while the scan is still choosing what to
/// believe. Two copies of this rule would let the scan accept a pointer the action then refuses,
/// which is exactly the run this was factored out of.
fn mutex_type_is_constructible(kind: u32) -> bool {
    let reduced = kind & !MTX_RECURSIVE;
    reduced == MTX_PLAIN || (kind & MTX_RECURSIVE) != 0 || (reduced & MTX_TRY) != 0
}

/// Whether `session` carries an `_Mtx_internal_imp_t` where a session carries one.
///
/// The identity half of [`lock_shape_refusal`] with the liveness half left out, for use as a
/// signature rather than as a gate. A held lock says nothing about whether the object is a
/// session -- it is a reason not to drive an action this instant, not a reason to reject the
/// pointer -- so the scan must ask only this much.
///
/// # The run that put this here
///
/// br-20260908-193258-27ba resolved session `0x860f90f8`, judged one match, and refused to cancel
/// it because `_Type` at `+0x100` was not a shape MSVC writes. The refusal was right and it came
/// too late: `cached_scan_for_session` had already latched that pointer, and it revalidates with
/// the same predicate that accepted it, so the wrong answer held for the process. The discriminator
/// that catches this existed the whole time; it just ran at the wrong end of the run.
pub(super) fn mutex_shape_identifies_a_session(abi: &ersc::Abi, session: usize) -> bool {
    let mutex = session + abi.session_guard_offset - MTX_COUNT_OFFSET;
    let Some(kind) =
        unsafe { er_game_base::mem::safe_read_i32(mutex + MTX_TYPE_OFFSET) }.map(|raw| raw as u32)
    else {
        return false;
    };
    // Deliberately stricter than `lock_shape_refusal`, which lets `_Mtx_plain` past because
    // `mtx_do_lock` cannot report a plain mutex busy. That is sound as a liveness gate and
    // useless as a signature: `_Mtx_plain` is `1`, the most common non-zero dword in memory.
    //
    // Live proof, run br-20260908-200726-15d4. The scan accepted `0x451200`, and reading it out
    // of the running process shows a table with stride `0x50`, every boundary holding
    // `01 00 00 00 00 00 00 00`. The state field is at `+0x150` and the mutex at `+0x100`, which
    // are `0x50` apart -- one stride -- so both checks read the same repeating `1` and agreed
    // with each other about an object that is neither. Two checks that land on one field are one
    // check.
    //
    // `_Mtx_try` is the bit MSVC's `std::mutex` constructor adds, and ERSC's session lock is a
    // `std::mutex` (`_Verify_ownership_levels` is inlined into the actions -- see
    // `ersc::Abi::session_guard_offset`), so a real session reads `0x03`, or `0x103` recursive.
    // This crate's own `lock_verdict` tests already pin `STD_MUTEX = 0x03`.
    if (kind & MTX_TRY) == 0 {
        return false;
    }
    // `_Count` on a non-recursive `std::mutex` only ever goes `0 -> 1`, so anything else is not
    // this object. The same `0x451200` reads `0x6fff` here -- 28,671 nested acquisitions, which
    // no lock has ever held.
    let count = unsafe { er_game_base::mem::safe_read_i32(mutex + MTX_COUNT_OFFSET) };
    let recursive = (kind & MTX_RECURSIVE) != 0;
    count.is_some_and(|raw| recursive || (raw as u32) <= 1)
}

/// Whether ERSC's own hide-predicate would draw its Cancel row in this state.
pub(super) fn cancel_row_offered(state: u32) -> bool {
    CANCEL_ROW_VISIBLE_STATES.contains(&state)
}

/// Refuse the Cancel action in a state where ERSC does not draw its own Cancel row.
///
/// The set comes from [`CANCEL_ROW_VISIBLE_STATES`], which is ERSC's own hide-predicate rather than
/// a rule of ours. Driving a row the player could not have clicked is driving the action outside
/// every precondition its author arranged for it, and the state where the filter judges an incoming
/// match is measurably outside that set -- the runtime-observed sequence runs `0x0e, 0x0f, 0x12,
/// 0x13, 0x14, 0x15`, and the row is withdrawn from `0x13` upward.
///
/// An unreadable state refuses: not knowing which state we are in is not a licence to drive.
pub(super) fn cancel_row_refusal(session: &SeamlessSession) -> Option<String> {
    let state = unsafe {
        er_game_base::mem::safe_read_i32(session.session + session.abi.session_state_offset)
    }
    .map(|raw| raw as u32);
    let Some(state) = state else {
        return Some(
            "the session state did not read back, so whether ERSC would offer its own Cancel row              is unknown"
            .to_owned(),
        );
    };
    if cancel_row_offered(state) {
        return None;
    }
    Some(format!(
        "the session is in state {state:#x}, and ERSC's own hide-predicate draws its Cancel row          only for {CANCEL_ROW_VISIBLE_STATES:#x?} -- so this would drive a row the player could          not have clicked"
    ))
}

/// Read everything the `_Mtx_lock` abort turns on, immediately before an ERSC action is driven.
///
/// Logs and returns. It decides nothing, and no caller branches on it.
///
/// # What the abort already proves, and what it does not
///
/// The first rejected match aborted the process inside `ersc+0x258d0`. Read out of the installed
/// v2.0.1 module, that function does six things before it does anything else:
///
/// ```text
///   mov  rdi,[rcx+0x58]                 ; the session
///   lea  rsi,[rdi+0x100]                ; its std::mutex
///   call ersc+0xf9828                   ; _Mtx_lock
///   test eax,eax
///   jne  ersc+0x25915                   ; -> _Throw_Cpp_error(5), errno 36
///   cmp  dword [rdi+0x14c],0x7fffffff   ; the guard, which is also the mutex's own _Count
/// ```
///
/// `ersc+0xf9828` is `_Mtx_lock`: `xor edx,edx; jmp mtx_do_lock`. Reading `mtx_do_lock` through to
/// its returns leaves exactly one path that hands back a nonzero result -- the caller's own thread
/// id is already in `_Thread_id`, the incremented `_Count` comes out above 1, and `_Type` does not
/// carry `MTX_RECURSIVE`. Every other path either takes the `SRWLOCK` and returns `_Thrd_success`
/// or blocks waiting for it. So the abort is not ambiguous about the owner: it names the calling
/// thread, and a busy return from a foreign owner is not a thing that function can produce.
///
/// That leaves two readings, and this line separates them. Either the session pointer is right and
/// a frame further up our own stack already took the lock, or the pointer is wrong and
/// `session+0x148` merely happens to hold this thread's id -- which `_Type` settles, since a live
/// `_Mtx_internal_imp_t` reads `0x03` or `0x103` there and arbitrary memory does not.
///
/// The guard field is worth watching for its own reason: `session+0x14c` is bit for bit the
/// mutex's `_Count`, so ERSC's `0x7fffffff` sentinel and the recursion count are the same dword.
/// The last reading published, so a per-frame drive does not publish it per frame.
///
/// Measured 2026-09-08 on run `br-20260908-185557-aa9c`: 22737 `about to drive` lines and 22735
/// refusals in one session, from five invasions. `drive_pending_reinvade` runs on the game task, so
/// every one of these is once per frame, and the line was written as though it were once per drive.
/// A log that repeats an unchanged fact twenty thousand times is not evidence, it is cover -- the
/// two lines that mattered were the two `REJECT`s, and they were three ten-thousandths of the file.
///
/// The latch is the reading itself, hashed, not a counter or a timer. An unchanged session
/// publishes once; the frame the pointer, the state, or the lock's owner moves publishes again,
/// which is exactly when a reader needs a new line. `0` is not a reachable hash for a real reading
/// (the verdict string is never empty), so it doubles as the empty state.
static LAST_REPORTED: AtomicU64 = AtomicU64::new(0);

/// Fold one reading into the latch key. Fnv1a, so a changed nibble anywhere changes the answer.
fn reading_key(parts: &[u64]) -> u64 {
    let mut hash = er_game_base::fnv1a::FNV1A64_OFFSET_BASIS;
    for part in parts {
        hash = er_game_base::fnv1a::fnv1a64_extend(hash, &part.to_le_bytes());
    }
    hash | 1
}

pub(super) fn report_lock_preconditions(session: &SeamlessSession, owner: usize, what: &str) {
    let abi = session.abi;
    let mutex = session.session + abi.session_guard_offset - MTX_COUNT_OFFSET;
    let linked = unsafe { er_game_base::mem::safe_read_usize(owner + ersc::NEXT_OBJECT_OFFSET) };
    let state =
        unsafe { er_game_base::mem::safe_read_i32(session.session + abi.session_state_offset) }
            .map(|raw| raw as u32);
    let guard =
        unsafe { er_game_base::mem::safe_read_i32(session.session + abi.session_guard_offset) }
            .map(|raw| raw as u32);
    let kind =
        unsafe { er_game_base::mem::safe_read_i32(mutex + MTX_TYPE_OFFSET) }.map(|raw| raw as u32);
    let thread = unsafe { er_game_base::mem::safe_read_i32(mutex + MTX_THREAD_ID_OFFSET) }
        .map(|raw| raw as u32);
    let count =
        unsafe { er_game_base::mem::safe_read_i32(mutex + MTX_COUNT_OFFSET) }.map(|raw| raw as u32);
    let me = current_thread_id();
    let key = reading_key(&[
        owner as u64,
        session.session as u64,
        linked.unwrap_or(0) as u64,
        u64::from(state.unwrap_or(u32::MAX)),
        u64::from(guard.unwrap_or(u32::MAX)),
        u64::from(kind.unwrap_or(u32::MAX)),
        u64::from(thread.unwrap_or(u32::MAX)),
        u64::from(count.unwrap_or(u32::MAX)),
        what.len() as u64,
    ]);
    if LAST_REPORTED.swap(key, Ordering::SeqCst) == key {
        return;
    }
    crate::standalone_log(format_args!(
        "local-invasion: about to drive ERSC {what} -- owner=0x{owner:x} ({}, tag {}) \
         session=0x{:x} ({}) owner+0x{:x}={} ({}) state={} {} (ERSC would draw its own Cancel row: \
         {}) guard=+0x{:x}={} ({}) mutex=0x{mutex:x} _Type={} _Thread_id={} (owned by: {}) \
         _Count={} this_thread=0x{me:x} (which is: {}) in_flight=[{}]. {}",
        if plausible_session_pointer(owner) {
            "plausible"
        } else {
            "implausible"
        },
        if osm_tag_matches(owner) {
            "present"
        } else {
            "absent"
        },
        session.session,
        if plausible_session_pointer(session.session) {
            "plausible"
        } else {
            "implausible"
        },
        ersc::NEXT_OBJECT_OFFSET,
        Reading(linked),
        match linked {
            Some(next) if next == session.session => "agrees",
            Some(_) => "disagrees, so the action will lock a different object than the one read",
            None => "unreadable, so the action's own first load would fault",
        },
        Reading(state),
        state.map_or("(no name)", |raw| state_name(abi, raw)),
        match state {
            Some(raw) if CANCEL_ROW_VISIBLE_STATES.contains(&raw) => "yes",
            Some(_) => "no, so this drives a row the user could not have clicked",
            None => "unknown",
        },
        abi.session_guard_offset,
        Reading(guard),
        match guard {
            Some(ersc::SESSION_GUARD_POISON) => "the sentinel",
            Some(_) => "clear",
            None => "unreadable",
        },
        Reading(kind),
        Reading(thread),
        thread.map_or("unreadable", handler_for_thread),
        Reading(count),
        handler_for_thread(me),
        HandlersInFlight,
        lock_verdict(kind, thread, count, me),
    ));
}

/// The verdict table, pinned so it is a check rather than a claim.
///
/// The whole value of the log line is that a reader can map one reading onto one hypothesis, and
/// the mapping is a chain of early returns whose order is load-bearing: a `_Type` that is not a
/// mutex has to be answered before `_Thread_id`, because on a wrong object that field is a
/// coincidence. Reordering the chain would still compile and would still log a confident sentence.
#[cfg(test)]
mod tests {
    use super::{
        MTX_THREAD_ID_UNOWNED, cancel_row_offered, enter_ersc_callback, inside_ersc_callback,
        lock_verdict,
    };

    /// `_Mtx_plain | _Mtx_try`, what MSVC's `std::mutex` constructor writes.
    const STD_MUTEX: u32 = 0x03;
    /// The same, plus `_Mtx_recursive`: `std::recursive_mutex`.
    const RECURSIVE_MUTEX: u32 = 0x103;
    const US: u32 = 0x1234;
    const SOMEONE_ELSE: u32 = 0x5678;

    #[test]
    fn an_unchanged_reading_publishes_once_and_a_changed_one_publishes_again() {
        // The whole point of the latch: 22737 identical lines in one session became one.
        use super::{LAST_REPORTED, reading_key};
        use std::sync::atomic::Ordering;
        LAST_REPORTED.store(0, Ordering::SeqCst);
        let a = reading_key(&[1, 2, 3]);
        let b = reading_key(&[1, 2, 4]);
        assert_ne!(a, b, "a changed field changes the key");
        assert_ne!(a, 0, "0 is reserved for the empty latch");
        assert_eq!(
            LAST_REPORTED.swap(a, Ordering::SeqCst),
            0,
            "first reading is new"
        );
        assert_eq!(
            LAST_REPORTED.swap(a, Ordering::SeqCst),
            a,
            "the same reading is a repeat"
        );
        assert_eq!(
            LAST_REPORTED.swap(b, Ordering::SeqCst),
            a,
            "a moved field speaks again"
        );
    }

    #[test]
    fn ersc_draws_its_cancel_row_only_for_the_four_states_its_predicate_lets_through() {
        for state in [0x0e, 0x0f, 0x10, 0x12] {
            assert!(
                cancel_row_offered(state),
                "{state:#x} is in the predicate's gap"
            );
        }
        // The range check hides everything from 0x13 up, and the bit mask hides 0x00..=0x0d plus
        // 0x11. The state the filter judges an incoming match in is on the hidden side of both.
        for state in [0x00, 0x01, 0x0d, 0x11, 0x13, 0x14, 0x15, 0x23] {
            assert!(!cancel_row_offered(state), "{state:#x} should be hidden");
        }
    }

    #[test]
    fn the_ersc_callback_depth_nests_rather_than_latching() {
        assert!(!inside_ersc_callback());
        let outer = enter_ersc_callback();
        assert!(inside_ersc_callback());
        {
            // `request_lobby_list_hook` drives the string-filter slot directly, which lands back in
            // our own filter handler -- a flag would be cleared here while the outer frame is live.
            let _inner = enter_ersc_callback();
            assert!(inside_ersc_callback());
        }
        assert!(inside_ersc_callback());
        drop(outer);
        assert!(!inside_ersc_callback());
    }

    #[test]
    fn a_read_that_faulted_is_reported_as_a_wrong_address_not_as_a_lock_state() {
        for reading in [
            lock_verdict(None, Some(US), Some(1), US),
            lock_verdict(Some(STD_MUTEX), None, Some(1), US),
            lock_verdict(Some(STD_MUTEX), Some(US), None, US),
        ] {
            assert!(reading.contains("did not read back"), "{reading}");
        }
    }

    #[test]
    fn the_shape_of_the_object_is_judged_before_its_owner() {
        // Both of these carry our own thread id, which on a real mutex would read as a
        // self-deadlock. Neither may be reported that way, because neither object is a mutex.
        let plain = lock_verdict(Some(0x01), Some(US), Some(1), US);
        assert!(plain.contains("cannot be the one"), "{plain}");
        let garbage = lock_verdict(Some(0x55), Some(US), Some(1), US);
        assert!(garbage.contains("not a shape"), "{garbage}");
    }

    #[test]
    fn a_recursive_mutex_cannot_be_the_source_of_the_abort() {
        let reading = lock_verdict(Some(RECURSIVE_MUTEX), Some(US), Some(3), US);
        assert!(reading.contains("recursive"), "{reading}");
    }

    #[test]
    fn our_own_thread_id_in_a_held_mutex_is_the_self_deadlock_reading() {
        let reading = lock_verdict(Some(STD_MUTEX), Some(US), Some(1), US);
        assert!(reading.contains("self-deadlock"), "{reading}");
    }

    #[test]
    fn our_own_thread_id_with_no_recursion_left_is_reported_as_torn_rather_than_settled() {
        let reading = lock_verdict(Some(STD_MUTEX), Some(US), Some(0), US);
        assert!(reading.contains("torn"), "{reading}");
    }

    #[test]
    fn an_unowned_lock_is_reported_free_whichever_field_says_so() {
        for reading in [
            lock_verdict(Some(STD_MUTEX), Some(MTX_THREAD_ID_UNOWNED), Some(0), US),
            lock_verdict(Some(STD_MUTEX), Some(SOMEONE_ELSE), Some(0), US),
        ] {
            assert!(reading.contains("free at this instant"), "{reading}");
        }
    }

    #[test]
    fn a_foreign_owner_is_reported_as_the_reading_that_does_not_explain_the_abort() {
        let reading = lock_verdict(Some(STD_MUTEX), Some(SOMEONE_ELSE), Some(1), US);
        assert!(
            reading.contains("another thread holds the lock"),
            "{reading}"
        );
        assert!(reading.contains("blocks"), "{reading}");
    }
}
