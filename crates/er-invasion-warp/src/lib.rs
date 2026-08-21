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
//! * [`param_row`] -- the DLL-owned synthetic `BonfireWarpParam` row a pin needs behind it: the
//!   row constructor reads the entity id, icon, category bits and all 8 labels out of one.
//! * [`map_surface`] -- which invasion points become world-map pins, and the private
//!   bonfire-entity-id band that lets a confirm hook recognise one of ours and map it back to a
//!   target. Pure and offline-testable.
//! * [`select`] -- which target to warp to: nearest-to-the-player and a stable cycle, ranked
//!   over targets the engine's coordinate conversion already accepted. Pure and offline-testable.
//! * [`warp`] -- the warp itself: the `TriggerAreaReload` sequence (block id + block-local xyz +
//!   euler yaw + stage kick) run against a chosen invasion target instead of the current spot.
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
pub mod join_progress;
pub mod keybind;
pub use invasion_warp::*;

pub mod live_read;
pub use live_read::*;

/// Invasion spawn points for the maps the `.aip` table does not cover -- every legacy dungeon,
/// cave, catacomb and tunnel. Not re-exported at the crate root: [`msb_invasion_points`] and
/// [`aip`] both define a point type, and glob-importing two of them is how the wrong one gets
/// used silently.
pub mod msb_invasion_points;

/// Where each legacy dungeon sits on the world map, read live so a marker can be offered for a
/// dungeon the player has never entered. Not re-exported for the same reason as
/// [`msb_invasion_points`]: it carries its own region type.
pub mod legacy_map_regions;
pub mod lobby_pool;
pub mod local_invasion;
pub mod local_invasion_config;

/// What destination a SEAMLESS invasion actually chose, read out of `CSGameMan` after the fact.
/// Not re-exported: it carries its own reading type, and Seamless's placement path has nothing
/// to do with the `.aip`/MSB tables the rest of this crate reads.
pub mod seamless_invade_probe;

pub mod oracles;
pub use oracles::*;

pub mod sampler;
pub use sampler::*;

pub mod map_surface;
pub use map_surface::*;

pub mod param_row;
pub mod reject_notice;
pub use param_row::*;

pub mod select;
pub use select::*;

pub mod warp;
pub use warp::*;
