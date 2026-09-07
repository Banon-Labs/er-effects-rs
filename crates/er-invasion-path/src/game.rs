//! Everything read out of the live game: who else is in the session, where they are, whether you
//! can see them, and where the camera is looking.
//!
//! Read-only throughout. This DLL never writes a byte of game memory, never patches a param
//! (which is what breaks Seamless invasions -- bd `param-patching-dlls-break-seamless-invasions`)
//! and installs no detour. The only game functions it calls are the navmesh query in
//! [`crate::navpath`] and the physics raycast below, both of which the engine treats as queries.

#![cfg(windows)]

use eldenring::cs::{CSCamera, ChrIns, ChrSet, PlayerIns, WorldChrMan};
use eldenring::position::HavokPosition;
use fromsoftware_shared::FromStatic;

use crate::census::{Census, NPC_CHR_TYPE, is_named_player_kind, is_player_kind};
use crate::geometry::{self, Camera};

/// Height above the physics origin that the "no route" arrow leaves the body at, so it emerges
/// from the character rather than from the dirt they are standing on.
const ARROW_ORIGIN_HEIGHT_METERS: f32 = 1.1;

/// How far above a waypoint the drawn line floats.
///
/// The navmesh sits on the collision surface, and a line drawn exactly on it z-fights with the
/// ground it is describing. A few centimetres reads as painted on the ground; more reads as
/// hovering above it.
const PATH_LIFT_METERS: f32 = 0.06;

/// One other player in the session, reduced to what the overlay needs.
///
/// There is deliberately no `ChrIns` pointer here. The route is planned for a body the size of
/// yours -- you are the one who has to walk it -- so the navmesh agent is always the local
/// player, and keeping a remote pointer around would only invite planning the route for the wrong
/// body.
pub(crate) struct RemotePlayer {
    /// The engine's own name for this character, used to keep a path's colour stable across
    /// frames even as players enter and leave.
    pub(crate) field_ins_handle: u64,
    /// Physics-space position, at the feet.
    pub(crate) position: [f32; 3],
    /// Straight-line distance from the local player, in metres.
    pub(crate) distance_meters: f32,
}

/// The local player, and everyone else worth drawing a path to.
pub(crate) struct Roster {
    pub(crate) local_chr_ins: usize,
    pub(crate) local_field_ins_handle: u64,
    pub(crate) local_position: [f32; 3],
    pub(crate) remotes: Vec<RemotePlayer>,
    /// What the walk saw on its way to `remotes`, for the log.
    pub(crate) census: Census,
}

fn to_array(position: HavokPosition) -> [f32; 3] {
    [position.0, position.1, position.2]
}

/// The `FieldInsHandle`'s eight raw bytes, which is the form the navmesh agent descriptor wants.
///
/// `FieldInsHandle` is `repr(C, align(8))` over a `u32` selector and an `i32` block id, so the
/// little-endian qword the engine copies is `selector | (block_id << 32)`. The block id is cast
/// through `u32` rather than sign-extended: `-1` is a legitimate "no block" value, and
/// sign-extending it would set the whole upper half and make two different handles compare equal.
fn handle_bits(chr_ins: &ChrIns) -> u64 {
    let handle = chr_ins.field_ins_handle;
    u64::from(handle.selector.0) | (u64::from(handle.block_id.0 as u32) << 32)
}

/// A character's physics position, or `None` when its physics module is not built yet.
///
/// A character mid-spawn has a module whose position is still the origin, and drawing a path to
/// `(0, 0, 0)` is a line across the entire map -- far more misleading than drawing nothing.
fn physics_position(chr_ins: &ChrIns) -> Option<[f32; 3]> {
    let position = to_array(chr_ins.modules.physics.position);
    position
        .iter()
        .all(|axis| axis.is_finite())
        .then_some(position)
        .filter(|position| geometry::length(*position) > f32::EPSILON)
}

/// Is this character loaded far enough to have a real position and a real body?
fn is_live(chr_ins: &ChrIns) -> bool {
    // Dead players are still in the ChrSet with a valid position; a path to a corpse is noise.
    chr_ins.modules.data.hp > 0
}

/// `ChrIns::chr_type`, read as the raw `i32` the field actually holds.
///
/// Deliberately not read as the `ChrType` enum. That enum names 0..=22 and `-1`, and constructing
/// one from a value outside that set is undefined behaviour -- which is exactly the risk here,
/// because the whole reason this function exists is that a Seamless session may type a character
/// in a way the vanilla enum never anticipated. Reading the raw integer can be surprised; it
/// cannot be unsound.
fn chr_type_raw(chr_ins: &ChrIns) -> i32 {
    // SAFETY: reads one `i32` field through a pointer to that same field.
    unsafe { *((&raw const chr_ins.chr_type).cast::<i32>()) }
}

/// The live local player, or `None` until one really exists.
///
/// `WorldChrMan::instance()` returning `Ok` does not mean there is a player: during boot the
/// singleton is up long before the world is, and `main_player` holds a non-null pointer to
/// nothing. Dereferencing it is an access violation on the game thread, every frame, from the
/// first tick -- which is exactly how this DLL killed the game at ~100ms on 2026-08-25, before
/// its overlay had rendered a single frame (`draws=0` in its own log, so the render path was
/// never even reached).
///
/// So the pointer is screened before any field of it is read: plausibly heap-aligned, and its
/// vtable inside the game image. That is the same discipline the product DLL uses for early-boot
/// reads, and it is not optional here.
///
/// # Safety
///
/// Must be called on the game thread.
unsafe fn live_main_player() -> Option<&'static PlayerIns> {
    // SAFETY: singleton access on the game thread; `Err` before the singleton exists.
    let world_chr_man = unsafe { WorldChrMan::instance() }.ok()?;
    let main_player = world_chr_man.main_player.as_ref()?;
    let address = std::ptr::from_ref::<PlayerIns>(main_player) as usize;
    // SAFETY: a plausibility screen on the raw address; reads nothing through it.
    if !unsafe { er_game_base::mem::is_heap_aligned_ptr(address) } {
        return None;
    }
    let module_base = er_game_base::mem::game_module_base().ok()?;
    // SAFETY: fault-tolerant read of the first qword; returns None rather than faulting.
    let vtable = unsafe { er_game_base::mem::safe_read_usize(address) }?;
    if !er_game_base::mem::vtable_in_game_image(vtable, module_base) {
        return None;
    }
    // SAFETY: the address is heap-plausible and carries a vtable from the game image, which is
    // as much as can be established without the engine's own liveness flag.
    Some(unsafe { &*(address as *const PlayerIns) })
}

/// The item the local player is using this frame, if any.
///
/// # Safety
///
/// Must be called on the game thread.
pub(crate) unsafe fn current_use_item() -> Option<u32> {
    // SAFETY: game thread; the player pointer is screened before any field is read.
    let player = unsafe { live_main_player() }?;
    player
        .chr_ins
        .tae_queued_use_item
        .as_valid()
        .map(|item| item.param_id())
}

/// The local player's feet, or `None` before there is a player.
///
/// # Safety
///
/// Must be called on the game thread.
pub(crate) unsafe fn local_position() -> Option<[f32; 3]> {
    // SAFETY: game thread; the reader screens the player pointer before any field is read.
    let main_player = unsafe { live_main_player() }?;
    physics_position(&main_player.chr_ins)
}

/// Read the session roster.
///
/// Returns `None` before the world exists -- at the title screen, during a load, and in any menu
/// state where there is no local player to draw from.
///
/// # Safety
///
/// Must be called on the game thread.
pub(crate) unsafe fn roster(max_targets: usize) -> Option<Roster> {
    // SAFETY: singleton access on the game thread; `Err` before the world is up.
    let world_chr_man = unsafe { WorldChrMan::instance() }.ok()?;
    // SAFETY: game thread; screened before any field is read.
    let main_player = unsafe { live_main_player() }?;
    let local_position = physics_position(&main_player.chr_ins)?;
    let local_chr_ins = std::ptr::from_ref(&main_player.chr_ins) as usize;

    let mut census = Census::default();
    let mut remotes = Vec::new();

    // The documented home of the players, and where a vanilla session keeps them.
    census.sets += 1;
    for player in world_chr_man.player_chr_set.characters() {
        let player: &PlayerIns = player;
        collect(
            &player.chr_ins,
            local_chr_ins,
            local_position,
            Origin::PlayerSet,
            &mut census,
            &mut remotes,
        );
    }

    // ...but "documented" is not "verified for a session Seamless is running". This workspace's
    // own enemy sweep already hedges that a co-op session may put other players in a set that is
    // not `player_chr_set` (`er-enemynpc-effects`), and a roster that trusts one set and finds
    // nobody looks exactly like a navmesh that found no route. So when the player set comes back
    // empty, walk every ChrSet the world holds and pick characters by kind instead.
    //
    // The wide walk is the fallback rather than the default because it is the expensive one: the
    // per-block sets are where the map's hundreds of enemies live, and paying for that sweep six
    // times a second to re-derive an answer the player set already gave would be waste.
    if remotes.is_empty() {
        census.widened = true;
        let inline_sets = [
            (&raw const world_chr_man.player_chr_set) as usize,
            (&raw const world_chr_man.ghost_chr_set) as usize,
            (&raw const world_chr_man.summon_buddy_chr_set) as usize,
            (&raw const world_chr_man.debug_chr_set) as usize,
        ];
        let mut walked: Vec<usize> = Vec::with_capacity(16);
        for chr_set in world_chr_man.chr_sets.iter().flatten() {
            let address = chr_set.as_ptr() as usize;
            if inline_sets.contains(&address) || walked.contains(&address) {
                continue;
            }
            walked.push(address);
            census.sets += 1;
            // SAFETY: an address the world itself stores in `chr_sets`, typed as the game types
            // it. This is the same walk `er-enemynpc-effects` performs every sweep.
            let chr_set = unsafe { &*(address as *const ChrSet<ChrIns>) };
            for chr_ins in chr_set.characters() {
                collect(
                    chr_ins,
                    local_chr_ins,
                    local_position,
                    Origin::WideSweep,
                    &mut census,
                    &mut remotes,
                );
            }
        }
    }

    // Nearest first, so the cap keeps the players who matter rather than whichever slot the
    // ChrSet happened to hand back first.
    remotes.sort_by(|a, b| {
        a.distance_meters
            .partial_cmp(&b.distance_meters)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    remotes.truncate(max_targets);

    Some(Roster {
        local_chr_ins,
        local_field_ins_handle: handle_bits(&main_player.chr_ins),
        local_position,
        remotes,
        census,
    })
}

/// Where a character was found, which decides how much its `chr_type` is trusted.
///
/// The two are deliberately asymmetric. See [`crate::census::is_named_player_kind`] for the live
/// measurement that forced the split: a single `ChrType 7` sitting among the map's NPCs was drawn
/// a permanent arrow outside any invasion, while the actual people in the session turned up in
/// `player_chr_set` exactly where the engine documents.
#[derive(Clone, Copy)]
enum Origin {
    /// `WorldChrMan::player_chr_set`. Membership is the evidence; an unfamiliar type here is
    /// accepted, because a session kind this build has not seen is likelier than a mistake.
    PlayerSet,
    /// Every other ChrSet in the world, walked only when the player set came back empty. An
    /// unfamiliar type here is map furniture, so only named player kinds are accepted.
    WideSweep,
}

impl Origin {
    fn accepts(self, chr_type: i32) -> bool {
        match self {
            Self::PlayerSet => is_player_kind(chr_type),
            Self::WideSweep => is_named_player_kind(chr_type),
        }
    }
}

/// Consider one character for the roster, counting it either way.
///
/// Every rejection is counted before it is made, which is the point: the census is what turns "no
/// line appeared" into "there were eleven characters and all eleven were type 5".
fn collect(
    chr_ins: &ChrIns,
    local_chr_ins: usize,
    local_position: [f32; 3],
    origin: Origin,
    census: &mut Census,
    remotes: &mut Vec<RemotePlayer>,
) {
    let chr_type = chr_type_raw(chr_ins);
    census.count(chr_type);
    // The local player is in the player ChrSet too, and a path from yourself to yourself is a dot.
    if std::ptr::from_ref(chr_ins) as usize == local_chr_ins {
        return;
    }
    if !origin.accepts(chr_type) || !is_live(chr_ins) {
        return;
    }
    let Some(position) = physics_position(chr_ins) else {
        return;
    };
    remotes.push(RemotePlayer {
        field_ins_handle: handle_bits(chr_ins),
        position,
        distance_meters: geometry::length(geometry::sub(position, local_position)),
    });
}

/// The nearest ordinary map character, as a destination to ask the navmesh about.
///
/// This exists so the navmesh call chain can be proven without a second player. Every address in
/// [`crate::navpath`] is byte-verified static RE, and none of it had ever executed: the request is
/// only ever issued for a remote player, so the first time eleven raw function pointers and a
/// container walk ran for real would have been in the middle of an invasion, where an access
/// violation costs the session it was meant to prove.
///
/// An `Npc` is the right destination precisely because it is not a player: the map authored it
/// standing on the navmesh, so a route to one exercises the whole chain -- resolve, refine,
/// enqueue, poll, fetch, walk the returned container, release it -- rather than stopping at a
/// refusal. Nothing is drawn from it; the result goes to the log and nowhere else.
///
/// # Safety
///
/// Must be called on the game thread.
pub(crate) unsafe fn npc_probe_target(local_position: [f32; 3]) -> Option<([f32; 3], f32)> {
    // SAFETY: singleton access on the game thread; `Err` before the world is up.
    let world_chr_man = unsafe { WorldChrMan::instance() }.ok()?;
    let mut best: Option<([f32; 3], f32)> = None;
    let mut consider = |chr_ins: &ChrIns| {
        if chr_type_raw(chr_ins) != NPC_CHR_TYPE || !is_live(chr_ins) {
            return;
        }
        let Some(position) = physics_position(chr_ins) else {
            return;
        };
        let distance = geometry::length(geometry::sub(position, local_position));
        // Far enough that the route has to be planned rather than answered by the two endpoints
        // landing in the same navmesh face, which would exercise none of the search.
        if distance < MIN_PROBE_DISTANCE_METERS {
            return;
        }
        if best.is_none_or(|(_, closest)| distance < closest) {
            best = Some((position, distance));
        }
    };
    for chr_ins in world_chr_man.open_field_chr_set.base.characters() {
        consider(chr_ins);
    }
    for chr_set in world_chr_man.chr_sets.iter().flatten() {
        // SAFETY: an address the world itself stores in `chr_sets`, typed as the game types it.
        let chr_set = unsafe { &*(chr_set.as_ptr() as *const ChrSet<ChrIns>) };
        for chr_ins in chr_set.characters() {
            consider(chr_ins);
        }
    }
    best
}

/// Closest an NPC may be and still be worth asking the navmesh to route to.
const MIN_PROBE_DISTANCE_METERS: f32 = 8.0;

/// The live camera, in the form [`crate::geometry`] projects with.
///
/// Read from `CSCamera::pers_cam_1`, whose `matrix` is the camera-to-world transform: rows 0..2
/// are the right/up/forward basis and row 3 is the eye. Returns `None` whenever the camera is not
/// up or its numbers are not finite, which is every frame before the world exists.
///
/// # Safety
///
/// Reads live game memory. Called from the render callback, where a torn read costs one frame's
/// worth of wrong geometry and nothing worse.
pub(crate) unsafe fn camera() -> Option<Camera> {
    // SAFETY: singleton access; `Err` before the camera exists.
    let cameras = unsafe { CSCamera::instance() }.ok()?;
    let cam = &cameras.pers_cam_1;
    let matrix = cam.matrix;
    let raw = [
        [matrix.0.0, matrix.0.1, matrix.0.2],
        [matrix.1.0, matrix.1.1, matrix.1.2],
        [matrix.2.0, matrix.2.1, matrix.2.2],
    ];
    let eye = [matrix.3.0, matrix.3.1, matrix.3.2];
    if raw
        .iter()
        .flatten()
        .chain(eye.iter())
        .any(|axis| !axis.is_finite())
        || !cam.fov.is_finite()
        || !cam.aspect_ratio.is_finite()
    {
        return None;
    }
    // Normalised, because the projection inverts this basis by TRANSPOSING it -- exact for an
    // orthonormal basis and silently wrong for a scaled one. The engine's own tag path calls a
    // general `Invert` instead; three square roots a frame buys the same guarantee. A zero-length
    // axis is the struct before the first real frame, and projecting through it would put every
    // path at the centre of the screen rather than nowhere.
    let (Some(right), Some(up), Some(forward)) = (
        geometry::normalize(raw[0]),
        geometry::normalize(raw[1]),
        geometry::normalize(raw[2]),
    ) else {
        return None;
    };
    Some(Camera {
        right,
        up,
        forward,
        eye,
        fov_y: cam.fov,
        aspect: cam.aspect_ratio,
    })
}

/// Where the "no route" arrow leaves the player's body.
pub(crate) fn arrow_origin(local_position: [f32; 3]) -> [f32; 3] {
    [
        local_position[0],
        local_position[1] + ARROW_ORIGIN_HEIGHT_METERS,
        local_position[2],
    ]
}

/// Lift a navmesh waypoint clear of the surface it describes.
pub(crate) fn lift_waypoint(point: [f32; 3]) -> [f32; 3] {
    [point[0], point[1] + PATH_LIFT_METERS, point[2]]
}
