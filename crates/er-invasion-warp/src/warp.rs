//! The local warp itself: put the player at an invasion spawn point.
//!
//! # Which primitive, and why
//!
//! Three candidates were reversed against ER 1.16.2. The plan doc named two of them and chose
//! neither; the one actually used here is the third, and it is the engine's own.
//!
//! * `WarpPlayer` (`0x1405f7ad0`) is **entity-id anchored** -- it moves you to a map's initial
//!   spawn entity, not to a coordinate. Unusable for arbitrary points.
//! * `PlayerIns::Respawn`, the `ChrIns` vtable slot `+0x5a0` (target `0x140657b60`, read out of
//!   the shipped image at `*(u64*)0x142a7d0e0`), does take arbitrary coordinates -- but it
//!   **heals to full and reinitialises SpEffects**, and it performs no map load or streaming
//!   request at all, so a long-distance teleport would drop the player into unstreamed world.
//! * `TriggerAreaReload` (`0x1405f2890`), the EMEVD `Event2003` warp, is an arbitrary-coordinate
//!   warp **with** the load. Its input shape is `BlockId + block-local xyz + euler yaw` --
//!   which is, field for field, the `.aip` record this crate already decodes. That is the one
//!   replicated here.
//!
//! `TriggerAreaReload` itself always reloads the *current* map, so it cannot be called directly;
//! what this module does is run its exact sequence with our destination and our coordinates
//! substituted for the "where I am standing now" values it derives.
//!
//! # Why the coordinates are handed over untouched
//!
//! `MoveMapStep`'s spawn resolver (`FUN_140afcf60`) reads the explicit-spawn slot and calls
//! `ConvertBlockCoordsToPhysicsCoords` on it *itself*. So the block-local `.aip` xyz goes in
//! raw: converting first would double-apply the block origin.
//!
//! # Hard boundary
//!
//! One session-manager call exists in this sequence and it is **vanilla**: `TriggerAreaReload`
//! calls `CSSessionManagerImp::SetupMapReentry` when `protocolState == InGame`, on every EMEVD
//! warp in the game. Omitting a step the engine always performs is how a reload softlocks, so
//! it is replicated -- and [`WarpOutcome::session_touches`] **counts** it rather than pretending
//! the number is zero. Nothing here starts, fakes or spoofs invasion/multiplayer state; no
//! `CSNetMan`, `QuickmatchManager` or `CSBreakInPointManager` code is entered.
//!
//! Every RVA below is byte-checked against `eldenring-deobf.bin` at shift 0
//! (`python3 scripts/check-dump-deobf-identity.py --count 32 0x<va>`); see
//! `docs/plans/world-map-invasion-warp.md`.

use crate::invasion_warp::InvasionWarpTarget;

/// The image base every VA in the RE notes is expressed against. For 1.16.2 the dump VA, the
/// `eldenring-deobf.bin` VA and the live runtime VA are all identical, so `RVA = VA - this`.
pub const RE_IMAGE_BASE: usize = 0x1_4000_0000;

/// `CS::GameMan::SetDisableMapEnterAnim(bool)` -- `0x14067a850`.
pub const SET_DISABLE_MAP_ENTER_ANIM_RVA: usize = 0x67_a850;
/// `CS::GameMan::SetMoveMapStepBlockId(BlockId *out, BlockId *in)` -- `0x14067abd0`.
///
/// The literal is declared exactly ONCE, in `er_game_base::rva`, because the product crate
/// needs the same address and two independent literals would be free to drift apart. There is
/// deliberately no host-side mirror here: a `cfg(not(windows))` copy would be exactly the
/// second literal the alias-drift gate exists to prevent.
#[cfg(windows)]
pub use er_game_base::rva::SET_MOVE_MAP_STEP_BLOCK_ID_RVA;
/// `FUN_14067ab20(FloatVector4 *blockLocalPos, FloatVector4 *euler)` -- the explicit-spawn
/// setter: writes `GameMan+0xc90`, `GameMan+0xca0`, and sets `GameMan+0xcb0 = 1`.
pub const SET_EXPLICIT_SPAWN_RVA: usize = 0x67_ab20;
/// `CS::GameMan::SetInitialAreaEntityId(int *in)` -- `0x14067abb0`.
///
/// `STEP_MoveMap_Init` latches this into `MoveMapStep+0xd8` and it becomes the spawn-point
/// entity id the destination-side resolver looks for. Grace fast-travel uses it with
/// `bonfireEntityId - 970`; we use [`DEFAULT_SPAWN_ENTITY_ID`].
pub const SET_INITIAL_AREA_ENTITY_ID_RVA: usize = 0x67_abb0;
/// Spawn-point entity id meaning "this map's own default player start".
///
/// `FUN_14061fc80` scans the destination MSB's Player parts and treats an unset entity id
/// (`-1`) as 0, so 0 matches the map's authored `c0000_0000` start. Verified present in every
/// non-overworld map that has Player parts at all.
pub const DEFAULT_SPAWN_ENTITY_ID: u32 = 0;
/// `FUN_14067a1c0()` -- reads the `GameMan+0xcb0` use-explicit-spawn flag back.
pub const GET_EXPLICIT_SPAWN_FLAG_RVA: usize = 0x67_a1c0;
/// `FUN_1406792a0(FloatVector4 *outPos, FloatVector4 *outEuler)` -- reads `GameMan+0xc90` /
/// `+0xca0` back. This is the requested-position / requested-yaw oracle.
pub const GET_EXPLICIT_SPAWN_RVA: usize = 0x67_92a0;
/// `WarpNextStageKick_()` -- `0x1405f7b70`. Kicks the stage transition.
pub const WARP_NEXT_STAGE_KICK_RVA: usize = 0x5f_7b70;
/// `CS::CSSessionManagerImp::SetupMapReentry(this, bool)` -- `0x140cafc30`.
pub const SETUP_MAP_REENTRY_RVA: usize = 0xca_fc30;
/// `GLOBAL_CSSessionManager` -- `0x143d7a4d0`, read from
/// `1405f2935: mov 0x3787b94(%rip),%rcx  # 0x143d7a4d0`.
pub const SESSION_MANAGER_GLOBAL_RVA: usize = 0x3d7_a4d0;
/// `GetCurrentMapId(BlockId *out)` -- `0x1405eefb0`. Used to report where the warp started.
pub const GET_CURRENT_MAP_ID_RVA: usize = 0x5e_efb0;
/// `ConvertBlockCoordsToPhysicsCoords(FloatVector3 *out, FloatVector3 *blockLocal, BlockId *id)`
/// -- `0x14061e120`. Block-local -> physics space, handling the overworld and interior cases,
/// and returning `false` when the block's world info is not resident. Session-free: its whole
/// body reads `GLOBAL_FieldArea->worldInfoOwner2`.
pub const CONVERT_BLOCK_COORDS_TO_PHYSICS_RVA: usize = 0x61_e120;
/// `ChrIns::GetPhysicsPosition(ChrIns *chr, FloatVector4 *out)` -- `0x1403f0bf0`.
pub const CHR_INS_GET_PHYSICS_POSITION_RVA: usize = 0x3f_0bf0;

/// Offset of `protocolState` on `CSSessionManagerImp`, from
/// `1405f293c: cmpl $0x6,0x10(%rcx)`.
pub const SESSION_PROTOCOL_STATE_OFFSET: usize = 0x10;
/// The `InGame` protocol state -- the literal `6` that same compare tests.
pub const SESSION_PROTOCOL_STATE_IN_GAME: i32 = 6;
/// The `WaitReentryToMap` protocol state -- the literal `7` that `SetupMapReentry` **writes as
/// its very first statement** (`140cafc47: movl $0x7,0x10(%rcx)`).
///
/// This is why the re-entry is self-latching: entering it moves the session OUT of `InGame`, so a
/// second warp issued before the engine has driven the session back sees `7` and skips the
/// re-entry. Seeing `7` here is therefore the EXPECTED reading straight after one of our own
/// warps; seeing it persist across many warps means the map re-entry never completed.
pub const SESSION_PROTOCOL_STATE_WAIT_REENTRY_TO_MAP: i32 = 7;
/// Offset of `lobbyState`, from `140cafc54: cmpl $0x3,0xc(%rcx)`. Reported alongside the protocol
/// state because `SetupMapReentry`'s `LeaveSession` branch is reachable only when this is `Host`.
pub const SESSION_LOBBY_STATE_OFFSET: usize = 0x0c;
/// `LobbyState::Host` -- the literal `3` that compare tests.
pub const SESSION_LOBBY_STATE_HOST: i32 = 3;

/// Why the session-manager re-entry did or did not run.
///
/// This replaces a bare count. `session_touches` could only ever say "1" or "0", and **three
/// materially different situations all produced `0`**: an unreadable global, a null manager, and
/// a live manager parked in some other protocol state. A user-visible warp failure that reported
/// `session_touches=0` was therefore undiagnosable without another launch -- which is exactly
/// what happened on 2026-08-04, where the counter flipped to `0` and stayed there across a dozen
/// confirms with no way to tell which of the three it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionGate {
    /// `protocolState == InGame`, so `SetupMapReentry` ran -- exactly as vanilla
    /// `TriggerAreaReload` does.
    Entered,
    /// The session-manager global could not be read at all.
    ManagerUnreadable,
    /// The session-manager global is null (teardown, or the world is not up).
    ManagerNull,
    /// The manager is live but its `protocolState` could not be read.
    StateUnreadable,
    /// The manager is live and not `InGame`. Carries the observed state and lobby state, because
    /// [`SESSION_PROTOCOL_STATE_WAIT_REENTRY_TO_MAP`] is a self-inflicted, expected value right
    /// after one of our own warps, while any other value is something else entirely.
    NotInGame {
        state: i32,
        lobby_state: Option<i32>,
    },
}

impl SessionGate {
    /// How many times the session manager was entered: 1 for [`Self::Entered`], else 0.
    ///
    /// Kept so the number vanilla's branch would produce is still reported, without it being the
    /// only thing reported.
    #[must_use]
    pub const fn touches(self) -> u32 {
        matches!(self, Self::Entered) as u32
    }

    /// Whether the session is parked in the re-entry state our own previous warp puts it in.
    #[must_use]
    pub const fn is_parked_in_reentry(self) -> bool {
        matches!(
            self,
            Self::NotInGame {
                state: SESSION_PROTOCOL_STATE_WAIT_REENTRY_TO_MAP,
                ..
            }
        )
    }

    /// A short, log-ready description that never loses which of the failure modes it was.
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::Entered => {
                format!("ENTERED (protocolState={SESSION_PROTOCOL_STATE_IN_GAME} InGame)")
            }
            Self::ManagerUnreadable => "SKIPPED (session-manager global unreadable)".into(),
            Self::ManagerNull => "SKIPPED (session-manager global is null)".into(),
            Self::StateUnreadable => "SKIPPED (protocolState unreadable)".into(),
            Self::NotInGame { state, lobby_state } => {
                let named = if state == SESSION_PROTOCOL_STATE_WAIT_REENTRY_TO_MAP {
                    " WaitReentryToMap -- the state OUR OWN previous warp set; the engine has not \
                     driven the session back to InGame"
                } else {
                    ""
                };
                format!("SKIPPED (protocolState={state}{named}, lobbyState={lobby_state:?})")
            }
        }
    }
}

/// `BlockId::NONE`: the sentinel `SetMoveMapStepBlockId` refuses to disaster-remap.
pub const BLOCK_ID_NONE: u32 = 0xFFFF_FFFF;

/// A 16-byte vector in the engine's layout.
///
/// `align(16)` is not decoration. The lean teleport path
/// (`CSChrPhysicsModule::ForceSetPosition`, `0x14045f910`) loads its argument with **`MOVAPS`**,
/// which `#GP`s on an unaligned address. The reload path used here writes with `MOVUPS` and
/// would tolerate misalignment, but the type is shared and a future caller must not have to
/// rediscover that the hard way.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FloatVector4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl FloatVector4 {
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}

/// The `w` component `TriggerAreaReload` stores alongside the spawn position (`__real_3f800000`).
pub const SPAWN_POSITION_W: f32 = 1.0;

/// Block-local `.aip` position -> the `FloatVector4` the explicit-spawn slot expects.
///
/// Handed over untouched: `MoveMapStep` runs `ConvertBlockCoordsToPhysicsCoords` on this itself,
/// so converting here would add the block origin twice.
#[must_use]
pub const fn spawn_position(position: [f32; 3]) -> FloatVector4 {
    FloatVector4::new(position[0], position[1], position[2], SPAWN_POSITION_W)
}

/// `.aip` yaw -> the euler `FloatVector4` the explicit-spawn slot expects.
///
/// The orientation argument is **euler angles in radians**, not a quaternion:
/// `CSChrPhysicsModule::SetOrientation` (`0x14045f7a0`) feeds it straight to `EulerToQuat`
/// (`0x140461a00`), which reads `.x`, `.y`, `.z` as half-angle rotations about `DL_X/Y/Z`.
/// Yaw is the `.y` slot -- confirmed by the inverse conversion
/// (`EulerFromTransformationMatrix`, `0x14039b0b0`, derives `.y` from `atan2` in the XZ plane)
/// and by `SosSignMan::SetMultiplayJoinData` (`0x1406fb577`) writing `{0, spawnAngle, 0, 0}`,
/// where `spawnAngle` occupies the same wire slot as the `.aip` fourth float.
///
/// So: **no negation, no degree conversion, and no wrapping.** The raw authored value goes in.
/// [`InvasionWarpTarget::heading_radians`] exists for compass/pin display, NOT for this -- using
/// the wrapped value here would silently rotate half the table by a full turn.
#[must_use]
pub const fn spawn_orientation(yaw: f32) -> FloatVector4 {
    FloatVector4::new(0.0, yaw, 0.0, 0.0)
}

/// Whether an invasion location may be used as a warp destination RIGHT NOW.
///
/// # Why this is a type and not two edits at the call sites
///
/// There are two ways to ask for one of these warps -- confirming a pin on the world map, and the
/// F7/F8/F9 hotkeys -- and a gate placed at each is a gate a third caller silently skips. The rule
/// is a statement about the PRIMITIVE, so the check lives inside [`native::request_invasion_warp`]
/// where nothing can route around it. The two callers were left alone deliberately: both already
/// handle [`WarpError`] and neither treats a refusal as a reason to fall through to anything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarpPolicy {
    /// An invasion attempt is in flight. The pins are informational markers for its duration and
    /// every warp request is refused before anything is written.
    MarkersOnly,
    /// No attempt in flight, so selecting an invasion location relocates the player as usual.
    Warpable,
}

/// Whether a Seamless invasion attempt is currently in flight.
///
/// Published every frame by the DLL's session tracer, which is the only thing that can see the
/// ersc session; this crate holds the latch so the policy gate below -- and the map's icon choice
/// -- read ONE value and cannot disagree about what the session is doing.
///
/// Defaults to `false`, which is the honest answer before anything has looked: with no session
/// resolvable there is no attempt, so nothing should be blocked. That also means a publisher that
/// never runs leaves warps ENABLED rather than silently disabling them, which is the failure a
/// player can diagnose ("it never blocks") instead of the one they cannot ("it blocks forever and
/// I do not know why").
static INVASION_ATTEMPT_IN_FLIGHT: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Record whether an invasion attempt is in flight. Called once per frame from the session tracer.
pub fn set_invasion_attempt_in_flight(in_flight: bool) {
    INVASION_ATTEMPT_IN_FLIGHT.store(in_flight, core::sync::atomic::Ordering::SeqCst);
}

/// Whether an invasion attempt is in flight, as last published.
#[must_use]
pub fn invasion_attempt_in_flight() -> bool {
    INVASION_ATTEMPT_IN_FLIGHT.load(core::sync::atomic::Ordering::SeqCst)
}

/// The policy in force this instant.
///
/// Scoped to an active attempt by user requirement (2026-08-12). The point of blocking the warp is
/// that moving mid-attempt is incoherent -- the destination Seamless is negotiating is where you
/// are supposed to end up -- and the point of the dim is to SAY SO on the pin. Neither works
/// unconditionally: a pin that is always dim cannot communicate "not clickable right now", because
/// there is no brighter state to read it against.
#[must_use]
pub fn invasion_warp_policy() -> WarpPolicy {
    if invasion_attempt_in_flight() {
        WarpPolicy::MarkersOnly
    } else {
        WarpPolicy::Warpable
    }
}

/// Why a warp request could not be issued. Every variant means "nothing was written".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WarpError {
    /// An invasion attempt is in flight, so invasion locations are markers rather than warp
    /// destinations for its duration. Refused before any engine state is touched.
    NotAWarpDestination,
    /// `GetModuleHandleA(NULL)` failed.
    ModuleBase(String),
    /// The target's block key is the `0xFFFFFFFF` sentinel.
    BlockIdIsNone,
    /// The explicit-spawn flag did not read back as set, so `MoveMapStep` would ignore our
    /// coordinates and drop the player at the block's default spawn instead. Fail before the
    /// stage kick rather than warp somewhere unintended.
    SpawnSlotDidNotLatch { flag: u8 },
    /// A coordinate-free warp found the explicit-spawn slot still ARMED from an earlier warp.
    /// `MoveMapStep` would use that stale coordinate instead of the destination block's own
    /// spawn, dropping the player at another map's position inside this one.
    StaleSpawnSlotArmed { flag: u8 },
}

impl core::fmt::Display for WarpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotAWarpDestination => write!(
                f,
                "an invasion attempt is in flight, so invasion locations are map markers rather \
                 than warp destinations until it ends; nothing was written and the player did not \
                 move"
            ),
            Self::ModuleBase(detail) => write!(f, "game module base unavailable: {detail}"),
            Self::BlockIdIsNone => write!(f, "target block id is the NONE sentinel (0xFFFFFFFF)"),
            Self::SpawnSlotDidNotLatch { flag } => write!(
                f,
                "explicit-spawn flag read back as {flag}, expected 1; refusing to kick the stage"
            ),
            Self::StaleSpawnSlotArmed { flag } => write!(
                f,
                "explicit-spawn flag read back as {flag} before a coordinate-free warp, expected \
                 0; a previous warp's coordinate is still armed and would be used instead of the \
                 destination's own spawn, so the stage was not kicked"
            ),
        }
    }
}

impl std::error::Error for WarpError {}

/// What a successfully-issued warp actually asked the engine for.
///
/// This is the evidence record, and it is deliberately full of *read-back* values rather than
/// the values we intended: a write we did not confirm proves nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpOutcome {
    /// The target we tried to reach.
    pub target: InvasionWarpTarget,
    /// The block the player was in when the warp was issued.
    pub origin_block: u32,
    /// The block we asked for.
    pub requested_block: u32,
    /// The block `SetMoveMapStepBlockId` actually stored. **Not always the requested one:**
    /// for areas 50..=88 -- which covers both shipped `.aip` areas (60 and 61) -- it rewrites
    /// the id through `CalcGetReplaceMapIdByDisaster`, so the destination can legitimately
    /// differ. Reported rather than asserted.
    pub effective_block: u32,
    /// `GameMan+0xcb0` read back after the write; 1 means `MoveMapStep` will honour our spawn.
    pub spawn_flag: u8,
    /// `GameMan+0xc90` read back: the block-local position `MoveMapStep` will convert.
    pub spawn_position: [f32; 3],
    /// `GameMan+0xca0` read back: the euler orientation, whose `.y` is the yaw.
    pub spawn_yaw: f32,
    /// How many times the sequence entered the session manager. Expected 0 or 1 -- 1 exactly
    /// when `protocolState == InGame`, matching vanilla `TriggerAreaReload`. Counted, never
    /// assumed.
    ///
    /// Derived from [`Self::session_gate`]; kept because it is the number vanilla's branch
    /// produces, but it must never be the only thing reported -- on its own it cannot say which
    /// of four situations a `0` was.
    pub session_touches: u32,
    /// WHY the re-entry did or did not run. This is the diagnostic field; `session_touches` is
    /// the summary of it.
    pub session_gate: SessionGate,
}

#[cfg(windows)]
mod native {
    use super::{
        BLOCK_ID_NONE, CHR_INS_GET_PHYSICS_POSITION_RVA, CONVERT_BLOCK_COORDS_TO_PHYSICS_RVA,
        DEFAULT_SPAWN_ENTITY_ID, FloatVector4, GET_CURRENT_MAP_ID_RVA, GET_EXPLICIT_SPAWN_FLAG_RVA,
        GET_EXPLICIT_SPAWN_RVA, SESSION_LOBBY_STATE_OFFSET, SESSION_MANAGER_GLOBAL_RVA,
        SESSION_PROTOCOL_STATE_IN_GAME, SESSION_PROTOCOL_STATE_OFFSET,
        SET_DISABLE_MAP_ENTER_ANIM_RVA, SET_EXPLICIT_SPAWN_RVA, SET_INITIAL_AREA_ENTITY_ID_RVA,
        SET_MOVE_MAP_STEP_BLOCK_ID_RVA, SETUP_MAP_REENTRY_RVA, SessionGate,
        WARP_NEXT_STAGE_KICK_RVA, WarpError, WarpOutcome, WarpPolicy, invasion_warp_policy,
        spawn_orientation, spawn_position,
    };
    use crate::invasion_warp::InvasionWarpTarget;
    use crate::select::ResolvedTarget;

    /// `SetDisableMapEnterAnim(true)`, exactly as `TriggerAreaReload` does before the kick.
    const DISABLE_MAP_ENTER_ANIM: bool = true;
    /// The `dl = 1` `TriggerAreaReload` passes to `SetupMapReentry`.
    const SETUP_MAP_REENTRY_ARG: bool = true;
    /// `GameMan+0xcb0` when the explicit spawn is armed.
    const SPAWN_FLAG_ARMED: u8 = 1;
    /// What the flag must read for a coordinate-free warp: nothing armed, so the engine
    /// resolves the destination block's own spawn.
    const SPAWN_FLAG_CLEAR: u8 = 0;

    type SetBoolFn = unsafe extern "system" fn(bool);
    type SetMoveMapStepBlockIdFn = unsafe extern "system" fn(*mut u32, *const u32) -> *mut u32;
    type SetExplicitSpawnFn = unsafe extern "system" fn(*const FloatVector4, *const FloatVector4);
    type SetInitialAreaEntityIdFn = unsafe extern "system" fn(*const u32);
    type GetExplicitSpawnFlagFn = unsafe extern "system" fn() -> u8;
    type GetExplicitSpawnFn = unsafe extern "system" fn(*mut FloatVector4, *mut FloatVector4);
    type VoidFn = unsafe extern "system" fn();
    type GetBlockIdFn = unsafe extern "system" fn(*mut u32) -> *mut u32;
    type SetupMapReentryFn = unsafe extern "system" fn(usize, bool);
    type ConvertBlockCoordsFn =
        unsafe extern "system" fn(*mut FloatVector4, *const FloatVector4, *const u32) -> bool;
    type GetPhysicsPositionFn =
        unsafe extern "system" fn(usize, *mut FloatVector4) -> *mut FloatVector4;

    /// Issue a local warp to `target`.
    ///
    /// Refuses while [`super::invasion_warp_policy`] reads [`WarpPolicy::MarkersOnly`], i.e. for
    /// as long as an invasion attempt is in flight. This is the single choke point every warp to
    /// an invasion location passes through -- the world-map confirm hook and the hotkeys both end
    /// up here -- so the refusal cannot be routed around by adding a caller.
    ///
    /// # Safety
    ///
    /// Must be called on the game task thread with the world loaded (`GameMan` and the session
    /// manager singletons live). It writes `GameMan`'s explicit-spawn slot and kicks a stage
    /// transition, so it must not run concurrently with the engine's own warp.
    pub unsafe fn request_invasion_warp(
        target: &InvasionWarpTarget,
    ) -> Result<WarpOutcome, WarpError> {
        // FIRST, before the block check and before the module base is even resolved: a refusal
        // must be indistinguishable from never having been called. Every later `return Err` in
        // this function is careful to leave engine state alone; this one does not have to be,
        // because it runs before any of it.
        if invasion_warp_policy() == WarpPolicy::MarkersOnly {
            return Err(WarpError::NotAWarpDestination);
        }
        let requested_block = target.block.raw();
        if requested_block == BLOCK_ID_NONE {
            return Err(WarpError::BlockIdIsNone);
        }
        let base = er_game_base::mem::game_module_base().map_err(WarpError::ModuleBase)?;

        // Where we are now -- recorded before anything is written, so a failed warp still
        // reports a truthful origin.
        let mut origin_block: u32 = BLOCK_ID_NONE;
        let get_current_map_id: GetBlockIdFn =
            unsafe { core::mem::transmute(base + GET_CURRENT_MAP_ID_RVA) };
        unsafe { get_current_map_id(&raw mut origin_block) };

        // Vanilla step 1: the session-manager re-entry, gated exactly as TriggerAreaReload
        // gates it. The REASON is recorded, not just a count -- entering it sets
        // `protocolState = WaitReentryToMap`, so this gate is self-latching and a `0` here on a
        // later warp is a fact about the previous one.
        let session_gate = unsafe { setup_map_reentry_if_in_game(base) };

        // Vanilla step 2: suppress the map-enter animation.
        let set_disable_map_enter_anim: SetBoolFn =
            unsafe { core::mem::transmute(base + SET_DISABLE_MAP_ENTER_ANIM_RVA) };
        unsafe { set_disable_map_enter_anim(DISABLE_MAP_ENTER_ANIM) };

        // Vanilla step 3: choose the destination block. `effective_block` is the OUT slot and
        // may differ from what we asked for (disaster remap over areas 50..=88).
        let mut effective_block: u32 = requested_block;
        let set_move_map_step_block_id: SetMoveMapStepBlockIdFn =
            unsafe { core::mem::transmute(base + SET_MOVE_MAP_STEP_BLOCK_ID_RVA) };
        unsafe { set_move_map_step_block_id(&raw mut effective_block, &raw const requested_block) };

        let get_explicit_spawn_flag: GetExplicitSpawnFlagFn =
            unsafe { core::mem::transmute(base + GET_EXPLICIT_SPAWN_FLAG_RVA) };

        // Vanilla step 4: arm the explicit spawn with the .aip record, untouched.
        //
        // SKIPPED ENTIRELY for a provisional target. `FUN_140afcf60` reads the explicit-spawn
        // flag and, when it is CLEAR, resolves the destination block's own authored player start
        // out of that map's MSB instead (`FUN_14061fc80`, on the destination side, after the
        // load). That is the same coordinate-free path the shipped `WarpPlayer` EMEVD
        // instruction and grace fast-travel take -- neither of them ever calls
        // `SET_EXPLICIT_SPAWN_RVA`. It is what lets a dungeon the player has never entered be a
        // warp destination at all: we do not need to know anything inside it.
        let (spawn_flag, position_readback, orientation_readback) = if target.is_provisional() {
            // The flag is consumed and cleared by `UpdatePlayerInfo` on every map load, so it is
            // normally already 0 here. If it is not, a previous warp's coordinate is still armed
            // and the engine would use THAT instead of this block's default -- landing the player
            // at another map's coordinates inside this one. Refuse; do not write GameMan to force
            // it, because the only native clearer also zeroes live warp state at +0xac4/+0xb28/
            // +0xb58/+0xb5c/+0xb5e/+0xb68/+0xc35.
            let flag = unsafe { get_explicit_spawn_flag() };
            if flag != SPAWN_FLAG_CLEAR {
                return Err(WarpError::StaleSpawnSlotArmed { flag });
            }
            // Spawn point 0 selects the map's own default Player part. Every non-overworld map
            // that has Player parts at all has one with entity id 0.
            let entity_id: u32 = DEFAULT_SPAWN_ENTITY_ID;
            let set_initial_area_entity_id: SetInitialAreaEntityIdFn =
                unsafe { core::mem::transmute(base + SET_INITIAL_AREA_ENTITY_ID_RVA) };
            unsafe { set_initial_area_entity_id(&raw const entity_id) };
            (flag, FloatVector4::default(), FloatVector4::default())
        } else {
            let position = spawn_position(target.position);
            let orientation = spawn_orientation(target.yaw);
            let set_explicit_spawn: SetExplicitSpawnFn =
                unsafe { core::mem::transmute(base + SET_EXPLICIT_SPAWN_RVA) };
            unsafe { set_explicit_spawn(&raw const position, &raw const orientation) };

            // Read the slot back BEFORE kicking. If the flag did not latch, MoveMapStep ignores
            // our coordinates and spawns the player at the block default -- a silently wrong warp
            // is worse than a refused one.
            let flag = unsafe { get_explicit_spawn_flag() };
            if flag != SPAWN_FLAG_ARMED {
                return Err(WarpError::SpawnSlotDidNotLatch { flag });
            }
            let mut position_readback = FloatVector4::default();
            let mut orientation_readback = FloatVector4::default();
            let get_explicit_spawn: GetExplicitSpawnFn =
                unsafe { core::mem::transmute(base + GET_EXPLICIT_SPAWN_RVA) };
            unsafe {
                get_explicit_spawn(&raw mut position_readback, &raw mut orientation_readback);
            }
            (flag, position_readback, orientation_readback)
        };

        // Vanilla step 5: kick the stage. Past this point the load is the engine's.
        let warp_next_stage_kick: VoidFn =
            unsafe { core::mem::transmute(base + WARP_NEXT_STAGE_KICK_RVA) };
        unsafe { warp_next_stage_kick() };

        Ok(WarpOutcome {
            target: *target,
            origin_block,
            requested_block,
            effective_block,
            spawn_flag,
            spawn_position: [
                position_readback.x,
                position_readback.y,
                position_readback.z,
            ],
            spawn_yaw: orientation_readback.y,
            session_touches: session_gate.touches(),
            session_gate,
        })
    }

    /// Convert one catalog target's block-local position into physics space via the engine's
    /// own `ConvertBlockCoordsToPhysicsCoords`, yielding a [`ResolvedTarget`] the selection
    /// layer can rank.
    ///
    /// Returns `None` when the engine declines the conversion -- which is the whole point of
    /// routing through it: a block whose world info is not resident cannot be placed, and a
    /// target that cannot be placed must never become a warp candidate.
    ///
    /// # Safety
    ///
    /// Game task thread, world loaded (`GLOBAL_FieldArea` live).
    pub unsafe fn resolve_target(
        base: usize,
        target: &InvasionWarpTarget,
    ) -> Option<ResolvedTarget> {
        let block = target.block.raw();
        if block == BLOCK_ID_NONE {
            return None;
        }
        let local = FloatVector4::new(
            target.position[0],
            target.position[1],
            target.position[2],
            0.0,
        );
        let mut world = FloatVector4::default();
        let convert: ConvertBlockCoordsFn =
            unsafe { core::mem::transmute(base + CONVERT_BLOCK_COORDS_TO_PHYSICS_RVA) };
        // The engine writes only x/y/z; `world.w` stays at the default and is never read.
        let ok = unsafe { convert(&raw mut world, &raw const local, &raw const block) };
        if !ok {
            return None;
        }
        Some(ResolvedTarget::new(*target, [world.x, world.y, world.z]))
    }

    /// The local player's physics-space position, or `None` when there is no player.
    ///
    /// # Safety
    ///
    /// Game task thread. Resolves `WorldChrMan` through the typed singleton, so it is `None`
    /// rather than a fault when the world is not up.
    pub unsafe fn player_physics_position(base: usize) -> Option<[f32; 3]> {
        use fromsoftware_shared::FromStatic;
        let world_chr_man = unsafe { eldenring::cs::WorldChrMan::instance() }.ok()?;
        let player = world_chr_man.main_player.as_ref()?;
        // `PlayerIns.chr_ins` is the struct's first field, so the PlayerIns pointer IS the
        // ChrIns pointer the engine expects here (RespawnPlayer relies on the same identity).
        let chr_ins = core::ptr::from_ref(&player.chr_ins) as usize;
        let mut out = FloatVector4::default();
        let get_physics_position: GetPhysicsPositionFn =
            unsafe { core::mem::transmute(base + CHR_INS_GET_PHYSICS_POSITION_RVA) };
        unsafe { get_physics_position(chr_ins, &raw mut out) };
        Some([out.x, out.y, out.z])
    }

    /// The block the player is currently in, or `None` when the read is not plausible.
    ///
    /// # Safety
    ///
    /// Game task thread.
    pub unsafe fn current_block_id(base: usize) -> Option<u32> {
        let mut block: u32 = BLOCK_ID_NONE;
        let get_current_map_id: GetBlockIdFn =
            unsafe { core::mem::transmute(base + GET_CURRENT_MAP_ID_RVA) };
        unsafe { get_current_map_id(&raw mut block) };
        if block == BLOCK_ID_NONE {
            return None;
        }
        Some(block)
    }

    /// `if (GLOBAL_CSSessionManager->protocolState == InGame) SetupMapReentry(mgr, true);`
    ///
    /// Returns WHY the re-entry did or did not run, so a caller can report a measured reason
    /// instead of a bare count that four different situations share.
    ///
    /// Note that entering the re-entry is self-latching: `SetupMapReentry`'s first statement is
    /// `protocolState = WaitReentryToMap`, so an immediately following warp will take the
    /// `NotInGame` path until the engine drives the session back to `InGame`.
    ///
    /// # Safety
    ///
    /// Game task thread, world loaded.
    unsafe fn setup_map_reentry_if_in_game(base: usize) -> SessionGate {
        // Fault-tolerant: during teardown the global can be null or stale, and a warp that
        // cannot read it must degrade to "did not touch the session", never to a crash.
        //
        // Each bail returns a DISTINCT reason. They used to collapse to a bare `0`, which made a
        // live failure unattributable without another launch.
        let Some(manager) =
            (unsafe { er_game_base::mem::safe_read_usize(base + SESSION_MANAGER_GLOBAL_RVA) })
        else {
            return SessionGate::ManagerUnreadable;
        };
        if manager == 0 {
            return SessionGate::ManagerNull;
        }
        // `cmpl $0x6,0x10(%rcx)` compares a 32-bit signed value, so read it the same width.
        let Some(state) =
            (unsafe { er_game_base::mem::safe_read_i32(manager + SESSION_PROTOCOL_STATE_OFFSET) })
        else {
            return SessionGate::StateUnreadable;
        };
        if state != SESSION_PROTOCOL_STATE_IN_GAME {
            let lobby_state =
                unsafe { er_game_base::mem::safe_read_i32(manager + SESSION_LOBBY_STATE_OFFSET) };
            return SessionGate::NotInGame { state, lobby_state };
        }
        let setup_map_reentry: SetupMapReentryFn =
            unsafe { core::mem::transmute(base + SETUP_MAP_REENTRY_RVA) };
        unsafe { setup_map_reentry(manager, SETUP_MAP_REENTRY_ARG) };
        SessionGate::Entered
    }
}

#[cfg(windows)]
pub use native::{
    current_block_id, player_physics_position, request_invasion_warp, resolve_target,
};

/// Where a requested warp has got to.
///
/// A warp is NOT proven by the request succeeding -- that only shows the explicit-spawn slot
/// latched. It is proven by the player being read back at the destination, which is what
/// [`Self::Arrived`] means and what [`ORACLE_INVASION_WARP_FINAL_BLOCK`] /
/// [`ORACLE_INVASION_WARP_FINAL_POSITION`] report.
///
/// [`ORACLE_INVASION_WARP_FINAL_BLOCK`]: crate::oracles::ORACLE_INVASION_WARP_FINAL_BLOCK
/// [`ORACLE_INVASION_WARP_FINAL_POSITION`]: crate::oracles::ORACLE_INVASION_WARP_FINAL_POSITION
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WarpArrival {
    /// The stage kick was issued; the world has not settled yet.
    Pending { ticks_waited: u32 },
    /// The player is in the destination block, within tolerance of the requested position.
    Arrived {
        final_block: u32,
        final_position: [f32; 3],
        ticks_waited: u32,
    },
    /// The player settled somewhere the request did not ask for. A wrong landing is a FAILED
    /// warp and must be reported as one, never rounded up to success.
    Mislanded {
        final_block: u32,
        final_position: [f32; 3],
    },
    /// The world never settled within the budget. Unproven, not failed.
    TimedOut { ticks_waited: u32 },
}

/// How many game-task ticks a warp may stay [`WarpArrival::Pending`] before it is called out
/// as unproven.
///
/// This is a diagnostic budget, not a synchronisation mechanism: arrival is detected by the
/// settled block/position read-back, and this only bounds how long a never-settling warp is
/// allowed to report nothing.
pub const WARP_ARRIVAL_TICK_BUDGET: u32 = 3600;

/// Classify a settled read-back against what the warp asked for.
///
/// `expected_position` is the destination in PHYSICS space (the block-local `.aip` point run
/// back through the engine's conversion once the destination block is resident).
#[must_use]
pub fn classify_arrival(
    outcome: &WarpOutcome,
    ticks_waited: u32,
    settled: Option<(u32, [f32; 3])>,
    expected_position: Option<[f32; 3]>,
) -> WarpArrival {
    let Some((final_block, final_position)) = settled else {
        return if ticks_waited >= WARP_ARRIVAL_TICK_BUDGET {
            WarpArrival::TimedOut { ticks_waited }
        } else {
            WarpArrival::Pending { ticks_waited }
        };
    };
    if final_block != outcome.effective_block {
        // Still in the old block: the load has not handed over yet. Only call it a mislanding
        // once the world has stopped changing under us.
        return if ticks_waited >= WARP_ARRIVAL_TICK_BUDGET {
            WarpArrival::Mislanded {
                final_block,
                final_position,
            }
        } else {
            WarpArrival::Pending { ticks_waited }
        };
    }
    let Some(expected) = expected_position else {
        return WarpArrival::Pending { ticks_waited };
    };
    if crate::oracles::warp_arrival_within_tolerance(expected, final_position) {
        WarpArrival::Arrived {
            final_block,
            final_position,
            ticks_waited,
        }
    } else if ticks_waited >= WARP_ARRIVAL_TICK_BUDGET {
        WarpArrival::Mislanded {
            final_block,
            final_position,
        }
    } else {
        WarpArrival::Pending { ticks_waited }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invasion_warp::{AUTHORED_YAW_EXTREME, BlockKey};

    #[test]
    fn the_rvas_are_the_byte_checked_vas_minus_the_image_base() {
        // Guards against a transcription slip turning a verified VA into a crash-hook.
        for (rva, va) in [
            (SET_DISABLE_MAP_ENTER_ANIM_RVA, 0x1_4067_a850_usize),
            (SET_EXPLICIT_SPAWN_RVA, 0x1_4067_ab20),
            (GET_EXPLICIT_SPAWN_FLAG_RVA, 0x1_4067_a1c0),
            (GET_EXPLICIT_SPAWN_RVA, 0x1_4067_92a0),
            (WARP_NEXT_STAGE_KICK_RVA, 0x1_405f_7b70),
            (SETUP_MAP_REENTRY_RVA, 0x1_40ca_fc30),
            (SESSION_MANAGER_GLOBAL_RVA, 0x1_43d7_a4d0),
            (GET_CURRENT_MAP_ID_RVA, 0x1_405e_efb0),
        ] {
            assert_eq!(rva + RE_IMAGE_BASE, va, "rva 0x{rva:x} -> 0x{va:x}");
        }
    }

    /// Checked separately because the value lives in `er_game_base`, which is a windows-only
    /// dependency -- the shared declaration must still resolve to the byte-checked VA.
    #[cfg(windows)]
    #[test]
    fn the_shared_move_map_step_rva_is_the_byte_checked_va() {
        assert_eq!(
            SET_MOVE_MAP_STEP_BLOCK_ID_RVA + RE_IMAGE_BASE,
            0x1_4067_abd0
        );
    }

    #[test]
    fn the_spawn_position_carries_the_engines_w_and_the_raw_block_local_xyz() {
        // Block-local, NOT world-space: MoveMapStep converts it. Converting here double-adds.
        let position = spawn_position([12.5, -3.25, 400.0]);
        assert_eq!(position, FloatVector4::new(12.5, -3.25, 400.0, 1.0));
    }

    #[test]
    fn the_spawn_orientation_puts_yaw_in_y_and_zeroes_the_rest() {
        assert_eq!(
            spawn_orientation(-1.5),
            FloatVector4::new(0.0, -1.5, 0.0, 0.0)
        );
    }

    #[test]
    fn the_spawn_orientation_passes_the_raw_yaw_through_unwrapped() {
        // The authored table reaches AUTHORED_YAW_EXTREME (-6.28). Wrapping it here (as
        // heading_radians does, for compass display) would rotate half the catalog by a full turn.
        let raw = AUTHORED_YAW_EXTREME;
        assert_eq!(spawn_orientation(raw).y, raw);
        let target = InvasionWarpTarget::new(BlockKey::from_parts(60, 34, 51, 0), 0, [0.0; 3], raw);
        assert_ne!(
            spawn_orientation(target.yaw).y,
            target.heading_radians(),
            "the warp must use the raw yaw, not the display-wrapped one"
        );
    }

    #[test]
    fn the_spawn_orientation_does_not_negate() {
        // SosSignMan::SetMultiplayJoinData writes {0, spawnAngle, 0, 0} with no sign flip.
        assert_eq!(spawn_orientation(0.75).y, 0.75);
        assert_eq!(spawn_orientation(-0.75).y, -0.75);
    }

    #[test]
    fn the_vector_is_sixteen_byte_aligned_and_sixteen_bytes_long() {
        // ForceSetPosition loads this type with MOVAPS; misalignment is a #GP, not a slowdown.
        assert_eq!(core::mem::align_of::<FloatVector4>(), 16);
        assert_eq!(core::mem::size_of::<FloatVector4>(), 16);
    }

    #[test]
    fn the_none_block_sentinel_matches_the_catalogs() {
        assert_eq!(BLOCK_ID_NONE, crate::invasion_warp::BLOCK_KEY_NONE_RAW);
    }

    fn outcome(effective_block: u32) -> WarpOutcome {
        WarpOutcome {
            target: InvasionWarpTarget::new(
                BlockKey::from_raw(effective_block),
                0,
                [1.0, 2.0, 3.0],
                -0.5,
            ),
            origin_block: 0x3C21_2200,
            requested_block: effective_block,
            effective_block,
            spawn_flag: 1,
            spawn_position: [1.0, 2.0, 3.0],
            spawn_yaw: -0.5,
            session_touches: 1,
            session_gate: SessionGate::Entered,
        }
    }

    #[test]
    fn the_session_gate_summarises_to_the_count_vanilla_would_produce() {
        assert_eq!(SessionGate::Entered.touches(), 1);
        for skipped in [
            SessionGate::ManagerUnreadable,
            SessionGate::ManagerNull,
            SessionGate::StateUnreadable,
            SessionGate::NotInGame {
                state: SESSION_PROTOCOL_STATE_WAIT_REENTRY_TO_MAP,
                lobby_state: None,
            },
        ] {
            assert_eq!(skipped.touches(), 0, "{skipped:?}");
        }
    }

    #[test]
    fn every_skipped_gate_describes_itself_distinctly() {
        // The whole point of the enum: a `0` used to be four situations wearing one number, so a
        // live failure could not be attributed without another launch.
        let described: Vec<String> = [
            SessionGate::Entered,
            SessionGate::ManagerUnreadable,
            SessionGate::ManagerNull,
            SessionGate::StateUnreadable,
            SessionGate::NotInGame {
                state: SESSION_PROTOCOL_STATE_WAIT_REENTRY_TO_MAP,
                lobby_state: Some(SESSION_LOBBY_STATE_HOST),
            },
        ]
        .iter()
        .map(|gate| gate.describe())
        .collect();
        let unique: std::collections::BTreeSet<&String> = described.iter().collect();
        assert_eq!(unique.len(), described.len(), "{described:#?}");
    }

    #[test]
    fn only_the_reentry_state_counts_as_parked_by_our_own_previous_warp() {
        assert!(
            SessionGate::NotInGame {
                state: SESSION_PROTOCOL_STATE_WAIT_REENTRY_TO_MAP,
                lobby_state: None,
            }
            .is_parked_in_reentry()
        );
        // Any other non-InGame state is a different situation and must not be blamed on us.
        assert!(
            !SessionGate::NotInGame {
                state: 0,
                lobby_state: None,
            }
            .is_parked_in_reentry()
        );
        assert!(!SessionGate::Entered.is_parked_in_reentry());
        assert!(!SessionGate::ManagerNull.is_parked_in_reentry());
    }

    #[test]
    fn the_reentry_state_is_the_one_setup_map_reentry_writes() {
        // `140cafc47: movl $0x7,0x10(%rcx)` -- the first statement of SetupMapReentry. If this
        // ever disagrees with the binary, the gate's diagnosis is wrong rather than merely stale.
        assert_eq!(SESSION_PROTOCOL_STATE_WAIT_REENTRY_TO_MAP, 7);
        assert_ne!(
            SESSION_PROTOCOL_STATE_WAIT_REENTRY_TO_MAP,
            SESSION_PROTOCOL_STATE_IN_GAME
        );
    }

    #[test]
    fn an_unsettled_world_is_pending_not_arrived() {
        let arrival = classify_arrival(&outcome(0x3C22_3300), 10, None, None);
        assert_eq!(arrival, WarpArrival::Pending { ticks_waited: 10 });
    }

    #[test]
    fn a_world_that_never_settles_times_out_rather_than_claiming_success() {
        let arrival = classify_arrival(&outcome(0x3C22_3300), WARP_ARRIVAL_TICK_BUDGET, None, None);
        assert_eq!(
            arrival,
            WarpArrival::TimedOut {
                ticks_waited: WARP_ARRIVAL_TICK_BUDGET
            }
        );
    }

    #[test]
    fn landing_in_the_destination_block_within_tolerance_is_arrival() {
        let expected = [100.0, 50.0, 200.0];
        let arrival = classify_arrival(
            &outcome(0x3C22_3300),
            42,
            Some((0x3C22_3300, [100.5, 50.0, 200.0])),
            Some(expected),
        );
        assert_eq!(
            arrival,
            WarpArrival::Arrived {
                final_block: 0x3C22_3300,
                final_position: [100.5, 50.0, 200.0],
                ticks_waited: 42,
            }
        );
    }

    #[test]
    fn still_being_in_the_old_block_is_pending_until_the_budget_then_a_mislanding() {
        let expected = Some([0.0, 0.0, 0.0]);
        let early = classify_arrival(
            &outcome(0x3C22_3300),
            5,
            Some((0x3C21_2200, [0.0, 0.0, 0.0])),
            expected,
        );
        assert_eq!(early, WarpArrival::Pending { ticks_waited: 5 });
        let late = classify_arrival(
            &outcome(0x3C22_3300),
            WARP_ARRIVAL_TICK_BUDGET,
            Some((0x3C21_2200, [0.0, 0.0, 0.0])),
            expected,
        );
        assert_eq!(
            late,
            WarpArrival::Mislanded {
                final_block: 0x3C21_2200,
                final_position: [0.0, 0.0, 0.0],
            }
        );
    }

    #[test]
    fn landing_far_from_the_requested_point_is_a_mislanding_not_a_success() {
        // The failure this exists to catch: the explicit-spawn flag did not take and the engine
        // used the block's DEFAULT spawn. Right block, wrong place -- that is a failed warp.
        let arrival = classify_arrival(
            &outcome(0x3C22_3300),
            WARP_ARRIVAL_TICK_BUDGET,
            Some((0x3C22_3300, [9000.0, 0.0, 9000.0])),
            Some([100.0, 50.0, 200.0]),
        );
        assert!(
            matches!(arrival, WarpArrival::Mislanded { .. }),
            "{arrival:?}"
        );
    }

    #[test]
    fn every_warp_error_says_what_went_wrong_without_claiming_a_warp_happened() {
        let errors = [
            WarpError::NotAWarpDestination,
            WarpError::ModuleBase("boom".to_string()),
            WarpError::BlockIdIsNone,
            WarpError::SpawnSlotDidNotLatch { flag: 0 },
        ];
        for error in errors {
            let rendered = error.to_string();
            assert!(!rendered.is_empty());
            assert!(!rendered.contains("warped"), "{rendered}");
        }
    }

    #[test]
    fn the_policy_follows_the_attempt_and_defaults_to_allowing_warps() {
        // The product requirement, asserted directly rather than inferred from the absence of a
        // warp in a log. `request_invasion_warp` is `#[cfg(windows)]`, so this is the only place
        // the rule can be checked on the host -- which is exactly why the rule was made a value
        // the gate reads instead of an `if` buried in Windows-only code.
        //
        // Serialised against the other test that moves this latch: it is process-global, and two
        // tests toggling it in parallel would flake in a way that looks like a policy bug.
        let _guard = POLICY_LATCH.lock().unwrap_or_else(|e| e.into_inner());
        set_invasion_attempt_in_flight(false);
        assert_eq!(
            invasion_warp_policy(),
            WarpPolicy::Warpable,
            "with no attempt in flight the pins behave normally"
        );
        set_invasion_attempt_in_flight(true);
        assert_eq!(
            invasion_warp_policy(),
            WarpPolicy::MarkersOnly,
            "while an attempt is in flight every warp is refused"
        );
        set_invasion_attempt_in_flight(false);
    }

    /// Serialises the two tests that move the process-global attempt latch.
    static POLICY_LATCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn nothing_published_means_no_attempt_which_means_warps_are_allowed() {
        // The fail-safe DIRECTION, which is the part worth pinning. If the publisher never runs --
        // Seamless absent, session unresolvable, tracer not reached -- the latch stays at its
        // initial value, and that value decides which failure a player gets. Defaulting to
        // "in flight" would refuse every warp forever with no invasion to explain it, and nothing
        // on screen would say why. Defaulting to "idle" degrades to the pre-feature behaviour.
        let _guard = POLICY_LATCH.lock().unwrap_or_else(|e| e.into_inner());
        set_invasion_attempt_in_flight(false);
        assert!(!invasion_attempt_in_flight());
        assert_eq!(invasion_warp_policy(), WarpPolicy::Warpable);
    }

    #[test]
    fn the_refusal_names_the_reason_rather_than_looking_like_a_malfunction() {
        // This string is what a player sees in the log after selecting a pin and not moving. If it
        // reads like a failure they will report a bug; it has to read like a decision.
        let rendered = WarpError::NotAWarpDestination.to_string();
        assert!(rendered.contains("markers"), "{rendered}");
        assert!(
            rendered.contains("invasion attempt is in flight"),
            "the refusal must name the CONDITION, or it reads as a permanent ban: {rendered}"
        );
        assert!(
            rendered.contains("until it ends"),
            "and it must say the condition passes: {rendered}"
        );
        assert!(
            rendered.contains("nothing was written"),
            "a refusal must say the engine was left alone: {rendered}"
        );
    }
}
