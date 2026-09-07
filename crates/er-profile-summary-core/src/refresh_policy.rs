//! When may the picked-save `CS::ProfileSummary` re-read run again, and when has it given up?
//!
//! Host-portable on purpose: the whole re-read is otherwise reachable only from a live boot, so
//! the throttle -- the one thing standing between a ~26 MB file read and a per-frame ~26 MB file
//! read -- had no test at all.

/// Autoload ticks between re-read attempts.
///
/// The work is a ~26 MB file read plus ten record rewrites, so it must not run per frame; and
/// there is nothing to gain from trying faster, because what it is waiting for (`GameDataMan` ->
/// `ProfileSummary` coming up) is a boot milestone, not a race.
pub const REFRESH_ATTEMPT_INTERVAL_TICKS: usize = 30;

/// Hard cap on real attempts. Every failure is structural -- no summary pointer yet, no
/// resolvable staged path, an unreadable file, a container with no active slots -- so retrying
/// forever would only churn a 26 MB read behind a boot that is never going to succeed. The cap
/// is generous enough (40 x 30 ticks = 1200 ticks, ~20 s at 60 fps) to outlast the summary
/// allocation.
pub const REFRESH_MAX_ATTEMPTS: usize = 40;

/// What this tick should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshStep {
    /// Throttled: not on an interval boundary. Do nothing, cheaply.
    Wait,
    /// Run attempt number N (1-based) and store N as the new attempt count.
    Attempt(usize),
    /// The attempt budget is spent; never read the container again this session.
    Exhausted,
}

/// Decide this tick from the tick counter and the attempts already spent.
///
/// `tick` is the value before the increment (the `fetch_add` result), matching the caller.
#[must_use]
pub fn refresh_step(tick: usize, attempts: usize) -> RefreshStep {
    if !tick.is_multiple_of(REFRESH_ATTEMPT_INTERVAL_TICKS) {
        return RefreshStep::Wait;
    }
    if attempts >= REFRESH_MAX_ATTEMPTS {
        return RefreshStep::Exhausted;
    }
    RefreshStep::Attempt(attempts + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_tick_attempts_immediately() {
        // Tick 0 is a multiple of the interval, so the first autoload tick after the picker
        // resolves does real work rather than waiting a half second for nothing.
        assert_eq!(refresh_step(0, 0), RefreshStep::Attempt(1));
    }

    #[test]
    fn only_every_interval_th_tick_does_work() {
        for tick in 1..REFRESH_ATTEMPT_INTERVAL_TICKS {
            assert_eq!(refresh_step(tick, 0), RefreshStep::Wait, "tick {tick}");
        }
        assert_eq!(
            refresh_step(REFRESH_ATTEMPT_INTERVAL_TICKS, 0),
            RefreshStep::Attempt(1)
        );
        assert_eq!(
            refresh_step(2 * REFRESH_ATTEMPT_INTERVAL_TICKS, 1),
            RefreshStep::Attempt(2)
        );
    }

    #[test]
    fn the_budget_is_spent_after_max_attempts_and_never_recovers() {
        assert_eq!(
            refresh_step(0, REFRESH_MAX_ATTEMPTS - 1),
            RefreshStep::Attempt(REFRESH_MAX_ATTEMPTS)
        );
        assert_eq!(
            refresh_step(0, REFRESH_MAX_ATTEMPTS),
            RefreshStep::Exhausted
        );
        assert_eq!(
            refresh_step(600 * REFRESH_ATTEMPT_INTERVAL_TICKS, REFRESH_MAX_ATTEMPTS),
            RefreshStep::Exhausted
        );
    }

    #[test]
    fn an_exhausted_budget_still_costs_nothing_off_the_boundary() {
        // Order matters: the throttle is checked first, so a spent budget on a non-boundary
        // tick reports Wait. Both outcomes do nothing; the test pins the branch order so a
        // later reshuffle cannot start reading the file on every frame.
        assert_eq!(refresh_step(1, REFRESH_MAX_ATTEMPTS), RefreshStep::Wait);
    }
}
