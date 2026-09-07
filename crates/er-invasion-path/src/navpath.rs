//! Asking Elden Ring's own Havok-AI navmesh for a walkable route between two points.
//!
//! # Why the engine's pathfinder and not our own
//!
//! "Walkable" is not a property of the terrain mesh, it is a property of the navmesh the game
//! authored for that map: where an NPC may drop, which ledges are edges, which doorways connect.
//! Any route we computed ourselves would be a second opinion about walkability, and it would be
//! wrong exactly where it matters -- the cliff that looks crossable and is not. So this asks the
//! same `CSHkAiWorld` the game's own characters ask, and draws its answer.
//!
//! # The call chain, and where it came from
//!
//! Reversed out of `CS::CSAiFunc`'s path request (`FUN_1402ea2f0`, `0x1402ea2f0`) against ER
//! 1.16.2, where the dump VA, the `eldenring-deobf.bin` VA and the live runtime VA are all
//! identical (shift 0). The sequence is:
//!
//! 1. [`PARAMS_INIT_RVA`] fills a 0x34-byte request-parameter block with defaults.
//! 2. [`RESOLVE_POINT_RVA`] snaps a world position onto the navmesh, returning the snapped point
//!    and the `(section, face)` pair it landed in. Called once for the start, once for the end.
//! 3. [`REFINE_START_RVA`] adjusts the start point/face pair.
//! 4. [`REQUEST_RVA`] enqueues the search under the world's own mutex and returns a request id.
//!    Zero means it refused.
//! 5. [`IS_READY_RVA`] polls that id; [`FETCH_RVA`] then drains the finished route into a
//!    container and returns a status, where **2 is a complete route**.
//!
//! The search itself runs on the AI world's job, not on our thread -- which is why this is a
//! two-phase state machine rather than a function that returns a path.
//!
//! # Honest status
//!
//! Every address and every structure offset below is derived from static RE and cross-checked
//! against two independent call sites. None of it has been executed against a live game yet, so
//! the container walk in [`read_waypoints`] validates everything it reads (capacity is a power of
//! two, count is bounded, every element pointer is non-null, every coordinate is finite and
//! plausible) and refuses rather than trusting. A refusal degrades to the direction arrow, which
//! is the same thing the feature does for a genuinely unreachable player.

#![cfg(windows)]

use er_game_base::mem::{game_rva, safe_read_f32, safe_read_usize};

/// `GLOBAL_CSHkAiManager` -- `0x143d74ae8`, from
/// `1402ea34d: MOV RCX, qword ptr [0x143d74ae8]`.
const HK_AI_MANAGER_GLOBAL_RVA: u32 = 0x3d7_4ae8;
/// `CSHkAiManager::hkAiWorld`, from `1402ea39c: MOV RCX, qword ptr [RCX + 0x20]`.
const HK_AI_WORLD_OFFSET: usize = 0x20;
/// `GLOBAL_CSWorldNvmManager` -- `0x143d75870`. Read only as a residency check: it is the owner of
/// the loaded navmesh data, and a null one means no map's navmesh is resident.
const WORLD_NVM_MANAGER_GLOBAL_RVA: u32 = 0x3d7_5870;
/// `CSHkAiWorld`'s nav-data holder, from `FUN_140bddfe0`'s
/// `*(longlong *)(*(longlong *)(param_1 + 0xe8) + 0x130)`.
const HK_AI_WORLD_NAV_DATA_OFFSET: usize = 0xe8;
/// The section table inside that holder. Null while no navmesh is loaded, and the resolve
/// function dereferences it without checking.
const NAV_DATA_SECTIONS_OFFSET: usize = 0x130;

/// `FUN_140be4840` -- default-fill the request parameters.
const PARAMS_INIT_RVA: u32 = 0xbe_4840;
/// `FUN_140bddfe0(world, pos, params, out_point, out_face)` -- snap a position onto the navmesh.
const RESOLVE_POINT_RVA: u32 = 0xbd_dfe0;
/// `FUN_140bdd570(world, point, face, params)` -- refine the start pair before the request.
const REFINE_START_RVA: u32 = 0xbd_d570;
/// `FUN_140bdec90(world, start_pt, start_face, end_pt, end_face, params) -> request_id`.
const REQUEST_RVA: u32 = 0xbd_ec90;
/// `FUN_140bdfde0(world, request_id) -> bool` -- true once the request is no longer pending.
const IS_READY_RVA: u32 = 0xbd_fde0;
/// `FUN_140bdf610(world, container, &mut request_id) -> status` -- drain the finished route.
const FETCH_RVA: u32 = 0xbd_f610;
/// `FUN_140be4880(container)` -- construct the route container on the AI heap.
const CONTAINER_INIT_RVA: u32 = 0xbe_4880;
/// `FUN_14029cc10(container)` -- release the container's chunks and chunk table.
const CONTAINER_RELEASE_RVA: u32 = 0x29_cc10;
/// `CS::ChrIns::GetPhysicsHitRadius` -- `0x1403efc30`. The agent's width, which is what makes the
/// route avoid gaps a player cannot fit through.
const CHR_PHYSICS_HIT_RADIUS_RVA: u32 = 0x3e_fc30;
/// `CS::ChrIns::GetPhysicsHitHeight` -- `0x1403efc20`.
const CHR_PHYSICS_HIT_HEIGHT_RVA: u32 = 0x3e_fc20;
/// `DAT_143d61dc0` -- the traversal-cost table `CS::CSAiFunc` uses for the ordinary world.
///
/// The alternative, `DAT_143d61e10`, is the one the game swaps in inside a sealed world
/// (`CSWorldAiManagerImp::IsInSealdWorld`). Using the ordinary table everywhere means a route in
/// a sealed area may cost differently than the AI's own, never that it is invalid.
const NAV_COST_TABLE_RVA: u32 = 0x3d6_1dc0;

/// The `FETCH_RVA` status that means a complete route, from
/// `1402ea602: CMP dword ptr [RSP + 0x48], 0x2`.
const FETCH_STATUS_COMPLETE: i32 = 2;

/// Size of the request-parameter block. `FUN_140be4840` writes through `+0x30`; the block is
/// over-allocated and zeroed so no field is ever read uninitialised.
const PARAMS_BYTES: usize = 0x40;
/// Size of the route container. `FUN_140be4880` writes six qwords (`0x30`); over-allocated for
/// the same reason.
const CONTAINER_BYTES: usize = 0x40;

/// Multiplier the engine applies to the agent radius when filling `params+0x00`, from
/// `140c38fe0: MULSS XMM0, dword ptr [0x14329e684]` (2.0).
const AGENT_RADIUS_MULTIPLIER: f32 = 2.0;
/// How hard the engine is allowed to look. Both values come from the config, already clamped.
///
/// `params+0x20` is the range, where a negative value is the engine's own "unlimited", and
/// `params+0x24` is the iteration budget.
///
/// Both used to be hard-coded here as `150.0` and `800`, copied from `CS::CSAiFunc` -- and
/// copying from `CSAiFunc` was the mistake. That is an NPC working out how to walk at something
/// it can already see, budgeted to run once per character per frame. This runs once per target
/// every half second, for a player who wants an answer. The engine's own initialiser
/// `FUN_140be4840` sets `-1.0f` and `100000`.
///
/// The difference is the whole bug behind "it fails on downward corkscrews": a spiral descent has
/// an enormous path length inside a tiny footprint, built from many small navmesh faces, so an
/// NPC-sized iteration budget is spent going round and round before it reaches the bottom. The
/// search then reports no route, which is indistinguishable from there being none.
///
/// Measured live 2026-08-25 at the old values: the boot self-check returned "no route" to a map
/// character **28 m** away -- a fifth of the supposed 150 m range -- while `routes_found` sat
/// frozen at 19 and `arrows` climbed past 970. Range was never the binding constraint.

#[derive(Clone, Copy)]
pub(crate) struct SearchLimits {
    /// `params+0x20`. Negative is the engine's own "unlimited".
    pub(crate) range: f32,
    /// `params+0x24`, iterations.
    pub(crate) budget: i32,
}

impl SearchLimits {
    /// Carry an already-sanitised pair from the config.
    ///
    /// The clamping lives in [`crate::config`], not here: this module is `#[cfg(windows)]` and so
    /// is never compiled by `cargo test`, and a rule about which values are safe to hand the
    /// engine is exactly the kind of thing that must be tested on the host.
    pub(crate) fn new(range: f32, budget: i32) -> Self {
        Self { range, budget }
    }
}

/// The most waypoints a route may contain before it is treated as a corrupt read rather than a
/// long walk. A real navmesh route across a whole map is a few hundred points.
const MAX_WAYPOINTS: usize = 4096;
/// Havok coordinates beyond this magnitude are not a position, they are garbage.
const MAX_PLAUSIBLE_COORDINATE: f32 = 1.0e6;

/// The agent the route is planned for: `params+0x08` points at one of these.
///
/// Layout from `CS::SummonBuddyWarpManager::GetNavmeshPosNearSessionHost`
/// (`140c39023`..`140c39045`): the owner's `FieldInsHandle`, then the physics hit radius and
/// height, then a float the engine leaves at zero, then a flag byte whose low three bits it
/// clears. `FUN_140bdd570` reads the flag at `+0x14`, which is why this is 0x18 bytes and not the
/// bare 8-byte handle it first looks like.
#[repr(C, align(8))]
struct AgentDescriptor {
    field_ins_handle: u64,
    hit_radius: f32,
    hit_height: f32,
    reserved: f32,
    flags: u8,
    _padding: [u8; 3],
}

/// A `(section, face)` pair naming one navmesh triangle. `-1` in the first slot means "off mesh".
type FacePair = [i32; 2];

/// A position in the layout the engine's own vector calls take: xyzw, 16-byte aligned.
#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct Vector4 {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}

impl Vector4 {
    fn from_xyz(position: [f32; 3]) -> Self {
        Self {
            x: position[0],
            y: position[1],
            z: position[2],
            w: 0.0,
        }
    }
}

type ParamsInitFn = unsafe extern "C" fn(*mut u8) -> *mut u8;
type ResolvePointFn =
    unsafe extern "C" fn(usize, *const Vector4, *const u8, *mut Vector4, *mut FacePair);
type RefineStartFn = unsafe extern "C" fn(usize, *const Vector4, *mut FacePair, *const u8);
type RequestFn = unsafe extern "C" fn(
    usize,
    *const Vector4,
    *const FacePair,
    *const Vector4,
    *const FacePair,
    *const u8,
) -> i32;
type IsReadyFn = unsafe extern "C" fn(usize, i32) -> bool;
type FetchFn = unsafe extern "C" fn(usize, *mut u8, *mut i32) -> i32;
type ContainerInitFn = unsafe extern "C" fn(*mut u8, usize, usize, usize) -> *mut u8;
type ContainerReleaseFn = unsafe extern "C" fn(*mut u8);
type ChrFloatFn = unsafe extern "C" fn(usize) -> f32;

/// Why a route request could not even be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestRefusal {
    /// `CSHkAiManager`, its world, or `CSWorldNvmManager` is not up yet.
    AiWorldAbsent,
    /// No map's navmesh is resident. Common for a frame or two after a load, and permanent in
    /// places that have none.
    NavmeshAbsent,
    /// One of the two endpoints is not on the navmesh at all -- the usual answer for a player
    /// standing somewhere no character can walk.
    EndpointOffMesh,
    /// The world's request ring declined the search.
    QueueRefused,
}

/// Resolve `rva` to a callable address, or `None` when the module base is unavailable.
fn function(rva: u32) -> Option<usize> {
    game_rva(rva).ok().filter(|address| *address != 0)
}

/// The live `CSHkAiWorld`, with the navmesh residency check already done.
fn hk_ai_world() -> Result<usize, RequestRefusal> {
    let manager_slot = function(HK_AI_MANAGER_GLOBAL_RVA).ok_or(RequestRefusal::AiWorldAbsent)?;
    let nvm_slot = function(WORLD_NVM_MANAGER_GLOBAL_RVA).ok_or(RequestRefusal::AiWorldAbsent)?;
    // SAFETY: fault-tolerant reads of two singleton slots inside the game image.
    let manager = unsafe { safe_read_usize(manager_slot) }.unwrap_or(0);
    let nvm_manager = unsafe { safe_read_usize(nvm_slot) }.unwrap_or(0);
    if manager == 0 || nvm_manager == 0 {
        return Err(RequestRefusal::AiWorldAbsent);
    }
    // SAFETY: the manager pointer is non-null and the offset is inside its 0x28-byte body.
    let world = unsafe { safe_read_usize(manager + HK_AI_WORLD_OFFSET) }.unwrap_or(0);
    if world == 0 {
        return Err(RequestRefusal::AiWorldAbsent);
    }
    // `FUN_140bddfe0` walks `world->navData->sections` with no null check of its own, so both
    // links are checked here. Skipping this is not a wrong route, it is an access violation
    // inside Havok on the first frame after a map load.
    // SAFETY: `world` is non-null; both offsets are within `CSHkAiWorld`.
    let nav_data = unsafe { safe_read_usize(world + HK_AI_WORLD_NAV_DATA_OFFSET) }.unwrap_or(0);
    if nav_data == 0 {
        return Err(RequestRefusal::NavmeshAbsent);
    }
    // SAFETY: `nav_data` is non-null and the offset is the section table the resolver reads.
    let sections = unsafe { safe_read_usize(nav_data + NAV_DATA_SECTIONS_OFFSET) }.unwrap_or(0);
    if sections == 0 {
        return Err(RequestRefusal::NavmeshAbsent);
    }
    Ok(world)
}

/// Build the request parameters for a character-sized agent.
///
/// # Safety
///
/// `chr_ins` must be a live `ChrIns`.
unsafe fn build_params(
    chr_ins: usize,
    field_ins_handle: u64,
    limits: SearchLimits,
    params: &mut [u8; PARAMS_BYTES],
    agent: &mut AgentDescriptor,
) -> Option<()> {
    let init: ParamsInitFn =
        // SAFETY: the RVA is a validated function address in the game image.
        unsafe { std::mem::transmute::<usize, ParamsInitFn>(function(PARAMS_INIT_RVA)?) };
    let radius_of: ChrFloatFn =
        // SAFETY: as above.
        unsafe { std::mem::transmute::<usize, ChrFloatFn>(function(CHR_PHYSICS_HIT_RADIUS_RVA)?) };
    let height_of: ChrFloatFn =
        // SAFETY: as above.
        unsafe { std::mem::transmute::<usize, ChrFloatFn>(function(CHR_PHYSICS_HIT_HEIGHT_RVA)?) };
    let cost_table = function(NAV_COST_TABLE_RVA)?;

    // SAFETY: `params` is a zeroed block larger than the 0x34 bytes the initialiser writes.
    unsafe { init(params.as_mut_ptr()) };

    // SAFETY: `chr_ins` is a live character; both are plain accessors returning a float.
    let (radius, height) = unsafe { (radius_of(chr_ins), height_of(chr_ins)) };
    // A character whose physics shape has not been built yet reports zero or garbage, and a
    // zero-radius agent asks the navmesh a question about a point rather than about a body.
    if !radius.is_finite() || !height.is_finite() || radius <= 0.0 {
        return None;
    }
    agent.field_ins_handle = field_ins_handle;
    agent.hit_radius = radius;
    agent.hit_height = height;
    agent.reserved = 0.0;
    agent.flags = 0;

    write_f32(params, 0x00, radius * AGENT_RADIUS_MULTIPLIER);
    write_usize(params, 0x08, std::ptr::from_mut(agent) as usize);
    write_usize(params, 0x18, cost_table);
    write_f32(params, 0x20, limits.range);
    write_i32(params, 0x24, limits.budget);
    // `+0x2e` enables the agent descriptor at `+0x08`; the engine only ever sets the two
    // together, and `FUN_1402ec2f0` tests them as a pair.
    params[0x2e] = 1;
    write_i32(params, 0x30, 0);
    Some(())
}

fn write_f32(block: &mut [u8; PARAMS_BYTES], offset: usize, value: f32) {
    block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_i32(block: &mut [u8; PARAMS_BYTES], offset: usize, value: i32) {
    block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_usize(block: &mut [u8; PARAMS_BYTES], offset: usize, value: usize) {
    block[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// A route search that has been handed to the AI world and not yet collected.
///
/// # Dropping one is not free
///
/// This type looks like a plain handle and is not. Each request occupies a slot in a
/// **fixed-size ring that belongs to the world, shared with every NPC in the map** --
/// `world+0x130` is the slot array (stride `0x68`), `world+0x138` its capacity, `world+0x13c` the
/// round-robin cursor, and `slot+0x40` holds the request id, doubling as the busy flag.
/// `FUN_140be1bb0` walks that ring for a slot whose `+0x40` is zero and **returns 0 when every
/// slot is taken**.
///
/// Exactly one function clears a slot: `FUN_140bf3320`, reachable only from the fetch in
/// [`PendingRequest::fetch`]. It `hkUnref`s the Havok path at `slot+0x38`, deallocates the two
/// heap buffers at `slot+0x50` and `slot+0x58`, and zeroes `slot+0x40`. **There is no cancel.**
///
/// So dropping a `PendingRequest` without polling it to an answer holds its slot, its Havok
/// reference and its allocations for the rest of the process -- and because the ring is the
/// world's, filling it stops the game'S own characters pathfinding. Giving up on the answer is
/// fine; giving up on the request is not. `crate::abandon` is how the caller does the former.
pub(crate) struct PendingRequest {
    world: usize,
    request_id: i32,
}

/// Ask the navmesh for a route from `from` to `to`, for a body the size of `chr_ins`.
///
/// Returns the in-flight request; the answer arrives on a later frame via [`poll`].
///
/// # Safety
///
/// Must be called on the game thread, and `chr_ins` must be a live `ChrIns`.
pub(crate) unsafe fn request(
    chr_ins: usize,
    field_ins_handle: u64,
    limits: SearchLimits,
    from: [f32; 3],
    to: [f32; 3],
) -> Result<PendingRequest, RequestRefusal> {
    let world = hk_ai_world()?;

    let mut params = [0u8; PARAMS_BYTES];
    let mut agent = AgentDescriptor {
        field_ins_handle: 0,
        hit_radius: 0.0,
        hit_height: 0.0,
        reserved: 0.0,
        flags: 0,
        _padding: [0; 3],
    };
    // SAFETY: caller guarantees `chr_ins` is live.
    unsafe { build_params(chr_ins, field_ins_handle, limits, &mut params, &mut agent) }
        .ok_or(RequestRefusal::AiWorldAbsent)?;

    let resolve: ResolvePointFn = unsafe {
        // SAFETY: validated function address in the game image.
        std::mem::transmute::<usize, ResolvePointFn>(
            function(RESOLVE_POINT_RVA).ok_or(RequestRefusal::AiWorldAbsent)?,
        )
    };
    let refine: RefineStartFn = unsafe {
        // SAFETY: as above.
        std::mem::transmute::<usize, RefineStartFn>(
            function(REFINE_START_RVA).ok_or(RequestRefusal::AiWorldAbsent)?,
        )
    };
    let enqueue: RequestFn = unsafe {
        // SAFETY: as above.
        std::mem::transmute::<usize, RequestFn>(
            function(REQUEST_RVA).ok_or(RequestRefusal::AiWorldAbsent)?,
        )
    };

    let (start_in, end_in) = (Vector4::from_xyz(from), Vector4::from_xyz(to));
    let (mut start_point, mut end_point) = (Vector4::default(), Vector4::default());
    let (mut start_face, mut end_face): (FacePair, FacePair) = ([-1, -1], [-1, -1]);

    // The engine resolves the destination first and the origin second; the order is preserved
    // because the resolver mutates world state the second call reads.
    // SAFETY: every pointer is to a live local of the exact shape the callee expects, and the
    // navmesh residency was checked above.
    unsafe {
        resolve(
            world,
            &raw const end_in,
            params.as_ptr(),
            &raw mut end_point,
            &raw mut end_face,
        );
    }
    if end_face[0] == -1 || end_face[1] < 0 {
        return Err(RequestRefusal::EndpointOffMesh);
    }
    // SAFETY: as above.
    unsafe {
        resolve(
            world,
            &raw const start_in,
            params.as_ptr(),
            &raw mut start_point,
            &raw mut start_face,
        );
    }
    if start_face[0] == -1 || start_face[1] < 0 {
        return Err(RequestRefusal::EndpointOffMesh);
    }
    // SAFETY: as above; this adjusts the start pair in place.
    unsafe {
        refine(
            world,
            &raw const start_point,
            &raw mut start_face,
            params.as_ptr(),
        )
    };

    // SAFETY: as above. Returns zero when the world's request ring declines the search.
    let request_id = unsafe {
        enqueue(
            world,
            &raw const start_point,
            &raw const start_face,
            &raw const end_point,
            &raw const end_face,
            params.as_ptr(),
        )
    };
    if request_id == 0 {
        return Err(RequestRefusal::QueueRefused);
    }
    Ok(PendingRequest { world, request_id })
}

/// What a poll of an in-flight request found.
#[derive(Debug)]
pub(crate) enum PollOutcome {
    /// The AI world has not finished the search yet. Poll again next frame.
    Pending,
    /// A complete route, start to finish.
    Route(Vec<[f32; 3]>),
    /// The search finished without a complete route: the destination is not reachable on foot
    /// from where the search started.
    NoRoute,
}

impl PendingRequest {
    /// Collect the route if the search has finished.
    ///
    /// # Safety
    ///
    /// Must be called on the game thread, and at most once after the search completes -- the
    /// engine's fetch consumes the request slot.
    pub(crate) unsafe fn poll(&mut self) -> PollOutcome {
        let Some(is_ready) = function(IS_READY_RVA) else {
            return PollOutcome::NoRoute;
        };
        // SAFETY: validated function address; the callee takes its own mutex.
        let is_ready: IsReadyFn = unsafe { std::mem::transmute::<usize, IsReadyFn>(is_ready) };
        // SAFETY: `world` was validated when the request was made.
        if !unsafe { is_ready(self.world, self.request_id) } {
            return PollOutcome::Pending;
        }
        // SAFETY: the request is finished, so the fetch below consumes it exactly once.
        unsafe { self.fetch() }
    }

    /// Drain the finished search into a container, read it, and release the container.
    ///
    /// # Safety
    ///
    /// The request must be finished. Called at most once per request.
    unsafe fn fetch(&mut self) -> PollOutcome {
        let (Some(fetch), Some(container_init), Some(container_release)) = (
            function(FETCH_RVA),
            function(CONTAINER_INIT_RVA),
            function(CONTAINER_RELEASE_RVA),
        ) else {
            return PollOutcome::NoRoute;
        };
        // SAFETY: three validated function addresses in the game image.
        let (fetch, container_init, container_release): (
            FetchFn,
            ContainerInitFn,
            ContainerReleaseFn,
        ) = unsafe {
            (
                std::mem::transmute::<usize, FetchFn>(fetch),
                std::mem::transmute::<usize, ContainerInitFn>(container_init),
                std::mem::transmute::<usize, ContainerReleaseFn>(container_release),
            )
        };

        let mut container = [0u8; CONTAINER_BYTES];
        // SAFETY: a zeroed block larger than the six qwords the constructor writes. The
        // constructor takes its allocator from a global and panics only on an incompatible heap,
        // which is the AI heap being absent -- already excluded by the residency check.
        unsafe { container_init(container.as_mut_ptr(), 0, 0, 0) };

        // SAFETY: the container is constructed and the request is finished.
        let status = unsafe { fetch(self.world, container.as_mut_ptr(), &raw mut self.request_id) };
        // SAFETY: the container is constructed; read before it is released.
        let waypoints = unsafe { read_waypoints(container.as_ptr()) };
        // SAFETY: the container is constructed and no longer read from.
        unsafe {
            container_release(container.as_mut_ptr());
            release_container_node(container.as_ptr());
        }

        match (status, waypoints) {
            // Two or more points is a line. One point is the engine saying "you are already
            // there", which draws as nothing and is better reported as no route.
            (FETCH_STATUS_COMPLETE, Some(points)) if points.len() >= 2 => {
                PollOutcome::Route(points)
            }
            _ => PollOutcome::NoRoute,
        }
    }
}

/// Hand the container's node back to the allocator that produced it.
///
/// The engine's own teardown is two steps, and only the first has a name: `FUN_14029cc10` frees
/// the chunks and the chunk table, then the caller invokes the allocator's `Deallocate` slot on
/// the node -- `1402ea626`..`1402ea631`:
/// `MOV RCX,[container]; MOV RAX,[RCX]; MOV RDX,[container+8]; CALL [RAX+0x68]`. Omitting the
/// second step leaks one node per route request, forever.
///
/// # Safety
///
/// `container` must be a constructed container whose chunks have already been released.
unsafe fn release_container_node(container: *const u8) {
    /// `DLAllocator`'s `Deallocate` slot, the same `+0x68` every other shell in this workspace
    /// uses when it frees a heap-backed engine object.
    const DEALLOCATE_VTABLE_OFFSET: usize = 0x68;
    type DeallocateFn = unsafe extern "C" fn(usize, usize);

    let base = container as usize;
    // SAFETY: reads of two qwords inside a constructed container.
    let (allocator, node) = unsafe {
        (
            safe_read_usize(base).unwrap_or(0),
            safe_read_usize(base + 0x08).unwrap_or(0),
        )
    };
    if allocator == 0 || node == 0 {
        return;
    }
    // SAFETY: `allocator` is the pointer the constructor stored; its first qword is its vtable.
    let Some(vtable) = (unsafe { safe_read_usize(allocator) }).filter(|slot| *slot != 0) else {
        return;
    };
    // SAFETY: reading one function-pointer slot out of that vtable.
    let Some(deallocate) =
        (unsafe { safe_read_usize(vtable + DEALLOCATE_VTABLE_OFFSET) }).filter(|slot| *slot != 0)
    else {
        return;
    };
    // SAFETY: validated allocator, validated slot, and a node this allocator produced.
    unsafe { std::mem::transmute::<usize, DeallocateFn>(deallocate)(allocator, node) };
}

/// Read the waypoints out of a filled route container, refusing anything that does not look like
/// one.
///
/// # The container, and how its shape was established
///
/// It is a power-of-two ring of element pointers, not a flat array. From the push
/// (`FUN_1402ecad0`, `0x1402ecad0`):
///
/// | offset | field |
/// |--------|-------|
/// | `0x00` | allocator |
/// | `0x08` | node |
/// | `0x10` | chunk table -- an array of element pointers |
/// | `0x18` | capacity, a power of two |
/// | `0x20` | head index |
/// | `0x28` | element count |
///
/// with `slot = (capacity - 1) & (head + index)` and an element of 0x20 bytes whose first four
/// floats are the position. The independent read path (`FUN_1402e6340`, used by
/// `FUN_1402ebd70`) walks the same three fields and applies the same mask, which is what makes
/// this a cross-checked layout rather than one reading of one function.
///
/// # Safety
///
/// `container` must be a constructed container.
unsafe fn read_waypoints(container: *const u8) -> Option<Vec<[f32; 3]>> {
    let base = container as usize;
    // SAFETY: four qword reads inside a constructed container.
    let (table, capacity, head, count) = unsafe {
        (
            safe_read_usize(base + 0x10)?,
            safe_read_usize(base + 0x18)?,
            safe_read_usize(base + 0x20)?,
            safe_read_usize(base + 0x28)?,
        )
    };
    // Every one of these is a structural invariant of the container, so a violation means the
    // layout above is wrong for this build -- not that the route is unusual. Refusing here is
    // what keeps a wrong offset from becoming a wild pointer read a few lines down.
    if table == 0 || count == 0 || count > MAX_WAYPOINTS {
        return None;
    }
    if capacity == 0 || !capacity.is_power_of_two() || count > capacity {
        return None;
    }

    let mask = capacity - 1;
    let mut points = Vec::with_capacity(count);
    for index in 0..count {
        let slot = mask & head.wrapping_add(index);
        // SAFETY: `slot` is masked into the table's own capacity, and `table` is non-null.
        let element = unsafe { safe_read_usize(table + slot * size_of::<usize>()) }?;
        if element == 0 {
            return None;
        }
        // SAFETY: the first three floats of a 0x20-byte element are its position.
        let point = unsafe {
            [
                safe_read_f32(element)?,
                safe_read_f32(element + 4)?,
                safe_read_f32(element + 8)?,
            ]
        };
        if point
            .iter()
            .any(|axis| !axis.is_finite() || axis.abs() > MAX_PLAUSIBLE_COORDINATE)
        {
            return None;
        }
        points.push(point);
    }
    Some(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `FUN_140be4840` writes, i.e. what leaving the block alone would give.
    const ENGINE_DEFAULT_RANGE: f32 = -1.0;
    const ENGINE_DEFAULT_BUDGET: i32 = 100_000;

    #[test]
    fn the_agent_descriptor_matches_the_layout_the_engine_reads() {
        // `FUN_140bdd570` reads a flag byte at `+0x14`; a descriptor shorter than that is the
        // 8-byte-handle mistake this struct exists to avoid.
        assert_eq!(size_of::<AgentDescriptor>(), 0x18);
        assert_eq!(std::mem::offset_of!(AgentDescriptor, hit_radius), 0x08);
        assert_eq!(std::mem::offset_of!(AgentDescriptor, hit_height), 0x0c);
        assert_eq!(std::mem::offset_of!(AgentDescriptor, flags), 0x14);
    }

    #[test]
    fn the_params_and_container_blocks_cover_what_the_engine_writes() {
        // The initialiser writes through `params+0x30`; the constructor writes six qwords.
        const { assert!(PARAMS_BYTES >= 0x34) };
        const { assert!(CONTAINER_BYTES >= 0x30) };
    }

    #[test]
    fn a_position_is_four_aligned_floats() {
        assert_eq!(size_of::<Vector4>(), 16);
        assert_eq!(align_of::<Vector4>(), 16);
    }

    #[test]
    fn writing_a_param_field_lands_where_the_engine_reads_it() {
        let mut params = [0u8; PARAMS_BYTES];
        write_f32(&mut params, 0x20, ENGINE_DEFAULT_RANGE);
        write_i32(&mut params, 0x24, ENGINE_DEFAULT_BUDGET);
        write_usize(&mut params, 0x18, 0x1234_5678_9abc_def0);
        assert_eq!(
            f32::from_le_bytes(params[0x20..0x24].try_into().expect("four bytes")),
            ENGINE_DEFAULT_RANGE
        );
        assert_eq!(
            i32::from_le_bytes(params[0x24..0x28].try_into().expect("four bytes")),
            ENGINE_DEFAULT_BUDGET
        );
        assert_eq!(
            u64::from_le_bytes(params[0x18..0x20].try_into().expect("eight bytes")),
            0x1234_5678_9abc_def0
        );
        // `0x43160000` is the literal `CS::CSAiFunc` stores in this field.
        // 0x43160000 is 150.0f -- what CS::CSAiFunc writes, and what this crate used to copy
        // from it before a live run showed an NPC-sized budget failing at 28 m.
        assert_eq!(150.0f32.to_bits(), 0x4316_0000);
    }
}
