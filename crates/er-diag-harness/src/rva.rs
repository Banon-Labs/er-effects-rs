//! The addresses and log budgets the three traces own, moved verbatim out of the product's
//! `crates/er-quickload/src/constants/autoload_state.rs`.
//!
//! They moved with the code rather than being copied: one game address must have exactly one
//! literal declaration (`scripts/check-rva-alias-drift.py`), because divergent names for one
//! address are divergent claims about what it is. `DLC_ROOTS_REFILL_RVA` is the exception --
//! `er-title-flow`'s DLC-root self-heal also names it, so it already lives in the shared
//! `er_game_base::rva` table and is referenced from there by both.

/// `MsbFileCap` load-complete callback -- The sole writer of `msbResCap` (`cap+0x90`), 1.16.2 dump
/// `FUN_14021bbf0`. Byte-verified against `eldenring-deobf.bin` at the same VA (shift 0 on 1.16.2):
/// `48 8b c4 56 57 41 56 48 81 ec 80 00 00 00`, with the first rip-relative operand only at +0x1e,
/// so the prologue is safely detourable.
///
/// It writes `msbResCap` only when the cap's content is non-null, and returns normally otherwise --
/// leaving `(loadState=4, msbResCap=0)`, which wedges `WorldBlockRes` case 2 forever. Tracing it
/// separates "fired with null content" (empty read) from "never fired" (cache hit, no enqueue); no
/// passive read can, because both end in identical cap state.
pub(crate) const MSB_FILECAP_PARSE_CALLBACK_RVA: usize =
    er_game_base::rva::MSB_FILECAP_PARSE_CALLBACK_RVA;

/// How many successful parses to log before rate-limiting. Null-result parses are always logged.
pub(crate) const MSB_PARSE_TRACE_VERBOSE_CALLS: usize = 24;

/// How many NULL-result parses also carry a DLIO virtual-root dump. The null path fires ~13x/second
/// during the stall and the root walk is a vector scan, so only the first few need it -- the roots
/// do not change once the block is wedged, and the load-1 baseline comes from the verbose successes.
pub(crate) const MSB_PARSE_TRACE_ROOTS_ON_NULL_RESULTS: usize = 4;

/// `CS::MoveMapListStep::STEP_LoadListWait` -- the only live path that refills the DLC virtual roots
/// (it calls `FUN_140e05fb0(GLOBAL_CSDlc, true)` -> `CSDlcImp::AddVirtualFileRoots`). Proven to be
/// the fix site by bd `PROVEN-reload-softlock-is-blanked-dlc-virtual-root-mapstudio-dlc2-empty`.
///
/// Prologue is `40 53 48 83 ec 20 48 8b 81 c0 02 00 00` (`push rbx; sub rsp,0x20; mov rax,[rcx+0x2c0]`)
/// -- no rip-relative operand anywhere near the patch site, and the deobf bytes match the 1.16.2 dump
/// exactly, so a 5-byte detour relocates cleanly. `rcx` is the `MoveMapListStep` this-pointer.
pub(crate) const STEP_LOADLIST_WAIT_RVA: usize = er_game_base::rva::STEP_LOADLIST_WAIT_RVA;

/// Gate A operand: `MoveMapListStep::loadList`. The step proceeds when this is NULL **or** the int at
/// `*loadList` is 2 or 3 (`sub eax,2; cmp eax,1; ja bail`).
pub(crate) const MOVEMAPLISTSTEP_LOADLIST_2C0_OFFSET: usize = 0x2c0;

/// Gate B operand: must be 0 for the step to proceed (`cmp qword [rcx+0xb8],0; jnz bail`).
pub(crate) const MOVEMAPLISTSTEP_GATE_B8_OFFSET: usize = 0xb8;

/// `STEP_LoadListWait` runs every frame, so the trace logs only on verdict change plus this many
/// opening entries -- enough to capture the load-1 baseline without burying the reload.
pub(crate) const LOADLIST_WAIT_TRACE_VERBOSE_CALLS: usize = 6;

/// `FUN_140e06490(CSDlcImp*, bool)` -- Blanks the 13 `*_dlc2` virtual roots to `L""` and clears the
/// DLC ownership flags. Sole code caller is the title start-game flow `FUN_1409b24e0`.
pub(crate) const DLC_ROOTS_BLANK_RVA: usize = er_game_base::rva::DLC_ROOTS_BLANK_RVA;

/// `FUN_140e05fb0(CSDlcImp*, bool)` -- the REFILL: re-queries Steam DLC ownership and calls
/// `CSDlcImp::AddVirtualFileRoots`. Hooked at this shared entry rather than at either caller,
/// because a measured run showed `STEP_LoadListWait` never executes at all.
pub(crate) const DLC_ROOTS_REFILL_RVA: usize = er_game_base::rva::DLC_ROOTS_REFILL_RVA;

/// `FUN_140836f30` -- the `Do` of the MenuFunctorJob that eventually reaches the refill (vtable
/// 0x142acb638). One level above `FUN_140e05fb0`, so it separates "job never enqueued" from "job ran
/// and diverged inside". Prologue `48 89 54 24 10 53 48 83 ec 30`, no rip-relative in the window.
pub(crate) const DLC_ROOTS_JOB_RVA: usize = er_game_base::rva::DLC_ROOTS_JOB_RVA;

/// Smallest address treated as a plausible heap/image pointer. Matches the product's own
/// `TITLE_OWNER_SCAN_START_ADDRESS` guard value; anything at or below it is a null, a tagged
/// sentinel, or a small integer that landed in a pointer field.
pub(crate) const PTR_SANITY_MIN: usize = 0x10000;

/// `MhHook` trampoline slot sentinel: 0 = the detour is not installed, so forward nowhere.
pub(crate) const HOOK_ORIGINAL_UNSET: usize = 0;
