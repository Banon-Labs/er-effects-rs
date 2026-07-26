#!/usr/bin/env python3
"""Regression tests for scripts/check-marker-file-gates.py (deprecated-allowlist policy)."""

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


def write_baseline(
    diagnostic_gates: dict[str, str] | None = None,
    deprecated: dict[str, list[str]] | None = None,
) -> None:
    payload: dict[str, object] = {
        "sanctioned_marker_gate_names": [],
        "migrate_to_default": [],
        "diagnostic_gates": diagnostic_gates or {},
    }
    if deprecated:
        payload.update(deprecated)
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
        "# references diagnostic_gates; .exists() toggle shape\n"
        "allow if { input.marker_diagnostic_sanctioned; input.marker_rationale_present }\n"
        'deny contains message if { not input.marker_diagnostic_sanctioned; message := "forbidden" }\n'
    )


def rules_for(checker) -> set[str]:
    gates = checker.scan_marker_gates()
    data = checker.load_baseline_data()
    diagnostic_gates = checker.load_diagnostic_gates(data)
    findings = (
        checker.policy_findings()
        + checker.deprecation_findings(data)
        + checker.scan_findings(gates, diagnostic_gates)
    )
    return {f.rule for f in findings}


def main() -> int:
    if FIXTURE_ROOT.exists():
        shutil.rmtree(FIXTURE_ROOT)
    checker = load_checker()
    configure_module_paths(checker)

    write(".auto/marker_file_gate_policy.rego", valid_policy())

    # 1. A behavioral-fix marker gate (the incident shape) is FORBIDDEN.
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
    write_baseline({})
    assert rules_for(checker) == {"marker-gate-forbidden"}, rules_for(checker)

    # 2. A DIAGNOSTIC-logging marker gate becomes allowed once listed in diagnostic_gates
    #    (with a rationale) AND its fn is not behavioral.
    write(
        "src/diag.rs",
        "fn log_ids() {\n"
        '    if game_dir().join("er-effects-grsysmsg-log-x.txt").exists() {\n'
        '        append_autoload_debug("gr sys msg id seen");\n'
        "    }\n"
        "}\n",
    )
    (FIXTURE_ROOT / "src" / "return_title.rs").unlink()
    write_baseline({})
    assert rules_for(checker) == {"marker-gate-forbidden"}, rules_for(checker)
    write_baseline({"er-effects-grsysmsg-log-x.txt": "passive GR_System_Message id log; no game behavior."})
    assert rules_for(checker) == set(), rules_for(checker)

    # 2b. An EMPTY rationale is not enough -> still forbidden.
    write_baseline({"er-effects-grsysmsg-log-x.txt": "  "})
    assert rules_for(checker) == {"marker-gate-forbidden"}, rules_for(checker)

    # 3. A BEHAVIORAL fn cannot sneak into diagnostic_gates: even if listed, it is rejected.
    write(
        "src/inline.rs",
        "fn tick(p: *mut u8) {\n"
        '    if game_dir().join("er-effects-inline-fix.txt").exists() {\n'
        "        unsafe { core::ptr::write_volatile(p, 7u8); }\n"
        "    }\n"
        "}\n",
    )
    (FIXTURE_ROOT / "src" / "diag.rs").unlink()
    write_baseline({"er-effects-inline-fix.txt": "claims to be diagnostic but writes memory."})
    assert rules_for(checker) == {"marker-gate-diagnostic-is-behavioral"}, rules_for(checker)
    (FIXTURE_ROOT / "src" / "inline.rs").unlink()

    # 4. A real-runtime-condition fix with NO marker file is NOT flagged (desired shape).
    write(
        "src/default_fix.rs",
        "fn tick(p: *mut u8) {\n"
        "    if FRESH_DESER_DONE.load(Ordering::SeqCst) == 1 && mms18_stuck() {\n"
        "        unsafe { core::ptr::write_volatile(p, 9u8); }\n"
        "    }\n"
        "}\n",
    )
    write_baseline({})
    assert rules_for(checker) == set(), rules_for(checker)
    (FIXTURE_ROOT / "src" / "default_fix.rs").unlink()

    # 4b. A DATA control file read with read_to_string (not `.exists()`) is OUT OF SCOPE.
    write(
        "src/data_file.rs",
        "fn wanted_slot() -> Option<u32> {\n"
        '    let raw = std::fs::read_to_string(game_dir().join("er-effects-switch-slot.txt")).ok()?;\n'
        "    raw.trim().parse().ok()\n"
        "}\n",
    )
    write_baseline({})
    assert rules_for(checker) == set(), rules_for(checker)
    (FIXTURE_ROOT / "src" / "data_file.rs").unlink()

    # 5. Re-populating a DEPRECATED behavioral allowlist is itself a failure.
    write(
        "src/diag2.rs",
        "fn log_ids() {\n"
        '    if game_dir().join("er-effects-grsysmsg-log-x.txt").exists() {\n'
        '        append_autoload_debug("id");\n'
        "    }\n"
        "}\n",
    )
    write_baseline(
        {"er-effects-grsysmsg-log-x.txt": "passive log."},
        deprecated={"sanctioned_marker_gate_names": ["er-effects-grsysmsg-log-x.txt"]},
    )
    assert rules_for(checker) == {"marker-gate-allowlist-not-deprecated"}, rules_for(checker)
    write_baseline(
        {"er-effects-grsysmsg-log-x.txt": "passive log."},
        deprecated={"migrate_to_default": ["er-effects-grsysmsg-log-x.txt"]},
    )
    assert rules_for(checker) == {"marker-gate-allowlist-not-deprecated"}, rules_for(checker)
    (FIXTURE_ROOT / "src" / "diag2.rs").unlink()

    # 6. POLICY DRIFT: missing / drifted policy file is itself a finding.
    write_baseline({})
    (FIXTURE_ROOT / ".auto" / "marker_file_gate_policy.rego").unlink()
    assert rules_for(checker) == {"missing-marker-gate-policy"}, rules_for(checker)

    write(".auto/marker_file_gate_policy.rego", "package auto.marker_file_gate\n")
    assert rules_for(checker) == {"marker-gate-policy-drift"}, rules_for(checker)

    print("marker-file gate policy regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
