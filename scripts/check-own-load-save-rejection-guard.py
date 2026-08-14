#!/usr/bin/env python3
"""Fail closed if own-load can retry an identical unresolvable save verdict."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DRIVE = ROOT / "crates/er-effects-rs/src/experiments/own_load/drive.rs"
LOAD_DRIVE = ROOT / "crates/er-effects-rs/src/experiments/own_load/loaders/load_drive.rs"
SWITCH_RELOAD = ROOT / "crates/er-effects-rs/src/experiments/own_load/loaders/switch_reload.rs"
STATS = ROOT / "crates/er-effects-rs/src/experiments/startup_hooks/loading_cover/title_resources_stats_text.rs"
PATH_HOOKS = ROOT / "crates/er-effects-rs/src/experiments/save_redirect/path_hooks.rs"
SHARED = ROOT / "crates/er-save-redirect/src/lib.rs"
CHECK_SH = ROOT / "scripts/check.sh"


def rust_fn_body(source: str, name: str) -> str:
    marker = f"fn {name}("
    start = source.find(marker)
    if start < 0:
        raise AssertionError(f"missing function {name}")
    brace = source.find("{", start)
    depth = 0
    for index in range(brace, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[brace + 1 : index]
    raise AssertionError(f"unterminated function {name}")


def before(body: str, guard: str, action: str) -> bool:
    return 0 <= body.find(guard) < body.find(action)


def audit(texts: dict[str, str]) -> list[str]:
    failures: list[str] = []
    try:
        resolver = rust_fn_body(texts["drive"], "own_load_read_sl2_bytes")
        if not before(resolver, "own_load_save_rejection_terminal()", "switch_save_file_override()"):
            failures.append("resolver does not reject terminal re-entry before source/disk resolution")
        if "record_own_load_save_rejection(fingerprint)" not in resolver:
            failures.append("unresolvable active-mode save does not publish a terminal rejection")
        if "missing_save_selection_pending()" not in resolver:
            failures.append("pending picker can incorrectly consume the terminal rejection")
        if "direct_save_file_source_active()" not in resolver:
            failures.append("ordinary default-save absence can be mislabeled as a terminal staged-source rejection")

        probe = rust_fn_body(texts["load_drive"], "own_load_drive")
        if not before(probe, "own_load_save_rejection_terminal()", "own_load_read_sl2_bytes(base)"):
            failures.append("verify drive can re-enter the terminal resolver")

        switch = rust_fn_body(texts["switch"], "own_load_feed_deserialize")
        if not before(switch, "own_load_save_rejection_terminal()", "own_load_read_sl2_bytes(base)"):
            failures.append("switch feed can re-enter the terminal resolver")

        stats = rust_fn_body(texts["stats"], "ensure_profile_slot_stats_cached")
        if not before(stats, "PROFILE_SLOT_STATS_CACHE_STATE.load", "own_load_read_sl2_bytes(base)"):
            failures.append("stats cache does not consume its failed state before retrying")
        if not before(stats, "own_load_save_rejection_terminal()", "own_load_read_sl2_bytes(base)"):
            failures.append("stats cache can re-enter the terminal resolver")
    except AssertionError as error:
        failures.append(str(error))

    for field in (
        "oracle_own_load_save_rejection_state",
        "oracle_own_load_save_rejection_fingerprint",
        "oracle_own_load_save_rejection_attempts",
        "oracle_own_load_save_rejection_guard_checks",
        "oracle_own_load_save_rejection_probe_armed",
        "oracle_own_load_save_rejection_probe_fired",
        "oracle_own_load_save_rejection_probe_expected_fingerprint",
        "oracle_own_load_save_repeated_identical_rejections",
    ):
        if field not in texts["path_hooks"]:
            failures.append(f"missing structured telemetry field {field}")

    for test in (
        "terminal_rejection_simulation_resolves_once_across_recorded_runtime_churn",
        "repeated_identical_rejection_sets_a_nonzero_recurrence_semaphore",
    ):
        if test not in texts["shared"]:
            failures.append(f"missing host regression test {test}")

    if "check-own-load-save-rejection-guard.py" not in texts["check"]:
        failures.append("scripts/check.sh does not run this recurrence guard")
    if "-p er-save-redirect --lib" not in texts["check"]:
        failures.append("scripts/check.sh does not run the host rejection simulation")
    return failures


def fixture() -> dict[str, str]:
    return {
        "drive": """fn own_load_read_sl2_bytes() { if own_load_save_rejection_terminal() {} switch_save_file_override(); missing_save_selection_pending(); direct_save_file_source_active(); record_own_load_save_rejection(fingerprint); }""",
        "load_drive": """fn own_load_drive() { own_load_save_rejection_terminal(); own_load_read_sl2_bytes(base); }""",
        "switch": """fn own_load_feed_deserialize() { own_load_save_rejection_terminal(); own_load_read_sl2_bytes(base); }""",
        "stats": """fn ensure_profile_slot_stats_cached() { PROFILE_SLOT_STATS_CACHE_STATE.load(x); own_load_save_rejection_terminal(); own_load_read_sl2_bytes(base); }""",
        "path_hooks": " ".join((
            "oracle_own_load_save_rejection_state",
            "oracle_own_load_save_rejection_fingerprint",
            "oracle_own_load_save_rejection_attempts",
            "oracle_own_load_save_rejection_guard_checks",
            "oracle_own_load_save_rejection_probe_armed",
            "oracle_own_load_save_rejection_probe_fired",
            "oracle_own_load_save_rejection_probe_expected_fingerprint",
            "oracle_own_load_save_repeated_identical_rejections",
        )),
        "shared": "terminal_rejection_simulation_resolves_once_across_recorded_runtime_churn repeated_identical_rejection_sets_a_nonzero_recurrence_semaphore",
        "check": "check-own-load-save-rejection-guard.py cargo test -p er-save-redirect --lib",
    }


def selftest() -> int:
    base = fixture()
    if audit(base):
        print("selftest fixture unexpectedly failed", file=sys.stderr)
        return 1
    cases = {
        "missing drive guard": ("load_drive", "own_load_save_rejection_terminal()"),
        "missing staged-source scope": ("drive", "direct_save_file_source_active()"),
        "missing recurrence telemetry": ("path_hooks", "oracle_own_load_save_repeated_identical_rejections"),
        "missing host simulation": ("shared", "terminal_rejection_simulation_resolves_once_across_recorded_runtime_churn"),
        "missing check wiring": ("check", "check-own-load-save-rejection-guard.py"),
    }
    for name, (key, token) in cases.items():
        broken = dict(base)
        broken[key] = broken[key].replace(token, "", 1)
        if not audit(broken):
            print(f"selftest failed to catch {name}", file=sys.stderr)
            return 1
    print("[own-load-save-rejection-guard] selftest ok (5 mutations rejected)")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    texts = {
        "drive": DRIVE.read_text(encoding="utf-8"),
        "load_drive": LOAD_DRIVE.read_text(encoding="utf-8"),
        "switch": SWITCH_RELOAD.read_text(encoding="utf-8"),
        "stats": STATS.read_text(encoding="utf-8"),
        "path_hooks": PATH_HOOKS.read_text(encoding="utf-8"),
        "shared": SHARED.read_text(encoding="utf-8"),
        "check": CHECK_SH.read_text(encoding="utf-8"),
    }
    failures = audit(texts)
    for failure in failures:
        print(f"own-load save-rejection guard FAILED: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("[own-load-save-rejection-guard] ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
