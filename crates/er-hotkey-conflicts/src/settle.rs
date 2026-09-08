//! When "everything has loaded and the checks have run" is actually true.
//!
//! The brief for this DLL is one warning, printed once the profile has settled. A sleep would be
//! the obvious way to decide that and the wrong one: me3 loads natives in profile order, each one
//! spawns its own install thread, and several of them arm their input hooks lazily from a game
//! frame rather than at attach. A fixed delay is a guess about all of that, and it is a guess that
//! silently reports half a profile whenever a machine is slower than the machine it was tuned on.
//!
//! So the gate is evidence-driven, and it needs both halves:
//!
//! * **The module list has stopped changing.** A signature over the loaded modules, unchanged for
//!   [`STABLE_TICKS`] consecutive frames. This is what says every DLL is in.
//! * **Input is actually being polled.** [`MIN_CALLS`] observed calls. The module list goes stable
//!   at the title screen, long before any mod's per-frame hotkey loop is running, so stability
//!   alone would fire the report against an empty census and call the profile clean.
//!
//! And a backstop, because a report that never prints is worse than an early one: after
//! [`BACKSTOP_TICKS`] the gate fires regardless. A profile where no hook ever fired still gets its
//! verdict, which in that case is `UNKNOWN` and says so.

// Windows-only in practice; ungated so the gate is covered by `cargo test` -- it is a state
// machine whose failure mode is a report that never appears, which no other gate would catch.
#![cfg_attr(not(windows), allow(dead_code))]

/// Consecutive frames the loaded-module list must be unchanged. Ten seconds at 60fps.
///
/// Generous on purpose: the cost of waiting is a later log line, and the cost of firing early is a
/// warning that names half a profile and is therefore wrong.
pub const STABLE_TICKS: u64 = 600;

/// Input calls that must have been observed before the census is worth reporting on.
///
/// Roughly ten seconds of one mod polling one key each frame. Below this the census is not yet
/// evidence of anything.
pub const MIN_CALLS: u64 = 600;

/// Frames after which the report fires whatever the evidence looks like. Two minutes at 60fps.
pub const BACKSTOP_TICKS: u64 = 7200;

/// Fires exactly once, when the profile has settled or the backstop expires.
#[derive(Clone, Debug, Default)]
pub struct SettleGate {
    ticks: u64,
    stable_for: u64,
    last_signature: Option<u64>,
    fired: bool,
}

/// Why the gate fired, which the report needs in order to caveat itself correctly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Settled {
    /// The module list went stable and enough input was seen.
    Evidence,
    /// The backstop expired first. The census may be thin, and the report says so by way of its
    /// own call count.
    Backstop,
}

impl SettleGate {
    /// Fold in one frame. Returns `Some` exactly once per process.
    pub fn observe(&mut self, module_signature: u64, calls_seen: u64) -> Option<Settled> {
        if self.fired {
            return None;
        }
        self.ticks = self.ticks.saturating_add(1);
        if self.last_signature == Some(module_signature) {
            self.stable_for = self.stable_for.saturating_add(1);
        } else {
            self.last_signature = Some(module_signature);
            self.stable_for = 0;
        }
        let settled = self.stable_for >= STABLE_TICKS && calls_seen >= MIN_CALLS;
        let backstop = self.ticks >= BACKSTOP_TICKS;
        if !settled && !backstop {
            return None;
        }
        self.fired = true;
        Some(if settled {
            Settled::Evidence
        } else {
            Settled::Backstop
        })
    }

    /// Has the report already been produced?
    pub const fn fired(&self) -> bool {
        self.fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNATURE: u64 = 0xabc;

    fn run(gate: &mut SettleGate, frames: u64, signature: u64, calls: u64) -> Option<Settled> {
        let mut fired = None;
        for _ in 0..frames {
            if let Some(reason) = gate.observe(signature, calls) {
                fired = Some(reason);
            }
        }
        fired
    }

    #[test]
    fn a_stable_module_list_with_traffic_settles() {
        let mut gate = SettleGate::default();
        assert_eq!(
            run(&mut gate, STABLE_TICKS + 1, SIGNATURE, MIN_CALLS),
            Some(Settled::Evidence)
        );
    }

    /// The failure this gate's second half exists to prevent: the module list is stable at the
    /// title screen minutes before any mod's hotkey loop runs, and firing there would report an
    /// empty census as a clean profile.
    #[test]
    fn a_stable_module_list_with_no_traffic_does_not_settle() {
        let mut gate = SettleGate::default();
        assert_eq!(run(&mut gate, STABLE_TICKS + 1, SIGNATURE, 0), None);
        assert!(!gate.fired());
    }

    /// A DLL loading late resets the clock -- that is the whole point of watching the list.
    #[test]
    fn a_late_module_restarts_the_stability_clock() {
        let mut gate = SettleGate::default();
        run(&mut gate, STABLE_TICKS - 1, SIGNATURE, MIN_CALLS);
        assert_eq!(gate.observe(SIGNATURE + 1, MIN_CALLS), None);
        assert_eq!(
            run(&mut gate, STABLE_TICKS - 1, SIGNATURE + 1, MIN_CALLS),
            None
        );
        assert_eq!(
            gate.observe(SIGNATURE + 1, MIN_CALLS),
            Some(Settled::Evidence)
        );
    }

    /// A report that never prints is worse than an early one, so silence has a deadline.
    #[test]
    fn the_backstop_fires_even_with_no_traffic_at_all() {
        let mut gate = SettleGate::default();
        assert_eq!(
            run(&mut gate, BACKSTOP_TICKS, SIGNATURE, 0),
            Some(Settled::Backstop)
        );
    }

    /// One warning means one warning.
    #[test]
    fn the_gate_fires_exactly_once() {
        let mut gate = SettleGate::default();
        assert!(run(&mut gate, BACKSTOP_TICKS * 2, SIGNATURE, MIN_CALLS).is_some());
        assert_eq!(gate.observe(SIGNATURE, MIN_CALLS), None);
        assert_eq!(gate.observe(SIGNATURE + 9, 0), None);
    }

    /// A module list that never stabilises still hits the backstop rather than waiting forever.
    #[test]
    fn a_churning_module_list_still_reaches_the_backstop() {
        let mut gate = SettleGate::default();
        let mut fired = None;
        for tick in 0..BACKSTOP_TICKS {
            if let Some(reason) = gate.observe(tick, MIN_CALLS) {
                fired = Some(reason);
            }
        }
        assert_eq!(fired, Some(Settled::Backstop));
    }
}
