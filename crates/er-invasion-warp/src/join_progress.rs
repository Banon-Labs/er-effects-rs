//! Is a Seamless invasion join actually progressing toward a world load?
//!
//! # The failure this exists to see
//!
//! Measured 2026-08-16, four times in one session: our filter accepts a match (`Verdict::Keep`),
//! ERSC's session field sits at `0x15`, and **nothing further happens** for 53s, 59s, 91s and
//! 213s. Meanwhile the heartbeat shows the player's own coordinates changing every tick in their
//! own block -- they were running around their own world for three and a half minutes while the
//! session claimed a match. Seamless eventually gives up on its own; the times are not a constant
//! (a 4x spread), so waiting it out is not a bounded wait.
//!
//! # Why this is not a dwell timer
//!
//! Timing the ERSC state was tried and it cancelled a REAL invasion five seconds after accepting
//! it, which is why `0x15` is excluded from the stall watchdog's timed states and pinned there by
//! a regression test. It is worse than risky, it is meaningless: `0x15` is not a handshake stage
//! at all. In `ersc.dll` the only instruction that can produce it computes
//! `((flags >> 19) & 1) * 9 + 12` -- 12 or 21 from one bit -- and `0x15` is never written as a
//! literal anywhere in the binary. A number that is a published flag cannot be timed as progress.
//!
//! So the question is asked of the GAME instead: has the engine got a join in flight, and has it
//! committed to loading a world? Both are single reads of documented fields, and neither can be
//! confused with "the player is still standing in their own world".
//!
//! # What the engine's own timers cost
//!
//! `CSSessionManagerImp::Update` runs two 30-second phase countdowns (`joinCheckTimeout` then
//! `waitInitDataTimeout`), so once the handshake is genuinely under way the worst case is 60s.
//! That is the ceiling any early abort has to beat, and it is why the interesting reading is
//! "the engine never started" rather than "the engine is taking a while".

use core::fmt;

/// `GLOBAL_GameMan` -- `0x143d69918`. Holds the join destination `SetMultiplayJoinData` writes
/// and the flag `WarpNextStageKick_` sets when a warp is committed.
///
/// Derived, not restated: the address is declared once in `er-game-base`, so a future correction
/// there cannot leave this module pointing at a stale global.
pub use er_game_base::rva::GAME_MAN_SINGLETON_RVA as GAME_MAN_GLOBAL_RVA;
/// `GameMan::callForWarp`, written `true` by `WarpNextStageKick_` (`0x1405f7b70`) via
/// `SetCallForWarp` (`0x14067aea0`), whose whole body is `*(u8*)(gameMan + 0x10) = arg`.
///
/// This is the point of no return: the loading screen follows. One byte, and it cannot be
/// confused with a session state.
pub const GAME_MAN_CALL_FOR_WARP_OFFSET: usize = 0x10;

/// `CSSessionManagerImp::joinRequestHandle` -- non-zero while a Steam join RPC is outstanding.
/// `Update` consumes it and dispatches to the success path when it completes.
pub const SESSION_JOIN_REQUEST_HANDLE_OFFSET: usize = 0x28;
/// `FD4Time joinCheckTimeout`'s `time` field (`FD4Time` is `{ vfptr; f32 time; }`, so `+0x198 +
/// 0x8`). Counts DOWN from 30.0; `FD4Time::IsCompleted` is literally `0.0f >= time`.
pub const SESSION_JOIN_CHECK_REMAIN_OFFSET: usize = 0x1a0;
/// `FD4Time waitInitDataTimeout`'s `time` field (`+0x1a8 + 0x8`). Also 30.0.
pub const SESSION_WAIT_INIT_REMAIN_OFFSET: usize = 0x1b0;

/// The phase timers are force-zeroed every frame by the `else` branch of
/// `CSSessionManagerImp::Update` whenever `protocolState` is neither `JoinCheck` nor
/// `WaitInitData`. So a positive value here is proof the engine is mid-handshake RIGHT NOW.
pub const TIMER_EXPIRED: f32 = 0.0;

/// `LobbyState` (`CSSessionManagerImp + 0x0C`), from the immediates in the writing sites.
pub mod lobby_state {
    /// `DisconnectCleanup` -- no session.
    pub const NONE: i32 = 0;
    /// `CreateLobby` accepted the create RPC.
    pub const CREATING: i32 = 1;
    /// `CreateSession` reached this because `CreateLobby` returned false.
    pub const CREATE_FAILED: i32 = 2;
    /// `UpdateInitLobby` -- we are the host.
    pub const HOST: i32 = 3;
    /// The join RPC was accepted by Steam and is outstanding.
    pub const JOINING: i32 = 4;
    /// `CSSessionManager::JoinSession` failed synchronously; also fires `OnJoinFailed`.
    pub const JOIN_FAILED: i32 = 5;
    /// The join SUCCEEDED -- the P2P session exists.
    pub const CLIENT: i32 = 6;
    /// `LeaveSession` is unwinding.
    pub const CLOSING: i32 = 7;
}

/// `ProtocolState` (`CSSessionManagerImp + 0x10`).
pub mod protocol_state {
    pub const NONE: i32 = 0;
    pub const JOIN_CHECK: i32 = 1;
    pub const WAIT_INIT_DATA: i32 = 2;
    pub const WAIT_RELOAD_WAIT: i32 = 3;
    pub const WAIT_RELOAD: i32 = 4;
    pub const WAIT_RELOAD_2: i32 = 5;
    /// The world load finished.
    pub const IN_GAME: i32 = 6;
    /// `SetupMapReentry` -- the map reload has been ORDERED. For a guest this is set immediately
    /// before `WarpNextStageKick_`, so it is the earliest engine-side "we are going" marker.
    pub const WAIT_REENTRY_TO_MAP: i32 = 7;
}

/// One frame's reading of the engine-side join state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JoinProgress {
    pub lobby_state: i32,
    pub protocol_state: i32,
    /// Non-zero while a Steam join RPC is outstanding.
    pub join_request_handle: i32,
    /// Seconds left on `joinCheckTimeout`; `<= 0` means not running.
    pub join_check_remain: f32,
    /// Seconds left on `waitInitDataTimeout`; `<= 0` means not running.
    pub wait_init_remain: f32,
    /// `GameMan::callForWarp` -- the warp is kicked and a loading screen is coming.
    pub call_for_warp: bool,
}

/// What the engine is doing about this join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// A load is committed. Hands off -- this is the state in which a previous dwell-timer
    /// cancelled a real invasion.
    Committed,
    /// The engine has a join in flight: an outstanding RPC, a live session, or a running phase
    /// timer. It may still fail, but it is doing something and it has its own 30s bound.
    Progressing,
    /// The engine has NOTHING in flight. No RPC, no session, no timer running. If ERSC still
    /// believes an attempt is live while this holds, the attempt is dead in ERSC's own transport
    /// and no amount of waiting will produce a loading screen.
    Idle,
}

impl JoinProgress {
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        // Order matters: commitment beats everything, because acting after this point is what
        // cancelled a successful invasion before.
        if self.call_for_warp || self.protocol_state == protocol_state::WAIT_REENTRY_TO_MAP {
            return Verdict::Committed;
        }
        // A live P2P session, or any protocol phase past None, is the engine working.
        if self.lobby_state == lobby_state::CLIENT
            || self.protocol_state != protocol_state::NONE
            || self.join_request_handle != 0
            || self.join_check_remain > TIMER_EXPIRED
            || self.wait_init_remain > TIMER_EXPIRED
        {
            return Verdict::Progressing;
        }
        // `Joining` with no handle left is the ambiguous tail of an RPC we cannot see the end of;
        // treat it as progress rather than risk cancelling a join that is about to land.
        if matches!(
            self.lobby_state,
            lobby_state::CREATING | lobby_state::JOINING | lobby_state::HOST | lobby_state::CLOSING
        ) {
            return Verdict::Progressing;
        }
        Verdict::Idle
    }
}

impl fmt::Display for JoinProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "lobby={} proto={} rpc={} joinCheck={:.1} waitInit={:.1} warp={} -> {:?}",
            self.lobby_state,
            self.protocol_state,
            self.join_request_handle,
            self.join_check_remain,
            self.wait_init_remain,
            u8::from(self.call_for_warp),
            self.verdict()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reading measured live on 2026-08-16 while the player stood in their OWN world with
    /// Seamless loaded and no invasion under way: `lobbyState` moving 4 -> 5, `protocolState` 0.
    fn idle_seamless() -> JoinProgress {
        JoinProgress {
            lobby_state: lobby_state::JOIN_FAILED,
            protocol_state: protocol_state::NONE,
            join_request_handle: 0,
            join_check_remain: 0.0,
            wait_init_remain: 0.0,
            call_for_warp: false,
        }
    }

    #[test]
    fn a_committed_warp_is_never_interfered_with() {
        let mut sample = idle_seamless();
        sample.call_for_warp = true;
        assert_eq!(sample.verdict(), Verdict::Committed);

        let mut ordered = idle_seamless();
        ordered.protocol_state = protocol_state::WAIT_REENTRY_TO_MAP;
        assert_eq!(
            ordered.verdict(),
            Verdict::Committed,
            "the map reload is ordered before the warp flag is set; both must be hands-off"
        );
    }

    #[test]
    fn an_engine_with_a_join_in_flight_is_progressing() {
        for sample in [
            JoinProgress {
                lobby_state: lobby_state::JOINING,
                join_request_handle: 7,
                ..idle_seamless()
            },
            JoinProgress {
                lobby_state: lobby_state::CLIENT,
                protocol_state: protocol_state::JOIN_CHECK,
                join_check_remain: 29.5,
                ..idle_seamless()
            },
            JoinProgress {
                lobby_state: lobby_state::CLIENT,
                protocol_state: protocol_state::WAIT_INIT_DATA,
                wait_init_remain: 12.0,
                ..idle_seamless()
            },
        ] {
            assert_eq!(sample.verdict(), Verdict::Progressing, "{sample}");
        }
    }

    #[test]
    fn the_measured_dead_reading_is_idle() {
        // This is the whole point: this exact reading, held while ERSC still claims a match, is
        // the signature of the stall that costs 53-213 seconds.
        assert_eq!(idle_seamless().verdict(), Verdict::Idle);
    }

    #[test]
    fn a_running_phase_timer_alone_is_enough_to_be_progressing() {
        // The timers are zeroed every frame outside their phases, so a positive value cannot be
        // stale -- it is proof the engine ticked the handshake THIS frame.
        let mut sample = idle_seamless();
        sample.join_check_remain = 0.1;
        assert_eq!(sample.verdict(), Verdict::Progressing);

        sample.join_check_remain = TIMER_EXPIRED;
        assert_eq!(
            sample.verdict(),
            Verdict::Idle,
            "an expired timer is not progress"
        );
    }

    #[test]
    fn every_lobby_state_that_means_the_engine_is_busy_reads_as_progressing() {
        for state in [
            lobby_state::CREATING,
            lobby_state::HOST,
            lobby_state::JOINING,
            lobby_state::CLIENT,
            lobby_state::CLOSING,
        ] {
            let sample = JoinProgress {
                lobby_state: state,
                ..idle_seamless()
            };
            assert_eq!(sample.verdict(), Verdict::Progressing, "lobbyState {state}");
        }
    }

    #[test]
    fn only_the_settled_failure_states_read_as_idle() {
        for state in [
            lobby_state::NONE,
            lobby_state::CREATE_FAILED,
            lobby_state::JOIN_FAILED,
        ] {
            let sample = JoinProgress {
                lobby_state: state,
                ..idle_seamless()
            };
            assert_eq!(sample.verdict(), Verdict::Idle, "lobbyState {state}");
        }
    }

    #[test]
    fn the_display_line_carries_every_field_a_diagnosis_needs() {
        let text = idle_seamless().to_string();
        for needle in [
            "lobby=",
            "proto=",
            "rpc=",
            "joinCheck=",
            "waitInit=",
            "warp=",
            "Idle",
        ] {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
    }
}
