#!/usr/bin/env python3
"""Regression tests for scripts/check-env-gate-comments.py (deprecated-allowlist policy)."""

from __future__ import annotations

import importlib.util
import json
import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CHECK_PATH = REPO_ROOT / "scripts" / "check-env-gate-comments.py"
FIXTURE_ROOT = REPO_ROOT / "target" / "env-gate-comment-fixtures"


def load_checker():
    spec = importlib.util.spec_from_file_location("check_env_gate_comments", CHECK_PATH)
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
    checker.BASELINE_PATH = checker.AUTO_DIR / "env_gate_comment_baseline.json"
    checker.POLICY_PATH = checker.AUTO_DIR / "env_gate_comment_policy.rego"


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
        "sanctioned_env_vars": [],
        "sanctioned_env_gate_locations": [],
        "baseline": [],
        "diagnostic_gates": diagnostic_gates or {},
    }
    if deprecated:
        payload.update(deprecated)
    write(
        ".auto/env_gate_comment_baseline.json",
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
    )


def valid_policy() -> str:
    # Mirror the real policy's required snippets so policy_findings() stays clean.
    return (
        "package auto.env_gate_comment\n"
        "import rego.v1\n"
        "default allow := false\n"
        "# references diagnostic_gates\n"
        "allow if { input.env_gate_diagnostic_sanctioned; input.env_gate_rationale_present }\n"
        'deny contains message if { not input.env_gate_diagnostic_sanctioned; message := "forbidden" }\n'
    )


def rules_for(checker) -> set[str]:
    gates = checker.scan_gates()
    data = checker.load_baseline_data()
    diagnostic_gates = checker.load_diagnostic_gates(data)
    findings = (
        checker.policy_findings()
        + checker.deprecation_findings(data)
        + checker.scan_findings(gates, diagnostic_gates)
    )
    return {f.rule for f in findings}


BRAND = "ER_EFFECTS_BRAND_NEW"
KEY = f"{BRAND}@src/new_gate.rs"
GATE_SRC = (
    "pub(crate) fn brand_new_gate() -> bool {\n"
    f'    matches!(std::env::var("{BRAND}").as_deref(), Ok("1"))\n'
    "}\n"
)


def main() -> int:
    if FIXTURE_ROOT.exists():
        shutil.rmtree(FIXTURE_ROOT)
    checker = load_checker()
    configure_module_paths(checker)

    write(".auto/env_gate_comment_policy.rego", valid_policy())
    write("src/new_gate.rs", GATE_SRC)

    # 1. Any env gate not in diagnostic_gates is FORBIDDEN.
    write_baseline({})
    assert rules_for(checker) == {"env-gate-forbidden"}, rules_for(checker)

    # 2. Adding its exact ENV_VAR@path key to diagnostic_gates (with a rationale) permits it.
    write_baseline({KEY: "passive debug log only; no game behavior change."})
    assert rules_for(checker) == set(), rules_for(checker)

    # 3. A diagnostic_gates entry with an EMPTY rationale is NOT enough -> still forbidden.
    write_baseline({KEY: "   "})
    assert rules_for(checker) == {"env-gate-forbidden"}, rules_for(checker)

    # 4. A key for a DIFFERENT location does not rescue this gate.
    write_baseline({f"{BRAND}@src/other.rs": "unrelated."})
    assert rules_for(checker) == {"env-gate-forbidden"}, rules_for(checker)

    # 5. Re-populating a DEPRECATED behavioral allowlist is itself a failure.
    write_baseline(
        {KEY: "passive debug log only."},
        deprecated={"sanctioned_env_vars": [BRAND]},
    )
    assert rules_for(checker) == {"env-gate-allowlist-not-deprecated"}, rules_for(checker)

    write_baseline(
        {KEY: "passive debug log only."},
        deprecated={"baseline": [KEY], "sanctioned_env_gate_locations": [KEY]},
    )
    assert rules_for(checker) == {"env-gate-allowlist-not-deprecated"}, rules_for(checker)

    # 6. Missing / drifted policy file is itself a finding.
    write_baseline({KEY: "passive debug log only."})
    (FIXTURE_ROOT / ".auto" / "env_gate_comment_policy.rego").unlink()
    assert rules_for(checker) == {"missing-env-gate-policy"}, rules_for(checker)

    write(".auto/env_gate_comment_policy.rego", "package auto.env_gate_comment\n")  # drifted
    assert rules_for(checker) == {"env-gate-policy-drift"}, rules_for(checker)

    print("env-gate policy regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
