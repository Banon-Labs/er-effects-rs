//! The load-balancer frame step, guarded against a torn-down `SoloParamRepository`.
//!
//! # The crash
//!
//! A tester's Elden Ring died the same way twice, a day apart (2026-08-22 18:27Z and
//! 2026-08-23 04:35Z), on a profile carrying only this DLL and the crash logger. The fatal record
//! is a NULL write at `eldenring.exe+0x1eb9999` -- but that address is inside `DLPanic`'s body,
//! not inside the code with the bug. The access violation is the panic's death rattle.
//!
//! What actually happened is an assertion the game raises on itself:
//!
//! ```text
//! DLPanic("w:\\gr\\patch116\\...\\Core/Singleton/FD4Singleton.h", 0xb4,
//!         "%s:<uninitialised singleton accessed>", GetRuntimeClassName(0x143d5ae58))
//! ```
//!
//! Proven rather than inferred: the crash log's frame `#8` is `0x140d3d669`, which is exactly the
//! return address of the `CALL 0x141eb97a0` (`DLPanic`) at `0x140d3d664`, and the only path that
//! reaches that call is the `MOV RCX,[0x143d81ee8]; TEST RCX,RCX; JNZ` immediately above it. The
//! logged `stack_raw[0]` still held `eldenring.exe+0x3292b38` -- the message operand from the
//! `LEA R8` two instructions earlier.
//!
//! # Why it fires
//!
//! The full chain is `WinMain -> MainLoop -> CleanupUpdate -> FUN_140e0cca0 -> FUN_140d3cad0 ->`
//! this function. `FUN_140e0cca0` is the per-frame load-balancer refresh, and it is gated on
//! `GLOBAL_GXRenderingSystem->field_0x13a68 + 0x34`. So the rendering system is still alive and
//! still asking for `LoadBalancerParam` on a frame where the param repository has already been
//! destroyed: a teardown-ordering race inside the game, on the main thread, reachable by anyone.
//!
//! # Why the early return is the right answer
//!
//! This guard does not invent a value; it takes the branch the function already has. When
//! `GetParamResCap` returns null, or when the binary search misses, control falls to
//! `0x140d3d755`:
//!
//! ```text
//! 140d3d755: MOV dword ptr [RSI + 0x8], EBP    ; out->tag   = param_2
//! 140d3d758: MOV qword ptr [RSI], RBX          ; out->value = 0   (EBX was XORed at 0x140d3d634)
//! ```
//!
//! with `RSI` holding the out pointer (`rcx` on entry) and `EBP` holding `param_2` (`edx` on
//! entry, saved at `0x140d3d613`). The stub writes those two fields and returns, which is
//! byte-for-byte the "no row for this platform" outcome the caller is already written against --
//! the same rule the sibling guards follow.
//!
//! # Coverage
//!
//! This is a group of one. It does not pair with the `null_special_effect` guards and arming it
//! alone is a complete, safe state.

use core::sync::atomic::{AtomicU64, AtomicUsize};

use super::Guard;

/// `FUN_140d3d5f0(out, platformKind)` -- resolves the `LoadBalancerParam` row for a platform and
/// writes `{ row, platformKind }` into `out`. Named for what it does rather than by its address,
/// because the address is the thing most likely to change.
pub(crate) const LOAD_BALANCER_PARAM_RVA: usize = 0xd3_d5f0;

/// `GLOBAL_SoloParamRepository` -- the singleton slot this function dereferences at `0x140d3d636`
/// before it may touch any param. This is the pointer that was null.
///
/// Declared once in `er-game-base::rva`, because the product DLL guards the sibling
/// `LookupMenuOffscrRendParam` against the same slot; re-exported here so this module's own stub
/// and its address test keep the name they use.
///
/// The slot's absolute address is resolved at install time into [`SOLO_PARAM_REPOSITORY_SLOT`],
/// because a naked stub cannot compute the game's base.
pub(crate) use er_game_base::rva::SOLO_PARAM_REPOSITORY_GLOBAL_RVA;

/// Absolute address of the singleton slot, published by [`publish_repository_slot`] before the
/// hook is enabled. Zero means unresolved, and the stub treats that as "assume the singleton is
/// live" so a failure to resolve degrades to plain pass-through rather than to a silent guard that
/// swallows every call.
pub(crate) static SOLO_PARAM_REPOSITORY_SLOT: AtomicUsize = AtomicUsize::new(0);

pub(crate) static ORIG_LOAD_BALANCER_PARAM: AtomicUsize = AtomicUsize::new(super::ORIGINAL_UNSET);
pub(crate) static BLOCKED_LOAD_BALANCER_PARAM: AtomicU64 = AtomicU64::new(0);

// The expected prologue, assembled from named `iced-x86` instructions by this crate's `build.rs`
// and verified there against `eldenring-deobf.bin` when a copy is present.
include!(concat!(
    env!("OUT_DIR"),
    "/generated_null_param_repository_prologues.rs"
));

/// Resolve the singleton slot for this process. Called before the hook is enabled.
pub(crate) fn publish_repository_slot(base: usize) {
    SOLO_PARAM_REPOSITORY_SLOT.store(
        er_game_base::mem::game_data_addr(
            base,
            SOLO_PARAM_REPOSITORY_GLOBAL_RVA,
            "SOLO_PARAM_REPOSITORY_GLOBAL_RVA",
        ),
        core::sync::atomic::Ordering::SeqCst,
    );
}

/// Null `SoloParamRepository` -> write the function's own "no row" result and return.
///
/// # Safety
///
/// Installed as a MinHook detour on [`LOAD_BALANCER_PARAM_RVA`]; never called from Rust. Entering
/// it before `ORIG_LOAD_BALANCER_PARAM` holds a trampoline would jump to zero, which is why
/// installation publishes that slot before enabling the hook.
#[cfg(windows)]
#[unsafe(naked)]
pub(crate) unsafe extern "system" fn load_balancer_param_guard() {
    core::arch::naked_asm!(
        // `r11` is volatile under Win64 and is not an argument register, so the pass-through path
        // reaches the original with every register the caller set -- `rcx`, `rdx`, the lot --
        // exactly as it arrived. Only the flags differ, and Win64 does not preserve those.
        "mov r11, qword ptr [rip + {slot}]",
        "test r11, r11",
        // Unresolved slot: fall through to the original rather than guard on a guess.
        "jz 2f",
        "mov r11, qword ptr [r11]",
        "test r11, r11",
        "jnz 2f",
        // Atomic so the count stays exact when several threads miss at once, and memory-only so
        // nothing else is disturbed on the way to the return.
        "lock inc qword ptr [rip + {blocked}]",
        // The original's own miss path at 0x140d3d755, in its order: tag first, then value.
        "mov dword ptr [rcx + 8], edx",
        "mov qword ptr [rcx], 0",
        "ret",
        "2:",
        "jmp qword ptr [rip + {original}]",
        slot = sym SOLO_PARAM_REPOSITORY_SLOT,
        blocked = sym BLOCKED_LOAD_BALANCER_PARAM,
        original = sym ORIG_LOAD_BALANCER_PARAM,
    )
}

// The host build has no game to hook; the stub exists only so the registry still names a function
// pointer and the crate stays checkable off-target.
#[cfg(not(windows))]
pub(crate) unsafe extern "system" fn load_balancer_param_guard() {}

pub(crate) const LOAD_BALANCER_PARAM_GUARD: Guard = Guard {
    name: "LoadBalancerParam lookup",
    group: "null_param_repository",
    rva: LOAD_BALANCER_PARAM_RVA,
    expected_prologue: LOAD_BALANCER_PARAM_PROLOGUE,
    detour: load_balancer_param_guard,
    original: &ORIG_LOAD_BALANCER_PARAM,
    blocked: &BLOCKED_LOAD_BALANCER_PARAM,
    prepare: Some(publish_repository_slot),
    rationale: "null SoloParamRepository -> write the function's own no-row result ({value:0, \
                tag:param_2}, its branch at 0x140d3d755) instead of letting FD4Singleton.h:180 \
                DLPanic and take the process with it",
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Both addresses are the whole safety argument. `LOAD_BALANCER_PARAM_RVA` is where the hook
    /// goes; `SOLO_PARAM_REPOSITORY_GLOBAL_RVA` is the pointer the stub tests, and getting it
    /// wrong would mean guarding on an unrelated qword. The prologue bytes are pinned where they
    /// are produced, in `build.rs`, so repeating them here would only be a transcription to keep
    /// in step.
    #[test]
    fn rvas_match_er_1162_static_re() {
        assert_eq!(LOAD_BALANCER_PARAM_RVA, 0xd3_d5f0);
        assert_eq!(SOLO_PARAM_REPOSITORY_GLOBAL_RVA, 0x3d8_1ee8);
    }

    /// MinHook relocates the first five bytes of the entry, so the signature must cover at least
    /// the window the patch actually overwrites.
    #[test]
    fn prologue_signature_covers_the_patch_window() {
        const MINHOOK_PATCH_BYTES: usize = 5;
        assert!(LOAD_BALANCER_PARAM_PROLOGUE.len() >= MINHOOK_PATCH_BYTES);
    }

    /// The stub reads the singleton slot on its first call, so a guard that arms without a
    /// resolved address would test a null slot forever. The row must carry the resolver.
    #[test]
    fn the_guard_resolves_its_global_before_arming() {
        assert!(
            LOAD_BALANCER_PARAM_GUARD.prepare.is_some(),
            "the stub cannot compute the game base itself"
        );
        publish_repository_slot(0x1_4000_0000);
        assert_eq!(
            SOLO_PARAM_REPOSITORY_SLOT.load(core::sync::atomic::Ordering::SeqCst),
            0x1_4000_0000 + SOLO_PARAM_REPOSITORY_GLOBAL_RVA,
        );
    }

    /// This guard stands alone: it has nothing to do with the paired `null_special_effect` guards,
    /// and arming it by itself is a complete state. If it were ever put in their group, a run with
    /// only this one armed would be reported as UNGUARDED, which would be false.
    #[test]
    fn the_guard_is_its_own_group() {
        assert_eq!(LOAD_BALANCER_PARAM_GUARD.group, "null_param_repository");
        assert!(
            super::super::REGISTRY
                .iter()
                .filter(|guard| guard.group == LOAD_BALANCER_PARAM_GUARD.group)
                .count()
                == 1
        );
    }
}
