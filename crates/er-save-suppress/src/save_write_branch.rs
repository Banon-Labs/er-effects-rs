// WHICH WRITE BRANCH RAN.
//
// `include!`d into `lib.rs` (same module, same imports) like `save_job_completion.rs`.
//
// `SaveLoad2::SLSaveSession`'s job body `FUN_14240fd70` has TWO mutually exclusive write
// paths, and until these two counters existed nothing in the process could say which one a
// save took. Every claim about the steady state -- that a save over an existing container
// patches blocks in place and never rebuilds -- was read out of the decompile, not measured.
// These observers measure it.
//
// The choice is made INSIDE `FUN_14240fd70` (the `if`/`else` is its own), on the result code
// of a THIRD, separate call, `FUN_142413230` @line 114 of the decompile:
//
//   uVar4 = FUN_142413230(job+0x280, session, path);  // probe: is in-place viable?
//   FUN_14240dbf0(job, uVar4);                        // publish into job+0x9c
//   iVar5 = FUN_14240d8d0(job);                       // read it back out
//   if (iVar5 == 0) { FUN_142413860(job+0x281, ..) }  // -> FULL REBUILD
//   else            { FUN_1424142b0(job+0x282, ..);   // -> per-block IN-PLACE,
//                     FUN_1424142e0(job+0x283, ..) }  //    once per supplied block
//
// `FUN_142413230` mounts the container ALREADY ON DISK at the save path and walks every
// block the request supplies, testing `entry.size + entry.padding >= needed`. It returns `6`
// when the mount succeeded AND every block still fits, and `0` when the mount failed or any
// block outgrew its entry. Note that polarity is the INVERSE of the writers' own convention
// (both writers return 0 = success, 6 = failure), which is exactly why the branch reads like
// it selects the rebuild on success. It does not: 0 from the probe means "in-place is not
// viable", and the rebuild is the fallback.
//
// The probe mounts the ORIGINAL save path, so this decision does not depend on where the
// bytes eventually land -- a write-open redirect diverts the writers, not the probe. That
// part is CODE-DERIVED, not measured; these counters are the first instrument that can begin
// to test it.
//
// Both are pure observers: forward, count, return the callee's value unchanged.

/// `FUN_142413860` -- the FULL CONTAINER REBUILD branch, the `iVar5 == 0` arm. One
/// whole-buffer write from offset 0 after reading back every block the request did not
/// supply. Expected to be RARE (a block outgrew its entry, or no usable container).
///
/// Takes exactly FOUR register arguments and nothing on the stack. Its only call site
/// `0x142410158` is `lea rcx,[rbx+0x281]; mov rdx,[rbx+0xe0]; lea r8,[rbp-1]; xor r9d,r9d;
/// call` -- there is no write to `[rsp+0x20]`, so the union's 4-argument shape describes it
/// exactly and it goes through the shared union like the dispatch observers.
#[cfg(windows)]
const SAVE_WRITE_FULL_REBUILD_RVA: usize = 0x2413860;

/// `FUN_1424142e0` -- the PER-BLOCK IN-PLACE PATCHER, the `else` arm. Called once per
/// supplied block: `OpenFile` -> `Seek(entry.dataOffset)` -> `WriteBytes(block)` -> maybe
/// rewrite the 0x20-byte entry header -> `Seek(0,END)` -> close. Expected to be the steady
/// state, so this counter normally climbs by more than one per save.
///
/// Takes FIVE arguments, the fifth ON THE STACK, and the union's 4-argument shape must NOT be
/// used for it. Its call site `0x1424102d6` writes the fifth explicitly --
/// `mov [rsp+0x20], r12` between the `lea rcx` and the `call` -- and the callee both keeps it
/// (`local_1a8 = param_5`) and, when it is non-null, DEREFERENCES AND WRITES THROUGH it with
/// two qwords at the end of a successful write. A 4-argument detour would hand the callee
/// whatever the Rust frame happened to leave at `[rsp+0x28]` and it would be written to as an
/// out-parameter, so the union shape is not merely lossy here, it corrupts memory. This one
/// therefore gets its own `MhHook` with a 5-argument type, for the same reason the
/// quit-settle (0-argument) and save-job-body (1-argument void) observers do.
#[cfg(windows)]
const SAVE_WRITE_IN_PLACE_RVA: usize = 0x24142e0;

/// The result code both writers return for "the write did NOT happen" -- `uVar13 = 6` on
/// every failure path of `FUN_1424142e0`, and 6 = open/write/short-write failure in
/// `FUN_142413860`. It is what a missing-trampoline observer must return, because the other
/// value in that space, `0`, means SUCCESS to `FUN_14240dbf0`/`FUN_14240d8d0` and would tell
/// the game a save was written when nothing was.
///
/// Ungated like the prologue signatures so the host-side unit tests can assert it.
const SAVE_WRITE_FAILED_RESULT: usize = 6;

// Both write branches open `push rbp; push rsi; push rdi; push r12; push r13; push r14;
// push r15` -- seven whole one- and two-byte instructions before MinHook's 5-byte window
// closes, and no relative branch. They diverge only at byte 12, where the frame pointer is
// set up: the rebuild takes `lea rbp,[rsp-0x60]` and the patcher `lea rbp,[rsp-0xd0]`.
const SAVE_WRITE_FULL_REBUILD_SIG: &[u8] = &[
    0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8D, 0x6C, 0x24,
    0xA0, 0x48, 0x81, 0xEC, 0x60, 0x01, 0x00, 0x00,
];
const SAVE_WRITE_IN_PLACE_SIG: &[u8] = &[
    0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8D, 0xAC, 0x24,
    0x30, 0xFF, 0xFF, 0xFF, 0x48, 0x81, 0xEC, 0xD0,
];

/// `FUN_1424142e0`'s real shape. The fifth argument lives at `[rsp+0x28]` on entry and at
/// `[rsp+0x20]` of the frame we build to forward it, so an `extern "system"` five-argument
/// call hands the callee the SAME qword it would have received undetoured -- which matters,
/// because it is an out-parameter the callee writes two qwords through.
#[cfg(windows)]
type SaveWriteInPlaceFn = unsafe extern "system" fn(usize, usize, usize, usize, usize) -> usize;

#[cfg(windows)]
static ORIG_SAVE_WRITE_FULL_REBUILD: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_SAVE_WRITE_IN_PLACE: AtomicUsize = AtomicUsize::new(0);

/// Entries into `FUN_142413860`, the full container rebuild.
static WRITE_FULL_REBUILD_CALLS: AtomicU64 = AtomicU64::new(0);
/// Entries into `FUN_1424142e0`, the per-block in-place patcher. Climbs ONCE PER SUPPLIED
/// BLOCK, not once per save.
static WRITE_IN_PLACE_CALLS: AtomicU64 = AtomicU64::new(0);
/// Observers bound (0..=2). Zero makes both counters above meaningless -- they can only read
/// 0, which must NOT be read as "no save was written".
static WRITE_BRANCH_OBSERVERS_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Write-branch observer entries that found no trampoline and therefore reported failure to
/// the game. Structurally unreachable (the trampoline is stored before the hook is enabled);
/// counted because on that path the write does not happen, and the only thing worse than
/// losing a save is losing it silently.
static WRITE_BRANCH_NO_TRAMPOLINE: AtomicU64 = AtomicU64::new(0);

/// Write-branch observers bound (0..=2). **Read this before either call counter**: at 0 they
/// can only report 0, and 0 from an uninstalled observer is the absence of an observation,
/// never the absence of a write.
pub fn write_branch_observers_installed() -> usize {
    WRITE_BRANCH_OBSERVERS_INSTALLED.load(Ordering::SeqCst)
}

/// Completed entries into `FUN_142413860`, the FULL CONTAINER REBUILD write branch -- one
/// whole-buffer write from offset 0, taken when a block outgrew its entry or no usable
/// container was on disk. One per save that took it.
///
/// The decompile says this branch should be rare in the steady state. That was never measured
/// before this counter existed; a non-zero value on an ordinary repeat save falsifies it.
pub fn write_full_rebuild_calls() -> u64 {
    WRITE_FULL_REBUILD_CALLS.load(Ordering::SeqCst)
}

/// Completed entries into `FUN_1424142e0`, the PER-BLOCK IN-PLACE write branch.
///
/// Climbs ONCE PER SUPPLIED BLOCK, not once per save, so this is not a save count and the two
/// branch counters are not comparable as magnitudes -- only as zero/non-zero.
pub fn write_in_place_calls() -> u64 {
    WRITE_IN_PLACE_CALLS.load(Ordering::SeqCst)
}

/// Write-branch observer entries that found no trampoline and reported failure (6) to the
/// game rather than falsely reporting success (0). Non-zero means a save was refused BY THIS
/// INSTRUMENT -- nothing was written and nothing was corrupted, but the save did not happen.
pub fn write_branch_no_trampoline() -> u64 {
    WRITE_BRANCH_NO_TRAMPOLINE.load(Ordering::SeqCst)
}

/// Observer on `FUN_142413860`, the FULL CONTAINER REBUILD write branch.
///
/// Four register arguments, no stack argument (call site `0x142410158`), so the union's shape
/// is exact and all four are forwarded verbatim. Counts AFTER the call so the counter means
/// "a rebuild ran to completion", and returns the callee's result untouched.
#[cfg(windows)]
unsafe extern "system" fn save_write_full_rebuild_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let orig = ORIG_SAVE_WRITE_FULL_REBUILD.load(Ordering::SeqCst);
    if orig == 0 {
        // Unreachable once bound. Unlike the character serializer, 0 here means SUCCESS to
        // the job body, so returning 0 would tell the game a container was rebuilt when
        // nothing was written at all. Report the writers' own failure code instead: the file
        // is untouched, which is strictly safer than the real failure this code stands for,
        // and the game already handles it.
        WRITE_BRANCH_NO_TRAMPOLINE.fetch_add(1, Ordering::SeqCst);
        log_message(format_args!(
            "suppress: BUG -- full-rebuild write observer ran with no trampoline; reporting \
             'write failed' ({SAVE_WRITE_FAILED_RESULT}) to the game. NOTHING was written"
        ));
        return SAVE_WRITE_FAILED_RESULT;
    }
    let original: UnionFn = unsafe { core::mem::transmute(orig) };
    let ret = unsafe { original(a, b, c, d) };
    WRITE_FULL_REBUILD_CALLS.fetch_add(1, Ordering::SeqCst);
    ret
}

/// Observer on `FUN_1424142e0`, the PER-BLOCK IN-PLACE write branch.
///
/// FIVE arguments -- see [`SAVE_WRITE_IN_PLACE_RVA`] for why the four-argument union shape
/// would corrupt the fifth, which is an out-parameter the callee writes through. All five are
/// forwarded verbatim and the result is returned untouched.
#[cfg(windows)]
unsafe extern "system" fn save_write_in_place_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    e: usize,
) -> usize {
    let orig = ORIG_SAVE_WRITE_IN_PLACE.load(Ordering::SeqCst);
    if orig == 0 {
        // Same reasoning as the rebuild observer: 0 would claim the block was patched.
        WRITE_BRANCH_NO_TRAMPOLINE.fetch_add(1, Ordering::SeqCst);
        log_message(format_args!(
            "suppress: BUG -- in-place write observer ran with no trampoline; reporting \
             'write failed' ({SAVE_WRITE_FAILED_RESULT}) to the game. NOTHING was written"
        ));
        return SAVE_WRITE_FAILED_RESULT;
    }
    let original: SaveWriteInPlaceFn = unsafe { core::mem::transmute(orig) };
    let ret = unsafe { original(a, b, c, d, e) };
    WRITE_IN_PLACE_CALLS.fetch_add(1, Ordering::SeqCst);
    ret
}

/// Bind the two write-branch observers. Returns how many bound (0..=2).
///
/// Each is attempted independently and neither can abort or disarm anything: losing them
/// costs this run the answer to "which write path ran" and nothing else, so every failure is
/// logged, counted out through [`write_branch_observers_installed`], and stepped over.
///
/// The two go through DIFFERENT mechanisms on purpose. `FUN_142413860` takes four register
/// arguments and nothing on the stack, so it rides the shared union like the dispatch
/// observers and a future second registrant on that address stays safe. `FUN_1424142e0` takes
/// five, and the union's four-argument shape would drop an out-parameter the callee writes
/// through -- so it gets its own `MhHook` with a correctly shaped type, exactly as the
/// quit-settle and save-job-body observers do for their own shapes.
#[cfg(windows)]
fn install_write_branch_observers() -> usize {
    let mut bound = 0_usize;

    if let Some(address) = verify(
        SAVE_WRITE_FULL_REBUILD_RVA,
        SAVE_WRITE_FULL_REBUILD_SIG,
        "SaveWriteFullRebuild",
    ) {
        match unsafe {
            register_union_hook(
                address,
                save_write_full_rebuild_hook as UnionFn,
                &ORIG_SAVE_WRITE_FULL_REBUILD,
            )
        } {
            Ok(()) => bound += 1,
            Err(status) => log_message(format_args!(
                "suppress: full-rebuild write observer union registration @0x{address:x} failed: \
                 {status:?} -- saving is unaffected, this run just cannot say whether the full \
                 rebuild ran"
            )),
        }
    }

    if let Some(address) = verify(
        SAVE_WRITE_IN_PLACE_RVA,
        SAVE_WRITE_IN_PLACE_SIG,
        "SaveWriteInPlace",
    ) {
        match unsafe { MhHook::new(address as *mut c_void, save_write_in_place_hook as *mut c_void) }
        {
            Ok(hook) => {
                ORIG_SAVE_WRITE_IN_PLACE.store(hook.trampoline() as usize, Ordering::SeqCst);
                match unsafe { hook.queue_enable() } {
                    Ok(()) => match unsafe { MH_ApplyQueued() } {
                        MH_STATUS::MH_OK => bound += 1,
                        status => {
                            ORIG_SAVE_WRITE_IN_PLACE.store(0, Ordering::SeqCst);
                            log_message(format_args!(
                                "suppress: in-place write observer MH_ApplyQueued failed: \
                                 {status:?} -- saving is unaffected, this run just cannot say \
                                 whether the in-place patcher ran"
                            ));
                        }
                    },
                    Err(status) => {
                        ORIG_SAVE_WRITE_IN_PLACE.store(0, Ordering::SeqCst);
                        log_message(format_args!(
                            "suppress: in-place write observer queue_enable failed: {status:?} -- \
                             saving is unaffected, this run just cannot say whether the in-place \
                             patcher ran"
                        ));
                    }
                }
            }
            Err(status) => log_message(format_args!(
                "suppress: in-place write observer MhHook::new @0x{address:x} failed: {status:?} \
                 -- saving is unaffected, this run just cannot say whether the in-place patcher ran"
            )),
        }
    }

    WRITE_BRANCH_OBSERVERS_INSTALLED.store(bound, Ordering::SeqCst);
    bound
}
