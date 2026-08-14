//! er-game-base: shared low-level foundation below the product DLL, the
//! telemetry crate/DLL, and the zero-dep mini-DLLs.
//!
//! Tier A (default, zero external deps): FNV-1a fingerprints, fault-safe RAM readers,
//! game base/rva resolution, the stable singleton RVA/offset table, and a parameterized
//! append-only file logger.
//!
//! Tier B (`game-types` feature, cfg(windows)-gated): a re-export facade over
//! the typed eldenring / fromsoftware-shared accessors so the heavy consumers
//! share one import surface. The mini-DLLs enable tier A only.

pub mod fnv1a;
pub mod log;
pub mod mem;
#[cfg(all(windows, feature = "game-types"))]
pub mod pgd;
pub mod profile_summary;
pub mod rva;

/// Tier B typed-binding re-export facade. Only compiled when `game-types` is
/// enabled (product + er-telemetry); the zero-dep mini-DLLs never pull this in.
#[cfg(all(windows, feature = "game-types"))]
pub mod game_types {
    pub use eldenring;
    pub use fromsoftware_shared;
}
