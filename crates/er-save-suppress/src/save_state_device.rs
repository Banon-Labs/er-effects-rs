// The `saveState` field ITSELF: reading it, and putting back the one write that kills a save.
//
// `include!`d into `lib.rs` like the blocks around it. `save_orphan_drain.rs` names the dead state
// and repairs it after the fact; `save_state_witness.rs` names the frame that creates it. This
// file owns the field those two share, and the single write that stops the state from forming.
//
// Why the repair belongs at the field and not at the caller (1.16.2 decompile, shift 0; the 1.17
// bodies at `0x140679fd0` / `0x140e6fe80` are the same code):
//
// `FUN_140e6e080`, the load-side status call, opens
//
//     if (iodev+0x18 == 0 || iodev+0x20 == 0) return 4;
//
// `+0x18` is the load's payload pointer, so it is 0 for the entire life of a save. Its wrapper
// `FUN_140679180` then writes `saveState = 0` for any answer that is not 0 or 1. So one call to
// that wrapper while a save owns the device takes the mutex off the save having released nothing,
// and every later submit is refused by `FUN_140e6ef60`'s `iodev+0x10 == 0 && iodev+0x20 == 0`
// precondition -- for the rest of the process, because nothing polls a save the game no longer
// believes is in flight. That is the shape of the 2026-08-31 wedge, in which 8629 of 8638
// dispatches declined; which write produced it there is still open (see the end of this block).
//
// `CS::MoveMapStep::DoSaveStuff` never does this. It enters the wrapper only under
// `GameMan::IsSaveState2()`, i.e. only for a load it is actually waiting on. Four other callers
// have no such guard, and Ghidra's xref set for the wrapper is exactly these five on both images
// (the sixth reference is a `.pdata` unwind record, not a call), so the list is complete rather
// than merely the ones someone happened to find:
//
//   1.16.2 call   1.17 call    containing function                       guarded on `saveState`?
//   0x82a787      0x82b777     MenuJob step, 0x60000 buffer,             no
//                              `GameSettings::LoadDefault` -- game settings
//   0x82ab46      0x82bb36     MenuJob step, 0x240010 buffer,            no
//                              `FUN_140257340` -- system data
//   0x82c286      0x82d276     MenuJob step, 0x280000 buffer,            no
//                              `SetSaveSlot` + `ProfileSummary` -- character slot
//   0xaf1a46      0xaf2d56     `CS::MoveMapListStep::STEP_LoadSaveData_Wait`  no
//   0xafbc2b      0xafcf4b     `CS::MoveMapStep::DoSaveStuff`            yes (`IsSaveState2`)
//
// The call offset inside each function is identical on both builds (0x37, 0x26, 0x46, 0x16,
// 0x15b), which is an independent check that the five pairs are the same code. When the witness
// reports `oracle_save_state_first_caller_rva`, that hex is the return address -- the call above
// plus 5 -- so subtract 5 and read the row.
//
// In vanilla none of the four unguarded callers can meet a save, because each of them runs only
// while the load it submitted owns the device (`saveState == 2`), and a save cannot submit at
// `saveState == 2`. The product's own reload removes that coincidence: it submits and consumes the
// save-data read itself before `continue_confirm`, so the native world-load transition can reach a
// poll with no load outstanding and `saveState` back at 0, which is long enough for an autosave to
// take the device and meet an unguarded poll.
//
// What this does not claim. The 2026-08-31 wedge is not known to have come through this door. That
// run hooked `FUN_140679180` from boot (`b80_poll_679180` in `er-quickload-continue-trace.log`,
// unthrottled) and recorded exactly seven calls, the last of them the product's own reload drain,
// before the autosave that latched -- so on that run the wrapper was never called again and cannot
// be the writer. The repair below is correct where it applies and inert otherwise; it closes one
// of the doors, and the run's own trace says it was not the door used. See bd
// `the-2026-08-31-wedge-was-not-the-load-poll-two-of-three-strand-writes-ruled-out-2026-08-31`.
//
// What this repair does, and what it refuses to do. It puts `saveState` back to 1 -- the value the
// game itself had one instruction earlier -- when a poll left it elsewhere with the save still on
// the device. It does not change the wrapper's return value, so every caller behaves exactly as it
// does today; it does not poll, enqueue or release anything; and it hands the save back to the
// game's own pump (`DoSaveStuff` under `IsSaveState1` -> `FUN_140679510` -> `FUN_140e6e430`),
// which is the code that owns finishing it. That is deliberately not a private pump: the repair is
// one dword, and the native owner does the work.
//
// The failure mode if the pump never arrives is `saveState == 1` with a save on the device -- which
// is indistinguishable from an ordinary in-flight save, keeps `FUN_14067b940`'s `saveState == 0`
// guard closed so the request stays latched and retries, and is recoverable the moment any pump
// runs. The state it replaces is unrecoverable.

/// Read `GameMan+0xb80` (`saveState`), or `None` when it is not reachable.
#[cfg(windows)]
fn read_save_state() -> Option<u32> {
    let ptr = save_state_ptr()?;
    // SAFETY: `save_state_ptr` returned it, so the address was proven readable by the same
    // `safe_read_usize` that resolved the singleton.
    unsafe { er_game_base::mem::safe_read_usize(ptr as usize) }.map(|raw| (raw & 0xffff_ffff) as u32)
}

/// `GameMan+0xb80` as a pointer, or `None` when `GameMan` is not resolvable yet.
///
/// The singleton read goes through `safe_read_usize`, so a `Some` here means the game has a live
/// `GameMan` and the field address is inside it -- the same proof `read_save_state` used to do
/// inline, kept in one place now that a writer shares it.
#[cfg(windows)]
fn save_state_ptr() -> Option<*mut u32> {
    use er_game_base::{mem::safe_read_usize, rva::GAME_MAN_SINGLETON_RVA};

    /// `GameMan::saveState`. The Ghidra type name and "the same `+0xb80` store in every 1.17
    /// wrapper body" were this constant's whole provenance, which is a type declaration plus an
    /// observation with no address attached. Both are now backed by one: `CS::GameMan::GameMan`
    /// (1.16.2 0x140675ea0, 1.17 0x140676cf0) aligns 1296/1296 instructions with 142 field
    /// offsets and zero moved, and writes `mov %r14d,0xb80(%rsi)` at 0x14067616f (1.17
    /// 0x140676fbf). Frozen in `scripts/check-object-field-offsets-1170.py`.
    ///
    /// A third witness settles the name, which three other crates had backwards until 2026-08-31
    /// (`LOAD_IN_PROGRESS` / `LOAD_PHASE` / `LOAD_FSM`): the game spells it itself, in
    /// `IsSaveState1` (0x14067a010) and `IsSaveState2` (0x140679ff0), each a leaf that loads the
    /// GameMan singleton and does `cmp dword ptr [rax+0xb80],N`. That matters here more than
    /// anywhere else in the tree -- the wedge diagnosis this file exists for turns on the save
    /// lane owning this one slot, and a field named after the load lane makes the ownership story
    /// re-derive backwards. A complete scan of all 37 accesses across the 288 GameMan-referencing
    /// functions confirms both lanes stamp it: save/preview 1, load 2, resident 3.
    const GAME_MAN_SAVE_STATE_B80_OFFSET: usize = 0xb80;

    let base = er_game_base::mem::game_module_base().ok()?;
    let game_man = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GAME_MAN_SINGLETON_RVA,
            "GAME_MAN_SINGLETON_RVA",
        ))
    }?;
    if game_man < 0x10000 {
        return None;
    }
    // Prove the field itself is mapped before handing out a pointer that will be written.
    unsafe { safe_read_usize(game_man + GAME_MAN_SAVE_STATE_B80_OFFSET) }?;
    Some((game_man + GAME_MAN_SAVE_STATE_B80_OFFSET) as *mut u32)
}

/// Times the load poll's `saveState = 0` was put back to 1 over a save the device still held.
/// **Non-zero means the wedge happened and was intercepted**, which is a different verdict from
/// `oracle_save_state_abandoning_writes` alone (that counts the write; this counts the repair).
static SAVE_STATE_OWNER_RESTORES: AtomicU64 = AtomicU64::new(0);
/// Times the repair was wanted but `GameMan` could not be addressed, so the save stayed dead.
/// A loud failure, never a pass: it means a wedge went through unrepaired.
static SAVE_STATE_OWNER_RESTORE_FAILURES: AtomicU64 = AtomicU64::new(0);

/// Put `saveState` back to `SAVE_OWNS` after a poll took it off a save the device still holds.
///
/// Returns whether the write landed. The caller has already established, through
/// [`poll_abandoned_a_save`], that a save was in flight before the call, that `saveState` left 1,
/// and that `iodev+0x10`/`+0x20` are still populated -- so there is no state here to re-derive and
/// nothing to decide: the value being restored is the one the game held one instruction earlier.
#[cfg(windows)]
fn restore_save_state_owner() -> bool {
    let Some(ptr) = save_state_ptr() else {
        SAVE_STATE_OWNER_RESTORE_FAILURES.fetch_add(1, Ordering::SeqCst);
        return false;
    };
    // SAFETY: `save_state_ptr` proved `GameMan` resolves and the field is mapped. The write is a
    // single aligned dword to the field the game writes from this same thread (every caller of the
    // wrapper is a game-thread step or MenuJob body), so it cannot tear against a concurrent
    // native write.
    unsafe { ptr.write(GAME_MAN_SAVE_STATE_SAVE_OWNS) };
    SAVE_STATE_OWNER_RESTORES.fetch_add(1, Ordering::SeqCst);
    true
}

/// Repairs performed. Non-zero means a save would have been stranded and was not.
pub fn save_state_owner_restores() -> u64 {
    SAVE_STATE_OWNER_RESTORES.load(Ordering::SeqCst)
}

/// Repairs that could not be performed because `GameMan` was unaddressable. Non-zero means a save
/// was stranded despite the repair being wired.
pub fn save_state_owner_restore_failures() -> u64 {
    SAVE_STATE_OWNER_RESTORE_FAILURES.load(Ordering::SeqCst)
}

// Why the dispatch decline needs this field too, and what the instrument said without it.
//
// `FUN_14067b940` refuses on `if (GameMan->saveState != 0) return 0;` before it ever reaches
// `FUN_140e6ef60`, so on that path the device fields are bystanders. `classify_sl_bail` reads only
// the device, so it reported `save-content-and-job-latched-0x10+0x20` for both refusals and a
// reader could not tell "the device is stuck" from "a save is legitimately in flight". Sampling
// `saveState` at the same instant separates them, and the pair is what a run has to publish.

/// Did the lane refuse before it ever reached the submit builder?
///
/// `FUN_14067b940` opens `if (GameMan->saveState != 0) return 0;`, so on that path it never calls
/// `FUN_140e6ef60` and the device fields had nothing to do with the refusal. The device sample is
/// still worth recording -- it says what the device held at that moment -- but reading it as the
/// cause is a misattribution, and this is the clause that separates the two.
///
/// Pure and total for the same reason as [`classify_sl_bail`]: an instrument that names a culprit
/// has to be checkable without a game attached. An unreadable `saveState` is never reported as the
/// mutex, because "unknown" is not evidence.
pub fn dispatch_refusal_is_the_mutex(save_state: Option<u32>) -> bool {
    matches!(save_state, Some(state) if state != GAME_MAN_SAVE_STATE_IDLE)
}

/// `GameMan.saveState` at that same decline (`u32::MAX` = unsampled/unreadable). Without it the
/// device sample is ambiguous: `FUN_14067b940` refuses on a non-idle `saveState` before reaching
/// the builder, so a latched-looking device can be a bystander rather than the cause.
static DECLINE_SAVE_STATE: AtomicU32 = AtomicU32::new(u32::MAX);
/// `GameMan.saveState` at the most recent dispatch decline (`u32::MAX` = unsampled). Non-idle means
/// the lane never reached the submit builder, so [`decline_bail_reason`] below describes a
/// bystander rather than the cause.
pub fn decline_save_state() -> u32 {
    DECLINE_SAVE_STATE.load(Ordering::SeqCst)
}

// The two rules this file adds are decisions about which native branch is in play, not readings of
// a runtime value, so both are exercised with no game attached -- the same reason
// `poll_abandoned_a_save` and `classify_sl_bail` are.
#[cfg(test)]
mod save_state_device_tests {
    use super::*;

    /// A latched-looking device is only evidence when the lane actually reached the builder.
    #[test]
    fn a_non_idle_save_state_means_the_builder_was_never_reached() {
        // The wedge: the mutex is free, so `FUN_14067b940` fell through to `FUN_140e6ef60` and the
        // device fields are what refused the save.
        assert!(!dispatch_refusal_is_the_mutex(Some(
            GAME_MAN_SAVE_STATE_IDLE
        )));
        // An ordinary in-flight save. The lane returned at its first line; reporting the device
        // sample as the cause here is the misattribution this clause exists to prevent.
        assert!(dispatch_refusal_is_the_mutex(Some(
            GAME_MAN_SAVE_STATE_SAVE_OWNS
        )));
        // A load owns the device: also a refusal at the first line, for a different reason.
        assert!(dispatch_refusal_is_the_mutex(Some(
            GAME_MAN_SAVE_STATE_LOAD_OWNS
        )));
        assert!(dispatch_refusal_is_the_mutex(Some(
            GAME_MAN_SAVE_STATE_LOAD_RESIDENT
        )));
        // Unknown is never evidence, in either direction.
        assert!(!dispatch_refusal_is_the_mutex(None));
    }
}
