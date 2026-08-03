//! The user-facing trigger: three hotkeys that warp the player to invasion spawn points, and the
//! arrival oracle that decides whether the warp actually worked.
//!
//! F7 warps to the nearest point, F8 steps through the catalog's stable order, and F9 jumps to
//! another AREA entirely. Only F7 needs world coordinates -- see the note on the candidate set
//! in [`InvasionWarpDrive::tick`] for why that distinction is load-bearing.
//!
//! # Why hotkeys, when the goal is the world map
//!
//! The world-map surface is a shell around a warp call. Until that call is proven to relocate
//! the player and stream the destination, a map UI would be decoration over an unknown. These
//! hotkeys are the smallest trigger that exercises the whole payload -- catalog -> engine
//! coordinate conversion -> target choice -> explicit spawn -> stage kick -> settled read-back --
//! and they answer the questions static RE could not: whether the world streams at the
//! destination, whether the disaster remap moves the block under us, and whether the yaw lands
//! the way the `.aip` table authored it.
//!
//! # What "it worked" means here
//!
//! Not "the DLL loaded", not "no crash", and not "the hotkey fired". A warp counts only when
//! the player is READ BACK in the destination block within
//! [`er_invasion_warp::oracles::INVASION_WARP_POSITION_TOLERANCE_METRES`] of the requested
//! point ([`WarpArrival::Arrived`]). Landing in the right block at the wrong place is the
//! signature of the explicit-spawn slot not taking -- the engine falls back to the block's
//! default spawn -- and is reported as [`WarpArrival::Mislanded`], a FAILURE.

use er_invasion_warp::warp::{WARP_ARRIVAL_TICK_BUDGET, WarpArrival, WarpOutcome};

#[cfg(windows)]
use er_invasion_warp::invasion_warp::InvasionWarpTarget;
#[cfg(windows)]
use er_invasion_warp::select::{
    ResolvedTarget, first_in_other_area, nearest_from, next_target_from,
};
#[cfg(windows)]
use er_invasion_warp::warp::classify_arrival;

/// `VK_F7`: warp to the nearest invasion spawn point that is not the one under our feet.
pub const VK_WARP_NEAREST: i32 = 0x76;
/// `VK_F8`: step to the next invasion spawn point in the catalog's stable order. Unlike
/// "nearest" this crosses the map, because the order is by block id rather than by distance.
pub const VK_WARP_NEXT: i32 = 0x77;
/// `VK_F9`: jump to the first spawn point in a DIFFERENT area than the one the player is in --
/// base game <-> Shadow of the Erdtree. A deliberate single action rather than something
/// inferred from candidate counts, because "can the warp leave its own area" is the one
/// question a filtered candidate list cannot answer.
pub const VK_WARP_OTHER_AREA: i32 = 0x78;

/// `GetAsyncKeyState` sets the high bit while the key is held.
#[cfg(windows)]
const KEY_DOWN_MASK: i16 = -0x8000;
/// `GetAsyncKeyState`'s low bit: the key was pressed since the previous call on this thread.
/// This is what catches a press shorter than one game frame.
#[cfg(windows)]
const KEY_PRESSED_SINCE_MASK: i16 = 0x0001;
/// How often the driver logs a heartbeat while idle, in game-task ticks (~60/s, so ~10s).
///
/// Without it, "the hotkey never fired" and "the driver never ran" and "the window never had
/// focus" are the same silence, and a run cannot tell them apart.
#[cfg(windows)]
const HEARTBEAT_TICK_INTERVAL: u64 = 600;
/// The catalog read + per-target coordinate conversion is a deliberate one-frame cost paid on a
/// keypress. This bounds it so a corrupt catalog cannot turn one keypress into an unbounded
/// walk on the game thread.
#[cfg(windows)]
const MAX_TARGETS_TO_RESOLVE: usize = 16_384;

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(vkey: i32) -> i16;
    fn GetForegroundWindow() -> isize;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcessId() -> u32;
}

#[cfg(windows)]
/// Only react when Elden Ring itself has focus, so alt-tabbing to another window and pressing
/// F7 there does not teleport the player.
fn game_has_focus() -> bool {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd == 0 {
        return false;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &raw mut pid) };
    pid != 0 && pid == unsafe { GetCurrentProcessId() }
}

#[cfg(windows)]
/// Edge-detected key state: fires once per physical press, not once per frame held.
struct KeyEdge {
    vkey: i32,
    was_down: bool,
}

#[cfg(windows)]
impl KeyEdge {
    const fn new(vkey: i32) -> Self {
        Self {
            vkey,
            was_down: false,
        }
    }

    /// True once per physical press.
    ///
    /// Both bits of `GetAsyncKeyState` are used, and the low one is not optional. The high bit
    /// (`0x8000`) means "down right now", which this samples once per game frame -- so a press
    /// shorter than a frame falls between two samples and is never seen. The low bit (`0x0001`)
    /// means "was pressed since the previous call on this thread", which latches exactly that
    /// case. Without it a synthetic keystroke (and, rarely, a very fast human tap) does nothing
    /// at all, silently.
    fn pressed_this_tick(&mut self) -> bool {
        let state = unsafe { GetAsyncKeyState(self.vkey) };
        let down = (state & KEY_DOWN_MASK) != 0;
        let pressed_since_last_call = (state & KEY_PRESSED_SINCE_MASK) != 0;
        let edge = (down && !self.was_down) || pressed_since_last_call;
        self.was_down = down;
        edge
    }

    /// The raw state word, for the diagnostic heartbeat.
    fn raw_state(&self) -> i16 {
        unsafe { GetAsyncKeyState(self.vkey) }
    }

    /// Drop the latch when we stop listening (focus lost), so returning to the game does not
    /// replay a press that happened in another window.
    fn forget(&mut self) {
        self.was_down = false;
    }
}

#[cfg(windows)]
/// What the driver is doing right now.
enum DriveState {
    /// Waiting for a keypress.
    Idle,
    /// A warp was issued; waiting for the world to settle so arrival can be judged.
    AwaitingArrival {
        outcome: Box<WarpOutcome>,
        ticks_waited: u32,
    },
}

#[cfg(windows)]
/// The whole driver. One instance, owned by the game task.
pub struct InvasionWarpDrive {
    nearest_key: KeyEdge,
    next_key: KeyEdge,
    other_area_key: KeyEdge,
    state: DriveState,
    /// Stable id of the last target warped to, so [`next_from`] advances instead of repeating.
    last_target_id: Option<u64>,
    warps_issued: u32,
    warps_arrived: u32,
    /// Game-task ticks seen, for the heartbeat cadence.
    ticks: u64,
}

#[cfg(windows)]
impl Default for InvasionWarpDrive {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl InvasionWarpDrive {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            nearest_key: KeyEdge::new(VK_WARP_NEAREST),
            next_key: KeyEdge::new(VK_WARP_NEXT),
            other_area_key: KeyEdge::new(VK_WARP_OTHER_AREA),
            state: DriveState::Idle,
            last_target_id: None,
            warps_issued: 0,
            warps_arrived: 0,
            ticks: 0,
        }
    }

    /// One game-task tick.
    ///
    /// # Safety
    ///
    /// Must run on the game task thread with the runtime singletons resolved.
    pub unsafe fn tick(&mut self, log: fn(std::fmt::Arguments<'_>), publish: fn(&str)) {
        if !er_invasion_warp::host::invasion_warp_enabled() {
            return;
        }
        let Ok(base) = er_game_base::mem::game_module_base() else {
            return;
        };

        // An in-flight warp is judged first: the settled read-back is the proof, and it must be
        // taken before any new press is honoured.
        if let DriveState::AwaitingArrival {
            outcome,
            ticks_waited,
        } = &mut self.state
        {
            *ticks_waited = ticks_waited.saturating_add(1);
            let settled = unsafe { settled_reading(base) };
            let expected = unsafe {
                er_invasion_warp::warp::resolve_target(base, &outcome.target)
                    .map(|resolved| resolved.world_position)
            };
            let arrival = classify_arrival(outcome, *ticks_waited, settled, expected);
            match arrival {
                WarpArrival::Pending { .. } => return,
                WarpArrival::Arrived { .. } => {
                    self.warps_arrived = self.warps_arrived.saturating_add(1);
                }
                WarpArrival::Mislanded { .. } | WarpArrival::TimedOut { .. } => {}
            }
            let outcome = *outcome.clone();
            log(format_args!(
                "invasion-warp: {}",
                describe_arrival(&outcome, &arrival)
            ));
            publish(&warp_oracle_json(
                &outcome,
                &arrival,
                expected,
                self.warps_issued,
                self.warps_arrived,
            ));
            self.state = DriveState::Idle;
            return;
        }

        self.ticks = self.ticks.wrapping_add(1);
        let focused = game_has_focus();

        // The heartbeat exists because every "nothing happened" path below is silent, and
        // during the first live run they were indistinguishable: no focus, no keypress and no
        // driver all looked identical in the log.
        if self.ticks % HEARTBEAT_TICK_INTERVAL == 0 {
            let player = unsafe { er_invasion_warp::warp::player_physics_position(base) };
            log(format_args!(
                "invasion-warp: heartbeat tick={} focused={focused} f7_state={:#06x} \
                 f8_state={:#06x} player={} -- press F7 (nearest), F8 (next), F9 (other area)",
                self.ticks,
                self.nearest_key.raw_state() as u16,
                self.next_key.raw_state() as u16,
                player.map_or_else(
                    || "none".to_string(),
                    |p| format!("[{:.1}, {:.1}, {:.1}]", p[0], p[1], p[2])
                ),
            ));
        }

        if !focused {
            self.nearest_key.forget();
            self.next_key.forget();
            self.other_area_key.forget();
            return;
        }
        let want_nearest = self.nearest_key.pressed_this_tick();
        let want_next = self.next_key.pressed_this_tick();
        let want_other_area = self.other_area_key.pressed_this_tick();
        if !want_nearest && !want_next && !want_other_area {
            return;
        }
        log(format_args!(
            "invasion-warp: hotkey edge detected (nearest={want_nearest} next={want_next})"
        ));

        let Some(player_position) =
            (unsafe { er_invasion_warp::warp::player_physics_position(base) })
        else {
            log(format_args!(
                "invasion-warp: hotkey ignored -- no local player yet (load into the world first)"
            ));
            return;
        };

        // The FULL catalog is the candidate set for everything except "nearest".
        //
        // Only "nearest" needs world coordinates, because only it needs distances. The warp
        // itself never does: the explicit-spawn slot takes BLOCK-LOCAL coordinates and
        // MoveMapStep runs ConvertBlockCoordsToPhysicsCoords on them once the destination area
        // has loaded. Filtering every candidate through that conversion up front -- it resolves
        // only blocks in the player's currently resident area -- silently made every OTHER area
        // unreachable. A live run showed it as "2591 of 7073 targets converted" while standing
        // in the DLC: exactly the dlc02 count, with the whole base game excluded.
        let catalog =
            match unsafe { er_invasion_warp::invasion_warp::collect_invasion_warp_catalog() } {
                Ok(catalog) => catalog,
                Err(error) => {
                    log(format_args!(
                        "invasion-warp: hotkey ignored -- catalog unavailable: {error}"
                    ));
                    return;
                }
            };
        let targets = catalog.targets();
        if targets.is_empty() {
            log(format_args!(
                "invasion-warp: hotkey ignored -- the catalog is empty"
            ));
            return;
        }

        let target = if want_nearest {
            let resolved = unsafe { resolve_catalog(base, log) };
            match nearest_from(&resolved, player_position) {
                Some(index) => resolved[index].target,
                None => {
                    log(format_args!(
                        "invasion-warp: hotkey ignored -- no NEAREST target ({} of {} placeable \
                         in this area, and none outside the same-point radius)",
                        resolved.len(),
                        targets.len()
                    ));
                    return;
                }
            }
        } else if want_next {
            match next_target_from(targets, self.last_target_id) {
                Some(index) => targets[index],
                None => return,
            }
        } else {
            let current_area = unsafe { er_invasion_warp::warp::current_block_id(base) }
                .map(|block| er_invasion_warp::invasion_warp::BlockKey::from_raw(block).area());
            let Some(current_area) = current_area else {
                log(format_args!(
                    "invasion-warp: hotkey ignored -- current block id unavailable, so 'other \
                     area' has nothing to be other than"
                ));
                return;
            };
            match first_in_other_area(targets, current_area) {
                Some(index) => {
                    log(format_args!(
                        "invasion-warp: cross-area jump: leaving area {current_area} for area {}",
                        targets[index].block.area()
                    ));
                    targets[index]
                }
                None => {
                    log(format_args!(
                        "invasion-warp: hotkey ignored -- every catalog target is already in \
                         area {current_area}"
                    ));
                    return;
                }
            }
        };

        match unsafe { er_invasion_warp::warp::request_invasion_warp(&target) } {
            Ok(outcome) => {
                self.warps_issued = self.warps_issued.saturating_add(1);
                self.last_target_id = Some(target.stable_id());
                log(format_args!(
                    "invasion-warp: warp issued via {} -> block {} (requested {:#010x}, effective \
                     {:#010x}) point {} pos [{:.2}, {:.2}, {:.2}] yaw {:.4} spawn_flag={} \
                     session_touches={} candidates={}",
                    if want_nearest {
                        "NEAREST"
                    } else if want_next {
                        "NEXT"
                    } else {
                        "OTHER-AREA"
                    },
                    target.block,
                    outcome.requested_block,
                    outcome.effective_block,
                    target.point_index,
                    outcome.spawn_position[0],
                    outcome.spawn_position[1],
                    outcome.spawn_position[2],
                    outcome.spawn_yaw,
                    outcome.spawn_flag,
                    outcome.session_touches,
                    targets.len(),
                ));
                self.state = DriveState::AwaitingArrival {
                    outcome: Box::new(outcome),
                    ticks_waited: 0,
                };
            }
            Err(error) => {
                log(format_args!(
                    "invasion-warp: warp REFUSED for block {} point {}: {error}",
                    target.block, target.point_index
                ));
            }
        }
    }
}

#[cfg(windows)]
/// The settled `(block, position)` reading arrival is judged against, or `None` while either
/// half is unavailable (mid-load the player does not exist).
///
/// # Safety
///
/// Game task thread.
unsafe fn settled_reading(base: usize) -> Option<(u32, [f32; 3])> {
    let block = unsafe { er_invasion_warp::warp::current_block_id(base) }?;
    let position = unsafe { er_invasion_warp::warp::player_physics_position(base) }?;
    Some((block, position))
}

#[cfg(windows)]
/// Read the live catalog and convert every target through the engine's own
/// `ConvertBlockCoordsToPhysicsCoords`, dropping the ones it refuses.
///
/// # Safety
///
/// Game task thread.
unsafe fn resolve_catalog(base: usize, log: fn(std::fmt::Arguments<'_>)) -> Vec<ResolvedTarget> {
    let catalog = match unsafe { er_invasion_warp::invasion_warp::collect_invasion_warp_catalog() }
    {
        Ok(catalog) => catalog,
        Err(error) => {
            log(format_args!(
                "invasion-warp: catalog unavailable at hotkey time: {error}"
            ));
            return Vec::new();
        }
    };
    let targets: &[InvasionWarpTarget] = catalog.targets();
    let considered = targets.len().min(MAX_TARGETS_TO_RESOLVE);
    if targets.len() > considered {
        log(format_args!(
            "invasion-warp: catalog has {} targets, resolving only the first {considered} \
             (MAX_TARGETS_TO_RESOLVE)",
            targets.len()
        ));
    }
    let mut resolved = Vec::with_capacity(considered);
    for target in &targets[..considered] {
        if let Some(entry) = unsafe { er_invasion_warp::warp::resolve_target(base, target) } {
            resolved.push(entry);
        }
    }
    if resolved.len() != considered {
        log(format_args!(
            "invasion-warp: {} of {considered} targets converted to world coordinates; the rest \
             sit in blocks whose world info is not resident",
            resolved.len()
        ));
    }
    resolved
}

/// One human-readable line describing how a warp ended.
#[must_use]
pub fn describe_arrival(outcome: &WarpOutcome, arrival: &WarpArrival) -> String {
    match arrival {
        WarpArrival::Pending { ticks_waited } => {
            format!("warp pending after {ticks_waited} ticks")
        }
        WarpArrival::Arrived {
            final_block,
            final_position,
            ticks_waited,
        } => format!(
            "ARRIVED block {final_block:#010x} pos [{:.2}, {:.2}, {:.2}] after {ticks_waited} \
             ticks (requested block {:#010x})",
            final_position[0], final_position[1], final_position[2], outcome.effective_block
        ),
        WarpArrival::Mislanded {
            final_block,
            final_position,
        } => format!(
            "MISLANDED block {final_block:#010x} pos [{:.2}, {:.2}, {:.2}] -- expected block \
             {:#010x} near [{:.2}, {:.2}, {:.2}]; the explicit-spawn slot did not take",
            final_position[0],
            final_position[1],
            final_position[2],
            outcome.effective_block,
            outcome.spawn_position[0],
            outcome.spawn_position[1],
            outcome.spawn_position[2]
        ),
        WarpArrival::TimedOut { ticks_waited } => format!(
            "UNPROVEN: the world never settled within {ticks_waited} ticks (budget \
             {WARP_ARRIVAL_TICK_BUDGET})"
        ),
    }
}

/// The warp oracle document, using the names `er_invasion_warp::oracles` reserved for them.
#[must_use]
pub fn warp_oracle_json(
    outcome: &WarpOutcome,
    arrival: &WarpArrival,
    expected_physics: Option<[f32; 3]>,
    warps_issued: u32,
    warps_arrived: u32,
) -> String {
    use er_invasion_warp::oracles::{
        ORACLE_INVASION_WARP_FINAL_BLOCK, ORACLE_INVASION_WARP_FINAL_POSITION,
        ORACLE_INVASION_WARP_REQUESTED_BLOCK, ORACLE_INVASION_WARP_REQUESTED_POSITION,
        ORACLE_INVASION_WARP_REQUESTED_YAW, ORACLE_INVASION_WARP_SELECTED_ID,
        ORACLE_INVASION_WARP_SESSION_TOUCHES, encode_position_oracle, encode_scalar_oracle,
    };
    let (verdict, passed) = match arrival {
        WarpArrival::Arrived { .. } => ("arrived", true),
        WarpArrival::Mislanded { .. } => ("mislanded", false),
        WarpArrival::TimedOut { .. } => ("unproven_timeout", false),
        WarpArrival::Pending { .. } => ("pending", false),
    };
    let (final_block, final_position) = match arrival {
        WarpArrival::Arrived {
            final_block,
            final_position,
            ..
        }
        | WarpArrival::Mislanded {
            final_block,
            final_position,
        } => (Some(*final_block), Some(*final_position)),
        _ => (None, None),
    };
    let requested = encode_position_oracle(outcome.spawn_position);
    let final_encoded = final_position.map(encode_position_oracle);
    // The two position fields are in DIFFERENT SPACES and must be labelled as such.
    // `requested_position` is the BLOCK-LOCAL .aip value read back out of GameMan+0xc90 --
    // that is what the engine was handed. `final_position` is the player's PHYSICS position.
    // They coincide only where the block origin happens to be ~0, which made a live run look
    // self-consistent by luck. `expected_position_physics` is the requested point put through
    // the engine's own conversion, and IS the value the arrival verdict compares against.
    let expected_encoded = expected_physics.map(encode_position_oracle);
    format!(
        "{{\"{ORACLE_INVASION_WARP_SELECTED_ID}\":{selected},\
         \"{ORACLE_INVASION_WARP_REQUESTED_BLOCK}\":{requested_block},\
         \"{ORACLE_INVASION_WARP_REQUESTED_POSITION}\":[{rx},{ry},{rz}],\
         \"{ORACLE_INVASION_WARP_REQUESTED_YAW}\":{yaw},\
         \"requested_position_space\":\"block_local\",\
         \"expected_position_physics\":{expected_position_json},\
         \"final_position_space\":\"physics\",\
         \"{ORACLE_INVASION_WARP_FINAL_BLOCK}\":{final_block_json},\
         \"{ORACLE_INVASION_WARP_FINAL_POSITION}\":{final_position_json},\
         \"{ORACLE_INVASION_WARP_SESSION_TOUCHES}\":{session_touches},\
         \"effective_block\":{effective_block},\"spawn_flag\":{spawn_flag},\
         \"warps_issued\":{warps_issued},\"warps_arrived\":{warps_arrived},\
         \"verdict\":\"{verdict}\",\"passed\":{passed},\
         \"session_touches_note\":\"vanilla TriggerAreaReload calls SetupMapReentry when \
         protocolState==InGame; this counts that call rather than asserting zero\"}}",
        selected = outcome.target.stable_id(),
        requested_block = outcome.requested_block,
        rx = requested[0],
        ry = requested[1],
        rz = requested[2],
        yaw = encode_scalar_oracle(outcome.spawn_yaw),
        expected_position_json = expected_encoded.map_or_else(
            || "null".to_string(),
            |p| format!("[{},{},{}]", p[0], p[1], p[2])
        ),
        final_block_json = final_block.map_or_else(|| "null".to_string(), |b| b.to_string()),
        final_position_json = final_encoded.map_or_else(
            || "null".to_string(),
            |p| format!("[{},{},{}]", p[0], p[1], p[2])
        ),
        session_touches = outcome.session_touches,
        effective_block = outcome.effective_block,
        spawn_flag = outcome.spawn_flag,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    // Imported here rather than relying on the module-level import, which is cfg(windows):
    // these tests are the host-side coverage of the arrival verdicts and must build on Linux.
    use er_invasion_warp::invasion_warp::{BlockKey, InvasionWarpTarget};

    fn outcome() -> WarpOutcome {
        WarpOutcome {
            target: InvasionWarpTarget::new(
                BlockKey::from_parts(60, 34, 51, 0),
                7,
                [10.0, 20.0, 30.0],
                -1.25,
            ),
            origin_block: 0x3C21_2200,
            requested_block: 0x3C22_3300,
            effective_block: 0x3C22_3300,
            spawn_flag: 1,
            spawn_position: [10.0, 20.0, 30.0],
            spawn_yaw: -1.25,
            session_touches: 1,
        }
    }

    #[test]
    fn the_two_hotkeys_are_distinct_function_keys() {
        assert_ne!(VK_WARP_NEAREST, VK_WARP_NEXT);
        assert_eq!(VK_WARP_NEAREST, 0x76, "F7");
        assert_eq!(VK_WARP_NEXT, 0x77, "F8");
    }

    #[test]
    fn only_an_arrival_reports_passed() {
        // The whole point: a warp that was merely ISSUED must never read as success.
        let cases = [
            (
                WarpArrival::Arrived {
                    final_block: 0x3C22_3300,
                    final_position: [10.0, 20.0, 30.0],
                    ticks_waited: 5,
                },
                true,
            ),
            (
                WarpArrival::Mislanded {
                    final_block: 0x3C22_3300,
                    final_position: [900.0, 0.0, 900.0],
                },
                false,
            ),
            (WarpArrival::TimedOut { ticks_waited: 60 }, false),
            (WarpArrival::Pending { ticks_waited: 1 }, false),
        ];
        for (arrival, expected) in cases {
            let json = warp_oracle_json(&outcome(), &arrival, None, 1, u32::from(expected));
            assert!(
                json.contains(&format!("\"passed\":{expected}")),
                "{arrival:?} -> {json}"
            );
        }
    }

    #[test]
    fn a_pending_warp_reports_null_final_values_rather_than_zeros() {
        // A zero would read as "settled at the origin"; null says "not measured yet".
        let json = warp_oracle_json(
            &outcome(),
            &WarpArrival::Pending { ticks_waited: 3 },
            None,
            1,
            0,
        );
        assert!(
            json.contains("\"oracle_invasion_warp_final_block\":null"),
            "{json}"
        );
        assert!(
            json.contains("\"oracle_invasion_warp_final_position\":null"),
            "{json}"
        );
    }

    #[test]
    fn the_two_position_fields_are_labelled_with_their_coordinate_space() {
        // A live run had requested_position (block-local) numerically EQUAL to final_position
        // (physics) because that block's origin was ~0, which made a mixed-space document look
        // self-consistent by luck. The labels stop that reading.
        let json = warp_oracle_json(
            &outcome(),
            &WarpArrival::Arrived {
                final_block: 0x3C22_3300,
                final_position: [10.0, 20.0, 30.0],
                ticks_waited: 9,
            },
            Some([1000.0, 20.0, 30.0]),
            1,
            1,
        );
        assert!(
            json.contains("\"requested_position_space\":\"block_local\""),
            "{json}"
        );
        assert!(
            json.contains("\"final_position_space\":\"physics\""),
            "{json}"
        );
        // The physics-space expectation is the value the verdict actually compared against,
        // and it must be present and distinct from the block-local one.
        assert!(
            json.contains("\"expected_position_physics\":[1000000,20000,30000]"),
            "{json}"
        );
    }

    #[test]
    fn the_session_touch_count_is_reported_not_asserted_to_be_zero() {
        let json = warp_oracle_json(
            &outcome(),
            &WarpArrival::Pending { ticks_waited: 0 },
            None,
            1,
            0,
        );
        assert!(
            json.contains("\"oracle_invasion_warp_session_touches\":1"),
            "{json}"
        );
        assert!(json.contains("session_touches_note"), "{json}");
    }

    #[test]
    fn the_mislanding_description_names_the_explicit_spawn_slot_as_the_suspect() {
        let text = describe_arrival(
            &outcome(),
            &WarpArrival::Mislanded {
                final_block: 0x3C22_3300,
                final_position: [900.0, 0.0, 900.0],
            },
        );
        assert!(text.contains("MISLANDED"), "{text}");
        assert!(text.contains("explicit-spawn"), "{text}");
    }

    #[test]
    fn the_arrival_description_carries_the_settled_block_and_position() {
        let text = describe_arrival(
            &outcome(),
            &WarpArrival::Arrived {
                final_block: 0x3C22_3300,
                final_position: [10.0, 20.0, 30.0],
                ticks_waited: 12,
            },
        );
        assert!(text.contains("ARRIVED"), "{text}");
        assert!(text.contains("12"), "{text}");
    }

    #[test]
    fn the_selected_id_survives_into_the_oracle_document() {
        let outcome = outcome();
        let json = warp_oracle_json(
            &outcome,
            &WarpArrival::Pending { ticks_waited: 0 },
            None,
            1,
            0,
        );
        assert!(
            json.contains(&format!(
                "\"oracle_invasion_warp_selected_id\":{}",
                outcome.target.stable_id()
            )),
            "{json}"
        );
    }
}
