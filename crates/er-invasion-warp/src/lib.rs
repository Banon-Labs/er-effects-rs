//! World-map invasion-spawn warp targets (bd `er-effects-rs-5es`).
//!
//! The feature is a LOCAL exploration surface. Elden Ring already ships a table of fixed
//! auto-invasion spawn coordinates (`other:/AutoInvadePoint.aipbnd`, loaded into the
//! `CSAutoInvadePoint` singleton). This crate turns that table into stable Rust records so
//! the world-map warp UI can offer them as selectable targets and warp locally by
//! BlockId/position/yaw, the same affordance a Site of Grace uses.
//!
//! # Hard boundary
//!
//! Nothing here fakes an invasion, starts or spoofs multiplayer/session state, or touches
//! host/guest behaviour. `CSAutoInvadePoint` is a coordinate table and is read ONLY. The
//! engine's own consumer of that table (`CSBreakInPointManager` /
//! `CS::QuickmatchManager`) is deliberately NOT called: that path is where session state
//! lives. The coordinate math here was read out of it statically and reimplemented, so no
//! multiplayer code is entered.
//!
//! # Modules
//!
//! * [`invasion_warp`] -- the catalog: `CSAutoInvadePoint` -> sorted, deduped
//!   [`InvasionWarpTarget`] records, block grouping, summary, and the block-local ->
//!   world-space coordinate math.
//! * [`aip`] -- the on-disk `.aip` record decoder, reverse-engineered from
//!   `CS::CSAutoInvadePoint::AddForBlockId`. Lets the catalog be validated against the local
//!   extraction corpus offline, with no game running.
//! * [`live_read`] -- the FAIL-CLOSED walk of the live singleton's red-black tree. Every
//!   pointer is plausibility-checked and read through a fault-tolerant primitive, and the walk
//!   carries a hard visit budget, because this runs inside the user's game where a crash or a
//!   hung game thread is a far worse outcome than a missing oracle.
//! * [`sampler`] -- the driver that keeps re-reading until the totals settle, so a catalog
//!   caught mid-load is never reported as the final answer, and ORACLE 1 lands.
//! * [`oracles`] -- the RAM/telemetry semaphore names the eventual runtime proof must go
//!   green on. A rendered/behavioural feature is never proven by build or launch success.
//! * [`host`] -- the dependency-injection seam back to whichever DLL hosts this crate.
//!
//! Every reverse-engineered address cited in this crate is byte-checked against
//! `eldenring-deobf.bin` (`python3 scripts/check-dump-deobf-identity.py <va>`); see
//! `docs/plans/world-map-invasion-warp.md` for the evidence table.

pub mod host;
pub use host::*;

pub mod aip;
pub use aip::*;

pub mod invasion_warp;
pub use invasion_warp::*;

pub mod live_read;
pub use live_read::*;

pub mod oracles;
pub use oracles::*;

pub mod sampler;
pub use sampler::*;
