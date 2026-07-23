#!/usr/bin/env python3
"""Validate the repo-local run_experiment contract.

This cannot intercept Pi's tool call by itself; it is the executable policy source
for agents and CI/measure gates. Agents must call run_experiment with the same
input this policy allows: command ./.auto/measure.sh and timeout_seconds <= 45.
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
POLICY = REPO_ROOT / ".auto" / "run_experiment_policy.rego"
TESTS = REPO_ROOT / ".auto" / "run_experiment_policy_test.rego"
MAX_TIMEOUT_SECONDS = 45
REQUIRED_SNIPPETS = (
    "package auto.run_experiment",
    "max_timeout_seconds := 45",
    "input.command == \"./.auto/measure.sh\"",
    "input.timeout_seconds <= max_timeout_seconds",
    "input.checks_timeout_seconds <= max_timeout_seconds",
)


def fail(message: str) -> int:
    print(message, file=sys.stderr)
    return 1


def opa_eval(input_payload: dict[str, object], query: str) -> str:
    run = subprocess.run(
        [
            "opa",
            "eval",
            "--format",
            "raw",
            "-d",
            str(POLICY),
            "-I",
            query,
        ],
        input=json.dumps(input_payload),
        text=True,
        capture_output=True,
        timeout=10,
        check=False,
    )
    if run.returncode != 0:
        raise RuntimeError(run.stderr.strip() or run.stdout.strip())
    return run.stdout.strip()


def main() -> int:
    if not POLICY.exists() or not TESTS.exists():
        return fail("missing .auto/run_experiment_policy.rego or test file")
    text = POLICY.read_text(encoding="utf-8", errors="replace")
    missing = [snippet for snippet in REQUIRED_SNIPPETS if snippet not in text]
    if missing:
        return fail("run_experiment policy missing required snippets: " + ", ".join(missing))
    if shutil.which("opa") is None:
        return fail("missing required command: opa")
    for cmd in (["opa", "check", str(POLICY), str(TESTS)], ["opa", "test", str(POLICY), str(TESTS)]):
        run = subprocess.run(cmd, text=True, capture_output=True, timeout=30, check=False)
        if run.returncode != 0:
            return fail((run.stderr or run.stdout).strip())
    allowed = opa_eval(
        {"command": "./.auto/measure.sh", "timeout_seconds": MAX_TIMEOUT_SECONDS, "checks_timeout_seconds": MAX_TIMEOUT_SECONDS},
        "data.auto.run_experiment.allow",
    )
    denied = opa_eval(
        {"command": "./.auto/measure.sh", "timeout_seconds": MAX_TIMEOUT_SECONDS + 1, "checks_timeout_seconds": MAX_TIMEOUT_SECONDS},
        "data.auto.run_experiment.allow",
    )
    if allowed != "true" or denied != "false":
        return fail(f"unexpected run_experiment policy result: allowed={allowed!r} denied={denied!r}")
    print("run_experiment contract ok: command=./.auto/measure.sh timeout_seconds<=45")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
