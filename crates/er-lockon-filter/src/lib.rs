//! Standalone lock-on filter: while you are invading, other invaders are not lock-on targets.
//!
//! # The problem
//!
//! Two invaders in one world are both targetable by each other. Elden Ring's lock-on candidate
//! walk admits anyone the team-relation table calls a rival, and two reds are rivals, so the
//! target you get when you press lock-on -- and every target the left/right switch steps
//! through -- includes the other invader standing next to the host. That is the wrong half of
//! the fight.
//!
//! # Where the decision is made
//!
//! `FUN_140716260` is `CS::LockTgtMan`'s per-frame update (1.16.2; `0x1407170b0` on 1.17). It
//! walks the act-point list at `GLOBAL_ActPntMan + 0x10` and admits each point on exactly four
//! tests, in this order:
//!
//! ```text
//! point+0x75 & 1                          the point is enabled
//! owner = PointOwner(point)               and has a character behind it
//! CS::ChrIns::CanTargetTeamType(me, owner)  which my team may target
//! owner != me                             and is not me
//! ```
//!
//! then a distance test, then the candidate's own `disableLockOnAng` cone, and only then does it
//! insert the point into the candidate set. `PointOwner` is the second test, and it is this
//! crate's seam.
//!
//! # What this detours, and why that one
//!
//! `PointOwner` (`FUN_140713db0` at 1.16.2 `0x140713db0`, 1.17 `0x140714c00`) is the whole
//! function:
//!
//! ```text
//! if ((point->fieldInsHandle & 0xf0000000) != 0x10000000) return nullptr;
//! return CS::WorldChrManImp::GetChrInsFromHandle(GLOBAL_WorldChrMan, &point->fieldInsHandle);
//! ```
//!
//! So `nullptr` is already its own vocabulary for "no character owns this point", every one of
//! its eleven call sites was written against that answer, and returning it for a fellow invader
//! costs the engine nothing it does not already handle. All eleven callers are lock-on code:
//! five inside the update above, two building the `NetSyncData` for the target you currently
//! hold, and two `LockTgtMan` accessors that read flags off the owner. Nothing else in the image
//! calls it -- which is what makes a detour here a statement about targeting alone rather than
//! about the character.
//!
//! The alternatives were both worse. `CanTargetTeamType` is the test that actually rejects a
//! candidate, but it has 21 call sites across ai targeting and damage, so a detour there decides
//! far more than lock-on. Patching the team-relation matrix cell for two invaders to `Friend`
//! reaches the same 21 call sites through the data instead of the code, and would stop the two
//! invaders damaging each other as well.
//!
//! # Both builds
//!
//! The prologue this DLL byte-checks is the same 13 bytes in 1.16.2 and 1.17, and the pair is in
//! `docs/recon/rva-map-1162-to-1170.verified.tsv` as `IDENTICAL-WHOLE` over the whole declared
//! body, so `er-hook` translates the address rather than refusing it.
//!
//! # No switch
//!
//! Loading the DLL is the feature. There is no hotkey, no toggle and no config file: the rule is
//! [`rules::hides`] over two constant lists, and the only state the crate keeps is the trampoline,
//! the addresses it resolved at install, and the counters its log lines are made of. A profile
//! that lists this DLL wants invaders unlockable while invading; a profile that does not, does
//! not load it.
//!
//! # What it deliberately does not do
//!
//! It writes no game memory, patches no param, and touches nothing about the other invader
//! except the answer one targeting query gets. They still take your damage and you still take
//! theirs, their nameplate is unchanged, and a lock you were already holding when you became an
//! invader is not stolen away -- the next candidate walk simply stops offering them.
//!
//! It does reach one thing beyond the candidate list, and that is the same statement rather than
//! a side effect: `CSChrAutoHomingModule` is fed from inside the admitted-candidate block above,
//! so a point with no owner is not an auto-homing target either. A fellow invader stops bending
//! your swings towards them for the same reason they stop appearing under your reticle.

#![cfg_attr(not(windows), allow(dead_code))]

mod rules;

#[cfg(windows)]
mod runtime;

/// `DllMain` success.
const DLL_MAIN_SUCCESS: i32 = 1;

/// `ChrIns *PointOwner(ActPnt *point)` -- the lock-on system's point-to-character resolver.
///
/// Named for what it answers rather than for its Ghidra symbol, which is `FUN_140713db0`: the
/// 1.16.2 dump has no name for it, and the 1.17 dump has no names at all.
const LOCK_ON_POINT_OWNER_RVA: usize = 0x0071_3db0;

/// `CS::ChrIns::chr_type`, from the two-instruction getter `CS::ChrIns::GetCharacterType`
/// (1.16.2 `0x1403eec10`, 1.17 `0x1403eee40`), which is `mov eax,[rcx+0x68]` in both images.
const CHR_INS_CHR_TYPE_OFFSET: usize = 0x68;

/// `CS::GameMan::summonParamType` -- the multiplayer role this session was matched on.
///
/// From `CS::GameMan::GetSummonParamType`, which is a null check and one load: `mov rax,
/// [GLOBAL_GameMan]; test rax,rax; jz; mov eax,[rax+0xd84]; ret`. The whole body is byte-identical
/// in both images bar the singleton's own displacement -- 1.16.2 `0x140679300` reading
/// `0x143d65f88`-era `GLOBAL_GameMan`, 1.17 `0x14067a150` reading `0x143d6d988`, which is the
/// destination the data map already records for `GAME_MAN_SINGLETON_RVA`.
const GAME_MAN_SUMMON_PARAM_TYPE_OFFSET: usize = 0xd84;

/// Log file, beside the game executable.
const LOG_FILE_NAME: &str = "er-lockon-filter.log";

include!(concat!(env!("OUT_DIR"), "/generated_prologues.rs"));

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    module: *mut core::ffi::c_void,
    reason: u32,
    reserved: *mut core::ffi::c_void,
) -> i32 {
    unsafe { runtime::dll_main(module, reason, reserved) }
}

/// Keeps the crate linkable on the host, where there is no game to filter and the runtime module
/// is compiled out. The parser and the rules above are what `cargo test` runs here.
#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_lockon_filter_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
