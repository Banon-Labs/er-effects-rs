#!/usr/bin/env python3
"""Regression tests for scripts/check-env-gate-comments.py."""
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


def write_baseline(keys: list[str], sanctioned: list[str] | None = None) -> None:
    payload: dict[str, object] = {"baseline": keys}
    # Default the allowlist to the one env var used across these fixtures so the
    # frozen-allowlist hard gate does not spuriously fire in unrelated cases.
    payload["sanctioned_env_vars"] = (
        sanctioned if sanctioned is not None else ["ER_EFFECTS_BRAND_NEW"]
    )
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
        "deny contains message if { not input.env_var_sanctioned; message := \"unknown env var\" }\n"
        "allow if { input.env_var_sanctioned; input.has_rationale_comment == true }\n"
        "allow if { input.env_var_sanctioned; input.in_baseline == true }\n"
        "deny contains message if { input.env_var_sanctioned; not allow; message := \"ENV-GATE RATIONALE marker required\" }\n"
    )


def rules_for(checker) -> set[str]:
    gates = checker.scan_gates()
    baseline = checker.load_baseline()
    sanctioned = checker.load_sanctioned_env_vars()
    return {f.rule for f in checker.scan_findings(gates, baseline, sanctioned)}


def main() -> int:
    if FIXTURE_ROOT.exists():
        shutil.rmtree(FIXTURE_ROOT)
    checker = load_checker()
    configure_module_paths(checker)

    write(".auto/env_gate_comment_policy.rego", valid_policy())

    # 1. A NEW non-compliant gate (no comment above its fn, not baselined) -> flagged.
    write(
        "src/new_gate.rs",
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    write_baseline([])
    rules = rules_for(checker)
    assert rules == {"env-gate-missing-rationale"}, rules

    # 2. The SAME gate becomes compliant via the ENV-GATE RATIONALE marker -> passes.
    write(
        "src/new_gate.rs",
        "// ENV-GATE RATIONALE: this gate only flips a debug log, no save side effects.\n"
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    rules = rules_for(checker)
    assert rules == set(), rules

    # 2b. Compliant via a >=2-line /// doc comment (option 2), no marker -> passes.
    write(
        "src/new_gate.rs",
        "/// Enables the brand-new feature.\n"
        "/// Save-safe: reads memory only, never writes the .sl2.\n"
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    rules = rules_for(checker)
    assert rules == set(), rules

    # 2c. A single /// line is NOT enough (needs >= 2) -> flagged.
    write(
        "src/new_gate.rs",
        "/// Enables the brand-new feature.\n"
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    rules = rules_for(checker)
    assert rules == {"env-gate-missing-rationale"}, rules

    # 3. A non-compliant gate that IS in the baseline -> passes (ratchet).
    write_baseline(["ER_EFFECTS_BRAND_NEW@src/new_gate.rs"])
    write(
        "src/new_gate.rs",
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    rules = rules_for(checker)
    assert rules == set(), rules

    # 3b. A baselined gate that has SINCE gained a comment is reported as shrinkable
    #     (soft), but is NOT a hard finding.
    write(
        "src/new_gate.rs",
        "// ENV-GATE RATIONALE: documented now.\n"
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    gates = checker.scan_gates()
    baseline = checker.load_baseline()
    assert rules_for(checker) == set()
    assert checker.shrinkable_baseline_entries(gates, baseline) == [
        "ER_EFFECTS_BRAND_NEW@src/new_gate.rs"
    ]

    # 3c. FROZEN ALLOWLIST HARD GATE: an env var NOT in sanctioned_env_vars FAILS
    #     even WITH a valid rationale comment (and even if baselined). This is the
    #     core gap-closing test: a comment must NOT rescue an unknown env var.
    write_baseline([], sanctioned=["ER_EFFECTS_KNOWN"])
    write(
        "src/unknown_gate.rs",
        "// ENV-GATE RATIONALE: this gate only flips a debug log, no save side effects.\n"
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    rules = rules_for(checker)
    assert rules == {"env-gate-unknown-var"}, rules

    # 3d. A baseline entry must ALSO not rescue an unknown env var.
    write_baseline(
        ["ER_EFFECTS_BRAND_NEW@src/unknown_gate.rs"], sanctioned=["ER_EFFECTS_KNOWN"]
    )
    write(
        "src/unknown_gate.rs",
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    rules = rules_for(checker)
    assert rules == {"env-gate-unknown-var"}, rules

    # 3e. An ALLOWLISTED env var WITH a comment passes (the allowlist gates, the
    #     comment ratchet still applies on top).
    write_baseline([], sanctioned=["ER_EFFECTS_BRAND_NEW"])
    write(
        "src/unknown_gate.rs",
        "// ENV-GATE RATIONALE: this gate only flips a debug log, no save side effects.\n"
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    rules = rules_for(checker)
    assert rules == set(), rules

    # 3f. An ALLOWLISTED env var WITHOUT a comment (and not baselined) still fails
    #     the existing comment ratchet -- the allowlist did not weaken it.
    write(
        "src/unknown_gate.rs",
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    rules = rules_for(checker)
    assert rules == {"env-gate-missing-rationale"}, rules

    # Clean up the unknown-gate fixture so it does not leak into the policy tests below.
    (FIXTURE_ROOT / "src" / "unknown_gate.rs").unlink()
    write_baseline([])

    # 4. Missing / drifted policy file is itself a finding.
    write_baseline([])
    write(
        "src/new_gate.rs",
        "// ENV-GATE RATIONALE: documented.\n"
        "pub(crate) fn brand_new_gate() -> bool {\n"
        '    matches!(std::env::var("ER_EFFECTS_BRAND_NEW").as_deref(), Ok("1"))\n'
        "}\n",
    )
    (FIXTURE_ROOT / ".auto" / "env_gate_comment_policy.rego").unlink()
    rules = rules_for(checker)
    assert rules == {"missing-env-gate-policy"}, rules

    write(".auto/env_gate_comment_policy.rego", "package auto.env_gate_comment\n")  # drifted
    rules = rules_for(checker)
    assert rules == {"env-gate-policy-drift"}, rules

    print("env-gate comment policy regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
