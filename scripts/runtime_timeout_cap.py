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

# One minute is the hard truth for the runtime-probe wall-clock cap. The fallback (used if the
# canonical file is missing/unreadable) and the absolute ceiling (a clamp against a corrupted/tampered
# file) are pinned to that same value so no other number can leak in. To change the cap, change
# .auto/runtime_timeout_cap_seconds AND these.
RUNTIME_TIMEOUT_CAP_FALLBACK_SECONDS = 60
RUNTIME_TIMEOUT_CAP_CEILING_SECONDS = 60


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
