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
    include_timeout: bool = True


DEFAULT_BASH_TIMEOUT_MS = 30000


def run_case(case: PolicyCase) -> None:
    tool_input: dict[str, object] = {"command": case.command}
    if case.include_timeout:
        tool_input["timeout"] = DEFAULT_BASH_TIMEOUT_MS
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
        timeout=30,
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
            "deny-missing-tool-timeout-field",
            "rtk ls",
            False,
            "missing Bash tool timeout parameter",
            include_timeout=False,
        ),
        PolicyCase(
            "deny-tool-timeout-too-large",
            "rtk ls",
            False,
            "no more than 30 seconds",
            {"timeout": 30001},
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
        # An `=` inside a quoted argument is not an env assignment.
        PolicyCase(
            "allow-equals-in-quoted-grep",
            'rtk grep -n "FOO=bar|PROTON=x" .auto/runtime_probe.sh',
            True,
        ),
        PolicyCase(
            "allow-equals-in-double-quoted-echo",
            'echo "PATH=/usr/bin works"',
            True,
        ),
        PolicyCase(
            "allow-equals-in-heredoc-body",
            "python3 - <<'PY'\nimport os\nFOO=os.getpid()\nPY",
            True,
        ),
        # Real inline env assignment with a quoted value must still be caught.
        PolicyCase(
            "deny-quoted-value-inline-env",
            'FOO="bar baz" ./scripts/check.sh',
            False,
            "named-env.env",
        ),
        PolicyCase(
            "deny-semicolon-split",
            "echo one; echo two",
            False,
            "Prefer splitting up each command split by ; into its own file",
        ),
        # Quoted semicolons are not command separators (no command_ast supplied
        # at runtime, so the quote-stripping fallback must handle these).
        PolicyCase(
            "allow-semicolon-in-double-quoted-commit",
            'git commit -m "fix a; fix b"',
            True,
        ),
        PolicyCase(
            "allow-semicolon-in-python-dash-c",
            'python3 -c "import sys; print(sys.version)"',
            True,
        ),
        PolicyCase(
            "allow-semicolon-in-single-quoted-arg",
            "bd remember --key k 'first clause; second clause'",
            True,
        ),
        PolicyCase(
            "deny-real-split-between-quoted-args",
            'echo "a"; echo "b"',
            False,
            "Prefer splitting up each command split by ; into its own file",
        ),
        # Backslash-escaped quotes inside a quoted message must not desync the
        # quote-stripping (a commit message that quotes example commands).
        PolicyCase(
            "allow-escaped-quotes-with-semicolons",
            'git commit -m "guard ignores quotes; e.g. python3 -c \\"a; b\\" works"',
            True,
        ),
        # Heredoc bodies are interpreter input; their semicolons are not shell
        # separators (e.g. python statement separators inside python3 - <<'PY').
        PolicyCase(
            "allow-heredoc-body-with-semicolons",
            "python3 - <<'PY'\nimport os; print(os.getpid()); print(1)\nPY",
            True,
        ),
        PolicyCase(
            "deny-real-split-before-heredoc",
            "echo one; python3 - <<'PY'\nx = 1\nPY",
            False,
            "Prefer splitting up each command split by ; into its own file",
        ),
        # RTK read-only guard: native tool words inside quoted arguments or
        # heredoc bodies are not native invocations and must be allowed.
        PolicyCase(
            "allow-rtk-words-in-quoted-arg",
            'bd remember --key k "please find and grep the list"',
            True,
        ),
        PolicyCase(
            "allow-rtk-words-in-commit-message",
            'git commit -m "find and ls the files"',
            True,
        ),
        PolicyCase(
            "allow-rtk-words-in-heredoc-body",
            "python3 - <<'PY'\n# find grep ls git status in body\nprint('find grep ls')\nPY",
            True,
        ),
        # Real native invocations must still be denied.
        PolicyCase(
            "deny-native-grep",
            "grep -n foo src",
            False,
            "RTK path",
        ),
        PolicyCase(
            "deny-native-find",
            "find . -name x",
            False,
            "RTK path",
        ),
        PolicyCase(
            "deny-native-ls-target",
            "ls target",
            False,
            "RTK path",
        ),
        PolicyCase(
            "deny-native-git-status",
            "git status",
            False,
            "git inspection",
        ),
        # A real rtk invocation with a native word in a quoted arg stays allowed.
        PolicyCase(
            "allow-rtk-grep-quoted-find",
            'rtk grep "find"',
            True,
        ),
    ]
    for case in cases:
        run_case(case)
    print("cupcake policy regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
