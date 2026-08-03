//! The dependency-injection seam between this feature crate and its host DLL.
//!
//! Same pattern as `er_loading_portrait::host` and `er_quit_menu::host`: function pointers
//! installed once at DLL attach, neutral defaults until then, crate-internal wrappers
//! bearing the names the feature code calls. Until a host installs, every seam answers a
//! neutral default (logging is a no-op, the feature gate is off), so the crate is INERT
//! rather than wrong.
//!
//! # Why this seam is small
//!
//! The repo rule is that a seam entry is legitimate only when a consumer OUTSIDE the feature
//! genuinely needs the thing; a cross-call whose consumers all live inside the feature is a
//! MOVE, not a seam entry. This feature is self-contained -- it reads one engine singleton
//! and owns its own catalog -- so the only things that genuinely cross are the host's log
//! sink and the host's decision that the feature is on at all. Nothing here reaches back
//! into `er-effects-rs`, and nothing may be added speculatively: an entry lands with the
//! slice that first calls it.

use std::sync::OnceLock;

/// Host callbacks the invasion-warp feature reads through the seam. Every field has a
/// neutral default (see [`InvasionWarpHost::defaults`]); hosts overwrite the ones they own.
#[derive(Clone, Copy)]
pub struct InvasionWarpHost {
    /// Structured debug logging sink (the product's `append_autoload_debug`).
    pub append_autoload_debug: fn(std::fmt::Arguments<'_>),
    /// Product gate: is the world map allowed to offer invasion-spawn warp targets?
    ///
    /// Owned by the host, not the feature, because it is a profile-level decision (which
    /// DLLs a me3 profile loads and what the product's own state says), and because the repo
    /// forbids introducing a new per-feature env/marker gate to answer it.
    pub invasion_warp_enabled: fn() -> bool,
    /// Publish the feature's oracle telemetry document (the string
    /// [`crate::oracles::catalog_oracle_json`] builds).
    ///
    /// The feature owns the CONTENT; the host owns WHERE it lands, which is genuinely a host
    /// decision and the reason this crosses the seam: the standalone shell writes its own file
    /// next to the executable, while a host that already owns a telemetry document would fold
    /// the fields into that instead. Lands with the catalog slice that first calls it.
    pub publish_oracle_json: fn(&str),
}

fn default_log(_args: std::fmt::Arguments<'_>) {}
fn default_gate_off() -> bool {
    false
}
fn default_publish_oracle_json(_body: &str) {}

impl InvasionWarpHost {
    /// Neutral defaults: no-op logging, feature off, telemetry dropped.
    pub const fn defaults() -> Self {
        Self {
            append_autoload_debug: default_log,
            invasion_warp_enabled: default_gate_off,
            publish_oracle_json: default_publish_oracle_json,
        }
    }
}

impl Default for InvasionWarpHost {
    fn default() -> Self {
        Self::defaults()
    }
}

static DEFAULT_HOST: InvasionWarpHost = InvasionWarpHost::defaults();
static HOST: OnceLock<InvasionWarpHost> = OnceLock::new();

/// Install the host seam ONCE, at DLL attach, BEFORE any hook install or task spawn can run
/// feature code. Returns false (and changes nothing) if a host was already installed.
pub fn install_host(host: InvasionWarpHost) -> bool {
    HOST.set(host).is_ok()
}

fn host() -> &'static InvasionWarpHost {
    HOST.get().unwrap_or(&DEFAULT_HOST)
}

// --- crate-internal wrappers bearing the names the feature code calls -------------------

pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    (host().append_autoload_debug)(args)
}

/// True when the host says the invasion-spawn warp surface may be offered.
#[must_use]
pub fn invasion_warp_enabled() -> bool {
    (host().invasion_warp_enabled)()
}

/// Hand the host a finished oracle telemetry document to store wherever it keeps them.
pub(crate) fn publish_oracle_json(body: &str) {
    (host().publish_oracle_json)(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_host_the_crate_is_inert_rather_than_wrong() {
        // Deliberately does NOT install a host: a crate loaded into a process that never
        // called install_host must answer "feature off", never "feature on".
        assert!(!invasion_warp_enabled());
        // And logging / publishing through the default sinks must not panic.
        append_autoload_debug(format_args!("inert"));
        publish_oracle_json("{}");
    }
}
