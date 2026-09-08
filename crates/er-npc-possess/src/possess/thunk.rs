//! The VTABLE SWIZZLE: one slot of the creature's own `ComManipulator` vtable replaced, on a
//! patched copy of that table, so its brain stops running and nothing else about it changes.
//!
//! # What it is for
//!
//! `[vt+0x48]` is `if (aiIns && ShouldUpdateAi) AiIns::Update(aiIns, dt)` and nothing else -- goal
//! selection, target acquisition, path replanning, AI timers. `[vt+0x50]` is the per-frame tick
//! that runs `AiIns::UpdateMovement` and turns `walkType`/`wantToMoveTo` into a published move
//! vector. **Think** and **execute** live in two different slots, so no-oping one and leaving the
//! other alone buys AI-free locomotion with no other change. Damage, animation and physics are not
//! in the manipulator at all; they hang off `ChrIns::Update`, which is untouched.
//!
//! Only slot `+0x48` differs from the creature's own table. Every other entry is copied verbatim,
//! which is what makes the swizzle invisible to the rest of the engine.
//!
//! # why this replaced the `ChrCtrl+0x3b0` override, measured 2026-09-02
//!
//! The previous design put a separate 55-slot object in `ChrCtrl+0x3b0` -- a manipulator override
//! slot retail never writes -- whose stubs swapped `rcx` to the real `ComManipulator` and
//! tail-jumped. Dispatch worked. The problem was that the engine does not only dispatch through
//! that slot: fourteen sites resolve `chrManipulator ?? manipulator` and then use the answer as an
//! object. Enumerate them with `find-deobf-field-access.py 0x3b0`.
//!
//! The publish and the consume are a matched pair, and the `rcx` swap put them on opposite sides:
//!
//! ```text
//! publish:  FUN_1403cdc20(manip, vec)   writes ChrManipulator +0x10 AND +0x70
//! consume:  FUN_1403cd770(manip, out)   MOVUPS XMM0,[RCX+0x10] ; MOVAPS [RDX],XMM0 ; RET
//! caller:   FUN_1403cbff0(ChrCtrl*)     resolves chrManipulator ?? manipulator,
//!                                       then calls FUN_1403cd770 on the result
//! ```
//!
//! So `[vt+0x50]` published into the real object while the consumer read the override's zeroes.
//! `FUN_1403cd4c0`, called on the same pointer two lines later, reads `+0x20`..`+0x50` and was
//! starved identically. Runtime telemetry: `staged == published` on the real object with the body
//! frozen to five decimals.
//!
//! One object cannot diverge from itself. Swizzling the real manipulator's vptr keeps every field
//! read and every field write on the same object, and `ChrCtrl+0x3b0` is left NULL -- which also
//! retires the `ChrCtrl::Unref` DLPanic that the old teardown had to sequence around.
//!
//! # Why 55 slots
//!
//! `0x1B8` bytes, agreed by three independent oracles: an `.rdata` COL-boundary scan measuring
//! every one of the eight manipulator vtables at exactly `0x1b8`; Ghidra's curated
//! `ChrManipulator_vtable` struct at 55 fields / 440 bytes; and both game images separately. The
//! copy must be the full length or the engine reads past its end -- an under-sized table is not a
//! smaller feature, it is a shipped crash on whichever slot nobody happened to exercise.
//!
//! # Why the RTTI pointer is copied too
//!
//! MSVC puts the `CompleteObjectLocator` at `vptr[-1]`, and `dynamic_cast` and the
//! `GetRuntimeClassMetadata` paths read it by walking backwards off the vtable pointer. A copy
//! that started at slot 0 would leave those reads looking at whatever precedes our allocation, so
//! the page holds that pointer first and the address we hand out is one slot past it.
//!
//! # The one ordering that is a crash
//!
//! The page is ours; the object is the game's. While the swizzle is installed the creature's
//! manipulator holds a pointer into this page, so the original vptr must go back before the page
//! is freed and before the creature is destroyed. `crate::possess::teardown` sequences that, and
//! `Step::RestoreManipulatorVtable` is the step whose failure is treated as unrecoverable.
//!
// The emitter below is pure byte arithmetic and stays ungated so its tests run on the host; the
// allocation and install half is `#[cfg(windows)]`.
#![cfg_attr(not(windows), allow(dead_code))]

/// Slots in a `ChrManipulator` vtable. See the module docs for the three oracles.
pub(crate) const SLOT_COUNT: usize = 55;

/// Bytes in that vtable. Named separately from `SLOT_COUNT * 8` because it is the number the three
/// oracles actually measured, and a test asserts the two agree.
pub(crate) const VTABLE_BYTES: usize = 0x1b8;

/// `[vt+0x08]` -- the scalar deleting destructor.
///
/// It is copied verbatim, and that is a reversal of the previous design. When the override object
/// was a separate allocation this slot had to return `this` and do nothing, because forwarding it
/// would have freed the creature's real manipulator. Now the object is the creature's real
/// manipulator, the engine owns its lifetime, and a slot that refused to destroy it would leak the
/// object and leave a live vptr pointing into a page we free. Verbatim is the only correct answer.
pub(crate) const SLOT_DESTRUCTOR: usize = 0x08;

/// `[vt+0x48]` -- `UpdateAi`. The one slot that is replaced, and the reason the mod works.
pub(crate) const SLOT_UPDATE_AI: usize = 0x48;

/// The slot we replace must not be the slot that destroys the object.
///
/// A `const` assertion rather than a test, because the consequence is not a wrong answer: a
/// patched table whose destructor pointed at `xor eax,eax; ret` would leak the creature's
/// manipulator and leave our freed page in its vptr. That should be a build error.
const _: () = assert!(SLOT_DESTRUCTOR != SLOT_UPDATE_AI);

/// Bytes reserved for the no-op stub, padded so a fall-through lands on a breakpoint.
pub(crate) const STUB_STRIDE: usize = 16;

/// The MSVC RTTI `CompleteObjectLocator` pointer, which lives at `vptr[-1]`.
///
/// Part of the copy even though it is not a slot: `dynamic_cast` and the `GetRuntimeClassMetadata`
/// paths read it by walking backwards off the vtable pointer, so a copy that starts at slot 0
/// leaves those reads looking at whatever happens to precede our allocation.
pub(crate) const RTTI_SLOT_BYTES: usize = core::mem::size_of::<usize>();

/// `int3`. Pads the stub out to [`STUB_STRIDE`].
const INT3: u8 = 0xcc;

/// The no-op that replaces `UpdateAi`.
///
/// `xor eax, eax; ret` -- the same body `ComManipulator`'s own `[vt+0xf8]` already has, so it is a
/// shape the engine tolerates from a manipulator. `rcx` is untouched because with the vtable
/// swizzled there is nothing to swap: `this` already is the real `ComManipulator`.
#[must_use]
pub(crate) fn no_op_stub_bytes() -> [u8; STUB_STRIDE] {
    let mut out = [INT3; STUB_STRIDE];
    // 31 c0  xor eax, eax
    // c3     ret
    out[..3].copy_from_slice(&[0x31, 0xc0, 0xc3]);
    out
}

/// The patched vtable: the creature's own, with [`SLOT_UPDATE_AI`] pointed at the no-op.
///
/// Every other slot is the original function pointer, so there are no stubs, no `rcx` swap and no
/// second object -- which is the whole point. The divergence the previous design created (the
/// engine writing fields on one object while fourteen consumers read them off another) cannot
/// exist when there is only one object.
#[must_use]
pub(crate) fn patched_vtable(original: &[usize], no_op_at: usize) -> Vec<usize> {
    let mut out = original.to_vec();
    if let Some(slot) = out.get_mut(SLOT_UPDATE_AI / core::mem::size_of::<usize>()) {
        *slot = no_op_at;
    }
    out
}

/// Where each piece sits inside the single page.
pub(crate) mod plan {
    use super::{RTTI_SLOT_BYTES, STUB_STRIDE, VTABLE_BYTES};

    /// The copied `vptr[-1]` RTTI pointer, first, so the vtable that follows is what we hand out.
    pub(crate) const RTTI_AT: usize = 0;
    /// The vtable proper. This address is what goes in the object's vptr.
    pub(crate) const VTABLE_AT: usize = RTTI_AT + RTTI_SLOT_BYTES;
    /// The single no-op stub.
    pub(crate) const STUB_AT: usize = VTABLE_AT + VTABLE_BYTES;
    /// Total bytes to reserve.
    pub(crate) const TOTAL_BYTES: usize = STUB_AT + STUB_STRIDE;

    /// The vtable must stay 8-aligned, which it is because the RTTI slot is one pointer.
    const _: () = assert!(VTABLE_AT.is_multiple_of(8));
    /// ...and the whole thing must fit in one page, which is what makes a single `VirtualAlloc`
    /// the right shape.
    const _: () = assert!(TOTAL_BYTES <= 0x1000);
}

#[cfg(windows)]
pub(crate) use windows_impl::Thunk;

#[cfg(windows)]
mod windows_impl {
    use super::{RTTI_SLOT_BYTES, SLOT_COUNT, no_op_stub_bytes, patched_vtable, plan};
    use er_game_base::mem::{is_heap_aligned_ptr, safe_read_usize};

    unsafe extern "system" {
        fn VirtualAlloc(
            address: *mut core::ffi::c_void,
            size: usize,
            allocation_type: u32,
            protect: u32,
        ) -> *mut core::ffi::c_void;
        fn VirtualFree(address: *mut core::ffi::c_void, size: usize, free_type: u32) -> i32;
    }

    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_EXECUTE_READWRITE: u32 = 0x40;

    /// A patched vtable for one creature's real `ComManipulator`, plus the original vptr to put
    /// back.
    ///
    /// # Lifetime, and the one ordering that is a crash
    ///
    /// The page is ours; the object is the game's. While the swizzle is installed the creature's
    /// manipulator holds a pointer into this page, so freeing it before restoring the original
    /// vptr leaves the engine dispatching through unmapped memory on its next tick. The restore
    /// must therefore run before `Drop`, and the teardown state machine in
    /// [`crate::possess::teardown`] is what enforces that order -- the same guarantee it used to
    /// give the `ChrCtrl+0x3b0` write, for the same reason.
    #[derive(Debug)]
    pub(crate) struct Thunk {
        page: *mut u8,
        real_com: usize,
        original_vptr: usize,
    }

    // The page is reached only from the game task thread, and the raw pointer is the reason the
    // compiler cannot see that. `PossessionEngine` requires `Send`, so say it explicitly.
    unsafe impl Send for Thunk {}

    impl Thunk {
        /// Copy `real_com`'s vtable, patch `UpdateAi` out of the copy, and keep the original.
        ///
        /// Nothing is installed here -- the swizzle is a separate step, so a failure to build
        /// leaves the creature completely untouched.
        ///
        /// Returns `None` when the manipulator's vtable will not read or the page cannot be
        /// reserved. Both are refusals, not partial states.
        pub(crate) fn build(real_com: usize) -> Option<Self> {
            let original_vptr = unsafe { safe_read_usize(real_com) }?;
            if !unsafe { is_heap_aligned_ptr(original_vptr) } {
                return None;
            }
            // The whole table plus the RTTI pointer one slot before it, every read checked: a
            // manipulator whose table is not fully mapped is one to decline, not one to copy
            // garbage out of.
            let rtti = unsafe { safe_read_usize(original_vptr.checked_sub(RTTI_SLOT_BYTES)?) }?;
            let mut slots = Vec::with_capacity(SLOT_COUNT);
            for index in 0..SLOT_COUNT {
                slots.push(unsafe { safe_read_usize(original_vptr + index * 8) }?);
            }

            let page = unsafe {
                VirtualAlloc(
                    core::ptr::null_mut(),
                    plan::TOTAL_BYTES,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_EXECUTE_READWRITE,
                )
            }
            .cast::<u8>();
            if page.is_null() {
                return None;
            }
            unsafe { core::ptr::write_bytes(page, 0, plan::TOTAL_BYTES) };

            let stub = no_op_stub_bytes();
            unsafe {
                core::ptr::copy_nonoverlapping(stub.as_ptr(), page.add(plan::STUB_AT), stub.len());
            }
            let table = patched_vtable(&slots, page as usize + plan::STUB_AT);
            unsafe {
                page.add(plan::RTTI_AT).cast::<usize>().write(rtti);
                core::ptr::copy_nonoverlapping(
                    table.as_ptr(),
                    page.add(plan::VTABLE_AT).cast::<usize>(),
                    SLOT_COUNT,
                );
            }
            Some(Self {
                page,
                real_com,
                original_vptr,
            })
        }

        /// The vtable pointer this thunk hands the creature's manipulator.
        pub(crate) fn vtable_address(&self) -> usize {
            self.page as usize + plan::VTABLE_AT
        }

        /// The real `ComManipulator` whose vtable this patches.
        pub(crate) fn real_com(&self) -> usize {
            self.real_com
        }

        /// The vptr that was there before, and the value the restore puts back.
        pub(crate) fn original_vptr(&self) -> usize {
            self.original_vptr
        }
    }

    impl Drop for Thunk {
        fn drop(&mut self) {
            // Nothing checks the result: a failing `VirtualFree` on a page we allocated means the
            // address is already gone, and there is no recovery to attempt from a Drop.
            unsafe { VirtualFree(self.page.cast(), 0, MEM_RELEASE) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for a creature's real vtable: 55 distinct, recognisable pointers.
    fn original_table() -> Vec<usize> {
        (0..SLOT_COUNT)
            .map(|index| 0x1_4000_0000 + index * 0x10)
            .collect()
    }

    /// The size that is a crash if it is wrong. Three oracles say 55 slots / `0x1b8` bytes, and
    /// the engine reads past a shorter table.
    #[test]
    fn the_vtable_is_fifty_five_slots_and_exactly_0x1b8_bytes() {
        assert_eq!(SLOT_COUNT, 55);
        assert_eq!(VTABLE_BYTES, 0x1b8);
        assert_eq!(SLOT_COUNT * core::mem::size_of::<usize>(), VTABLE_BYTES);
    }

    /// Exactly one slot differs, and it is `UpdateAi`. Everything else is the creature's own
    /// function pointer -- that is what makes the swizzle invisible to every consumer, and it is
    /// the property the previous design could not have because it emitted 55 stubs of its own.
    #[test]
    fn the_patched_table_changes_update_ai_and_nothing_else() {
        let original = original_table();
        let patched = patched_vtable(&original, 0xdead_0000);
        assert_eq!(patched.len(), SLOT_COUNT);
        let update_ai = SLOT_UPDATE_AI / core::mem::size_of::<usize>();
        for index in 0..SLOT_COUNT {
            if index == update_ai {
                assert_eq!(patched[index], 0xdead_0000, "the no-op");
            } else {
                assert_eq!(
                    patched[index], original[index],
                    "slot {index} must be verbatim"
                );
            }
        }
    }

    /// The slot that reversed meaning with this design, and the one that crashes if it is wrong.
    ///
    /// With a separate override object the destructor had to return `this`, because forwarding it
    /// would free the creature's real manipulator. The object is that manipulator now, so the
    /// engine's own destructor is the only correct body: refusing it would leak the object and
    /// leave our freed page in its vptr.
    #[test]
    fn the_destructor_slot_is_copied_verbatim_and_is_not_the_no_op() {
        let original = original_table();
        let patched = patched_vtable(&original, 0xdead_0000);
        let destructor = SLOT_DESTRUCTOR / core::mem::size_of::<usize>();
        assert_eq!(patched[destructor], original[destructor]);
        assert_ne!(patched[destructor], 0xdead_0000);
        // ...and it is a different slot from the one we do replace.
        assert_ne!(SLOT_DESTRUCTOR, SLOT_UPDATE_AI);
    }

    /// The no-op returns false and touches nothing. `rcx` in particular is untouched, because
    /// `this` is already the object the engine meant to call.
    #[test]
    fn the_update_ai_stub_is_xor_eax_ret_and_padded_to_a_breakpoint() {
        let stub = no_op_stub_bytes();
        assert_eq!(&stub[..3], &[0x31, 0xc0, 0xc3]);
        assert!(stub[3..].iter().all(|&byte| byte == INT3));
        assert_eq!(stub.len(), STUB_STRIDE);
        // No jump of any kind: a forwarding byte pair here would call the creature's brain.
        assert!(!stub.windows(2).any(|pair| pair == [0xff, 0xa0]));
    }

    /// The RTTI pointer is inside the allocation and before the vtable, or `dynamic_cast` reads
    /// whatever precedes our page.
    #[test]
    fn the_rtti_slot_sits_one_pointer_before_the_vtable() {
        assert_eq!(plan::VTABLE_AT - plan::RTTI_AT, RTTI_SLOT_BYTES);
        assert_eq!(RTTI_SLOT_BYTES, core::mem::size_of::<usize>());
        assert_eq!(plan::STUB_AT, plan::VTABLE_AT + VTABLE_BYTES);
        assert_eq!(plan::TOTAL_BYTES, plan::STUB_AT + STUB_STRIDE);
    }

    /// A short table must not be padded with invented slots, and the patch must not land off the
    /// end of one -- both would be a call through zero.
    #[test]
    fn a_short_original_table_is_copied_as_far_as_it_goes_and_no_further() {
        let short: Vec<usize> = vec![0x1234; 4];
        let patched = patched_vtable(&short, 0xdead_0000);
        assert_eq!(patched.len(), 4, "no invented slots");
        assert!(
            patched.iter().all(|&slot| slot == 0x1234),
            "and no patch off the end"
        );
    }
}
