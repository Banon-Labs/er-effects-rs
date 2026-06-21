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


def main() -> int:
    compile_checks()
    helper_checks()
    print("preflight-runtime-watcher: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
