#!/usr/bin/env python3
"""Regression tests for scripts/check-marker-file-gates.py."""

from __future__ import annotations

import importlib.util
import json
import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CHECK_PATH = REPO_ROOT / "scripts" / "check-marker-file-gates.py"
FIXTURE_ROOT = REPO_ROOT / "target" / "marker-file-gate-fixtures"


def load_checker():
    spec = importlib.util.spec_from_file_location("check_marker_file_gates", CHECK_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {CHECK_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def configure_module_paths(checker) -> None:
    checker.REPO_ROOT = FIXTURE_ROOT
    checker.SRC_DIR = FIXTURE_ROOT / "src"
    checker.AUTO_DIR = FIXTURE_ROOT / ".auto"
    checker.BASELINE_PATH = checker.AUTO_DIR / "marker_file_gate_baseline.json"
    checker.POLICY_PATH = checker.AUTO_DIR / "marker_file_gate_policy.rego"


def write(relative: str, body: str) -> Path:
    path = FIXTURE_ROOT / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")
    return path


def write_baseline(names: list[str], migrate: list[str] | None = None) -> None:
    payload = {
        "sanctioned_marker_gate_names": sorted(names),
        "migrate_to_default": sorted(migrate or []),
    }
    write(
        ".auto/marker_file_gate_baseline.json",
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
    )


def valid_policy() -> str:
    # Mirror the real policy's required snippets so policy_findings() stays clean.
    return (
        "package auto.marker_file_gate\n"
        "import rego.v1\n"
        "default allow := false\n"
        "allow if { input.marker_name_sanctioned }\n"
        'deny contains message if { not input.marker_name_sanctioned; message := "new marker .exists() gate" }\n'
    )


def rules_for(checker) -> set[str]:
    gates = checker.scan_marker_gates()
    sanctioned = checker.load_sanctioned_names()
    return {f.rule for f in checker.scan_findings(gates, sanctioned)}


def main() -> int:
    if FIXTURE_ROOT.exists():
        shutil.rmtree(FIXTURE_ROOT)
    checker = load_checker()
    configure_module_paths(checker)

    write(".auto/marker_file_gate_policy.rego", valid_policy())

    # 1. POSITIVE: a NEW behavioral-fix marker gate (bool `_enabled()` fn, marker name
    #    not allowlisted) -> flagged. This is the exact incident shape.
    write(
        "src/return_title.rs",
        "fn reload_b73_hold_enabled() -> bool {\n"
        '    game_dir().join("er-effects-reload-b73hold.txt").exists()\n'
        "}\n"
        "fn apply_fix(p: *mut u8) {\n"
        "    if reload_b73_hold_enabled() && stuck() {\n"
        "        unsafe { *p = 1; }\n"
        "    }\n"
        "}\n",
    )
    write_baseline([])
    rules = rules_for(checker)
    assert rules == {"marker-gate-new-name"}, rules

    # 1b. POSITIVE: an INLINE behavioral gate is detected AND classified behavioral.
    write(
        "src/inline.rs",
        "fn tick(p: *mut u8) {\n"
        '    if game_dir().join("er-effects-inline-fix.txt").exists() {\n'
        "        unsafe { core::ptr::write_volatile(p, 7u8); }\n"
        "    }\n"
        "}\n",
    )
    write_baseline([])
    gates = checker.scan_marker_gates()
    inline = [g for g in gates if g.name == "er-effects-inline-fix.txt"][0]
    assert inline.classification == "behavioral", inline.classification
    # ...and the b73hold bool-fn gate classifies unknown (its fn body has no behavioral
    # tokens) yet is STILL flagged by the hard name gate -- the point of name-freezing.
    b73 = [g for g in gates if g.name == "er-effects-reload-b73hold.txt"][0]
    assert b73.classification == "unknown", b73.classification
    assert rules_for(checker) == {"marker-gate-new-name"}
    (FIXTURE_ROOT / "src" / "inline.rs").unlink()

    # 2. NEGATIVE: the SAME gate becomes allowlisted -> passes (the build-preserving
    #    allowlist for pre-existing markers).
    write_baseline(["er-effects-reload-b73hold.txt"])
    rules = rules_for(checker)
    assert rules == set(), rules

    # 3. A DIAGNOSTIC-logging marker gate. An INLINE gate whose fn body only logs is
    #    classified `diagnostic` (steering the reviewer that it MAY be allowlisted). It
    #    still fails closed on a new name until allowlisted -- new names always fail
    #    closed, diagnostic or not. (A trivial bool `_enabled()` fn instead classifies
    #    `unknown`, since its body carries no tokens -- the documented limitation; the
    #    hard name gate still catches it either way.)
    write(
        "src/diag.rs",
        "fn log_ids() {\n"
        '    if game_dir().join("er-effects-grsysmsg-log-x.txt").exists() {\n'
        '        append_autoload_debug("gr sys msg id seen");\n'
        "    }\n"
        "}\n",
    )
    write_baseline(["er-effects-reload-b73hold.txt"])
    # New diagnostic name is still flagged (fails closed), but classification == diagnostic.
    rules = rules_for(checker)
    assert rules == {"marker-gate-new-name"}, rules
    gates = checker.scan_marker_gates()
    diag = [g for g in gates if g.name == "er-effects-grsysmsg-log-x.txt"][0]
    assert diag.classification == "diagnostic", diag.classification
    # Allowlisting the diagnostic name clears it.
    write_baseline(
        ["er-effects-reload-b73hold.txt", "er-effects-grsysmsg-log-x.txt"]
    )
    assert rules_for(checker) == set()
    (FIXTURE_ROOT / "src" / "diag.rs").unlink()

    # 4. NEGATIVE: a real-runtime-condition fix with NO marker file is NOT flagged.
    #    (This is the desired product shape -- default behavior on a genuine condition.)
    write(
        "src/default_fix.rs",
        "fn tick(p: *mut u8) {\n"
        "    if FRESH_DESER_DONE.load(Ordering::SeqCst) == 1 && mms18_stuck() {\n"
        "        unsafe { core::ptr::write_volatile(p, 9u8); }\n"
        "    }\n"
        "}\n",
    )
    write_baseline(["er-effects-reload-b73hold.txt"])
    rules = rules_for(checker)
    assert rules == set(), rules
    (FIXTURE_ROOT / "src" / "default_fix.rs").unlink()

    # 4b. NEGATIVE: a DATA control file read with read_to_string (a slot number, not a
    #     boolean toggle) is OUT OF SCOPE -- no `.exists()`, so not flagged.
    write(
        "src/data_file.rs",
        "fn wanted_slot() -> Option<u32> {\n"
        '    let raw = std::fs::read_to_string(game_dir().join("er-effects-switch-slot.txt")).ok()?;\n'
        "    raw.trim().parse().ok()\n"
        "}\n",
    )
    write_baseline(["er-effects-reload-b73hold.txt"])
    rules = rules_for(checker)
    assert rules == set(), rules
    (FIXTURE_ROOT / "src" / "data_file.rs").unlink()

    # 5. MIGRATION NOTE: a sanctioned-but-behavioral marker still present is a SOFT note,
    #    not a hard finding.
    write_baseline(
        ["er-effects-reload-b73hold.txt"], migrate=["er-effects-reload-b73hold.txt"]
    )
    gates = checker.scan_marker_gates()
    migrate = checker.load_migrate_to_default()
    assert rules_for(checker) == set()
    assert checker.migration_notes(gates, migrate) == ["er-effects-reload-b73hold.txt"]

    # 6. POLICY DRIFT: missing / drifted policy file is itself a finding.
    write_baseline(["er-effects-reload-b73hold.txt"])
    (FIXTURE_ROOT / ".auto" / "marker_file_gate_policy.rego").unlink()
    rules = rules_for(checker)
    assert rules == {"missing-marker-gate-policy"}, rules

    write(".auto/marker_file_gate_policy.rego", "package auto.marker_file_gate\n")
    rules = rules_for(checker)
    assert rules == {"marker-gate-policy-drift"}, rules

    # 7. EMPTY / MISSING allowlist fails ALL gates closed (allowlist must be explicit).
    write(".auto/marker_file_gate_policy.rego", valid_policy())
    (FIXTURE_ROOT / ".auto" / "marker_file_gate_baseline.json").unlink()
    rules = rules_for(checker)
    assert rules == {"marker-gate-new-name"}, rules

    print("marker-file gate policy regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
