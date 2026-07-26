#!/usr/bin/env python3
"""Progress-idle teardown watchdog for game runtime probes (pure logic, no I/O).

The runtime model (user directive 2026-07-17, bd runtime-teardown-semaphore-progress-watchdog-2026-07-17)
is semaphore-progress, not wall-clock: a game run should tear down a small delay AFTER the last
in-memory RAM oracle the specific test cares about, and should be killed early if it makes NO
progress for an idle window -- with the canonical 180s cap only as a never-should-hit backstop.

This class is the enforcement. A probe feeds it the parsed telemetry dict every sample; it returns
a Decision. It is deliberately I/O-free so it unit-tests without a game and drops into any probe
loop (Windows y22i, Linux readiness watcher, future goal-validation harnesses).

THE ONE RULE THAT MAKES IT CORRECT: the idle timer is reset ONLY by a monotonic PROGRESS oracle
advancing (oracle_loading_bar_progress_permille = real Gauge_3 world-load progress, phase counters,
mount-phase). It must NEVER be gated on a liveness counter like oracle_present_hook_hits or a frame
tick -- those advance every rendered frame even while the world is wedged at WORLD RES WAIT, which
would mask the exact stall we are trying to catch. "Progress" here = a watched key's numeric value
INCREASED since the previous sample (so permille resetting 1000->0 between the first and second load
is correctly treated as a non-event, not a stall and not progress).
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable

# Default progress oracles: each is a monotonic-within-a-phase advance driven by real game-state
# reads. Explicitly excludes liveness counters (oracle_present_hook_hits, *_frames tick counters).
DEFAULT_PROGRESS_KEYS = (
    "oracle_loading_bar_progress_permille",  # PRIMARY: real Gauge_3 world-load progress (0..1000)
    "oracle_own_load_phase",                 # own-load phase step machine
    "oracle_cold_char_mount_phase",          # cold char mount phase
    "oracle_continue_phase",                 # continue phase step machine
)

# Decision strings the probe acts on.
CONTINUE = "continue"
TEARDOWN_TERMINAL = "teardown:terminal"  # reached the semaphore this test cares about (+delay)
TEARDOWN_STALL = "teardown:stall"        # no progress oracle advanced for the idle window
TEARDOWN_CAP = "teardown:cap"            # hit the hard backstop (should be rare -- a wedge)


def coerce_number(value) -> float | None:
    """Best-effort numeric read of an oracle value (int / bool / decimal or hex string)."""
    if isinstance(value, bool):
        return 1.0 if value else 0.0
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        s = value.strip()
        try:
            return float(int(s, 16)) if s.lower().startswith("0x") else float(s)
        except ValueError:
            return None
    return None


@dataclass
class ProgressWatchdog:
    """Decide when a game runtime probe should tear down.

    idle_window_seconds : max time (once armed) with NO progress-oracle advance before a stall
                          teardown. The forcing-function target is ~1s; start looser and tighten
                          per phase as sub-second progress instrumentation is proven.
    teardown_delay_seconds : after the terminal semaphore fires, keep running this long so final
                          evidence flushes, then tear down.
    hard_cap_seconds    : absolute backstop (the canonical runtime cap). Only hit by a true wedge
                          that also never advances a progress oracle for the idle window -- normally
                          the stall path fires first.
    terminal_predicate  : telemetry -> bool; the objective is answered (e.g. world readiness).
    arm_predicate       : telemetry -> bool; until this is True the idle timer does NOT run (so a
                          legitimately coarse pre-load boot phase cannot false-stall). None == armed
                          immediately.
    progress_keys       : oracle keys whose INCREASE counts as progress.
    """

    idle_window_seconds: float = 10.0
    teardown_delay_seconds: float = 3.0
    hard_cap_seconds: float = 180.0
    terminal_predicate: Callable[[dict], bool] | None = None
    arm_predicate: Callable[[dict], bool] | None = None
    progress_keys: tuple[str, ...] = DEFAULT_PROGRESS_KEYS

    _t0: float | None = field(default=None, init=False)
    _armed: bool = field(default=False, init=False)
    _last_progress_t: float = field(default=0.0, init=False)
    _prev: dict = field(default_factory=dict, init=False)
    _terminal_t: float | None = field(default=None, init=False)
    last_reason: str = field(default="", init=False)

    def observe(self, telemetry: dict, now: float) -> str:
        """Feed one telemetry sample at monotonic time `now`; return a Decision string."""
        if self._t0 is None:
            self._t0 = now
            self._last_progress_t = now

        # Hard backstop first: a wedge that somehow evades the stall path still dies here.
        if now - self._t0 >= self.hard_cap_seconds:
            self.last_reason = f"hard cap {self.hard_cap_seconds}s reached with no terminal semaphore"
            return TEARDOWN_CAP

        # Progress detection: any watched key strictly greater than its previous sample.
        advanced = False
        for key in self.progress_keys:
            cur = coerce_number(telemetry.get(key))
            if cur is None:
                continue
            prev = self._prev.get(key)
            if prev is not None and cur > prev:
                advanced = True
            self._prev[key] = cur
        if advanced:
            self._last_progress_t = now

        # Arm the idle timer once we are in the phase we expect sub-second progress from. Arming
        # resets the idle clock so the transition itself is not counted as a stall.
        if not self._armed and (self.arm_predicate is None or self.arm_predicate(telemetry)):
            self._armed = True
            self._last_progress_t = now

        # Terminal semaphore: record first time seen, then run out the flush delay before teardown.
        if self._terminal_t is None and self.terminal_predicate and self.terminal_predicate(telemetry):
            self._terminal_t = now
        if self._terminal_t is not None:
            if now - self._terminal_t >= self.teardown_delay_seconds:
                self.last_reason = (
                    f"terminal semaphore reached; tore down {self.teardown_delay_seconds}s later"
                )
                return TEARDOWN_TERMINAL
            return CONTINUE

        # Stall: armed and no progress oracle advanced for the idle window.
        if self._armed and (now - self._last_progress_t) >= self.idle_window_seconds:
            self.last_reason = (
                f"no progress oracle advanced for {self.idle_window_seconds}s "
                f"(keys={list(self.progress_keys)})"
            )
            return TEARDOWN_STALL

        return CONTINUE
