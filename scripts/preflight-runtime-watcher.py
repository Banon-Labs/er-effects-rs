#!/usr/bin/env python3
"""Offline preflight for the runtime-probe harness, run BEFORE spending an Elden Ring launch.

Why this exists: a watcher edit that did `telemetry.get(...)` on a None telemetry crashed the
readiness watcher at runtime and burned a full gated ER launch (the game booted and closed in
seconds) to discover a pure-Python harness bug. A runtime launch is expensive (boot time, the
user watching, save-safety stakes) and must never be spent to surface an offline-checkable defect.

This validator:
  1. py_compiles every probe Python script and `bash -n`-checks the probe shell scripts.
  2. Imports er-readiness-watch.py and exercises EVERY `telemetry_*` decision helper against the
     telemetry states a real run actually produces -- None (file not written yet), {} (empty),
     and representative oracle payloads -- asserting none raise (the exact failure mode that
     escaped before, because the inline check skipped the helper pattern's None guard).

Exit non-zero on any failure so the launcher can fail closed before launching the game.
"""
from __future__ import annotations

import importlib.util
import inspect
import py_compile
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PY_SCRIPTS = [
    REPO / "scripts/er-readiness-watch.py",
    REPO / "scripts/run-product-continue-direct-probe.sh",  # shell, checked below
]
SHELL_SCRIPTS = [
    REPO / "scripts/run-product-continue-direct-probe.sh",
    REPO / ".auto/runtime_probe.sh",
]
WATCHER = REPO / "scripts/er-readiness-watch.py"

# Telemetry states a real run moves through; every decision helper must tolerate all of them.
TELEMETRY_FIXTURES = [
    None,                                           # file not written yet (the bug that crashed it)
    {},                                             # present but empty
    {"oracle_policy_window_total_builds": 0},       # benign early title
    {"oracle_policy_window_total_builds": 1, "oracle_policy_window_any_seen": True},
    {"oracle_msgbox_total_builds": 1, "oracle_msgbox_any_seen": True},
    {"oracle_cold_char_mount_phase": 0},
    {"oracle_cold_char_mount_phase": 5},
    {"oracle_server_status_text_id": 401120},
    {"oracle_msgbox_builder_args": [0, 0, 0, 0]},
    {"oracle_msgbox_builder_args": "not-a-list"},   # malformed
]


def fail(msg: str) -> None:
    print(f"preflight-runtime-watcher: FAIL: {msg}", file=sys.stderr)
    raise SystemExit(1)


def compile_checks() -> None:
    try:
        py_compile.compile(str(WATCHER), doraise=True)
    except py_compile.PyCompileError as exc:
        fail(f"py_compile {WATCHER.name}: {exc}")
    for sh in SHELL_SCRIPTS:
        if not sh.exists():
            fail(f"missing shell script {sh}")
        res = subprocess.run(
            ["bash", "-n", str(sh)], capture_output=True, text=True, timeout=30
        )
        if res.returncode != 0:
            fail(f"bash -n {sh.name}: {res.stderr.strip()}")
    print("preflight: py_compile + bash -n OK")


def load_watcher_module():
    spec = importlib.util.spec_from_file_location("er_readiness_watch", WATCHER)
    module = importlib.util.module_from_spec(spec)
    # Register before exec so @dataclass introspection can resolve the module (Python 3.14).
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)  # safe: argparse/main is guarded by __name__ == "__main__"
    return module


def helper_checks() -> None:
    module = load_watcher_module()
    # The None-critical helpers are the boolean early-exit predicates evaluated each poll BEFORE
    # telemetry is confirmed non-None (names end in `_detected` / `_complete`). Other telemetry_*
    # functions (e.g. *_chain_stage report builders) run after a None guard, so are out of scope.
    helpers = [
        name
        for name, obj in inspect.getmembers(module, inspect.isfunction)
        if name.startswith("telemetry_")
        and (name.endswith("_detected") or name.endswith("_complete"))
        and len(inspect.signature(obj).parameters) == 1
    ]
    if not helpers:
        fail("no telemetry_*_detected/_complete early-exit predicates found to validate")
    for name in helpers:
        fn = getattr(module, name)
        for fixture in TELEMETRY_FIXTURES:
            try:
                result = fn(fixture)
            except Exception as exc:  # the class of bug that burned a launch
                fail(f"{name}({fixture!r}) raised {type(exc).__name__}: {exc}")
            if not isinstance(result, bool):
                fail(f"{name}({fixture!r}) returned non-bool {result!r}")
    # The specific regression: terminal cold-mount phase must trigger, None/early must not.
    cc = getattr(module, "telemetry_cold_char_mount_complete", None)
    if cc is None:
        fail("telemetry_cold_char_mount_complete helper missing")
    if cc(None) or cc({}) or cc({"oracle_cold_char_mount_phase": 0}):
        fail("cold_char_mount_complete fired on a non-terminal/None state")
    if not cc({"oracle_cold_char_mount_phase": 5}):
        fail("cold_char_mount_complete did not fire on PHASE_DONE")
    print(f"preflight: validated {len(helpers)} telemetry helpers against {len(TELEMETRY_FIXTURES)} states OK")

    # World-stream stall semaphore: the watermark/armed helpers parse hex-string OWN-LOAD oracle
    # fields each poll and MUST tolerate every real telemetry state (None/empty/malformed) without
    # raising -- same class of bug that burned a launch before. The watermark must be None until the
    # map-load arms (continue fired + player block present) so a slow boot/title never trips it.
    armed = getattr(module, "world_stream_armed", None)
    watermark = getattr(module, "world_stream_progress_watermark", None)
    step = getattr(module, "world_stream_stall_step", None)
    if armed is None or watermark is None or step is None:
        fail("world-stream stall semaphore helpers missing")
    for fixture in TELEMETRY_FIXTURES:
        try:
            armed_result = armed(fixture)
            wm = watermark(fixture)
            step(fixture, None, None, 0.0, 20.0)
        except Exception as exc:
            fail(f"world_stream_* helper raised on {fixture!r}: {type(exc).__name__}: {exc}")
        if not isinstance(armed_result, bool):
            fail(f"world_stream_armed({fixture!r}) returned non-bool {armed_result!r}")
        if wm is not None:
            fail(f"world_stream_progress_watermark({fixture!r}) armed on a non-OWN-LOAD state")
    stalled_state = {
        "oracle_own_load_continue_fired": True,
        "oracle_own_load_target_block_present": 1,
        "oracle_own_load_wbr_max_phase": "0x2",
        "oracle_own_load_stream_io_inflight": "0x0",
        "oracle_own_load_stream_mms_state": 3,
        "oracle_own_m28_dispatch_fired": 0,
        "oracle_player_present": False,
    }
    if not armed(stalled_state) or watermark(stalled_state) is None:
        fail("world-stream watermark did not arm on a begun OWN-LOAD map-load")
    # Pre-continue must never arm even with everything else flat.
    if armed({**stalled_state, "oracle_own_load_continue_fired": False}):
        fail("world-stream stall armed before continue fired")
    if armed({**stalled_state, "oracle_own_load_target_block_present": 0}):
        fail("world-stream stall armed before the player block registered")
    # Flat past the window stalls; forward progress within it does not.
    _, _, must_stall = step(stalled_state, watermark(stalled_state), 0.0, 25.0, 20.0)
    if not must_stall:
        fail("world-stream stall did not fire on a flat watermark past the window")
    progressed = {**stalled_state, "oracle_own_load_stream_io_inflight": "0x4"}
    _, _, must_not_stall = step(progressed, watermark(stalled_state), 0.0, 25.0, 20.0)
    if must_not_stall:
        fail("world-stream stall fired despite forward streaming progress")
    print("preflight: validated world-stream stall semaphore (arm gate + watermark + stall window) OK")

    # PER-PHASE PROGRESS WATCHDOG: the phase predicates + progress watermarks + step parse telemetry
    # each poll and MUST tolerate every real telemetry state (None/empty/malformed) without raising --
    # same class of bug that burned a launch before. No watched phase may arm before telemetry, and
    # once continue has fired the watchdog must hand off to world_stream (no watched phase active).
    phase_step = getattr(module, "phase_progress_stall_step", None)
    active_phase = getattr(module, "active_watchdog_phase", None)
    phase_title_active = getattr(module, "phase_title_active", None)
    phase_continue_active = getattr(module, "phase_continue_active", None)
    if phase_step is None or active_phase is None or phase_title_active is None or phase_continue_active is None:
        fail("per-phase progress watchdog helpers missing")
    for fixture in TELEMETRY_FIXTURES:
        try:
            st = {"phase": None, "value": None, "since": None}
            stalled, name = phase_step(fixture, st, 0.0, 3.0)
            active_phase(fixture)
            ta = phase_title_active(fixture)
            ca = phase_continue_active(fixture)
        except Exception as exc:
            fail(f"phase watchdog helper raised on {fixture!r}: {type(exc).__name__}: {exc}")
        if not isinstance(stalled, bool):
            fail(f"phase_progress_stall_step({fixture!r}) returned non-bool stalled {stalled!r}")
        if not isinstance(ta, bool) or not isinstance(ca, bool):
            fail(f"phase active predicate returned non-bool on {fixture!r}")
    # Title phase: telemetry present, continue not fired, owner not yet captured -> active + arms a
    # watermark, and stays flat past the window -> title_stalled.
    title_state = {
        "game_task_ticks": 100,
        "title_owner_scan_attempts": 5,
        "title_owner_scan_last_state": 4,
        "title_handoff_complete": False,
        "oracle_own_load_continue_fired": False,
    }
    if not phase_title_active(title_state) or active_phase(title_state) is None:
        fail("title phase did not arm on a pre-continue title telemetry state")
    st = {"phase": None, "value": None, "since": None}
    phase_step(title_state, st, 0.0, 3.0)          # arm
    flat_stalled, flat_reason = phase_step(title_state, st, 5.0, 3.0)  # flat past window
    if not flat_stalled or flat_reason != "title_stalled":
        fail("phase watchdog did not fire title_stalled on a flat title watermark past the window")
    # Forward progress within the window resets the timer (no false-fail on a slow-but-moving phase).
    st2 = {"phase": None, "value": None, "since": None}
    phase_step(title_state, st2, 0.0, 3.0)
    moved = {**title_state, "game_task_ticks": 200}
    prog_stalled, _ = phase_step(moved, st2, 5.0, 3.0)
    if prog_stalled:
        fail("phase watchdog fired despite forward title progress")
    # Once continue has fired, no watched phase is active (world_stream owns the tail).
    if active_phase({**title_state, "oracle_own_load_continue_fired": True}) is not None:
        fail("phase watchdog stayed armed after continue fired (should hand off to world_stream)")
    print("preflight: validated per-phase progress watchdog (title/continue arm + watermark + stall window) OK")


def main() -> int:
    compile_checks()
    helper_checks()
    print("preflight-runtime-watcher: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
