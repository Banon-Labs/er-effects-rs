//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

// Fault-safe RAM readers + game base/rva/image primitives now live in the shared
// er-game-base crate (single source of truth across product + telemetry + the
// mini-DLLs). Re-exported at their historical `crate::experiments::` paths so the
// ~40 product/telemetry call sites (bare via glob and fully-qualified) are
// unchanged. patch_3byte_stub / apply_xor_ret_stub stay below: they depend on the
// windows crate + product constants + append_autoload_debug.
pub(crate) use er_game_base::mem::{
    game_module_base, game_rva, game_rva_for_hook, is_heap_aligned_ptr, safe_read_f32,
    safe_read_i32, safe_read_u8, safe_read_u16, safe_read_usize, vtable_in_game_image,
};

// PlayerGameData character-name layout/readers now live beside the shared fault-safe RAM readers.
// Keep the historical flat product names while the remaining experiments modules are extracted.
pub(crate) use er_game_base::pgd::{
    read_utf16_name_units, utf16_name_empty_like, utf16_names_equal,
};
// safe_read_usize/i32/f32/u8/u16 moved to er_game_base::mem (re-exported above).

/// `base + rva`, resolved for the running build, or `None` when this build moved the function and
/// nothing verified where to.
///
/// # Why every direct call has to come through here
///
/// A detour installed by `er-hook` already asks: it translates a known 1.16.2 address for the
/// running build, or refuses, which is why a moved function now produces a log line instead of a
/// corrupted image. A hand-built function pointer asks nothing. On ELDEN RING 1.17 it transfers
/// control into whatever now occupies the 1.16.2 address -- routinely the middle of an unrelated
/// function, which faults with no unwind information and an exception record naming nothing of
/// ours. That is the worse of the two failures: a refused detour makes one feature inert and says
/// so, a stale call takes the process down.
///
/// A mapped constant is not safer this way. The map knows exactly where the function went;
/// `base + rva` simply never asks it.
#[cfg(windows)]
pub(crate) fn gated_game_fn(rva: usize, what: &'static str) -> Option<usize> {
    er_game_base::mem::game_rva_named(rva as u32, what).ok()
}

/// Whether the UTF-16 string at `ptr` is exactly `ascii`, terminator included.
///
/// A fault-safe read per unit, so an unmapped or misread pointer answers `false` rather than
/// faulting. It stood in `quit_menu/system_quit_dialog_handlers.rs` and has nothing to do with
/// the rows -- it reads memory, which is what this module is.
pub(crate) unsafe fn wide_equals_ascii(ptr: usize, ascii: &[u8]) -> bool {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || ascii.is_empty() {
        return false;
    }
    for (idx, want) in ascii.iter().copied().enumerate() {
        let Some(unit) = (unsafe { safe_read_u16(ptr + idx * core::mem::size_of::<u16>()) }) else {
            return false;
        };
        if unit != want as u16 {
            return false;
        }
    }
    matches!(
        unsafe { safe_read_u16(ptr + ascii.len() * core::mem::size_of::<u16>()) },
        Some(0)
    )
}
