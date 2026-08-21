//! Guard: a `CS::ChrIns` whose `SpecialEffect` container is null.
//!
//! # The crash
//!
//! Captured live 2026-08-15, ~46 minutes into a Seamless Co-op session:
//!
//! ```text
//! exception_address        = 0x1404f9940 {eldenring.exe+0x4f9940}
//! exception_access_address = 0x8
//! context_rcx              = 0x0      <- the SpecialEffect container
//! context_rdx              = 0x578    <- SpEffect id 1400
//! ```
//!
//! `CS::SpecialEffect::HasSpecialEffectId` opens with `MOV RCX,[RCX+8]` -- it null-checks the
//! *list* on every iteration but never the container. It is correct for its contract; a caller
//! passed null.
//!
//! # Who passed it
//!
//! The fault is on the function's *first* instruction, so nothing had been pushed and `[rsp]` is
//! still the return address. The crash log's `stack_raw[0]` is `0x1404b878d`, the instruction after
//! `CALL 0x1403f1f80` inside `CS::SummonBuddyManager::Update` (`0x1404b81b0`).
//!
//! `0x1403f1f80` is `CS::ChrIns::HasSpecialEffectId`, which is two instructions long:
//!
//! ```text
//! MOV RCX, [RCX + 0x178]   ; chr->specialEffect
//! JMP 0x1404f9940          ; TAIL jump -- so the return address belongs to Update, not to this
//! ```
//!
//! The faulting address is `0x8`, not `0x178`, which proves the `ChrIns` itself was a valid pointer
//! and `chr->specialEffect` was the null. (`ersc.dll+0x92142` appears in the log's `stack_modules`,
//! but that list is a stack *scan*, not an unwind; that address is the instruction after a
//! `__security_check_cookie` call and is not a real frame here.)
//!
//! # Why only Seamless reaches it
//!
//! The crashing loop sits under `if (IsSpiritSummmonNetworkingEnabled())` (`0x1404b71f0`), which
//! returns true only when the main player's `CharacterType` is `Arena` *and*
//! `QuickmatchManager::SpiritAshesAllowed`. Vanilla open-world play never enters the branch. A
//! Seamless session does, during ordinary co-op, where summon `ChrIns` objects are constructed and
//! torn down per peer -- so the list can hold one whose `SpecialEffect` container does not exist
//! yet. The loop is:
//!
//! ```text
//! for each summon -> GetBuddyParam -> if hostile -> for i in 0..5:
//!     id = BuddyParam::GetArenaHostileSpEffect(row, i)
//!     if id >= 0 && !ChrIns::HasSpecialEffectId(chr, id) { ChrIns::ApplySpEffect(chr, id, true) }
//! ```
//!
//! SpEffect 1400 came out of that param row, which is why no call site in the whole image loads
//! `0x578` as an immediate.
//!
//! # Why this guard has two halves
//!
//! Guarding only the query is a trap. `false` makes `!has` pass, and the loop calls
//! `ChrIns::ApplySpEffect` on the very same `ChrIns`:
//!
//! ```text
//! ChrIns::ApplySpEffect 0x1403e8be0 -> FUN_1403e8c90 -> FUN_1403fade0
//!   -> 0x1403fae2a  MOV RCX,[RBX+0x178]        (loads the same null container)
//!   -> SpecialEffect::Apply 0x1404fa8e0 -> CheckApplyConditions 0x1404fc4e0 -> FUN_1404fc690
//!   -> 0x1404fc6ae  MOV RAX,[RCX+0x8]          (the identical unguarded read)
//! ```
//!
//! That is the same access violation at the same offset, just at a different RVA. So the query and
//! the apply are guarded together, and each returns the value the original already returns when it
//! has nothing to report:
//!
//! * `HasSpecialEffectId` -> `false`, which is its own `XOR AL,AL; RET` at `0x1404f995e`.
//! * `Apply` -> `-1`, which is its own `OR RAX,-1; RET` at `0x1404fa9df`. `FUN_1403fade0` already
//!   tests that with `JS` and returns false, so the chain unwinds the way the game expects.
//!
//! Neither value is invented and neither is a lie: a `ChrIns` with no `SpecialEffect` container has
//! no effects and can receive none. Together they turn the crashing frame into a skip. Because
//! `SummonBuddyManager::Update` runs every frame, the effect is simply applied on a later frame,
//! once the summon has finished constructing.
//!
//! # Why not a blanket return on the leaf
//!
//! There is no constant that is inert everywhere. Across all 72 call sites of `0x1404f9940` and its
//! `ChrIns` wrapper, roughly 27 are `TEST AL,AL; JZ skip` (where `false` is the inert answer) and
//! roughly 19 are `TEST AL,AL; JNZ skip` (where `true` is), with the rest consuming the boolean
//! directly. The guard is narrow on purpose: it fires only for a null container, and it pairs the
//! query with the apply rather than picking a winner among the callers.
//!
//! # Coverage
//!
//! Hooking the leaf also covers `CS::ChrIns::HasSpecialEffectId`, since that wrapper jumps into it.
//! `SpecialEffect::Apply` is reached through vtables as well as direct calls (data references at
//! `0x1434d72e0`, `0x1434d72f0`, `0x1448b60ac`), so guarding the function entry covers virtual
//! dispatch too.

use super::{Guard, null_arg1_guard};

/// `bool CS::SpecialEffect::HasSpecialEffectId(SpecialEffect *container, uint spEffectId)`.
///
/// 1.16.2 prologue, byte-verified against `eldenring-deobf.bin` (file offset == RVA, base
/// `0x140000000`): `48 8b 49 08 48 85 c9 74 15` -- `MOV RCX,[RCX+8]; TEST RCX,RCX; JZ +0x15`.
pub(crate) const HAS_SPECIAL_EFFECT_ID_RVA: usize = 0x4f9940;
pub(crate) const HAS_SPECIAL_EFFECT_ID_PROLOGUE: &[u8] =
    &[0x48, 0x8b, 0x49, 0x08, 0x48, 0x85, 0xc9, 0x74, 0x15];

/// `int CS::SpecialEffect::Apply(SpecialEffect *container, int spEffectId, ChrIns *, ChrIns *,
/// FloatVector4 *, byte, bool, byte)`.
///
/// 1.16.2 prologue, byte-verified the same way: `48 89 6c 24 10 48 89 74 24 18` --
/// `MOV [RSP+0x10],RBP; MOV [RSP+0x18],RSI`.
///
/// Four of its eight arguments arrive on the stack and at least one slot is typed inconsistently
/// between call sites, which is exactly why the detour tail-jumps instead of re-marshalling
/// arguments.
pub(crate) const APPLY_RVA: usize = 0x4fa8e0;
pub(crate) const APPLY_PROLOGUE: &[u8] =
    &[0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18];

null_arg1_guard! {
    /// Null container -> `false`, matching the original's own empty-list return.
    stub = has_special_effect_id_guard,
    original = ORIG_HAS_SPECIAL_EFFECT_ID,
    blocked = BLOCKED_HAS_SPECIAL_EFFECT_ID,
    // `XOR AL,AL` is what the original executes at 0x1404f995e when the walk finds nothing.
    ret = ["xor eax, eax"],
}

null_arg1_guard! {
    /// Null container -> `-1`, matching the original's own conditions-not-met return.
    stub = apply_guard,
    original = ORIG_APPLY,
    blocked = BLOCKED_APPLY,
    // `OR RAX,-1` is what the original executes at 0x1404fa9df when CheckApplyConditions fails.
    ret = ["or rax, -1"],
}

pub(crate) const HAS_SPECIAL_EFFECT_ID_GUARD: Guard = Guard {
    name: "SpecialEffect::HasSpecialEffectId",
    rva: HAS_SPECIAL_EFFECT_ID_RVA,
    expected_prologue: HAS_SPECIAL_EFFECT_ID_PROLOGUE,
    detour: has_special_effect_id_guard,
    original: &ORIG_HAS_SPECIAL_EFFECT_ID,
    blocked: &BLOCKED_HAS_SPECIAL_EFFECT_ID,
    rationale: "null container -> false (the original's own empty-list answer); paired with the \
                Apply guard so the caller's 'if !has then apply' cannot fault on the same field",
};

pub(crate) const APPLY_GUARD: Guard = Guard {
    name: "SpecialEffect::Apply",
    rva: APPLY_RVA,
    expected_prologue: APPLY_PROLOGUE,
    detour: apply_guard,
    original: &ORIG_APPLY,
    blocked: &BLOCKED_APPLY,
    rationale: "null container -> -1 (the original's own conditions-not-met answer); FUN_1403fade0 \
                already tests it with JS and unwinds to false",
};

#[cfg(test)]
mod tests {
    use super::*;

    /// These RVAs and bytes are the whole safety argument for the hook: if either drifts, the
    /// install-time prologue check is comparing against the wrong thing.
    #[test]
    fn rvas_and_prologues_match_er_1162_static_re() {
        assert_eq!(HAS_SPECIAL_EFFECT_ID_RVA, 0x4f9940);
        assert_eq!(APPLY_RVA, 0x4fa8e0);
        assert_eq!(
            HAS_SPECIAL_EFFECT_ID_PROLOGUE,
            &[0x48, 0x8b, 0x49, 0x08, 0x48, 0x85, 0xc9, 0x74, 0x15]
        );
        assert_eq!(
            APPLY_PROLOGUE,
            &[0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18]
        );
    }

    /// MinHook relocates the first five bytes of the entry. Both prologues must therefore start
    /// with whole instructions that are position-independent, and neither may contain a branch
    /// target inside that window. Asserting the length keeps a future edit from shortening a
    /// signature to fewer bytes than the patch actually overwrites.
    #[test]
    fn prologue_signatures_cover_the_patch_window() {
        const MINHOOK_PATCH_BYTES: usize = 5;
        for prologue in [HAS_SPECIAL_EFFECT_ID_PROLOGUE, APPLY_PROLOGUE] {
            assert!(prologue.len() >= MINHOOK_PATCH_BYTES);
        }
    }
}
