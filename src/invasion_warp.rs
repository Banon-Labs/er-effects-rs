//! Read-only catalog helpers for the world-map invasion-spawn warp feature.
//!
//! The feature goal is local exploration: list fixed invasion spawn locations
//! on the world map and warp to the selected coordinates like a Site of Grace.
//! This module deliberately does **not** start, spoof, or depend on invasion /
//! multiplayer session state. It only turns the engine's existing
//! `CSAutoInvadePoint` singleton into stable Rust records that the map UI layer
//! can later present as synthetic warp targets.

use eldenring::cs::{BlockId, CSAutoInvadePoint};
use fromsoftware_shared::FromStatic;

/// One fixed invasion-spawn location that can become a local map warp target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct InvasionWarpTarget {
    /// Engine block/map id that owns the point.
    pub block_id: i32,
    /// Index within the owning `AutoInvadePointBlockEntry`.
    pub point_index: usize,
    /// World-space position from `AutoInvadePoint.position`.
    pub position: [f32; 3],
    /// Facing angle from `AutoInvadePoint.yaw`.
    pub yaw: f32,
}

impl InvasionWarpTarget {
    #[must_use]
    pub fn new(block_id: BlockId, point_index: usize, position: [f32; 3], yaw: f32) -> Self {
        Self {
            block_id: i32::from(block_id),
            point_index,
            position,
            yaw,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct InvasionWarpCatalogSummary {
    pub block_count: usize,
    pub target_count: usize,
}

impl InvasionWarpCatalogSummary {
    #[must_use]
    pub fn from_targets(targets: &[InvasionWarpTarget]) -> Self {
        let mut block_ids: Vec<i32> = targets.iter().map(|target| target.block_id).collect();
        block_ids.sort_unstable();
        block_ids.dedup();

        Self {
            block_count: block_ids.len(),
            target_count: targets.len(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InvasionWarpCatalogError {
    AutoInvadePointUnavailable(String),
}

impl std::fmt::Display for InvasionWarpCatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AutoInvadePointUnavailable(error) => {
                write!(f, "CSAutoInvadePoint unavailable: {error}")
            }
        }
    }
}

impl std::error::Error for InvasionWarpCatalogError {}

/// Reads the engine's loaded `CSAutoInvadePoint` singleton.
///
/// # Safety
///
/// This must only be called after FromSoftware runtime singletons have been
/// initialized and from a context where reading `CSAutoInvadePoint` is stable.
/// It performs no writes and has no multiplayer/session side effects.
pub(crate) unsafe fn collect_auto_invade_point_targets(
) -> Result<Vec<InvasionWarpTarget>, InvasionWarpCatalogError> {
    // SAFETY: the caller's contract guarantees that the FromSoftware singleton
    // pointer has been initialized and is stable for read-only access.
    let auto_invade_point = unsafe { CSAutoInvadePoint::instance() }
        .map_err(|error| InvasionWarpCatalogError::AutoInvadePointUnavailable(error.to_string()))?;

    Ok(collect_auto_invade_point_targets_from(auto_invade_point))
}

#[must_use]
pub(crate) fn collect_auto_invade_point_targets_from(
    auto_invade_point: &CSAutoInvadePoint,
) -> Vec<InvasionWarpTarget> {
    let mut targets = Vec::new();

    for block_entry in &auto_invade_point.entries {
        let block_id = block_entry.first;
        for (point_index, point) in block_entry.second.items().iter().enumerate() {
            targets.push(InvasionWarpTarget::new(
                block_id,
                point_index,
                [point.position.0, point.position.1, point.position.2],
                point.yaw,
            ));
        }
    }

    targets.sort_by(|left, right| {
        left.block_id
            .cmp(&right.block_id)
            .then(left.point_index.cmp(&right.point_index))
    });
    targets
}

#[cfg(test)]
mod tests {
    use super::{InvasionWarpCatalogSummary, InvasionWarpTarget};

    #[test]
    fn summary_counts_unique_blocks_and_targets() {
        let targets = [
            InvasionWarpTarget {
                block_id: 100,
                point_index: 0,
                position: [1.0, 2.0, 3.0],
                yaw: 0.0,
            },
            InvasionWarpTarget {
                block_id: 100,
                point_index: 1,
                position: [4.0, 5.0, 6.0],
                yaw: 1.0,
            },
            InvasionWarpTarget {
                block_id: 200,
                point_index: 0,
                position: [7.0, 8.0, 9.0],
                yaw: 2.0,
            },
        ];

        assert_eq!(
            InvasionWarpCatalogSummary::from_targets(&targets),
            InvasionWarpCatalogSummary {
                block_count: 2,
                target_count: 3,
            }
        );
    }
}
