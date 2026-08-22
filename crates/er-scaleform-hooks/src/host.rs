//! The one dependency-inversion seam from native Scaleform plumbing to feature policy.
//!
//! The hook layer will own the shared named-child detour, but the meaning of a bound
//! child remains with title/ProfileSelect feature code. That concern crosses through
//! one post-bind callback instead of an `er-title-flow` dependency or a mirror of the
//! product DLL's globals. Other descriptor, resource, and message families do not gain
//! speculative callbacks here; R24 adds an entry only when a moved slice proves it is
//! genuinely host-owned.

use std::sync::OnceLock;

/// The native values observed after the original named-child bind has completed.
///
/// The hook layer reports facts only. The installed feature callback may inspect or
/// mutate the resolved proxy according to its own policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamedChildBindEvent {
    /// Native parent scene-object proxy passed to the binder.
    pub parent: usize,
    /// Native output proxy populated by the binder.
    pub out_proxy: usize,
    /// Pointer to the binder's NUL-terminated child name.
    pub name_ptr: usize,
    /// Native return value from the original binder.
    pub result: usize,
}

/// Concern-specific callbacks supplied by the DLL that hosts this library.
#[derive(Clone, Copy)]
pub struct ScaleformHooksHost {
    /// Apply feature-owned policy after a named child has been resolved.
    ///
    /// # Safety
    ///
    /// The callback runs inside the native binder detour. It must treat every pointer
    /// in the event as untrusted game memory and must not retain borrowed pointees.
    pub named_child_bound: unsafe fn(NamedChildBindEvent),
}

unsafe fn ignore_named_child(_event: NamedChildBindEvent) {}

impl ScaleformHooksHost {
    /// Neutral defaults: no feature policy is applied until a host installs one.
    pub const fn defaults() -> Self {
        Self {
            named_child_bound: ignore_named_child,
        }
    }
}

impl Default for ScaleformHooksHost {
    fn default() -> Self {
        Self::defaults()
    }
}

static DEFAULT_HOST: ScaleformHooksHost = ScaleformHooksHost::defaults();
static HOST: OnceLock<ScaleformHooksHost> = OnceLock::new();

/// Install the host seam once, before any Scaleform hook is installed.
///
/// Returns `false` without changing the active host when one is already installed.
pub fn install_host(host: ScaleformHooksHost) -> bool {
    HOST.set(host).is_ok()
}

fn host() -> &'static ScaleformHooksHost {
    HOST.get().unwrap_or(&DEFAULT_HOST)
}

/// Dispatch a completed native named-child bind to feature-owned policy.
///
/// # Safety
///
/// The caller must provide the untouched values from a live native binder invocation.
// The dispatch half of the seam this crate exists to define: `install_host` is public and
// exported, but its only in-crate caller is the shared named-child detour, which does not
// move here until R24. Kept (with `host`/`DEFAULT_HOST`, live only through it) so the seam
// stays whole and unit-tested rather than becoming a write-only sink.
#[allow(dead_code)]
pub(crate) unsafe fn named_child_bound(event: NamedChildBindEvent) {
    unsafe { (host().named_child_bound)(event) }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static LAST_RESULT: AtomicUsize = AtomicUsize::new(0);

    unsafe fn record(event: NamedChildBindEvent) {
        CALLS.fetch_add(1, Ordering::SeqCst);
        LAST_RESULT.store(event.result, Ordering::SeqCst);
    }

    #[test]
    fn host_is_inert_until_installed_and_cannot_be_replaced() {
        let event = NamedChildBindEvent {
            parent: 1,
            out_proxy: 2,
            name_ptr: 3,
            result: 4,
        };

        unsafe { named_child_bound(event) };
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);

        assert!(install_host(ScaleformHooksHost {
            named_child_bound: record,
        }));
        unsafe { named_child_bound(event) };
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(LAST_RESULT.load(Ordering::SeqCst), event.result);

        assert!(!install_host(ScaleformHooksHost::defaults()));
        unsafe { named_child_bound(event) };
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }
}
