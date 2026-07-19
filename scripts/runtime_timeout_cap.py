#!/usr/bin/env python3
"""Single source of truth for the runtime-probe wall-clock cap (Python consumers).

The cap VALUE lives in `.auto/runtime_timeout_cap_seconds`; the runtime path
(run-product-continue-direct-probe.sh -> .auto/runtime_probe.sh -> er-readiness-watch.py)
reads that file and passes the value through as `--max-runtime-seconds`. Python consumers
(the readiness watcher and the contract checker) share this one reader so the fallback and
the absolute sanity ceiling are defined exactly once.
"""
from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
RUNTIME_TIMEOUT_CAP_PATH = REPO_ROOT / ".auto" / "runtime_timeout_cap_seconds"

# Three minutes is the hard backstop for the GAME runtime probe -- NOT a wall-clock target. It is
# the idle/stall ceiling: the primary teardown is semaphore-driven (a run tears down a small delay
# after the last in-memory oracle the specific test cares about, so most runs finish far under this),
# and this value only bounds a run that makes no semaphore progress (a hang). It governs the GAME
# path only; non-game/agent-shell ops stay hard-capped at 30s by scripts/check-no-timeouts.py
# (MAX_TIMEOUT_SECONDS), a separate, tighter limit -- an unbounded Ghidra query still fails fast.
# The fallback (canonical file missing/unreadable) and the absolute ceiling (clamp against a
# corrupted/tampered file) are pinned to the same value so no other number can leak in. To change
# it, change .auto/runtime_timeout_cap_seconds, these two, AND the rego literal, then re-run
# scripts/check-runtime-probe-contract.py. See bd runtime-teardown-semaphore-progress-watchdog-2026-07-17.
RUNTIME_TIMEOUT_CAP_FALLBACK_SECONDS = 180
RUNTIME_TIMEOUT_CAP_CEILING_SECONDS = 180


def runtime_timeout_cap_seconds() -> int:
    """Return the canonical runtime-probe cap in whole seconds, clamped fail-safe."""
    try:
        value = int(RUNTIME_TIMEOUT_CAP_PATH.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return RUNTIME_TIMEOUT_CAP_FALLBACK_SECONDS
    if 0 < value <= RUNTIME_TIMEOUT_CAP_CEILING_SECONDS:
        return value
    return RUNTIME_TIMEOUT_CAP_FALLBACK_SECONDS


if __name__ == "__main__":
    print(runtime_timeout_cap_seconds())
