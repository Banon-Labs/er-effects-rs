//! The guard registry.
//!
//! A *guard* is a detour that intercepts one vanilla function, recognises one argument shape the
//! function's own contract does not admit, and returns immediately with a value the function
//! itself already produces. Everything else falls through to the original.
//!
//! # Rules a new guard must satisfy
//!
//! 1. **Return the guarded function's OWN value for "nothing here".** Find the branch inside the
//!    original that already handles the empty/absent case and return exactly what it returns. A
//!    guard that invents a return value is asserting something the game never asserts, and the
//!    callers were not written against it.
//! 2. **Check the callers before trusting that value.** A crash is not automatically worse than
//!    what a caller does with a wrong answer. `null_special_effect` exists in two halves precisely
//!    because guarding only the query would have sent the crashing caller into an apply path that
//!    faults on the same field -- see that module's header.
//! 3. **Pin the address by its bytes.** Every RVA here is a 1.16.2 address. `expected_prologue` is
//!    checked against the live image before the hook is installed, so a different build disarms
//!    that guard loudly instead of detouring whatever now lives at the address.
//! 4. **Never clobber the ABI.** Detours are naked stubs that either return a constant or tail-jump
//!    to the original with every register and the entire incoming stack frame untouched. That is
//!    what makes a guard safe on a function whose stack arguments we have not typed.
//!
//! Adding a guard: write the stub with [`null_arg1_guard`], then add one [`Guard`] row to
//! [`REGISTRY`]. No install, logging, or telemetry code changes.

use core::sync::atomic::{AtomicU64, AtomicUsize};

pub(crate) mod null_special_effect;

/// Value stored in a guard's `original` slot before installation succeeds. A stub whose slot still
/// holds this must never be entered, which is why `original` is only published after `MhHook::new`
/// (or the product union) hands back a real trampoline.
pub(crate) const ORIGINAL_UNSET: usize = 0;

/// One intercepted function.
pub(crate) struct Guard {
    /// Name used in the install log and the telemetry line.
    pub(crate) name: &'static str,
    /// 1.16.2 RVA of the guarded function's entry.
    pub(crate) rva: usize,
    /// Bytes this address was verified against, re-checked against the live image before hooking.
    /// A mismatch disarms this guard alone; the others still install.
    pub(crate) expected_prologue: &'static [u8],
    /// The naked stub that replaces the entry.
    pub(crate) detour: unsafe extern "system" fn(),
    /// Trampoline, or the next handler when routed through the product DLL's hook union. The stub
    /// tail-jumps through this slot, so it must be published before the hook is enabled.
    pub(crate) original: &'static AtomicUsize,
    /// Times the guard's early-return branch actually fired. This is the guard's semaphore: a run
    /// that never increments it never met the bug, and proves nothing about the fix.
    pub(crate) blocked: &'static AtomicU64,
    /// Why the early-return value is the right answer, printed at install time so a reader of the
    /// log does not have to open this source to judge the guard.
    pub(crate) rationale: &'static str,
}

/// Every guard this DLL installs.
pub(crate) static REGISTRY: &[Guard] = &[
    null_special_effect::HAS_SPECIAL_EFFECT_ID_GUARD,
    null_special_effect::APPLY_GUARD,
];

/// Define a naked stub that returns early when the first integer argument (`rcx` under Win64) is
/// null, and otherwise tail-jumps to the original.
///
/// The tail jump is what keeps this safe on functions with untyped stack arguments: control reaches
/// the original with the caller's own frame, arguments, and shadow space exactly as they arrived,
/// so the original cannot tell it was detoured. Only the flags differ, and Win64 does not preserve
/// flags across a call boundary.
///
/// `$ret` is the instruction sequence that materialises the guarded function's own "nothing here"
/// return value in `rax`/`al`.
macro_rules! null_arg1_guard {
    (
        $(#[$meta:meta])*
        stub = $stub:ident,
        original = $original:ident,
        blocked = $blocked:ident,
        ret = [$($ret:literal),+ $(,)?] $(,)?
    ) => {
        pub(crate) static $original: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new($crate::guards::ORIGINAL_UNSET);
        pub(crate) static $blocked: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);

        $(#[$meta])*
        ///
        /// # Safety
        ///
        /// Installed as a MinHook detour on the address named by its [`Guard`] row; never called
        /// from Rust. Entering it before `original` holds a trampoline would jump to zero, which is
        /// why installation publishes `original` before enabling the hook.
        #[cfg(windows)]
        #[unsafe(naked)]
        pub(crate) unsafe extern "system" fn $stub() {
            core::arch::naked_asm!(
                "test rcx, rcx",
                "jnz 2f",
                // Atomic so the count stays exact when several game threads fault at once, and
                // memory-only so no register is disturbed on the way to the return.
                "lock inc qword ptr [rip + {blocked}]",
                $($ret,)+
                "ret",
                "2:",
                "jmp qword ptr [rip + {original}]",
                blocked = sym $blocked,
                original = sym $original,
            )
        }

        // The host build has no game to hook; the stub exists only so the registry still names a
        // function pointer and the crate stays checkable off-target.
        #[cfg(not(windows))]
        pub(crate) unsafe extern "system" fn $stub() {}
    };
}

pub(crate) use null_arg1_guard;
