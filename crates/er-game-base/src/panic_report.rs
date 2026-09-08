//! Make a Rust panic in a mod DLL say where it came from.
//!
//! # The gap this closes
//!
//! A `panic!` inside a cdylib loaded into ELDEN RING is, by default, invisible. The message goes
//! to `stderr`, which under me3 + Proton is nobody's file; the unwind then crosses an
//! `extern "system"` boundary -- a render-loop callback, a detour handler -- and the process
//! aborts. What reaches an investigator is a `0xe06d7363` record with `cpp_throw_type=rust_panic`
//! and a stack scan, which names the module and nothing else.
//!
//! Measured, 2026-08-29. `er_build_watermark.dll` panicked 229 ms after the first backbuffer draw
//! and took the boot with it. `er-crash-latest.txt` identified the module and the throw type; the
//! backtrace symbolised to `core::result::unwrap_failed` and then to `??`, because the frames
//! below it were a raw stack scan rather than an unwind. Three `.expect()` calls in the
//! statically-linked renderer could have produced exactly that, and nothing on disk could say
//! which -- for a panic that had already been carried as unexplained for weeks.
//!
//! # What this does instead
//!
//! [`report_panics_to`] installs a `std::panic` hook that writes the payload and the source
//! location into the calling module's own log before the unwind starts, then chains to whatever
//! hook was already installed. The message is the part that matters: `.expect("D3DCompile")` and
//! `.expect("D3D12SerializeRootSignature")` are one line apart in the same file and produce
//! identical stack scans, and only the payload tells them apart.
//!
//! It does not stop the process dying. A panic unwinding out of an `extern "system"` callback is
//! already an abort by the time any hook runs, and pretending otherwise would be worse than the
//! silence -- see [`report_panics_to`]'s own note.

use core::sync::atomic::{AtomicBool, Ordering};

/// One install per module. Two calls would chain the hook to itself.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Log every panic in this module, with its message and source location, before it unwinds.
///
/// `module` names the DLL in the line, because several of these logs get read side by side and
/// "panicked at src/lib.rs:88" is ambiguous across seventeen shells that all have a `src/lib.rs`.
/// `log` is the module's own append-only logger -- the same `fn` it hands to
/// `er_hook::set_hook_logger`.
///
/// # What it cannot do
///
/// This does not make the panic survivable. Catching would mean a `catch_unwind` at the boundary
/// the panic crosses, and that boundary belongs to a statically-linked renderer inside somebody
/// else's `extern "system"` callback. What it buys is the message, which is what turns "a module
/// panicked" into "this line panicked" -- and a named failure can be fixed, where an anonymous
/// one gets rediscovered.
///
/// Idempotent: the second call on a module is a no-op rather than a hook chained to itself.
pub fn report_panics_to(module: &'static str, log: fn(core::fmt::Arguments<'_>)) {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = match info.location() {
            Some(at) => format!("{}:{}:{}", at.file(), at.line(), at.column()),
            None => "<no location>".to_string(),
        };
        // The payload is a `&str` for `panic!("literal")` and a `String` for a formatted one, and
        // `.expect(msg)` on a Result produces the latter. Both shapes have to be read or the
        // message this exists for is the one that goes missing.
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        log(format_args!(
            "PANIC in {module} at {location}: {message} -- this unwinds out of a callback the \
             game owns, so the process is about to die; the line above is where to look"
        ));
        previous(info);
    }));
}
