#!/usr/bin/env python3
"""Regression tests for repo-local Cupcake policy decisions."""
from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


@dataclass(frozen=True)
class PolicyCase:
    name: str
    command: str
    should_allow: bool
    expected_text: str | None = None
    extra_tool_input: dict[str, object] | None = None
    extra_event: dict[str, object] | None = None


def run_case(case: PolicyCase) -> None:
    tool_input: dict[str, object] = {"command": case.command}
    if case.extra_tool_input:
        tool_input.update(case.extra_tool_input)
    event = {
        "session_id": f"cupcake-policy-regression-{case.name}",
        "transcript_path": f"/tmp/cupcake-policy-regression-{case.name}.jsonl",
        "cwd": str(REPO_ROOT),
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": tool_input,
    }
    if case.extra_event:
        event.update(case.extra_event)
    result = subprocess.run(
        ["cupcake", "eval", "--harness", "claude", "--strict", "--log-level", "error"],
        cwd=REPO_ROOT,
        input=json.dumps(event),
        text=True,
        capture_output=True,
        check=False,
    )
    output = result.stdout + result.stderr
    allowed = result.returncode == 0
    if allowed != case.should_allow:
        raise AssertionError(
            f"{case.name}: expected allow={case.should_allow}, got returncode={result.returncode}\n{output}"
        )
    if case.expected_text and case.expected_text not in output:
        raise AssertionError(f"{case.name}: missing {case.expected_text!r}\n{output}")


def main() -> int:
    cases = [
        PolicyCase("allow-rtk", "rtk ls", True),
        PolicyCase(
            "deny-tool-timeout-field",
            "rtk ls",
            False,
            "Bash tool timeout parameter",
            {"timeout": 1000},
        ),
        PolicyCase("deny-shell-sleep", "sleep 1", False, "shell sleep command"),
        PolicyCase("deny-native-ls", "ls target", False, "RTK path"),
        PolicyCase(
            "deny-inline-env",
            "FOO=bar ./scripts/check.sh",
            False,
            "named-env.env",
        ),
        PolicyCase(
            "deny-ast-inline-env",
            "FOO=bar ./scripts/check.sh",
            False,
            "named-env.env",
            {
                "command_ast": {
                    "parse_ok": True,
                    "statements": [
                        {
                            "env_setting": True,
                            "command_name": "./scripts/check.sh",
                        }
                    ],
                }
            },
        ),
        PolicyCase(
            "allow-shell-variable-bookkeeping",
            "./scripts/check-no-timeouts.py\nrc=$?\necho \"$rc\"",
            True,
        ),
        PolicyCase(
            "allow-ast-shell-variable-bookkeeping",
            "rc=$?",
            True,
            None,
            {
                "command_ast": {
                    "parse_ok": True,
                    "statements": [
                        {
                            "env_setting": True,
                            "command_name": None,
                        }
                    ],
                }
            },
        ),
        PolicyCase(
            "allow-flattened-shell-variable-bookkeeping",
            "set +e false rc=$? set -e echo \"$rc\"",
            True,
        ),
        PolicyCase(
            "allow-python-heredoc-with-overbroad-affected-root",
            "python3 - <<'PY'\nprint(1)\nPY",
            True,
            extra_event={"affected_parent_directories": ["/"]},
        ),
        PolicyCase(
            "allow-repo-cupcake-system-path-not-absolute-system",
            "opa check .cupcake/system .cupcake/policies/claude/builtins/protected_paths.rego",
            True,
        ),
        PolicyCase(
            "deny-destructive-parent-root",
            "rm -rf /",
            False,
            "would be affected by operation on /",
            extra_event={"affected_parent_directories": ["/"]},
        ),
        PolicyCase(
            "deny-semicolon-split",
            "echo one; echo two",
            False,
            "Prefer splitting up each command split by ; into its own file",
        ),
    ]
    for case in cases:
        run_case(case)
    print("cupcake policy regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
