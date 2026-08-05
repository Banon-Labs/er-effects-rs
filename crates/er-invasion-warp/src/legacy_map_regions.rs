//! Every legacy dungeon the world map can project, and where each one sits on it.
//!
//! # Why this exists
//!
//! The `.aip` table covers areas 60 and 61 only, so Leyndell, Stormveil, Farum Azula, the
//! Haligtree and every cave/catacomb/tunnel contribute no invasion markers from it. Their
//! invasion points live in each map's MSB, and an MSB is only readable while its map is
//! RESIDENT -- so the player had to physically walk into a dungeon before it could show a
//! marker.
//!
//! The expensive-looking way out is to read every map file off disk. That is both a big job
//! and the wrong answer: a mod that rewrites invasion locations at runtime would make the disk
//! copy confidently wrong.
//!
//! This is the cheap and correct way. `CS::WorldMapLegacyConverter` already holds, in live
//! memory, one entry per legacy block: the block id, the overworld block it projects into, and
//! the map-space origin of that projection. The engine keeps it because the world map has to
//! draw legacy dungeons whether or not the player has been to them.
//!
//! # The entry that makes this work
//!
//! `ConvertLegacyDungeonPositionToOverworldPositionForMap` (`0x1408775e0`) is, for a
//! non-overworld block, a lookup followed by a PURE ADD:
//!
//! ```text
//! *blockIdOut = node->overrideBlockId;
//! *coordsOut  = coordsIn + node->centerMapCoordinates;
//! ```
//!
//! `coordsIn` is only an offset. Feeding it `(0, 0, 0)` therefore yields exactly
//! `centerMapCoordinates` -- the dungeon's own centre in world-map space -- with no knowledge
//! of anything inside the dungeon. That is precisely what a marker for an unvisited dungeon
//! needs, and it is why this module reads the table directly instead of calling the converter
//! with a made-up position.
//!
//! # Shape
//!
//! The table is an MSVC `std::map` and there are two indirections to get wrong, both of which
//! yield a readable pointer that is not a tree -- so both fail as an EMPTY table rather than a
//! crash, which is why the census log exists:
//!
//! 1. `WorldMapLegacyConverter+0x08` is the container itself, embedded, not a pointer to one.
//!    Its `root` member is at `+0x08` within it, so the head is at `+0x10`. (`+0x08` is the
//!    container's allocator.)
//! 2. That `root` is the HEAD SENTINEL, not the root: the real root is `head->parent`, the
//!    head's `isNull` byte is set, and every leaf's child pointers lead back to the head.
//!
//! `ConvertLegacyDungeonPositionToOverworldPositionForMap` does exactly this:
//! `pWVar5 = (param_1->blockCoordinates).root; if (pWVar5->parent->isNull == false) ...`,
//! and returns false when the search lands back on `pWVar5` -- the miss.

use crate::invasion_warp::BlockKey;

/// `CS::WorldMapAreaConverter+0x28` -- the `WorldMapLegacyConverter` this converter owns.
pub const AREA_CONVERTER_LEGACY_OFFSET: usize = 0x28;

/// `CS::WorldMapLegacyConverter+0x10` -- the `std::map` head sentinel.
///
/// TWO corrections live in this one number, and getting either wrong reads a valid pointer that
/// is not a tree and yields an empty table:
///
/// * `+0x08` is not the head. It is `blockCoordinates`, an EMBEDDED
///   `WorldMapAreaLegacyConverter` (24 bytes: `allocator` `+0x00`, `root` `+0x08`, `length`
///   `+0x10`), so the head sits at `0x08 + 0x08`. Reading `+0x08` yields the ALLOCATOR pointer.
///   That was the first live run's result: `legacy_offered=0`, no dungeon markers.
/// * `root` is still not the ROOT. It is the head sentinel; the real root is `root->parent`,
///   exactly as `ConvertLegacyDungeonPositionToOverworldPositionForMap` reads it
///   (`pWVar5 = blockCoordinates.root; ... pWVar5->parent`).
pub const LEGACY_TREE_HEAD_OFFSET: usize = 0x10;

/// `CS::WorldMapLegacyConverter+0x18` -- the container's own entry count.
///
/// The engine maintains it, so it is an independent answer to "how many legacy dungeons are
/// there". Comparing it against what the walk collected turns a silent traversal bug into a
/// stated disagreement: equal counts mean the walk saw the whole tree, and a walk that returns
/// fewer is a defect the log names rather than a coverage gap nobody notices.
pub const LEGACY_TREE_LENGTH_OFFSET: usize = 0x18;

/// `WorldMapLegacyConverter+0x08` -- the embedded `WorldMapAreaLegacyConverter` container.
///
/// Named so the two offsets below are visibly DERIVED from the struct layout rather than
/// asserted as bare numbers. Reading this address as the head is the bug that cost a live run.
pub const LEGACY_CONTAINER_OFFSET: usize = 0x08;
/// `root` within that container. `+0x00` is its allocator.
pub const CONTAINER_ROOT_OFFSET: usize = 0x08;
/// `length` within that container.
pub const CONTAINER_LENGTH_OFFSET: usize = 0x10;

/// `_Tree_node` layout, shared by every MSVC associative container.
pub const TREE_NODE_LEFT_OFFSET: usize = 0x00;
/// Parent link. On the head sentinel this is the real root.
pub const TREE_NODE_PARENT_OFFSET: usize = 0x08;
/// Right child.
pub const TREE_NODE_RIGHT_OFFSET: usize = 0x10;
/// `_Isnil`. Non-zero marks the sentinel, whose value bytes are uninitialised.
pub const TREE_NODE_IS_NIL_OFFSET: usize = 0x19;

/// `key` -- the legacy dungeon's `BlockId`.
pub const TREE_NODE_KEY_OFFSET: usize = 0x1c;
/// `overrideBlockId` -- the overworld block the dungeon projects into.
pub const TREE_NODE_OVERRIDE_BLOCK_OFFSET: usize = 0x20;
/// `centerMapCoordinates` -- `FloatVector3`, the additive map-space origin.
pub const TREE_NODE_CENTER_OFFSET: usize = 0x24;
/// Node stride, from the dump's type database.
pub const TREE_NODE_STRIDE: usize = 0x30;

/// Hard bound on nodes visited.
///
/// A cycle in a corrupted tree would otherwise spin the game thread forever. The shipped table
/// is on the order of a hundred entries, so this is far above any real value while still being
/// a bound -- it exists to make the walk terminate, not to cap coverage.
pub const MAX_TREE_NODES: usize = 4096;

/// One legacy dungeon, as the world map understands it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyMapRegion {
    /// The legacy dungeon's own block.
    pub block: BlockKey,
    /// The overworld block its map projection lands in.
    pub override_block: BlockKey,
    /// Map-space origin of the projection: the dungeon's centre on the world map.
    pub center: [f32; 3],
}

/// A node as read out of process memory, before it is interpreted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawTreeNode {
    pub left: usize,
    pub right: usize,
    pub is_nil: bool,
    pub key: u32,
    pub override_block: u32,
    pub center: [f32; 3],
}

/// Walk the tree from `root`, collecting every non-sentinel entry.
///
/// `head` is the sentinel address, which every leaf points back to and which must never be
/// interpreted as an entry. `read` returns `None` for an unreadable address, and an unreadable
/// node prunes its subtree rather than aborting the walk: a single bad pointer should cost the
/// dungeons under it, not the whole table.
///
/// Split from the memory read so the traversal rules -- sentinel handling, cycle bound, prune
/// on unreadable -- are testable without a game.
pub fn walk_tree(
    head: usize,
    root: usize,
    read: &mut dyn FnMut(usize) -> Option<RawTreeNode>,
) -> Vec<LegacyMapRegion> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut visited = 0usize;

    while let Some(node) = stack.pop() {
        // The head is the universal leaf terminator, so this is the normal exit from a branch,
        // not an error.
        if node == 0 || node == head {
            continue;
        }
        visited += 1;
        if visited > MAX_TREE_NODES {
            break;
        }
        let Some(raw) = read(node) else {
            continue;
        };
        if raw.is_nil {
            continue;
        }
        out.push(LegacyMapRegion {
            block: BlockKey::from_raw(raw.key),
            override_block: BlockKey::from_raw(raw.override_block),
            center: raw.center,
        });
        stack.push(raw.left);
        stack.push(raw.right);
    }

    out.sort_by_key(|region| region.block.raw());
    out.dedup_by_key(|region| region.block.raw());
    out
}

#[cfg(windows)]
mod native {
    use super::{
        AREA_CONVERTER_LEGACY_OFFSET, LEGACY_TREE_HEAD_OFFSET, LEGACY_TREE_LENGTH_OFFSET,
        LegacyMapRegion, RawTreeNode, TREE_NODE_CENTER_OFFSET, TREE_NODE_IS_NIL_OFFSET,
        TREE_NODE_KEY_OFFSET, TREE_NODE_LEFT_OFFSET, TREE_NODE_OVERRIDE_BLOCK_OFFSET,
        TREE_NODE_PARENT_OFFSET, TREE_NODE_RIGHT_OFFSET, walk_tree,
    };

    /// Read one node through the fault-tolerant primitive.
    ///
    /// Every field is required: a node that cannot be read in full is not a node we are willing
    /// to turn into a map marker.
    ///
    /// # Safety
    /// `node` is an address in the game process; all reads are fault-closed.
    unsafe fn read_node(node: usize) -> Option<RawTreeNode> {
        let read_ptr = |offset: usize| unsafe { er_game_base::mem::safe_read_usize(node + offset) };
        let center = {
            let mut center = [0f32; 3];
            for (index, axis) in center.iter_mut().enumerate() {
                *axis = unsafe {
                    er_game_base::mem::safe_read_f32(node + TREE_NODE_CENTER_OFFSET + index * 4)
                }?;
            }
            center
        };
        // `BlockId` is a newtype over `i32`; the catalog keys on the same raw `u32` bit pattern
        // the `.aip` and MSB paths use, so the three sources share one identity space.
        Some(RawTreeNode {
            left: read_ptr(TREE_NODE_LEFT_OFFSET)?,
            right: read_ptr(TREE_NODE_RIGHT_OFFSET)?,
            is_nil: unsafe { er_game_base::mem::safe_read_u8(node + TREE_NODE_IS_NIL_OFFSET) }?
                != 0,
            key: unsafe { er_game_base::mem::safe_read_i32(node + TREE_NODE_KEY_OFFSET) }? as u32,
            override_block: unsafe {
                er_game_base::mem::safe_read_i32(node + TREE_NODE_OVERRIDE_BLOCK_OFFSET)
            }? as u32,
            center,
        })
    }

    /// What the container itself says its entry count is, for cross-checking the walk.
    ///
    /// # Safety
    /// Game thread, `area_converter` a live `CS::WorldMapAreaConverter`.
    #[must_use]
    pub unsafe fn legacy_entry_count_for_converter(area_converter: usize) -> Option<usize> {
        let legacy = unsafe {
            er_game_base::mem::safe_read_usize(area_converter + AREA_CONVERTER_LEGACY_OFFSET)
        }?;
        if legacy == 0 {
            return None;
        }
        unsafe { er_game_base::mem::safe_read_usize(legacy + LEGACY_TREE_LENGTH_OFFSET) }
    }

    /// Every legacy dungeon one `WorldMapAreaConverter` can project.
    ///
    /// # Safety
    /// Game thread, `area_converter` a live `CS::WorldMapAreaConverter`.
    #[must_use]
    pub unsafe fn legacy_regions_for_converter(area_converter: usize) -> Vec<LegacyMapRegion> {
        let Some(legacy) = (unsafe {
            er_game_base::mem::safe_read_usize(area_converter + AREA_CONVERTER_LEGACY_OFFSET)
        }) else {
            return Vec::new();
        };
        if legacy == 0 {
            return Vec::new();
        }
        let Some(head) =
            (unsafe { er_game_base::mem::safe_read_usize(legacy + LEGACY_TREE_HEAD_OFFSET) })
        else {
            return Vec::new();
        };
        if head == 0 {
            return Vec::new();
        }
        // MSVC keeps the real root in the head sentinel's parent slot. Reading the head as the
        // root would hand the walk a node whose key bytes are uninitialised.
        let Some(root) =
            (unsafe { er_game_base::mem::safe_read_usize(head + TREE_NODE_PARENT_OFFSET) })
        else {
            return Vec::new();
        };
        walk_tree(head, root, &mut |node| unsafe { read_node(node) })
    }
}

#[cfg(windows)]
pub use native::{legacy_entry_count_for_converter, legacy_regions_for_converter};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a reader over a synthetic tree keyed by fake addresses.
    fn reader(
        nodes: HashMap<usize, RawTreeNode>,
    ) -> impl FnMut(usize) -> Option<RawTreeNode> + use<> {
        move |address| nodes.get(&address).copied()
    }

    fn node(left: usize, right: usize, key: u32) -> RawTreeNode {
        RawTreeNode {
            left,
            right,
            is_nil: false,
            key,
            override_block: 0x3c00_0000,
            center: [key as f32, 0.0, 0.0],
        }
    }

    const HEAD: usize = 0x1000;

    fn sentinel() -> RawTreeNode {
        RawTreeNode {
            left: 0,
            right: 0,
            is_nil: true,
            key: 0xdead_beef,
            override_block: 0xdead_beef,
            center: [f32::NAN; 3],
        }
    }

    #[test]
    fn the_head_offset_is_derived_from_the_embedded_container_not_asserted() {
        // The first live run read the container's ALLOCATOR as the head and collected nothing:
        // a readable pointer that is not a tree fails as an empty table, not a crash, so nothing
        // but the census log distinguished it from "no dungeons to add".
        assert_eq!(
            LEGACY_TREE_HEAD_OFFSET,
            LEGACY_CONTAINER_OFFSET + CONTAINER_ROOT_OFFSET
        );
        assert_eq!(
            LEGACY_TREE_LENGTH_OFFSET,
            LEGACY_CONTAINER_OFFSET + CONTAINER_LENGTH_OFFSET
        );
        assert_ne!(
            LEGACY_TREE_HEAD_OFFSET, LEGACY_CONTAINER_OFFSET,
            "the container base is the allocator, never the head"
        );
    }

    #[test]
    fn an_empty_tree_yields_nothing() {
        let nodes = HashMap::from([(HEAD, sentinel())]);
        assert!(walk_tree(HEAD, HEAD, &mut reader(nodes)).is_empty());
    }

    #[test]
    fn every_entry_in_a_balanced_tree_is_collected() {
        let nodes = HashMap::from([
            (HEAD, sentinel()),
            (0x2000, node(0x3000, 0x4000, 20)),
            (0x3000, node(HEAD, HEAD, 10)),
            (0x4000, node(HEAD, HEAD, 30)),
        ]);
        let found = walk_tree(HEAD, 0x2000, &mut reader(nodes));
        assert_eq!(
            found.iter().map(|r| r.block.raw()).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
    }

    #[test]
    fn the_head_sentinel_never_becomes_an_entry() {
        // Its key bytes are uninitialised padding; a walk that mistakes the head for the root
        // would emit 0xdeadbeef as a legacy dungeon.
        let nodes = HashMap::from([(HEAD, sentinel()), (0x2000, node(HEAD, HEAD, 7))]);
        let found = walk_tree(HEAD, 0x2000, &mut reader(nodes));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].block.raw(), 7);
    }

    #[test]
    fn a_node_flagged_nil_is_skipped_even_when_it_is_not_the_head() {
        let mut nil_child = sentinel();
        nil_child.left = 0;
        nil_child.right = 0;
        let nodes = HashMap::from([
            (HEAD, sentinel()),
            (0x2000, node(0x3000, HEAD, 5)),
            (0x3000, nil_child),
        ]);
        let found = walk_tree(HEAD, 0x2000, &mut reader(nodes));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].block.raw(), 5);
    }

    #[test]
    fn an_unreadable_node_prunes_its_subtree_without_losing_the_rest() {
        // 0x9999 is absent from the map, so it reads as None. Its sibling must survive.
        let nodes = HashMap::from([
            (HEAD, sentinel()),
            (0x2000, node(0x9999, 0x4000, 20)),
            (0x4000, node(HEAD, HEAD, 30)),
        ]);
        let found = walk_tree(HEAD, 0x2000, &mut reader(nodes));
        assert_eq!(
            found.iter().map(|r| r.block.raw()).collect::<Vec<_>>(),
            vec![20, 30]
        );
    }

    #[test]
    fn a_cycle_terminates_instead_of_hanging_the_game_thread() {
        // A corrupted tree whose child points back at its parent. Without the visit bound this
        // spins forever on the game thread, which is a hang, not a crash -- far worse to debug.
        let nodes = HashMap::from([(HEAD, sentinel()), (0x2000, node(0x2000, HEAD, 1))]);
        let found = walk_tree(HEAD, 0x2000, &mut reader(nodes));
        assert!(found.len() <= MAX_TREE_NODES);
    }

    #[test]
    fn the_same_block_appearing_twice_yields_one_region() {
        let nodes = HashMap::from([
            (HEAD, sentinel()),
            (0x2000, node(0x3000, 0x4000, 42)),
            (0x3000, node(HEAD, HEAD, 42)),
            (0x4000, node(HEAD, HEAD, 43)),
        ]);
        let found = walk_tree(HEAD, 0x2000, &mut reader(nodes));
        assert_eq!(
            found.iter().map(|r| r.block.raw()).collect::<Vec<_>>(),
            vec![42, 43]
        );
    }

    #[test]
    fn regions_come_back_in_block_order_regardless_of_tree_shape() {
        // A degenerate right-leaning chain must produce the same order as a balanced tree.
        let nodes = HashMap::from([
            (HEAD, sentinel()),
            (0x2000, node(HEAD, 0x3000, 30)),
            (0x3000, node(HEAD, 0x4000, 10)),
            (0x4000, node(HEAD, HEAD, 20)),
        ]);
        let found = walk_tree(HEAD, 0x2000, &mut reader(nodes));
        assert_eq!(
            found.iter().map(|r| r.block.raw()).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
    }

    #[test]
    fn the_center_is_carried_through_verbatim() {
        // It is the additive map-space origin; rounding or reordering it moves the marker.
        let mut n = node(HEAD, HEAD, 11);
        n.center = [1234.5, -67.25, 890.125];
        let nodes = HashMap::from([(HEAD, sentinel()), (0x2000, n)]);
        let found = walk_tree(HEAD, 0x2000, &mut reader(nodes));
        assert_eq!(found[0].center, [1234.5, -67.25, 890.125]);
    }

    #[test]
    fn a_null_root_is_an_empty_table_not_a_panic() {
        let nodes = HashMap::from([(HEAD, sentinel())]);
        assert!(walk_tree(HEAD, 0, &mut reader(nodes)).is_empty());
    }
}
