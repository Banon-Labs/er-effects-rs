//! Native Elden Ring Scaleform hook plumbing.
//!
//! `er-gfx` owns host-testable GFX bytes and transformations. This crate owns the
//! game-process layer that will install hooks, validate native `MemoryFile` values,
//! replace their buffers, and observe Scaleform resource/message activity as the R24
//! slices move. It is a normal library linked into a host DLL, not another ME3 native.
//!
//! R23 establishes only this ownership and dependency boundary. Hook implementations
//! remain in their current owners until their individual R24 moves and runtime proofs.

#[cfg(windows)]
mod descriptor_guard;
mod host;

#[cfg(windows)]
pub use descriptor_guard::{
    DescriptorGuardInstall, DescriptorGuardInstallError, install_scaleform_descriptor_guard,
};
pub use host::{NamedChildBindEvent, ScaleformHooksHost, install_host};
