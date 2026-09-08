//! Who owns the swapchain's imgui context, and how everybody else draws on it anyway.
//!
//! # The bug this exists to make impossible
//!
//! hudhook's install latch is a plain `static`, and statics are per DLL. Two of our modules each
//! calling `Hudhook::apply()` therefore both believe they are first, both hook `Present`, and one
//! of them silently loses every frame from then on. [`claim_owner`] was added to stop that, but
//! it only ever told the loser to give up -- it gave the loser nowhere to draw. So the module
//! with the interactive UI could lose the race to a module that draws six words of grey text, and
//! the user would simply find their panel gone.
//!
//! Measured 2026-08-25, live, on the user's own session: `er-build-watermark` logged
//! `first render display_width=3840 rows=14` while `er-net-effects` logged
//! `hudhook dx12 overlay installed` and then `hudhook_render_count = 0` -- installed, never
//! rendered, no error anywhere. The interactive bar had been invisible since #336 added the
//! watermark shell, because me3 loads `er_build_watermark.dll` before `er_net_effects.dll` and
//! alphabetical order is not a design.
//!
//! A prior fix had the watermark sleep six seconds to let a richer UI claim the context first.
//! That was removed for being a sleep used as synchronization -- correctly -- but the yield it
//! implemented was load-bearing and nothing replaced it. This does, without any sleep.
//!
//! # The shape
//!
//! Exactly one module hosts the render loop; every other module registers a draw callback and the
//! host calls it each frame. Load order stops mattering, because whoever gets there first hosts
//! and everyone else is a guest -- the outcome is the same either way.
//!
//! A guest finds the host by walking the loaded-module list and calling
//! [`REGISTER_EXPORT`] on each. Every shell that links this crate exports it; each
//! implementation registers only if that module is the host, so exactly one call answers `true`.
//!
//! # The ABI, and why the tag is not optional
//!
//! The `ui` pointer crosses a DLL boundary as `*const c_void` and is cast back to `&Ui` in the
//! guest. That is only sound while host and guest were built against the same imgui, so every
//! registration carries [`OVERLAY_ABI_TAG`] and a host refuses a tag it does not recognise. A
//! refused guest draws nothing, which is a missing panel; accepting a mismatched one would
//! reinterpret an imgui context through the wrong struct layout inside `Present`, which is a
//! crash in the renderer with no useful stack. Missing panel is the better failure, and it logs.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use hudhook::imgui::Ui;

/// A guest's per-frame draw. Receives the host's `&Ui` erased to a pointer.
///
/// # Safety
///
/// The pointer is a live `&Ui` for the duration of the call and must not outlive it.
pub type OverlayDrawFn = unsafe extern "C" fn(frame: *const OverlayFrame);

/// Everything a guest needs to draw into the host's imgui, handed over once per frame.
///
/// # Why the context and the allocators travel with the pointer
///
/// Dear ImGui keeps its current context in a plain global, and each DLL that links imgui gets its
/// own copy of that global. A guest handed only a `&Ui` therefore calls `ui.io()` against a NULL
/// `GImGui` and dies on the first dereference -- which is exactly what happened on 2026-08-25:
/// the guest logged that its render loop had initialised and then never logged the line four
/// statements later, drew nothing, and raised no crash anyone could see.
///
/// `imgui_context` is the host's `igGetCurrentContext()`, and the guest must install it with
/// `igSetCurrentContext` before touching `ui`. The allocator triple matters for the same reason
/// and is easier to miss: imgui allocates draw-list vertices internally, so a guest running on
/// its own allocator globals would allocate from one heap and hand the buffer to a host that
/// frees it on another.
#[repr(C)]
pub struct OverlayFrame {
    /// The host's live `&Ui`, erased. Valid only for the duration of the call.
    pub ui: *const c_void,
    /// The host's `ImGuiContext*`.
    pub imgui_context: *mut c_void,
    /// `ImGuiMemAllocFunc` as taken from the host.
    pub alloc_func: *mut c_void,
    /// `ImGuiMemFreeFunc` as taken from the host.
    pub free_func: *mut c_void,
    /// The allocator user-data the host was configured with.
    pub alloc_user_data: *mut c_void,
}

/// Bumped whenever the imgui version behind this ABI changes. Host and guest must agree; see the
/// module docs for why a mismatch is refused rather than tolerated.
///
/// `0x0903` is hudhook 0.9.2 / imgui-sys 0.12 with the [`OverlayFrame`] handoff. The tag was
/// bumped from `0x0902` when the frame gained the imgui context and allocators: a guest built
/// against the older signature would read a bare `&Ui` out of a struct pointer.
pub const OVERLAY_ABI_TAG: u32 = 0x0903;

/// The undecorated export every shell linking this crate must provide, so a guest can find the
/// host without knowing which module won.
pub const REGISTER_EXPORT: &[u8] = b"er_overlay_register_guest_v1\0";

/// True in the one module that won the mutex, set the instant it wins.
///
/// Distinct from [`IS_CONFIRMED_HOST`] on purpose, and the distinction is the whole race. The
/// watermark claims from a spawned thread (hudhook's install takes locks that must not run under
/// the loader lock), so `apply()` finishes some unknown time after the claim. A guest that looks
/// for a host in that window would find nobody, claim the mutex itself, fail because the
/// watermark already holds it, and give up -- which is precisely the vanished-panel bug, merely
/// moved. Designation happens synchronously inside [`claim_owner`], so from the moment the mutex
/// is taken there is always exactly one module answering yes.
static IS_DESIGNATED_HOST: AtomicBool = AtomicBool::new(false);

/// True once that module's `Hudhook::apply()` actually returned `Ok`.
static IS_CONFIRMED_HOST: AtomicBool = AtomicBool::new(false);

/// Guests registered with this module. Only the host's copy is ever non-empty.
static GUESTS: Mutex<Vec<OverlayDrawFn>> = Mutex::new(Vec::new());

/// Guest draws dispatched, so "the host never rendered" and "the host rendered but the guest was
/// never registered" are different numbers instead of the same blank screen.
static GUEST_DISPATCHES: AtomicUsize = AtomicUsize::new(0);

/// Guests refused for an ABI tag this host does not speak.
static GUESTS_REFUSED: AtomicUsize = AtomicUsize::new(0);

/// Called by [`claim_owner`] the instant the mutex is won, before any install is attempted.
pub fn designate_host() {
    IS_DESIGNATED_HOST.store(true, Ordering::SeqCst);
}

/// Confirm the render loop is really installed. Called after `apply()` returns `Ok`.
pub fn become_host() {
    IS_CONFIRMED_HOST.store(true, Ordering::SeqCst);
    designate_host();
}

/// Is this module the one that will host the render loop (installed or about to be)?
pub fn is_host() -> bool {
    IS_DESIGNATED_HOST.load(Ordering::SeqCst)
}

/// Has this module's render loop actually been installed?
pub fn is_confirmed_host() -> bool {
    IS_CONFIRMED_HOST.load(Ordering::SeqCst)
}

/// How many guest draws this host has dispatched.
pub fn guest_dispatches() -> usize {
    GUEST_DISPATCHES.load(Ordering::Relaxed)
}

/// How many guests were refused for an ABI mismatch.
pub fn guests_refused() -> usize {
    GUESTS_REFUSED.load(Ordering::Relaxed)
}

/// Guests currently registered with this module.
pub fn guest_count() -> usize {
    GUESTS.lock().map(|g| g.len()).unwrap_or(0)
}

/// Accept a guest, if this module is the host and speaks its ABI.
///
/// This is the body every shell's `er_overlay_register_guest_v1` export forwards to. Returning
/// `false` is the ordinary answer from every non-host module -- a guest calls this on each loaded
/// module in turn and exactly one says yes.
pub fn register_guest(abi_tag: u32, draw: OverlayDrawFn) -> bool {
    if !is_host() {
        return false;
    }
    if abi_tag != OVERLAY_ABI_TAG {
        GUESTS_REFUSED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    match GUESTS.lock() {
        Ok(mut guests) => {
            // A module that registers twice would draw twice, and the second draw would fight the
            // first for the same pointer state. Idempotent by identity.
            if !guests
                .iter()
                .any(|existing| std::ptr::fn_addr_eq(*existing, draw))
            {
                guests.push(draw);
            }
            true
        }
        Err(_) => false,
    }
}

/// Call every registered guest with the live frame. Host render loops call this once per frame.
pub fn dispatch_guests(ui: &Ui) {
    // Cloned out of the lock before calling: a guest's draw may do anything, and holding the
    // registry lock across foreign code inside `Present` is how a renderer deadlocks.
    let guests: Vec<OverlayDrawFn> = match GUESTS.lock() {
        Ok(guests) => guests.clone(),
        Err(_) => return,
    };
    if guests.is_empty() {
        return;
    }
    let mut alloc_func = None;
    let mut free_func = None;
    let mut alloc_user_data = std::ptr::null_mut();
    // SAFETY: three live out-params; imgui always has allocators set by the time it renders.
    unsafe {
        hudhook::imgui::sys::igGetAllocatorFunctions(
            &mut alloc_func,
            &mut free_func,
            &mut alloc_user_data,
        );
    }
    let frame = OverlayFrame {
        ui: std::ptr::from_ref(ui).cast::<c_void>(),
        // SAFETY: called from inside the host's own render, so a context is current.
        imgui_context: unsafe { hudhook::imgui::sys::igGetCurrentContext() }.cast::<c_void>(),
        alloc_func: alloc_func.map_or(std::ptr::null_mut(), |f| f as *mut c_void),
        free_func: free_func.map_or(std::ptr::null_mut(), |f| f as *mut c_void),
        alloc_user_data,
    };
    for guest in guests {
        // SAFETY: `frame` outlives the call, and the guest accepted OVERLAY_ABI_TAG at
        // registration, so it reads the layout this crate wrote.
        unsafe { guest(&raw const frame) };
    }
    GUEST_DISPATCHES.fetch_add(1, Ordering::Relaxed);
}

/// Register `draw` with whichever loaded module hosts the overlay.
///
/// Walks the process's module list and offers the guest to each in turn; exactly one -- the host
/// -- accepts. Returns whether a host took it. A `false` return means no module in this process
/// hosts an overlay yet, which is the caller's cue to host it itself.
#[cfg(windows)]
pub fn register_with_host(draw: OverlayDrawFn) -> bool {
    type RegisterFn = unsafe extern "C" fn(u32, OverlayDrawFn) -> bool;

    for module in er_game_base::build_id::loaded_module_handles() {
        // SAFETY: a handle straight out of the loader's own module list, and a NUL-terminated
        // export name. A module without the export answers None.
        let Some(symbol) =
            (unsafe { er_game_base::build_id::module_export(module, REGISTER_EXPORT) })
        else {
            continue;
        };
        // SAFETY: this export is defined only by `export_overlay_host!` in this crate, so any
        // module answering to the name has our signature.
        let register: RegisterFn = unsafe { std::mem::transmute(symbol) };
        // SAFETY: FFI into a sibling module of this workspace; it only records the pointer.
        if unsafe { register(OVERLAY_ABI_TAG, draw) } {
            return true;
        }
    }
    false
}

#[cfg(not(windows))]
pub fn register_with_host(_draw: OverlayDrawFn) -> bool {
    false
}

/// Register `draw` with a host that is known to exist, waiting out its designation.
///
/// Call this only after [`crate::OverlayClaim::LostToAnotherModule`], which proves some other
/// module created the ownership mutex. That module calls [`designate_host`] a handful of
/// instructions after the `CreateMutexW` this caller observed -- so the answer is almost always
/// yes on the first attempt, but "almost always" is exactly the gap that a single try loses to,
/// and losing it means a permanently blank overlay.
///
/// Bounded by [`er_game_base::wait::poll_until`], which spins in user space and reaches no
/// wineserver, so a host that somehow never designates itself costs a bounded spin on this
/// module's own install thread rather than the process.
#[cfg(windows)]
pub fn register_with_host_retrying(draw: OverlayDrawFn) -> bool {
    er_game_base::wait::poll_until(|| register_with_host(draw).then_some(())).is_some()
}

#[cfg(not(windows))]
pub fn register_with_host_retrying(_draw: OverlayDrawFn) -> bool {
    false
}

/// Adopt the host's imgui context and allocators, then hand back its `&Ui`.
///
/// Every guest draw calls this first. Skipping it is not a subtle degradation: imgui's context is
/// a per-DLL global, so the guest's copy is NULL and the first `ui.io()` faults.
///
/// # Safety
///
/// `frame` must be the pointer the host just passed, and the returned reference must not outlive
/// the call.
#[cfg(windows)]
pub unsafe fn adopt_frame<'a>(frame: *const OverlayFrame) -> Option<&'a Ui> {
    if frame.is_null() {
        return None;
    }
    // SAFETY: the host passes a live `OverlayFrame` for the duration of the call.
    let frame = unsafe { &*frame };
    if frame.ui.is_null() || frame.imgui_context.is_null() {
        return None;
    }
    // SAFETY: adopting the host's context and allocator globals into this module's copies, which
    // is the documented way to drive imgui from more than one DLL.
    unsafe {
        hudhook::imgui::sys::igSetCurrentContext(frame.imgui_context.cast());
        if !frame.alloc_func.is_null() && !frame.free_func.is_null() {
            hudhook::imgui::sys::igSetAllocatorFunctions(
                Some(std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(usize, *mut c_void) -> *mut c_void,
                >(frame.alloc_func)),
                Some(std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut c_void, *mut c_void),
                >(frame.free_func)),
                frame.alloc_user_data,
            );
        }
        Some(&*(frame.ui.cast::<Ui>()))
    }
}

/// Define this module's `er_overlay_register_guest_v1` export.
///
/// Every shell that links this crate must invoke this once, or it becomes a host that no guest
/// can find -- the exact silent failure this module exists to remove.
#[macro_export]
macro_rules! export_overlay_host {
    () => {
        /// # Safety
        ///
        /// Called across a DLL boundary by [`er_build_watermark_core::overlay_host`].
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn er_overlay_register_guest_v1(
            abi_tag: u32,
            draw: $crate::overlay_host::OverlayDrawFn,
        ) -> bool {
            $crate::overlay_host::register_guest(abi_tag, draw)
        }
    };
}
