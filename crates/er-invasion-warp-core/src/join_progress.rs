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
//! Timing the ERSC state was tried and it cancelled a real invasion five seconds after accepting
//! it, which is why `0x15` is excluded from the stall watchdog's timed states and pinned there by
//! a regression test. It is worse than risky, it is meaningless: `0x15` is not a handshake stage
//! at all. In `ersc.dll` the only instruction that can produce it computes
//! `((flags >> 19) & 1) * 9 + 12` -- 12 or 21 from one bit -- and `0x15` is never written as a
//! literal anywhere in the binary. A number that is a published flag cannot be timed as progress.
//!
//! So the question is asked of the game instead: has the engine got a join in flight, and has it
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
/// `WaitInitData`. So a positive value here is proof the engine is mid-handshake right now.
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
    /// The join succeeded -- the P2P session exists.
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
    /// `SetupMapReentry` -- the map reload has been ordered. For a guest this is set immediately
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
    /// The engine has nothing in flight. No RPC, no session, no timer running. If ERSC still
    /// believes an attempt is live while this holds, the attempt is dead in ERSC's own transport
    /// and no amount of waiting will produce a loading screen.
    Idle,
    /// The engine tried and the attempt died: `lobbyState` is `CreateFailed` or `JoinFailed`.
    ///
    /// Separated from `Idle` because the two are opposite diagnoses that used to print the same
    /// word. `Idle` means the engine never started, and points at ERSC's transport. `Failed` means
    /// the engine started, went out to Steam, and got an answer it could not use -- which is what a
    /// join to a host who cannot be reached looks like from here, a Steam block among the reasons.
    /// Reported live 2026-09-10: an invasion involving a blocked player "bugs out or times out",
    /// and this reading was the one that would have said so.
    Failed,
}

impl JoinProgress {
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        // Order matters: commitment beats everything, because acting after this point is what
        // cancelled a successful invasion before.
        if self.call_for_warp || self.protocol_state == protocol_state::WAIT_REENTRY_TO_MAP {
            return Verdict::Committed;
        }
        // A torn-down session, whatever the phase field says.
        //
        // `CSSessionManagerImp::DisconnectCleanup` sets `lobbyState = None` and does not write
        // `protocolState`, so a session that has been cleaned up leaves the phase reading whatever
        // it last was -- `InGame` after a real invasion. Without this the next test sees a
        // non-`None` phase and calls it progress forever.
        //
        // Measured live on run br-20260910-021456-38bc: `lobby=0 proto=6 rpc=0 joinCheck=0.0
        // waitInit=0.0` held while Seamless still read `0x16`, and the session clock ran to 2029
        // seconds. The player had to spend the invasion item to get out of a session the engine
        // had already destroyed.
        //
        // The three companion fields are required to be clear as well, so this cannot fire on a
        // frame sampled before `lobbyState` has been set for a join that is genuinely starting.
        if self.lobby_state == lobby_state::NONE
            && self.join_request_handle == 0
            && self.join_check_remain <= TIMER_EXPIRED
            && self.wait_init_remain <= TIMER_EXPIRED
        {
            return Verdict::Idle;
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
        if self.lobby_state == lobby_state::CREATE_FAILED
            || self.lobby_state == lobby_state::JOIN_FAILED
        {
            return Verdict::Failed;
        }
        Verdict::Idle
    }

    /// Whether this reading is a dead attempt: the engine has nothing in flight, whether because it
    /// never started or because what it started failed.
    ///
    /// The stall counter wants both and the log line wants them apart, so the distinction lives in
    /// the verdict and the union lives here rather than at each call site.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        matches!(self.verdict(), Verdict::Idle | Verdict::Failed)
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

    /// The reading measured live on 2026-08-16 while the player stood in their own world with
    /// Seamless loaded and no invasion under way: `lobbyState` moving 4 -> 5, `protocolState` 0.
    fn dead_reading() -> JoinProgress {
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
        let mut sample = dead_reading();
        sample.call_for_warp = true;
        assert_eq!(sample.verdict(), Verdict::Committed);

        let mut ordered = dead_reading();
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
                ..dead_reading()
            },
            JoinProgress {
                lobby_state: lobby_state::CLIENT,
                protocol_state: protocol_state::JOIN_CHECK,
                join_check_remain: 29.5,
                ..dead_reading()
            },
            JoinProgress {
                lobby_state: lobby_state::CLIENT,
                protocol_state: protocol_state::WAIT_INIT_DATA,
                wait_init_remain: 12.0,
                ..dead_reading()
            },
        ] {
            assert_eq!(sample.verdict(), Verdict::Progressing, "{sample}");
        }
    }

    #[test]
    fn the_measured_dead_reading_is_a_failed_join() {
        // This is the whole point: this exact reading, held while ERSC still claims a match, is
        // the signature of the stall that costs 53-213 seconds.
        //
        // Its `lobbyState` is `JoinFailed`, so the engine had already been out to Steam and been
        // told no. For two years of this module's life that read as `Idle` -- "the engine never
        // started" -- which is the opposite diagnosis and sent every investigation at ERSC's
        // transport instead of at the join.
        assert_eq!(dead_reading().verdict(), Verdict::Failed);
        assert!(dead_reading().is_finished());
    }

    #[test]
    fn a_running_phase_timer_alone_is_enough_to_be_progressing() {
        // The timers are zeroed every frame outside their phases, so a positive value cannot be
        // stale -- it is proof the engine ticked the handshake this frame.
        let mut sample = dead_reading();
        sample.join_check_remain = 0.1;
        assert_eq!(sample.verdict(), Verdict::Progressing);

        sample.join_check_remain = TIMER_EXPIRED;
        assert_eq!(
            sample.verdict(),
            Verdict::Failed,
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
                ..dead_reading()
            };
            assert_eq!(sample.verdict(), Verdict::Progressing, "lobbyState {state}");
        }
    }

    #[test]
    fn a_cleaned_up_session_is_idle_however_stale_the_phase_field_reads() {
        // The exact reading from run br-20260910-021456-38bc, held for 2029 seconds of session
        // clock while Seamless claimed a match.
        let sample = JoinProgress {
            lobby_state: lobby_state::NONE,
            protocol_state: protocol_state::IN_GAME,
            join_request_handle: 0,
            join_check_remain: 0.0,
            wait_init_remain: 0.0,
            call_for_warp: false,
        };
        assert_eq!(sample.verdict(), Verdict::Idle, "{sample}");
        assert!(sample.is_finished());

        // A join that is genuinely starting still wins: the companion fields are not clear.
        let starting = JoinProgress {
            join_request_handle: 4,
            ..sample
        };
        assert_eq!(starting.verdict(), Verdict::Progressing, "{starting}");

        // And a committed warp is never touched, whatever `lobbyState` reads.
        let committed = JoinProgress {
            call_for_warp: true,
            ..sample
        };
        assert_eq!(committed.verdict(), Verdict::Committed, "{committed}");
    }

    #[test]
    fn nothing_started_reads_as_idle_and_a_dead_attempt_does_not() {
        let sample = JoinProgress {
            lobby_state: lobby_state::NONE,
            ..dead_reading()
        };
        assert_eq!(sample.verdict(), Verdict::Idle);
        assert!(sample.is_finished());
        // `CreateFailed` and `JoinFailed` are the engine reporting that it tried and could not.
        // Reading them as `Idle` said the opposite -- that it never went out to Steam at all --
        // and it is the reading a join to an unreachable host produces.
        for state in [lobby_state::CREATE_FAILED, lobby_state::JOIN_FAILED] {
            let sample = JoinProgress {
                lobby_state: state,
                ..dead_reading()
            };
            assert_eq!(sample.verdict(), Verdict::Failed, "lobbyState {state}");
            assert!(sample.is_finished(), "lobbyState {state}");
        }
    }

    #[test]
    fn the_display_line_carries_every_field_a_diagnosis_needs() {
        let text = dead_reading().to_string();
        for needle in [
            "lobby=",
            "proto=",
            "rpc=",
            "joinCheck=",
            "waitInit=",
            "warp=",
            "Failed",
        ] {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
    }
}
