//! Tier A: read the return-address stack and answer two questions about the game image.
//!
//! Both functions here walk the caller's stack with `RtlCaptureStackBackTrace` and compare the
//! captured return addresses against the running module base. Neither reads nor writes any
//! product state, so they belong beside [`crate::mem::game_module_base`] rather than inside a
//! single mod DLL -- which is how they arrive here.
//!
//! # Why these moved out of the product
//!
//! [`trace_first_game_caller_rva`] had two definitions. The product owned the real one, and
//! `er-loading-portrait-core` carried a `LoadingCoverHost` function-pointer field whose only job
//! was to reach back into the product for it. A crate that depends on `er-game-base` can now call
//! it directly, so that seam field is gone: a pure function reached through a host pointer is a
//! seam entry that buys nothing and has to be wired correctly by every shell that installs a host.
//!
//! [`callstack_contains_game_rva`] moves for the same reason plus one more. It is what the
//! System>Quit row cloner uses to tell which of the two native `AddCancelButton` calls it is
//! standing in, so any crate that grows the row-cloning machinery needs it, not just the product.
//!
//! # The frame window, and why it is not the crash logger's
//!
//! `FRAMES_TO_SKIP` is zero and `FRAME_COUNT` is eight, which are the values these two functions
//! have always run with. `er-crash-logging-core` captures 24 frames and skips 2 because it is
//! describing a fault for a human to read; these two are answering a yes/no about the immediate
//! caller on a hot path, and widening the window would change both the cost and, for
//! [`trace_first_game_caller_rva`], the answer.

/// Return addresses captured per walk. Eight is enough to reach the game frame that called into a
/// detour through our own prologue, and short enough to run on a per-press path.
#[cfg(windows)]
const FRAME_COUNT: usize = 8;
/// Frames discarded before the first captured one. Zero: the callers below account for their own
/// prefix by range-testing every frame rather than by assuming a fixed depth.
#[cfg(windows)]
const FRAMES_TO_SKIP: u32 = 0;
/// Sentinel for "the module base did not resolve", and the `rva` answer that means "nothing
/// qualifying was captured". Both are zero, and zero is never a meaningful answer for either.
const NO_ADDRESS: usize = 0;
/// Return addresses this far above the module base are not game `.text`. The game image is well
/// under 64 MB, so an address beyond this is a different module that happens to sit above the base.
#[cfg(windows)]
const GAME_TEXT_RVA_LIMIT: usize = 0x0400_0000;

#[cfg(windows)]
unsafe extern "system" {
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        back_trace: *mut *mut core::ffi::c_void,
        back_trace_hash: *mut u32,
    ) -> u16;
    fn GetModuleHandleA(module_name: *const u8) -> isize;
}

/// The captured return addresses, and the module base to measure them against.
///
/// Returns `None` when the module base does not resolve, because every caller's answer is then a
/// refusal rather than a computed value -- there is no base to subtract.
#[cfg(windows)]
fn walk() -> Option<([*mut core::ffi::c_void; FRAME_COUNT], usize, usize)> {
    let mut frames = [core::ptr::null_mut::<core::ffi::c_void>(); FRAME_COUNT];
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            FRAMES_TO_SKIP,
            frames.len() as u32,
            frames.as_mut_ptr(),
            core::ptr::null_mut(),
        )
    } as usize;
    let module_base = unsafe { GetModuleHandleA(core::ptr::null()) };
    if module_base == 0 {
        return None;
    }
    Some((frames, captured, module_base as usize))
}

/// Is any captured return address inside `start_rva..end_rva` of the running module?
///
/// The band is half-open, and an address below the module base is never a match. Used to identify
/// which native call site a detour is standing in -- the System>Quit row cloner asks this twice,
/// once per `AddCancelButton` call, because the two calls differ only by their return address.
#[cfg(windows)]
pub fn callstack_contains_game_rva(start_rva: usize, end_rva: usize) -> bool {
    let Some((frames, captured, module_base)) = walk() else {
        return false;
    };
    frames.iter().take(captured).any(|frame| {
        let address = *frame as usize;
        address >= module_base
            && address.wrapping_sub(module_base) >= start_rva
            && address.wrapping_sub(module_base) < end_rva
    })
}

/// The `rva` of the first captured return address that lands inside the game image.
///
/// Returns [`NO_ADDRESS`] when the module base does not resolve or nothing captured qualifies, so
/// a caller logging this value prints `0` rather than an address it would then chase.
#[cfg(windows)]
pub fn trace_first_game_caller_rva() -> usize {
    let Some((frames, captured, module_base)) = walk() else {
        return NO_ADDRESS;
    };
    frames
        .iter()
        .take(captured)
        .filter_map(|frame| {
            let address = *frame as usize;
            if address >= module_base {
                let rva = address.wrapping_sub(module_base);
                if rva < GAME_TEXT_RVA_LIMIT {
                    return Some(rva);
                }
            }
            None
        })
        .next()
        .unwrap_or(NO_ADDRESS)
}

/// Host builds have no game image and no `RtlCaptureStackBackTrace`, so the answer is the same
/// refusal the windows arm gives when the module base does not resolve.
#[cfg(not(windows))]
pub fn callstack_contains_game_rva(_start_rva: usize, _end_rva: usize) -> bool {
    false
}

/// Host-build counterpart to the windows [`trace_first_game_caller_rva`]; see that arm.
#[cfg(not(windows))]
pub fn trace_first_game_caller_rva() -> usize {
    NO_ADDRESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a host build both readers must answer the refusal rather than panic or read memory, so
    /// a crate that calls them from cross-platform code still has host tests.
    #[test]
    fn host_build_answers_the_refusal() {
        #[cfg(not(windows))]
        {
            assert!(!callstack_contains_game_rva(0, usize::MAX));
            assert_eq!(trace_first_game_caller_rva(), NO_ADDRESS);
        }
        // On windows the module base resolves, so the only invariant that holds without a game
        // image is that an empty band matches nothing.
        #[cfg(windows)]
        assert!(!callstack_contains_game_rva(0, 0));
    }
}
