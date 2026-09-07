//! `CS::CSFreeListMemorySystem`'s leak assertion, defused so quitting the game does not kill it.
//!
//! # The crash
//!
//! deto's Elden Ring died the same way twice on 2026-08-25 (20:16:43Z and 20:19:55Z), on a profile
//! carrying only this DLL and the crash logger. Unlike every other crash this DLL answers, it is
//! not an access violation: `exception_code=0x80000003` is `STATUS_BREAKPOINT`, and
//! `exception_address == context_rip == eldenring.exe+0xc57676` is a literal `INT3` the game
//! executes on purpose. Both records are identical apart from thread id and stack pointer.
//!
//! # It only happens on the way out
//!
//! `MainLoop` (`0x140c8fe90`) is `while (MainUpdate()) KillWindowTimer();`, and `CleanupUpdate`
//! (`0x140de9f50`) is reached only after that loop ends, in a bounded 200-iteration teardown loop.
//! The call at `0x140c8ff7b` returns to `0x140c8ff80`, which is frame `#14` of deto's crash log. So
//! this is quit-to-desktop: returning to the title, ending an invasion and ending a Seamless
//! session all stay inside `MainUpdate` and cannot reach this code at all.
//!
//! The rest of the chain, from the same log: `CleanupUpdate -> CSTaskImp(0x140eb2180) ->
//! FD4TaskManager(0x142655360) -> 0x14269f970 -> 0x14269f8a0 ->
//! CS::CSHkThreadLocalProcess::~(0x140c57190) -> 0x1416ccf10 -> INT3`. `0x1416ccf10` is what tears
//! down the `CSFreeListMemorySystem` singleton and NULLs its global, so the allocator really is
//! being destroyed here; this is not a per-frame path that merely looks like one.
//!
//! # What the assertion checks
//!
//! `0x140c575e0` is vtable slot 2 of `CS::CSFreeListMemorySystem` -- its RTTI complete object
//! locator at `0x143352958` names `.?AVCSFreeListMemorySystem@CS@@` -- called with flags `3`, which
//! is its free-everything path. It walks a chain of `0x28`-byte nodes linked at `+0x20` and breaks
//! on any node whose byte at `+0x18` is still set:
//!
//! ```text
//! 140c57670: 80 7a 18 00     CMP byte ptr [RDX + 0x18], 0x0
//! 140c57674: 74 01           JZ  0x140c57677
//! 140c57676: cc              INT3
//! 140c57677: 48 8b 4f 08     MOV RCX, [RDI + 0x8]      ; free the node -- on BOTH paths
//! ```
//!
//! That byte is the registrar's "checked out" flag. The registrar is vtable slot 3
//! (`0x140c579f0`): it reuses a node whose `+0x18` is zero, otherwise allocates exactly `0x28`
//! bytes, links it at `+0x20`, and sets `+0x18` to `1`. The sibling debug dump at `0x140c576b0`
//! walks the same chain under the heading `"unused in thread local freelists"`. So a set flag means
//! a thread-local free-list is still checked out while the allocator that owns it is being
//! destroyed -- and deto's minidump holds 52 live threads at that instant, including roughly twenty
//! `eldenring.exe` task workers still parked in `WaitForSingleObject`.
//!
//! # Why removing the `INT3` is safe
//!
//! The `JZ` at `0x140c57674` targets `0x140c57677`, which is `RIP + 1` from the `INT3`. Both the
//! asserting and the non-asserting path continue at the same instruction, and the node is freed
//! either way. The break reports; it does not protect. A debugger would step over it and the
//! function would carry on, which is exactly what this patch makes happen with no debugger
//! attached: `0xcc` becomes `0x90`, and the shutdown takes the path it already takes on every node
//! whose flag is clear.
//!
//! This does not fix the leak, and is not meant to. It cannot: by the time the assertion fires the
//! allocator is already being destroyed and the game frees the node regardless. What it removes is
//! an unhandled `STATUS_BREAKPOINT` that kills the process while it was already quitting.
//!
//! # Evidence this is not ours to begin with
//!
//! Every frame in the chain is `eldenring.exe`, and this DLL's sibling
//! [`null_param_repository`](crate::guards::null_param_repository) guard cannot reach it: that stub
//! tail-jumps to the original unless `GLOBAL_SoloParamRepository` is already null, so it can only
//! act after that singleton is destroyed -- later than any thread-join step. What it did do is
//! remove the *earlier* fatal stop. deto's 2026-08-22 and 2026-08-23 crashes died in this same
//! `CleanupUpdate` phase, on the `DLPanic` that guard now suppresses. The same broken shutdown now
//! simply runs one step further.

use core::sync::atomic::AtomicBool;

use super::Patch;

/// RVA of the `.pdata` function entry that contains the assertion: `CS::CSFreeListMemorySystem`
/// vtable slot 2, the free-everything path documented above.
///
/// # Why the constant names the function and not the window
///
/// The window itself, `0xc57670`, is `0x90` bytes inside this function, and a mid-function address
/// can never appear in a 1.16.2 -> 1.17 map: those tables are keyed on `.pdata` function starts,
/// which is what a masked signature can identify. `docs/recon/rva-map-1162-to-1170.verified.tsv`
/// records the refusal of `0xc57670` for exactly that reason.
///
/// The obvious repair is wrong, and every existing gate passes it. `.pdata` declares a record at
/// `0xc57666`, `0xa` bytes before the window, and `scripts/classify-1170-entry-kind.py` calls it
/// `ENTRY` on both builds; the pair `0xc57666 -> 0xc58d36` is already in
/// `docs/recon/rva-map-1162-to-1170.functions.tsv`. It is not a function start. Its `UNWIND_INFO`
/// carries `UNW_FLAG_CHAININFO`, so it is an MSVC chained-unwind continuation and the real
/// prologue is `0x86` bytes earlier -- here. A constant naming `0xc57666` would be selected into
/// the maps with `BOTH-ENTRIES` and would license MinHook to write five bytes `0x86` into a live
/// function body. `scripts/check-no-chained-continuation-rows.py` is the gate for that shape.
///
/// Confirmed on both images: `scripts/pdata-chain-root-1170.py` reports `0xc57666` (1.16.2) and
/// `0xc58d36` (1.17) as chained continuations of `0xc575e0` / `0xc58cb0`, both of which are root
/// records of extent `0x86`; RTTI names both `.?AVCSFreeListMemorySystem@CS@@` slot 2; and the
/// window's offset within the function is `0x90` in both builds
/// (`0xc57670 - 0xc575e0 == 0xc58d40 - 0xc58cb0`).
pub(crate) const FREELIST_SHUTDOWN_ASSERT_FN_RVA: usize = 0xc5_75e0;

/// Byte offset of the verified window's first byte within [`FREELIST_SHUTDOWN_ASSERT_FN_RVA`].
///
/// Added in Rust after the function is resolved, so it never enters an address table and can never
/// be mistaken for a detour licence -- see `er_game_base::game_build::resolve_call_site_rva`.
pub(crate) const FREELIST_SHUTDOWN_ASSERT_WINDOW_OFFSET: usize = 0x90;

/// 1.16.2 RVA of the `CMP` that opens the assertion, i.e. the first byte of the verified window.
/// The `INT3` itself is [`INT3_OFFSET`] bytes further on.
///
/// Derived rather than written, so the two halves above cannot drift from the address `build.rs`
/// ground-truths the generated window against. It is deliberately not a `= 0x...` literal:
/// `scripts/select-needed-1170-rows.py` scans for that spelling, and selecting this mid-function
/// address is the thing that must not happen.
///
/// Only the tests read it: since the address became a sum the install path goes through
/// [`Patch::rva`], so a plain build has no caller and `-D warnings` rejects it as dead. It is
/// `cfg(test)` rather than `allow(dead_code)` because it is not a retained-but-unread RE address
/// -- it is the independent second spelling the assertions below check `Patch::rva` against, and
/// cfg-ing it out of the shipped build says that exactly.
#[cfg(test)]
pub(crate) const FREELIST_SHUTDOWN_ASSERT_RVA: usize =
    FREELIST_SHUTDOWN_ASSERT_FN_RVA + FREELIST_SHUTDOWN_ASSERT_WINDOW_OFFSET;

/// Index of the `INT3` within the window: past `CMP byte ptr [RDX+0x18],0` (4 bytes) and `JZ +1`
/// (2 bytes).
const INT3_OFFSET: usize = 6;

/// `NOP`. One byte, like the `INT3` it replaces, so nothing after it moves and the `JZ` that jumps
/// over it still lands on the instruction it always landed on.
const NOP: u8 = 0x90;

/// Set once the byte has been written and read back as [`NOP`].
static ASSERT_DEFUSED: AtomicBool = AtomicBool::new(false);

// The expected window, assembled from named `iced-x86` instructions by this crate's `build.rs` and
// verified there against `eldenring-deobf.bin` when a copy is present.
include!(concat!(
    env!("OUT_DIR"),
    "/generated_freelist_shutdown_assert.rs"
));

pub(crate) const FREELIST_SHUTDOWN_ASSERT: Patch = Patch {
    name: "CSFreeListMemorySystem shutdown leak assert",
    function_rva: FREELIST_SHUTDOWN_ASSERT_FN_RVA,
    offset_in_function: FREELIST_SHUTDOWN_ASSERT_WINDOW_OFFSET,
    expected_window: FREELIST_SHUTDOWN_ASSERT_WINDOW,
    offset: INT3_OFFSET,
    replacement: NOP,
    applied: &ASSERT_DEFUSED,
    rationale: "the assert's own JZ targets RIP+1, so both paths converge on the MOV that frees \
                the node; replacing the INT3 with a NOP is the path the game already takes when \
                the flag is clear, and turns an unhandled STATUS_BREAKPOINT at quit into a clean \
                exit",
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The window is what pins the address, so its shape is worth asserting rather than trusting.
    #[test]
    fn window_is_the_assert_idiom() {
        assert_eq!(
            FREELIST_SHUTDOWN_ASSERT_WINDOW,
            &[
                0x80, 0x7a, 0x18, 0x00, 0x74, 0x01, 0xcc, 0x48, 0x8b, 0x4f, 0x08
            ],
            "the 1.16.2 bytes of CMP/JZ/INT3/MOV at 0x140c57670"
        );
    }

    #[test]
    fn the_patched_byte_is_the_int3() {
        assert_eq!(
            FREELIST_SHUTDOWN_ASSERT.replaced(),
            Some(0xcc),
            "the offset must name the INT3, not one of the instructions that pin it"
        );
    }

    /// `JZ +1` is the entire safety argument: it proves the break is skippable by design.
    #[test]
    fn the_jump_lands_one_byte_past_the_int3() {
        let jump_displacement = FREELIST_SHUTDOWN_ASSERT_WINDOW[INT3_OFFSET - 1];
        assert_eq!(
            usize::from(jump_displacement),
            1,
            "the JZ must skip exactly the INT3; a different displacement means this is not the \
             advisory-break idiom the patch is argued from"
        );
    }

    /// The displacement alone does not say what it is a displacement of. `74` is `JZ rel8`, and
    /// an `rel8` jump is measured from the end of its own two bytes -- so this recomputes the
    /// landing address the way the CPU does and requires it to be the byte after the `INT3`. Had
    /// the opcode been some other two-byte form, `+1` would land somewhere else entirely and the
    /// safety argument would not hold, while the displacement test above would still pass.
    #[test]
    fn the_jump_is_a_rel8_jz_measured_from_its_own_end() {
        const JZ_REL8: u8 = 0x74;
        const JZ_REL8_LEN: usize = 2;
        let jz_at = INT3_OFFSET - JZ_REL8_LEN;
        assert_eq!(
            FREELIST_SHUTDOWN_ASSERT_WINDOW[jz_at], JZ_REL8,
            "the instruction before the INT3 must be JZ rel8"
        );
        let landing = jz_at + JZ_REL8_LEN + usize::from(FREELIST_SHUTDOWN_ASSERT_WINDOW[jz_at + 1]);
        assert_eq!(
            landing,
            INT3_OFFSET + 1,
            "the JZ must land on the instruction AFTER the INT3 -- that convergence is why \
             removing the break is behaviour-preserving"
        );
        assert!(
            landing < FREELIST_SHUTDOWN_ASSERT_WINDOW.len(),
            "the instruction both paths converge on must be inside the verified window, or the \
             window does not actually pin the far side of the branch"
        );
    }

    /// The replacement has to be one byte wide and do nothing. A multi-byte replacement would
    /// shift the `MOV` the `JZ` targets; a one-byte replacement that is not a NOP would execute.
    #[test]
    fn the_replacement_is_a_one_byte_nop() {
        const X86_NOP: u8 = 0x90;
        assert_eq!(FREELIST_SHUTDOWN_ASSERT.replacement, X86_NOP);
        assert_eq!(
            FREELIST_SHUTDOWN_ASSERT.offset + 1,
            INT3_OFFSET + 1,
            "the patch must write at the INT3's own offset, so nothing after it moves"
        );
    }

    /// The RVA is what the install path adds to the live module base, and it is written
    /// separately from the VA `build.rs` ground-truths against `eldenring-deobf.bin`. If the two
    /// ever disagree, the bytes get verified at one address and the write lands at another.
    ///
    /// Since 2026-08-30 the address is a sum, so this also pins the two halves: the entry has to
    /// be the one `.pdata` roots the chain at, and the offset has to be the one measured in both
    /// images. Asserting only the sum would let a wrong entry be cancelled by a wrong offset --
    /// and a wrong entry is what gets resolved on 1.17, so the sum being right on 1.16.2 would
    /// prove nothing about the build this exists for.
    #[test]
    fn the_rva_matches_the_va_the_window_was_generated_from() {
        const PREFERRED_IMAGE_BASE: usize = 0x1_4000_0000;
        assert_eq!(
            PREFERRED_IMAGE_BASE + FREELIST_SHUTDOWN_ASSERT_RVA,
            0x1_40c5_7670,
            "the VA named in build.rs as FREELIST_SHUTDOWN_ASSERT_VA"
        );
        assert_eq!(FREELIST_SHUTDOWN_ASSERT.rva(), FREELIST_SHUTDOWN_ASSERT_RVA);
        assert_eq!(
            PREFERRED_IMAGE_BASE + FREELIST_SHUTDOWN_ASSERT_FN_RVA,
            0x1_40c5_75e0,
            "the ROOT .pdata record 0xc57666 chains to, and vtable slot 2 of \
             CS::CSFreeListMemorySystem in both 1.16.2 and 1.17"
        );
        assert_eq!(
            FREELIST_SHUTDOWN_ASSERT_WINDOW_OFFSET, 0x90,
            "0xc57670 - 0xc575e0 on 1.16.2, and 0xc58d40 - 0xc58cb0 on 1.17"
        );
    }

    /// The chained-continuation record `0xc57666` is the wrong answer that reads as the right one:
    /// `.pdata` declares it, `classify-1170-entry-kind.py` calls it `ENTRY`, and `functions.tsv`
    /// already pairs it. Naming it here would put a `BOTH-ENTRIES` row into the maps for an
    /// address `0x86` inside a live function.
    #[test]
    fn the_function_rva_is_not_the_chained_continuation_record() {
        const CHAINED_CONTINUATION_RVA: usize = 0xc5_7666;
        assert_ne!(
            FREELIST_SHUTDOWN_ASSERT_FN_RVA, CHAINED_CONTINUATION_RVA,
            "0xc57666 carries UNW_FLAG_CHAININFO; the root of its chain is 0xc575e0"
        );
        // `const` block: both sides are constants, so clippy::assertions_on_constants is right
        // that a runtime assert is the wrong tool. Checking it at compile time is strictly
        // stronger -- an edit that inverted the two would fail to build rather than fail a test.
        const {
            assert!(
                FREELIST_SHUTDOWN_ASSERT_FN_RVA < CHAINED_CONTINUATION_RVA,
                "the root record must start BEFORE the continuation that chains to it"
            )
        };
        assert_eq!(
            CHAINED_CONTINUATION_RVA - FREELIST_SHUTDOWN_ASSERT_FN_RVA,
            0x86,
            "the root record's extent, as read from both images' .pdata"
        );
    }

    /// A patch has no block counter, so `applied` is the only thing it ever reports. It must start
    /// false, or a run that never reached the install path would still claim the byte changed.
    #[test]
    fn the_defused_flag_starts_false() {
        assert!(!ASSERT_DEFUSED.load(core::sync::atomic::Ordering::Relaxed));
    }
}
