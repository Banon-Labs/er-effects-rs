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
    tool_name: str = "Bash"


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
        "tool_name": case.tool_name,
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
            "allow-local-shell-vars-before-commands-with-coarse-ast",
            "run_id=$(date +%Y%m%d-%H%M%S)\n"
            "log_dir=\"target/runtime-probe/profile-portrait-capture-measure-$run_id\"\n"
            "mkdir -p \"$log_dir\"\n"
            "touch .auto/run_profile_portrait_capture_once\n"
            "nohup ./.auto/measure.sh > \"$log_dir/measure.out\" 2> \"$log_dir/measure.err\" &\n"
            "pid=$!\n"
            "echo \"$pid\" > \"$log_dir/measure.pid\"\n"
            "echo \"$log_dir\"",
            True,
            None,
            {
                "command_ast": {
                    "parse_ok": True,
                    "statements": [
                        {
                            "env_setting": True,
                            "command_name": "mkdir",
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
        PolicyCase(
            "deny-steam-applaunch-elden-ring",
            "steam -applaunch 1245620",
            False,
            "blocked this Elden Ring launch command",
        ),
        PolicyCase(
            "deny-steam-rungameid-elden-ring",
            "steam steam://rungameid/1245620",
            False,
            "blocked this Elden Ring launch command",
        ),
        PolicyCase(
            "deny-xdg-open-steam-run-elden-ring",
            "xdg-open steam://run/1245620",
            False,
            "blocked this Elden Ring launch command",
        ),
        PolicyCase(
            "allow-pgrep-start-protected-detection",
            "pgrep -x start_protected_game.exe",
            True,
        ),
        PolicyCase(
            "allow-runtime-preflight-pgrep-start-protected-detection",
            "if pgrep -x eldenring.exe >/dev/null || pgrep -x start_protected_game.exe >/dev/null; then echo 'already running'; exit 2; fi",
            True,
        ),
        PolicyCase(
            "deny-pgrep-then-proton-start-protected-launch",
            "pgrep -x start_protected_game.exe >/dev/null; proton run /tmp/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-proton-start-protected-game",
            "proton run /tmp/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-wine-start-protected-game",
            "wine /opt/er/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-dot-slash-start-protected-game",
            "./start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        # Read-only /proc comm scans may NAME the EAC launcher inside quoted
        # string literals (2026-07-05 false positive: the sanctioned no-pgrep
        # process-detection heredoc was denied by the raw marker fallback).
        PolicyCase(
            "allow-proc-comm-scan-heredoc-naming-eac-launcher",
            "python3 - <<'PY'\n"
            "import glob\n"
            "names = ('steam', 'eldenring.exe', 'start_protected_game.exe')\n"
            "found = {n: False for n in names}\n"
            "for path in glob.glob('/proc/[0-9]*/comm'):\n"
            "    try:\n"
            "        comm = open(path).read().strip()\n"
            "    except OSError:\n"
            "        continue\n"
            "    if comm in names:\n"
            "        found[comm] = True\n"
            "for n in names:\n"
            "    print(n, 'up' if found[n] else 'down')\n"
            "PY",
            True,
            extra_tool_input={
                "description": "Report Steam/eldenring/EAC launcher process state from /proc"
            },
        ),
        PolicyCase(
            "allow-proc-comm-scan-python-c-naming-eac-launcher",
            "python3 -c 'import glob; print(any(open(p).read().strip() =="
            ' "start_protected_game.exe" for p in glob.glob("/proc/[0-9]*/comm")))\'',
            True,
        ),
        # ... but the /proc mention must never become a launch bypass.
        PolicyCase(
            "deny-proc-scan-heredoc-with-subprocess-launch",
            "python3 - <<'PY'\n"
            "import subprocess\n"
            "print(open('/proc/1/comm').read())\n"
            "subprocess.run(['wine', 'start_protected_game.exe'])\n"
            "PY",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-proc-scan-heredoc-trailing-quoted-launch",
            "python3 - <<'PY'\n"
            "print(open('/proc/1/comm').read())\n"
            "PY\n"
            "setsid '/opt/er/start_protected_game.exe'",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-direct-start-protected-game",
            "/tmp/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-ersc-dll-copy-bundle",
            "cp -f /tmp/ersc.dll target/release/ersc.dll",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        PolicyCase(
            "allow-quoted-forbidden-launch-note",
            "echo 'do not run steam -applaunch 1245620'",
            True,
        ),
        # bd only records text; forbidden-form MENTIONS in quoted issue text
        # must not deny even when generic executable marker words ("bash",
        # "python", ...) appear in the prose (2026-07-04 false positive).
        PolicyCase(
            "allow-bd-create-mentioning-eac-launcher",
            '/home/banon/.local/bin/bd create "me3 launch path" -d "me3 Linux'
            " launch via bash scripts must not use forbidden forms (steam"
            ' -applaunch / steam:// URLs / start_protected_game.exe)." -t task -p 1',
            True,
        ),
        PolicyCase(
            "allow-bd-remember-mentioning-eac-launcher",
            "/home/banon/.local/bin/bd remember --key k 'never launch"
            " start_protected_game.exe from bash or python wrappers'",
            True,
        ),
        PolicyCase(
            "allow-bd-create-mentioning-steam-applaunch-appid",
            '/home/banon/.local/bin/bd create "launch policy" -d "steam'
            ' -applaunch 1245620 is a forbidden form; drive it from bash probes" -t task',
            True,
        ),
        # The bd exemption must not leak to chained or indirected launches.
        PolicyCase(
            "deny-bd-chained-proton-start-protected",
            '/home/banon/.local/bin/bd create "note" -d "text" && proton run'
            " /tmp/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-bd-chained-python-c-start-protected",
            '/home/banon/.local/bin/bd create "note" -d "text"; python3 -c'
            " 'import subprocess; subprocess.run([\"proton\",\"run\",\"start_protected_game.exe\"])'",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-bash-c-start-protected-game",
            "bash -c '/opt/er/start_protected_game.exe'",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-ctx-execute-python-steam-applaunch",
            "",
            False,
            "blocked this Elden Ring launch command",
            {"language": "python", "code": "import subprocess; subprocess.run(['steam','-applaunch','1245620'])"},
            tool_name="ctx_execute",
        ),
        PolicyCase(
            "deny-heredoc-python-start-protected",
            "python3 - <<'PY'\nimport subprocess\nsubprocess.run(['proton','run','start_protected_game.exe'])\nPY",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-ctx-execute-python-ersc-copy",
            "",
            False,
            "blocked this Seamless Co-op DLL bundling command",
            {"language": "python", "code": "import shutil; shutil.copy2('SeamlessCoop/ersc.dll', 'target/release/ersc.dll')"},
            tool_name="ctx_execute",
        ),
        PolicyCase(
            "allow-mutating-git-branch-delete",
            "git branch -d merged-topic",
            True,
        ),
        # Global GitHub attribution guard: a --body "$(cat <<'EOF'...)" command
        # substitution cannot be expanded by the gh_context signal, which falls
        # back to matching the raw command text (2026-07-05 false positive:
        # footer present in the heredoc was denied). Footer present -> allow.
        PolicyCase(
            "allow-gh-pr-edit-heredoc-substitution-body-with-footer",
            'gh pr edit 19 --repo Banon-Labs/er-effects-rs --body "$(cat <<\'EOF\'\n'
            "Body text describing the change.\n\n"
            "\U0001f916 Written by Claude Fable 5, authorized by @chozandrias76\n"
            'EOF\n)"',
            True,
        ),
        # ... and the same form WITHOUT the footer must still deny (the raw
        # command fallback must not weaken the guard).
        PolicyCase(
            "deny-gh-pr-edit-heredoc-substitution-body-without-footer",
            'gh pr edit 19 --repo Banon-Labs/er-effects-rs --body "$(cat <<\'EOF\'\n'
            "Body text without attribution.\n"
            'EOF\n)"',
            False,
            "attribution footer",
        ),
        # A real rtk invocation with a native word in a quoted arg stays allowed.
        PolicyCase(
            "allow-rtk-grep-quoted-find",
            'rtk grep "find"',
            True,
        ),
        # No authoring scripts into /tmp (artifacts to /tmp are fine).
        PolicyCase(
            "deny-write-script-into-tmp",
            "",
            False,
            "authoring a script into /tmp",
            {"file_path": "/tmp/ghidra_scripts/Foo.java", "content": "class Foo {}"},
            include_timeout=False,
            tool_name="Write",
        ),
        PolicyCase(
            "deny-edit-py-script-into-tmp",
            "",
            False,
            "authoring a script into /tmp",
            {"file_path": "/tmp/scratch/tool.py"},
            include_timeout=False,
            tool_name="Edit",
        ),
        PolicyCase(
            "allow-write-data-artifact-into-tmp",
            "",
            True,
            None,
            {"file_path": "/tmp/claude/dump_funcs.tsv", "content": "a\tb"},
            include_timeout=False,
            tool_name="Write",
        ),
        PolicyCase(
            "allow-write-script-into-repo",
            "",
            True,
            None,
            {"file_path": str(REPO_ROOT / "scripts" / "ghidra" / "Foo.java"), "content": "class Foo {}"},
            include_timeout=False,
            tool_name="Write",
        ),
        PolicyCase(
            "allow-write-log-into-tmp",
            "",
            True,
            None,
            {"file_path": "/tmp/run.log", "content": "ok"},
            include_timeout=False,
            tool_name="Write",
        ),
    ]
    for case in cases:
        run_case(case)
    print("cupcake policy regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
