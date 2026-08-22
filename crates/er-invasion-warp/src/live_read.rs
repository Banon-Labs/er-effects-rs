//! FAIL-CLOSED read of the live `CSAutoInvadePoint` red-black tree.
//!
//! # Why this is not `for entry in &singleton.entries`
//!
//! The typed binding (`eldenring::cs::CSAutoInvadePoint`) walks the `DLMap` by dereferencing
//! every node pointer directly and builds each block's point slice with
//! `slice::from_raw_parts(head, count)`. That is correct once the container is mounted and
//! catastrophic before it: a half-initialised node, a torn `count`, or a stale pointer becomes
//! an access violation inside the user's live game. A cyclic/corrupt tree is worse still --
//! the successor walk never terminates and the GAME THREAD hangs.
//!
//! This module reads the same structure through a fault-tolerant [`CatalogMemory`] (in the DLL,
//! `ReadProcessMemory` against our own process, which returns FALSE instead of raising on
//! unmapped memory) and refuses anything implausible. Every failure mode is a `Result`, never a
//! fault and never an unbounded loop:
//!
//! * an unreadable address anywhere -> `AutoInvadePointUnavailable`
//! * a node/point pointer outside the plausible user-address range, or misaligned -> the same
//! * `size`, a block's point count, or the running target total past its ceiling -> the same
//! * more nodes visited than the tree claims to hold (a cycle, or a torn rotation) -> the same
//! * a non-finite coordinate -> the same
//! * the tree structurally empty (`size == 0`, or the root is the sentinel) -> `CatalogEmpty`,
//!   which is a TIMING answer: the `.aipbnd` files have not been processed yet, ask again later
//!
//! The walk is an explicit-stack DFS with a hard visit budget, so it cannot recurse and cannot
//! spin. It ends by asserting that the number of value nodes it visited EQUALS the `size` the
//! map itself carries -- the cheapest available check that the read did not race the loader.
//!
//! That last check is load-bearing rather than belt-and-braces: the `.aipbnd` entries are fed in
//! one `AddForBlockId` at a time while the game boots, so a walk CAN overlap an insert and the
//! rebalancing rotations it performs. Nothing is freed during a build-up, so the worst a
//! concurrent rotation can do is make the walk visit a node twice or miss one -- and both show
//! up as `value_nodes != count`, which returns an error the sampler simply retries. It cannot
//! silently produce a plausible-but-wrong total, which is precisely what oracle 1 must not do.
//!
//! # Layout provenance (ELDEN RING 1.16.2; dump VA == deobf VA == runtime VA, shift 0)
//!
//! Offsets are the 1.16.2 Ghidra structures, cross-checked against the `fromsoftware-rs`
//! bindings this crate would otherwise use (`DLMap` = MSVC `std::map`, `AutoInvadePointBlockEntry`,
//! `AutoInvadePoint`) and against `CSAutoInvadePoint`'s own lookup-by-`BlockId`
//! (`0x140a693e0`), which walks `head->parent` down `left`/`right` testing `isNull`.
//!
//! | Ghidra type | field | offset | mirror in `fromsoftware-rs` |
//! |---|---|---|---|
//! | `CS::CSAutoInvadePoint` | `allocator` | `0x00` | `RbTree::allocator` |
//! | | `head` | `0x08` | `RbTree::head` |
//! | | `count` | `0x10` | `RbTree::size` |
//! | `CSAutoInvadePointTreeNode` (size `0x38`) | `left` | `0x00` | `RbNode::left` |
//! | | `parent` | `0x08` | `RbNode::parent` |
//! | | `right` | `0x10` | `RbNode::right` |
//! | | `color` | `0x18` | `RbNode::color` |
//! | | `isNull` | `0x19` | `RbNode::is_nil` |
//! | | `data` | `0x20` | `RbNode::value` (`Pair<BlockId, ..>`) |
//! | `CSAutoInvadePointNodeData` (size `0x18`) | `blockId` | `0x00` | `Pair::first` |
//! | | `pointCount` | `0x08` | `AutoInvadePointBlockEntry::count` |
//! | | `points` | `0x10` | `AutoInvadePointBlockEntry::head` |
//!
//! One point is a `FloatVector4`: `x`, `y`, `z`, `yaw`, matching the on-disk `.aip` record
//! ([`crate::aip::AIP_POINT_LEN`]) because `AddForBlockId` (`0x140a69550`) `memcpy`s the file
//! payload verbatim into a `fileSize - 0x10` allocation.
//!
//! # `pointCount` IS 32 BITS, and its upper dword is uninitialised stack
//!
//! This resolves the open risk `docs/plans/world-map-invasion-warp.md` §6 flagged as "not proven
//! either way offline". It is proven now, statically, and it decides how the field is read.
//!
//! `AddForBlockId` (`0x140a69550`) stages the map value on the stack and inserts it:
//!
//! ```text
//! 140a695d5  MOV   ECX, dword ptr [RSI + 0xc]     ; count, a u32 in the .aip header
//! 140a695e9  MOV   dword ptr [RSP + 0x30], ECX    ; 32-BIT store; [RSP+0x34] never written
//! 140a695f0  MOV   qword ptr [RSP + 0x38], RBX    ; the points allocation
//! 140a695f8  MOVUPS XMM0, xmmword ptr [RSP + 0x30]
//! 140a69609  MOVUPS xmmword ptr [RSP + 0x48], XMM0 ; -> pair.pointCount, pair.points
//! ```
//!
//! The 16-byte copy carries `[RSP+0x34]` -- a stack slot this function never writes -- into the
//! upper half of the value's 8-byte `pointCount`. The engine never notices because it only ever
//! reads the low dword: `_GetCurBreakInPointVecFromAutoIntrudePoint` (`0x140a0c4f0`) tests
//! `0 < (int)count` and loops `while ((int)i < (int)count)`.
//!
//! So this module reads `pointCount` as a **u32** and IGNORES the upper dword. Two consequences
//! worth stating plainly:
//!
//! * reading it as the `usize` the `fromsoftware-rs` binding declares would pick up that garbage
//!   and reject every block (or, via that binding's `slice::from_raw_parts`, fault immediately);
//! * a stale-stack upper dword is normal engine behaviour here, NOT evidence of corruption, so
//!   it must not fail the read.

use crate::aip::AIP_POINT_LEN;
use crate::invasion_warp::{
    BlockKey, InvasionWarpCatalog, InvasionWarpCatalogError, InvasionWarpTarget,
};

/// A fault-tolerant view of the address space the catalog is read out of.
///
/// The whole safety argument of this module rests on the implementation's contract: `read`
/// MUST return `false` for an unmapped/unreadable range rather than faulting. The DLL satisfies
/// it with `ReadProcessMemory` on the current-process pseudo-handle; the host tests satisfy it
/// with an in-memory image, which is why every branch below is reachable from `cargo test`.
pub trait CatalogMemory {
    /// Read exactly `out.len()` bytes at `addr`, or return `false` having written nothing
    /// meaningful. Must never fault, and must never partially succeed and report `true`.
    fn read(&self, addr: u64, out: &mut [u8]) -> bool;
}

// --- structure offsets (see the module docs for provenance) -----------------------------

/// `CSAutoInvadePoint::head` -- the `std::map` sentinel node.
pub const MAP_HEAD_OFFSET: u64 = 0x08;
/// `CSAutoInvadePoint::count` -- the number of value nodes in the tree.
pub const MAP_COUNT_OFFSET: u64 = 0x10;
/// Bytes of the map header this module reads in one go.
pub const MAP_HEADER_LEN: usize = 0x18;

/// `CSAutoInvadePointTreeNode::left`.
pub const NODE_LEFT_OFFSET: usize = 0x00;
/// `CSAutoInvadePointTreeNode::parent` (on the sentinel, this is the ROOT).
pub const NODE_PARENT_OFFSET: usize = 0x08;
/// `CSAutoInvadePointTreeNode::right`.
pub const NODE_RIGHT_OFFSET: usize = 0x10;
/// `CSAutoInvadePointTreeNode::isNull` -- 1 on the sentinel and on every leaf terminator.
pub const NODE_IS_NULL_OFFSET: usize = 0x19;
/// `CSAutoInvadePointTreeNode::data.blockId`.
pub const NODE_BLOCK_ID_OFFSET: usize = 0x20;
/// `CSAutoInvadePointTreeNode::data.pointCount` -- read as a **u32**, see
/// [`read_node`] and the module docs. The upper dword of this 8-byte slot is uninitialised.
pub const NODE_POINT_COUNT_OFFSET: usize = 0x28;
/// `CSAutoInvadePointTreeNode::data.points`.
pub const NODE_POINTS_OFFSET: usize = 0x30;
/// `sizeof(CSAutoInvadePointTreeNode)`.
pub const NODE_LEN: usize = 0x38;

// --- plausibility ceilings ---------------------------------------------------------------

/// Lowest address a heap object can occupy (Windows permanently reserves the low 64 KiB).
pub const MIN_PLAUSIBLE_ADDRESS: u64 = 0x1_0000;
/// Top of the 47-bit user address space.
pub const MAX_PLAUSIBLE_ADDRESS: u64 = 1 << 47;
/// Ceiling on the map's own `count`. The shipped data is 365 blocks
/// ([`crate::aip::AIP_FINGERPRINT_BASE`] + `_DLC02`); anything past this is not a catalog.
pub const MAX_PLAUSIBLE_BLOCKS: u64 = 4096;
/// Ceiling on one block's `pointCount`. The largest shipped block holds 68 points.
pub const MAX_PLAUSIBLE_POINTS_PER_BLOCK: u64 = 1024;
/// Ceiling on the running target total. The shipped data is 7073.
pub const MAX_PLAUSIBLE_TARGETS: usize = 131_072;
/// Slack over `count` for the DFS visit budget: a healthy walk touches each value node once
/// plus its two nil terminators, and a cyclic/torn tree blows straight past this instead of
/// spinning the game thread forever.
pub const NODE_VISIT_BUDGET_MULTIPLIER: u64 = 4;
/// Floor under the visit budget so a tiny tree still has room for its sentinels.
pub const NODE_VISIT_BUDGET_FLOOR: u64 = 64;

/// True when `addr` could be a live 8-byte-aligned heap object in this process.
#[must_use]
pub const fn plausible_object_pointer(addr: u64) -> bool {
    addr >= MIN_PLAUSIBLE_ADDRESS && addr < MAX_PLAUSIBLE_ADDRESS && addr.is_multiple_of(8)
}

fn unavailable(reason: String) -> InvasionWarpCatalogError {
    InvasionWarpCatalogError::AutoInvadePointUnavailable(reason)
}

fn read_u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(word)
}

fn read_u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(word)
}

fn read_f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_bits(read_u32_at(bytes, offset))
}

/// The `{head, count}` pair at the top of the singleton.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MapHeader {
    head: u64,
    count: u64,
}

/// One decoded tree node. Only the fields the walk needs; nothing is dereferenced yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TreeNode {
    left: u64,
    parent: u64,
    right: u64,
    is_null: bool,
    block: BlockKey,
    point_count: u64,
    points: u64,
}

fn read_map_header<M: CatalogMemory>(
    memory: &M,
    singleton: u64,
) -> Result<MapHeader, InvasionWarpCatalogError> {
    if !plausible_object_pointer(singleton) {
        return Err(unavailable(format!(
            "CSAutoInvadePoint pointer 0x{singleton:x} is not a plausible aligned object address"
        )));
    }
    let mut bytes = [0u8; MAP_HEADER_LEN];
    if !memory.read(singleton, &mut bytes) {
        return Err(unavailable(format!(
            "CSAutoInvadePoint header at 0x{singleton:x} is unreadable"
        )));
    }
    Ok(MapHeader {
        head: read_u64_at(&bytes, MAP_HEAD_OFFSET as usize),
        count: read_u64_at(&bytes, MAP_COUNT_OFFSET as usize),
    })
}

/// Decode one node. `pointCount` is read as a **u32** on purpose: `AddForBlockId` stores it with
/// a 32-bit `MOV` and the 16-byte copy that follows drags an uninitialised stack dword into the
/// upper half of the slot, so only the low dword is data. The engine itself reads it the same
/// way (`(int)count`). See the module docs for the disassembly.
fn read_node<M: CatalogMemory>(memory: &M, addr: u64) -> Option<TreeNode> {
    if !plausible_object_pointer(addr) {
        return None;
    }
    let mut bytes = [0u8; NODE_LEN];
    if !memory.read(addr, &mut bytes) {
        return None;
    }
    Some(TreeNode {
        left: read_u64_at(&bytes, NODE_LEFT_OFFSET),
        parent: read_u64_at(&bytes, NODE_PARENT_OFFSET),
        right: read_u64_at(&bytes, NODE_RIGHT_OFFSET),
        is_null: bytes[NODE_IS_NULL_OFFSET] != 0,
        block: BlockKey::from_raw(read_u32_at(&bytes, NODE_BLOCK_ID_OFFSET)),
        point_count: u64::from(read_u32_at(&bytes, NODE_POINT_COUNT_OFFSET)),
        points: read_u64_at(&bytes, NODE_POINTS_OFFSET),
    })
}

/// Decode one block's point array into targets, refusing an implausible count/pointer and any
/// non-finite coordinate.
fn read_block_points<M: CatalogMemory>(
    memory: &M,
    node: &TreeNode,
    into: &mut Vec<InvasionWarpTarget>,
) -> Result<(), InvasionWarpCatalogError> {
    if node.point_count > MAX_PLAUSIBLE_POINTS_PER_BLOCK {
        return Err(unavailable(format!(
            "block {} claims {} points, past the {MAX_PLAUSIBLE_POINTS_PER_BLOCK} ceiling",
            node.block, node.point_count
        )));
    }
    if node.point_count == 0 {
        return Ok(());
    }
    if !plausible_object_pointer(node.points) {
        return Err(unavailable(format!(
            "block {} point array 0x{:x} is not a plausible aligned address",
            node.block, node.points
        )));
    }
    let len = node.point_count as usize * AIP_POINT_LEN;
    let mut bytes = vec![0u8; len];
    if !memory.read(node.points, &mut bytes) {
        return Err(unavailable(format!(
            "block {} point array at 0x{:x} ({len} bytes) is unreadable",
            node.block, node.points
        )));
    }
    for point_index in 0..node.point_count as usize {
        let base = point_index * AIP_POINT_LEN;
        let x = read_f32_at(&bytes, base);
        let y = read_f32_at(&bytes, base + 4);
        let z = read_f32_at(&bytes, base + 8);
        let yaw = read_f32_at(&bytes, base + 12);
        if !(x.is_finite() && y.is_finite() && z.is_finite() && yaw.is_finite()) {
            return Err(unavailable(format!(
                "block {} point {point_index} holds a non-finite coordinate",
                node.block
            )));
        }
        into.push(InvasionWarpTarget::new(
            node.block,
            point_index as u32,
            [x, y, z],
            yaw,
        ));
    }
    Ok(())
}

/// Read the live `CSAutoInvadePoint` at `singleton` into a catalog, or say why not.
///
/// Never faults and never loops unboundedly: see the module docs for the full failure table.
pub fn read_catalog<M: CatalogMemory>(
    memory: &M,
    singleton: u64,
) -> Result<InvasionWarpCatalog, InvasionWarpCatalogError> {
    let header = read_map_header(memory, singleton)?;
    if header.count > MAX_PLAUSIBLE_BLOCKS {
        return Err(unavailable(format!(
            "CSAutoInvadePoint claims {} blocks, past the {MAX_PLAUSIBLE_BLOCKS} ceiling",
            header.count
        )));
    }
    if header.count == 0 {
        return Err(InvasionWarpCatalogError::CatalogEmpty);
    }
    let head = read_node(memory, header.head).ok_or_else(|| {
        unavailable(format!(
            "CSAutoInvadePoint head node 0x{:x} is unreadable or implausible",
            header.head
        ))
    })?;
    if !head.is_null {
        return Err(unavailable(format!(
            "CSAutoInvadePoint head node 0x{:x} is not the sentinel (isNull=0)",
            header.head
        )));
    }
    if head.parent == header.head {
        // A constructed-but-unpopulated map: sentinel is its own root.
        return Err(InvasionWarpCatalogError::CatalogEmpty);
    }

    let budget = header
        .count
        .saturating_mul(NODE_VISIT_BUDGET_MULTIPLIER)
        .max(NODE_VISIT_BUDGET_FLOOR);
    let mut targets: Vec<InvasionWarpTarget> = Vec::new();
    let mut stack: Vec<u64> = vec![head.parent];
    let mut visits: u64 = 0;
    let mut value_nodes: u64 = 0;

    while let Some(addr) = stack.pop() {
        if addr == header.head {
            continue;
        }
        visits += 1;
        if visits > budget {
            return Err(unavailable(format!(
                "walk of CSAutoInvadePoint exceeded its {budget}-node budget for {} blocks -- \
                 the tree is torn or cyclic",
                header.count
            )));
        }
        let node = read_node(memory, addr).ok_or_else(|| {
            unavailable(format!(
                "CSAutoInvadePoint node 0x{addr:x} is unreadable or implausible"
            ))
        })?;
        if node.is_null {
            continue;
        }
        value_nodes += 1;
        read_block_points(memory, &node, &mut targets)?;
        if targets.len() > MAX_PLAUSIBLE_TARGETS {
            return Err(unavailable(format!(
                "CSAutoInvadePoint yielded more than {MAX_PLAUSIBLE_TARGETS} points"
            )));
        }
        stack.push(node.left);
        stack.push(node.right);
    }

    if value_nodes != header.count {
        return Err(unavailable(format!(
            "CSAutoInvadePoint walk found {value_nodes} blocks but the map claims {} -- \
             the read raced the loader",
            header.count
        )));
    }
    InvasionWarpCatalog::try_from_targets(targets)
}

// ---------------------------------------------------------------------------------------
// Windows: the fault-tolerant reader over our own process.
// ---------------------------------------------------------------------------------------

/// [`CatalogMemory`] backed by `ReadProcessMemory` on the current-process pseudo-handle, which
/// reports failure for unmapped/freed pages instead of raising an access violation. This is the
/// repo's standing fault-safe read primitive (`er_game_base::mem`), shared with the product
/// DLL's own address-space probes.
#[cfg(windows)]
pub struct ProcessMemory;

#[cfg(windows)]
impl CatalogMemory for ProcessMemory {
    fn read(&self, addr: u64, out: &mut [u8]) -> bool {
        let Ok(addr) = usize::try_from(addr) else {
            return false;
        };
        // SAFETY: `read_bytes` is itself the fault-tolerant primitive -- it forwards to
        // `ReadProcessMemory`, which validates the range in the kernel and returns FALSE for
        // anything unmapped rather than faulting in our thread.
        unsafe { er_game_base::mem::read_bytes(addr, out) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invasion_warp::AUTHORED_YAW_EXTREME;

    /// An in-memory address space: a flat image mapped at `base`, plus an optional readable
    /// span so a test can model "this pointer is not mapped".
    struct FakeMemory {
        base: u64,
        bytes: Vec<u8>,
    }

    impl FakeMemory {
        fn new(base: u64, len: usize) -> Self {
            Self {
                base,
                bytes: vec![0u8; len],
            }
        }

        fn offset(&self, addr: u64) -> usize {
            (addr - self.base) as usize
        }

        fn put_u64(&mut self, addr: u64, value: u64) {
            let at = self.offset(addr);
            self.bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }

        fn put_u32(&mut self, addr: u64, value: u32) {
            let at = self.offset(addr);
            self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }

        fn put_u8(&mut self, addr: u64, value: u8) {
            let at = self.offset(addr);
            self.bytes[at] = value;
        }

        fn put_f32(&mut self, addr: u64, value: f32) {
            self.put_u32(addr, value.to_bits());
        }
    }

    impl CatalogMemory for FakeMemory {
        fn read(&self, addr: u64, out: &mut [u8]) -> bool {
            if addr < self.base {
                return false;
            }
            let start = (addr - self.base) as usize;
            let Some(end) = start.checked_add(out.len()) else {
                return false;
            };
            if end > self.bytes.len() {
                return false;
            }
            out.copy_from_slice(&self.bytes[start..end]);
            true
        }
    }

    const BASE: u64 = 0x2_0000;
    const SINGLETON: u64 = BASE;
    const HEAD: u64 = BASE + 0x100;
    const NODE_0: u64 = BASE + 0x200;
    const NODE_1: u64 = BASE + 0x300;
    const NODE_2: u64 = BASE + 0x400;
    const POINTS_0: u64 = BASE + 0x800;
    const POINTS_1: u64 = BASE + 0x900;
    const POINTS_2: u64 = BASE + 0xA00;

    /// Write a value node with `count` points at `points`, both children pointing at the
    /// sentinel (a leaf).
    fn write_leaf(memory: &mut FakeMemory, node: u64, block: u32, count: u64, points: u64) {
        memory.put_u64(node + NODE_LEFT_OFFSET as u64, HEAD);
        memory.put_u64(node + NODE_PARENT_OFFSET as u64, HEAD);
        memory.put_u64(node + NODE_RIGHT_OFFSET as u64, HEAD);
        memory.put_u8(node + NODE_IS_NULL_OFFSET as u64, 0);
        memory.put_u32(node + NODE_BLOCK_ID_OFFSET as u64, block);
        memory.put_u64(node + NODE_POINT_COUNT_OFFSET as u64, count);
        memory.put_u64(node + NODE_POINTS_OFFSET as u64, points);
    }

    fn write_points(memory: &mut FakeMemory, at: u64, points: &[([f32; 3], f32)]) {
        for (index, (position, yaw)) in points.iter().enumerate() {
            let base = at + (index * AIP_POINT_LEN) as u64;
            memory.put_f32(base, position[0]);
            memory.put_f32(base + 4, position[1]);
            memory.put_f32(base + 8, position[2]);
            memory.put_f32(base + 12, *yaw);
        }
    }

    /// Three blocks: root NODE_0 with children NODE_1 (left) and NODE_2 (right).
    /// 2 + 1 + 3 = 6 points across areas 60 and 61.
    fn healthy_map() -> FakeMemory {
        let mut memory = FakeMemory::new(BASE, 0x1000);
        memory.put_u64(SINGLETON + MAP_HEAD_OFFSET, HEAD);
        memory.put_u64(SINGLETON + MAP_COUNT_OFFSET, 3);
        // Sentinel: isNull=1, parent == root.
        memory.put_u64(HEAD + NODE_PARENT_OFFSET as u64, NODE_0);
        memory.put_u64(HEAD + NODE_LEFT_OFFSET as u64, NODE_1);
        memory.put_u64(HEAD + NODE_RIGHT_OFFSET as u64, NODE_2);
        memory.put_u8(HEAD + NODE_IS_NULL_OFFSET as u64, 1);

        write_leaf(&mut memory, NODE_0, 0x3C22_3300, 2, POINTS_0);
        memory.put_u64(NODE_0 + NODE_LEFT_OFFSET as u64, NODE_1);
        memory.put_u64(NODE_0 + NODE_RIGHT_OFFSET as u64, NODE_2);
        write_leaf(&mut memory, NODE_1, 0x3C21_2800, 1, POINTS_1);
        write_leaf(&mut memory, NODE_2, 0x3D2C_2900, 3, POINTS_2);

        write_points(
            &mut memory,
            POINTS_0,
            &[
                ([103.79, 456.07, -66.27], -1.09),
                ([1.0, 2.0, 3.0], AUTHORED_YAW_EXTREME),
            ],
        );
        write_points(&mut memory, POINTS_1, &[([4.0, 5.0, 6.0], 0.0)]);
        write_points(
            &mut memory,
            POINTS_2,
            &[
                ([7.0, 8.0, 9.0], -0.5),
                ([10.0, 11.0, 12.0], -1.5),
                ([13.0, 14.0, 15.0], -2.5),
            ],
        );
        memory
    }

    #[test]
    fn a_healthy_tree_reads_back_every_block_and_point_in_catalog_order() {
        let catalog = read_catalog(&healthy_map(), SINGLETON).expect("healthy tree must read");
        let summary = catalog.summary();
        assert_eq!(summary.block_count, 3);
        assert_eq!(summary.target_count, 6);
        assert_eq!(summary.area_count, 2);
        // Catalog order is (block, point_index) ascending regardless of DFS order.
        let first = catalog.targets().first().expect("non-empty");
        assert_eq!(first.block.to_string(), "m60_33_40_00");
        assert_eq!(first.point_index, 0);
        // The coordinates survive the decode intact.
        let m60_34 = catalog.targets_in_block(BlockKey::from_raw(0x3C22_3300));
        assert_eq!(m60_34.len(), 2);
        assert!((m60_34[0].position[0] - 103.79).abs() < 1e-3);
        assert!((m60_34[0].yaw - -1.09).abs() < 1e-3);
    }

    #[test]
    fn every_point_index_is_the_position_within_its_own_block() {
        let catalog = read_catalog(&healthy_map(), SINGLETON).expect("healthy tree must read");
        for group in catalog.block_groups() {
            let block = catalog.targets_in_block(group.block);
            let indices: Vec<u32> = block.iter().map(|target| target.point_index).collect();
            assert_eq!(
                indices,
                (0..group.target_count as u32).collect::<Vec<u32>>(),
                "block {} point indices are not 0..n",
                group.block
            );
        }
    }

    #[test]
    fn an_unmapped_singleton_is_not_ready_rather_than_a_fault() {
        let memory = healthy_map();
        // Plausible-looking, aligned, but outside the mapped image.
        let error = read_catalog(&memory, BASE + 0x10_0000).expect_err("must refuse");
        assert!(matches!(
            error,
            InvasionWarpCatalogError::AutoInvadePointUnavailable(_)
        ));
        assert!(error.to_string().contains("unreadable"), "{error}");
    }

    #[test]
    fn a_null_or_misaligned_singleton_is_refused_before_any_read() {
        for candidate in [0u64, 1, 0x1_0004, MAX_PLAUSIBLE_ADDRESS] {
            let error = read_catalog(&healthy_map(), candidate).expect_err("must refuse");
            assert!(
                error
                    .to_string()
                    .contains("plausible aligned object address"),
                "0x{candidate:x} -> {error}"
            );
        }
    }

    #[test]
    fn an_unpopulated_map_reports_empty_not_unavailable() {
        // count == 0: the .aipbnd files have not been processed yet.
        let mut memory = healthy_map();
        memory.put_u64(SINGLETON + MAP_COUNT_OFFSET, 0);
        assert_eq!(
            read_catalog(&memory, SINGLETON),
            Err(InvasionWarpCatalogError::CatalogEmpty)
        );
        // Sentinel as its own root: constructed, still empty.
        let mut memory = healthy_map();
        memory.put_u64(HEAD + NODE_PARENT_OFFSET as u64, HEAD);
        assert_eq!(
            read_catalog(&memory, SINGLETON),
            Err(InvasionWarpCatalogError::CatalogEmpty)
        );
    }

    #[test]
    fn a_head_that_is_not_the_sentinel_is_refused() {
        let mut memory = healthy_map();
        memory.put_u8(HEAD + NODE_IS_NULL_OFFSET as u64, 0);
        let error = read_catalog(&memory, SINGLETON).expect_err("must refuse");
        assert!(error.to_string().contains("not the sentinel"), "{error}");
    }

    #[test]
    fn an_absurd_block_count_is_refused_before_the_walk() {
        let mut memory = healthy_map();
        memory.put_u64(SINGLETON + MAP_COUNT_OFFSET, MAX_PLAUSIBLE_BLOCKS + 1);
        let error = read_catalog(&memory, SINGLETON).expect_err("must refuse");
        assert!(error.to_string().contains("ceiling"), "{error}");
    }

    #[test]
    fn an_absurd_point_count_is_refused_instead_of_reading_the_array() {
        let mut memory = healthy_map();
        memory.put_u64(
            NODE_1 + NODE_POINT_COUNT_OFFSET as u64,
            MAX_PLAUSIBLE_POINTS_PER_BLOCK + 1,
        );
        let error = read_catalog(&memory, SINGLETON).expect_err("must refuse");
        assert!(error.to_string().contains("past the"), "{error}");
    }

    #[test]
    fn an_unmapped_point_array_is_refused() {
        let mut memory = healthy_map();
        memory.put_u64(NODE_1 + NODE_POINTS_OFFSET as u64, BASE + 0x10_0000);
        let error = read_catalog(&memory, SINGLETON).expect_err("must refuse");
        assert!(error.to_string().contains("unreadable"), "{error}");
    }

    #[test]
    fn a_misaligned_point_array_is_refused_before_the_read() {
        let mut memory = healthy_map();
        memory.put_u64(NODE_1 + NODE_POINTS_OFFSET as u64, POINTS_1 + 1);
        let error = read_catalog(&memory, SINGLETON).expect_err("must refuse");
        assert!(
            error
                .to_string()
                .contains("not a plausible aligned address"),
            "{error}"
        );
    }

    #[test]
    fn a_non_finite_coordinate_fails_the_read_rather_than_entering_the_catalog() {
        for (offset, name) in [(0u64, "x"), (4, "y"), (8, "z"), (12, "yaw")] {
            let mut memory = healthy_map();
            memory.put_f32(POINTS_1 + offset, f32::NAN);
            let error = read_catalog(&memory, SINGLETON).expect_err("must refuse");
            assert!(
                error.to_string().contains("non-finite"),
                "{name} NaN -> {error}"
            );
        }
    }

    #[test]
    fn a_cyclic_tree_exhausts_the_visit_budget_instead_of_hanging_the_game_thread() {
        let mut memory = healthy_map();
        // NODE_2's right child points back at the root: a genuine cycle.
        memory.put_u64(NODE_2 + NODE_RIGHT_OFFSET as u64, NODE_0);
        let error = read_catalog(&memory, SINGLETON).expect_err("must refuse");
        assert!(error.to_string().contains("budget"), "{error}");
        assert!(error.to_string().contains("torn or cyclic"), "{error}");
    }

    #[test]
    fn a_walk_that_finds_fewer_blocks_than_the_map_claims_is_a_torn_read() {
        let mut memory = healthy_map();
        // The map says 4 blocks; the tree only holds 3. That is exactly the "read raced the
        // loader" case oracle 1 must never report as a smaller-but-fine total.
        memory.put_u64(SINGLETON + MAP_COUNT_OFFSET, 4);
        let error = read_catalog(&memory, SINGLETON).expect_err("must refuse");
        assert!(error.to_string().contains("raced the loader"), "{error}");
    }

    #[test]
    fn a_node_pointer_into_unmapped_memory_is_refused_mid_walk() {
        let mut memory = healthy_map();
        memory.put_u64(NODE_0 + NODE_RIGHT_OFFSET as u64, BASE + 0x10_0000);
        let error = read_catalog(&memory, SINGLETON).expect_err("must refuse");
        assert!(
            error.to_string().contains("unreadable or implausible"),
            "{error}"
        );
    }

    #[test]
    fn a_block_with_zero_points_contributes_nothing_and_does_not_read_its_pointer() {
        let mut memory = healthy_map();
        memory.put_u64(NODE_1 + NODE_POINT_COUNT_OFFSET as u64, 0);
        // A null array pointer alongside count==0 must NOT be treated as corruption.
        memory.put_u64(NODE_1 + NODE_POINTS_OFFSET as u64, 0);
        let catalog = read_catalog(&memory, SINGLETON).expect("zero-point block is legal");
        assert_eq!(catalog.summary().block_count, 2);
        assert_eq!(catalog.summary().target_count, 5);
    }

    #[test]
    fn stale_stack_garbage_above_the_32_bit_point_count_is_ignored_not_treated_as_corruption() {
        // `AddForBlockId` (0x140a69550) stores pointCount with a 32-bit MOV and then MOVUPS-copies
        // 16 bytes, dragging an uninitialised stack dword into the upper half of the slot. Reading
        // the field as the `usize` the fromsoftware-rs binding declares would see a colossal count
        // and reject every block; the engine itself only reads `(int)count`.
        let mut memory = healthy_map();
        for node in [NODE_0, NODE_1, NODE_2] {
            memory.put_u32(node + NODE_POINT_COUNT_OFFSET as u64 + 4, 0xDEAD_BEEF);
        }
        let catalog = read_catalog(&memory, SINGLETON).expect("upper-dword garbage is normal here");
        assert_eq!(catalog.summary().block_count, 3);
        assert_eq!(catalog.summary().target_count, 6);
    }

    #[test]
    fn plausible_object_pointer_rejects_null_low_misaligned_and_non_canonical() {
        assert!(plausible_object_pointer(MIN_PLAUSIBLE_ADDRESS));
        assert!(plausible_object_pointer(0x7FFF_FFFF_F000));
        assert!(!plausible_object_pointer(0));
        assert!(!plausible_object_pointer(MIN_PLAUSIBLE_ADDRESS - 8));
        assert!(!plausible_object_pointer(MIN_PLAUSIBLE_ADDRESS + 1));
        assert!(!plausible_object_pointer(MAX_PLAUSIBLE_ADDRESS));
        assert!(!plausible_object_pointer(u64::MAX - 7));
    }
}
